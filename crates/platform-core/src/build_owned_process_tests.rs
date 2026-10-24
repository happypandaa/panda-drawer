// Actual candidate/npm pipeline plus controlled reader completion/panic.
// The reader barrier does not simulate a physical device or power loss.
use super::*;

fn candidate_path(project: &ProjectHandle, id: &str) -> PathBuf {
    let root = project.root.join(".platform/builds").join(id);
    let record: Value = serde_json::from_slice(&fs::read(root.join("candidate-v1.json")).unwrap()).unwrap();
    root.join(record["path"].as_str().unwrap())
}

async fn reader_fixture(manager: &BuildManager, panic: bool) -> (tokio::sync::oneshot::Receiver<()>, Option<tokio::sync::oneshot::Sender<()>>) {
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    *manager.reader_fixture.lock().unwrap() = Some(BuildReaderFixture {
        operation: "run".into(), entered, release: (!panic).then_some(blocked), panic,
    });
    (ready, (!panic).then_some(release))
}

#[tokio::test]
async fn owned_web_build_pending_reader_holds_candidate_phase_gate_and_task_lease_until_rollback() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable pending npm reader").await;
    fixture(&project, BUILD);
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let first = manager.submit(&project.manifest.id).await.unwrap();
    assert_eq!(wait(&manager, &first.task_id).await.status, TaskStatus::Succeeded);
    assert!(store.processes().snapshot().iter().any(|record| record.component == "node-environment/version-probe"), "build version checks use the actual store registry");
    let previous = fs::read(project.root.join("dist/index.html")).unwrap();
    let database = fs::read(project.root.join("data/main.sqlite")).unwrap();
    let attachment = project.root.join("data/attachments/retained.txt");
    fs::write(&attachment, "中文附件\n完整内容\n").unwrap();
    let bad_output = format!("{BUILD}\nrequire('fs').writeFileSync({},'pending writer changed output');", serde_json::to_string(&project.root.join("dist/index.html")).unwrap());
    fs::write(project.root.join("app/build.cjs"), bad_output).unwrap();
    let (entered, release) = reader_fixture(&manager, false).await;
    let task = manager.submit(&project.manifest.id).await.unwrap();
    entered.await.unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        while !store.processes().snapshot().iter().any(|record| record.component == "web-build/npm" && record.phase == ProcessPhase::CleanupPending) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(manager.task(&task.task_id).unwrap().status, TaskStatus::Running);
    let candidate = candidate_path(&project, &task.task_id);
    assert!(candidate.is_dir());
    assert!(project.operation_gate.try_write().is_err());
    assert_eq!(store.reserve_development_task(&project.manifest.id, "late-task").err().unwrap().code, "PROJECT_BUSY");
    assert_ne!(fs::read(project.root.join("dist/index.html")).unwrap(), previous, "rollback did not race the still-owned log writer");
    release.unwrap().send(()).unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Failed);
    let error = done.error.unwrap();
    assert_eq!(error.details["processCauseCode"], "PROCESS_CLEANUP_PENDING");
    assert_eq!(error.details["processResourceClosed"], true);
    assert_eq!(fs::read(project.root.join("dist/index.html")).unwrap(), previous);
    assert!(!candidate.exists());
    assert!(project.operation_gate.try_write().is_ok());
    drop(store.reserve_development_task(&project.manifest.id, "new-task").unwrap());
    assert_eq!(fs::read(project.root.join("data/main.sqlite")).unwrap(), database);
    assert_eq!(fs::read_to_string(attachment).unwrap(), "中文附件\n完整内容\n");
    manager.shutdown().await;
    assert!(store.processes().shutdown().await.is_empty());
}

#[tokio::test]
async fn owned_web_build_reader_panic_is_unknown_keeps_guards_and_never_rolls_back() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable unknown npm reader").await;
    fixture(&project, BUILD);
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let first = manager.submit(&project.manifest.id).await.unwrap();
    assert_eq!(wait(&manager, &first.task_id).await.status, TaskStatus::Succeeded);
    let previous = fs::read(project.root.join("dist/index.html")).unwrap();
    fs::write(project.root.join("app/build.cjs"), format!("{BUILD}\nrequire('fs').writeFileSync({},'unknown writer evidence');", serde_json::to_string(&project.root.join("dist/index.html")).unwrap())).unwrap();
    let (entered, _) = reader_fixture(&manager, true).await;
    let task = manager.submit(&project.manifest.id).await.unwrap();
    entered.await.unwrap();
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Failed);
    assert_eq!(done.error.unwrap().details["processResourceClosed"], false);
    assert_eq!(fs::read(project.root.join("dist/index.html")).unwrap(), b"unknown writer evidence");
    let root = project.root.join(".platform/builds").join(&task.task_id);
    assert_eq!(fs::read(root.join("previous/index.html")).unwrap(), previous);
    assert!(candidate_path(&project, &task.task_id).is_dir());
    assert!(project.operation_gate.try_write().is_err());
    assert_eq!(store.reserve_development_task(&project.manifest.id, "late-task").err().unwrap().code, "PROJECT_BUSY");
    let issues = store.processes().shutdown().await;
    assert!(issues.iter().any(|issue| issue.code == "PROCESS_IO_JOIN_FAILED" && !issue.retryable));
    assert_eq!(store.processes().shutdown().await, issues);
    manager.shutdown().await;
}
