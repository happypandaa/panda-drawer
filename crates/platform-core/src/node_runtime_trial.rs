//! Disposable macOS-only experiment. This is not a project runtime or a sandbox.
//!
//! It never discovers a workspace, starts the platform, installs dependencies, or
//! falls back to a Node from PATH. Callers must explicitly stop the instance.
//! Drop is best-effort cleanup; host hard-crash/orphan recovery is not implemented.

use crate::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, ChildStdin, Command},
    sync::mpsc,
    task::JoinHandle,
};
use uuid::Uuid;

pub const EXPECTED_NODE_VERSION: &str = "24.22.0";
const MESSAGE_LIMIT: usize = 8192;
const HTTP_LIMIT: usize = 16384;
const STDERR_LIMIT: u64 = 65536;
// Used only by the macOS ownership path. Keep unsupported-target compilation
// independent of libc's platform-specific signal constant exports.
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
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialStatus {
    pub instance_id: String,
    pub pid: u32,
    pub state: String,
    pub port: Option<u16>,
    pub exit_code: Option<i32>,
    pub stderr_bytes_discarded: u64,
    pub forced: bool,
    pub stopping_observed: bool,
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

pub struct NodeRuntimeTrial {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<TrialResult<ControlMessage>>,
    readers: Vec<JoinHandle<()>>,
    token: String,
    status: TrialStatus,
    stderr_count: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    control_failed: Arc<AtomicBool>,
    ownership: Arc<ProcessOwnership>,
}

#[derive(Clone, Copy)]
struct ExitObservation {
    code: Option<i32>,
}

struct ProcessOwnership {
    state: Mutex<OwnershipState>,
    #[cfg(test)]
    signals_sent: AtomicU64,
}

struct OwnershipState {
    pid: u32,
    armed: bool,
}

impl ProcessOwnership {
    fn new(pid: u32) -> Self {
        Self {
            state: Mutex::new(OwnershipState { pid, armed: true }),
            #[cfg(test)]
            signals_sent: AtomicU64::new(0),
        }
    }

    fn observe(&self) -> TrialResult<Option<ExitObservation>> {
        let mut state = self.state.lock().unwrap();
        Self::check_owned(&mut state)
    }

    fn check_owned(state: &mut OwnershipState) -> TrialResult<Option<ExitObservation>> {
        if !state.armed {
            return Err(fail(
                "TRIAL_OWNERSHIP_RELEASED",
                "Trial process ownership has been released",
            ));
        }
        match observe_without_reaping(state.pid) {
            Ok(status) => Ok(status),
            Err(error) => {
                // Never signal a numeric PID/PGID once child ownership is uncertain.
                state.armed = false;
                Err(error)
            }
        }
    }

    fn signal(&self, signal: i32) -> TrialResult<()> {
        let _ = signal;
        let mut state = self.state.lock().unwrap();
        Self::check_owned(&mut state)?;
        #[cfg(unix)]
        {
            // The unreaped leader pins this PID/PGID until disarm_before_reap.
            let result = unsafe { libc::kill(-(state.pid as i32), signal) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err(fail(
                    "TRIAL_SIGNAL_FAILED",
                    "Could not signal the owned trial group",
                ));
            }
            #[cfg(test)]
            self.signals_sent.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn has_descendants(&self) -> TrialResult<bool> {
        let mut state = self.state.lock().unwrap();
        Self::check_owned(&mut state)?;
        group_has_other_members(state.pid)
    }

    fn disarm_before_reap(&self) {
        self.state.lock().unwrap().armed = false;
    }
}

/// WNOWAIT keeps the exited leader reserved. Tokio's macOS SignalReaper does
/// not reap a retained Child until wait/try_wait is polled or the handle drops.
fn observe_without_reaping(pid: u32) -> TrialResult<Option<ExitObservation>> {
    #[cfg(target_os = "macos")]
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        };
        if result == 0 {
            return if unsafe { info.si_pid() } == 0 {
                Ok(None)
            } else {
                Ok(Some(ExitObservation {
                    code: if info.si_code == libc::CLD_EXITED {
                        Some(unsafe { info.si_status() })
                    } else {
                        None
                    },
                }))
            };
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(fail(
                "TRIAL_PROCESS_OWNERSHIP_LOST",
                "Could not confirm unreaped child ownership",
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(fail(
            "TRIAL_PLATFORM_UNSUPPORTED",
            "Non-reaping observation is only supported on macOS",
        ))
    }
}

fn group_has_other_members(pid: u32) -> TrialResult<bool> {
    #[cfg(target_os = "macos")]
    {
        // PROC_PGRP_ONLY from the macOS SDK sys/proc_info.h; return value is bytes.
        const PROC_PGRP_ONLY: u32 = 2;
        let mut members = [0i32; 4096];
        let bytes = std::mem::size_of_val(&members);
        let count = unsafe {
            libc::proc_listpids(
                PROC_PGRP_ONLY,
                pid,
                members.as_mut_ptr().cast(),
                bytes as i32,
            )
        };
        if count < 0
            || count as usize >= bytes
            || !(count as usize).is_multiple_of(std::mem::size_of::<i32>())
        {
            return Err(fail(
                "TRIAL_GROUP_UNCONFIRMED",
                "Could not completely inspect the owned trial group",
            ));
        }
        Ok(members[..count as usize / std::mem::size_of::<i32>()]
            .iter()
            .any(|member| *member > 0 && *member != pid as i32))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(fail(
            "TRIAL_PLATFORM_UNSUPPORTED",
            "Group inspection is only supported on macOS",
        ))
    }
}

fn fail(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}

fn checked_path(path: &Path, directory: bool) -> TrialResult<PathBuf> {
    if !path.is_absolute() {
        return Err(fail("TRIAL_PATH_INVALID", "Trial paths must be absolute"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err(fail("TRIAL_PATH_INVALID", "Trial paths must be normalized"));
        }
        current.push(component.as_os_str());
        let meta = fs::symlink_metadata(&current)
            .map_err(|_| fail("TRIAL_PATH_INVALID", "A trial path does not exist"))?;
        if meta.file_type().is_symlink() {
            return Err(fail(
                "TRIAL_PATH_INVALID",
                "Linked trial paths are not accepted",
            ));
        }
    }
    let meta = fs::metadata(path)
        .map_err(|_| fail("TRIAL_PATH_INVALID", "A trial path cannot be inspected"))?;
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err(fail(
            "TRIAL_PATH_INVALID",
            "A trial path has the wrong file type",
        ));
    }
    Ok(path.to_path_buf())
}

fn checked_storage(
    data: PathBuf,
    cache: PathBuf,
    backend: &Path,
    trial_root: &Path,
) -> TrialResult<(PathBuf, PathBuf)> {
    let data = checked_path(&data, true)?;
    let cache = checked_path(&cache, true)?;
    let overlaps = |left: &Path, right: &Path| left.starts_with(right) || right.starts_with(left);
    if overlaps(&data, &cache)
        || [&data, &cache]
            .iter()
            .any(|path| overlaps(path, backend) || overlaps(path, trial_root))
    {
        return Err(fail(
            "TRIAL_STORAGE_INVALID",
            "Persistent storage must be separate from source and run directories",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for path in [&data, &cache] {
            let metadata = fs::metadata(path).map_err(|_| {
                fail(
                    "TRIAL_STORAGE_INVALID",
                    "Persistent storage cannot be inspected",
                )
            })?;
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(fail(
                    "TRIAL_STORAGE_INVALID",
                    "Persistent storage must belong to the current user",
                ));
            }
        }
    }
    Ok((data, cache))
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
        let valid_name = crate::node_app_manifest::valid_business_variable(name);
        total = total.saturating_add(name.len()).saturating_add(value.len());
        if !valid_name
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
    let mut file = fs::File::open(path)
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

async fn verify_node_version(path: &Path) -> TrialResult<()> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .map_err(|_| fail("TRIAL_NODE_VERSION", "Private Node version probe timed out"))?
        .map_err(|_| fail("TRIAL_NODE_VERSION", "Private Node could not be executed"))?;
    if !output.status.success()
        || output.stdout.len() > 64
        || String::from_utf8_lossy(&output.stdout).trim() != format!("v{EXPECTED_NODE_VERSION}")
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

struct PreparedLaunch {
    node: PathBuf,
    cwd: PathBuf,
    entry: PathBuf,
    trial_root: PathBuf,
    persistent: Option<(PathBuf, PathBuf)>,
}

async fn prepare_launch(
    config: &RuntimeLaunchConfig,
    persistent: Option<(&Path, &Path)>,
) -> TrialResult<PreparedLaunch> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err(fail(
            "TRIAL_PLATFORM_UNSUPPORTED",
            "This disposable experiment only supports macOS arm64",
        ));
    }
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
            checked_storage(data.to_path_buf(), cache.to_path_buf(), &cwd, &trial_root)
        })
        .transpose()?;
    verify_node_digest(&node, &config.expected_node_sha256)?;
    verify_node_version(&node).await?;
    Ok(PreparedLaunch {
        node,
        cwd,
        entry,
        trial_root,
        persistent,
    })
}

impl NodeRuntimeTrial {
    pub async fn launch(config: RuntimeLaunchConfig) -> TrialResult<Self> {
        Self::launch_inner(config, None).await
    }

    /// Use caller-owned persistent data/cache while preserving a fresh run/tmp.
    /// The registration layer must also reject overlap with the full application
    /// source root, which is not represented by this backend-only launch config.
    pub async fn launch_with_storage(
        config: RuntimeLaunchConfig,
        data: PathBuf,
        cache: PathBuf,
    ) -> TrialResult<Self> {
        Self::launch_inner(config, Some((data, cache))).await
    }

    /// Read-only application preflight. Only the pinned private Node's version
    /// probe executes; no application code or application directories are written.
    /// Launch repeats these checks after the caller records its start intent.
    pub async fn preflight(
        config: &RuntimeLaunchConfig,
        data: &Path,
        cache: &Path,
    ) -> TrialResult<()> {
        prepare_launch(config, Some((data, cache)))
            .await
            .map(|_| ())
    }

    async fn launch_inner(
        config: RuntimeLaunchConfig,
        persistent: Option<(PathBuf, PathBuf)>,
    ) -> TrialResult<Self> {
        let PreparedLaunch {
            node,
            cwd,
            entry,
            trial_root,
            persistent,
        } = prepare_launch(
            &config,
            persistent
                .as_ref()
                .map(|(data, cache)| (data.as_path(), cache.as_path())),
        )
        .await?;
        let instance_id = Uuid::new_v4().to_string();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let run_directory = trial_root.join(format!("run-{instance_id}"));
        fs::create_dir(&run_directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&run_directory, fs::Permissions::from_mode(0o700))?;
        }
        let (data, cache) = if let Some(paths) = persistent {
            paths
        } else {
            fs::create_dir(run_directory.join("data"))?;
            fs::create_dir(run_directory.join("cache"))?;
            (run_directory.join("data"), run_directory.join("cache"))
        };
        for directory in [&data, &cache] {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
            }
        }
        fs::create_dir(run_directory.join("tmp"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(run_directory.join("tmp"), fs::Permissions::from_mode(0o700))?;
        }
        let mut command = Command::new(node);
        command
            .arg(entry)
            .current_dir(cwd)
            .env_clear()
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .env("APP_INSTANCE_ID", &instance_id)
            .env("APP_HOST", "127.0.0.1")
            .env("APP_PORT", "0")
            .env("APP_DATA_DIR", data)
            .env("APP_CACHE_DIR", cache)
            .env("APP_TEMP_DIR", run_directory.join("tmp"))
            .env("TMPDIR", run_directory.join("tmp"))
            .envs(config.business_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|_| {
            fail(
                "TRIAL_PROCESS_FAILED",
                "Private Node could not start the trial entry",
            )
        })?;
        let pid = child
            .id()
            .ok_or_else(|| fail("TRIAL_PROCESS_FAILED", "Trial process has no identity"))?;
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
        let ownership = Arc::new(ProcessOwnership::new(pid));
        let diagnostic_sender = sender.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let control_failed = Arc::new(AtomicBool::new(false));
        let reader_stopping = stopping.clone();
        let reader_failed = control_failed.clone();
        let reader_ownership = ownership.clone();
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
                    terminate_unexpected(&reader_ownership, &reader_stopping).await;
                }
                if sender.send(result).await.is_err() || terminal {
                    break;
                }
            }
        });
        let stderr_count = Arc::new(AtomicU64::new(0));
        let counter = stderr_count.clone();
        let diagnostic_stopping = stopping.clone();
        let diagnostic_failed = control_failed.clone();
        let diagnostic_ownership = ownership.clone();
        let diagnostic_reader = tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                // Never retain or print application diagnostics: they may contain keys.
                counter.store(
                    counter.load(Ordering::Relaxed).saturating_add(n as u64),
                    Ordering::Relaxed,
                );
                if counter.load(Ordering::Relaxed) > STDERR_LIMIT {
                    diagnostic_failed.store(true, Ordering::Release);
                    terminate_unexpected(&diagnostic_ownership, &diagnostic_stopping).await;
                    let _ = diagnostic_sender
                        .send(Err(fail(
                            "TRIAL_LOG_LIMIT",
                            "Trial diagnostic output exceeded its 64 KiB budget",
                        )))
                        .await;
                    break;
                }
            }
        });
        let mut runtime = Self {
            child: Some(child),
            stdin,
            messages,
            readers: vec![control_reader, diagnostic_reader],
            token,
            stderr_count,
            stopping,
            control_failed,
            ownership,
            status: TrialStatus {
                instance_id,
                pid,
                state: "starting".into(),
                port: None,
                exit_code: None,
                stderr_bytes_discarded: 0,
                forced: false,
                stopping_observed: false,
                run_directory,
                hard_crash_recovery_implemented: false,
            },
        };
        let started = tokio::time::timeout(START_TIMEOUT, async {
            runtime.send(json!({"protocolVersion":1,"type":"initialize","instanceId":runtime.status.instance_id,"token":runtime.token})).await?;
            match runtime.receive().await? {
                ControlMessage::Ready { protocol_version: 1, instance_id, host, port }
                    if instance_id == runtime.status.instance_id && host == "127.0.0.1" && port > 0 => runtime.status.port = Some(port),
                _ => return Err(fail("TRIAL_READY_INVALID", "Trial ready identity or address did not match")),
            }
            runtime.verify_health().await?;
            runtime.status.state = "running".into();
            Ok(())
        }).await.unwrap_or_else(|_| Err(fail("TRIAL_START_TIMEOUT", "Trial was not ready within 10 seconds")));
        if let Err(error) = started {
            let _ = runtime.stop().await;
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
                self.status.state = "failed".into();
            }
        }
        let mut status = self.status.clone();
        status.stderr_bytes_discarded = self.stderr_count.load(Ordering::Relaxed);
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
        let value = self.get_json("/_platform/health").await?;
        if value != json!({"protocolVersion":1,"instanceId":self.status.instance_id,"status":"ok"})
        {
            return Err(fail(
                "TRIAL_HEALTH_INVALID",
                "Trial HTTP health identity did not match",
            ));
        }
        Ok(())
    }

    /// Budgeted, authenticated loopback GET for this disposable instance only.
    /// It does not follow redirects or forward platform credentials.
    pub async fn get_json(&self, path: &str) -> TrialResult<Value> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.len() > 512
            || path.bytes().any(|b| b <= 32 || b >= 127)
        {
            return Err(fail("TRIAL_HTTP_INVALID", "Invalid trial request path"));
        }
        let port = self
            .status
            .port
            .ok_or_else(|| fail("TRIAL_NOT_READY", "Trial has not reported a port"))?;
        tokio::time::timeout(REQUEST_TIMEOUT, async {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await
                .map_err(|_| fail("TRIAL_HTTP_FAILED", "Trial HTTP listener is unavailable"))?;
            let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n", self.token);
            stream.write_all(request.as_bytes()).await.map_err(|_| fail("TRIAL_HTTP_FAILED", "Trial HTTP request failed"))?;
            let mut response = Vec::new();
            stream.take((HTTP_LIMIT + 1) as u64).read_to_end(&mut response).await
                .map_err(|_| fail("TRIAL_HTTP_FAILED", "Trial HTTP response failed"))?;
            if response.len() > HTTP_LIMIT { return Err(fail("TRIAL_HTTP_LIMIT", "Trial HTTP response exceeded its budget")); }
            let split = response.windows(4).position(|w| w == b"\r\n\r\n")
                .ok_or_else(|| fail("TRIAL_HTTP_INVALID", "Invalid trial HTTP response"))?;
            let header = std::str::from_utf8(&response[..split])
                .map_err(|_| fail("TRIAL_HTTP_INVALID", "Invalid trial HTTP headers"))?;
            if !(header.starts_with("HTTP/1.1 200 ") || header.starts_with("HTTP/1.0 200 "))
                || header.to_ascii_lowercase().contains("transfer-encoding:") {
                return Err(fail("TRIAL_HTTP_INVALID", "Trial HTTP response was not supported success JSON"));
            }
            serde_json::from_slice(&response[split + 4..])
                .map_err(|_| fail("TRIAL_HTTP_INVALID", "Invalid trial HTTP JSON"))
        }).await.unwrap_or_else(|_| Err(fail("TRIAL_HTTP_TIMEOUT", "Trial HTTP request timed out")))
    }

    pub async fn stop(&mut self) -> TrialResult<TrialStatus> {
        if self.child.is_none() {
            return self.status();
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
            self.ownership.signal(SIGNAL_TERM)?;
            if !wait_for_owned_exit(&self.ownership, Duration::from_millis(250)).await? {
                self.ownership.signal(SIGNAL_KILL)?;
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
            self.status.forced = true;
            self.ownership.signal(SIGNAL_TERM)?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            if self.ownership.has_descendants()? {
                self.ownership.signal(SIGNAL_KILL)?;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while self.ownership.has_descendants()? && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            !self.ownership.has_descendants()?
        } else {
            true
        };
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
        for reader in self.readers.drain(..) {
            reader.abort();
            let _ = reader.await;
        }
        self.token.clear();
        if !descendants_exited {
            self.status.state = "failed".into();
            return Err(fail(
                "TRIAL_DESCENDANTS_REMAIN",
                "Could not confirm the trial process group exited",
            ));
        }
        self.status()
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
        let _ = ownership.signal(SIGNAL_TERM);
        tokio::time::sleep(Duration::from_millis(250)).await;
        if !stopping.load(Ordering::Acquire) {
            let _ = ownership.signal(SIGNAL_KILL);
        }
    }
}

impl Drop for NodeRuntimeTrial {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.stdin.take();
        for reader in self.readers.drain(..) {
            reader.abort();
        }
        if let Some(mut child) = self.child.take() {
            // Signal only while waitid proves ownership. Disarm before moving
            // the final wait into another task (or Tokio's orphan reaper).
            let _ = self.ownership.signal(SIGNAL_KILL);
            self.ownership.disarm_before_reap();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
        self.token.clear();
    }
}

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
assert 'NODE_OPTIONS' not in os.environ and 'HOME' not in os.environ and 'PATH' not in os.environ
assert os.path.isdir(os.environ['APP_DATA_DIR']) and os.path.isdir(os.environ['APP_CACHE_DIR'])
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
    def log_message(self, *args): pass
    def do_GET(self):
        if self.headers.get('Authorization') != 'Bearer ' + token:
            self.send_response(401); self.send_header('Content-Length','0'); self.end_headers(); return
        body = json.dumps({'protocolVersion':1, 'instanceId':instance, 'status':'ok'}).encode()
        self.send_response(200); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
print(json.dumps({'protocolVersion':1,'type':'ready','instanceId':instance,'host':'127.0.0.1','port':server.server_address[1]}), flush=True)
print('artificial-secret-for-log-test', file=sys.stderr, flush=True)
for line in sys.stdin:
    msg = json.loads(line)
    if msg['type'] == 'ping':
        if behavior == 'close-on-ping':
            os.close(sys.stdout.fileno())
            import time
            while True: time.sleep(1)
        print(json.dumps({'protocolVersion':1,'type':'pong','instanceId':instance,'requestId':msg['requestId']}), flush=True)
        if behavior == 'exit-after-pong': sys.exit(0)
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
            },
        )
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn real_mock_process_health_control_and_repeated_stop() {
        let (_fixture, config) = protocol_fixture("");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.ping().await.unwrap();
        runtime.verify_health().await.unwrap();
        let running = runtime.status().unwrap();
        let (port, token) = runtime.proxy_target().unwrap();
        assert_eq!(Some(port), running.port);
        assert_eq!(token.len(), 64);
        assert_eq!(running.state, "running");
        assert!(!serde_json::to_string(&running)
            .unwrap()
            .contains("artificial-secret"));
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(stopped.stopping_observed);
        assert!(!stopped.forced);
        assert!(!group_exists(stopped.pid));
        assert_eq!(runtime.stop().await.unwrap().state, "stopped");
        assert!(runtime.proxy_target().is_err());
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
            let error = NodeRuntimeTrial::launch(config).await.err().unwrap();
            assert_eq!(error.code, expected);
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn uncooperative_process_is_forced_and_reaped() {
        let (_fixture, config) = protocol_fixture("hang-stop");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        let stopped = runtime.stop().await.unwrap();
        assert!(stopped.forced);
        assert_eq!(stopped.state, "failed");
        assert!(!group_exists(stopped.pid));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn lost_control_channel_closes_the_existing_instance() {
        let (_fixture, config) = protocol_fixture("close-on-ping");
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        assert_eq!(
            runtime.ping().await.unwrap_err().code,
            "TRIAL_CONTROL_CLOSED"
        );
        assert!(runtime.proxy_target().is_err());
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "failed");
        assert!(!group_exists(stopped.pid));
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
    async fn exited_leader_stays_owned_across_repeated_status_and_delayed_drop() {
        let (_fixture, config) = protocol_fixture("exit-after-pong");
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
const lines = readline.createInterface({input:process.stdin});
let server, token, instance;
for await (const line of lines) {
 const msg=JSON.parse(line);
 if(msg.type==='initialize') {
  ({token,instanceId:instance}=msg);
  assert.equal(process.env.APP_INSTANCE_ID,instance);
  for(const name of ['PATH','HOME','NODE_OPTIONS','NODE_PATH']) assert.equal(process.env[name],undefined);
  assert.equal(process.env.TRIAL_BEHAVIOR,'');
  server=http.createServer((req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401,{'content-length':0});return res.end();}
   const body=JSON.stringify({protocolVersion:1,instanceId:instance,status:'ok'});
   res.writeHead(200,{'content-type':'application/json','content-length':Buffer.byteLength(body)});res.end(body);
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
        let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
        runtime.ping().await.unwrap();
        runtime.verify_health().await.unwrap();
        let stopped = runtime.stop().await.unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(stopped.stopping_observed);
        assert!(!group_exists(stopped.pid));
    }

    #[test]
    fn reserved_and_malformed_environment_is_rejected_without_echoing_values() {
        for key in [
            "NODE_OPTIONS",
            "NODE_PATH",
            "PATH",
            "APP_PORT",
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
