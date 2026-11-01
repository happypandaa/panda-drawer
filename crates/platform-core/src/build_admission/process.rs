//! Owned Git command sessions. Operation leases, phase gates and candidate
//! owners outlive a disappearing request until actual process/I/O completion.
use super::*;
use crate::owned_process::{
    IoPlan, OwnedProcessRegistry, ProcessOptions, ProcessOutput, ProcessPhase, ProcessRun,
};

type Owner = Arc<dyn Send + Sync>;
struct KeepAlive {
    _temporary: Arc<TempDir>,
    _retained: Vec<Owner>,
}
pub(super) struct Context {
    temporary: Arc<TempDir>,
    root: PathBuf,
    pub(super) stop: watch::Receiver<bool>,
    pub(super) cancel: watch::Receiver<bool>,
    pub(super) deadline: Instant,
    processes: OwnedProcessRegistry,
    retained: Vec<Owner>,
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
        if !native_io::valid_context_root(&root) {
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
            retained: vec![],
        })
    }
    pub(super) fn bind_keep_alive(
        &mut self,
        processes: OwnedProcessRegistry,
        owner: Arc<dyn Send + Sync>,
    ) {
        self.processes = processes;
        self.retained.push(owner);
    }
    fn keep_alive(&self) -> Owner {
        Arc::new(KeepAlive {
            _temporary: self.temporary.clone(),
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
        let environment = environment.with_process_registry(self.processes.clone());
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
        let command =
            native_io::prepare_git_command(command).map_err(|error| closure(error, true))?;
        self.processes
            .start(
                crate::owned_process::OwnedCommand::cleared(command),
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
