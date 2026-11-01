//! An owned coordinator retains the instance and unfinished handles even after
//! a consuming shutdown caller receives an error or loses its future.
use super::*;
use crate::shutdown::{ShutdownIssue, ShutdownReport, ShutdownStatus};
use futures_util::FutureExt;

pub(super) struct RuntimeShutdown {
    shared: Arc<RuntimeSwitch>,
    resources: tokio::sync::Mutex<Resources>,
    coordination: Mutex<Coordination>,
    progress: watch::Sender<ShutdownReport>,
    retry: Notify,
    completion: Arc<Completion>,
}
struct Completion {
    done: AtomicBool,
    changed: Notify,
}
struct Coordination {
    runner: Option<JoinHandle<()>>,
    report: ShutdownReport,
}
struct Resources {
    signal: Option<oneshot::Sender<()>>,
    http: Option<JoinHandle<std::io::Result<()>>>,
    migration: Vec<JoinHandle<()>>,
    // Deliberately held after quiesce; release only when the successfully closed
    // service owner is dropped. Failed owned coordinators keep their owner alive.
    _instance_lock: InstanceLock,
    states: Vec<Arc<HttpState>>,
    stage: u8,
    issues: Vec<ShutdownIssue>,
}
impl RuntimeShutdown {
    pub(super) fn new(
        shared: Arc<RuntimeSwitch>,
        signal: oneshot::Sender<()>,
        http: JoinHandle<std::io::Result<()>>,
        lock: InstanceLock,
    ) -> Arc<Self> {
        let report = ShutdownReport::default();
        let (progress, _) = watch::channel(report.clone());
        Arc::new(Self {
            resources: tokio::sync::Mutex::new(Resources {
                signal: Some(signal),
                http: Some(http),
                migration: Vec::new(),
                _instance_lock: lock,
                states: vec![shared.snapshot()],
                stage: 0,
                issues: Vec::new(),
            }),
            shared,
            coordination: Mutex::new(Coordination {
                runner: None,
                report,
            }),
            progress,
            retry: Notify::new(),
            completion: Arc::new(Completion {
                done: AtomicBool::new(false),
                changed: Notify::new(),
            }),
        })
    }
    pub(super) fn status(&self) -> ShutdownReport {
        self.coordination
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .report
            .clone()
    }
    fn publish(&self, report: ShutdownReport) {
        self.coordination
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .report = report.clone();
        self.progress.send_replace(report);
    }
    fn phase(&self, phase: &str) {
        let mut report = self.status();
        report.phase = phase.into();
        report.status = ShutdownStatus::Pending;
        self.publish(report);
    }
    fn stop_now(&self) {
        let _submission = self
            .shared
            .migration_submission
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.shared.io_lifetime.seal();
        self.shared.processes.seal();
        self.shared.closing.store(true, Ordering::Release);
        let state = self.shared.snapshot();
        state.stopping.store(true, Ordering::SeqCst);
        let _ = state.stop.send(true);
    }
    pub(super) fn request(self: &Arc<Self>) -> ShutdownReport {
        self.stop_now();
        let mut coordination = self.coordination.lock().unwrap_or_else(|p| p.into_inner());
        if coordination.report.status == ShutdownStatus::Complete {
            return coordination.report.clone();
        }
        if coordination.runner.is_none()
            && coordination.report.status == ShutdownStatus::Blocked
            && coordination
                .report
                .issues
                .iter()
                .any(|issue| !issue.retryable)
        {
            return coordination.report.clone();
        }
        if coordination.runner.is_some() {
            if coordination.report.status == ShutdownStatus::Blocked
                && coordination
                    .report
                    .issues
                    .iter()
                    .all(|issue| issue.retryable)
            {
                coordination.report.attempt += 1;
                coordination.report.status = ShutdownStatus::Pending;
                coordination.report.phase = "retrying".into();
                self.progress.send_replace(coordination.report.clone());
                self.retry.notify_one();
            }
            return coordination.report.clone();
        }
        coordination.report.attempt = 1;
        coordination.report.phase = "signalling".into();
        self.progress.send_replace(coordination.report.clone());
        let owner = self.clone();
        let completion = self.completion.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                coordination.runner = Some(runtime.spawn(async move {
                    owner.run().await;
                    completion.done.store(true, Ordering::Release);
                    completion.changed.notify_waiters();
                }));
            }
            Err(_) => {
                coordination.report.status = ShutdownStatus::Blocked;
                coordination.report.issues.push(ShutdownIssue::new(
                    "coordinator",
                    "SHUTDOWN_RUNTIME_UNAVAILABLE",
                    "No executor is available to prove resource shutdown; instance retained",
                    false,
                    false,
                ));
                self.progress.send_replace(coordination.report.clone());
                // No executor can own the async drain. Preserve evidence/lock
                // until process termination rather than Drop deleting discovery.
                let _ = Arc::into_raw(owner);
            }
        }
        coordination.report.clone()
    }
    pub(super) fn on_service_drop(self: &Arc<Self>) {
        self.stop_now();
        // Drop may initiate a first best-effort drain, but never requests an
        // automatic retry after an explicitly reported failure.
        if self.status().attempt == 0 {
            self.request();
        }
    }
    pub(super) async fn wait(self: &Arc<Self>) -> ShutdownReport {
        let mut progress = self.progress.subscribe();
        let initial = self.request();
        // An HTTP/native operation may request stop, but must return its own
        // response/lease instead of waiting for the drain that includes itself.
        if crate::io_lifetime::in_tracked_operation() {
            return initial;
        }
        loop {
            let report = progress.borrow_and_update().clone();
            if report.status == ShutdownStatus::Complete {
                loop {
                    let finished = self.completion.changed.notified();
                    if self.completion.done.load(Ordering::Acquire) {
                        break;
                    }
                    finished.await;
                }
                return report;
            }
            if report.status == ShutdownStatus::Blocked {
                return report;
            }
            if progress.changed().await.is_err() {
                return self.status();
            }
        }
    }
    async fn run(self: Arc<Self>) {
        loop {
            let outcome = std::panic::AssertUnwindSafe(self.attempt())
                .catch_unwind()
                .await;
            match outcome {
                Ok(report) => {
                    let complete = report.is_complete();
                    self.publish(report);
                    if complete {
                        return;
                    }
                }
                Err(_) => {
                    let mut report = self.status();
                    report.status = ShutdownStatus::Blocked;
                    report.resource_closed = false;
                    report.issues.push(ShutdownIssue::new("coordinator", "SHUTDOWN_COORDINATOR_PANICKED", "Shutdown coordinator panicked; unfinished resources and instance are retained", false, false));
                    self.publish(report);
                    // A partially-polled manager join may already have moved
                    // handles. Keep its unknown result sticky while still
                    // signalling independent resources and observing HTTP.
                    let _ = std::panic::AssertUnwindSafe(self.emergency_cleanup())
                        .catch_unwind()
                        .await;
                }
            }
            // Retain the entire owner on Blocked, including after shutdown(self)
            // returns Err and drops the last external service handle.
            self.retry.notified().await;
        }
    }
    async fn emergency_cleanup(&self) {
        let mut resources = self.resources.lock().await;
        for state in &resources.states {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| signal_state(state)));
        }
        if let Ok(current) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.shared.snapshot()))
        {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| signal_state(&current)));
        }
        if let Some(signal) = resources.signal.take() {
            let _ = signal.send(());
        }
        if let Some(http) = resources.http.as_mut() {
            let issue = match http.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(ShutdownIssue::new(
                    "http",
                    "SHUTDOWN_HTTP_FAILED",
                    error.to_string(),
                    false,
                    false,
                )),
                Err(error) => Some(ShutdownIssue::join("http", error)),
            };
            if let Some(issue) = issue {
                resources.issues.push(issue.clone());
                let mut report = self.status();
                report.issues.push(issue);
                self.publish(report);
            }
            resources.http = None;
        }
        // Do not close databases after an unknown manager lifetime. Preserve
        // the state, all remaining resources and the instance lock for review.
    }
    async fn attempt(&self) -> ShutdownReport {
        let mut resources = self.resources.lock().await;
        if resources.stage == 0 {
            self.phase("migration");
            self.stop_now();
            for state in &resources.states {
                signal_state(state);
            }
            {
                let _submission = self
                    .shared
                    .migration_submission
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                resources.migration =
                    std::mem::take(&mut *self.shared.migration_previous.lock().unwrap());
                if let Some(runner) = self.shared.migration_runner.lock().unwrap().take() {
                    resources.migration.push(runner);
                }
            }
            let migrations = std::mem::take(&mut resources.migration);
            resources
                .issues
                .extend(crate::shutdown::join_all("workspaceMigration", migrations).await);
            let current = self.shared.snapshot();
            signal_state(&current);
            if !resources
                .states
                .iter()
                .any(|state| Arc::ptr_eq(state, &current))
            {
                resources.states.push(current);
            }
            if let Some(retained) = self.shared.migration_retained_state.lock().unwrap().clone() {
                signal_state(&retained);
                if !resources
                    .states
                    .iter()
                    .any(|state| Arc::ptr_eq(state, &retained))
                {
                    resources.states.push(retained);
                }
            }
            resources.issues.extend(std::mem::take(
                &mut *self.shared.shutdown_issues.lock().unwrap(),
            ));
            if let Some(signal) = resources.signal.take() {
                let _ = signal.send(());
            }
            resources.stage = 1;
        }
        if resources.stage == 1 {
            self.phase("operations");
            for state in &resources.states {
                loop {
                    let notified = state.drained.notified();
                    if state.active.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                    notified.await;
                }
            }
            self.shared.io_lifetime.drain().await;
            resources
                .issues
                .extend(self.shared.io_lifetime.shutdown_issues());
            resources.stage = 2;
        }
        // A helper's deadline cannot discard the independently owned process.
        // Retryable issues are removed only by a subsequent real drain receipt.
        // Observe this before manager joins: a manager may retain its pipeline
        // until the process actually closes before rolling back output files.
        self.phase("processes");
        let process_issues = self.shared.processes.shutdown().await;
        resources
            .issues
            .retain(|issue| !(issue.component.starts_with("process/") && issue.retryable));
        for issue in process_issues {
            if !resources.issues.contains(&issue) {
                resources.issues.push(issue);
            }
        }
        if resources
            .issues
            .iter()
            .any(|issue| issue.component.starts_with("process/") && !issue.resource_closed)
        {
            // A retained process/session can still own project gates, files or
            // SQLite-dependent work. Do not close later resources underneath it.
            return ShutdownReport {
                status: ShutdownStatus::Blocked,
                phase: "processes".into(),
                attempt: self.status().attempt,
                resource_closed: false,
                issues: resources.issues.clone(),
            };
        }
        if resources.stage == 2 {
            self.phase("managers");
            for state in resources.states.clone() {
                resources.issues.extend(drain_managers(&state).await);
            }
            resources
                .issues
                .extend(self.shared.project_cleanup.shutdown_artifacts().await);
            resources.stage = 3;
        }
        if resources.stage == 3 {
            self.phase("database");
            for state in resources.states.clone() {
                resources.issues.extend(state.store.shutdown().await);
            }
            resources.stage = 4;
        }
        if resources.stage == 4 {
            self.phase("http");
            // Axum's graceful server future waits its independently spawned
            // connections. A timeout/parent abort is never completion evidence.
            if let Some(http) = resources.http.as_mut() {
                let issue = match http.await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(ShutdownIssue::new(
                        "http",
                        "SHUTDOWN_HTTP_FAILED",
                        error.to_string(),
                        false,
                        false,
                    )),
                    Err(error) => Some(ShutdownIssue::join("http", error)),
                };
                if let Some(issue) = issue {
                    resources.issues.push(issue);
                }
            }
            resources.http = None;
            resources.stage = 5;
        }
        // Only retry the failed cleanup; already-joined None handles cannot
        // erase sticky runner errors or be reinterpreted as success.
        resources
            .issues
            .retain(|issue| !(issue.component == "discovery" && issue.retryable));
        if resources.issues.is_empty() {
            self.phase("discovery");
            let state = self.shared.snapshot();
            if let Some(issue) = cleanup_discovery(&state) {
                resources.issues.push(issue);
            }
        }
        let issues = resources.issues.clone();
        ShutdownReport {
            status: if issues.is_empty() {
                ShutdownStatus::Complete
            } else {
                ShutdownStatus::Blocked
            },
            phase: if issues.is_empty() {
                "complete".into()
            } else {
                self.status().phase
            },
            attempt: self.status().attempt,
            resource_closed: issues.iter().all(|issue| issue.resource_closed),
            issues,
        }
    }
}

// The existing guarded removal can unlink connection.json before token unlink
// fails. An orphan token must never be reinterpreted as a successful retry.
fn cleanup_discovery(state: &HttpState) -> Option<ShutdownIssue> {
    let connection = state.paths.connection_path();
    let before = std::fs::symlink_metadata(&connection);
    let result = match &before {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(anyhow::anyhow!("Discovery is not an ordinary file; retained for review"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(state.paths.token_path()) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
                _ => return Some(ShutdownIssue::new(
                    "discovery", "SHUTDOWN_DISCOVERY_PARTIAL",
                    "Discovery is absent while credential evidence remains or cannot be inspected; instance retained for review",
                    true, false,
                )),
            }
        }
        Err(error) => Err(anyhow::anyhow!("Cannot inspect discovery: {error}")),
        Ok(_) => {
            let owned = state.paths.remove_stopped_runtime_if_owned(&state.info.instance_id, &state.token);
            match owned {
                Ok(false) => return Some(changed_discovery()),
                Ok(true) => Ok(()),
                Err(error) => Err(error),
            }
        }
    };
    if result.is_ok() {
        let absent = |path| {
            std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        };
        return if absent(connection) && absent(state.paths.token_path()) {
            None
        } else {
            Some(changed_discovery())
        };
    }
    result.err().map(|error| {
        let partial = before.is_ok() && std::fs::symlink_metadata(&connection).is_err();
        ShutdownIssue::new(
            "discovery",
            if partial {
                "SHUTDOWN_DISCOVERY_PARTIAL"
            } else {
                "SHUTDOWN_DISCOVERY_FAILED"
            },
            error.to_string(),
            true,
            !partial,
        )
    })
}

fn changed_discovery() -> ShutdownIssue {
    ShutdownIssue::new("discovery", "SHUTDOWN_DISCOVERY_CHANGED", "Discovery or credential ownership changed, or cleanup did not remove both owned files; evidence and instance retained for review", false, false)
}

pub(super) fn signal_state(state: &Arc<HttpState>) {
    state.stopping.store(true, Ordering::SeqCst);
    let _ = state.stop.send(true);
    state
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
    state
        .tickets
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
    let mut issues = Vec::new();
    let mut signal = |component: &str, action: &dyn Fn()| {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err() {
            issues.push(ShutdownIssue::new(
                component,
                "SHUTDOWN_SIGNAL_FAILED",
                "Stop broadcast panicked; resources remain unconfirmed",
                false,
                false,
            ));
        }
    };
    signal("storage", &|| state.storage.request_stop());
    signal("lifecycle", &|| state.lifecycle.request_stop());
    signal("nodeGitImport", &|| state.node_git_import.request_stop());
    signal("builds", &|| state.builds.request_stop());
    signal("database", &|| state.store.request_stop());
    if let Some(shared) = state.shared.upgrade() {
        shared
            .shutdown_issues
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(issues);
    }
}
pub(super) async fn drain_managers(state: &Arc<HttpState>) -> Vec<ShutdownIssue> {
    // Separate owned tasks isolate a manager panic from every other drain.
    // All have already received stop before any runner is awaited.
    let mut runners = Vec::new();
    macro_rules! drain {
        ($component:literal, $manager:expr) => {{
            let manager = $manager.clone();
            runners.push((
                $component,
                tokio::spawn(async move { manager.shutdown().await }),
            ));
        }};
    }
    drain!("storage", state.storage);
    drain!("lifecycle", state.lifecycle);
    drain!("nodeGitImport", state.node_git_import);
    let mut issues = Vec::new();
    for (component, runner) in runners {
        match runner.await {
            Ok(report) => issues.extend(report),
            Err(error) => issues.push(ShutdownIssue::join(component, error)),
        }
    }
    // BuildManager also owns the shared task registry. Only reconcile remaining
    // non-final records after every business runner has finished writing them.
    let builds = state.builds.clone();
    match tokio::spawn(async move { builds.shutdown().await }).await {
        Ok(report) => issues.extend(report),
        Err(error) => issues.push(ShutdownIssue::join("builds", error)),
    }
    issues
}

#[cfg(test)]
#[path = "service_shutdown_tests.rs"]
mod tests;
