//! Normal-Quit orchestration only. ReadyForNativeExit deliberately leaves the
//! diagnostic manager to Tauri; it is never an installer/WebKit-complete permit.
#[cfg(target_os = "macos")]
use platform_update_handoff::{DrainProof, SessionBinding};
use serde::Serialize;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{sync::watch, task::JoinHandle};

/// Strict success of the four observed jobs and fresh core state, bound to one
/// pre-armed session. This does not prove native manager/WebKit/process exit.
/// Never construct this from a Report, Ready status, JSON or resource_closed.
#[cfg(target_os = "macos")]
#[allow(dead_code)] // Internal adapter point; deliberately not runtime-wired.
pub(super) struct CleanDrainReceipt {
    binding: SessionBinding,
    attempt: u64,
}
#[cfg(target_os = "macos")]
impl DrainProof for CleanDrainReceipt {
    fn binding(&self) -> &SessionBinding {
        &self.binding
    }
}

type Work<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub(super) type Guard = Box<dyn Send>;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Issue {
    pub code: String,
    pub resource_closed: bool,
    pub retryable: bool,
}
impl Issue {
    pub fn new(code: &str, resource_closed: bool, retryable: bool) -> Self {
        Self {
            code: code.into(),
            resource_closed,
            retryable,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum Status {
    Pending,
    Blocked,
    ReadyForNativeExit,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Report {
    pub status: Status,
    pub phase: String,
    pub attempt: u64,
    pub issues: Vec<Issue>,
}
impl Default for Report {
    fn default() -> Self {
        Self {
            status: Status::Pending,
            phase: "running".into(),
            attempt: 0,
            issues: vec![],
        }
    }
}
#[derive(Clone, Default)]
pub(super) struct CoreState {
    pub closed: bool,
    pub blocked: bool,
    pub issues: Vec<Issue>,
}
pub(super) trait Backend: Send + Sync + 'static {
    fn start_core(&self);
    fn core_state(&self) -> CoreState;
    fn confirm_core(&self) -> Work<Result<(), Issue>>;
    fn windows(&self) -> Work<Result<(), Issue>>;
    fn nodes(&self) -> Work<Result<(), Issue>>;
    fn sessions(&self) -> Work<Result<(), Issue>>;
    fn changed(&self, report: &Report);
    fn ready(&self);
}
struct Scopes {
    windows: Mutex<Option<Guard>>,
    nodes: Mutex<Option<Guard>>,
}
#[derive(Default)]
struct Meta {
    backend: Option<Arc<dyn Backend>>,
    scopes: Option<Arc<Scopes>>,
    started: bool,
    sticky: Vec<Issue>,
    #[cfg(target_os = "macos")]
    arm_attempted: bool,
    #[cfg(target_os = "macos")]
    handoff: Option<SessionBinding>,
    #[cfg(target_os = "macos")]
    clean_drain: Option<CleanDrainReceipt>,
    #[cfg(target_os = "macos")]
    coordinator_joined: bool,
}
#[derive(Default)]
struct Job {
    handle: tokio::sync::Mutex<Option<JoinHandle<Result<(), Issue>>>>,
    result: Mutex<Option<Result<(), Issue>>>,
}
impl Job {
    fn result(&self) -> Option<Result<(), Issue>> {
        self.result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn closed(&self) -> bool {
        self.result()
            .is_some_and(|r| r.as_ref().err().is_none_or(|e| e.resource_closed))
    }
    async fn start(&self, work: impl FnOnce() -> Work<Result<(), Issue>>) {
        let mut handle = self.handle.lock().await;
        if handle.is_some() || self.result().is_some() {
            return;
        }
        *self.result.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *handle = Some(tokio::spawn(work()));
    }
    fn retry(&self) {
        let mut result = self.result.lock().unwrap_or_else(|p| p.into_inner());
        if result.as_ref().is_some_and(|r| {
            r.as_ref()
                .err()
                .is_some_and(|e| !e.resource_closed && e.retryable)
        }) {
            *result = None;
        }
    }
    async fn observe(&self) {
        let mut handle = self.handle.lock().await;
        if !handle.as_ref().is_some_and(|h| h.is_finished()) {
            return;
        }
        let result = match handle.as_mut().expect("observed handle").await {
            Ok(result) => result,
            Err(_) => Err(Issue::new(
                "DESKTOP_SHUTDOWN_WORKER_JOIN_FAILED",
                false,
                false,
            )),
        };
        *self.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
        *handle = None;
    }
}
pub(super) struct DesktopShutdown {
    meta: Mutex<Meta>,
    requests: watch::Sender<u64>,
    report: watch::Sender<Report>,
    windows: Job,
    nodes: Job,
    sessions: Job,
    core: Job,
    coordinator: Mutex<Option<JoinHandle<()>>>,
}
impl Default for DesktopShutdown {
    fn default() -> Self {
        Self {
            meta: Mutex::new(Meta::default()),
            requests: watch::channel(0).0,
            report: watch::channel(Report::default()).0,
            windows: Job::default(),
            nodes: Job::default(),
            sessions: Job::default(),
            core: Job::default(),
            coordinator: Mutex::new(None),
        }
    }
}
struct CoordinatorGuard(Option<Arc<DesktopShutdown>>);
impl Drop for CoordinatorGuard {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            {
                let mut meta = owner.meta.lock().unwrap_or_else(|p| p.into_inner());
                #[cfg(target_os = "macos")]
                {
                    meta.clean_drain = None;
                }
                meta.sticky.push(Issue::new(
                    "DESKTOP_SHUTDOWN_COORDINATOR_UNKNOWN",
                    false,
                    false,
                ));
            }
            let old = owner.status();
            // A Drop during unwinding must not call UI/backend code, which
            // might itself be the source of the panic. Publish only to our
            // owned channel and keep the actual resources reachable below.
            owner.report.send_replace(Report {
                status: Status::Blocked,
                phase: "coordinator-unknown".into(),
                attempt: old.attempt,
                issues: vec![Issue::new(
                    "DESKTOP_SHUTDOWN_COORDINATOR_UNKNOWN",
                    false,
                    false,
                )],
            });
            // Never let unwinding/executor loss destroy outstanding scopes or
            // unobserved handles and reinterpret their absence as completion.
            let _ = Arc::into_raw(owner);
        }
    }
}
impl DesktopShutdown {
    #[cfg(target_os = "macos")]
    #[allow(dead_code)] // Future trusted adapter, before requesting normal drain.
    pub fn arm_handoff(&self, binding: SessionBinding) -> Result<(), Issue> {
        let mut meta = self.meta.lock().unwrap_or_else(|p| p.into_inner());
        if meta.started || meta.arm_attempted {
            return Err(Issue::new("HANDOFF_ARM_REJECTED", false, false));
        }
        meta.arm_attempted = true;
        if platform_update_handoff::current_parent_matches(&binding).is_err() {
            return Err(Issue::new("HANDOFF_PARENT_MISMATCH", false, false));
        }
        meta.handoff = Some(binding);
        Ok(())
    }
    #[cfg(target_os = "macos")]
    #[allow(dead_code)] // Consumed once by a future typed ParentEndpoint adapter.
    pub fn take_clean_drain_receipt(&self) -> Option<CleanDrainReceipt> {
        // Poll and consume the actual owned JoinHandle, never is_finished or a
        // serialized Ready report. Pending keeps the same owner reachable.
        let joined = {
            let mut coordinator = self.coordinator.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(handle) = coordinator.as_mut() {
                let mut context = std::task::Context::from_waker(std::task::Waker::noop());
                match Pin::new(handle).poll(&mut context) {
                    std::task::Poll::Pending => None,
                    std::task::Poll::Ready(result) => {
                        coordinator.take();
                        Some(result.is_ok())
                    }
                }
            } else {
                None
            }
        };
        let mut meta = self.meta.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(joined) = joined {
            meta.coordinator_joined = joined;
        }
        if !meta.sticky.is_empty() || self.status().status != Status::ReadyForNativeExit {
            meta.clean_drain = None;
            return None;
        }
        if !meta.coordinator_joined {
            return None;
        }
        meta.clean_drain.take()
    }

    pub fn status(&self) -> Report {
        self.report.borrow().clone()
    }
    pub fn request(
        self: &Arc<Self>,
        backend: Arc<dyn Backend>,
        runtime: Option<tokio::runtime::Handle>,
        scopes: impl FnOnce() -> Result<(Guard, Guard), Issue>,
    ) -> Report {
        let mut meta = self.meta.lock().unwrap_or_else(|p| p.into_inner());
        let previous = self.status();
        if meta.started {
            if previous.status != Status::Blocked
                || previous
                    .issues
                    .iter()
                    .any(|i| !i.resource_closed && !i.retryable)
            {
                return previous;
            }
            self.requests.send_modify(|n| *n += 1);
            return previous;
        }
        let (windows, nodes) = match scopes() {
            Ok(scopes) => scopes,
            Err(issue) => {
                meta.backend = Some(backend);
                meta.started = true;
                meta.sticky.push(issue.clone());
                drop(meta);
                self.publish(Report {
                    status: Status::Blocked,
                    phase: "scope-admission".into(),
                    attempt: 1,
                    issues: vec![issue],
                });
                return self.status();
            }
        };
        meta.backend = Some(backend);
        meta.scopes = Some(Arc::new(Scopes {
            windows: Mutex::new(Some(windows)),
            nodes: Mutex::new(Some(nodes)),
        }));
        meta.started = true;
        self.requests.send_replace(1);
        drop(meta);
        let Some(runtime) = runtime else {
            self.meta
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .sticky
                .push(Issue::new(
                    "DESKTOP_SHUTDOWN_EXECUTOR_UNAVAILABLE",
                    false,
                    false,
                ));
            self.publish(Report {
                status: Status::Blocked,
                phase: "executor-unavailable".into(),
                attempt: 1,
                issues: vec![Issue::new(
                    "DESKTOP_SHUTDOWN_EXECUTOR_UNAVAILABLE",
                    false,
                    false,
                )],
            });
            let _ = Arc::into_raw(self.clone());
            return self.status();
        };
        let owner = self.clone();
        let guard = CoordinatorGuard(Some(owner.clone()));
        *self.coordinator.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(runtime.spawn(async move {
                let mut guard = guard;
                owner.drive().await;
                guard.0 = None;
            }));
        self.status()
    }
    fn publish(&self, report: Report) {
        if *self.report.borrow() == report {
            return;
        }
        self.report.send_replace(report.clone());
        let backend = self
            .meta
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .backend
            .clone();
        if let Some(backend) = backend {
            backend.changed(&report);
        }
    }
    async fn drive(self: Arc<Self>) {
        let (backend, scopes) = {
            let meta = self.meta.lock().unwrap_or_else(|p| p.into_inner());
            (
                meta.backend.as_ref().unwrap().clone(),
                meta.scopes.as_ref().unwrap().clone(),
            )
        };
        let mut requests = self.requests.subscribe();
        let mut attempt = 0;
        loop {
            requests.borrow_and_update();
            attempt += 1;
            self.publish(Report {
                status: Status::Pending,
                phase: "draining".into(),
                attempt,
                issues: vec![],
            });
            // Both actual UI/Node leases were registered before this seal. The
            // coordinator itself owns no lease and cannot wait for itself.
            backend.start_core();
            for job in [&self.windows, &self.nodes, &self.sessions, &self.core] {
                job.retry();
            }
            self.windows.start(|| backend.windows()).await;
            self.nodes.start(|| backend.nodes()).await;
            loop {
                self.windows.observe().await;
                self.nodes.observe().await;
                self.sessions.observe().await;
                self.core.observe().await;
                if self.windows.closed() {
                    scopes
                        .windows
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .take();
                }
                if self.windows.closed() && self.nodes.closed() {
                    self.sessions.start(|| backend.sessions()).await;
                }
                if self.nodes.closed() && self.sessions.closed() {
                    scopes
                        .nodes
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .take();
                }
                let core = backend.core_state();
                if core.closed
                    && self.windows.closed()
                    && self.nodes.closed()
                    && self.sessions.closed()
                {
                    self.core.start(|| backend.confirm_core()).await;
                }
                let mut issues = self
                    .meta
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .sticky
                    .clone();
                for job in [&self.windows, &self.nodes, &self.sessions, &self.core] {
                    if let Some(Err(issue)) = job.result() {
                        if !issues.contains(&issue) {
                            issues.push(issue);
                        }
                    }
                }
                if core.blocked {
                    issues.extend(core.issues.clone());
                }
                if core.closed
                    && self.windows.closed()
                    && self.nodes.closed()
                    && self.sessions.closed()
                    && self.core.closed()
                    && issues.iter().all(|i| i.resource_closed)
                {
                    self.publish(Report {
                        status: Status::ReadyForNativeExit,
                        phase: "ready-for-native-exit".into(),
                        attempt,
                        issues,
                    });
                    backend.ready();
                    #[cfg(target_os = "macos")]
                    {
                        let armed = self
                            .meta
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .handoff
                            .is_some();
                        if armed {
                            // Re-read after observing confirm_core's actual Join.
                            // Normal Quit may allow closed business errors; the
                            // strict handoff receipt never inherits that allowance.
                            let fresh = backend.core_state();
                            let mut meta = self.meta.lock().unwrap_or_else(|p| p.into_inner());
                            if meta.sticky.is_empty()
                                && fresh.closed
                                && !fresh.blocked
                                && fresh.issues.is_empty()
                                && [&self.windows, &self.nodes, &self.sessions, &self.core]
                                    .iter()
                                    .all(|job| job.result() == Some(Ok(())))
                            {
                                if let Some(binding) = meta.handoff.take() {
                                    meta.clean_drain = Some(CleanDrainReceipt { binding, attempt });
                                }
                            }
                        }
                    }
                    return;
                }
                let blocked = core.blocked || issues.iter().any(|i| !i.resource_closed);
                self.publish(Report {
                    status: if blocked {
                        Status::Blocked
                    } else {
                        Status::Pending
                    },
                    phase: if blocked {
                        "blocked"
                    } else {
                        "waiting-for-real-closure"
                    }
                    .into(),
                    attempt,
                    issues,
                });
                tokio::select! {
                    changed=requests.changed()=>{if changed.is_ok(){break;}},
                    _=tokio::time::sleep(std::time::Duration::from_millis(25))=>{},
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "desktop_shutdown_tests.rs"]
mod tests;
