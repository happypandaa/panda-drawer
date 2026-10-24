//! Real source admission, task and lock owners around a disposable command;
//! only the final reader tail is paused. This does not claim escaped tracking.
use super::*;
use crate::{
    node_apps::NodeApps,
    owned_process::{IoPlan, ProcessPhase},
};

#[tokio::test]
async fn owned_node_git_pending_and_dropped_request_keep_source_token_task_and_lock_fds() {
    for abort in [false, true] {
        let (_temporary, paths) = RuntimePaths::for_test();
        let source_temporary = tempfile::tempdir().unwrap();
        let root = source_temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-node-owned-git-source");
        fs::create_dir_all(root.join("public")).unwrap();
        fs::create_dir_all(root.join("server")).unwrap();
        fs::write(root.join("public/index.html"), "disposable").unwrap();
        fs::write(root.join("server/main.mjs"), "throw Error('never started')").unwrap();
        atomic_json(&root.join("local-app.json"), &json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "dataContract":{"id":"disposable.node-owned-git","version":1},
            "app":{"id":"disposable.node-owned-git","name":"可丢弃 Node Git 所有权","version":"1.0.0"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
        })).unwrap();
        let apps = Arc::new(
            NodeApps::new(
                paths.data_root().into(),
                "/nonexistent-disposable-node".into(),
            )
            .unwrap(),
        );
        let id = apps.register(root.clone()).await.unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let source = apps.development_source_lease(&id).await.unwrap();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let key = format!("node-development:{id}");
        let lease = Arc::new(store.reserve_task(&key, "disposable-node-git").unwrap());
        let registry = store.processes();
        let manager = NodeGit::new(GitEnvironmentManager::new(paths.data_root().into(), None))
            .with_process_scope(
                registry.clone(),
                Arc::new((source.keep_alive(), lease, apps.clone())),
            );
        let (mut context, _senders) = manager.context().unwrap();
        drop((manager, source));
        let private = context.root().to_path_buf();
        let lock_path = root.join("owned-index.lock");
        let lock = IndexLock::acquire(&lock_path).unwrap();
        context.retain(lock.retained.clone());
        let marker = root.join("actual-reader-write");
        let actual_marker = marker.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'disposable Node Git output'"]);
        let run = context
            .start(command, move |pipes| {
                drop(pipes.stdin);
                IoPlan::new(async move {
                    let _ = read_output(pipes.stdout, true, 1024).await?;
                    let _ = entered.send(());
                    let _ = blocked.await;
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(actual_marker)?;
                    file.write_all(b"owned Node Git reader finished\n")?;
                    file.sync_all()?;
                    Ok(())
                })
                .drain("stderr", async move {
                    read_output(pipes.stderr, false, 1024).await.map(|_| ())
                })
            })
            .unwrap();
        let request = tokio::spawn(async move {
            let _lock = lock;
            context.wait(run).await
        });
        ready.await.unwrap();
        if abort {
            request.abort();
            assert!(request.await.err().unwrap().is_cancelled());
        } else {
            let issue = request.await.unwrap().err().unwrap();
            assert_eq!(issue.code, "PROCESS_CLEANUP_PENDING");
            assert_eq!(issue.details["processResourceClosed"], false);
            assert_eq!(issue.details["operationOutcome"], "unknown");
        }
        assert!(private.is_dir());
        assert!(lock_path.is_file());
        assert!(store.reserve_task(&key, "late-task").is_err());
        assert_eq!(
            apps.start(&id).await.unwrap_err().code,
            "NODE_DEVELOPMENT_BUSY"
        );
        assert_eq!(
            apps.delete_preview(&id).await.unwrap_err().code,
            "NODE_DEVELOPMENT_BUSY"
        );
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4), async {
            while registry
                .snapshot()
                .iter()
                .any(|record| record.phase != ProcessPhase::Closed)
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(registry.shutdown().await.is_empty());
        assert_eq!(
            fs::read(marker).unwrap(),
            b"owned Node Git reader finished\n"
        );
        assert!(!private.exists());
        assert!(!lock_path.exists());
        drop(store.reserve_task(&key, "after-actual-reader").unwrap());
        apps.install_target_snapshot(Some(&id)).await.unwrap();
        assert!(store.shutdown().await.is_empty());
    }
}

#[test]
fn owned_node_git_status_does_not_swallow_a_process_start_failure_as_unavailable_git() {
    let (temporary, root) = source();
    let data_root = temporary.path().canonicalize().unwrap();
    let tool = data_root.join("disposable-git-probe");
    fs::write(&tool, "#!/bin/sh\nprintf 'git version 2.56.0\\n'\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    atomic_json(
        &data_root.join("git-toolchain.json"),
        &json!({"schemaVersion":1,"gitPath":tool}),
    )
    .unwrap();
    let registry = crate::owned_process::OwnedProcessRegistry::default();
    let environment = GitEnvironmentManager::with_processes(
        temporary.path().canonicalize().unwrap(),
        None,
        registry.clone(),
    );
    let manager = NodeGit::new(environment).with_process_scope(registry.clone(), Arc::new(()));
    // The runtime intentionally has timers but no I/O reactor. Preflight must
    // reject before spawning, and status must preserve that process error.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let issue = runtime.block_on(manager.status(&root)).unwrap_err();
    assert!(issue.code.starts_with("PROCESS_"), "{issue:?}");
    assert!(
        registry.snapshot().is_empty(),
        "rejected preflight never starts a command"
    );
}
