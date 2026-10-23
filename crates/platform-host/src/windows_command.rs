//! Protocol-independent, explicitly configured Windows tool execution.
//! Every child enters an owned Job before its first instruction. This manages
//! process lifetime; it does not sandbox trusted build scripts or their access.
use crate::{windows_process::JobChild, WindowsSourceDirectory};
use platform_project_format::ApiError;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{watch, Notify},
};

const MAX_STREAM_LIMIT: usize = 16 * 1024 * 1024;
const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

struct Activity {
    count: AtomicUsize,
    idle: Notify,
}
fn activity() -> &'static Activity {
    static ACTIVITY: OnceLock<Activity> = OnceLock::new();
    ACTIVITY.get_or_init(|| Activity {
        count: AtomicUsize::new(0),
        idle: Notify::new(),
    })
}
struct ActiveExecution;
impl ActiveExecution {
    fn new() -> Self {
        activity().count.fetch_add(1, Ordering::AcqRel);
        Self
    }
}
impl Drop for ActiveExecution {
    fn drop(&mut self) {
        if activity().count.fetch_sub(1, Ordering::AcqRel) == 1 {
            activity().idle.notify_waiters();
        }
    }
}

/// Wait until command Jobs and their retained cleanup guards have drained.
/// For a shutdown barrier, first close admission and drain submitters so new
/// commands cannot follow this idle observation. An error handler may also wait
/// here while retaining its tool lease. Failed queries never establish idleness.
pub async fn wait_for_idle() {
    let activity = activity();
    loop {
        let notified = activity.idle.notified();
        tokio::pin!(notified);
        // Register before checking, including when completion races this poll.
        notified.as_mut().enable();
        if activity.count.load(Ordering::Acquire) == 0 {
            return;
        }
        notified.await;
    }
}

pub struct CommandSpec {
    /// Invoke this exact executable directly, with no shell or PATH search.
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub working_directory: PathBuf,
    /// Complete environment, with ASCII, case-unique names. Nothing is inherited.
    pub environment: BTreeMap<OsString, OsString>,
    /// Written concurrently with both output readers, then closed to deliver EOF.
    pub stdin: Vec<u8>,
    pub timeout: Duration,
    /// Each stream retains its own prefix and drains/discards excess bytes.
    pub stdout_limit: usize,
    pub stderr_limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandTermination {
    Exited,
    Cancelled,
    TimedOut,
    /// The leader exited while owned descendants still ran; those were stopped.
    DescendantsTerminated,
}

#[derive(Debug, Default)]
pub struct CapturedOutput {
    pub bytes: Vec<u8>,
    pub discarded_bytes: u64,
    pub read_failed: bool,
}

#[derive(Debug)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub termination: CommandTermination,
    pub stdout: CapturedOutput,
    pub stderr: CapturedOutput,
    pub stdin_complete: bool,
    pub output_truncated: bool,
    /// True only after the entire owned Job, including its leader, is empty.
    pub tree_exited: bool,
}
impl CommandOutput {
    /// A zero leader exit never hides cancellation, truncated data or leftovers.
    pub fn success(&self) -> bool {
        self.termination == CommandTermination::Exited
            && self.exit_code == Some(0)
            && self.tree_exited
            && self.stdin_complete
            && !self.output_truncated
            && !self.stdout.read_failed
            && !self.stderr.read_failed
    }
}

fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn native_error(mut value: ApiError) -> ApiError {
    if let Some(code) = value.code.strip_prefix("JOB_") {
        value.code = format!("COMMAND_{code}");
    }
    value
}

/// Resolve the OS system root directly; the caller explicitly selects which
/// environment entries to provide to a tool.
pub fn windows_directory() -> Result<PathBuf, ApiError> {
    crate::windows_process::windows_directory()
        .map(PathBuf::from)
        .map_err(native_error)
}

fn binding(path: &Path, directory: bool) -> Result<WindowsSourceDirectory, ApiError> {
    let invalid =
        || error("COMMAND_PATH_INVALID", "工具路径或工作目录无法安全验证。").outcome("not_started");
    if !path.is_absolute() {
        return Err(invalid());
    }
    if directory {
        WindowsSourceDirectory::open(path).map_err(|_| invalid())
    } else {
        let parent = WindowsSourceDirectory::open(path.parent().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        parent
            .pin_file(
                path.file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(invalid)?,
            )
            .map_err(|_| invalid())
    }
}

struct Execution {
    child: Option<JobChild>,
    bindings: Vec<WindowsSourceDirectory>,
    active: Option<ActiveExecution>,
}
impl Execution {
    fn child(&self) -> &JobChild {
        self.child.as_ref().unwrap()
    }
    fn child_mut(&mut self) -> &mut JobChild {
        self.child.as_mut().unwrap()
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        let Some(child) = self.child.take() else {
            return;
        };
        let active = self.active.take();
        if child.ownership().active_processes().ok() == Some(0) {
            drop(child);
            self.bindings.clear();
            drop(active);
            return;
        }
        // Cancellation of the run future must kill immediately, without waiting
        // for an async destructor or relying on the Tokio runtime staying alive.
        let _ = child.ownership().terminate();
        // The activity guard moves with the exact Job and path bindings. Even
        // after an aborted future, native exit must wait for this cleanup owner.
        let cleanup = Arc::new((child, std::mem::take(&mut self.bindings), active));
        let retained = cleanup.clone();
        if std::thread::Builder::new()
            .name("windows-command-cleanup".into())
            .spawn(move || retained.0.wait_for_tree_exit_blocking())
            .is_err()
        {
            // If a cleanup thread cannot be created, keep the exact bindings
            // locally until the Job is empty. A failed query is never success.
            cleanup.0.wait_for_tree_exit_blocking();
        }
    }
}

async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
async fn capture(mut file: tokio::fs::File, limit: usize) -> CapturedOutput {
    let mut output = CapturedOutput::default();
    let mut buffer = [0; 8192];
    loop {
        match file.read(&mut buffer).await {
            Ok(0) => return output,
            Ok(count) => {
                let retained = count.min(limit.saturating_sub(output.bytes.len()));
                output.bytes.extend_from_slice(&buffer[..retained]);
                output.discarded_bytes = output
                    .discarded_bytes
                    .saturating_add((count - retained) as u64);
            }
            Err(_) => {
                output.read_failed = true;
                return output;
            }
        }
    }
}

/// Execute one bounded command. A closed cancellation channel also cancels it.
/// Returns only after all owned processes exit. Cleanup uncertainty is an error,
/// and retained HANDLE/path guards continue cleanup even if this future drops.
pub async fn run(
    spec: CommandSpec,
    mut cancel: watch::Receiver<bool>,
) -> Result<CommandOutput, ApiError> {
    if spec.stdin.len() > MAX_STDIN_BYTES
        || spec.stdout_limit > MAX_STREAM_LIMIT
        || spec.stderr_limit > MAX_STREAM_LIMIT
        || spec.timeout.is_zero()
        || spec.timeout > Duration::from_secs(24 * 60 * 60)
    {
        return Err(error(
            "COMMAND_LIMIT_INVALID",
            "工具输入、输出或执行时限超出受管范围。",
        )
        .outcome("not_started"));
    }
    if *cancel.borrow() || cancel.has_changed().is_err() {
        return Ok(CommandOutput {
            exit_code: None,
            termination: CommandTermination::Cancelled,
            stdout: CapturedOutput::default(),
            stderr: CapturedOutput::default(),
            stdin_complete: spec.stdin.is_empty(),
            output_truncated: false,
            tree_exited: true,
        });
    }
    let executable = binding(&spec.executable, false)?;
    let cwd = binding(&spec.working_directory, true)?;
    let application = executable.path().join(spec.executable.file_name().unwrap());
    // No await may separate admission/spawn from the owning Execution guard.
    let active = ActiveExecution::new();
    let child = JobChild::spawn(&application, &spec.args, cwd.path(), &spec.environment)
        .map_err(|value| native_error(value).outcome("not_started"))?;
    let mut execution = Execution {
        child: Some(child),
        bindings: vec![executable, cwd],
        active: Some(active),
    };
    let stdout = tokio::spawn(capture(
        execution.child_mut().stdout.take().unwrap(),
        spec.stdout_limit,
    ));
    let stderr = tokio::spawn(capture(
        execution.child_mut().stderr.take().unwrap(),
        spec.stderr_limit,
    ));
    let mut stdin = execution.child_mut().stdin.take().unwrap();
    let input = tokio::spawn(async move {
        stdin.write_all(&spec.stdin).await.is_ok() && stdin.shutdown().await.is_ok()
    });
    let mut termination = tokio::select! {
        result = execution.child_mut().wait() => {
            result.map_err(|_| error("COMMAND_OBSERVATION_FAILED", "无法确认受管工具退出状态。").outcome("unknown"))?;
            CommandTermination::Exited
        }
        () = cancelled(&mut cancel) => CommandTermination::Cancelled,
        () = tokio::time::sleep(spec.timeout) => CommandTermination::TimedOut,
    };
    let remaining = execution
        .child()
        .ownership()
        .active_processes()
        .map_err(|_| {
            error(
                "COMMAND_CLEANUP_UNCONFIRMED",
                "无法确认受管工具子进程已全部退出。",
            )
            .outcome("unknown")
        })?;
    if remaining > 0 {
        if termination == CommandTermination::Exited {
            termination = CommandTermination::DescendantsTerminated;
        }
        execution.child().ownership().terminate().map_err(|_| {
            error("COMMAND_CLEANUP_UNCONFIRMED", "无法停止受管工具子进程。").outcome("unknown")
        })?;
    }
    tokio::time::timeout(CLEANUP_TIMEOUT, execution.child().wait_for_tree_exit())
        .await
        .map_err(|_| {
            error(
                "COMMAND_CLEANUP_UNCONFIRMED",
                "受管工具子进程尚未确认全部退出。",
            )
            .outcome("unknown")
        })?;
    let exit_code = execution
        .child_mut()
        .wait()
        .await
        .map_err(|_| {
            error("COMMAND_OBSERVATION_FAILED", "无法确认受管工具退出状态。").outcome("unknown")
        })?
        .code();
    let (stdout, stderr, stdin_complete) = tokio::time::timeout(CLEANUP_TIMEOUT, async {
        Ok::<_, ApiError>((
            stdout.await.map_err(|_| {
                error("COMMAND_IO_FAILED", "无法收集工具标准输出。").outcome("unknown")
            })?,
            stderr.await.map_err(|_| {
                error("COMMAND_IO_FAILED", "无法收集工具错误输出。").outcome("unknown")
            })?,
            input.await.map_err(|_| {
                error("COMMAND_IO_FAILED", "无法确认工具输入写入完成。").outcome("unknown")
            })?,
        ))
    })
    .await
    .map_err(|_| error("COMMAND_IO_FAILED", "工具通道尚未确认关闭。").outcome("unknown"))??;
    Ok(CommandOutput {
        exit_code,
        termination,
        output_truncated: stdout.discarded_bytes != 0 || stderr.discarded_bytes != 0,
        stdout,
        stderr,
        stdin_complete,
        tree_exited: true,
    })
}

#[cfg(test)]
#[path = "windows_command_tests.rs"]
mod tests;
