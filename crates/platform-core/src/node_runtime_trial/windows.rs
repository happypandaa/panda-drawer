//! Interactive Node ownership uses the original creation HANDLE and Job. No
//! numeric PID acquires stopping authority, and pipe workers retain real EOF.
use super::*;
use crate::owned_process::{
    windows::pipe_bridge::{PipeThread, PreparedPumps},
    OwnedCommand,
};
use platform_native::windows::{files as native_files, process::OwnedProcess as NativeProcess};
use std::{io, os::windows::process::ExitStatusExt, process::ExitStatus, sync::Mutex};
use tokio::{io::DuplexStream, runtime::Handle, sync::watch};

pub(super) type ChildStdin = DuplexStream;

struct Resources {
    process: Option<NativeProcess>,
    workers: Vec<(String, PipeThread)>,
    setup_error: Option<ApiError>,
    io_error: Option<ApiError>,
    armed: bool,
}
struct Owner(Mutex<Resources>);

pub(super) struct Child {
    owner: Arc<Owner>,
    pid: u32,
    pub(super) stdin: Option<DuplexStream>,
    pub(super) stdout: Option<DuplexStream>,
    pub(super) stderr: Option<DuplexStream>,
}

pub(super) struct ProcessOwnership {
    owner: Arc<Owner>,
}

fn observation_error(code: &str, cause: impl std::fmt::Display) -> ApiError {
    fail(
        code,
        "Could not confirm the retained Node process resources",
    )
    .detail("cause", cause.to_string())
}

impl Child {
    pub(super) fn id(&self) -> Option<u32> {
        Some(self.pid)
    }

    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        let state = self.owner.0.lock().unwrap_or_else(|p| p.into_inner());
        let process = state
            .process
            .as_ref()
            .ok_or_else(|| io::Error::other("Node owner is missing"))?;
        let code = process
            .wait(Duration::ZERO)?
            .ok_or_else(|| io::Error::other("Node root is still running"))?;
        if process.active_processes()? != 0 {
            return Err(io::Error::other("Node Job still has active processes"));
        }
        // Retain Job/root and worker handles until finish_io has joined every
        // actual native pump. Waiting for a root is not an I/O closure receipt.
        Ok(ExitStatus::from_raw(code))
    }
}

impl ProcessOwnership {
    fn with_process<T>(
        &self,
        action: impl FnOnce(&NativeProcess) -> io::Result<T>,
    ) -> TrialResult<T> {
        let state = self.owner.0.lock().unwrap_or_else(|p| p.into_inner());
        if !state.armed {
            return Err(fail(
                "TRIAL_OWNERSHIP_RELEASED",
                "Trial process ownership has been released",
            ));
        }
        let process = state.process.as_ref().ok_or_else(|| {
            fail(
                "TRIAL_PROCESS_OWNERSHIP_LOST",
                "Node process owner is missing",
            )
        })?;
        action(process).map_err(|error| {
            observation_error("TRIAL_PROCESS_OWNERSHIP_LOST", &error)
                .detail("errno", json!(error.raw_os_error()))
        })
    }

    pub(super) fn observe(&self) -> TrialResult<Option<ExitObservation>> {
        self.with_process(NativeProcess::exit_code).map(|code| {
            code.map(|code| ExitObservation {
                code: ExitStatus::from_raw(code).code(),
                signal: None,
            })
        })
    }

    pub(super) fn signal(&self, _signal: i32) -> TrialResult<()> {
        // Shared trial call sites identify their escalation stage. Windows
        // never sends a Unix signal: after the protocol grace it terminates
        // this exact Job (or the original never-resumed root creation HANDLE).
        self.with_process(|process| process.terminate(137))
    }

    pub(super) async fn signal_with_reobserve(&self, signal: i32) -> TrialResult<()> {
        crate::owned_process::retry_owned_signal("TRIAL_SIGNAL_FAILED", || self.signal(signal))
            .await
    }

    pub(super) fn has_descendants(&self) -> TrialResult<bool> {
        self.with_process(|process| process.active_processes().map(|count| count != 0))
    }

    pub(super) fn disarm_before_reap(&self) {
        self.owner.0.lock().unwrap_or_else(|p| p.into_inner()).armed = false;
    }
}

pub(super) fn spawn(command: Command) -> TrialResult<Child> {
    let no_child = |error: ApiError| error.detail("trialChildCreated", false);
    let runtime = Handle::try_current()
        .map_err(|error| no_child(observation_error("TRIAL_PROCESS_FAILED", error)))?;
    let request =
        crate::owned_process::windows::request(OwnedCommand::cleared(command)).map_err(no_child)?;
    let (_stop, stop) = watch::channel(false);
    let mut pumps = PreparedPumps::new(&runtime, stop).map_err(no_child)?;
    let mut process = NativeProcess::create_suspended(request)
        .map_err(|error| no_child(observation_error("TRIAL_PROCESS_FAILED", error)))?;
    // From successful creation onward, every failure travels with its actual
    // owner. Never return Err here and drop the last retryable suspended root.
    let mut setup_error = process
        .assign_job()
        .err()
        .map(|error| observation_error("TRIAL_JOB_ASSIGN_FAILED", error));
    let workers = pumps.take_workers();
    if let Err(error) = pumps.transfer(&mut process) {
        setup_error.get_or_insert(error);
    }
    let pipes = pumps.take_pipes();
    let pid = process.pid;
    let owner = Arc::new(Owner(Mutex::new(Resources {
        process: Some(process),
        workers,
        setup_error,
        io_error: None,
        armed: true,
    })));
    Ok(Child {
        owner,
        pid,
        stdin: Some(pipes.stdin),
        stdout: Some(pipes.stdout),
        stderr: Some(pipes.stderr),
    })
}

pub(super) fn ownership(child: &Child, _pid: u32) -> ProcessOwnership {
    ProcessOwnership {
        owner: child.owner.clone(),
    }
}

pub(super) fn activate(child: &mut Child) -> TrialResult<()> {
    let mut state = child.owner.0.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(error) = &state.setup_error {
        return Err(error.clone());
    }
    match state
        .process
        .as_mut()
        .expect("created Node owner retained")
        .resume()
    {
        Ok(()) => Ok(()),
        Err(error) => {
            let error = observation_error("TRIAL_PROCESS_RESUME_FAILED", error);
            state.setup_error = Some(error.clone());
            Err(error)
        }
    }
}

pub(super) fn spawn_failed_before_creation(error: &ApiError) -> bool {
    error.details["trialChildCreated"] == false
}

pub(super) async fn finish_io(
    owner: &ProcessOwnership,
    deadline: tokio::time::Instant,
) -> TrialResult<()> {
    loop {
        {
            let mut state = owner.owner.0.lock().unwrap_or_else(|p| p.into_inner());
            let mut index = 0;
            while index < state.workers.len() {
                if !state.workers[index].1.is_finished() {
                    index += 1;
                    continue;
                }
                let (name, mut worker) = state.workers.remove(index);
                let cause = match worker.join() {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(()) => Some(fail(
                        "TRIAL_NATIVE_WORKER_PANICKED",
                        "Native Node pipe worker did not join",
                    )),
                };
                if let Some(cause) = cause {
                    state.io_error.get_or_insert_with(|| {
                        fail(
                            "TRIAL_NATIVE_IO_JOIN_FAILED",
                            "Native Node pipe work did not join normally",
                        )
                        .detail("worker", name)
                        .detail("cause", json!(cause))
                        .detail("ioClosed", false)
                    });
                }
            }
            if state.workers.is_empty() {
                if let Some(error) = &state.io_error {
                    return Err(error.clone());
                }
                if let Some(process) = &state.process {
                    if process.stdin.is_some()
                        || process.stdout.is_some()
                        || process.stderr.is_some()
                    {
                        return Err(fail(
                            "TRIAL_NATIVE_PIPE_HANDOFF_UNCONFIRMED",
                            "Created Node pipe handles remain in their original owner",
                        )
                        .detail("ioClosed", false));
                    }
                    if process
                        .wait(Duration::ZERO)
                        .map_err(|e| observation_error("TRIAL_STOP_FAILED", e))?
                        .is_none()
                        || process
                            .active_processes()
                            .map_err(|e| observation_error("TRIAL_GROUP_UNCONFIRMED", e))?
                            != 0
                    {
                        return Err(fail(
                            "TRIAL_NATIVE_IO_CLEANUP_PENDING",
                            "Node root and Job closure have not been confirmed",
                        )
                        .detail("ioClosed", false));
                    }
                }
                state.process.take();
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(fail(
                "TRIAL_NATIVE_IO_CLEANUP_PENDING",
                "Native Node pipe workers are still owned and have not joined",
            )
            .detail("ioClosed", false));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(super) fn require_supported() -> TrialResult<()> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Ok(())
    } else {
        Err(fail(
            "TRIAL_PLATFORM_UNSUPPORTED",
            "Node runtime requires Windows x64",
        ))
    }
}

pub(super) fn checked_path(path: &Path, directory: bool) -> TrialResult<PathBuf> {
    if !path.is_absolute() {
        return Err(fail(
            "TRIAL_PATH_INVALID",
            "Trial paths must be normalized and absolute",
        ));
    }
    let opened = native_files::open_no_reparse(path).map_err(|_| {
        fail(
            "TRIAL_PATH_INVALID",
            "A trial path does not exist or passes through a link",
        )
    })?;
    if opened.identity.directory != directory || (!directory && !opened.file.metadata()?.is_file())
    {
        return Err(fail(
            "TRIAL_PATH_INVALID",
            "A trial path has the wrong file type",
        ));
    }
    // Compare directory relationships in the actual Windows namespace, so a
    // differently cased spelling cannot bypass source/storage separation.
    let canonical = fs::canonicalize(path)
        .map_err(|_| fail("TRIAL_PATH_INVALID", "A trial path cannot be resolved"))?;
    let resolved = native_files::open_no_reparse(&canonical).map_err(|_| {
        fail(
            "TRIAL_PATH_INVALID",
            "A resolved trial path cannot be inspected",
        )
    })?;
    if !opened.identity.same_object(&resolved.identity) {
        return Err(fail(
            "TRIAL_PATH_INVALID",
            "A trial path changed while resolving it",
        ));
    }
    Ok(canonical)
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
    for path in [&data, &cache] {
        let opened = native_files::open_no_reparse(path).map_err(|_| {
            fail(
                "TRIAL_STORAGE_INVALID",
                "Persistent storage cannot be inspected",
            )
        })?;
        native_files::verify_current_user_owner(&opened.file).map_err(|_| {
            fail(
                "TRIAL_STORAGE_INVALID",
                "Persistent storage must belong to the current user",
            )
        })?;
    }
    Ok((data, cache))
}

/// Project data is the one deliberate source/storage overlap. Private cache
/// and runtime directories still remain separate from the project root.
pub(super) fn checked_project_storage(
    source: &Path,
    cache: &Path,
    backend: &Path,
    trial_root: &Path,
) -> TrialResult<(PathBuf, PathBuf)> {
    let source = checked_path(source, true)?;
    let cache = checked_path(cache, true)?;
    let overlaps = |left: &Path, right: &Path| left.starts_with(right) || right.starts_with(left);
    if !backend.starts_with(&source)
        || overlaps(&source, &cache)
        || overlaps(&source, trial_root)
        || overlaps(&cache, trial_root)
    {
        return Err(fail(
            "TRIAL_STORAGE_INVALID",
            "Project data must use its registered folder; cache and runtime must be separate",
        ));
    }
    let data = source.join("data");
    match fs::symlink_metadata(&data) {
        Ok(_) => {
            checked_path(&data, true)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let opened = native_files::open_no_reparse(&cache)?;
    native_files::verify_current_user_owner(&opened.file)?;
    Ok((data, cache))
}

pub(super) fn prepare_project_data(source: &Path) -> TrialResult<()> {
    // Retain the selected directory and all ancestors throughout creation.
    let _source = native_files::pin_directory(source)?;
    let data = source.join("data");
    match fs::create_dir(&data) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    let _data = native_files::pin_directory(&data)?;
    // Unlike private cache/control, do not rewrite the user's existing DACL.
    Ok(())
}

pub(super) fn prepare_directories(
    run_directory: &Path,
    persistent: Option<(PathBuf, PathBuf)>,
    private_data: bool,
) -> TrialResult<(PathBuf, PathBuf)> {
    native_files::create_private_directory(run_directory)?;
    let (data, cache) = if let Some(paths) = persistent {
        paths
    } else {
        let data = run_directory.join("data");
        let cache = run_directory.join("cache");
        native_files::create_private_directory(&data)?;
        native_files::create_private_directory(&cache)?;
        (data, cache)
    };
    for directory in std::iter::once(&cache).chain(private_data.then_some(&data)) {
        native_files::make_directory_private(directory)?;
    }
    native_files::create_private_directory(&run_directory.join("tmp"))?;
    Ok((data, cache))
}

pub(super) fn argument(path: &Path) -> io::Result<PathBuf> {
    crate::node_tool_layout::command_argument(path)
}
pub(super) fn open_node(path: &Path) -> io::Result<fs::File> {
    native_files::open_read_file_allowing_hard_links(path)
}
pub(super) fn configure_environment(command: &mut Command, temporary: &Path) -> TrialResult<()> {
    // Node's OpenSSL initialization needs the Windows installation directory
    // even when the application environment is intentionally cleared. Obtain
    // it from Win32 rather than inheriting a caller-controlled environment.
    let system = platform_native::windows::system_paths::SystemPaths::current()
        .map_err(|error| observation_error("TRIAL_SYSTEM_PATH_UNAVAILABLE", error))?;
    let path = argument(temporary)?;
    command
        .env("SystemRoot", &system.windows)
        .env("TEMP", &path)
        .env("TMP", path);
    Ok(())
}

pub(super) fn drop_runtime(runtime: &mut NodeRuntimeTrial) {
    runtime.stopping.store(true, Ordering::Release);
    runtime.stdin.take();
    let readers = std::mem::take(&mut runtime.readers);
    for reader in &readers {
        reader.abort();
    }
    let mut child = runtime.child.take();
    let ownership = runtime.ownership.clone();
    let _ = ownership.signal(SIGNAL_KILL);
    ownership.disarm_before_reap();
    if let Ok(executor) = Handle::try_current() {
        let log = runtime.log.clone();
        let exit_logged = runtime.exit_logged;
        executor.spawn(async move {
            // Drop remains best effort and never retires a persistent marker.
            // Keep the actual owners until root/Job and both classes of reader
            // have joined, rather than treating a dropped pipe as real EOF.
            for reader in readers {
                let _ = reader.await;
            }
            if let Some(child) = &mut child {
                loop {
                    if let Ok(exit) = child.wait().await {
                        if !exit_logged {
                            log.event("exited", json!({"exitCode":exit.code(),"signal":null}));
                        }
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            if finish_io(
                &ownership,
                tokio::time::Instant::now() + Duration::from_secs(2),
            )
            .await
            .is_err()
            {
                // An unconfirmed closure retains its real retryable owner; it
                // is never transformed into a successful Drop cleanup receipt.
                let _retained = (ownership, child);
                std::future::pending::<()>().await;
            }
        });
    }
    runtime.token.clear();
    runtime.identity_nonce.clear();
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
