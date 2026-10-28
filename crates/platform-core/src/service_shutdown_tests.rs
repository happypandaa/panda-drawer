//! Real owned tasks, SQLite threads and loopback connections; artificial
//! TempDir workspaces only. No installed applications or user data are read.
use super::*;
use crate::shutdown::ShutdownStatus;

fn process_fixture_io(
    pipes: crate::owned_process::ProcessPipes,
) -> crate::owned_process::IoPlan<()> {
    use tokio::io::AsyncReadExt;
    let crate::owned_process::ProcessPipes {
        stdin,
        mut stdout,
        mut stderr,
        stop: _,
    } = pipes;
    drop(stdin);
    crate::owned_process::IoPlan::new(async move {
        let mut bytes = [0u8; 1024];
        while stdout.read(&mut bytes).await? != 0 {}
        Ok(())
    })
    .drain("stderr", async move {
        let mut bytes = [0u8; 1024];
        while stderr.read(&mut bytes).await? != 0 {}
        Ok(())
    })
}

#[tokio::test]
async fn owned_process_quiesce_retains_discovery_until_pending_reader_really_joins() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let registry = service.store().processes();
    let project =
        super::super::tests::create(&service, "Disposable owned-process SQLite boundary").await;
    let store = service.store();
    let result = store
        .execute(
            &project.id,
            serde_json::from_value(json!({"sql":"SELECT 1 AS value"})).unwrap(),
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    assert_eq!(result.rows[0]["value"], 1);
    assert!(store.fixture_has_database_worker(&project.id));
    let writer_directory = Arc::new(
        tempfile::Builder::new()
            .prefix("disposable-process-owned-writer-")
            .tempdir()
            .unwrap(),
    );
    let marker = writer_directory.path().join("complete");
    let writer_marker = marker.clone();
    let (release, blocked) = oneshot::channel();
    let mut options = crate::owned_process::ProcessOptions::new(
        "disposable-service-reader",
        Duration::from_secs(3),
    );
    options.cleanup_timeout = Duration::from_millis(25);
    let run = registry
        .start(
            tokio::process::Command::new("/usr/bin/true"),
            options,
            writer_directory,
            |pipes| {
                process_fixture_io(pipes).drain("owned-held-reader", async move {
                    let _ = blocked.await;
                    std::fs::write(writer_marker, b"actual owned writer completed")?;
                    Ok(())
                })
            },
        )
        .unwrap();
    assert_eq!(run.await.unwrap_err().code, "PROCESS_CLEANUP_PENDING");
    let manager_drained = Arc::new(AtomicBool::new(false));
    let observed = manager_drained.clone();
    let manager_registry = registry.clone();
    service
        .shared
        .snapshot()
        .project_git
        .as_ref()
        .unwrap()
        .fixture_shutdown_runner(tokio::spawn(async move {
            while manager_registry.snapshot()[0].phase != crate::owned_process::ProcessPhase::Closed
            {
                tokio::task::yield_now().await;
            }
            // A real pipeline must wait for this proof before touching output
            // rollback. Joining it first would hide the cleanup-pending report.
            observed.store(true, Ordering::Release);
        }));
    let failed = tokio::time::timeout(Duration::from_secs(3), service.quiesce())
        .await
        .expect("process pending is reported before manager join");
    assert_eq!(failed.status, ShutdownStatus::Blocked);
    assert!(!failed.resource_closed);
    assert_eq!(failed.phase, "processes");
    assert!(
        store.fixture_has_database_worker(&project.id),
        "actual SQLite worker stays owned before process drain"
    );
    assert!(
        service.shutdown.resources.lock().await.http.is_some(),
        "HTTP join owner is not consumed early"
    );
    assert_eq!(service.shutdown.resources.lock().await.stage, 2);
    assert!(!manager_drained.load(Ordering::Acquire));
    assert!(paths.connection_path().is_file());
    assert!(paths.acquire_lock().is_err());
    assert!(failed
        .issues
        .iter()
        .any(|i| i.code == "PROCESS_CLEANUP_PENDING" && i.retryable));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while registry.snapshot()[0].phase != crate::owned_process::ProcessPhase::Closed {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let complete = service.quiesce().await;
    assert!(complete.is_complete(), "{complete:?}");
    assert!(manager_drained.load(Ordering::Acquire));
    assert!(registry.snapshot()[0].resource_closed());
    assert!(
        !store.fixture_has_database_worker(&project.id),
        "worker closes only after real process and writer joins"
    );
    assert!(!paths.connection_path().exists());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn owned_process_registry_and_pending_owner_survive_real_workspace_migration() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let registry = service.store().processes();
    let (release, blocked) = oneshot::channel();
    let mut options = crate::owned_process::ProcessOptions::new(
        "disposable-migration-reader",
        Duration::from_secs(3),
    );
    options.cleanup_timeout = Duration::from_millis(25);
    let run = registry
        .start(
            tokio::process::Command::new("/usr/bin/true"),
            options,
            (),
            |pipes| {
                process_fixture_io(pipes).drain("global-fixture-reader", async move {
                    let _ = blocked.await;
                    Ok(())
                })
            },
        )
        .unwrap();
    assert_eq!(run.await.unwrap_err().code, "PROCESS_CLEANUP_PENDING");
    let process_id = registry.snapshot()[0].id.clone();
    let instance = service.info().instance_id;
    let destination = paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-owned-process-selected-workspace");
    std::fs::create_dir(&destination).unwrap();
    let first = submit_migration(
        service.shared.clone(),
        service.shared.snapshot(),
        crate::workspace::MigrationInput {
            destination_path: destination.clone(),
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.shared.migration_active.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        service.info().instance_id,
        instance,
        "pending writer prevents switching the workspace"
    );
    let blocked = crate::workspace::task(&paths, &first.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(blocked.status, crate::TaskStatus::Failed);
    assert!(blocked.error.unwrap().message.contains("写入进程"));
    assert!(
        std::fs::read_dir(&destination).unwrap().next().is_none(),
        "no destination copy starts before real writer closure"
    );
    assert_eq!(service.shared.processes.snapshot()[0].id, process_id);
    assert!(service.store().processes().shares_registry(&registry));
    assert_eq!(
        service.shared.processes.snapshot()[0].phase,
        crate::owned_process::ProcessPhase::CleanupPending
    );
    assert!(!registry.snapshot()[0].resource_closed());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while registry.snapshot()[0].phase != crate::owned_process::ProcessPhase::Closed {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    crate::durable_writer::require_all_clear(paths.data_root()).unwrap();
    // Only an explicit retry after the actual group/readers finish may migrate.
    submit_migration(
        service.shared.clone(),
        service.shared.snapshot(),
        crate::workspace::MigrationInput {
            destination_path: destination,
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.info().instance_id == instance
            || service.shared.migration_active.load(Ordering::Acquire)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(service.shared.processes.snapshot()[0].id, process_id);
    assert!(service.store().processes().shares_registry(&registry));
    assert!(service.quiesce().await.is_complete());
    service.shutdown().await.unwrap();
}

async fn http_panic_fixture(service: &RuntimeService) {
    let mut resources = service.shutdown.resources.lock().await;
    if let Some(signal) = resources.signal.take() {
        let _ = signal.send(());
    }
    if let Some(server) = resources.http.take() {
        server.await.unwrap().unwrap();
    }
    resources.http = Some(tokio::spawn(async {
        panic!("controlled artificial server panic");
    }));
}

#[tokio::test]
async fn consuming_error_retains_instance_discovery_and_sticky_http_failure() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    http_panic_fixture(&service).await;
    let owner = Arc::downgrade(&service.shutdown);
    assert!(service.shutdown().await.is_err());
    assert!(paths.acquire_lock().is_err());
    assert!(paths.connection_path().is_file());
    let owner = owner
        .upgrade()
        .expect("failed consuming shutdown retains owned coordinator");
    let initial = owner.status();
    assert_eq!(initial.status, ShutdownStatus::Blocked);
    assert!(!initial.resource_closed);
    assert!(initial.issues.iter().any(|issue| issue.component == "http"));
    let again = owner.wait().await;
    assert_eq!(again.status, ShutdownStatus::Blocked);
    assert_eq!(again.attempt, initial.attempt);
    assert_eq!(again.issues, initial.issues);
    assert!(paths.acquire_lock().is_err());
}

#[tokio::test]
async fn discovery_failure_keeps_lock_and_requires_an_explicit_retry() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let original = std::fs::read(paths.connection_path()).unwrap();
    std::fs::write(paths.connection_path(), b"artificial broken discovery").unwrap();
    let failed = service.quiesce().await;
    assert_eq!(failed.status, ShutdownStatus::Blocked);
    assert!(failed.resource_closed);
    assert!(failed
        .issues
        .iter()
        .all(|issue| issue.component == "discovery" && issue.retryable));
    assert!(paths.acquire_lock().is_err());
    std::fs::write(paths.connection_path(), original).unwrap();
    let success = service.quiesce().await;
    assert!(success.is_complete());
    assert_eq!(success.attempt, failed.attempt + 1);
    assert!(!paths.connection_path().exists());
    assert!(
        paths.acquire_lock().is_err(),
        "non-consuming quiesce retains instance ownership"
    );
    service.shutdown().await.unwrap();
    assert!(paths.acquire_lock().is_ok());
}

#[tokio::test]
async fn consuming_discovery_error_does_not_retry_from_drop() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let original = std::fs::read(paths.connection_path()).unwrap();
    std::fs::write(paths.connection_path(), b"artificial malformed discovery").unwrap();
    let weak = Arc::downgrade(&service.shutdown);
    assert!(service.shutdown().await.is_err());
    std::fs::write(paths.connection_path(), original).unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    let owner = weak.upgrade().unwrap();
    assert_eq!(owner.status().status, ShutdownStatus::Blocked);
    assert_eq!(owner.status().attempt, 1);
    assert!(paths.connection_path().exists());
    assert!(paths.acquire_lock().is_err());
    assert!(owner.wait().await.is_complete());
}

#[tokio::test]
async fn dropped_quiesce_waiter_cannot_cancel_the_actual_drain() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let service = Arc::new(service);
    let (release, blocked) = oneshot::channel();
    let lease = service.handle().io_lifetime().admit().unwrap();
    let work = tokio::spawn(lease.run(async move {
        let _ = blocked.await;
    }));
    let waiting = {
        let service = service.clone();
        tokio::spawn(async move { service.quiesce().await })
    };
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(service.shutdown_status().status, ShutdownStatus::Pending);
    waiting.abort();
    let _ = waiting.await;
    assert!(paths.acquire_lock().is_err());
    release.send(()).unwrap();
    work.await.unwrap().unwrap();
    let report = service.quiesce().await;
    assert!(report.is_complete());
    assert_eq!(report.attempt, 1);
}

#[tokio::test]
async fn actual_http_body_outlives_old_timeout_and_requires_child_completion() {
    let (_temporary, paths, service, client) = super::super::tests::start().await;
    let service = Arc::new(service);
    let (release, blocked) = oneshot::channel();
    let blocked = Arc::new(Mutex::new(Some(blocked)));
    *service.shared.router.write().unwrap() = Router::new()
        .route(
            "/artificial-long-body",
            get(move || {
                let blocked = blocked.lock().unwrap().take().unwrap();
                async move {
                    Body::from_stream(stream::unfold(
                        (false, Some(blocked)),
                        |(sent, blocked)| async move {
                            if !sent {
                                Some((Ok::<_, Infallible>("artificial bytes"), (true, blocked)))
                            } else {
                                let _ = blocked.unwrap().await;
                                None
                            }
                        },
                    ))
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            service.shared.snapshot(),
            validate_host,
        ));
    let response = client
        .get(format!("{}/artificial-long-body", service.info().base_url))
        .send()
        .await
        .unwrap();
    let mut waiter = {
        let service = service.clone();
        tokio::spawn(async move { service.quiesce().await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(3150), &mut waiter)
            .await
            .is_err()
    );
    assert_eq!(service.shutdown_status().status, ShutdownStatus::Pending);
    assert_eq!(service.shutdown_status().phase, "http");
    assert!(paths.connection_path().exists());
    assert!(paths.acquire_lock().is_err());
    release.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(report.is_complete());
    drop(response);
}

#[tokio::test]
async fn http_shutdown_request_returns_pending_without_waiting_for_its_own_lease() {
    let (_temporary, _paths, service, client) = super::super::tests::start().await;
    let service = Arc::new(service);
    let owned = Arc::downgrade(&service);
    *service.shared.router.write().unwrap() = Router::new()
        .route(
            "/artificial-stop",
            get(move || {
                let service = owned.upgrade().unwrap();
                async move { Json(service.quiesce().await) }
            }),
        )
        .layer(middleware::from_fn_with_state(
            service.shared.snapshot(),
            validate_host,
        ));
    let response = client
        .get(format!("{}/artificial-stop", service.info().base_url))
        .send()
        .await
        .unwrap();
    let report: crate::shutdown::ShutdownReport = response.json().await.unwrap();
    assert_eq!(report.status, ShutdownStatus::Pending);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), service.quiesce())
            .await
            .unwrap()
            .is_complete()
    );
}

#[tokio::test]
async fn finished_panic_survives_later_read_only_git_request_and_shutdown() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let project = super::super::tests::create(&service, "Disposable shutdown task fault").await;
    let manager = service.shared.snapshot().project_git.clone().unwrap();
    let fault = tokio::spawn(async {
        panic!("controlled artificial Git runner panic");
    });
    while !fault.is_finished() {
        tokio::task::yield_now().await;
    }
    manager.fixture_shutdown_runner(fault);
    let view = manager
        .status(&project.id, service.shared.snapshot().stop.subscribe())
        .await
        .unwrap();
    assert!(!view.can_save);
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.component == "projectGit" && issue.code == "SHUTDOWN_JOIN_FAILED"));
    assert!(paths.acquire_lock().is_err());
    let again = service.quiesce().await;
    assert_eq!(again.issues, report.issues);
}

#[tokio::test]
async fn retired_panicked_sqlite_worker_cannot_disappear_from_shutdown_report() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let project = super::super::tests::create(&service, "Disposable SQLite join fault").await;
    let store = service.store();
    store.fixture_panicked_database_worker(&project.id);
    let result = store
        .execute(
            &project.id,
            serde_json::from_value(json!({"sql":"SELECT 1 AS value"})).unwrap(),
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    assert_eq!(result.rows[0]["value"], 1);
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(report.resource_closed);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == "DATABASE_THREAD_JOIN_FAILED"));
    assert!(paths.connection_path().exists());
    assert!(paths.acquire_lock().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migration_registration_and_shutdown_share_the_same_submission_lock() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let destination = paths
        .data_root()
        .parent()
        .unwrap()
        .join("artificial-selected-workspace");
    std::fs::create_dir(&destination).unwrap();
    let old_instance = service.info().instance_id;
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    *service.shared.migration_registration_pause.lock().unwrap() = Some((entered, release_rx));
    let service = Arc::new(service);
    let shared = service.shared.clone();
    let state = shared.snapshot();
    let submit = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(submit_migration(
            shared,
            state,
            crate::workspace::MigrationInput {
                destination_path: destination,
            },
        ))
    });
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    let mut closing = {
        let service = service.clone();
        tokio::spawn(async move { service.quiesce().await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut closing)
            .await
            .is_err()
    );
    assert!(paths.acquire_lock().is_err());
    release.send(()).unwrap();
    submit.await.unwrap().unwrap();
    let report = tokio::time::timeout(Duration::from_secs(8), closing)
        .await
        .unwrap()
        .unwrap();
    assert!(report.is_complete(), "{report:?}");
    let current = service.shared.snapshot();
    assert_ne!(current.info.instance_id, old_instance);
    assert!(current.stopping.load(Ordering::SeqCst));
    assert!(!current.paths.connection_path().exists());
}

#[tokio::test]
async fn all_manager_stop_signals_precede_waiting_for_native_io() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let input = paths
        .workspace_root()
        .join("artificial-cancel-dependent-file");
    std::fs::write(&input, b"artificial gated file").unwrap();
    let mut file = std::fs::File::open(input).unwrap();
    let (release, blocked) = std::sync::mpsc::channel();
    let (started, ready) = oneshot::channel();
    service
        .shared
        .snapshot()
        .project_git
        .as_ref()
        .unwrap()
        .fixture_shutdown_cancel(release);
    let work = tokio::spawn(
        service
            .handle()
            .io_lifetime()
            .admit()
            .unwrap()
            .run(async move {
                tokio::task::spawn_blocking(move || {
                    let _ = started.send(());
                    blocked.recv().unwrap();
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).unwrap();
                    assert_eq!(bytes, b"artificial gated file");
                })
                .await
                .unwrap();
            }),
    );
    ready.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), service.quiesce())
            .await
            .unwrap()
            .is_complete()
    );
    work.await.unwrap().unwrap();
}

#[tokio::test]
async fn previous_migration_panic_cannot_be_replaced_by_a_later_migration() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let fault = tokio::spawn(async {
        panic!("controlled earlier migration panic");
    });
    while !fault.is_finished() {
        tokio::task::yield_now().await;
    }
    *service.shared.migration_runner.lock().unwrap() = Some(fault);
    let destination = paths
        .data_root()
        .parent()
        .unwrap()
        .join("artificial-after-migration-panic");
    std::fs::create_dir(&destination).unwrap();
    submit_migration(
        service.shared.clone(),
        service.shared.snapshot(),
        crate::workspace::MigrationInput {
            destination_path: destination,
        },
    )
    .await
    .unwrap();
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(report.issues.iter().any(
        |issue| issue.component == "workspaceMigration" && issue.code == "SHUTDOWN_JOIN_FAILED"
    ));
    assert!(paths.acquire_lock().is_err());
    assert!(service.shared.snapshot().stopping.load(Ordering::SeqCst));
}

#[tokio::test]
async fn independent_manager_database_and_http_failures_are_all_reported() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let project = super::super::tests::create(&service, "Disposable aggregate join faults").await;
    service
        .store()
        .fixture_panicked_database_worker(&project.id);
    service
        .store()
        .execute(
            &project.id,
            serde_json::from_value(json!({"sql":"SELECT 1"})).unwrap(),
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    let manager = service.shared.snapshot().project_git.clone().unwrap();
    manager.fixture_shutdown_runner(tokio::spawn(async {
        panic!("controlled aggregate manager panic");
    }));
    http_panic_fixture(&service).await;
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    for component in ["projectGit", "database", "http"] {
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.component == component),
            "missing {component}: {report:?}"
        );
    }
    assert!(paths.connection_path().exists());
    assert!(paths.acquire_lock().is_err());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn partial_discovery_unlink_cannot_turn_into_success_on_retry() {
    use std::os::unix::ffi::OsStrExt;
    struct ClearFlag(std::ffi::CString);
    impl Drop for ClearFlag {
        fn drop(&mut self) {
            unsafe {
                libc::chflags(self.0.as_ptr(), 0);
            }
        }
    }
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let token = std::ffi::CString::new(paths.token_path().as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::chflags(token.as_ptr(), libc::UF_IMMUTABLE) },
        0
    );
    let flag = ClearFlag(token);
    let failed = service.quiesce().await;
    assert_eq!(failed.status, ShutdownStatus::Blocked);
    assert!(failed
        .issues
        .iter()
        .any(|issue| issue.code == "SHUTDOWN_DISCOVERY_PARTIAL" && !issue.retryable));
    assert!(!paths.connection_path().exists());
    assert!(paths.token_path().exists());
    drop(flag);
    let again = service.quiesce().await;
    assert_eq!(again.issues, failed.issues);
    assert_eq!(again.attempt, failed.attempt);
    assert!(paths.acquire_lock().is_err());
}

#[tokio::test]
async fn coordinator_panic_keeps_unknown_sticky_and_still_observes_http_exit() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let shared = service.shared.clone();
    let panic = std::thread::spawn(move || {
        let _guard = shared.migration_previous.lock().unwrap();
        panic!("controlled coordinator mutex poison");
    });
    assert!(panic.join().is_err());
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(!report.resource_closed);
    assert!(report
        .issues
        .iter()
        .any(|issue| { issue.code == "SHUTDOWN_COORDINATOR_PANICKED" && !issue.retryable }));
    let resources = tokio::time::timeout(Duration::from_secs(2), service.shutdown.resources.lock())
        .await
        .unwrap();
    assert!(
        resources.http.is_none(),
        "HTTP must still be actually joined"
    );
    drop(resources);
    let again = service.quiesce().await;
    assert_eq!(again.attempt, report.attempt);
    assert!(!again.is_complete());
    assert!(paths.connection_path().exists());
    assert!(paths.acquire_lock().is_err());
}

#[tokio::test]
async fn native_io_panic_is_sticky_after_the_request_receives_its_error() {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let error = service
        .handle()
        .io_lifetime()
        .admit()
        .unwrap()
        .run::<()>(async { panic!("controlled owned native I/O panic") })
        .await
        .unwrap_err();
    assert_eq!(error.code, "NATIVE_IO_FAILED");
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(!report.resource_closed);
    assert!(report
        .issues
        .iter()
        .any(|issue| { issue.code == "SHUTDOWN_IO_PANICKED" && !issue.retryable }));
    assert!(paths.connection_path().exists());
    assert!(paths.acquire_lock().is_err());
    assert_eq!(service.quiesce().await.attempt, report.attempt);
}

async fn import_inspection_shutdown_case(drop_request: bool) {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    let project = super::super::tests::create(&service, "Disposable import shutdown").await;
    let lifecycle = service.shared.snapshot().lifecycle.clone();
    let archive = paths
        .data_root()
        .parent()
        .unwrap()
        .join("artificial-shutdown-import.zip");
    let export = lifecycle
        .submit_project(&project.id, "export", json!({"destinationPath":archive}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let task = service
                .shared
                .snapshot()
                .builds
                .task(&export.task_id)
                .unwrap();
            if task.status.is_final() {
                assert!(task.error.is_none(), "{task:?}");
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let task_directories = || -> std::collections::BTreeSet<_> {
        std::fs::read_dir(paths.workspace_root().join(".platform/tasks"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    };
    let before = task_directories();
    let (entered, ready) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    lifecycle.fixture_import_inspection_pause(entered, blocked);
    let request_manager = lifecycle.clone();
    let request = tokio::spawn(async move {
        request_manager
            .submit_import(json!({"archivePath":archive,"mode":"copy"}))
            .await
    });
    ready.await.unwrap();
    if drop_request {
        request.abort();
    }
    let initial = service.begin_quiesce();
    assert_eq!(initial.status, ShutdownStatus::Pending);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let pending = service.shutdown_status();
    assert_eq!(pending.status, ShutdownStatus::Pending);
    assert_eq!(pending.phase, "managers");
    assert!(paths.acquire_lock().is_err());
    release.send(()).unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(5), service.quiesce())
        .await
        .unwrap();
    assert!(completed.is_complete(), "{completed:?}");
    if drop_request {
        assert!(request.await.unwrap_err().is_cancelled());
    } else {
        assert_eq!(request.await.unwrap().unwrap_err().code, "SERVICE_STOPPING");
    }
    assert_eq!(
        task_directories(),
        before,
        "stopped inspection cannot publish a new import task"
    );
    let late = lifecycle
        .submit_import(json!({"archivePath":paths.data_root().join("must-not-open.zip")}))
        .await
        .unwrap_err();
    assert_eq!(late.code, "SERVICE_STOPPING");
    assert!(service.store().list().await.unwrap().len() == 1);
}

#[tokio::test]
async fn import_inspection_keeps_owned_lifetime_after_request_waiter_is_lost() {
    import_inspection_shutdown_case(true).await;
}

#[tokio::test]
async fn closing_during_import_inspection_refuses_late_operation_registration() {
    import_inspection_shutdown_case(false).await;
}

#[tokio::test]
async fn shared_task_registry_is_not_reconciled_before_its_business_runner_finishes() {
    let (_temporary, _paths, service, _client) = super::super::tests::start().await;
    let state = service.shared.snapshot();
    let task = state
        .builds
        .create_workspace_operation("fixtureShutdownBoundary")
        .unwrap();
    let id = task.task_id.clone();
    let tasks = state.builds.clone();
    let (release, blocked) = oneshot::channel();
    state
        .project_git
        .as_ref()
        .unwrap()
        .fixture_shutdown_runner(tokio::spawn(async move {
            blocked.await.unwrap();
            tasks
                .finish(&id, Ok(json!({"artificialWorkFinished":true})))
                .unwrap();
        }));
    service.begin_quiesce();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if service.shutdown_status().phase == "managers" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!state.builds.task(&task.task_id).unwrap().status.is_final());
    release.send(()).unwrap();
    assert!(service.quiesce().await.is_complete());
    let final_task = state.builds.task(&task.task_id).unwrap();
    assert_eq!(final_task.status, crate::TaskStatus::Succeeded);
    assert_eq!(final_task.result.unwrap()["artificialWorkFinished"], true);
}

async fn changed_discovery_is_preserved(change_token: bool) {
    let (_temporary, paths, service, _client) = super::super::tests::start().await;
    if change_token {
        let token = paths.read_control_token().unwrap();
        let replacement = if token == "a".repeat(64) {
            "b".repeat(64)
        } else {
            "a".repeat(64)
        };
        std::fs::write(paths.token_path(), replacement).unwrap();
    } else {
        let mut discovery = paths.read_discovery().unwrap();
        discovery.instance_id = Uuid::new_v4().to_string();
        std::fs::write(
            paths.connection_path(),
            serde_json::to_vec(&discovery).unwrap(),
        )
        .unwrap();
    }
    let connection = std::fs::read(paths.connection_path()).unwrap();
    let token = std::fs::read(paths.token_path()).unwrap();
    let weak = Arc::downgrade(&service.shutdown);
    let report = service.quiesce().await;
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(!report.resource_closed);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == "SHUTDOWN_DISCOVERY_CHANGED" && !issue.retryable));
    assert!(std::fs::read(paths.connection_path()).unwrap() == connection);
    assert!(std::fs::read(paths.token_path()).unwrap() == token);
    assert!(service.shutdown().await.is_err());
    tokio::time::sleep(Duration::from_millis(30)).await;
    let owner = weak.upgrade().unwrap();
    assert_eq!(owner.status().attempt, report.attempt);
    assert_eq!(owner.status().status, ShutdownStatus::Blocked);
    assert!(std::fs::read(paths.connection_path()).unwrap() == connection);
    assert!(std::fs::read(paths.token_path()).unwrap() == token);
    assert!(paths.acquire_lock().is_err());
    assert!(!owner.wait().await.is_complete());
}

#[tokio::test]
async fn valid_foreign_instance_discovery_never_claims_cleanup_success() {
    changed_discovery_is_preserved(false).await;
}

#[tokio::test]
async fn mismatched_control_credential_never_claims_cleanup_success() {
    changed_discovery_is_preserved(true).await;
}
