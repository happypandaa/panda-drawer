// Actual candidate/npm pipeline plus controlled reader completion/panic.
// The reader barrier does not simulate a physical device or power loss.
use super::*;
use std::os::unix::fs::PermissionsExt;

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

struct GitPipeRelease(PathBuf);
impl Drop for GitPipeRelease {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"release disposable Git reader");
    }
}
async fn wait_git_marker(path: &Path) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Git phase did not reach the owned inherited-pipe marker");
}
async fn finish_git_io(store: &ProjectStore) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let processes = store.processes();
            if processes.shutdown().await.is_empty()
                && processes
                    .snapshot()
                    .iter()
                    .all(|record| record.resource_closed())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("released Git reader did not actually join");
}
fn configure_pending_git(
    paths: &RuntimePaths,
    project: &ProjectHandle,
    official_git: Option<&Path>,
    on_capture: bool,
) -> (PathBuf, PathBuf, GitPipeRelease) {
    let tools = paths.data_root().join("disposable-web-git-tools");
    ensure_dir(&tools).unwrap();
    let marker = tools.join("inherited-reader-entered");
    let release = tools.join("release-inherited-reader");
    let build_done = tools.join("npm-completed");
    let child = "import os,sys,time\nopen(sys.argv[1],'w').write('owned pipe barrier')\nend=time.monotonic()+20\nwhile not os.path.exists(sys.argv[2]) and time.monotonic()<end: time.sleep(.01)\n";
    let command = tools.join("git-fixture.py");
    let body = format!(
        r#"#!/usr/bin/python3
import os,subprocess,sys,time
real_git={real_git}
args=sys.argv[1:]
if real_git:
    result=subprocess.run([real_git]+args)
    if result.returncode: sys.exit(result.returncode)
else:
    if '--version' in args:
        print('git version 2.56.0');sys.exit(0)
    if 'config' in args: sys.exit(1)
if 'ls-files' in args and ({capture} or os.path.exists({build_done})):
    subprocess.Popen(['/usr/bin/python3','-c',{child},{marker},{release}],start_new_session=True)
    while not os.path.isfile({marker}): time.sleep(.005)
    # This leader exits normally only after the self-expiring child is in its
    # separate session. No test PID/PGID is signalled. The registry must retain
    # actual pipe ownership even after the ordinary Git group has exited.
    os._exit(0)
sys.exit(0)
"#,
        real_git = serde_json::to_string(
            &official_git
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default()
        )
        .unwrap(),
        capture = if on_capture { "True" } else { "False" },
        build_done = serde_json::to_string(&build_done).unwrap(),
        child = serde_json::to_string(child).unwrap(),
        marker = serde_json::to_string(&marker).unwrap(),
        release = serde_json::to_string(&release).unwrap(),
    );
    fs::write(&command, body).unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        paths.data_root().join("git-toolchain.json"),
        serde_json::to_vec(&json!({"schemaVersion":1,"gitPath":command})).unwrap(),
    )
    .unwrap();
    if let Some(git) = official_git {
        // This fixture retains each real Child through wait_with_output. It only
        // creates a new TempDir repository and never contacts a remote.
        let home = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "--initial-branch=main"],
            vec!["add", "app", "docs"],
            vec!["commit", "-m", "disposable web build source"],
        ] {
            let output = std::process::Command::new(git)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Disposable build fixture")
                .env("GIT_COMMITTER_NAME", "Disposable build fixture")
                .env("GIT_AUTHOR_EMAIL", "test@localhost")
                .env("GIT_COMMITTER_EMAIL", "test@localhost")
                .current_dir(&project.root)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "commit.gpgSign=false",
                ])
                .args(&args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    } else {
        let git = project.root.join(".git");
        ensure_dir(&git.join("objects")).unwrap();
        ensure_dir(&git.join("refs")).unwrap();
        fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(
            git.join("config"),
            b"[core]\nrepositoryformatversion = 0\nbare = false\n",
        )
        .unwrap();
    }
    (marker, build_done, GitPipeRelease(release))
}

#[tokio::test]
async fn owned_web_build_git_capture_pending_retains_actual_project_lease_and_gate() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable capture-pending Git build").await;
    fixture(&project, BUILD);
    ensure_dir(&project.root.join("dist")).unwrap();
    fs::write(
        project.root.join("dist/index.html"),
        b"existing page before Git capture",
    )
    .unwrap();
    let before = fs::read(project.root.join("dist/index.html")).unwrap();
    let (marker, _, release) = configure_pending_git(&paths, &project, None, true);
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    wait_git_marker(&marker).await;
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Failed, "{done:?}");
    let error = done.error.unwrap();
    assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
    assert_eq!(error.details["processResourceClosed"], false);
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert!(store
        .processes()
        .snapshot()
        .iter()
        .any(|record| record.component == "project-git" && !record.io_joined));
    assert!(project.operation_gate.try_write().is_err());
    assert_eq!(
        store
            .reserve_development_task(&project.manifest.id, "must-stay-busy")
            .err()
            .unwrap()
            .code,
        "PROJECT_BUSY"
    );
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        before
    );
    assert!(
        !project
            .root
            .join(".platform/builds")
            .join(&task.task_id)
            .exists(),
        "capture precedes candidate creation"
    );
    drop(release);
    finish_git_io(&store).await;
    assert!(project.operation_gate.try_write().is_ok());
    drop(
        store
            .reserve_development_task(&project.manifest.id, "after-real-join")
            .unwrap(),
    );
    assert!(manager.shutdown().await.is_empty());
    assert!(store.shutdown().await.is_empty());
}

async fn recheck_pending_pipeline(official_git: Option<PathBuf>) {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable recheck-pending Git build").await;
    fixture(&project, BUILD);
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let first = manager.submit(&project.manifest.id).await.unwrap();
    assert_eq!(
        wait(&manager, &first.task_id).await.status,
        TaskStatus::Succeeded
    );
    let previous = fs::read(project.root.join("dist/index.html")).unwrap();
    let provenance = fs::read(provenance_path(
        paths.workspace_root(),
        &project.manifest.id,
    ))
    .unwrap();
    let database = fs::read(project.root.join("data/main.sqlite")).unwrap();
    let attachment = project.root.join("data/attachments/retained.txt");
    fs::write(&attachment, b"unmodified attachment evidence\n").unwrap();
    let (marker, build_done, release) =
        configure_pending_git(&paths, &project, official_git.as_deref(), false);
    fs::write(project.root.join("app/build.cjs"),format!("{BUILD}\nrequire('fs').writeFileSync({},'recheck-pending output evidence');require('fs').writeFileSync({},'npm truly finished');",serde_json::to_string(&project.root.join("dist/index.html")).unwrap(),serde_json::to_string(&build_done).unwrap())).unwrap();
    let task = manager.submit(&project.manifest.id).await.unwrap();
    wait_git_marker(&marker).await;
    let done = wait(&manager, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Failed, "{done:?}");
    let error = done.error.unwrap();
    assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
    assert_eq!(error.details["processResourceClosed"], false);
    assert_eq!(error.details["processClosed"], false);
    assert_eq!(error.details["operationOutcome"], "unknown");
    let candidate = candidate_path(&project, &task.task_id);
    let task_root = project.root.join(".platform/builds").join(&task.task_id);
    assert!(candidate.is_dir());
    assert!(
        task_root.join("dist/index.html").is_file(),
        "actual npm candidate output exists"
    );
    assert_eq!(
        fs::read(task_root.join("previous/index.html")).unwrap(),
        previous
    );
    assert!(
        !task_root.join("restore").exists(),
        "pending Git must not enter rollback"
    );
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        b"recheck-pending output evidence"
    );
    assert_eq!(
        fs::read(provenance_path(
            paths.workspace_root(),
            &project.manifest.id
        ))
        .unwrap(),
        provenance,
        "failed recheck must not publish a new build"
    );
    assert!(project.operation_gate.try_write().is_err());
    assert_eq!(
        store
            .reserve_development_task(&project.manifest.id, "must-stay-busy")
            .err()
            .unwrap()
            .code,
        "PROJECT_BUSY"
    );
    let records = store.processes().snapshot();
    assert!(records
        .iter()
        .any(|record| record.component == "web-build/npm" && record.phase == ProcessPhase::Closed));
    assert!(records
        .iter()
        .any(|record| record.component == "project-git"
            && record.phase == ProcessPhase::CleanupPending
            && !record.io_joined));
    assert!(store
        .processes()
        .shutdown()
        .await
        .iter()
        .any(|issue| issue.code == "PROCESS_CLEANUP_PENDING"));
    assert!(candidate.is_dir());
    assert!(project.operation_gate.try_write().is_err());
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        b"recheck-pending output evidence"
    );
    drop(release);
    finish_git_io(&store).await;
    assert!(
        !candidate.exists(),
        "candidate cleanup waits for actual Git pipe/session join"
    );
    assert!(project.operation_gate.try_write().is_ok());
    drop(
        store
            .reserve_development_task(&project.manifest.id, "after-real-join")
            .unwrap(),
    );
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        b"recheck-pending output evidence",
        "a failed task does not start a later implicit rollback"
    );
    assert_eq!(
        fs::read(task_root.join("previous/index.html")).unwrap(),
        previous
    );
    let original_receipt = manager.task(&task.task_id).unwrap();
    assert_eq!(original_receipt.status, TaskStatus::Failed);
    let original_error = original_receipt.error.unwrap();
    assert_eq!(original_error.details["operationOutcome"], "unknown");
    assert_eq!(original_error.details["processResourceClosed"], false,
        "the original task receipt is not rewritten when later registry cleanup closes");
    assert_eq!(
        fs::read(project.root.join("data/main.sqlite")).unwrap(),
        database
    );
    assert_eq!(
        fs::read(attachment).unwrap(),
        b"unmodified attachment evidence\n"
    );
    assert!(manager.shutdown().await.is_empty());
    assert!(store.shutdown().await.is_empty());
}

#[tokio::test]
async fn owned_web_build_git_recheck_pending_never_rolls_back_and_retains_candidate_until_real_join(
) {
    recheck_pending_pipeline(None).await;
}

#[tokio::test]
#[ignore = "requires LOCAL_PROJECTS_TEST_GIT_ROOT disposable official Git and local Node/npm"]
async fn official_git_web_build_recheck_pending_never_rolls_back_and_retains_actual_scope() {
    let root = PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    assert!(root.starts_with(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.test-artifacts")
            .canonicalize()
            .unwrap()
    ));
    recheck_pending_pipeline(Some(root.join("bin/git"))).await;
}
