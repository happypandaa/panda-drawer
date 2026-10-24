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
    core_closed: Arc<AtomicBool>,
    core_blocked: AtomicBool,
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
            core_closed: Arc::new(AtomicBool::new(false)),
            core_blocked: AtomicBool::new(false),
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
        self.history.lock().unwrap().push(report.clone());
    }
    fn ready(&self) {
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
