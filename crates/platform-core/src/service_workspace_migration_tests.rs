//! TempDir-only integration of runtime receipts, actual file I/O and retry.
use super::*;

async fn wait_idle(service: &RuntimeService) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while service.shared.migration_active.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn destination(temp: &tempfile::TempDir) -> PathBuf {
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-new-workspace");
    std::fs::create_dir(&path).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migration_waits_actual_non_artifact_attachment_reader_after_losing_its_waiter() {
    let (temp, paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable migration attachment drain").await;
    let path = project.project_root.join("data/attachments/disposable.bin");
    std::fs::write(&path, b"attachment read before switch").unwrap();
    let file = std::fs::File::open(path).unwrap();
    let gate = service
        .store()
        .project(&project.id)
        .unwrap()
        .operation_gate
        .clone();
    let registry = service.handle().io_lifetime();
    let (started, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let lease = registry.admit().unwrap();
    let caller = tokio::spawn(lease.run(async move {
        let _gate = gate.read_owned().await;
        tokio::task::spawn_blocking(move || {
            let mut file = file;
            let _ = started.send(());
            wait.recv().unwrap();
            let mut contents = Vec::new();
            file.read_to_end(&mut contents).unwrap();
            assert_eq!(contents, b"attachment read before switch");
        })
        .await
        .unwrap();
    }));
    ready.await.unwrap();
    caller.abort();
    let _ = caller.await;
    let target = destination(&temp);
    let task = service
        .handle()
        .workspace_migrate(json!({"destinationPath":target}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if registry
                .admit()
                .err()
                .is_some_and(|e| e.code == "WORKSPACE_MIGRATING")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(paths.workspace_root().exists());
    assert_eq!(
        std::fs::read_dir(&target).unwrap().count(),
        0,
        "no copy while actual reader remains"
    );
    release.send(()).unwrap();
    wait_idle(&service).await;
    let completed = service
        .handle()
        .task(task["taskId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(completed["status"], "succeeded", "{completed}");
    assert_eq!(completed["result"]["sourceRetained"], false, "{completed}");
    assert!(!paths.workspace_root().exists());
    assert!(registry.admit().is_ok());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_cleanup_failure_keeps_new_business_available_and_id_retry_uses_saved_copy() {
    let (temp, paths, service, _client) = tests::start().await;
    tests::create(&service, "Disposable before cleanup").await;
    let target = destination(&temp);
    service
        .handle()
        .set_workspace_migration_hook(Arc::new(|notice| {
            Box::pin(async move {
                if notice.stage == WorkspaceMigrationStage::Reopened {
                    assert!(notice.source_path.is_dir());
                    std::fs::write(
                        notice.source_path.join("disposable-new-source-file"),
                        b"keep unknown source write",
                    )
                    .unwrap();
                }
                Ok(())
            })
        }));
    let task = service
        .handle()
        .workspace_migrate(json!({"destinationPath":target}))
        .await
        .unwrap();
    wait_idle(&service).await;
    let migration = task["taskId"].as_str().unwrap();
    let completed = service.handle().task(migration).await.unwrap();
    assert_eq!(completed["status"], "succeeded", "{completed}");
    assert_eq!(completed["result"]["cleanupPending"], true, "{completed}");
    assert!(paths.workspace_root().is_dir());
    let info = service.handle().workspace_info().await.unwrap();
    assert_eq!(info["migrationState"], "ready");
    assert_eq!(info["migrationCleanupTask"]["status"], "failed");
    let project = tests::create(&service, "Disposable legitimate new business write").await;
    assert!(project.project_root.starts_with(&target));
    assert!(service
        .handle()
        .workspace_migration_cleanup("task_unknown".into())
        .await
        .is_err());
    let error = service
        .handle()
        .workspace_migrate(json!({"destinationPath":temp.path().join("unused")}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "WORKSPACE_SOURCE_CLEANUP_PENDING");
    std::fs::remove_file(paths.workspace_root().join("disposable-new-source-file")).unwrap();
    let state = service.shared.snapshot();
    let client = reqwest::Client::new();
    let endpoint = format!(
        "{}/api/v1/workspace/migrations/{migration}/cleanup",
        state.info.base_url
    );
    assert_eq!(
        client
            .post(&endpoint)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let wrong: Value = client
        .post(&endpoint)
        .bearer_auth(&state.token)
        .json(&json!({"path":paths.workspace_root()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(wrong["error"]["code"], "INVALID_INPUT");
    let response = client
        .post(endpoint)
        .bearer_auth(&state.token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value = response.json().await.unwrap();
    let retry = body["data"].clone();
    wait_idle(&service).await;
    let retry = service
        .handle()
        .task(retry["taskId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(retry["status"], "succeeded", "{retry}");
    assert!(!paths.workspace_root().exists());
    assert!(project.project_root.is_dir());
    // Once the old source scope ended, another directory at that path is not
    // silently adopted by retry or complete-local-data ownership accounting.
    std::fs::create_dir(paths.workspace_root()).unwrap();
    std::fs::write(
        paths.workspace_root().join("foreign"),
        b"preserve replacement",
    )
    .unwrap();
    let roots = crate::workspace::local_cleanup_roots(&service.shared.snapshot().paths).unwrap();
    assert_eq!(roots, vec![target]);
    assert!(service
        .handle()
        .workspace_migration_cleanup(migration.into())
        .await
        .is_err());
    assert!(paths.workspace_root().join("foreign").exists());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn committed_native_reopen_failure_retains_source_until_explicit_verified_retry() {
    let (temp, paths, service, _client) = tests::start().await;
    tests::create(&service, "Disposable native reopen failure").await;
    let target = destination(&temp);
    service
        .handle()
        .set_workspace_migration_hook(Arc::new(|notice| {
            Box::pin(async move {
                if notice.stage == WorkspaceMigrationStage::Reopened {
                    return Err(ApiError::new(
                        "DISPOSABLE_NATIVE_REOPEN_FAILED",
                        "artificial unconfirmed native receipt",
                    ));
                }
                Ok(())
            })
        }));
    let task = service
        .handle()
        .workspace_migrate(json!({"destinationPath":target}))
        .await
        .unwrap();
    wait_idle(&service).await;
    let id = task["taskId"].as_str().unwrap();
    let task = service.handle().task(id).await.unwrap();
    assert_eq!(task["status"], "failed", "{task}");
    assert_eq!(service.info().workspace_root, target);
    assert!(paths.workspace_root().exists());
    assert_eq!(
        crate::workspace::read_journal(&paths, id)
            .unwrap()
            .source_cleanup
            .unwrap()
            .status(paths.workspace_root())["phase"],
        "awaitingReopen"
    );
    tests::create(&service, "Disposable new root remains usable").await;
    service
        .handle()
        .workspace_migration_cleanup(id.into())
        .await
        .unwrap();
    wait_idle(&service).await;
    assert!(!paths.workspace_root().exists());
    service.shutdown().await.unwrap();
}

async fn interrupted_committed_fixture(
    automatic: bool,
) -> (tempfile::TempDir, RuntimePaths, RuntimePaths, String) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    store
        .create(CreateProjectInput {
            name: "Disposable interrupted migration".into(),
            description: None,
        })
        .await
        .unwrap();
    let target = destination(&temp);
    store.begin_workspace_migration().unwrap();
    let task = crate::workspace::create(&paths, &target).unwrap();
    store.pause_workspace().await.unwrap();
    let (selected, mut journal) =
        crate::workspace::copy_and_commit(&paths, &store, &task.task_id).unwrap();
    let selected_store = ProjectStore::new(target).unwrap();
    crate::workspace::initialize_selected(&selected, &selected_store, &mut journal).unwrap();
    if !automatic {
        journal.source_cleanup = None;
        crate::workspace::save_journal(&selected, &journal).unwrap();
    }
    selected_store.shutdown().await;
    store.shutdown().await;
    (temp, paths, selected, task.task_id)
}

#[tokio::test]
async fn committed_copy_waits_native_ready_and_legacy_journals_never_gain_cleanup_authority() {
    for automatic in [true, false] {
        let (_temp, paths, selected, id) = interrupted_committed_fixture(automatic).await;
        let lock = selected.acquire_lock().unwrap();
        let service = RuntimeService::start_inner(selected, lock, None, true)
            .await
            .unwrap();
        assert!(
            paths.workspace_root().exists(),
            "metadata recovery must not delete"
        );
        assert!(service.handle().check_native_ready().is_err());
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        service
            .handle()
            .set_workspace_migration_hook(Arc::new(move |notice| {
                let seen = seen.clone();
                Box::pin(async move {
                    assert_eq!(notice.stage, WorkspaceMigrationStage::Startup);
                    assert!(notice.source_path.exists());
                    assert!(notice.destination_path.exists());
                    seen.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }));
        service.handle().complete_native_startup();
        wait_idle(&service).await;
        assert_eq!(paths.workspace_root().exists(), !automatic);
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(automatic));
        if !automatic {
            assert!(service
                .handle()
                .workspace_migration_cleanup(id)
                .await
                .is_err());
        }
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn synchronous_startup_cleanup_submission_failures_are_visible_and_retryable() {
    for block_tasks in [false, true] {
        let (_temp, paths, selected, id) = interrupted_committed_fixture(true).await;
        let lock = selected.acquire_lock().unwrap();
        let service = RuntimeService::start_inner(selected.clone(), lock, None, true)
            .await
            .unwrap();
        let held = if block_tasks {
            None
        } else {
            Some(
                service
                    .store()
                    .reserve_transient_operation(
                        "workspace-source-cleanup",
                        "disposable-conflicting-owner",
                    )
                    .unwrap(),
            )
        };
        let task_dir = selected.workspace_root().join(".platform/tasks");
        if block_tasks {
            std::fs::remove_dir(&task_dir).unwrap();
            std::fs::write(&task_dir, b"disposable wrong task directory type").unwrap();
        }
        service.handle().complete_native_startup();
        assert!(!service.shared.migration_active.load(Ordering::Acquire));
        let journal = crate::workspace::read_journal(&selected, &id).unwrap();
        let status = journal
            .source_cleanup
            .unwrap()
            .status(paths.workspace_root());
        assert_eq!(status["phase"], "awaitingReopen");
        assert_eq!(status["cleanupPending"], true, "{status}");
        assert!(status["error"].is_object());
        assert!(paths.workspace_root().exists());
        assert!(service.handle().check_native_ready().is_ok());
        drop(held);
        if block_tasks {
            std::fs::remove_file(&task_dir).unwrap();
            std::fs::create_dir(&task_dir).unwrap();
        }
        service
            .handle()
            .workspace_migration_cleanup(id)
            .await
            .unwrap();
        wait_idle(&service).await;
        assert!(!paths.workspace_root().exists());
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn commit_before_install_failure_never_releases_old_native_or_artifact_authority() {
    let (temp, paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable install failure").await;
    let target = destination(&temp);
    let stages = Arc::new(Mutex::new(vec![]));
    let seen = stages.clone();
    service
        .handle()
        .set_workspace_migration_hook(Arc::new(move |notice| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(notice.stage);
                Ok(())
            })
        }));
    *service.shared.migration_reopen_failure.lock().unwrap() = Some(ApiError::new(
        "DISPOSABLE_BEFORE_INSTALL_FAILURE",
        "injected after durable commit",
    ));
    let task = service
        .handle()
        .workspace_migrate(json!({"destinationPath":target}))
        .await
        .unwrap();
    wait_idle(&service).await;
    let task = service
        .handle()
        .task(task["taskId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(task["status"], "failed", "{task}");
    assert!(paths.workspace_root().is_dir());
    assert!(target.is_dir());
    assert_eq!(
        *stages.lock().unwrap(),
        vec![WorkspaceMigrationStage::Prepare]
    );
    assert!(service.shared.snapshot().stopping.load(Ordering::SeqCst));
    assert!(service.handle().launch(&project.id).await.is_err());
    assert!(service
        .shared
        .migration_retained_state
        .lock()
        .unwrap()
        .is_some());
    // The settings commit survives even if the immediately following journal
    // checkpoint was lost. It still cannot release old runtime/native access,
    // but must permit exact owned-discovery cleanup and a normal safe restart.
    let migration = task["taskId"].as_str().unwrap();
    let mut checkpoint = crate::workspace::read_journal(&paths, migration).unwrap();
    assert!(checkpoint.committed);
    assert!(checkpoint.source_cleanup.as_ref().unwrap().needs_reopen());
    checkpoint.committed = false;
    crate::workspace::save_journal(&paths, &checkpoint).unwrap();
    assert!(crate::workspace::is_committed(&paths, migration).unwrap());
    let selected = paths.reinitialize_for_test().unwrap();
    assert_eq!(selected.workspace_root(), target);
    let snapshot = service.shared.snapshot();
    assert!(!snapshot
        .paths
        .remove_stopped_runtime_if_owned(&Uuid::new_v4().to_string(), &snapshot.token)
        .unwrap());
    assert!(!snapshot
        .paths
        .remove_stopped_runtime_if_owned(&snapshot.info.instance_id, &"b".repeat(64))
        .unwrap());
    assert!(snapshot.paths.connection_path().exists());
    assert!(snapshot.paths.token_path().exists());
    service.shutdown().await.unwrap();
    assert!(
        paths.workspace_root().exists(),
        "shutdown is not a reopen receipt or source cleanup authorization"
    );
    assert!(!snapshot.paths.connection_path().exists());
    assert!(!snapshot.paths.token_path().exists());

    let lock = selected.acquire_lock().unwrap();
    let service = RuntimeService::start(selected, lock).await.unwrap();
    wait_idle(&service).await;
    assert!(!paths.workspace_root().exists());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn old_source_cleanup_excludes_live_project_deletion_receipts() {
    let (_temp, paths) = RuntimePaths::for_test();
    let coordinator = crate::project_cleanup::ProjectCleanupCoordinator::new(paths.data_root());
    let notice = crate::project_cleanup::DeletedProjectNotice::new(
        crate::project_cleanup::ProjectKind::Node,
        Uuid::new_v4().to_string(),
        vec![],
        vec![],
        vec![],
    );
    let notice = coordinator.prepare(notice).await.unwrap();
    assert!(coordinator.try_migration_source_cleanup().is_err());
    let clone = notice.clone();
    drop(notice);
    assert!(
        coordinator.try_migration_source_cleanup().is_err(),
        "actual receipt clones retain source exclusion"
    );
    drop(clone);
    let held = coordinator.try_migration_source_cleanup().unwrap();
    drop(held);
}

#[tokio::test]
async fn a_precommit_aborted_migration_does_not_reserve_current_source_folders_forever() {
    let (temp, paths, service, _client) = tests::start().await;
    let destination = destination(&temp);
    let task = crate::workspace::create(&paths, &destination).unwrap();
    crate::workspace::finish(
        &paths,
        &task.task_id,
        Err(ApiError::new(
            "DISPOSABLE_PRECOMMIT_ABORT",
            "No target selected",
        )),
    )
    .unwrap();
    assert!(crate::workspace::require_source_outside_pending_migrations(
        paths.data_root(),
        &paths
            .workspace_root()
            .join("custom/disposable-current-source")
    )
    .is_ok());
    assert!(paths.workspace_root().is_dir());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn selected_commit_blocks_source_claims_when_the_final_journal_flag_was_not_saved() {
    let (_temp, paths, selected, id) = interrupted_committed_fixture(true).await;
    let mut journal = crate::workspace::read_journal(&selected, &id).unwrap();
    journal.committed = false;
    crate::workspace::save_journal(&selected, &journal).unwrap();
    let error = crate::workspace::require_source_outside_pending_migrations(
        paths.data_root(),
        &paths.workspace_root().join("custom/disposable-source"),
    )
    .unwrap_err();
    assert_eq!(error.code, "NODE_SOURCE_MIGRATION_PENDING");
    assert!(paths.workspace_root().is_dir());
}
