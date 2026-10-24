// Actual install journals/files/locked public log, with a controlled reader tail.
use super::*;
use crate::{node_development_build::process_scope::NodeReaderFault, owned_process::ProcessPhase};

async fn exercise(panic: bool) {
    let fixture = Arc::new(Fixture::new("printf 'owned disposable npm output\\n'"));
    let registry = fixture.store.processes();
    let task = fixture.tasks.create_workspace_operation("nodeInstallPrepare").unwrap();
    let source = fixture.source();
    let key = format!("source:{}", source.source_id);
    let lease = Arc::new(fixture.store.reserve_task(&key, &task.task_id).unwrap());
    let before = fs::read(fixture.source.join("local-app.json")).unwrap();
    let user_marker = fixture.paths.data_root().join("disposable-retained-data");
    fs::write(&user_marker, "中文多行数据\n完整附件\n").unwrap();
    let late_writer = fixture.paths.data_root().join("disposable-owned-reader-finished");
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let scope = NodeProcessScope::new(registry.clone(), Arc::new((lease, fixture.clone())))
        .with_reader_fault(NodeReaderFault { component:"node-install/npm".into(), step:"ci".into(), entered, release:(!panic).then_some(blocked), panic, writer:Some(late_writer.clone()) });
    let (stop_sender, cancel_sender, control) = live_control();
    let work = { let fixture=fixture.clone(); let id=task.task_id.clone(); tokio::spawn(async move { fixture.installer.prepare_with_scope(&id, source, InstallTargetSnapshot::fresh(), fixture.tools.clone(), control, &fixture.tasks, scope).await }) };
    ready.await.unwrap();
    if !panic {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !registry.snapshot().iter().any(|record| record.component == "node-install/npm" && record.phase == ProcessPhase::CleanupPending) { tokio::time::sleep(Duration::from_millis(10)).await; }
        }).await.unwrap();
        assert!(!work.is_finished());
        assert!(!fixture.tasks.task(&task.task_id).unwrap().status.is_final());
    }
    assert!(fixture.store.reserve_task(&key, "late-task").is_err());
    let operation = fs::read_dir(fixture.installer.operations()).unwrap().next().unwrap().unwrap().path();
    assert!(operation.join("program/local-app.json").is_file());
    if !panic { release.send(()).unwrap(); }
    let error = work.await.unwrap().unwrap_err();
    assert_eq!(error.details["processResourceClosed"], !panic);
    let journal: Value = serde_json::from_slice(&fs::read(operation.join("install.json")).unwrap()).unwrap();
    assert_eq!(journal["state"], "failed");
    assert!(journal["prepared"].is_null());
    assert!(fixture.tasks.task(&task.task_id).unwrap().status.is_final());
    assert_eq!(fs::read(fixture.source.join("local-app.json")).unwrap(), before);
    assert_eq!(fs::read_to_string(user_marker).unwrap(), "中文多行数据\n完整附件\n");
    if panic {
        assert!(!late_writer.exists());
        assert!(fixture.store.reserve_task(&key, "late-task").is_err());
        assert!(registry.shutdown().await.iter().any(|issue| issue.code == "PROCESS_IO_JOIN_FAILED"));
    } else {
        assert_eq!(fs::read(late_writer).unwrap(), b"actual owned Node writer completed\n");
        drop(fixture.store.reserve_task(&key, "after-real-join").unwrap());
        assert!(registry.shutdown().await.is_empty());
    }
    assert!(operation.join("program").is_dir(), "retained evidence is never deleted");
    fixture.tasks.shutdown().await;
    drop((stop_sender, cancel_sender));
}
#[tokio::test]
async fn owned_node_install_pending_does_not_end_task_or_release_source_or_prepare() { exercise(false).await; }
#[tokio::test]
async fn owned_node_install_unknown_keeps_source_candidate_and_real_log_owner() { exercise(true).await; }
