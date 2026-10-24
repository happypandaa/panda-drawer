// Real source/candidate files and ordinary npm-process fixture; only reader
// completion and panic are controlled, not an OS or physical-device claim.
use super::*;
use crate::{node_apps::NodeApps, owned_process::{OwnedProcessRegistry, ProcessPhase}};

async fn exercise(panic: bool) {
    let mut fixture = Fixture::new(); fixture.npm();
    let fixture = Arc::new(fixture);
    let (temporary, paths) = RuntimePaths::for_test();
    let temporary = Arc::new(temporary);
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let apps = Arc::new(NodeApps::new(paths.data_root().into(), "/nonexistent-disposable-node".into()).unwrap());
    let id = apps.register(fixture.root.clone()).await.unwrap()["id"].as_str().unwrap().to_owned();
    let source_lease = apps.development_build_source_lease(&id).await.unwrap();
    let key = format!("node-development:{id}");
    let task_lease = Arc::new(store.reserve_task(&key, "owned-node-build-test").unwrap());
    let registry: OwnedProcessRegistry = store.processes();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let late_writer = fixture.operation.join("owned-reader-finished");
    let scope = NodeProcessScope::new(registry.clone(), Arc::new((task_lease, source_lease.keep_alive(), apps.clone(), fixture.clone(), temporary.clone())))
        .with_reader_fault(process_scope::NodeReaderFault { component:"node-development/npm".into(), step:"run".into(), entered, release:(!panic).then_some(blocked), panic, writer:Some(late_writer.clone()) });
    drop(source_lease);
    let author_before = fs::read(fixture.root.join("local-app.json")).unwrap();
    let (stop_sender, stop) = watch::channel(false);
    let (cancel_sender, cancel) = watch::channel(false);
    let paths = vec!["local-app.json", "package.json", "package-lock.json", "public/index.html", "server/main.mjs"].into_iter().map(str::to_owned).collect::<Vec<_>>();
    let work = { let fixture=fixture.clone(); tokio::spawn(async move { build_repository_scoped(&fixture.root, &fixture.operation, &fixture.tools, stop, cancel, &paths, scope).await }) };
    ready.await.unwrap();
    if !panic {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !registry.snapshot().iter().any(|record| record.component == "node-development/npm" && record.phase == ProcessPhase::CleanupPending) { sleep(Duration::from_millis(10)).await; }
        }).await.unwrap();
        assert!(!work.is_finished());
        assert!(!fixture.operation.join("program").exists(), "pending never exposes a prepared output");
    }
    assert_eq!(apps.start(&id).await.unwrap_err().code, "NODE_DEVELOPMENT_BUSY");
    assert!(store.reserve_task(&key, "late-task").is_err());
    assert!(fixture.operation.join("repository/release").is_dir());
    if !panic { release.send(()).unwrap(); }
    let issue = work.await.unwrap().unwrap_err();
    assert_eq!(issue.details["processResourceClosed"], !panic);
    assert_eq!(fs::read(fixture.root.join("local-app.json")).unwrap(), author_before);
    assert!(!fixture.operation.join("program").exists());
    if panic {
        assert!(!late_writer.exists());
        assert_eq!(apps.delete_preview(&id).await.unwrap_err().code, "NODE_DEVELOPMENT_BUSY");
        assert!(store.reserve_task(&key, "late-task").is_err());
        let issues = registry.shutdown().await;
        assert!(issues.iter().any(|issue| issue.code == "PROCESS_IO_JOIN_FAILED" && !issue.retryable));
    } else {
        assert_eq!(fs::read(late_writer).unwrap(), b"actual owned Node writer completed\n");
        apps.install_target_snapshot(Some(&id)).await.unwrap();
        drop(store.reserve_task(&key, "after-real-join").unwrap());
        assert!(registry.shutdown().await.is_empty());
    }
    drop((stop_sender, cancel_sender));
}

#[tokio::test]
async fn owned_node_build_pending_keeps_real_instance_token_task_lease_and_candidate() { exercise(false).await; }
#[tokio::test]
async fn owned_node_build_unknown_keeps_instance_and_files_and_never_publishes() { exercise(true).await; }
