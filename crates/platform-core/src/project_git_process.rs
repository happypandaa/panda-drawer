//! Owned Git command sessions. Operation leases, phase gates and candidate
//! owners outlive a disappearing request until actual process/I/O completion.
use super::*;
use crate::owned_process::{
    IoPlan, OwnedProcessRegistry, ProcessOptions, ProcessOutput, ProcessPhase, ProcessRun,
};
use tokio::io::AsyncWrite;

type Owner = Arc<dyn Send + Sync>;
struct KeepAlive {
    _temporary: Arc<TempDir>,
    _operation: Option<Arc<ProjectTaskLease>>,
    _phase: Option<Owner>,
    _retained: Vec<Owner>,
}
pub(super) struct Context {
    temporary: Arc<TempDir>,
    root: PathBuf,
    pub(super) stop: watch::Receiver<bool>,
    pub(super) cancel: watch::Receiver<bool>,
    pub(super) deadline: Instant,
    processes: OwnedProcessRegistry,
    operation: Option<Arc<ProjectTaskLease>>,
    phase: Option<Owner>,
    retained: Vec<Owner>,
    #[cfg(test)]
    pub(super) writer_pause: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    #[cfg(test)]
    pub(super) commit_ack_observed: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Context {
    pub(super) fn new(
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Self, ApiError> {
        let temporary = tempfile::Builder::new()
            .prefix("local-projects-git-operation-")
            .tempdir()
            .map_err(|_| git_failed())?;
        let root = temporary.path().canonicalize().map_err(|_| unsafe_path())?;
        if root
            .to_str()
            .is_none_or(|path| path.contains(':') || path.chars().any(char::is_control))
        {
            return Err(unsafe_path());
        }
        private_dir(&root.join("empty-hooks"))?;
        private_dir(&root.join("home"))?;
        Ok(Self {
            temporary: Arc::new(temporary),
            root,
            stop,
            cancel,
            deadline: Instant::now() + TIMEOUT,
            processes: OwnedProcessRegistry::default(),
            operation: None,
            phase: None,
            retained: vec![],
            #[cfg(test)]
            writer_pause: None,
            #[cfg(test)]
            commit_ack_observed: None,
        })
    }
    pub(super) fn bind(
        &mut self,
        processes: OwnedProcessRegistry,
        operation: Arc<ProjectTaskLease>,
    ) {
        self.processes = processes;
        self.operation = Some(operation);
    }
    pub(super) fn bind_keep_alive(
        &mut self,
        processes: OwnedProcessRegistry,
        owner: Arc<dyn Send + Sync>,
    ) {
        self.processes = processes;
        self.retained.push(owner);
    }
    pub(super) fn clear_operation(&mut self) {
        self.operation = None;
    }
    pub(super) fn phase<T: Send + Sync + 'static>(&mut self, owner: Arc<T>) {
        self.phase = Some(owner);
    }
    pub(super) fn clear_phase(&mut self) {
        self.phase = None;
    }
    pub(super) fn retain<T: Send + Sync + 'static>(&mut self, owner: Arc<T>) {
        self.retained.push(owner);
    }
    fn keep_alive(&self) -> Owner {
        Arc::new(KeepAlive {
            _temporary: self.temporary.clone(),
            _operation: self.operation.clone(),
            _phase: self.phase.clone(),
            _retained: self.retained.clone(),
        })
    }
    pub(super) fn root(&self) -> &Path {
        &self.root
    }
    pub(super) fn checkpoint(&self) -> Result<(), ApiError> {
        checkpoint(&self.stop, &self.cancel, self.deadline)
    }
    pub(super) async fn resolve(
        &mut self,
        environment: &GitEnvironmentManager,
    ) -> Result<GitToolchain, ApiError> {
        self.checkpoint().map_err(|error| closure(error, true))?;
        let keep_alive = self.keep_alive();
        // Resource verification is real blocking I/O before a process record
        // exists. Product callers retain this request until prepare truly ends.
        let prepared = environment
            .prepare_resolution()
            .await
            .map_err(|error| closure(environment_error(error), true))?;
        self.checkpoint().map_err(|error| closure(error, true))?;
        // Only this prepared phase consists of registry-owned probes. Dropping
        // it sends stop while its actual process/G remain owned in the registry.
        let tools = tokio::select! {
            biased;
            _ = watch_stop(&mut self.stop) => return Err(closure(interrupted(), false)),
            _ = watch_stop(&mut self.cancel) => return Err(closure(interrupted(), false)),
            _ = tokio::time::sleep_until(self.deadline) => return Err(closure(timeout(), false)),
            result = environment.resolve_prepared_with_keep_alive(prepared, keep_alive) => result.map_err(environment_error)?,
        };
        self.checkpoint().map_err(|error| closure(error, true))?;
        if !supported_version(&tools.git.version) {
            return Err(closure(
                super::error(
                    "PROJECT_GIT_VERSION_UNSUPPORTED",
                    "本地版本操作目前只验证 Git 2.54.0 与 2.56.0；工具选择未改变。",
                ),
                true,
            ));
        }
        Ok(tools)
    }
    pub(super) fn start<T: Send + 'static>(
        &self,
        command: Command,
        make_io: impl FnOnce(crate::owned_process::ProcessPipes) -> IoPlan<T> + Send + 'static,
    ) -> Result<ProcessRun<T>, ApiError> {
        self.checkpoint().map_err(|error| closure(error, true))?;
        self.processes
            .start(
                command,
                ProcessOptions::new(
                    "project-git",
                    self.deadline.saturating_duration_since(Instant::now()),
                ),
                self.keep_alive(),
                make_io,
            )
            .map_err(|error| closure(error, true))
    }
    pub(super) async fn wait<T: Send + 'static>(
        &mut self,
        mut run: ProcessRun<T>,
    ) -> Result<ProcessOutput<T>, ApiError> {
        let interrupt = tokio::select! {
            biased;
            _ = watch_stop(&mut self.stop) => interrupted(),
            _ = watch_stop(&mut self.cancel) => interrupted(),
            _ = tokio::time::sleep_until(self.deadline) => timeout(),
            result = &mut run => return mapped_result(result, &run),
        };
        run.cancel();
        let result = (&mut run).await;
        match result {
            Err(error)
                if error.code != "PROCESS_CANCELLED" && error.code != "PROCESS_WORK_TIMEOUT" =>
            {
                Err(closure(error, io_closed(&run)))
            }
            _ => Err(closure(interrupt, io_closed(&run))),
        }
    }
    pub(super) async fn run(
        &mut self,
        command: Command,
        input: Vec<u8>,
    ) -> Result<Output, ApiError> {
        self.checkpoint().map_err(|error| closure(error, true))?;
        if input.len() > MAX_OUTPUT {
            return Err(closure(limit(), true));
        }
        let run = self.start(command, move |pipes| {
            let mut stdin = pipes.stdin;
            IoPlan::new(read_output(pipes.stdout, true, MAX_OUTPUT))
                .drain("stdin", async move {
                    stdin.write_all(&input).await.map_err(|_| git_failed())?;
                    stdin.shutdown().await.map_err(|_| git_failed())
                })
                .drain("stderr", async move {
                    read_output(pipes.stderr, false, 64 * 1024)
                        .await
                        .map(|_| ())
                })
        })?;
        let output = self.wait(run).await?;
        Ok(Output {
            code: output.status.code().unwrap_or(-1),
            stdout: output.value,
        })
    }
    pub(super) async fn commit_ref(
        &mut self,
        command: Command,
        new_head: &str,
        old_head: Option<&str>,
        validate: impl FnOnce() -> Result<(), ApiError> + Send,
    ) -> Result<(), ApiError> {
        let commands = format!(
            "start\0update HEAD\0{new_head}\0{}\0prepare\0",
            old_head.unwrap_or("0000000000000000000000000000000000000000")
        )
        .into_bytes();
        self.commit_transaction(command, commands, validate).await
    }
    pub(super) async fn commit_transaction(
        &mut self,
        command: Command,
        commands: Vec<u8>,
        validate: impl FnOnce() -> Result<(), ApiError> + Send,
    ) -> Result<(), ApiError> {
        self.checkpoint().map_err(|error| {
            closure(error, true)
                .detail("safeAbort", true)
                .detail("commitSent", false)
                .detail("commitAcknowledged", false)
        })?;
        let progress = Arc::new(CommitProgress::default());
        let session_progress = progress.clone();
        #[cfg(test)]
        let commit_ack_observed = self.commit_ack_observed.take();
        let (prepared_sender, mut prepared) = tokio::sync::oneshot::channel();
        let (authorize, authorization) = tokio::sync::oneshot::channel::<Result<(), ApiError>>();
        let mut run = self
            .start(command, move |pipes| {
                let mut stdin = pipes.stdin;
                let mut stdout = pipes.stdout;
                IoPlan::new(async move {
                    stdin.write_all(&commands).await.map_err(|_| git_failed())?;
                    stdin.flush().await.map_err(|_| git_failed())?;
                    expect_line(&mut stdout, b"start: ok\n").await?;
                    expect_line(&mut stdout, b"prepare: ok\n").await?;
                    let _ = prepared_sender.send(());
                    let validation = authorization.await.unwrap_or_else(|_| Err(interrupted()));
                    if let Err(error) = validation {
                        stdin
                            .write_all(b"abort\0")
                            .await
                            .map_err(|_| git_failed())?;
                        stdin.shutdown().await.map_err(|_| git_failed())?;
                        drop(stdin);
                        expect_line(&mut stdout, b"abort: ok\n").await?;
                        if !read_output(stdout, true, 1024).await?.is_empty() {
                            return Err(git_failed());
                        }
                        return Ok(Err(error));
                    }
                    // Store before the first byte: any partial commit write is unknown.
                    session_progress.sent.store(true, Ordering::Release);
                    stdin
                        .write_all(b"commit\0")
                        .await
                        .map_err(|_| git_failed())?;
                    stdin.shutdown().await.map_err(|_| git_failed())?;
                    drop(stdin);
                    expect_line(&mut stdout, b"commit: ok\n").await?;
                    session_progress.acknowledged.store(true, Ordering::Release);
                    #[cfg(test)]
                    if let Some(observed) = commit_ack_observed {
                        let _ = observed.send(());
                    }
                    if !read_output(stdout, true, 1024).await?.is_empty() {
                        return Err(git_failed());
                    }
                    Ok(Ok(()))
                })
                .drain("stderr", async move {
                    read_output(pipes.stderr, false, 64 * 1024)
                        .await
                        .map(|_| ())
                })
            })
            .map_err(|error| {
                error
                    .detail("safeAbort", true)
                    .detail("commitSent", false)
                    .detail("commitAcknowledged", false)
            })?;
        let mut authorize = Some(authorize);
        let mut validate = Some(validate);
        let mut prepared_observed = false;
        let mut delivered = false;
        let result = loop {
            tokio::select! {
                biased;
                _ = watch_stop(&mut self.stop) => break Err(interrupted()),
                _ = watch_stop(&mut self.cancel) => break Err(interrupted()),
                _ = tokio::time::sleep_until(self.deadline) => break Err(timeout()),
                receipt = &mut prepared, if !prepared_observed => {
                    prepared_observed = true;
                    if receipt.is_ok() {
                        let validation = validate.take().expect("single validation")();
                        let _ = authorize.take().expect("single authorization").send(validation);
                    } else {
                        authorize.take();
                    }
                },
                output = &mut run => {
                    delivered = true;
                    break match output {
                        Ok(output) if output.status.success() => output.value,
                        Ok(_) => Err(git_failed()),
                        Err(error) => Err(error),
                    };
                },
            }
        };
        // A dropped authorization aborts the prepared transaction inside its
        // owned session. That session/stdio/G continue to be retained by registry.
        drop(authorize);
        let result = if !delivered {
            run.cancel();
            let cleanup = (&mut run).await;
            match cleanup {
                Err(error)
                    if error.code != "PROCESS_CANCELLED"
                        && error.code != "PROCESS_WORK_TIMEOUT" =>
                {
                    Err(error)
                }
                _ => result,
            }
        } else {
            result
        };
        let closed = io_closed(&run);
        let acknowledged = progress.acknowledged.load(Ordering::Acquire);
        let sent = acknowledged || progress.sent.load(Ordering::Acquire);
        result.map_err(|error| {
            let resource_closed = resources_closed(&error, closed);
            closure(error, resource_closed)
                .detail("safeAbort", resource_closed && !sent)
                .detail("commitSent", sent)
                .detail("commitAcknowledged", acknowledged)
                .outcome(if acknowledged {
                    "committed"
                } else if resource_closed && !sent {
                    "not_committed"
                } else {
                    "unknown"
                })
        })
    }
}
#[derive(Default)]
struct CommitProgress {
    sent: AtomicBool,
    acknowledged: AtomicBool,
}
fn io_closed<T>(run: &ProcessRun<T>) -> bool {
    let state = run.snapshot();
    state.phase == ProcessPhase::Closed
        && state.leader_reaped
        && state.group_exited
        && state.io_joined
}
fn mapped_result<T>(
    result: Result<ProcessOutput<T>, ApiError>,
    run: &ProcessRun<T>,
) -> Result<ProcessOutput<T>, ApiError> {
    result.map_err(|error| {
        let error = match error.code.as_str() {
            "PROCESS_CANCELLED" => interrupted(),
            "PROCESS_WORK_TIMEOUT" => timeout(),
            _ => error,
        };
        closure(error, io_closed(run))
    })
}
fn timeout() -> ApiError {
    super::error(
        "PROJECT_GIT_TIMEOUT",
        "Git 本地操作超过 120 秒内部限制；结果以任务提交点记录为准。",
    )
}
pub(super) fn checkpoint(
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
    deadline: Instant,
) -> Result<(), ApiError> {
    if *stop.borrow()
        || *cancel.borrow()
        || stop.has_changed().is_err()
        || cancel.has_changed().is_err()
    {
        return Err(interrupted());
    }
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    Ok(())
}

/// Construct on the runtime, use only within a blocking session whose actual
/// JoinHandle remains awaited by IoPlan. No cancellable tokio::fs write is hidden.
pub(super) struct BlockingPipe<T> {
    stream: T,
    runtime: tokio::runtime::Handle,
}
impl<T> BlockingPipe<T> {
    pub(super) fn new(stream: T) -> Self {
        Self {
            stream,
            runtime: tokio::runtime::Handle::current(),
        }
    }
}
impl<T: AsyncRead + Unpin> Read for BlockingPipe<T> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.runtime.block_on(self.stream.read(bytes))
    }
}
impl<T: AsyncWrite + Unpin> Write for BlockingPipe<T> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.runtime.block_on(self.stream.write(bytes))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.runtime.block_on(self.stream.flush())
    }
}

pub(super) fn expect_blocking_line(
    stream: &mut impl Read,
    expected: &[u8],
) -> Result<(), ApiError> {
    let mut line = vec![0u8; expected.len()];
    stream.read_exact(&mut line).map_err(|_| output_invalid())?;
    if line != expected {
        return Err(output_invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_git_process_tests.rs"]
mod tests;

fn environment_error(error: ApiError) -> ApiError {
    // Non-process tool errors are produced before spawn or after joined probes.
    // Unknown process outcomes have no closure receipt available at this layer.
    let closed = resources_closed(&error, true);
    let error = if error.code.starts_with("PROCESS_") {
        error
    } else {
        super::error(
            "PROJECT_GIT_ENVIRONMENT_UNAVAILABLE",
            "Git 工具不可用，请先检查工具设置；不会改用另一份 Git。",
        )
    };
    closure(error, closed)
}
fn resources_closed(error: &ApiError, observed: bool) -> bool {
    observed
        && error.details["processResourceClosed"] != false
        && error.details["processClosed"] != false
        && (!error.code.starts_with("PROCESS_")
            || error.details["operationOutcome"] == "not_started"
            || matches!(
                error.code.as_str(),
                "PROCESS_CANCELLED" | "PROCESS_WORK_TIMEOUT"
            ))
}
fn closure(error: ApiError, closed: bool) -> ApiError {
    let closed = resources_closed(&error, closed);
    let error = if !closed
        && !matches!(
            error.details["operationOutcome"].as_str(),
            Some("committed" | "not_started")
        ) {
        error.outcome("unknown")
    } else {
        error
    };
    error
        .detail("processClosed", closed)
        .detail("processResourceClosed", closed)
}
