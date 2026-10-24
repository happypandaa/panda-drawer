//! Receipt-state and race tests only; not Cocoa/physical device validation.
use super::*;
#[tokio::test]
async fn queued_creation_is_rejected_after_the_same_admission_is_sealed() {
    let lifetimes = Arc::new(WindowLifetimes::default());
    let creation = Arc::new(tokio::sync::Mutex::new(()));
    let held = creation.lock().await;
    let (entered, ready) = tokio::sync::oneshot::channel();
    let gate = creation.clone();
    let windows = lifetimes.clone();
    let queued = tokio::spawn(async move {
        let _ = entered.send(());
        let _lock = gate.lock().await;
        windows.reserve("project-artificial")
    });
    ready.await.unwrap();
    lifetimes.seal();
    drop(held);
    assert!(queued.await.unwrap().is_err());
    assert!(lifetimes.active().is_empty());
}
#[tokio::test]
async fn registry_absence_and_destroy_request_do_not_acknowledge_native_destruction() {
    let windows = WindowLifetimes::default();
    let receipt = windows.reserve("project-artificial").unwrap();
    assert!(windows.reserve("project-artificial").is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), receipt.wait())
            .await
            .is_err()
    );
    assert!(!receipt.destroyed());
    windows.destroyed(&receipt, || {});
    receipt.wait().await;
    assert!(receipt.destroyed());
    assert!(windows.active().is_empty());
}
#[test]
fn late_old_destroyed_cannot_acknowledge_or_untrack_the_replacement() {
    let windows = WindowLifetimes::default();
    let old = windows.reserve("node-app-artificial").unwrap();
    let mutations = std::sync::atomic::AtomicUsize::new(0);
    windows.destroyed(&old, || {
        mutations.fetch_add(1, Ordering::SeqCst);
    });
    let fresh = windows.reserve("node-app-artificial").unwrap();
    assert_ne!(old.generation, fresh.generation);
    windows.destroyed(&old, || {
        mutations.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
    assert!(!fresh.destroyed());
    assert!(Arc::ptr_eq(
        &windows.receipt("node-app-artificial").unwrap(),
        &fresh
    ));
}
#[test]
fn failed_build_has_no_synthetic_destroyed_receipt() {
    let windows = WindowLifetimes::default();
    let receipt = windows.reserve("project-artificial").unwrap();
    receipt.failed();
    assert!(receipt.creation_failed());
    assert!(!receipt.destroyed());
    assert!(windows.reserve("project-artificial").is_err());
}
