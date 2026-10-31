//! Synthetic event-order tests exercise only the owner state machine. The
//! opt-in native fixture separately supplies real Env5 exit receipts.
use super::*;
use platform_core::{project_cleanup::ProjectKind, webview_storage::StoreLedger};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Release(Arc<AtomicUsize>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}
fn fixture() -> (tempfile::TempDir, Arc<BrowserOwner>, Arc<AtomicUsize>) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-browser-owner-")
        .tempdir()
        .unwrap();
    let ledger = StoreLedger::new(temporary.path().canonicalize().unwrap());
    ledger
        .record(ProjectKind::Web, "prj_disposable_owner")
        .unwrap();
    let profile = Arc::new(
        ledger
            .bind_profile(ProjectKind::Web, "prj_disposable_owner")
            .unwrap(),
    );
    let owner = BrowserOwner::new("project-prj_disposable_owner".into(), 17, profile).unwrap();
    let released = Arc::new(AtomicUsize::new(0));
    owner.hold_resource(Release(released.clone())).unwrap();
    (temporary, owner, released)
}
#[test]
fn synthetic_exit_requires_the_sealed_generation_and_destroyed_callback_before_retiring() {
    let (_temporary, owner, released) = fixture();
    owner.bind_pid(101).unwrap();
    owner.browser_exited(101);
    assert!(!owner.document_allowed());
    assert!(owner.retire().is_err());
    owner.seal_pid(101).unwrap();
    assert!(owner.retire().is_err());
    assert_eq!(released.load(Ordering::Acquire), 0);
    owner.destroyed();
    assert!(!owner.closed_and_retired());
    owner.retire().unwrap();
    assert!(owner.closed_and_retired());
    assert_eq!(released.load(Ordering::Acquire), 1);
    assert!(owner.profile().is_err());
    owner.retire().unwrap();
    assert_eq!(released.load(Ordering::Acquire), 1);
}
#[test]
fn synthetic_stale_browser_exit_cannot_release_the_last_browser_or_its_lease() {
    let (_temporary, owner, released) = fixture();
    owner.bind_pid(101).unwrap();
    owner.browser_exited(101);
    owner.bind_pid(202).unwrap();
    owner.seal_pid(202).unwrap();
    assert!(!owner.document_allowed());
    owner.destroyed();
    owner.browser_exited(101);
    assert!(owner.retire().is_err());
    assert_eq!(released.load(Ordering::Acquire), 0);
    owner.browser_exited(202);
    owner.retire().unwrap();
    assert_eq!(released.load(Ordering::Acquire), 1);
}
#[test]
fn synthetic_unknown_or_unsealed_destruction_keeps_resources_and_actual_profile_pinned() {
    for fail_explicitly in [false, true] {
        let (_temporary, owner, released) = fixture();
        owner.bind_pid(101).unwrap();
        if fail_explicitly {
            owner.seal_pid(101).unwrap();
            owner.fail();
        }
        owner.destroyed();
        owner.browser_exited(101);
        assert!(owner.retire().is_err());
        assert_eq!(released.load(Ordering::Acquire), 0);
        let profile = owner.profile().unwrap();
        let parent = profile.directory().parent().unwrap();
        assert!(std::fs::rename(profile.directory(), parent.join("foreign-substitute")).is_err());
        drop(profile);
        // No engine exists in this synthetic fixture; ending its in-memory
        // owner here is not a production browser/process release assertion.
        drop(owner);
        assert_eq!(released.load(Ordering::Acquire), 1);
    }
}
#[test]
fn synthetic_pid_change_after_sealing_never_changes_the_accepted_last_pid() {
    let (_temporary, owner, released) = fixture();
    owner.bind_pid(101).unwrap();
    owner.seal_pid(101).unwrap();
    assert!(owner.bind_pid(202).is_err());
    assert!(owner.seal_pid(202).is_err());
    owner.destroyed();
    owner.browser_exited(101);
    owner.browser_exited(202);
    assert!(owner.retire().is_err());
    assert_eq!(released.load(Ordering::Acquire), 0);
}
