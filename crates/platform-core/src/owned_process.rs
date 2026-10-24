//! Owned ordinary process groups and their I/O. This is not a sandbox or an
//! installer permission. A dropped reply cannot drop the actual resource owner.
use crate::{shutdown::ShutdownIssue, ApiError};
use futures_util::{future::BoxFuture, FutureExt};
use serde::Serialize;
#[cfg(unix)]
use std::process::Stdio;
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    process::{Child, ExitStatus},
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream},
    process::Command,
    sync::{oneshot, watch, Notify},
    task::JoinHandle,
    time::Instant,
};

#[path = "owned_process_group.rs"]
mod group;
use group::ProcessGroup;

/// The caller must independently retain unreaped-leader ownership throughout
/// this inspection. This function alone never authorizes numeric signals.
pub(crate) fn group_has_other_members(pid: u32) -> Result<bool, ApiError> {
    group::members(pid)
}

#[derive(Clone, Default)]
pub(crate) struct OwnedProcessRegistry(Arc<RegistryInner>);
#[derive(Default)]
struct RegistryInner {
    state: Mutex<RegistryState>,
    changed: Notify,
}
#[derive(Default)]
struct RegistryState {
    sealed: bool,
    records: BTreeMap<String, Arc<dyn ErasedRecord>>,
    #[cfg(test)]
    submission_pause: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ProcessPhase {
    Running,
    Stopping,
    CleanupPending,
    Unconfirmed,
    Closed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessSnapshot {
    pub id: String,
    pub component: String,
    pub pid: u32,
    pub phase: ProcessPhase,
    pub leader_reaped: bool,
    pub group_exited: bool,
    pub io_joined: bool,
    pub driver_joined: bool,
    pub issues: Vec<ShutdownIssue>,
    pub resolved_issues: Vec<ShutdownIssue>,
}
impl ProcessSnapshot {
    pub fn resource_closed(&self) -> bool {
        self.phase == ProcessPhase::Closed
            && self.leader_reaped
            && self.group_exited
            && self.io_joined
            && self.driver_joined
            && self.issues.is_empty()
    }
}

pub(crate) struct ProcessOptions {
    pub component: String,
    pub work_timeout: Duration,
    pub cleanup_timeout: Duration,
    pub terminate_grace: Duration,
}
impl ProcessOptions {
    pub fn new(component: &str, work_timeout: Duration) -> Self {
        Self {
            component: component.into(),
            work_timeout,
            cleanup_timeout: Duration::from_secs(2),
            terminate_grace: Duration::from_millis(100),
        }
    }
}

/// Sessions own their streams and candidate files. Independent stderr/output
/// readers must be separate jobs, so one failed job cannot drop another reader.
pub(crate) struct ProcessPipes {
    pub stdin: DuplexStream,
    pub stdout: DuplexStream,
    pub stderr: DuplexStream,
    pub stop: watch::Receiver<bool>,
}
pub(crate) struct IoPlan<T> {
    session: BoxFuture<'static, Result<T, ApiError>>,
    drains: Vec<(String, BoxFuture<'static, Result<(), ApiError>>)>,
}
impl<T: Send + 'static> IoPlan<T> {
    pub fn new(session: impl Future<Output = Result<T, ApiError>> + Send + 'static) -> Self {
        Self {
            session: Box::pin(session),
            drains: vec![],
        }
    }
    pub fn drain(
        mut self,
        name: &str,
        future: impl Future<Output = Result<(), ApiError>> + Send + 'static,
    ) -> Self {
        self.drains.push((name.into(), Box::pin(future)));
        self
    }
}
#[derive(Debug)]
pub(crate) struct ProcessOutput<T> {
    pub status: ExitStatus,
    pub value: T,
}
type ProcessResult<T> = Result<ProcessOutput<T>, ApiError>;

pub(crate) struct ProcessRun<T> {
    reply: oneshot::Receiver<ProcessResult<T>>,
    owner: Arc<dyn ErasedRecord>,
    received: bool,
}
impl<T> ProcessRun<T> {
    pub fn cancel(&self) {
        self.owner.stop();
    }
    pub fn snapshot(&self) -> ProcessSnapshot {
        self.owner.snapshot()
    }
}
impl<T> Future for ProcessRun<T> {
    type Output = ProcessResult<T>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.reply).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(value) => {
                this.received = true;
                Poll::Ready(value.unwrap_or_else(|_| {
                    Err(failure(
                        "PROCESS_REPLY_LOST",
                        "Process owner did not deliver a verified result",
                    ))
                }))
            }
        }
    }
}
impl<T> Drop for ProcessRun<T> {
    fn drop(&mut self) {
        if !self.received {
            self.owner.stop();
        }
    }
}

trait ErasedRecord: Send + Sync {
    fn snapshot(&self) -> ProcessSnapshot;
    fn stop(&self);
    fn join_driver(self: Arc<Self>) -> BoxFuture<'static, ()>;
}
enum IoValue<T> {
    Session(T),
    Drain,
}
struct IoOwner<T> {
    name: String,
    handle: Option<JoinHandle<Result<IoValue<T>, ApiError>>>,
    joined: bool,
}
struct Resources<T, G> {
    child: Option<Child>,
    group: ProcessGroup,
    io: Vec<IoOwner<T>>,
    output: Option<T>,
    exit: Option<ExitStatus>,
    keep_alive: Option<G>,
}
struct Record<T, G> {
    registry: Weak<RegistryInner>,
    options: ProcessOptions,
    meta: Mutex<ProcessSnapshot>,
    resources: tokio::sync::Mutex<Resources<T, G>>,
    driver: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    reply: Mutex<Option<oneshot::Sender<ProcessResult<T>>>>,
    cancel: watch::Sender<bool>,
}

fn failure(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message).outcome("unknown")
}
fn not_started(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message).outcome("not_started")
}
fn component(label: &str) -> String {
    format!("process/{label}")
}

#[cfg(unix)]
type OsPipe = tokio::net::UnixStream;
#[cfg(not(unix))]
type OsPipe = DuplexStream;

/// Register I/O before a child can exist. Tokio's Command::spawn performs its
/// reactor/SIGCHLD registration after spawning; its error cannot return ownership
/// of that std Child. Keep std Child directly and reap only after WNOWAIT proof.
fn prepare_stdio(command: &mut Command) -> Result<(OsPipe, OsPipe, OsPipe), ApiError> {
    #[cfg(unix)]
    {
        use std::os::{fd::OwnedFd, unix::net::UnixStream};
        fn channel() -> Result<(OsPipe, Stdio), ApiError> {
            let (parent, child) = UnixStream::pair()?;
            parent.set_nonblocking(true)?;
            let parent = tokio::net::UnixStream::from_std(parent)?;
            Ok((parent, Stdio::from(OwnedFd::from(child))))
        }
        // A runtime with no I/O driver panics while registering, before spawn.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drop(tokio::time::sleep(Duration::ZERO));
            let (input, child_input) = channel()?;
            let (output, child_output) = channel()?;
            let (diagnostics, child_diagnostics) = channel()?;
            command
                .stdin(child_input)
                .stdout(child_output)
                .stderr(child_diagnostics);
            Ok((input, output, diagnostics))
        }))
        .unwrap_or_else(|_| {
            Err(not_started(
                "PROCESS_EXECUTOR_UNAVAILABLE",
                "No I/O executor can own process streams",
            ))
        })
        .map_err(|error| {
            if error.code == "PROCESS_EXECUTOR_UNAVAILABLE" {
                error
            } else {
                not_started(
                    "PROCESS_STDIO_UNAVAILABLE",
                    "Could not prepare owned process streams before spawn",
                )
            }
        })
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        Err(not_started(
            "PROCESS_PLATFORM_UNSUPPORTED",
            "Owned process streams require macOS",
        ))
    }
}

/// A closed consumer switches to a sink; it never abandons the actual reader.
async fn pump_output(
    mut reader: impl AsyncRead + Unpin,
    mut consumer: DuplexStream,
) -> Result<(), ApiError> {
    let mut bytes = [0u8; 8192];
    let mut consuming = true;
    loop {
        let count = reader.read(&mut bytes).await?;
        if count == 0 {
            return Ok(());
        }
        if consuming && consumer.write_all(&bytes[..count]).await.is_err() {
            consuming = false;
        }
    }
}

impl OwnedProcessRegistry {
    pub(crate) fn shares_registry(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Admission and spawn/registration linearize with seal on this same lock.
    /// The keep-alive must contain the operation lease and disposable directory
    /// owners that would otherwise disappear when the result waiter is dropped.
    pub fn start<T: Send + 'static, G: Send + 'static>(
        &self,
        mut command: Command,
        options: ProcessOptions,
        keep_alive: G,
        make_io: impl FnOnce(ProcessPipes) -> IoPlan<T> + Send + 'static,
    ) -> Result<ProcessRun<T>, ApiError> {
        if !cfg!(target_os = "macos") {
            return Err(not_started(
                "PROCESS_PLATFORM_UNSUPPORTED",
                "Owned process group verification currently requires macOS",
            ));
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            not_started(
                "PROCESS_EXECUTOR_UNAVAILABLE",
                "No executor can own the process drain",
            )
        })?;
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.sealed {
            return Err(not_started(
                "SERVICE_STOPPING",
                "Process admission is sealed",
            ));
        }
        #[cfg(test)]
        if let Some((entered, release)) = state.submission_pause.take() {
            entered.send(()).expect("fixture receives admission signal");
            release.recv().expect("fixture releases admission");
        }
        let (stdin, stdout, stderr) = prepare_stdio(&mut command)?;
        #[cfg(unix)]
        command.process_group(0);
        let child = command.as_std_mut().spawn().map_err(|_| {
            not_started("PROCESS_SPAWN_FAILED", "Could not start the owned command")
        })?;
        let pid = child.id();
        drop(command);
        let (cancel, stop) = watch::channel(false);
        let (input, mut input_pump) = tokio::io::duplex(64 * 1024);
        let (output, output_pump) = tokio::io::duplex(64 * 1024);
        let (diagnostics, diagnostics_pump) = tokio::io::duplex(64 * 1024);
        let pipes = ProcessPipes {
            stdin: input,
            stdout: output,
            stderr: diagnostics,
            stop,
        };
        let (reply, receiver) = oneshot::channel();
        let record = Arc::new(Record {
            registry: Arc::downgrade(&self.0),
            meta: Mutex::new(ProcessSnapshot {
                id: uuid::Uuid::new_v4().to_string(),
                component: options.component.clone(),
                pid,
                phase: ProcessPhase::Running,
                leader_reaped: false,
                group_exited: false,
                io_joined: false,
                driver_joined: false,
                issues: vec![],
                resolved_issues: vec![],
            }),
            resources: tokio::sync::Mutex::new(Resources {
                child: Some(child),
                group: ProcessGroup::new(pid),
                io: vec![],
                output: None,
                exit: None,
                keep_alive: Some(keep_alive),
            }),
            driver: tokio::sync::Mutex::new(None),
            reply: Mutex::new(Some(reply)),
            cancel,
            options,
        });
        let erased: Arc<dyn ErasedRecord> = record.clone();
        state.records.insert(record.snapshot().id, erased.clone());
        let mut resources = record
            .resources
            .try_lock()
            .expect("new record is not being driven");
        // The actual OS pipes always belong to independently registered pumps.
        // A factory panic or a session that drops its facade cannot drop them.
        let mut stdin = stdin;
        resources.io.push(IoOwner {
            name: "stdin-pump".into(),
            handle: Some(runtime.spawn(async move {
                tokio::io::copy(&mut input_pump, &mut stdin).await?;
                if let Err(error) = stdin.shutdown().await {
                    if error.kind() != std::io::ErrorKind::NotConnected {
                        return Err(error.into());
                    }
                }
                Ok(IoValue::Drain)
            })),
            joined: false,
        });
        resources.io.push(IoOwner {
            name: "stdout-pump".into(),
            handle: Some(runtime.spawn(async move {
                pump_output(stdout, output_pump)
                    .await
                    .map(|()| IoValue::Drain)
            })),
            joined: false,
        });
        resources.io.push(IoOwner {
            name: "stderr-pump".into(),
            handle: Some(runtime.spawn(async move {
                pump_output(stderr, diagnostics_pump)
                    .await
                    .map(|()| IoValue::Drain)
            })),
            joined: false,
        });
        drop(resources);
        let owner = record.clone();
        *record.driver.try_lock().expect("new driver slot") = Some(runtime.spawn(async move {
            owner.run(pipes, make_io).await;
        }));
        drop(state);
        Ok(ProcessRun {
            reply: receiver,
            owner: erased,
            received: false,
        })
    }

    pub fn seal(&self) {
        let records = {
            let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
            state.sealed = true;
            state.records.values().cloned().collect::<Vec<_>>()
        };
        for record in records {
            record.stop();
        }
        self.0.changed.notify_waiters();
    }

    pub fn snapshot(&self) -> Vec<ProcessSnapshot> {
        self.0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .records
            .values()
            .map(|r| r.snapshot())
            .collect()
    }

    /// A cleanup deadline returns an issue while its owned driver keeps running.
    /// A subsequent call may resolve that issue only with complete real receipts.
    pub async fn shutdown(&self) -> Vec<ShutdownIssue> {
        self.seal();
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let records = self
                .0
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .records
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for record in records {
                record.join_driver().await;
            }
            let snapshots = self.snapshot();
            if snapshots
                .iter()
                .any(|s| s.phase == ProcessPhase::Closed && !s.driver_joined)
            {
                continue;
            }
            if snapshots.iter().all(|s| {
                matches!(
                    s.phase,
                    ProcessPhase::Closed | ProcessPhase::CleanupPending | ProcessPhase::Unconfirmed
                )
            }) {
                return snapshots.into_iter().flat_map(|s| s.issues).collect();
            }
            changed.await;
        }
    }
}

impl<T: Send + 'static, G: Send + 'static> Record<T, G> {
    fn notify(&self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.changed.notify_waiters();
        }
    }
    fn update(&self, change: impl FnOnce(&mut ProcessSnapshot)) {
        change(&mut self.meta.lock().unwrap_or_else(|p| p.into_inner()));
        self.notify();
    }
    fn issue(&self, code: &str, message: &str, retryable: bool) {
        self.update(|meta| {
            if !meta.issues.iter().any(|i| i.code == code) {
                meta.issues.push(ShutdownIssue::new(
                    &component(&meta.component),
                    code,
                    message,
                    false,
                    retryable,
                ));
            }
        });
    }
    fn send(&self, result: ProcessResult<T>) {
        if let Some(reply) = self.reply.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = reply.send(result);
        }
    }
    async fn run(
        self: Arc<Self>,
        pipes: ProcessPipes,
        make_io: impl FnOnce(ProcessPipes) -> IoPlan<T>,
    ) {
        let plan = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make_io(pipes)));
        {
            let mut resources = self.resources.lock().await;
            match plan {
                Ok(plan) => {
                    resources.io.push(IoOwner {
                        name: "session".into(),
                        handle: Some(tokio::spawn(async move {
                            plan.session.await.map(IoValue::Session)
                        })),
                        joined: false,
                    });
                    for (name, drain) in plan.drains {
                        resources.io.push(IoOwner {
                            name,
                            handle: Some(tokio::spawn(async move {
                                drain.await.map(|()| IoValue::Drain)
                            })),
                            joined: false,
                        });
                    }
                }
                Err(_) => {
                    self.issue(
                        "PROCESS_IO_BUILDER_PANICKED",
                        "Process I/O builder panicked; owner retained",
                        false,
                    );
                    self.cancel.send_replace(true);
                }
            }
        }
        let result = std::panic::AssertUnwindSafe(self.drive())
            .catch_unwind()
            .await;
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error,
            Err(_) => failure(
                "PROCESS_DRIVER_PANICKED",
                "Process driver panicked; remaining resources retained",
            ),
        };
        self.issue(&error.code, &error.message, false);
        self.update(|m| m.phase = ProcessPhase::Unconfirmed);
        self.send(Err(error));
        // A failed owner must survive loss of the registry/waiter too. Its child,
        // unjoined I/O and keepalive remain in the record, never in this future.
        std::future::pending::<()>().await;
    }

    async fn drive(&self) -> Result<(), ApiError> {
        let mut resources = self.resources.lock().await;
        let start = Instant::now();
        let mut cleanup_start = None;
        let mut killed = false;
        let mut pending_reported = false;
        let mut operation_error = None;
        loop {
            for index in 0..resources.io.len() {
                let ready = resources.io[index]
                    .handle
                    .as_ref()
                    .is_some_and(|h| h.is_finished());
                if ready {
                    let result = resources.io[index]
                        .handle
                        .as_mut()
                        .expect("observed handle")
                        .await;
                    resources.io[index].joined = true;
                    resources.io[index].handle = None;
                    match result {
                        Ok(Ok(IoValue::Session(value))) => resources.output = Some(value),
                        Ok(Ok(IoValue::Drain)) => (),
                        Ok(Err(error)) => {
                            operation_error.get_or_insert(error);
                        }
                        Err(_) => {
                            self.issue(
                                "PROCESS_IO_JOIN_FAILED",
                                &format!("{} did not complete normally", resources.io[index].name),
                                false,
                            );
                            operation_error.get_or_insert_with(|| {
                                failure("PROCESS_IO_JOIN_FAILED", "Process I/O task failed")
                            });
                        }
                    }
                }
            }
            if operation_error.is_none() && *self.cancel.borrow() {
                operation_error = Some(failure(
                    "PROCESS_CANCELLED",
                    "Process operation was stopped",
                ));
            }
            if operation_error.is_none() && start.elapsed() >= self.options.work_timeout {
                operation_error = Some(failure(
                    "PROCESS_WORK_TIMEOUT",
                    "Process work deadline reached; draining owned resources",
                ));
                self.cancel.send_replace(true);
            }
            if operation_error.is_some() {
                self.cancel.send_replace(true);
            }
            let exited = if resources.exit.is_none() {
                resources.group.observe()?
            } else {
                None
            };
            if cleanup_start.is_none() && (exited.is_some() || operation_error.is_some()) {
                cleanup_start = Some(Instant::now());
                self.update(|m| m.phase = ProcessPhase::Stopping);
                if resources.exit.is_none() {
                    resources.group.signal(group::TERMINATE)?;
                }
            }
            if let Some(cleanup) = cleanup_start {
                if resources.exit.is_none() {
                    if !killed && cleanup.elapsed() >= self.options.terminate_grace {
                        resources.group.signal(group::KILL)?;
                        killed = true;
                    }
                    if let Some(observed) = resources.group.observe()? {
                        let _ = observed.code;
                        if !resources.group.has_descendants()? {
                            // No numeric signal is allowed after this point.
                            resources.group.disarm_before_reap();
                            let status = resources
                                .child
                                .as_mut()
                                .ok_or_else(|| {
                                    failure(
                                        "PROCESS_CHILD_MISSING",
                                        "Child handle disappeared without a reap receipt",
                                    )
                                })?
                                .wait()
                                .map_err(|_| {
                                    failure("PROCESS_REAP_FAILED", "Could not reap owned leader")
                                })?;
                            resources.exit = Some(status);
                            resources.child = None;
                            self.update(|m| {
                                m.leader_reaped = true;
                                m.group_exited = true;
                            });
                        }
                    }
                }
                let io_joined = resources.io.iter().all(|io| io.joined);
                if resources.exit.is_some() && io_joined {
                    self.update(|m| m.io_joined = true);
                    if self.snapshot().issues.iter().any(|i| !i.retryable) {
                        return Err(failure(
                            "PROCESS_DRAIN_UNCONFIRMED",
                            "A process owner failed; previous failure cannot be erased",
                        ));
                    }
                    let result = match operation_error {
                        Some(error) => Err(error),
                        None => Ok(ProcessOutput {
                            status: resources.exit.take().expect("reaped leader"),
                            value: resources.output.take().ok_or_else(|| {
                                failure("PROCESS_SESSION_MISSING", "No session result receipt")
                            })?,
                        }),
                    };
                    resources.keep_alive.take();
                    self.update(|m| {
                        let resolved = std::mem::take(&mut m.issues);
                        m.resolved_issues.extend(resolved);
                        m.phase = ProcessPhase::Closed;
                    });
                    self.send(result);
                    return Ok(());
                }
                if !pending_reported && cleanup.elapsed() >= self.options.cleanup_timeout {
                    pending_reported = true;
                    self.issue(
                        "PROCESS_CLEANUP_PENDING",
                        "Owned process or I/O is not yet confirmed closed; owner retained",
                        true,
                    );
                    self.update(|m| m.phase = ProcessPhase::CleanupPending);
                    self.send(Err(failure(
                        "PROCESS_CLEANUP_PENDING",
                        "Cleanup remains owned and pending; no completion permission",
                    )));
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl<T: Send + 'static, G: Send + 'static> ErasedRecord for Record<T, G> {
    fn snapshot(&self) -> ProcessSnapshot {
        self.meta.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    fn stop(&self) {
        self.cancel.send_replace(true);
    }
    fn join_driver(self: Arc<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let mut driver = self.driver.lock().await;
            let snapshot = self.snapshot();
            if snapshot.driver_joined || snapshot.phase == ProcessPhase::Unconfirmed {
                return;
            }
            if snapshot.phase != ProcessPhase::Closed
                && !driver.as_ref().is_some_and(|h| h.is_finished())
            {
                return;
            }
            let Some(handle) = driver.as_mut() else {
                self.issue(
                    "PROCESS_DRIVER_MISSING",
                    "Driver disappeared without a join receipt",
                    false,
                );
                self.update(|m| m.phase = ProcessPhase::Unconfirmed);
                return;
            };
            match handle.await {
                Ok(()) if self.snapshot().phase == ProcessPhase::Closed => {
                    self.update(|m| m.driver_joined = true)
                }
                _ => {
                    self.issue(
                        "PROCESS_DRIVER_JOIN_FAILED",
                        "Driver did not provide normal completion proof",
                        false,
                    );
                    self.update(|m| m.phase = ProcessPhase::Unconfirmed);
                    self.send(Err(failure(
                        "PROCESS_DRIVER_JOIN_FAILED",
                        "Process driver stopped without completion proof",
                    )));
                }
            }
            *driver = None;
        })
    }
}

#[cfg(test)]
#[path = "owned_process_tests.rs"]
mod tests;
