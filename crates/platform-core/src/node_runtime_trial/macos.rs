//! Original direct-Child ownership, path and directory policy.
use super::*;
use std::{path::Component, sync::Mutex};
pub(super) use tokio::process::{Child, ChildStdin};

pub(super) struct ProcessOwnership {
    state: Mutex<OwnershipState>,
    #[cfg(test)]
    pub(super) signals_sent: AtomicU64,
}

struct OwnershipState {
    pid: u32,
    armed: bool,
}

impl ProcessOwnership {
    pub(super) fn new(pid: u32) -> Self {
        Self {
            state: Mutex::new(OwnershipState { pid, armed: true }),
            #[cfg(test)]
            signals_sent: AtomicU64::new(0),
        }
    }

    pub(super) fn observe(&self) -> TrialResult<Option<ExitObservation>> {
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

    pub(super) fn signal(&self, signal: i32) -> TrialResult<()> {
        let _ = signal;
        let mut state = self.state.lock().unwrap();
        Self::check_owned(&mut state)?;
        #[cfg(unix)]
        {
            // The unreaped leader pins this PID/PGID until disarm_before_reap.
            let result = unsafe { libc::kill(-(state.pid as i32), signal) };
            let error = std::io::Error::last_os_error().raw_os_error();
            if result != 0
                && error != Some(libc::ESRCH)
                && !(Self::check_owned(&mut state)?.is_some()
                    && !group_has_other_members(state.pid)?)
            {
                return Err(fail(
                    "TRIAL_SIGNAL_FAILED",
                    "Could not signal the owned trial group",
                )
                .detail("errno", serde_json::json!(error)));
            }
            #[cfg(test)]
            self.signals_sent.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    pub(super) async fn signal_with_reobserve(&self, signal: i32) -> TrialResult<()> {
        // signal() releases its std MutexGuard before the async retry yields.
        // Drop retains the synchronous best-effort path; real stop owns this
        // bounded future and keeps the original unreaped Child throughout.
        crate::owned_process::retry_owned_signal("TRIAL_SIGNAL_FAILED", || self.signal(signal))
            .await
    }

    pub(super) fn has_descendants(&self) -> TrialResult<bool> {
        let mut state = self.state.lock().unwrap();
        Self::check_owned(&mut state)?;
        group_has_other_members(state.pid)
    }

    pub(super) fn disarm_before_reap(&self) {
        self.state.lock().unwrap().armed = false;
    }
}

/// WNOWAIT keeps the exited leader reserved. Tokio's macOS SignalReaper does
/// not reap a retained Child until wait/try_wait is polled or the handle drops.
pub(super) fn observe_without_reaping(pid: u32) -> TrialResult<Option<ExitObservation>> {
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
                    signal: if matches!(info.si_code, libc::CLD_KILLED | libc::CLD_DUMPED) {
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

pub(super) fn group_has_other_members(pid: u32) -> TrialResult<bool> {
    #[cfg(target_os = "macos")]
    {
        // ProcessOwnership (or the disposable test's retained Child) pins the
        // unreaped leader throughout both complete member scans. Keep the
        // established trial error vocabulary at this boundary.
        crate::owned_process::group_has_other_members(pid).map_err(|_| {
            fail(
                "TRIAL_GROUP_UNCONFIRMED",
                "Could not completely confirm the owned trial group",
            )
        })
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

pub(super) fn checked_path(path: &Path, directory: bool) -> TrialResult<PathBuf> {
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

pub(super) fn checked_storage(
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

pub(super) fn prepare_directories(
    run_directory: &Path,
    persistent: Option<(PathBuf, PathBuf)>,
) -> TrialResult<(PathBuf, PathBuf)> {
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
    Ok((data, cache))
}

pub(super) fn require_supported() -> TrialResult<()> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err(fail(
            "TRIAL_PLATFORM_UNSUPPORTED",
            "This disposable experiment only supports macOS arm64",
        ));
    }
    Ok(())
}
pub(super) fn argument(path: &Path) -> std::io::Result<PathBuf> {
    Ok(path.to_path_buf())
}
pub(super) fn open_node(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}
pub(super) fn configure_environment(_command: &mut Command, _temporary: &Path) -> TrialResult<()> {
    Ok(())
}
pub(super) fn spawn(mut command: Command) -> TrialResult<Child> {
    #[cfg(unix)]
    command.process_group(0);
    command.spawn().map_err(|_| {
        fail(
            "TRIAL_PROCESS_FAILED",
            "Private Node could not start the trial entry",
        )
    })
}
pub(super) fn ownership(_child: &Child, pid: u32) -> ProcessOwnership {
    ProcessOwnership::new(pid)
}
pub(super) fn activate(_child: &mut Child) -> TrialResult<()> {
    Ok(())
}
pub(super) fn spawn_failed_before_creation(_error: &ApiError) -> bool {
    false
}
pub(super) async fn finish_io(
    _owner: &ProcessOwnership,
    _deadline: tokio::time::Instant,
) -> TrialResult<()> {
    Ok(())
}

pub(super) fn drop_runtime(runtime: &mut NodeRuntimeTrial) {
    runtime.stopping.store(true, Ordering::Release);
    runtime.stdin.take();
    for reader in runtime.readers.drain(..) {
        reader.abort();
    }
    if let Some(mut child) = runtime.child.take() {
        // Signal only while waitid proves ownership. Disarm before moving
        // the final wait into another task (or Tokio's orphan reaper).
        let terminate = match runtime.ownership.observe() {
            Ok(None) => true,
            Ok(Some(exit)) => {
                runtime.record_exit(exit.code, exit.signal);
                runtime.ownership.has_descendants().unwrap_or(false)
            }
            Err(_) => false,
        };
        if terminate {
            log_termination(
                &runtime.log,
                &runtime.termination_logged,
                RuntimeStopReason::OwnerDropped,
            );
            let _ = runtime.ownership.signal(SIGNAL_KILL);
        }
        runtime.ownership.disarm_before_reap();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let log = runtime.log.clone();
            let exit_logged = runtime.exit_logged;
            handle.spawn(async move {
                if let Ok(exit) = child.wait().await {
                    let signal = platform_native::process_status::signal(&exit);
                    if !exit_logged {
                        log.event("exited", json!({"exitCode":exit.code(),"signal":signal}));
                    }
                }
            });
        }
    }
    runtime.token.clear();
    runtime.identity_nonce.clear();
}
