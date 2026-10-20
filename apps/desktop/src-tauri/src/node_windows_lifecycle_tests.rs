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
async fn delayed_old_cleanup_cannot_stop_a_replacement_generation() {
    let windows = Arc::new(NodeWindows::default());
    let old = fresh_session(Phase::Closing);
    put(&windows, "same-application", old.clone());
    let operation = windows.operations.lock().await;
    let stopped = Arc::new(AtomicUsize::new(0));
    let (queued, observed) = oneshot::channel();
    let cleanup_windows = windows.clone();
    let cleanup_stopped = stopped.clone();
    let cleanup = tokio::spawn(async move {
        queued.send(()).unwrap();
        let _operation = cleanup_windows.operations.lock().await;
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
    let operation = windows.operations.lock().await;
    let stopped = Arc::new(AtomicUsize::new(0));
    let (queued, observed) = oneshot::channel();
    let cleanup_windows = windows.clone();
    let cleanup_stopped = stopped.clone();
    let cleanup = tokio::spawn(async move {
        queued.send(()).unwrap();
        let _operation = cleanup_windows.operations.lock().await;
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
