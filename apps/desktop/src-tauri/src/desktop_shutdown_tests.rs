//! Same production coordinator, real IoLifetime leases/FDs and task joins.
//! Delayed window acknowledgements are modeled; no native GUI is instantiated.
use super::*;
use platform_core::io_lifetime::IoLifetime;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::oneshot;

enum Step {
    Reply(Result<(), Issue>),
    Wait(oneshot::Receiver<()>),
    Panic,
}
fn run_step(step: Step) -> Work<Result<(), Issue>> {
    Box::pin(async move {
        match step {
            Step::Reply(result) => result,
            Step::Wait(wait) => {
                wait.await.unwrap();
                Ok(())
            }
            Step::Panic => panic!("controlled artificial desktop worker panic"),
        }
    })
}
struct TestBackend {
    io: IoLifetime,
    windows: Mutex<VecDeque<Step>>,
    nodes: Mutex<VecDeque<Step>>,
    sessions: Mutex<VecDeque<Step>>,
    confirm: Mutex<VecDeque<Step>>,
    window_calls: AtomicUsize,
    node_calls: AtomicUsize,
    session_calls: AtomicUsize,
    core_calls: AtomicUsize,
    panic_on_start: AtomicBool,
    panic_on_ready: AtomicBool,
    panic_on_ready_report: AtomicBool,
    ready_hold: AtomicBool,
    ready_entered: AtomicBool,
    core_closed: Arc<AtomicBool>,
    core_blocked: AtomicBool,
    unblocked_core_issue: AtomicBool,
    clear_core_on_retry: AtomicBool,
    core_worker: Arc<tokio::sync::Mutex<Option<JoinHandle<()>>>>,
    core_joined: Arc<AtomicBool>,
    ready: AtomicBool,
    history: Mutex<Vec<Report>>,
    runtime_owner: Arc<()>,
}
impl TestBackend {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            io: IoLifetime::default(),
            windows: Mutex::new(VecDeque::new()),
            nodes: Mutex::new(VecDeque::new()),
            sessions: Mutex::new(VecDeque::new()),
            confirm: Mutex::new(VecDeque::new()),
            window_calls: AtomicUsize::new(0),
            node_calls: AtomicUsize::new(0),
            session_calls: AtomicUsize::new(0),
            core_calls: AtomicUsize::new(0),
            panic_on_start: AtomicBool::new(false),
            panic_on_ready: AtomicBool::new(false),
            panic_on_ready_report: AtomicBool::new(false),
            ready_hold: AtomicBool::new(false),
            ready_entered: AtomicBool::new(false),
            core_closed: Arc::new(AtomicBool::new(false)),
            core_blocked: AtomicBool::new(false),
            unblocked_core_issue: AtomicBool::new(false),
            clear_core_on_retry: AtomicBool::new(false),
            core_worker: Arc::new(tokio::sync::Mutex::new(None)),
            core_joined: Arc::new(AtomicBool::new(false)),
            ready: AtomicBool::new(false),
            history: Mutex::new(vec![]),
            runtime_owner: Arc::new(()),
        })
    }
    fn step(queue: &Mutex<VecDeque<Step>>) -> Step {
        queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Reply(Ok(())))
    }
}
impl Backend for TestBackend {
    fn start_core(&self) {
        assert!(
            !self.panic_on_start.load(Ordering::Acquire),
            "controlled artificial coordinator panic"
        );
        let count = self.core_calls.fetch_add(1, Ordering::SeqCst);
        if count > 0 {
            if self.clear_core_on_retry.load(Ordering::SeqCst) {
                self.core_blocked.store(false, Ordering::SeqCst);
            }
            return;
        }
        self.io.seal();
        let io = self.io.clone();
        let closed = self.core_closed.clone();
        *self.core_worker.try_lock().unwrap() = Some(tokio::spawn(async move {
            io.drain().await;
            closed.store(true, Ordering::Release);
        }));
    }
    fn core_state(&self) -> CoreState {
        let blocked = self.core_blocked.load(Ordering::Acquire);
        CoreState {
            closed: self.core_closed.load(Ordering::Acquire) && !blocked,
            blocked,
            issues: if blocked {
                vec![Issue::new("ARTIFICIAL_CORE_RETRYABLE", false, true)]
            } else if self.unblocked_core_issue.load(Ordering::Acquire) {
                vec![Issue::new("ARTIFICIAL_UNBLOCKED_CORE_ISSUE", true, false)]
            } else {
                vec![]
            },
        }
    }
    fn confirm_core(&self) -> Work<Result<(), Issue>> {
        let step = Self::step(&self.confirm);
        let worker = self.core_worker.clone();
        let joined = self.core_joined.clone();
        Box::pin(async move {
            run_step(step).await?;
            let mut worker = worker.lock().await;
            if let Some(handle) = worker.as_mut() {
                handle.await.unwrap();
                *worker = None;
                joined.store(true, Ordering::Release);
            }
            assert!(joined.load(Ordering::Acquire));
            Ok(())
        })
    }
    fn windows(&self) -> Work<Result<(), Issue>> {
        self.window_calls.fetch_add(1, Ordering::SeqCst);
        run_step(Self::step(&self.windows))
    }
    fn nodes(&self) -> Work<Result<(), Issue>> {
        self.node_calls.fetch_add(1, Ordering::SeqCst);
        run_step(Self::step(&self.nodes))
    }
    fn sessions(&self) -> Work<Result<(), Issue>> {
        self.session_calls.fetch_add(1, Ordering::SeqCst);
        run_step(Self::step(&self.sessions))
    }
    fn changed(&self, report: &Report) {
        assert!(
            !(report.status == Status::ReadyForNativeExit
                && self.panic_on_ready_report.load(Ordering::Acquire)),
            "controlled ready report panic"
        );
        self.history.lock().unwrap().push(report.clone());
    }
    fn ready(&self) {
        self.ready_entered.store(true, Ordering::Release);
        while self.ready_hold.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            !self.panic_on_ready.load(Ordering::Acquire),
            "controlled ready callback panic"
        );
        assert!(self.core_joined.load(Ordering::Acquire));
        self.ready.store(true, Ordering::Release);
    }
}
struct FileGuard {
    file: Option<std::fs::File>,
    closed: Arc<AtomicBool>,
}
impl Drop for FileGuard {
    fn drop(&mut self) {
        drop(self.file.take());
        self.closed.store(true, Ordering::Release);
    }
}
struct Fixture {
    root: tempfile::TempDir,
    backend: Arc<TestBackend>,
    owner: Arc<DesktopShutdown>,
    closed: [Arc<AtomicBool>; 2],
    admissions: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            backend: TestBackend::new(),
            owner: Arc::new(DesktopShutdown::default()),
            closed: [
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
            ],
            admissions: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn scopes(&self) -> impl FnOnce() -> Result<(Guard, Guard), Issue> + Send + 'static {
        let root = self.root.path().to_path_buf();
        let closed = self.closed.clone();
        let io = self.backend.io.clone();
        let admissions = self.admissions.clone();
        move || {
            admissions.fetch_add(1, Ordering::SeqCst);
            let guard = |index: usize| {
                let path = root.join(format!("artificial-owned-{index}"));
                std::fs::write(&path, b"artificial preserved bytes").unwrap();
                let file = std::fs::File::open(&path).unwrap();
                let lease = io.admit_cleanup().unwrap();
                Box::new(lease.hold(FileGuard {
                    file: Some(file),
                    closed: closed[index].clone(),
                })) as Guard
            };
            Ok((guard(0), guard(1)))
        }
    }
    fn request(&self) {
        self.backend.io.close();
        self.owner.request(
            self.backend.clone(),
            Some(tokio::runtime::Handle::current()),
            self.scopes(),
        );
    }
}
async fn until(check: impl Fn() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !check() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn cleanup_requires_all_actual_jobs_and_a_fresh_clean_core() {
    let f = Fixture::new();
    assert!(!f.owner.cleanup_ready());
    let (release, wait) = oneshot::channel();
    f.backend
        .windows
        .lock()
        .unwrap()
        .push_back(Step::Wait(wait));
    f.request();
    until(|| f.backend.node_calls.load(Ordering::Acquire) > 0).await;
    assert!(!f.owner.cleanup_ready());
    release.send(()).unwrap();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert!(f.owner.cleanup_ready());
    f.backend
        .unblocked_core_issue
        .store(true, Ordering::Release);
    assert!(!f.owner.cleanup_ready());
    f.backend
        .unblocked_core_issue
        .store(false, Ordering::Release);
    f.backend.core_closed.store(false, Ordering::Release);
    assert!(!f.owner.cleanup_ready());
}

#[tokio::test]
async fn normal_quit_closed_resource_errors_never_authorize_cleanup() {
    let f = Fixture::new();
    f.backend
        .nodes
        .lock()
        .unwrap()
        .push_back(Step::Reply(Err(Issue::new(
            "artificial-closed-business-error",
            true,
            false,
        ))));
    f.request();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.owner.status().status, Status::ReadyForNativeExit);
    assert!(!f.owner.cleanup_ready());
}
#[tokio::test]
async fn aggregate_scopes_block_real_io_drain_and_do_not_serialize_node_stop_behind_windows() {
    let f = Fixture::new();
    let (release, wait) = oneshot::channel();
    f.backend
        .windows
        .lock()
        .unwrap()
        .push_back(Step::Wait(wait));
    f.request();
    until(|| f.backend.node_calls.load(Ordering::SeqCst) == 1).await;
    assert!(!f.backend.core_closed.load(Ordering::Acquire));
    assert!(!f.backend.ready.load(Ordering::Acquire));
    assert!(f.closed.iter().all(|c| !c.load(Ordering::Acquire)));
    f.owner.request(
        f.backend.clone(),
        Some(tokio::runtime::Handle::current()),
        || panic!("duplicate pending request must not readmit after seal"),
    );
    assert_eq!(f.admissions.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.owner.status().status, Status::ReadyForNativeExit);
    assert!(f.closed.iter().all(|c| c.load(Ordering::Acquire)));
    assert_eq!(f.backend.session_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(f.root.path().join("artificial-owned-0")).unwrap(),
        b"artificial preserved bytes"
    );
}
#[tokio::test]
async fn retry_is_explicit_and_reuses_registered_scopes_and_known_completed_jobs() {
    let f = Fixture::new();
    f.backend
        .windows
        .lock()
        .unwrap()
        .push_back(Step::Reply(Err(Issue::new(
            "ARTIFICIAL_DESTROY_RETRY",
            false,
            true,
        ))));
    f.request();
    until(|| f.owner.status().status == Status::Blocked).await;
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert_eq!(f.backend.window_calls.load(Ordering::SeqCst), 1);
    assert!(!f.backend.ready.load(Ordering::Acquire));
    assert!(!f.closed[0].load(Ordering::Acquire));
    f.owner.request(
        f.backend.clone(),
        Some(tokio::runtime::Handle::current()),
        || panic!("retry cannot admit after core seal"),
    );
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.owner.status().attempt, 2);
    assert_eq!(f.admissions.load(Ordering::SeqCst), 1);
    assert_eq!(f.backend.node_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.backend.window_calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn core_blocked_never_becomes_exit_ready_until_positive_retry_and_join() {
    let f = Fixture::new();
    f.backend.core_blocked.store(true, Ordering::Release);
    f.backend.clear_core_on_retry.store(true, Ordering::Release);
    f.request();
    until(|| f.owner.status().status == Status::Blocked).await;
    until(|| f.backend.core_closed.load(Ordering::Acquire)).await;
    assert!(!f.backend.ready.load(Ordering::Acquire));
    f.owner.request(
        f.backend.clone(),
        Some(tokio::runtime::Handle::current()),
        || panic!("no new scope"),
    );
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.backend.core_calls.load(Ordering::SeqCst), 2);
    assert!(f.backend.core_joined.load(Ordering::Acquire));
}
#[tokio::test]
async fn worker_panic_is_sticky_keeps_resource_and_runtime_owner_and_does_not_cancel_other_worker()
{
    let f = Fixture::new();
    let (release, wait) = oneshot::channel();
    f.backend
        .windows
        .lock()
        .unwrap()
        .push_back(Step::Wait(wait));
    f.backend.nodes.lock().unwrap().push_back(Step::Panic);
    let runtime = Arc::downgrade(&f.backend.runtime_owner);
    f.request();
    until(|| f.owner.status().status == Status::Blocked).await;
    assert!(!f.backend.ready.load(Ordering::Acquire));
    release.send(()).unwrap();
    until(|| f.closed[0].load(Ordering::Acquire)).await;
    assert!(!f.closed[1].load(Ordering::Acquire));
    assert!(runtime.upgrade().is_some());
    f.owner.request(
        f.backend.clone(),
        Some(tokio::runtime::Handle::current()),
        || panic!("sticky failure cannot retry/readmit"),
    );
    assert_eq!(f.backend.node_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.owner.status().attempt, 1);
    assert!(!f.backend.ready.load(Ordering::Acquire));
}
#[tokio::test]
async fn losing_the_quit_caller_does_not_cancel_the_owned_coordinator() {
    let f = Fixture::new();
    let (release, wait) = oneshot::channel();
    f.backend
        .windows
        .lock()
        .unwrap()
        .push_back(Step::Wait(wait));
    f.request();
    let weak = Arc::downgrade(&f.owner);
    let backend = f.backend.clone();
    drop(f.owner);
    until(|| backend.node_calls.load(Ordering::SeqCst) == 1).await;
    assert!(weak.upgrade().is_some());
    assert!(!backend.ready.load(Ordering::Acquire));
    release.send(()).unwrap();
    until(|| backend.ready.load(Ordering::Acquire)).await;
}
#[tokio::test]
async fn native_event_thread_without_current_tokio_uses_the_injected_executor() {
    let f = Fixture::new();
    let runtime = tokio::runtime::Handle::current();
    let owner = f.owner.clone();
    let backend = f.backend.clone();
    let scopes = f.scopes();
    f.backend.io.close();
    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        owner.request(backend, Some(runtime), scopes);
    })
    .join()
    .unwrap();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.admissions.load(Ordering::SeqCst), 1);
}
#[test]
fn missing_executor_is_blocked_and_preserves_real_scopes_without_panicking() {
    let f = Fixture::new();
    f.backend.io.close();
    f.owner.request(f.backend.clone(), None, f.scopes());
    assert_eq!(f.owner.status().status, Status::Blocked);
    assert!(f
        .owner
        .status()
        .issues
        .iter()
        .any(|i| i.code == "DESKTOP_SHUTDOWN_EXECUTOR_UNAVAILABLE"));
    assert!(f.closed.iter().all(|c| !c.load(Ordering::Acquire)));
    assert!(!f.backend.ready.load(Ordering::Acquire));
}
#[tokio::test]
async fn known_closed_business_error_can_exit_but_still_waits_for_core_driver_join() {
    let f = Fixture::new();
    #[cfg(target_os = "macos")]
    f.owner.arm_handoff(handoff_binding(&f)).unwrap();
    f.backend
        .nodes
        .lock()
        .unwrap()
        .push_back(Step::Reply(Err(Issue::new(
            "ARTIFICIAL_UNCLEAN_BUT_CLOSED",
            true,
            false,
        ))));
    f.request();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.owner.status().status, Status::ReadyForNativeExit);
    assert!(f
        .owner
        .status()
        .issues
        .iter()
        .any(|i| i.code == "ARTIFICIAL_UNCLEAN_BUT_CLOSED" && i.resource_closed));
    assert!(f.backend.core_joined.load(Ordering::Acquire));
    #[cfg(target_os = "macos")]
    assert!(f.owner.take_clean_drain_receipt().is_none());
}

#[cfg(target_os = "macos")]
fn handoff_binding(f: &Fixture) -> SessionBinding {
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    platform_update_handoff::SessionDirectory::create_in(
        &std::fs::File::open(f.root.path()).unwrap(),
    )
    .unwrap()
    .binding()
    .clone()
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn strict_handoff_receipt_requires_actual_confirmation_join_and_is_consumed_once() {
    let f = Fixture::new();
    let binding = handoff_binding(&f);
    f.owner.arm_handoff(binding.clone()).unwrap();
    assert!(f.owner.arm_handoff(binding.clone()).is_err());
    let (release, wait) = oneshot::channel();
    f.backend
        .confirm
        .lock()
        .unwrap()
        .push_back(Step::Wait(wait));
    f.request();
    until(|| f.backend.core_closed.load(Ordering::Acquire)).await;
    assert!(f.owner.take_clean_drain_receipt().is_none());
    assert!(!f.backend.core_joined.load(Ordering::Acquire));
    release.send(()).unwrap();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    until(|| {
        f.owner
            .coordinator
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|h| h.is_finished())
    })
    .await;
    let receipt = f.owner.take_clean_drain_receipt().unwrap();
    assert_eq!(receipt.binding(), &binding);
    assert_eq!(receipt.attempt, 1);
    assert!(f.owner.take_clean_drain_receipt().is_none());
    assert!(f.owner.arm_handoff(binding).is_err());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn ready_report_cannot_mint_receipt_late_or_hide_unblocked_core_issues() {
    let f = Fixture::new();
    f.backend
        .unblocked_core_issue
        .store(true, Ordering::Release);
    f.owner.arm_handoff(handoff_binding(&f)).unwrap();
    f.request();
    until(|| f.backend.ready.load(Ordering::Acquire)).await;
    assert_eq!(f.owner.status().status, Status::ReadyForNativeExit);
    assert!(f.owner.status().issues.is_empty());
    assert!(f.owner.take_clean_drain_receipt().is_none());
    let late = Fixture::new();
    late.request();
    until(|| late.backend.ready.load(Ordering::Acquire)).await;
    assert!(late.owner.arm_handoff(handoff_binding(&late)).is_err());
    assert!(late.owner.take_clean_drain_receipt().is_none());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn ready_callback_or_report_panic_cannot_leave_a_strict_receipt_after_unknown() {
    for report_panic in [false, true] {
        let f = Fixture::new();
        f.owner.arm_handoff(handoff_binding(&f)).unwrap();
        if report_panic {
            f.backend
                .panic_on_ready_report
                .store(true, Ordering::Release);
        } else {
            f.backend.panic_on_ready.store(true, Ordering::Release);
        }
        f.request();
        until(|| f.owner.status().phase == "coordinator-unknown").await;
        assert!(f.owner.take_clean_drain_receipt().is_none());
        assert!(f.owner.meta.lock().unwrap().clean_drain.is_none());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn strict_receipt_waits_for_actual_coordinator_join_after_ready_callback_returns() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let wait = |check: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !check() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let f = Fixture::new();
    f.owner.arm_handoff(handoff_binding(&f)).unwrap();
    f.backend.ready_hold.store(true, Ordering::Release);
    struct ReleaseReady<'a>(&'a AtomicBool);
    impl Drop for ReleaseReady<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _release_on_failure = ReleaseReady(&f.backend.ready_hold);
    f.request();
    wait(&|| f.backend.ready_entered.load(Ordering::Acquire));
    assert_eq!(f.owner.status().status, Status::ReadyForNativeExit);
    assert!(f.owner.take_clean_drain_receipt().is_none());
    f.backend.ready_hold.store(false, Ordering::Release);
    wait(&|| {
        f.owner
            .coordinator
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|h| h.is_finished())
    });
    assert!(f.owner.take_clean_drain_receipt().is_some());
    assert!(f.owner.meta.lock().unwrap().coordinator_joined);
    assert!(f.owner.take_clean_drain_receipt().is_none());
}
#[tokio::test]
async fn final_confirmation_panic_cannot_turn_a_core_closed_snapshot_into_exit_permission() {
    let f = Fixture::new();
    f.backend.confirm.lock().unwrap().push_back(Step::Panic);
    f.request();
    until(|| f.owner.status().status == Status::Blocked).await;
    assert!(f.backend.core_closed.load(Ordering::Acquire));
    assert!(!f.backend.ready.load(Ordering::Acquire));
    assert!(f
        .owner
        .status()
        .issues
        .iter()
        .any(|i| i.code == "DESKTOP_SHUTDOWN_WORKER_JOIN_FAILED"));
}

#[tokio::test]
async fn unexpected_coordinator_panic_publishes_sticky_unknown_and_keeps_both_scopes() {
    let f = Fixture::new();
    f.backend.panic_on_start.store(true, Ordering::Release);
    f.request();
    until(|| f.owner.status().status == Status::Blocked).await;
    let report = f.owner.status();
    assert_eq!(report.phase, "coordinator-unknown");
    assert_eq!(
        report.issues[0].code,
        "DESKTOP_SHUTDOWN_COORDINATOR_UNKNOWN"
    );
    assert!(!report.issues[0].retryable);
    assert!(!f.backend.ready.load(Ordering::Acquire));
    assert!(f.closed.iter().all(|v| !v.load(Ordering::Acquire)));
    f.request();
    assert_eq!(f.admissions.load(Ordering::Acquire), 1);
    assert_eq!(f.owner.status(), report);
    let runtime = Arc::downgrade(&f.backend.runtime_owner);
    drop(f.owner);
    drop(f.backend);
    assert!(runtime.upgrade().is_some());
}
