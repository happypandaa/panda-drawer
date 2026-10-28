//! These regressions exercise the same session-ownership guards used by the
//! native callbacks. They do not prove Cocoa event delivery or audio release.
use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::oneshot;

fn fresh_session(phase: Phase) -> Arc<Mutex<Session>> {
    Arc::new(Mutex::new(Session {
        origin: Some(Url::parse("http://127.0.0.1:49152/").unwrap()),
        phase,
    }))
}

fn put(windows: &NodeWindows, id: &str, current: Arc<Mutex<Session>>) {
    windows.sessions.lock().unwrap().insert(id.into(), current);
}

#[tokio::test]
async fn abandoning_delete_does_not_wait_for_the_desktop_key_of_a_long_git_operation() {
    use serde_json::json;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let data = root.join("disposable-cancel-data");
    let source = root.join("disposable-cancel-source");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir_all(source.join("public")).unwrap();
    std::fs::create_dir(source.join("server")).unwrap();
    std::fs::write(source.join("public/index.html"), "disposable").unwrap();
    std::fs::write(source.join("server/main.mjs"), "// never executed").unwrap();
    std::fs::write(
        source.join("local-app.json"),
        serde_json::to_vec(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"test.disposable.cancel","name":"Disposable cancellation","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
        }))
        .unwrap(),
    )
    .unwrap();
    let apps = platform_core::node_apps::NodeApps::new(data, "/disposable/no-node".into()).unwrap();
    let id = apps.register(source.clone()).await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let windows = Arc::new(NodeWindows::default());
    let _git = apps.admit_operation(&id, "Git 操作").unwrap();
    let _git_key = windows.instance_operation(&id).await;
    let input = serde_json::from_value(
        json!({"expectedRevision":apps.delete_preview(&id).await.unwrap()["expectedRevision"]}),
    )
    .unwrap();
    let before_windows = windows.clone();
    let before_id = id.clone();
    let (entered, observed) = oneshot::channel();
    let task = apps
        .submit_deletion_with(
            &id,
            input,
            Box::new(move |permit| {
                Box::pin(async move {
                    assert!(
                        deletion_window_operation(&before_windows, &before_id, &permit)
                            .await
                            .is_none()
                    );
                    entered.send(()).unwrap();
                    Ok(())
                })
            }),
        )
        .await
        .unwrap();
    apps.abandon_deletion(
        task["taskId"].as_str().unwrap(),
        &id,
        task["journalRevision"].as_u64().unwrap(),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), observed)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if apps
                .deletion_task_status(task["taskId"].as_str().unwrap(), &id)
                .unwrap()["phase"]
                == "abandoned"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(source.join("server/main.mjs").exists());
    assert!(apps.admit_window_operation(&id, "用户后来打开").is_ok());
}

#[tokio::test]
async fn delayed_old_cleanup_cannot_stop_a_replacement_generation() {
    let windows = Arc::new(NodeWindows::default());
    let old = fresh_session(Phase::Closing);
    put(&windows, "same-application", old.clone());
    let operation = windows.instance_operation("same-application").await;
    let stopped = Arc::new(AtomicUsize::new(0));
    let (queued, observed) = oneshot::channel();
    let cleanup_windows = windows.clone();
    let cleanup_stopped = stopped.clone();
    let cleanup = tokio::spawn(async move {
        queued.send(()).unwrap();
        let _operation = cleanup_windows.instance_operation("same-application").await;
        if cleanup_windows.owns("same-application", &old) {
            cleanup_stopped.fetch_add(1, Ordering::SeqCst);
        }
    });
    observed.await.unwrap();
    // This models a delayed OS callback that outlives its native container.
    let replacement = fresh_session(Phase::Running);
    put(&windows, "same-application", replacement.clone());
    drop(operation);
    cleanup.await.unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 0);
    assert!(windows.owns("same-application", &replacement));
}

#[tokio::test]
async fn delayed_cleanup_still_stops_its_own_generation() {
    let windows = Arc::new(NodeWindows::default());
    let current = fresh_session(Phase::Closing);
    put(&windows, "same-application", current.clone());
    let operation = windows.instance_operation("same-application").await;
    let stopped = Arc::new(AtomicUsize::new(0));
    let (queued, observed) = oneshot::channel();
    let cleanup_windows = windows.clone();
    let cleanup_stopped = stopped.clone();
    let cleanup = tokio::spawn(async move {
        queued.send(()).unwrap();
        let _operation = cleanup_windows.instance_operation("same-application").await;
        if cleanup_windows.owns("same-application", &current) {
            cleanup_stopped.fetch_add(1, Ordering::SeqCst);
        }
    });
    observed.await.unwrap();
    drop(operation);
    cleanup.await.unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_instance_waiter_does_not_hold_shared_or_other_instance_operations() {
    let windows = Arc::new(NodeWindows::default());
    let first = windows
        .instance_operation("first-disposable-instance")
        .await;
    let queued_windows = windows.clone();
    let (queued, observed) = oneshot::channel();
    let waiter = tokio::spawn(async move {
        queued.send(()).unwrap();
        queued_windows
            .instance_operation("first-disposable-instance")
            .await
    });
    observed.await.unwrap();
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        windows.instance_operation("second-disposable-instance"),
    )
    .await
    .unwrap();
    assert!(windows
        .try_instance_operation("first-disposable-instance")
        .is_none());
    let shared = windows
        .operations
        .try_lock()
        .expect("instance waits must leave shared coordination available");
    assert!(!waiter.is_finished());
    drop(shared);
    drop(second);
    drop(first);
    drop(waiter.await.unwrap());
}

#[tokio::test]
async fn destroyed_receipt_completes_before_cleanup_can_take_the_same_instance_lock() {
    let windows = Arc::new(NodeWindows::default());
    let lifetimes = Arc::new(super::super::business_window_lifetime::WindowLifetimes::default());
    let id = "00000000-0000-4000-8000-000000000003";
    let receipt = lifetimes.reserve(&label(id).unwrap()).unwrap();
    let operation = windows.instance_operation(id).await;
    let cleanup_windows = windows.clone();
    let delivered = receipt.clone();
    let callback = tokio::spawn(async move {
        // The native callback records Destroyed synchronously. Only its later
        // runtime/session cleanup queues behind the operation that requested it.
        lifetimes.destroyed(&delivered, || {});
        let _cleanup = cleanup_windows.instance_operation(id).await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), wait_destroyed(receipt))
        .await
        .unwrap()
        .unwrap();
    assert!(!callback.is_finished());
    drop(operation);
    callback.await.unwrap();
}

#[tokio::test]
async fn shutdown_snapshot_waits_for_queued_native_creation_without_blocking_instance_stop() {
    let windows = Arc::new(NodeWindows::default());
    let registration = windows.operations.clone().lock_owned().await;
    let (release, queued) = oneshot::channel();
    let native = tokio::spawn(async move {
        // Production moves this owned guard into the queued main-thread
        // closure, so dropping its caller cannot release the reservation.
        let _registration = registration;
        queued.await.unwrap();
    });
    let snapshot_windows = windows.clone();
    let (started, observed) = oneshot::channel();
    let snapshot = tokio::spawn(async move {
        started.send(()).unwrap();
        let _shared = snapshot_windows.operations.lock().await;
        snapshot_windows.freeze_for_shutdown()
    });
    observed.await.unwrap();
    assert!(!snapshot.is_finished());
    // Native close/stop acquires only this keyed operation, independently of
    // the shared creation/snapshot lock.
    let stopping = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        windows.instance_operation("another-disposable-stopping-instance"),
    )
    .await
    .unwrap();
    assert!(!snapshot.is_finished());
    drop(stopping);
    release.send(()).unwrap();
    native.await.unwrap();
    snapshot.await.unwrap();
}

#[tokio::test]
async fn close_during_restart_wait_prevents_starting_or_navigating() {
    let windows = Arc::new(NodeWindows::default());
    let current = fresh_session(Phase::Restarting);
    put(&windows, "same-application", current.clone());
    assert!(windows.restart_allowed("same-application", &current));
    let (waiting, observed) = oneshot::channel();
    let (resume, resumed) = oneshot::channel();
    let continuation_windows = windows.clone();
    let continuation_current = current.clone();
    let continuation = tokio::spawn(async move {
        waiting.send(()).unwrap();
        resumed.await.unwrap();
        // The actual restart path checks this guard both after blanking
        // and after backend restart, before navigating the new origin.
        continuation_windows.restart_allowed("same-application", &continuation_current)
    });
    observed.await.unwrap();
    phase(&current, Phase::Closing, None);
    resume.send(()).unwrap();
    assert!(!continuation.await.unwrap());
    assert!(windows.owns("same-application", &current));
    assert!(current.lock().unwrap().phase == Phase::Closing);
}

#[test]
fn restart_guard_requires_both_current_generation_and_restarting_phase() {
    let windows = NodeWindows::default();
    let old = fresh_session(Phase::Restarting);
    put(&windows, "same-application", old.clone());
    assert!(windows.restart_allowed("same-application", &old));
    let replacement = fresh_session(Phase::Restarting);
    put(&windows, "same-application", replacement.clone());
    assert!(!windows.restart_allowed("same-application", &old));
    assert!(windows.restart_allowed("same-application", &replacement));
    for next in [
        Phase::Opening,
        Phase::Running,
        Phase::Closing,
        Phase::Failed,
    ] {
        phase(&replacement, next, None);
        assert!(!windows.restart_allowed("same-application", &replacement));
    }
}

#[test]
fn shutdown_freezes_navigation_and_retains_sessions_until_real_destroyed_receipts() {
    let windows = NodeWindows::default();
    let id = "00000000-0000-4000-8000-000000000001";
    let current = fresh_session(Phase::Running);
    put(&windows, id, current.clone());
    let receipts = super::super::business_window_lifetime::WindowLifetimes::default();
    let receipt = receipts.reserve(&label(id).unwrap()).unwrap();
    let captured = windows.freeze_for_shutdown();
    assert!(closing(&current));
    assert!(current.lock().unwrap().origin.is_none());
    assert_eq!(
        windows
            .finish_shutdown(&captured, &receipts)
            .unwrap_err()
            .code,
        "NODE_WINDOW_DESTROY_UNCONFIRMED"
    );
    assert!(windows.owns(id, &current));
    receipts.destroyed(&receipt, || {});
    windows.finish_shutdown(&captured, &receipts).unwrap();
    assert!(!windows.owns(id, &current));
    // Repeating completed session cleanup is harmless and still checks receipts.
    windows.finish_shutdown(&captured, &receipts).unwrap();
}

#[test]
fn shutdown_cannot_unregister_a_replacement_session_or_synthesize_a_missing_receipt() {
    let windows = NodeWindows::default();
    let id = "00000000-0000-4000-8000-000000000002";
    let old = fresh_session(Phase::Running);
    put(&windows, id, old);
    let captured = windows.freeze_for_shutdown();
    let receipts = super::super::business_window_lifetime::WindowLifetimes::default();
    assert_eq!(
        windows
            .finish_shutdown(&captured, &receipts)
            .unwrap_err()
            .code,
        "NODE_WINDOW_DESTROY_UNCONFIRMED"
    );
    let replacement = fresh_session(Phase::Running);
    put(&windows, id, replacement.clone());
    let receipt = receipts.reserve(&label(id).unwrap()).unwrap();
    receipts.destroyed(&receipt, || {});
    assert_eq!(
        windows
            .finish_shutdown(&captured, &receipts)
            .unwrap_err()
            .code,
        "NODE_SESSION_GENERATION_CHANGED"
    );
    assert!(windows.owns(id, &replacement));
}

#[tokio::test]
async fn project_purge_removes_only_closed_session_keys_and_late_callbacks_cannot_recreate_them() {
    let windows = Arc::new(NodeWindows::default());
    let a = fresh_session(Phase::Running);
    let b = fresh_session(Phase::Running);
    put(&windows, "disposable-a", a.clone());
    put(&windows, "disposable-b", b.clone());
    let a_operation = windows.instance_operation("disposable-a").await;
    let b_operation = windows.instance_operation("disposable-b").await;
    let b_key = windows.existing_instance_mutex("disposable-b").unwrap();
    windows.seal_project("disposable-a");
    assert!(closing(&a));
    assert!(b.lock().unwrap().phase == Phase::Running);
    assert!(windows.forget_project("disposable-a").is_err());
    let queued_windows = windows.clone();
    let queued_a = a.clone();
    let (queued, observed) = oneshot::channel();
    let callback = tokio::spawn(async move {
        let key = queued_windows
            .existing_instance_mutex("disposable-a")
            .unwrap();
        queued.send(()).unwrap();
        let _operation = key.lock_owned().await;
        assert!(!queued_windows.owns("disposable-a", &queued_a));
    });
    observed.await.unwrap();
    windows.sessions.lock().unwrap().remove("disposable-a");
    windows.forget_project("disposable-a").unwrap();
    drop(a_operation);
    callback.await.unwrap();
    assert!(windows
        .existing_instance_operation("disposable-a")
        .await
        .is_none());
    assert!(windows.try_instance_operation("disposable-a").is_none());
    assert_eq!(windows.instance_operations.lock().unwrap().len(), 1);
    assert!(Arc::ptr_eq(
        &windows.existing_instance_mutex("disposable-b").unwrap(),
        &b_key
    ));
    assert!(windows.owns("disposable-b", &b));
    drop(b_operation);
}
