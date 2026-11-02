//! Owned Node runtime lifecycle, with process and file policy in platform adapters.
//!
//! It never discovers a workspace, starts the platform, installs dependencies, or
//! falls back to a Node from PATH. Callers must explicitly stop the instance.
//! Drop is best-effort cleanup; persisted run reconciliation belongs to the
//! registration layer, separate from this retained direct-Child owner.

use crate::node_process_identity::{self, RuntimeProcessIdentity, RUN_IDENTITY_ENV};
use crate::node_runtime_log::{DiagnosticLines, RuntimeLog};
use crate::owned_process::{IoPlan, OwnedProcessRegistry, ProcessOptions};
use crate::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    task::JoinHandle,
};
use uuid::Uuid;

#[path = "node_runtime_http.rs"]
mod http;

#[cfg(not(windows))]
#[path = "node_runtime_trial/macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "node_runtime_trial/windows.rs"]
mod platform;
use platform::{checked_path, checked_storage, Child, ChildStdin, ProcessOwnership};
#[cfg(all(test, not(windows)))]
use platform::{group_has_other_members, observe_without_reaping};

pub const EXPECTED_NODE_VERSION: &str = "24.22.0";
const MESSAGE_LIMIT: usize = 8192;
// Existing escalation labels stay independent of libc. The Windows adapter
// terminates its retained Job and never sends either as a Unix signal.
const SIGNAL_TERM: i32 = 15;
const SIGNAL_KILL: i32 = 9;
const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
type TrialResult<T> = Result<T, ApiError>;

/// Caller-supplied release digest must come from independently verified metadata.
/// Do not accept a digest merely because an untrusted app declares it.
pub struct RuntimeLaunchConfig {
    pub node_path: PathBuf,
    pub expected_node_sha256: String,
    pub entry_path: PathBuf,
    pub working_directory: PathBuf,
    pub trial_root: PathBuf,
    pub business_env: BTreeMap<String, String>,
    pub control_directory: PathBuf,
    pub redaction_values: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialStatus {
    pub instance_id: String,
    pub pid: u32,
    pub state: String,
    pub port: Option<u16>,
    pub exit_code: Option<i32>,
    pub stderr_bytes_received: u64,
    pub log_error: Option<String>,
    pub forced: bool,
    pub stopping_observed: bool,
    /// True after no live owned-group members remain and the leader is reaped.
    /// Dead adopted zombies awaiting the OS reaper are not running processes.
    pub process_group_exited: bool,
    pub run_directory: PathBuf,
    pub hard_crash_recovery_implemented: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum ControlMessage {
    #[serde(rename_all = "camelCase")]
    Ready {
        protocol_version: u32,
        instance_id: String,
        host: String,
        port: u16,
    },
    #[serde(rename_all = "camelCase")]
    Pong {
        protocol_version: u32,
        instance_id: String,
        request_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Stopping {
        protocol_version: u32,
        instance_id: String,
    },
}

/// Fixed platform reasons contain no application configuration or error text.
#[derive(Clone, Copy)]
pub enum RuntimeStopReason {
    UserRequest,
    WindowClosed,
    Restart,
    PlatformExit,
    StartupRollback,
    HealthCheckFailed,
    ControlProtocolFailed,
    Deletion,
    OwnerDropped,
}
impl RuntimeStopReason {
    fn label(self) -> &'static str {
        match self {
            Self::UserRequest => "user-request",
            Self::WindowClosed => "window-closed",
            Self::Restart => "restart",
            Self::PlatformExit => "platform-exit",
            Self::StartupRollback => "startup-rollback",
            Self::HealthCheckFailed => "health-check-failed",
            Self::ControlProtocolFailed => "control-protocol-failed",
            Self::Deletion => "project-deletion",
            Self::OwnerDropped => "runtime-owner-dropped",
        }
    }
}

pub struct NodeRuntimeTrial {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<TrialResult<ControlMessage>>,
    readers: Vec<JoinHandle<()>>,
    reader_failure: Option<ApiError>,
    token: String,
    identity_nonce: String,
    status: TrialStatus,
    stderr_count: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    control_failed: Arc<AtomicBool>,
    ownership: Arc<ProcessOwnership>,
    log: RuntimeLog,
    termination_logged: Arc<AtomicBool>,
    exit_logged: bool,
}

#[derive(Clone, Copy)]
struct ExitObservation {
    code: Option<i32>,
    signal: Option<i32>,
}

fn fail(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}

/// User-facing explanations use fixed text, never an application's raw error.
pub(crate) fn startup_failure_explanation(error: &ApiError) -> String {
    match error.code.as_str() {
        "TRIAL_HTTP_STATUS" => match error.details.get("httpStatus").and_then(Value::as_u64) {
            Some(status) => format!("后端启动检查接口返回了 HTTP {status}，需要返回 HTTP 200。"),
            None => "后端启动检查接口没有返回成功结果。".into(),
        },
        "TRIAL_HTTP_INVALID" => "后端启动检查的回复格式无法识别。".into(),
        "TRIAL_HTTP_LIMIT" => "后端启动检查的回复超过了大小限制。".into(),
        "TRIAL_HTTP_FAILED" => "后端报告准备完成，但平台无法连接它的本机服务。".into(),
        "TRIAL_HTTP_TIMEOUT" => "后端未在 3 秒内完成本机健康检查。".into(),
        "TRIAL_HEALTH_INVALID" => "后端健康检查的协议版本、启动身份或状态不符合要求。".into(),
        "TRIAL_READY_INVALID" => "后端报告的启动身份、监听地址或端口不符合要求。".into(),
        "TRIAL_PROTOCOL_INVALID" | "TRIAL_PROTOCOL_LIMIT" => {
            "后端的启动消息不符合平台控制协议，请检查是否把普通日志写到了标准输出。".into()
        }
        "TRIAL_CONTROL_CLOSED" => "后端在完成启动前关闭了控制通道或退出。".into(),
        "TRIAL_START_TIMEOUT" => "后端未在 10 秒内完成启动。".into(),
        _ => "后端未能完成启动，请查看本次运行日志中的具体原因。".into(),
    }
}

fn validate_business_environment(values: &BTreeMap<String, String>) -> TrialResult<()> {
    if values.len() > 32 {
        return Err(fail(
            "TRIAL_ENV_INVALID",
            "Too many explicit trial variables",
        ));
    }
    let mut total = 0usize;
    for (name, value) in values {
        let valid_name = crate::panda_manifest::valid_business_variable(name);
        total = total.saturating_add(name.len()).saturating_add(value.len());
        if !valid_name
            || name == RUN_IDENTITY_ENV
            || name.len() > 128
            || value.len() > 8192
            || value.contains('\0')
            || total > 32768
        {
            return Err(fail(
                "TRIAL_ENV_INVALID",
                "A trial variable is invalid or reserved",
            ));
        }
    }
    Ok(())
}

fn verify_node_digest(path: &Path, expected: &str) -> TrialResult<()> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail(
            "TRIAL_NODE_DIGEST_INVALID",
            "A verified Node SHA-256 is required",
        ));
    }
    let mut file = platform::open_node(path)
        .map_err(|_| fail("TRIAL_NODE_INVALID", "The private Node could not be read"))?;
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file
            .read(&mut bytes)
            .map_err(|_| fail("TRIAL_NODE_INVALID", "The private Node could not be read"))?;
        if n == 0 {
            break;
        }
        digest.update(&bytes[..n]);
    }
    if format!("{:x}", digest.finalize()) != expected.to_ascii_lowercase() {
        return Err(fail(
            "TRIAL_NODE_DIGEST_MISMATCH",
            "Private Node does not match the pinned release digest",
        ));
    }
    Ok(())
}

#[cfg(test)]
async fn verify_node_version(path: &Path, processes: &OwnedProcessRegistry) -> TrialResult<()> {
    verify_node_version_with_receipt(
        path,
        processes,
        &mut FailedLaunchCleanup::NoApplicationSpawned,
    )
    .await
}

async fn verify_node_version_with_receipt(
    path: &Path,
    processes: &OwnedProcessRegistry,
    cleanup: &mut FailedLaunchCleanup,
) -> TrialResult<()> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut probe = processes.start(
        crate::owned_process::OwnedCommand::cleared(command),
        ProcessOptions::new("node-runtime/version-probe", Duration::from_secs(3)),
        (),
        |pipes| {
            drop(pipes.stdin);
            IoPlan::new(async move {
                let mut stdout = pipes.stdout;
                let mut output = Vec::new();
                let mut buffer = [0u8; 65];
                loop {
                    let count = stdout.read(&mut buffer).await?;
                    if count == 0 {
                        return Ok(output);
                    }
                    if output.len() + count > 64 {
                        return Err(fail(
                            "TRIAL_NODE_VERSION",
                            "Private Node version output exceeded its budget",
                        ));
                    }
                    output.extend_from_slice(&buffer[..count]);
                }
            })
            .drain("stderr", async move {
                let mut stderr = pipes.stderr;
                let mut bytes = [0u8; 4096];
                let mut total = 0usize;
                loop {
                    let count = stderr.read(&mut bytes).await?;
                    if count == 0 {
                        return Ok(());
                    }
                    total += count;
                    if total > 16 * 1024 {
                        return Err(fail(
                            "TRIAL_NODE_VERSION",
                            "Private Node version diagnostics exceeded their budget",
                        ));
                    }
                }
            })
        },
    )?;
    // Registry::start only returns Err before registration/spawn succeeds.
    // Once admitted, both the probe and its real reader/driver handles need proof.
    *cleanup = FailedLaunchCleanup::Unconfirmed;
    let output = (&mut probe).await;
    if probe.resource_closed().await {
        *cleanup = FailedLaunchCleanup::NoApplicationSpawned;
    } else {
        return Err(fail(
            "TRIAL_NODE_PROBE_CLEANUP_UNCONFIRMED",
            "Private Node version probe did not confirm complete resource closure",
        ));
    }
    let output = output?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.value).trim() != format!("v{EXPECTED_NODE_VERSION}")
    {
        return Err(fail(
            "TRIAL_NODE_VERSION",
            "Trial requires the pinned Node 24.22.0 release",
        ));
    }
    Ok(())
}

async fn limited_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> TrialResult<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| fail("TRIAL_CONTROL_CLOSED", "Control output could not be read"))?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(fail(
                    "TRIAL_PROTOCOL_INVALID",
                    "Control output ended within a message",
                ))
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |n| n + 1);
        if bytes.len().saturating_add(count) > MESSAGE_LIMIT {
            return Err(fail(
                "TRIAL_PROTOCOL_LIMIT",
                "Control message exceeded 8 KiB",
            ));
        }
        let done = available[count - 1] == b'\n';
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if done {
            return Ok(Some(bytes));
        }
    }
}

/// Only positive, local launch receipts permit clearing this launch's intent.
/// An empty pending handle or an absent PID never establishes either receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailedLaunchCleanup {
    NoApplicationSpawned,
    ResourcesStopped,
    Unconfirmed,
}

impl FailedLaunchCleanup {
    pub(crate) fn permits_intent_removal(self) -> bool {
        matches!(self, Self::NoApplicationSpawned | Self::ResourcesStopped)
    }
}

struct PreparedLaunch {
    node: PathBuf,
    cwd: PathBuf,
    entry: PathBuf,
    trial_root: PathBuf,
    persistent: Option<(PathBuf, PathBuf)>,
    project_root: Option<PathBuf>,
}

async fn prepare_launch(
    config: &RuntimeLaunchConfig,
    persistent: Option<(&Path, &Path)>,
    project_root: Option<&Path>,
    processes: &OwnedProcessRegistry,
    cleanup: &mut FailedLaunchCleanup,
) -> TrialResult<PreparedLaunch> {
    platform::require_supported()?;
    validate_business_environment(&config.business_env)?;
    let node = checked_path(&config.node_path, false)?;
    let cwd = checked_path(&config.working_directory, true)?;
    let entry = checked_path(&config.entry_path, false)?;
    if !entry.starts_with(&cwd) || entry == cwd {
        return Err(fail(
            "TRIAL_ENTRY_INVALID",
            "Entry must be inside the explicit backend directory",
        ));
    }
    let trial_root = checked_path(&config.trial_root, true)?;
    if !trial_root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("node-runtime-trial-"))
        || fs::read_dir(&trial_root)
            .map_err(|_| fail("TRIAL_ROOT_INVALID", "Trial root cannot be read"))?
            .next()
            .is_some()
    {
        return Err(fail(
            "TRIAL_ROOT_INVALID",
            "Use an existing empty node-runtime-trial-* disposable directory",
        ));
    }
    let persistent = persistent
        .map(|(data, cache)| {
            if let Some(source) = project_root {
                platform::checked_project_storage(source, cache, &cwd, &trial_root)
            } else {
                checked_storage(data.to_path_buf(), cache.to_path_buf(), &cwd, &trial_root)
            }
        })
        .transpose()?;
    verify_node_digest(&node, &config.expected_node_sha256)?;
    verify_node_version_with_receipt(&node, processes, cleanup).await?;
    Ok(PreparedLaunch {
        node,
        cwd,
        entry,
        trial_root,
        persistent,
        project_root: project_root.map(Path::to_path_buf),
    })
}

impl NodeRuntimeTrial {
    pub async fn launch(config: RuntimeLaunchConfig) -> TrialResult<Self> {
        Self::launch_inner(
            config,
            None,
            None,
            None,
            &OwnedProcessRegistry::default(),
            &mut FailedLaunchCleanup::NoApplicationSpawned,
        )
        .await
    }

    /// Use caller-owned persistent data/cache while preserving a fresh run/tmp.
    /// The registration layer must also reject overlap with the full application
    /// source root, which is not represented by this backend-only launch config.
    pub async fn launch_with_storage(
        config: RuntimeLaunchConfig,
        data: PathBuf,
        cache: PathBuf,
    ) -> TrialResult<Self> {
        Self::launch_inner(
            config,
            Some((data, cache)),
            None,
            None,
            &OwnedProcessRegistry::default(),
            &mut FailedLaunchCleanup::NoApplicationSpawned,
        )
        .await
    }

    /// The platform retains any failed-start handle whose group cleanup could
    /// not be confirmed. Ordinary trial callers keep their existing API.
    #[cfg(all(test, windows))]
    pub(crate) async fn launch_retaining_failed(
        config: RuntimeLaunchConfig,
        data: PathBuf,
        cache: PathBuf,
        processes: &OwnedProcessRegistry,
    ) -> (TrialResult<Self>, Option<Self>, FailedLaunchCleanup) {
        let mut pending = None;
        let mut cleanup = FailedLaunchCleanup::NoApplicationSpawned;
        let result = Self::launch_inner(
            config,
            Some((data, cache)),
            None,
            Some(&mut pending),
            processes,
            &mut cleanup,
        )
        .await;
        (result, pending, cleanup)
    }

    /// Read-only application preflight. Only the pinned private Node's version
    /// probe executes; no application code or application directories are written.
    /// Launch repeats these checks after the caller records its start intent.
    pub async fn preflight(
        config: &RuntimeLaunchConfig,
        data: &Path,
        cache: &Path,
    ) -> TrialResult<()> {
        Self::preflight_with_processes(config, data, cache, &OwnedProcessRegistry::default()).await
    }
    pub(crate) async fn preflight_with_processes(
        config: &RuntimeLaunchConfig,
        data: &Path,
        cache: &Path,
        processes: &OwnedProcessRegistry,
    ) -> TrialResult<()> {
        prepare_launch(
            config,
            Some((data, cache)),
            None,
            processes,
            &mut FailedLaunchCleanup::NoApplicationSpawned,
        )
        .await
        .map(|_| ())
    }

    /// Project starts create only source/data; preflight remains read-only.
    pub(crate) async fn preflight_project_with_processes(
        config: &RuntimeLaunchConfig,
        source: &Path,
        cache: &Path,
        processes: &OwnedProcessRegistry,
    ) -> TrialResult<()> {
        prepare_launch(
            config,
            Some((&source.join("data"), cache)),
            Some(source),
            processes,
            &mut FailedLaunchCleanup::NoApplicationSpawned,
        )
        .await
        .map(|_| ())
    }

    /// The caller must persist its start intent before this may create data/.
    /// Existing project data and its permissions are never reset on failed start.
    pub(crate) async fn launch_project_retaining_failed(
        config: RuntimeLaunchConfig,
        source: PathBuf,
        cache: PathBuf,
        processes: &OwnedProcessRegistry,
    ) -> (TrialResult<Self>, Option<Self>, FailedLaunchCleanup) {
        let mut pending = None;
        let mut cleanup = FailedLaunchCleanup::NoApplicationSpawned;
        let result = Self::launch_inner(
            config,
            Some((source.join("data"), cache)),
            Some(source),
            Some(&mut pending),
            processes,
            &mut cleanup,
        )
        .await;
        (result, pending, cleanup)
    }

    async fn launch_inner(
        config: RuntimeLaunchConfig,
        persistent: Option<(PathBuf, PathBuf)>,
        project_root: Option<PathBuf>,
        pending: Option<&mut Option<Self>>,
        processes: &OwnedProcessRegistry,
        cleanup: &mut FailedLaunchCleanup,
    ) -> TrialResult<Self> {
        let PreparedLaunch {
            node,
            cwd,
            entry,
            trial_root,
            persistent,
            project_root: prepared_project_root,
        } = prepare_launch(
            &config,
            persistent
                .as_ref()
                .map(|(data, cache)| (data.as_path(), cache.as_path())),
            project_root.as_deref(),
            processes,
            cleanup,
        )
        .await?;
        let instance_id = Uuid::new_v4().to_string();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        // Process identity survives host restarts independently of the private
        // control bearer. Never persist or expose the bearer as a run identity.
        let identity_nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let run_directory = trial_root.join(format!("run-{instance_id}"));
        if let Some(source) = &prepared_project_root {
            platform::prepare_project_data(source)?;
        }
        let (data, cache) = platform::prepare_directories(
            &run_directory,
            persistent,
            prepared_project_root.is_none(),
        )?;
        let log = RuntimeLog::start(&config.control_directory)?;
        let mut command = Command::new(node);
        command
            .arg(platform::argument(&entry)?)
            .current_dir(platform::argument(&cwd)?)
            .env_clear()
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .env("APP_INSTANCE_ID", &instance_id)
            .env(RUN_IDENTITY_ENV, &identity_nonce)
            .env("APP_HOST", "127.0.0.1")
            .env("APP_PORT", "0")
            .env("APP_DATA_DIR", platform::argument(&data)?)
            .env("APP_CACHE_DIR", platform::argument(&cache)?)
            .env(
                "APP_TEMP_DIR",
                platform::argument(&run_directory.join("tmp"))?,
            )
            .env("TMPDIR", platform::argument(&run_directory.join("tmp"))?)
            .envs(config.business_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        platform::configure_environment(&mut command, &run_directory.join("tmp"))?;
        // Tokio spawn can fail after OS work has begun. From this boundary on,
        // only the runtime's verified normal stop may authorize intent removal.
        *cleanup = FailedLaunchCleanup::Unconfirmed;
        let mut child = match platform::spawn(command) {
            Ok(child) => child,
            Err(error) => {
                if platform::spawn_failed_before_creation(&error) {
                    *cleanup = FailedLaunchCleanup::NoApplicationSpawned;
                }
                return Err(error);
            }
        };
        let pid = child
            .id()
            .ok_or_else(|| fail("TRIAL_PROCESS_FAILED", "Trial process has no identity"))?;
        log.event("started", json!({"pid":pid,"instanceId":instance_id}));
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail("TRIAL_PROCESS_FAILED", "Missing control output"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| fail("TRIAL_PROCESS_FAILED", "Missing diagnostic stream"))?;
        let (sender, messages) = mpsc::channel(16);
        let ownership = Arc::new(platform::ownership(&child, pid));

        let stopping = Arc::new(AtomicBool::new(false));
        let control_failed = Arc::new(AtomicBool::new(false));
        let reader_stopping = stopping.clone();
        let reader_failed = control_failed.clone();
        let reader_ownership = ownership.clone();
        let reader_log = log.clone();
        let termination_logged = Arc::new(AtomicBool::new(false));
        let reader_termination = termination_logged.clone();
        let control_reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let result = match limited_line(&mut reader).await {
                    Ok(Some(bytes)) => serde_json::from_slice(&bytes)
                        .map_err(|_| fail("TRIAL_PROTOCOL_INVALID", "Invalid control message")),
                    Ok(None) => Err(fail("TRIAL_CONTROL_CLOSED", "Trial control output closed")),
                    Err(error) => Err(error),
                };
                let terminal = result.is_err();
                if terminal && !reader_stopping.load(Ordering::Acquire) {
                    reader_failed.store(true, Ordering::Release);
                    if reader_ownership.observe().ok().flatten().is_none() {
                        log_termination(
                            &reader_log,
                            &reader_termination,
                            RuntimeStopReason::ControlProtocolFailed,
                        );
                    }
                    terminate_unexpected(&reader_ownership, &reader_stopping).await;
                }
                if sender.send(result).await.is_err() || terminal {
                    break;
                }
            }
        });
        let stderr_count = Arc::new(AtomicU64::new(0));
        let counter = stderr_count.clone();
        let diagnostic_log = log.clone();
        let mut redaction_values = config.redaction_values;
        redaction_values.extend([token.clone(), identity_nonce.clone()]);
        let diagnostic_reader = tokio::spawn(async move {
            let mut lines = DiagnosticLines::new(diagnostic_log.clone(), redaction_values);
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        counter.fetch_add(n as u64, Ordering::Relaxed);
                        // A full log rolls over. A write failure is retained while
                        // this reader continues draining without killing the app.
                        lines.push(&chunk[..n]);
                    }
                    Err(_) => {
                        diagnostic_log.event("stderr-read-failed", json!({}));
                        break;
                    }
                }
            }
            lines.finish();
        });
        let mut runtime = Self {
            child: Some(child),
            stdin,
            messages,
            readers: vec![control_reader, diagnostic_reader],
            reader_failure: None,
            token,
            identity_nonce,
            stderr_count,
            stopping,
            control_failed,
            ownership,
            log,
            termination_logged,
            exit_logged: false,
            status: TrialStatus {
                instance_id,
                pid,
                state: "starting".into(),
                port: None,
                exit_code: None,
                stderr_bytes_received: 0,
                log_error: None,
                forced: false,
                stopping_observed: false,
                process_group_exited: false,
                run_directory,
                hard_crash_recovery_implemented: false,
            },
        };
        let mut startup_phase = "initialize";
        let started = tokio::time::timeout(START_TIMEOUT, async {
            platform::activate(runtime.child.as_mut().expect("created runtime child"))?;
            runtime.send(json!({"protocolVersion":1,"type":"initialize","instanceId":runtime.status.instance_id,"token":runtime.token})).await?;
            startup_phase = "ready";
            match runtime.receive().await? {
                ControlMessage::Ready { protocol_version: 1, instance_id, host, port }
                    if instance_id == runtime.status.instance_id && host == "127.0.0.1" && port > 0 => runtime.status.port = Some(port),
                _ => return Err(fail("TRIAL_READY_INVALID", "Trial ready identity or address did not match")),
            }
            startup_phase = "health";
            runtime.verify_health().await?;
            runtime.status.state = "running".into();
            Ok(())
        }).await.unwrap_or_else(|_| Err(fail("TRIAL_START_TIMEOUT", "Trial was not ready within 10 seconds")));
        if let Err(error) = started {
            runtime.log.event(
                "startup-failed",
                json!({
                    "phase": startup_phase,
                    "causeCode": error.code,
                    "httpStatus": error.details.get("httpStatus").and_then(Value::as_u64),
                }),
            );
            match runtime
                .stop_with_reason(RuntimeStopReason::StartupRollback)
                .await
            {
                Ok(status) if status.process_group_exited => {
                    *cleanup = FailedLaunchCleanup::ResourcesStopped;
                }
                Err(_) => {
                    if let Some(pending) = pending {
                        *pending = Some(runtime);
                    }
                }
                _ => (),
            }
            return Err(error);
        }
        Ok(runtime)
    }

    /// Internal gateway access only. The capability is never serialized to a
    /// frontend, and cannot be acquired after exit, protocol loss or shutdown.
    pub(crate) fn proxy_target(&self) -> TrialResult<(u16, String)> {
        if self.status.state != "running"
            || self.child.is_none()
            || self.control_failed.load(Ordering::Acquire)
            || self.stopping.load(Ordering::Acquire)
            || self.token.is_empty()
            || self.ownership.observe()?.is_some()
        {
            return Err(fail(
                "TRIAL_NOT_READY",
                "Application backend is not running",
            ));
        }
        let port = self
            .status
            .port
            .ok_or_else(|| fail("TRIAL_NOT_READY", "Application backend has no private port"))?;
        Ok((port, self.token.clone()))
    }

    /// Capture only while this runtime still holds its original direct Child.
    /// A failed observation leaves the Child and all cleanup work owned here;
    /// callers must complete cleanup before retiring an incomplete start intent.
    /// A trusted entry may exec, so persist the executable the kernel observes.
    pub(crate) fn process_identity(&self) -> TrialResult<RuntimeProcessIdentity> {
        if self.child.is_none() || self.ownership.observe()?.is_some() {
            return Err(fail(
                "TRIAL_IDENTITY_UNAVAILABLE",
                "A live owned runtime is required to bind process identity",
            ));
        }
        let identity = node_process_identity::capture_owned(self.status.pid, &self.identity_nonce)?;
        if self.ownership.observe()?.is_some() {
            return Err(fail(
                "TRIAL_IDENTITY_UNAVAILABLE",
                "The runtime ended before its process identity was bound",
            ));
        }
        Ok(identity)
    }

    async fn send(&mut self, message: Value) -> TrialResult<()> {
        let mut bytes = serde_json::to_vec(&message)
            .map_err(|_| fail("TRIAL_PROTOCOL_INVALID", "Could not encode control message"))?;
        bytes.push(b'\n');
        if bytes.len() > MESSAGE_LIMIT {
            return Err(fail(
                "TRIAL_PROTOCOL_LIMIT",
                "Control message exceeded 8 KiB",
            ));
        }
        let input = self
            .stdin
            .as_mut()
            .ok_or_else(|| fail("TRIAL_CONTROL_CLOSED", "Trial control input is closed"))?;
        input
            .write_all(&bytes)
            .await
            .map_err(|_| fail("TRIAL_CONTROL_CLOSED", "Trial control input closed"))
    }

    async fn receive(&mut self) -> TrialResult<ControlMessage> {
        self.messages
            .recv()
            .await
            .unwrap_or_else(|| Err(fail("TRIAL_CONTROL_CLOSED", "Trial control output closed")))
    }

    /// Explicit observation, not a persistent background monitor.
    pub fn status(&mut self) -> TrialResult<TrialStatus> {
        if self.control_failed.load(Ordering::Acquire) && self.child.is_some() {
            self.status.state = "failed".into();
        }
        if self.child.is_some() {
            if let Some(exit) = self.ownership.observe()? {
                self.status.exit_code = exit.code;
                self.record_exit(exit.code, exit.signal);
                self.status.state = "failed".into();
            }
        }
        let mut status = self.status.clone();
        status.stderr_bytes_received = self.stderr_count.load(Ordering::Relaxed);
        status.log_error = self.log.error().map(|error| error.message);
        Ok(status)
    }

    pub async fn ping(&mut self) -> TrialResult<()> {
        let request_id = Uuid::new_v4().to_string();
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            self.send(json!({"protocolVersion":1,"type":"ping","instanceId":self.status.instance_id,"requestId":request_id})).await?;
            match self.receive().await? {
                ControlMessage::Pong { protocol_version: 1, instance_id, request_id: returned }
                    if instance_id == self.status.instance_id && returned == request_id => Ok(()),
                _ => Err(fail("TRIAL_PONG_INVALID", "Trial pong did not match this request")),
            }
        }).await.unwrap_or_else(|_| Err(fail("TRIAL_PING_TIMEOUT", "Trial control probe timed out")))
    }

    pub async fn verify_health(&self) -> TrialResult<()> {
        let result = async {
            let value = self.get_json("/_platform/health").await?;
            http::validate_health(&value, &self.status.instance_id)
        }
        .await;
        if let Err(error) = &result {
            // Fixed codes/status only: never record tokens, headers or app bodies.
            self.log.event(
                "health-check-failed",
                json!({
                    "causeCode": error.code,
                    "httpStatus": error.details.get("httpStatus").and_then(Value::as_u64),
                }),
            );
        }
        result
    }

    /// Budgeted, authenticated loopback GET for this disposable instance only.
    /// It does not follow redirects or forward platform credentials.
    pub async fn get_json(&self, path: &str) -> TrialResult<Value> {
        let port = self
            .status
            .port
            .ok_or_else(|| fail("TRIAL_NOT_READY", "Trial has not reported a port"))?;
        http::get_json(port, &self.token, path).await
    }

    pub async fn stop(&mut self) -> TrialResult<TrialStatus> {
        self.stop_with_reason(RuntimeStopReason::UserRequest).await
    }

    pub async fn stop_with_reason(
        &mut self,
        reason: RuntimeStopReason,
    ) -> TrialResult<TrialStatus> {
        if self.child.is_none() {
            self.finish_readers().await?;
            return self.status();
        }
        if self.ownership.observe()?.is_none() {
            log_termination(&self.log, &self.termination_logged, reason);
        }
        self.status.state = "stopping".into();
        self.stopping.store(true, Ordering::Release);
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            self.send(
                json!({"protocolVersion":1,"type":"shutdown","instanceId":self.status.instance_id}),
            ),
        )
        .await;
        self.stdin.take();
        // Do not poll Child::wait/try_wait here: the unreaped leader reserves the
        // PGID throughout every delayed control read and descendant cleanup.
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        let mut control_closed = false;
        while self.ownership.observe()?.is_none() && tokio::time::Instant::now() < deadline {
            tokio::select! {
                message = self.messages.recv(), if !control_closed => {
                    match message {
                        Some(Ok(ControlMessage::Stopping { protocol_version: 1, instance_id }))
                            if instance_id == self.status.instance_id => self.status.stopping_observed = true,
                        Some(_) => (),
                        None => control_closed = true,
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(20)) => (),
            }
        }
        if self.ownership.observe()?.is_none() {
            self.status.forced = true;
            self.ownership.signal_with_reobserve(SIGNAL_TERM).await?;
            if !wait_for_owned_exit(&self.ownership, Duration::from_millis(250)).await? {
                self.ownership.signal_with_reobserve(SIGNAL_KILL).await?;
                if !wait_for_owned_exit(&self.ownership, Duration::from_secs(2)).await? {
                    return Err(fail(
                        "TRIAL_STOP_FAILED",
                        "Owned trial did not exit after forced termination",
                    ));
                }
            }
        }
        // The exited child is still unreaped while output finishes delivery.
        if !self.status.stopping_observed {
            if let Ok(Some(Ok(ControlMessage::Stopping {
                protocol_version: 1,
                instance_id,
            }))) = tokio::time::timeout(Duration::from_millis(100), self.messages.recv()).await
            {
                self.status.stopping_observed = instance_id == self.status.instance_id;
            }
        }
        // A clean main-process exit alone does not prove its ordinary children exited.
        let descendants_exited = if self.ownership.has_descendants()? {
            log_termination(&self.log, &self.termination_logged, reason);
            self.status.forced = true;
            self.ownership.signal_with_reobserve(SIGNAL_TERM).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            if self.ownership.has_descendants()? {
                self.ownership.signal_with_reobserve(SIGNAL_KILL).await?;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while self.ownership.has_descendants()? && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            !self.ownership.has_descendants()?
        } else {
            true
        };
        if !descendants_exited {
            // Keep the unreaped leader and ownership guard so a later explicit
            // stop can safely retry this exact group. Never release the PGID
            // and then attempt to signal it by an old numeric identifier.
            self.status.state = "failed".into();
            return Err(fail(
                "TRIAL_DESCENDANTS_REMAIN",
                "Could not confirm the trial process group exited; owned cleanup can be retried",
            ));
        }
        // Atomically forbid all future group signals before the only reaping wait.
        self.ownership.disarm_before_reap();
        let exit = self
            .child
            .as_mut()
            .unwrap()
            .wait()
            .await
            .map_err(|_| fail("TRIAL_STOP_FAILED", "Could not reap the owned trial leader"))?;
        self.status.exit_code = exit.code();
        let signal = platform_native::process_status::signal(&exit);
        self.record_exit(exit.code(), signal);
        self.status.state = if self.status.forced
            || self.control_failed.load(Ordering::Acquire)
            || !self.status.stopping_observed
            || !exit.success()
        {
            "failed"
        } else {
            "stopped"
        }
        .into();
        self.child.take();
        self.status.process_group_exited = true;
        self.token.clear();
        self.identity_nonce.clear();
        self.finish_readers().await?;
        self.status()
    }

    fn record_exit(&mut self, code: Option<i32>, signal: Option<i32>) {
        if !self.exit_logged {
            self.log
                .event("exited", json!({"exitCode":code,"signal":signal}));
            self.exit_logged = true;
        }
    }

    async fn finish_readers(&mut self) -> TrialResult<()> {
        // Queued control delivery cannot prevent an accepted reader from ending.
        // Keep every unobserved handle in Self, including a lost stop waiter.
        self.messages.close();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while let Some(reader) = self.readers.first_mut() {
            match tokio::time::timeout_at(deadline, reader).await {
                Ok(result) => {
                    self.readers.remove(0);
                    if result.is_err() && self.reader_failure.is_none() {
                        self.status.state = "failed".into();
                        self.reader_failure = Some(fail(
                            "TRIAL_READER_JOIN_FAILED",
                            "Runtime output work did not join normally; its failure is retained",
                        ).detail("processGroupExited", self.status.process_group_exited).detail("ioClosed", false));
                    }
                }
                Err(_) => {
                    return Err(fail(
                        "TRIAL_READER_CLEANUP_PENDING",
                        "Runtime output work has not finished; its actual handles are retained",
                    )
                    .detail("processGroupExited", self.status.process_group_exited)
                    .detail("ioClosed", false))
                }
            }
        }
        platform::finish_io(&self.ownership, deadline).await?;
        match &self.reader_failure {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture_reader_failure(&mut self) {
        self.readers.push(tokio::spawn(async {
            panic!("controlled disposable runtime reader failure");
        }));
    }
}

#[cfg(test)]
fn group_exists(pid: u32) -> bool {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

async fn wait_for_owned_exit(ownership: &ProcessOwnership, timeout: Duration) -> TrialResult<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if ownership.observe()?.is_some() {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn terminate_unexpected(ownership: &ProcessOwnership, stopping: &AtomicBool) {
    if !stopping.load(Ordering::Acquire) {
        let _ = ownership.signal_with_reobserve(SIGNAL_TERM).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        if !stopping.load(Ordering::Acquire) {
            let _ = ownership.signal_with_reobserve(SIGNAL_KILL).await;
        }
    }
}

fn log_termination(log: &RuntimeLog, recorded: &AtomicBool, reason: RuntimeStopReason) {
    if !recorded.swap(true, Ordering::AcqRel) {
        log.event("termination-requested", json!({"reason":reason.label()}));
    }
}

impl Drop for NodeRuntimeTrial {
    fn drop(&mut self) {
        platform::drop_runtime(self);
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "node_runtime_signal_tests.rs"]
mod signal_reobserve_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn protocol_fixture(behavior: &str) -> (tempfile::TempDir, RuntimeLaunchConfig) {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::Builder::new()
            .prefix("node-runtime-fixture-")
            .tempdir()
            .unwrap();
        let root = fs::canonicalize(fixture.path()).unwrap();
        let app = root.join("app");
        fs::create_dir(&app).unwrap();
        let node = root.join("mock-node");
        // Real child-process/HTTP lifecycle with a fake interpreter identity. This
        // does not prove compatibility of the actual pinned Node distribution.
        fs::write(&node, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /usr/bin/python3 \"$@\"\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let entry = app.join("protocol-fixture.mjs");
        fs::write(&entry, r#"import sys, os, json, threading, signal
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
message = json.loads(sys.stdin.readline())
instance = message['instanceId']
token = message['token']
behavior = os.environ.get('TRIAL_BEHAVIOR', '')
assert os.environ['APP_INSTANCE_ID'] == instance
identity_nonce = os.environ['PANDA_DRAWER_RUN_IDENTITY']
assert len(identity_nonce) == 64 and all(ch in '0123456789abcdef' for ch in identity_nonce)
assert identity_nonce != token
assert 'NODE_OPTIONS' not in os.environ and 'HOME' not in os.environ and 'PATH' not in os.environ
assert os.path.isdir(os.environ['APP_DATA_DIR']) and os.path.isdir(os.environ['APP_CACHE_DIR'])
if behavior == 'runtime-identity':
    with open(os.path.join(os.environ['APP_DATA_DIR'], 'artificial-process-identity.json'), 'w') as output:
        json.dump({'pid':os.getpid(), 'pgid':os.getpgrp(), 'nonce':identity_nonce}, output)
if behavior in ['leak-descendants', 'exit-with-descendants']:
    import subprocess,time
    grand = "import os,signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);open(os.path.join(os.environ['APP_DATA_DIR'],'grandchild-pid'),'w').write(str(os.getpid()));time.sleep(60)"
    child = "import os,signal,subprocess,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);open(os.path.join(os.environ['APP_DATA_DIR'],'child-pid'),'w').write(str(os.getpid()));subprocess.Popen(['/usr/bin/python3','-c'," + repr(grand) + "],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);time.sleep(60)"
    subprocess.Popen(['/usr/bin/python3','-c',child],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    deadline=time.monotonic()+2
    while not os.path.isfile(os.path.join(os.environ['APP_DATA_DIR'],'grandchild-pid')):
        if time.monotonic()>deadline: raise RuntimeError('artificial descendants were not ready')
        time.sleep(.01)
if behavior == 'persistent-storage':
    visit = os.path.join(os.environ['APP_DATA_DIR'], 'artificial-visits.txt')
    count = int(open(visit).read()) if os.path.exists(visit) else 0
    with open(visit, 'w') as output: output.write(str(count + 1))
    with open(os.path.join(os.environ['APP_DATA_DIR'], 'artificial-last-temp.txt'), 'w') as output:
        output.write(os.environ['APP_TEMP_DIR'])
    assert not os.path.exists(os.path.join(os.environ['APP_TEMP_DIR'], 'fresh-marker'))
    with open(os.path.join(os.environ['APP_TEMP_DIR'], 'fresh-marker'), 'w') as output: output.write('temporary')
if behavior == 'oversized':
    print('x' * 8193, flush=True)
    sys.exit(0)
if behavior == 'closed':
    sys.exit(0)
class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    health_requests = 0
    def log_message(self, *args): pass
    def do_GET(self):
        if self.headers.get('Authorization') != 'Bearer ' + token:
            self.send_response(401); self.send_header('Content-Length','0'); self.end_headers(); return
        Handler.health_requests += 1
        if behavior == 'health-http-status' or (behavior == 'health-becomes-unhealthy' and Handler.health_requests > 1):
            self.send_response(503); self.send_header('Content-Length','0'); self.end_headers(); return
        body = json.dumps({'protocolVersion':1, 'instanceId':instance, 'status':'ok'}).encode()
        self.send_response(200); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
print(json.dumps({'protocolVersion':1,'type':'ready','instanceId':instance,'host':'127.0.0.1','port':server.server_address[1]}), flush=True)
print('artificial-secret-for-log-test', file=sys.stderr, flush=True)
print('control-token=' + token + ' identity-nonce=' + identity_nonce, file=sys.stderr, flush=True)
if behavior == 'verbose-stderr':
    for i in range(1024):
        print(str(i) + ' artificial-secret-for-log-test ' + ('x' * 1024), file=sys.stderr)
    print('newest-diagnostic', file=sys.stderr, flush=True)
for line in sys.stdin:
    msg = json.loads(line)
    if msg['type'] == 'ping':
        if behavior == 'close-on-ping':
            os.close(sys.stdout.fileno())
            import time
            while True: time.sleep(1)
        print(json.dumps({'protocolVersion':1,'type':'pong','instanceId':instance,'requestId':msg['requestId']}), flush=True)
        if behavior in ['exit-after-pong', 'exit-with-descendants']: sys.exit(0)
    elif msg['type'] == 'shutdown':
        if behavior == 'hang-stop':
            import subprocess
            subprocess.Popen(['/usr/bin/python3', '-c', 'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            import time
            while True: time.sleep(1)
        print(json.dumps({'protocolVersion':1,'type':'stopping','instanceId':instance}), flush=True)
        server.shutdown(); server.server_close()
        sys.exit(0)
server.shutdown(); server.server_close()
"#).unwrap();
        let trial_root = root.join("node-runtime-trial-data");
        fs::create_dir(&trial_root).unwrap();
        let expected_node_sha256 = format!("{:x}", Sha256::digest(fs::read(&node).unwrap()));
        (
            fixture,
            RuntimeLaunchConfig {
                node_path: node,
                expected_node_sha256,
                entry_path: entry,
                working_directory: app,
                trial_root,
                business_env: BTreeMap::from([("TRIAL_BEHAVIOR".into(), behavior.into())]),
                control_directory: root,
                redaction_values: vec!["artificial-secret-for-log-test".into()],
            },
        )
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn real_mock_process_health_control_and_repeated_stop() {
        let (_fixture, config) = protocol_fixture("");
        let control = config.control_directory.clone();
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.ping().await.unwrap();
        runtime.verify_health().await.unwrap();
        let running = runtime.status().unwrap();
        let (port, token) = runtime.proxy_target().unwrap();
        assert_eq!(Some(port), running.port);
        assert_eq!(token.len(), 64);
        let identity_nonce = runtime.identity_nonce.clone();
        assert_eq!(running.state, "running");
        assert!(!serde_json::to_string(&running)
            .unwrap()
            .contains("artificial-secret"));
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(stopped.stopping_observed);
        assert!(!stopped.forced);
        assert!(stopped.process_group_exited);
        assert!(!group_exists(stopped.pid));
        assert_eq!(runtime.stop().await.unwrap().state, "stopped");
        assert!(runtime.proxy_target().is_err());
        let output = fs::read_to_string(control.join("logs/current.log")).unwrap();
        assert!(!output.contains(&token));
        assert!(!output.contains(&identity_nonce));
        assert!(!output.contains("artificial-secret-for-log-test"));
        assert!(output.contains("control-token=*** identity-nonce=***"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn large_stderr_keeps_runtime_alive_and_rotates_private_redacted_logs() {
        use std::os::unix::fs::PermissionsExt;
        let (fixture, config) = protocol_fixture("verbose-stderr");
        let control = config.control_directory.clone();
        let mut next = RuntimeLaunchConfig {
            node_path: config.node_path.clone(),
            expected_node_sha256: config.expected_node_sha256.clone(),
            entry_path: config.entry_path.clone(),
            working_directory: config.working_directory.clone(),
            trial_root: control.join("node-runtime-trial-next"),
            business_env: BTreeMap::new(),
            control_directory: control.clone(),
            redaction_values: config.redaction_values.clone(),
        };
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        // The fixture handles ping only after more than 1 MiB of stderr.
        runtime.ping().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let bytes = fs::read(control.join("logs/current.log")).unwrap();
                if bytes
                    .windows(b"newest-diagnostic".len())
                    .any(|w| w == b"newest-diagnostic")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let status = runtime.status().unwrap();
        assert_eq!(status.state, "running");
        assert!(status.stderr_bytes_received > 1024 * 1024);
        assert!(status.log_error.is_none());
        runtime
            .stop_with_reason(RuntimeStopReason::Restart)
            .await
            .unwrap();
        let previous = fs::read_to_string(control.join("logs/current.log")).unwrap();
        assert!(previous.len() <= crate::node_runtime_log::LOG_LIMIT);
        assert!(!previous.contains("artificial-secret-for-log-test"));
        assert!(previous.contains("***"));
        assert!(previous.contains("\"reason\":\"restart\""));
        assert!(previous.contains("\"exitCode\":0"));
        assert_eq!(previous.matches("\"platformEvent\":\"exited\"").count(), 1);
        assert_eq!(
            fs::metadata(control.join("logs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(control.join("logs/current.log"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::create_dir(&next.trial_root).unwrap();
        // Read-only preflight must not rotate the completed run.
        let data = control.join("preflight-data");
        let cache = control.join("preflight-cache");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&cache).unwrap();
        NodeRuntimeTrial::preflight(&next, &data, &cache)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(control.join("logs/current.log")).unwrap(),
            previous
        );
        next.business_env.insert("TRIAL_BEHAVIOR".into(), "".into());
        let mut restarted = NodeRuntimeTrial::launch(next).await.unwrap();
        assert_eq!(
            fs::read_to_string(control.join("logs/previous.log")).unwrap(),
            previous
        );
        assert_eq!(
            fs::metadata(control.join("logs/previous.log"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        restarted.stop().await.unwrap();
        drop(fixture);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn runtime_log_failure_is_reported_while_stderr_is_still_drained() {
        let (_fixture, config) = protocol_fixture("verbose-stderr");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        {
            // A real failing file descriptor (read-only), without a disk-full
            // requirement or touching any user files.
            let path = runtime.status.run_directory.join("read-only-log-fixture");
            fs::write(&path, b"read only").unwrap();
            runtime
                .log
                .fixture_replace_file(fs::File::open(path).unwrap());
            runtime.log.line(b"artificial write failure\n");
        }
        runtime.ping().await.unwrap();
        assert_eq!(runtime.status().unwrap().state, "running");
        assert!(runtime.status().unwrap().log_error.is_some());
        assert!(runtime.stop().await.unwrap().process_group_exited);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn run_identity_nonce_is_unique_separate_from_bearer_and_injected_into_owned_group() {
        let mut previous_nonce = None;
        for _ in 0..2 {
            let (_fixture, config) = protocol_fixture("runtime-identity");
            let launch_executable = fs::canonicalize(&config.node_path).unwrap();
            let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
            let observed: Value = serde_json::from_slice(
                &fs::read(
                    runtime
                        .status
                        .run_directory
                        .join("data/artificial-process-identity.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(observed["pid"], runtime.status.pid);
            assert_eq!(observed["pgid"], runtime.status.pid);
            assert_eq!(observed["nonce"], runtime.identity_nonce);
            assert_eq!(runtime.identity_nonce.len(), 64);
            assert_ne!(runtime.identity_nonce, runtime.token);
            if let Some(previous) = previous_nonce {
                assert_ne!(runtime.identity_nonce, previous);
            }
            let public = serde_json::to_string(&runtime.status().unwrap()).unwrap();
            assert!(!public.contains(&runtime.identity_nonce));
            assert!(!public.contains(&runtime.token));

            // Retained Child ownership permits recording the actual executable
            // after this fixture deliberately execs Python through a wrapper.
            let identity = runtime.process_identity().unwrap();
            assert_eq!(identity.pid, runtime.status.pid);
            assert_eq!(identity.pgid, runtime.status.pid);
            assert_eq!(identity.nonce, runtime.identity_nonce);
            assert!(identity.executable.is_absolute());
            assert_ne!(identity.executable, launch_executable);
            assert_eq!(
                node_process_identity::observe(&identity).unwrap(),
                node_process_identity::Observation::Matching
            );

            // Failed identity binding must keep the cleanup owner intact.
            let nonce = runtime.identity_nonce.clone();
            runtime.identity_nonce = "0".repeat(64);
            assert!(runtime.process_identity().is_err());
            assert!(runtime.child.is_some());
            runtime.identity_nonce = nonce;
            runtime.ping().await.unwrap();
            assert_eq!(runtime.process_identity().unwrap(), identity);
            previous_nonce = Some(runtime.identity_nonce.clone());
            assert!(runtime.stop().await.unwrap().process_group_exited);
            assert_eq!(
                runtime.process_identity().unwrap_err().code,
                "TRIAL_IDENTITY_UNAVAILABLE"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn persistent_storage_survives_restart_but_run_and_temp_are_fresh() {
        let (fixture, config) = protocol_fixture("persistent-storage");
        let root = fs::canonicalize(fixture.path()).unwrap();
        let data = root.join("persistent-data");
        let cache = root.join("persistent-cache");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("artificial-cache-marker"), b"retained").unwrap();
        let second_root = root.join("node-runtime-trial-second");
        fs::create_dir(&second_root).unwrap();
        let second_config = RuntimeLaunchConfig {
            node_path: config.node_path.clone(),
            expected_node_sha256: config.expected_node_sha256.clone(),
            entry_path: config.entry_path.clone(),
            working_directory: config.working_directory.clone(),
            business_env: config.business_env.clone(),
            trial_root: second_root,
            control_directory: config.control_directory.clone(),
            redaction_values: config.redaction_values.clone(),
        };
        let mut first = NodeRuntimeTrial::launch_with_storage(config, data.clone(), cache.clone())
            .await
            .unwrap();
        let first_run = first.status().unwrap().run_directory;
        assert!(!first_run.join("data").exists());
        assert!(!first_run.join("cache").exists());
        let first_temp = fs::read_to_string(data.join("artificial-last-temp.txt")).unwrap();
        first.stop().await.unwrap();
        let mut second =
            NodeRuntimeTrial::launch_with_storage(second_config, data.clone(), cache.clone())
                .await
                .unwrap();
        let second_temp = fs::read_to_string(data.join("artificial-last-temp.txt")).unwrap();
        assert_eq!(
            fs::read_to_string(data.join("artificial-visits.txt")).unwrap(),
            "2"
        );
        assert_eq!(
            fs::read(cache.join("artificial-cache-marker")).unwrap(),
            b"retained"
        );
        assert_ne!(first_temp, second_temp);
        assert!(Path::new(&second_temp).starts_with(second.status().unwrap().run_directory));
        assert!(!second.stop().await.unwrap().forced);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn persistent_storage_refuses_links_source_and_run_overlap() {
        use std::os::unix::fs::symlink;
        let (fixture, config) = protocol_fixture("");
        let root = fs::canonicalize(fixture.path()).unwrap();
        let data = root.join("persistent-data");
        let cache = root.join("persistent-cache");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::create_dir(data.join("nested")).unwrap();
        fs::create_dir(config.working_directory.join("data")).unwrap();
        for (candidate_data, candidate_cache) in [
            (data.clone(), data.clone()),
            (data.clone(), data.join("nested")),
            (config.working_directory.clone(), cache.clone()),
            (config.working_directory.join("data"), cache.clone()),
            (root.clone(), cache.clone()),
            (config.trial_root.clone(), cache.clone()),
        ] {
            assert!(checked_storage(
                candidate_data,
                candidate_cache,
                &config.working_directory,
                &config.trial_root
            )
            .is_err());
        }
        let linked = root.join("linked-storage");
        symlink(&data, &linked).unwrap();
        assert!(checked_storage(
            linked,
            cache.clone(),
            &config.working_directory,
            &config.trial_root
        )
        .is_err());
        assert!(
            checked_storage(data, cache, &config.working_directory, &config.trial_root).is_ok()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn project_storage_allows_backend_root_and_preserves_existing_user_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (fixture, config) = protocol_fixture("");
        let source = config.working_directory.clone();
        let cache = fs::canonicalize(fixture.path())
            .unwrap()
            .join("project-cache");
        fs::create_dir(&cache).unwrap();
        let data = source.join("data");
        let checked =
            platform::checked_project_storage(&source, &cache, &source, &config.trial_root)
                .unwrap();
        assert_eq!(checked.0, data);
        assert!(
            !data.exists(),
            "project storage preflight must stay read-only"
        );
        platform::prepare_project_data(&source).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o750)).unwrap();
        fs::write(data.join("retained-user-file"), b"retain existing data").unwrap();
        let run = config.trial_root.join("disposable-project-run");
        platform::prepare_directories(&run, Some((data.clone(), cache.clone())), false).unwrap();
        assert_eq!(
            fs::metadata(&data).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            fs::read(data.join("retained-user-file")).unwrap(),
            b"retain existing data"
        );
        assert!(run.join("tmp").is_dir());
        assert!(!run.join("data").exists());
        fs::rename(&data, source.join("retained-data")).unwrap();
        symlink(source.join("retained-data"), &data).unwrap();
        assert!(
            platform::checked_project_storage(&source, &cache, &source, &config.trial_root)
                .is_err()
        );
        assert!(platform::prepare_project_data(&source).is_err());
        assert_eq!(
            fs::read(source.join("retained-data/retained-user-file")).unwrap(),
            b"retain existing data"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn project_launch_creates_only_project_data_and_keeps_run_temporary_private() {
        let (fixture, config) = protocol_fixture("persistent-storage");
        let source = config.working_directory.clone();
        let cache = fs::canonicalize(fixture.path())
            .unwrap()
            .join("project-cache");
        fs::create_dir(&cache).unwrap();
        NodeRuntimeTrial::preflight_project_with_processes(
            &config,
            &source,
            &cache,
            &OwnedProcessRegistry::default(),
        )
        .await
        .unwrap();
        assert!(!source.join("data").exists());
        let (started, pending, _) = NodeRuntimeTrial::launch_project_retaining_failed(
            config,
            source.clone(),
            cache,
            &OwnedProcessRegistry::default(),
        )
        .await;
        assert!(pending.is_none());
        let mut runtime = started.unwrap();
        let run = runtime.status().unwrap().run_directory;
        assert_eq!(
            fs::read_to_string(source.join("data/artificial-visits.txt")).unwrap(),
            "1"
        );
        let temporary = fs::read_to_string(source.join("data/artificial-last-temp.txt")).unwrap();
        assert!(Path::new(&temporary).starts_with(&run));
        assert!(!run.join("data").exists());
        assert!(!run.join("cache").exists());
        runtime.stop().await.unwrap();
        assert!(source.join("data/artificial-visits.txt").is_file());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn preflight_never_executes_application_or_writes_run_storage() {
        let (fixture, mut config) = protocol_fixture("persistent-storage");
        let root = fs::canonicalize(fixture.path()).unwrap();
        let data = root.join("persistent-data");
        let cache = root.join("persistent-cache");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&cache).unwrap();
        NodeRuntimeTrial::preflight(&config, &data, &cache)
            .await
            .unwrap();
        assert_eq!(fs::read_dir(&data).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&config.trial_root).unwrap().count(), 0);
        config.expected_node_sha256 = "0".repeat(64);
        assert_eq!(
            NodeRuntimeTrial::preflight(&config, &data, &cache)
                .await
                .unwrap_err()
                .code,
            "TRIAL_NODE_DIGEST_MISMATCH"
        );
        assert_eq!(fs::read_dir(&config.trial_root).unwrap().count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn control_failure_never_becomes_a_ready_instance() {
        for (behavior, expected) in [
            ("oversized", "TRIAL_PROTOCOL_LIMIT"),
            ("closed", "TRIAL_CONTROL_CLOSED"),
        ] {
            let (_fixture, config) = protocol_fixture(behavior);
            let control = config.control_directory.clone();
            let error = NodeRuntimeTrial::launch(config).await.err().unwrap();
            assert_eq!(error.code, expected);
            let output = fs::read_to_string(control.join("logs/current.log")).unwrap();
            let failure = output
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|event| event["platformEvent"] == "startup-failed")
                .unwrap();
            assert_eq!(failure["causeCode"], expected);
            assert_eq!(failure["phase"], "ready");
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn health_failures_record_safe_cause_at_startup_and_while_running() {
        for behavior in ["health-http-status", "health-becomes-unhealthy"] {
            let (_fixture, config) = protocol_fixture(behavior);
            let control = config.control_directory.clone();
            let launched = NodeRuntimeTrial::launch(config).await;
            if behavior == "health-http-status" {
                assert_eq!(launched.err().unwrap().code, "TRIAL_HTTP_STATUS");
            } else {
                let mut runtime = launched.unwrap();
                assert_eq!(
                    runtime.verify_health().await.unwrap_err().code,
                    "TRIAL_HTTP_STATUS"
                );
                assert!(runtime.stop().await.unwrap().process_group_exited);
            }
            let output = fs::read_to_string(control.join("logs/current.log")).unwrap();
            let events = output
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect::<Vec<_>>();
            let health_failure = events
                .iter()
                .find(|event| event["platformEvent"] == "health-check-failed")
                .unwrap();
            assert_eq!(health_failure["causeCode"], "TRIAL_HTTP_STATUS");
            assert_eq!(health_failure["httpStatus"], 503);
            assert!(health_failure.get("token").is_none());
            if behavior == "health-http-status" {
                let startup = events
                    .iter()
                    .find(|event| event["platformEvent"] == "startup-failed")
                    .unwrap();
                assert_eq!(startup["phase"], "health");
                assert_eq!(startup["causeCode"], "TRIAL_HTTP_STATUS");
                assert_eq!(startup["httpStatus"], 503);
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn uncooperative_process_is_forced_and_reaped() {
        let (_fixture, config) = protocol_fixture("hang-stop");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        let stopped = runtime.stop().await.unwrap();
        assert!(stopped.forced);
        assert!(stopped.process_group_exited);
        assert_eq!(stopped.state, "failed");
        assert!(!group_exists(stopped.pid));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn clean_leader_handshake_still_removes_stubborn_child_and_grandchild() {
        let (_fixture, config) = protocol_fixture("leak-descendants");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        let data = runtime.status.run_directory.join("data");
        let descendants: [i32; 2] = ["child-pid", "grandchild-pid"].map(|name| {
            fs::read_to_string(data.join(name))
                .unwrap()
                .parse()
                .unwrap()
        });
        let stopped = runtime.stop().await.unwrap();
        assert!(stopped.stopping_observed);
        assert_eq!(stopped.exit_code, Some(0));
        assert!(stopped.forced);
        assert!(stopped.process_group_exited);
        for pid in descendants {
            assert!(!process_is_live(pid));
        }
        assert!(runtime.stop().await.unwrap().process_group_exited);
    }

    #[cfg(target_os = "macos")]
    fn process_is_live(pid: i32) -> bool {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info);
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size as i32,
            )
        };
        got == size as i32 && info.pbi_status != libc::SZOMB
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn dead_zombie_descendant_does_not_count_as_a_running_process() {
        let mut leader=Command::new("/usr/bin/python3")
            .args(["-c","import subprocess,time;child=subprocess.Popen(['/usr/bin/true']);print(child.pid,flush=True);time.sleep(60)"])
            .stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).kill_on_drop(true).spawn().unwrap();
        let pid = leader.id().unwrap();
        let mut output = BufReader::new(leader.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).await.unwrap();
        let child_pid: i32 = line.trim().parse().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            // A disappearing proc_pidinfo lookup is not yet an empty-group
            // receipt. Wait for the same stable enumeration used by stop.
            while group_has_other_members(pid).unwrap() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!process_is_live(child_pid));
        assert!(!group_has_other_members(pid).unwrap());
        let owner = ProcessOwnership::new(pid);
        owner.signal(SIGNAL_KILL).unwrap();
        owner.disarm_before_reap();
        leader.wait().await.unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn lost_control_channel_closes_the_existing_instance() {
        let (_fixture, config) = protocol_fixture("close-on-ping");
        let control = config.control_directory.clone();
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        assert_eq!(
            runtime.ping().await.unwrap_err().code,
            "TRIAL_CONTROL_CLOSED"
        );
        assert!(runtime.proxy_target().is_err());
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "failed");
        assert!(!group_exists(stopped.pid));
        let log = fs::read_to_string(control.join("logs/current.log")).unwrap();
        assert!(log.contains("control-protocol-failed"));
        assert!(log.contains("\"signal\":15") || log.contains("\"signal\":9"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn dropping_a_live_handle_cleans_up_its_owned_group_in_the_active_runtime() {
        let (_fixture, config) = protocol_fixture("");
        let runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        let pid = runtime.status.pid;
        drop(runtime);
        tokio::time::timeout(Duration::from_secs(2), async {
            while group_exists(pid) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn exited_leader_stays_owned_across_repeated_status_and_delayed_stop() {
        let (_fixture, config) = protocol_fixture("exit-after-pong");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.ping().await.unwrap();
        assert!(
            wait_for_owned_exit(&runtime.ownership, Duration::from_secs(2))
                .await
                .unwrap()
        );
        for _ in 0..5 {
            assert_eq!(runtime.status().unwrap().state, "failed");
            assert!(runtime.ownership.observe().unwrap().is_some());
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        let owner = runtime.ownership.clone();
        let pid = runtime.status.pid;
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "failed");
        assert!(
            observe_without_reaping(pid).is_err(),
            "final wait must reap only after cleanup"
        );
        let signals = owner.signals_sent.load(Ordering::Relaxed);
        assert_eq!(
            owner.signal(SIGNAL_KILL).unwrap_err().code,
            "TRIAL_OWNERSHIP_RELEASED"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(owner.signals_sent.load(Ordering::Relaxed), signals);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn descendant_cleanup_logs_reason_after_leader_already_exited() {
        let (_fixture, config) = protocol_fixture("exit-with-descendants");
        let control = config.control_directory.clone();
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.ping().await.unwrap();
        assert!(
            wait_for_owned_exit(&runtime.ownership, Duration::from_secs(2))
                .await
                .unwrap()
        );
        runtime.status().unwrap();
        let stopped = runtime
            .stop_with_reason(RuntimeStopReason::HealthCheckFailed)
            .await
            .unwrap();
        assert!(stopped.forced && stopped.process_group_exited);
        let output = fs::read_to_string(control.join("logs/current.log")).unwrap();
        assert!(output.contains("health-check-failed"));
        assert_eq!(output.matches("\"platformEvent\":\"exited\"").count(), 1);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn exited_leader_stays_owned_across_repeated_status_and_delayed_drop() {
        let (_fixture, config) = protocol_fixture("exit-after-pong");
        let control = config.control_directory.clone();
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.ping().await.unwrap();
        assert!(
            wait_for_owned_exit(&runtime.ownership, Duration::from_secs(2))
                .await
                .unwrap()
        );
        for _ in 0..5 {
            runtime.status().unwrap();
            assert!(runtime.ownership.observe().unwrap().is_some());
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        let owner = runtime.ownership.clone();
        let pid = runtime.status.pid;
        drop(runtime);
        let signals = owner.signals_sent.load(Ordering::Relaxed);
        assert_eq!(
            owner.signal(SIGNAL_TERM).unwrap_err().code,
            "TRIAL_OWNERSHIP_RELEASED"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while observe_without_reaping(pid).is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(owner.signals_sent.load(Ordering::Relaxed), signals);
        let output = fs::read_to_string(control.join("logs/current.log")).unwrap();
        assert!(!output.contains("runtime-owner-dropped"));
        assert_eq!(output.matches("\"platformEvent\":\"exited\"").count(), 1);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn externally_reaped_pid_is_disarmed_before_any_group_signal() {
        let mut child = Command::new("/usr/bin/true")
            .process_group(0)
            .kill_on_drop(false)
            .spawn()
            .unwrap();
        let owner = ProcessOwnership::new(child.id().unwrap());
        child.wait().await.unwrap();
        assert_eq!(
            owner.signal(SIGNAL_KILL).unwrap_err().code,
            "TRIAL_PROCESS_OWNERSHIP_LOST"
        );
        assert_eq!(
            owner.signal(SIGNAL_TERM).unwrap_err().code,
            "TRIAL_OWNERSHIP_RELEASED"
        );
        assert_eq!(owner.signals_sent.load(Ordering::Relaxed), 0);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn owned_version_probe_stops_silent_descendants_and_bounds_output_while_reading() {
        use std::os::unix::fs::PermissionsExt;
        let (_fixture, config) = protocol_fixture("");
        let marker = config.node_path.with_file_name("late-version-child");
        fs::write(&config.node_path, format!("#!/bin/sh\n(/bin/sleep 0.3; printf unexpected > '{}') </dev/null >/dev/null 2>&1 &\nprintf 'v24.22.0\\n'\n", marker.display())).unwrap();
        fs::set_permissions(&config.node_path, fs::Permissions::from_mode(0o700)).unwrap();
        let processes = OwnedProcessRegistry::default();
        verify_node_version(&config.node_path, &processes)
            .await
            .unwrap();
        assert!(processes.shutdown().await.is_empty());
        assert!(processes
            .snapshot()
            .iter()
            .all(|record| record.resource_closed()));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!marker.exists());

        let processes = OwnedProcessRegistry::default();
        fs::write(
            &config.node_path,
            "#!/bin/sh\nwhile :; do printf 'too-much-version-output'; done\n",
        )
        .unwrap();
        assert_eq!(
            verify_node_version(&config.node_path, &processes)
                .await
                .unwrap_err()
                .code,
            "TRIAL_NODE_VERSION"
        );
        assert!(processes.shutdown().await.is_empty());
        assert!(processes
            .snapshot()
            .iter()
            .all(|record| record.resource_closed()));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn lost_version_probe_waiter_retains_owned_group_and_real_readers_until_join() {
        use std::os::unix::fs::PermissionsExt;
        let (_fixture, config) = protocol_fixture("");
        let marker = config.node_path.with_file_name("version-probe-started");
        fs::write(
            &config.node_path,
            format!(
                "#!/bin/sh\nprintf started > '{}'\n/bin/sleep 60 & wait\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&config.node_path, fs::Permissions::from_mode(0o700)).unwrap();
        let processes = OwnedProcessRegistry::default();
        let owned = processes.clone();
        let path = config.node_path.clone();
        let waiter = tokio::spawn(async move { verify_node_version(&path, &owned).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(!processes.snapshot().is_empty());
        assert!(processes.shutdown().await.is_empty());
        assert!(processes
            .snapshot()
            .iter()
            .all(|record| record.resource_closed()));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn pending_reader_retains_its_actual_handle_across_stop_retry_and_waiter_loss() {
        let (_fixture, config) = protocol_fixture("");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        let marker = runtime.status.run_directory.join("actual-reader-finished");
        let (release, wait) = tokio::sync::oneshot::channel();
        let written = marker.clone();
        runtime.readers.push(tokio::spawn(async move {
            let _ = wait.await;
            fs::write(written, b"actual registered work finished").unwrap();
        }));
        let error = runtime.stop().await.unwrap_err();
        assert_eq!(error.code, "TRIAL_READER_CLEANUP_PENDING");
        assert!(runtime.child.is_none());
        assert!(runtime.status.process_group_exited);
        assert_eq!(runtime.readers.len(), 1);
        assert!(!marker.exists());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), runtime.stop())
                .await
                .is_err()
        );
        assert_eq!(
            runtime.readers.len(),
            1,
            "losing the retry waiter lost its actual JoinHandle"
        );
        release.send(()).unwrap();
        runtime.stop().await.unwrap();
        assert!(runtime.readers.is_empty());
        assert_eq!(
            fs::read(marker).unwrap(),
            b"actual registered work finished"
        );
        runtime.stop().await.unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn reader_join_failure_remains_blocked_after_child_and_handle_slots_are_empty() {
        let (_fixture, config) = protocol_fixture("");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.readers.push(tokio::spawn(async {
            panic!("controlled disposable runtime reader failure");
        }));
        let first = runtime.stop().await.unwrap_err();
        assert_eq!(first.code, "TRIAL_READER_JOIN_FAILED");
        assert!(runtime.child.is_none());
        assert!(runtime.readers.is_empty());
        assert_eq!(runtime.stop().await.unwrap_err().code, first.code);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn release_digest_and_path_checks_precede_execution() {
        use std::os::unix::fs::symlink;
        let (_fixture, mut config) = protocol_fixture("");
        config.expected_node_sha256 = "0".repeat(64);
        assert_eq!(
            NodeRuntimeTrial::launch(config).await.err().unwrap().code,
            "TRIAL_NODE_DIGEST_MISMATCH"
        );
        let (_fixture, mut config) = protocol_fixture("");
        let linked = config.working_directory.join("linked.mjs");
        symlink(&config.entry_path, &linked).unwrap();
        config.entry_path = linked;
        assert_eq!(
            NodeRuntimeTrial::launch(config).await.err().unwrap().code,
            "TRIAL_PATH_INVALID"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Requires an explicit verified private Node path in NODE_RUNTIME_TRIAL_TEST_NODE"]
    async fn pinned_node_runs_disposable_javascript_and_stops() {
        let node_path = PathBuf::from(
            std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE").expect("explicit private Node path"),
        );
        let (_fixture, mut config) = protocol_fixture("");
        config.node_path = node_path;
        let release: Value =
            serde_json::from_str(include_str!("../../../scripts/node-runtime-release.json"))
                .unwrap();
        config.expected_node_sha256 = release["binary"]["sha256"].as_str().unwrap().into();
        fs::write(&config.entry_path, r#"import http from 'node:http';
import readline from 'node:readline';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
const lines = readline.createInterface({input:process.stdin});
let server, token, instance;
for await (const line of lines) {
 const msg=JSON.parse(line);
 if(msg.type==='initialize') {
  ({token,instanceId:instance}=msg);
  assert.equal(process.env.APP_INSTANCE_ID,instance);
  assert.match(process.env.PANDA_DRAWER_RUN_IDENTITY,/^[0-9a-f]{64}$/);
  assert.notEqual(process.env.PANDA_DRAWER_RUN_IDENTITY,token);
  for(const name of ['PATH','HOME','NODE_OPTIONS','NODE_PATH']) assert.equal(process.env[name],undefined);
  assert.equal(process.env.TRIAL_BEHAVIOR,'');
  assert.equal(process.env.APP_DATA_DIR,path.join(process.cwd(),'data'));
  fs.writeFileSync(path.join(process.env.APP_DATA_DIR,'real-node-project-data.txt'),'retained official Node data');
  server=http.createServer((req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401,{'content-length':0});return res.end();}
   const body=JSON.stringify({protocolVersion:1,instanceId:instance,status:'ok'});
   // Match frameworks that let Node select chunked transfer encoding.
   res.writeHead(200,{'content-type':'application/json'});res.end(body);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'ready',instanceId:instance,host:'127.0.0.1',port:server.address().port})+'\n');
 } else if(msg.type==='ping') {
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'pong',instanceId:instance,requestId:msg.requestId})+'\n');
 } else if(msg.type==='shutdown') {
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'stopping',instanceId:instance})+'\n');
  await new Promise(resolve=>server.close(resolve));lines.close();break;
 }
}
"#).unwrap();
        let expected_node = fs::canonicalize(&config.node_path).unwrap();
        let source = config.working_directory.clone();
        let cache = source.with_file_name("official-node-private-cache");
        fs::create_dir(&cache).unwrap();
        let (launched, pending, _) = NodeRuntimeTrial::launch_project_retaining_failed(
            config,
            source.clone(),
            cache,
            &OwnedProcessRegistry::default(),
        )
        .await;
        assert!(pending.is_none());
        let mut runtime = launched.unwrap();
        assert!(!runtime.status.run_directory.join("data").exists());
        runtime.ping().await.unwrap();
        runtime.verify_health().await.unwrap();
        let identity = runtime.process_identity().unwrap();
        assert_eq!(identity.pid, runtime.status.pid);
        assert_eq!(identity.pgid, runtime.status.pid);
        assert_eq!(identity.executable, expected_node);
        assert_eq!(identity.nonce, runtime.identity_nonce);
        assert_eq!(
            node_process_identity::observe(&identity).unwrap(),
            node_process_identity::Observation::Matching
        );
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(stopped.stopping_observed);
        assert!(!group_exists(stopped.pid));
        assert_eq!(
            fs::read(source.join("data/real-node-project-data.txt")).unwrap(),
            b"retained official Node data"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Requires an explicit verified private Node path in NODE_RUNTIME_TRIAL_TEST_NODE"]
    async fn pinned_node_handshake_cleans_child_and_grandchild_without_killing_standalone_node() {
        let node_path = PathBuf::from(
            std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE").expect("explicit private Node"),
        );
        let (_fixture, mut config) = protocol_fixture("");
        let release: Value =
            serde_json::from_str(include_str!("../../../scripts/node-runtime-release.json"))
                .unwrap();
        config.expected_node_sha256 = release["binary"]["sha256"].as_str().unwrap().into();
        verify_node_digest(&node_path, &config.expected_node_sha256).unwrap();
        config.node_path = node_path.clone();
        fs::write(&config.entry_path,r#"import http from 'node:http';
import readline from 'node:readline';
import {spawn} from 'node:child_process';
import {writeFileSync,existsSync} from 'node:fs';
import {join} from 'node:path';
import {setTimeout} from 'node:timers/promises';
const lines=readline.createInterface({input:process.stdin});
let server,token,instance;
for await (const line of lines) {
 const msg=JSON.parse(line);
 if(msg.type==='initialize') {
  ({token,instanceId:instance}=msg);
  const grand=`import {writeFileSync} from 'node:fs';import {join} from 'node:path';process.on('SIGTERM',()=>{});writeFileSync(join(process.env.APP_DATA_DIR,'grandchild-pid'),String(process.pid));setInterval(()=>{},1000);`;
  const child=`import {spawn} from 'node:child_process';import {writeFileSync} from 'node:fs';import {join} from 'node:path';process.on('SIGTERM',()=>{});writeFileSync(join(process.env.APP_DATA_DIR,'child-pid'),String(process.pid));const grand=spawn(process.execPath,['--input-type=module','-e',${JSON.stringify(grand)}],{stdio:'ignore'});grand.unref();setInterval(()=>{},1000);`;
  const descendant=spawn(process.execPath,['--input-type=module','-e',child],{stdio:'ignore'});descendant.unref();
  const deadline=Date.now()+2000;
  while(!existsSync(join(process.env.APP_DATA_DIR,'grandchild-pid'))) {if(Date.now()>deadline)throw new Error('artificial descendants not ready');await setTimeout(10);}
  server=http.createServer((req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401,{'content-length':0});return res.end();}
   const body=JSON.stringify({protocolVersion:1,instanceId:instance,status:'ok'});
   res.writeHead(200,{'content-length':Buffer.byteLength(body)});res.end(body);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'ready',instanceId:instance,host:'127.0.0.1',port:server.address().port})+'\n');
 } else if(msg.type==='shutdown') {
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'stopping',instanceId:instance})+'\n');
  await new Promise(resolve=>server.close(resolve));lines.close();break;
 }
}
"#).unwrap();
        let mut standalone = Command::new(node_path)
            .args(["-e", "setInterval(()=>{},1000)"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        let data = runtime.status.run_directory.join("data");
        let descendants: [i32; 2] = ["child-pid", "grandchild-pid"].map(|name| {
            fs::read_to_string(data.join(name))
                .unwrap()
                .parse()
                .unwrap()
        });
        let stopped = runtime.stop().await.unwrap();
        assert!(stopped.stopping_observed);
        assert_eq!(stopped.exit_code, Some(0));
        assert!(stopped.forced && stopped.process_group_exited);
        for pid in descendants {
            assert!(!process_is_live(pid));
        }
        assert!(standalone.try_wait().unwrap().is_none());
        standalone.kill().await.unwrap();
        standalone.wait().await.unwrap();
    }

    #[test]
    fn reserved_and_malformed_environment_is_rejected_without_echoing_values() {
        for key in [
            "NODE_OPTIONS",
            "NODE_PATH",
            "PATH",
            "APP_PORT",
            RUN_IDENTITY_ENV,
            "DYLD_INSERT_LIBRARIES",
            "bad-name",
        ] {
            let values = BTreeMap::from([(key.into(), "artificial-secret".into())]);
            let error = validate_business_environment(&values).unwrap_err();
            assert_eq!(error.code, "TRIAL_ENV_INVALID");
            assert!(!error.message.contains("artificial-secret"));
        }
        validate_business_environment(&BTreeMap::from([("AI_MODEL".into(), "test-model".into())]))
            .unwrap();
    }

    #[tokio::test]
    async fn oversized_and_partial_messages_fail_with_fixed_diagnostics() {
        let bytes = vec![b'x'; MESSAGE_LIMIT + 1];
        assert_eq!(
            limited_line(&mut &bytes[..]).await.unwrap_err().code,
            "TRIAL_PROTOCOL_LIMIT"
        );
        assert_eq!(
            limited_line(&mut &b"partial"[..]).await.unwrap_err().code,
            "TRIAL_PROTOCOL_INVALID"
        );
        assert!(limited_line(&mut &b""[..]).await.unwrap().is_none());
    }

    #[test]
    fn startup_explanations_keep_causes_without_echoing_untrusted_messages() {
        for code in [
            "TRIAL_HTTP_INVALID",
            "TRIAL_HTTP_TIMEOUT",
            "TRIAL_READY_INVALID",
            "unknown",
        ] {
            let error = fail(code, "artificial-sensitive-application-message");
            let explanation = startup_failure_explanation(&error);
            assert!(!explanation.contains("artificial-sensitive-application-message"));
            assert!(!explanation.is_empty());
        }
        let error = fail("TRIAL_HTTP_STATUS", "untrusted response").detail("httpStatus", 401);
        assert!(startup_failure_explanation(&error).contains("HTTP 401"));
    }

    #[test]
    fn unknown_control_fields_and_types_are_not_accepted() {
        for value in [
            json!({"type":"ready","protocolVersion":1,"instanceId":"x","host":"127.0.0.1","port":1234,"unexpected":true}),
            json!({"type":"log","message":"do not print this"}),
        ] {
            assert!(serde_json::from_value::<ControlMessage>(value).is_err());
        }
        let stopping: ControlMessage =
            serde_json::from_value(json!({"type":"stopping","protocolVersion":1,"instanceId":"x"}))
                .unwrap();
        assert!(
            matches!(stopping, ControlMessage::Stopping { protocol_version: 1, instance_id } if instance_id == "x")
        );
    }
}
