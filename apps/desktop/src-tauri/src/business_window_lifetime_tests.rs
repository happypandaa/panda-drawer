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
        windows.reserve("app-00000000-0000-4000-8000-000000000205")
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
    let receipt = windows
        .reserve("app-00000000-0000-4000-8000-000000000205")
        .unwrap();
    assert!(windows
        .reserve("app-00000000-0000-4000-8000-000000000205")
        .is_err());
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
    let old = windows
        .reserve("app-00000000-0000-4000-8000-000000000201")
        .unwrap();
    let mutations = std::sync::atomic::AtomicUsize::new(0);
    windows.destroyed(&old, || {
        mutations.fetch_add(1, Ordering::SeqCst);
    });
    let fresh = windows
        .reserve("app-00000000-0000-4000-8000-000000000201")
        .unwrap();
    assert_ne!(old.generation, fresh.generation);
    windows.destroyed(&old, || {
        mutations.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
    assert!(!fresh.destroyed());
    assert!(Arc::ptr_eq(
        &windows
            .receipt("app-00000000-0000-4000-8000-000000000201")
            .unwrap(),
        &fresh
    ));
}
#[test]
fn failed_build_has_no_synthetic_destroyed_receipt() {
    let windows = WindowLifetimes::default();
    let receipt = windows
        .reserve("app-00000000-0000-4000-8000-000000000205")
        .unwrap();
    receipt.failed();
    assert!(receipt.creation_failed());
    assert!(!receipt.destroyed());
    assert!(windows
        .reserve("app-00000000-0000-4000-8000-000000000205")
        .is_err());
}

#[tokio::test]
async fn project_seal_revokes_queued_creation_without_blocking_another_project() {
    let windows = Arc::new(WindowLifetimes::default());
    let a = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    let queued = a.clone();
    let b = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000107")
        .unwrap();
    windows
        .seal_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    assert!(queued.check().is_err());
    assert!(windows
        .reserve_project("app-00000000-0000-4000-8000-000000000204", &queued)
        .is_err());
    assert!(windows
        .admit_project("app", "00000000-0000-4000-8000-000000000106")
        .is_err());
    let other = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000206", &b)
        .unwrap();
    assert!(!other.destroyed());
    assert!(b.check().is_ok());
    windows.seal();
    assert!(b.check().is_err());
}

#[test]
fn project_forget_requires_actual_destroyed_and_retires_only_its_own_generation() {
    let windows = WindowLifetimes::default();
    let a = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    let b = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000107")
        .unwrap();
    let receipt_a = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000204", &a)
        .unwrap();
    let receipt_b = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000206", &b)
        .unwrap();
    windows
        .seal_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    receipt_a.failed();
    assert!(windows
        .forget_project("app", "00000000-0000-4000-8000-000000000106")
        .is_err());
    assert!(windows
        .finish_project("app", "00000000-0000-4000-8000-000000000106")
        .is_err());
    assert!(windows
        .receipt("app-00000000-0000-4000-8000-000000000204")
        .is_some());
    windows.destroyed(&receipt_a, || {});
    windows
        .forget_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    windows
        .finish_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    assert!(windows
        .receipt("app-00000000-0000-4000-8000-000000000204")
        .is_none());
    assert!(windows
        .projects
        .lock()
        .unwrap()
        .keys()
        .all(|key| key.id != "00000000-0000-4000-8000-000000000106"));
    assert!(Arc::ptr_eq(
        &windows
            .receipt("app-00000000-0000-4000-8000-000000000206")
            .unwrap(),
        &receipt_b
    ));
    assert!(b.check().is_ok());
    assert!(a.check().is_err());
    // New admission represents a later, independently authorized identity;
    // removing temporary seal metadata cannot un-revoke old callback tokens.
    let replacement = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    assert!(replacement.check().is_ok());
    assert!(windows
        .reserve_project("app-00000000-0000-4000-8000-000000000204", &a)
        .is_err());
    let fresh = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000204", &replacement)
        .unwrap();
    let mutated = std::sync::atomic::AtomicBool::new(false);
    windows.destroyed(&receipt_a, || mutated.store(true, Ordering::Release));
    assert!(!mutated.load(Ordering::Acquire));
    assert!(!fresh.destroyed());
}

#[test]
fn application_identity_is_the_admission_and_receipt_boundary() {
    let windows = WindowLifetimes::default();
    let a = "00000000-0000-4000-8000-000000000401";
    let b = "00000000-0000-4000-8000-000000000402";
    let first = windows.admit_project("app", a).unwrap();
    let second = windows.admit_project("app", b).unwrap();
    windows.seal_project("app", a).unwrap();
    assert!(first.check().is_err());
    let receipt = windows
        .reserve_project(&format!("app-{b}"), &second)
        .unwrap();
    assert!(Arc::ptr_eq(
        &windows.project_receipt("app", b).unwrap().unwrap(),
        &receipt
    ));
    assert!(windows.project_receipt("app", a).unwrap().is_none());
    assert!(windows.admit_project("web", a).is_err());
    assert!(windows.admit_project("node", a).is_err());
    assert!(windows.admit_project("app", "not-a-uuid").is_err());
}

#[tokio::test]
async fn project_operation_waiter_is_revoked_and_does_not_hold_another_projects_operation() {
    let windows = Arc::new(WindowLifetimes::default());
    let a = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    let b = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000107")
        .unwrap();
    let held = a.operation().await.unwrap();
    let waiting = a.clone();
    let queued = tokio::spawn(async move { waiting.operation().await });
    tokio::task::yield_now().await;
    let cleanup_key = windows
        .seal_project("app", "00000000-0000-4000-8000-000000000106")
        .unwrap();
    assert!(cleanup_key.try_lock().is_err());
    let other = tokio::time::timeout(std::time::Duration::from_secs(1), b.operation())
        .await
        .unwrap()
        .unwrap();
    assert!(!queued.is_finished());
    drop(held);
    assert!(queued.await.unwrap().is_err());
    let cleanup = tokio::time::timeout(std::time::Duration::from_secs(1), cleanup_key.lock_owned())
        .await
        .unwrap();
    assert!(a.operation().await.is_err());
    drop(cleanup);
    drop(other);
}

#[test]
fn normal_close_latch_retries_failure_and_does_not_follow_a_replacement_window() {
    let windows = WindowLifetimes::default();
    let admission = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000109")
        .unwrap();
    let receipt = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000208", &admission)
        .unwrap();
    assert!(receipt.begin_close());
    assert!(!receipt.begin_close());
    // A download timeout leaves the native receipt live and permits another
    // explicit close attempt rather than acknowledging a synthetic destruction.
    receipt.reset_close();
    assert!(!receipt.destroyed());
    assert!(receipt.begin_close());
    assert!(windows.is_current(&receipt));
    windows.destroyed(&receipt, || {});
    let replacement = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000208", &admission)
        .unwrap();
    assert!(!windows.is_current(&receipt));
    assert!(!receipt.begin_close());
    assert!(windows.is_current(&replacement));
    assert!(replacement.begin_close());
    receipt.reset_close();
    assert!(
        !replacement.begin_close(),
        "an old callback cannot reset the new latch"
    );
    assert!(!replacement.destroyed());
}

struct ResourceDrop(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for ResourceDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

#[tokio::test]
async fn window_resource_survives_close_unknown_creation_and_lost_waiter_until_destroyed() {
    let windows = Arc::new(WindowLifetimes::default());
    let admission = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000108")
        .unwrap();
    let receipt = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000207", &admission)
        .unwrap();
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    receipt
        .hold_resource(ResourceDrop(dropped.clone()))
        .unwrap();
    assert!(receipt.begin_close());
    receipt.failed();
    let waiting = receipt.clone();
    let waiter = tokio::spawn(async move { waiting.wait().await });
    tokio::task::yield_now().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    windows
        .seal_project("app", "00000000-0000-4000-8000-000000000108")
        .unwrap();
    assert!(windows
        .forget_project("app", "00000000-0000-4000-8000-000000000108")
        .is_err());
    assert_eq!(dropped.load(Ordering::Acquire), 0);
    assert!(!receipt.destroyed());

    windows.destroyed(&receipt, || {});
    receipt.wait().await;
    assert_eq!(dropped.load(Ordering::Acquire), 1);
    windows.destroyed(&receipt, || {});
    assert_eq!(dropped.load(Ordering::Acquire), 1);
    windows
        .forget_project("app", "00000000-0000-4000-8000-000000000108")
        .unwrap();
}

#[test]
fn an_old_destroyed_callback_releases_only_its_own_resource() {
    let windows = WindowLifetimes::default();
    let old = windows
        .reserve("app-00000000-0000-4000-8000-000000000207")
        .unwrap();
    let old_dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    old.hold_resource(ResourceDrop(old_dropped.clone()))
        .unwrap();
    windows.destroyed(&old, || {});
    assert_eq!(old_dropped.load(Ordering::Acquire), 1);
    let fresh = windows
        .reserve("app-00000000-0000-4000-8000-000000000207")
        .unwrap();
    let fresh_dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    fresh
        .hold_resource(ResourceDrop(fresh_dropped.clone()))
        .unwrap();

    windows.destroyed(&old, || panic!("old callback mutated the replacement"));
    assert_eq!(old_dropped.load(Ordering::Acquire), 1);
    assert_eq!(fresh_dropped.load(Ordering::Acquire), 0);
    assert!(!fresh.destroyed());
    windows.destroyed(&fresh, || {});
    assert_eq!(fresh_dropped.load(Ordering::Acquire), 1);
}

#[test]
fn destroyed_releases_resources_after_unlocking_the_registry_and_rejects_late_attachment() {
    struct RegistryDrop {
        windows: std::sync::Weak<WindowLifetimes>,
        unlocked: Arc<AtomicBool>,
    }
    impl Drop for RegistryDrop {
        fn drop(&mut self) {
            self.unlocked.store(
                self.windows
                    .upgrade()
                    .is_some_and(|windows| windows.entries.try_lock().is_ok()),
                Ordering::Release,
            );
        }
    }
    let windows = Arc::new(WindowLifetimes::default());
    let receipt = windows
        .reserve("app-00000000-0000-4000-8000-000000000207")
        .unwrap();
    let unlocked = Arc::new(AtomicBool::new(false));
    receipt
        .hold_resource(RegistryDrop {
            windows: Arc::downgrade(&windows),
            unlocked: unlocked.clone(),
        })
        .unwrap();
    windows.destroyed(&receipt, || {});
    assert!(unlocked.load(Ordering::Acquire));
    let rejected = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    assert!(receipt
        .hold_resource(ResourceDrop(rejected.clone()))
        .is_err());
    assert_eq!(rejected.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn workspace_pause_rejects_queued_creation_but_keeps_old_documents_closable() {
    let windows = WindowLifetimes::default();
    let old = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000111")
        .unwrap();
    let receipt = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000211", &old)
        .unwrap();
    let pause = windows.pause_workspace().unwrap();
    assert!(windows.workspace_paused());
    assert!(old.check().is_err());
    assert!(windows
        .admit_project("app", "00000000-0000-4000-8000-000000000110")
        .is_err());
    assert!(windows
        .reserve_project("app-00000000-0000-4000-8000-000000000212", &old)
        .is_err());
    assert!(old.closing_operation().await.is_ok());
    assert!(!receipt.destroyed());
    drop(pause);
    assert!(!windows.workspace_paused());
    assert!(
        old.check().is_err(),
        "old queue cannot revive after an abort"
    );
    assert!(old.closing_operation().await.is_ok());
    let fresh = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000110")
        .unwrap();
    assert!(windows
        .reserve_project("app-00000000-0000-4000-8000-000000000209", &fresh)
        .is_ok());
    windows
        .seal_project("app", "00000000-0000-4000-8000-000000000111")
        .unwrap();
    assert!(old.closing_operation().await.is_err());
}

#[test]
fn workspace_pause_drop_never_reopens_shutdown_or_project_seals() {
    let windows = WindowLifetimes::default();
    let pause = windows.pause_workspace().unwrap();
    assert!(windows.pause_workspace().is_err());
    assert!(windows.workspace_paused());
    windows
        .seal_project("app", "00000000-0000-4000-8000-000000000104")
        .unwrap();
    drop(pause);
    assert!(windows
        .admit_project("app", "00000000-0000-4000-8000-000000000104")
        .is_err());
    let pause = windows.pause_workspace().unwrap();
    windows.seal();
    drop(pause);
    assert!(!windows.workspace_paused());
    assert!(windows
        .admit_project("app", "00000000-0000-4000-8000-000000000101")
        .is_err());
    assert!(windows.pause_workspace().is_err());
}

#[test]
fn obsolete_marking_keeps_resources_and_only_current_app_generations_clear_it() {
    let windows = WindowLifetimes::default();
    let web = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000105")
        .unwrap();
    let node = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000102")
        .unwrap();
    let web = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000210", &web)
        .unwrap();
    let node = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000203", &node)
        .unwrap();
    let released = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    node.hold_resource(ResourceDrop(released.clone())).unwrap();
    windows.obsolete_workspace(Some("app"));
    assert!(node.obsolete());
    assert!(web.obsolete());
    assert!(!node.destroyed());
    assert_eq!(released.load(Ordering::Acquire), 0);
    let pause = windows.pause_workspace().unwrap();
    assert!(windows.acknowledge_restart(&node).is_err());
    drop(pause);
    windows.acknowledge_restart(&node).unwrap();
    assert!(!node.obsolete());
    windows.obsolete_workspace(None);
    assert!(web.obsolete());
    assert!(node.obsolete());
    windows.acknowledge_restart(&web).unwrap();
    assert!(!web.obsolete());
    assert_eq!(released.load(Ordering::Acquire), 0);
    windows.destroyed(&node, || {});
    let admission = windows
        .admit_project("app", "00000000-0000-4000-8000-000000000102")
        .unwrap();
    let fresh = windows
        .reserve_project("app-00000000-0000-4000-8000-000000000203", &admission)
        .unwrap();
    windows.obsolete_workspace(Some("app"));
    assert!(windows.acknowledge_restart(&node).is_err());
    assert!(fresh.obsolete());
    assert_eq!(released.load(Ordering::Acquire), 1);
}
