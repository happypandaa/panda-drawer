use super::{
    transport::{Ready, Transport},
    transport_for, update_root, write_json, InstallOutcome, InstallationResult, Preferences,
    CANNOT_INSTALL, RESULT_FILE,
};
use crate::{desktop_shutdown::Status, desktop_shutdown_native, AppState};
use flate2::read::GzDecoder;
use fs2::FileExt;
use platform_core::{build_profile::APP_VERSION, RuntimePaths, APP_IDENTIFIER};
use platform_update_handoff::{
    observe_parent_exit_from_inherited_descriptors, Error as HandoffError, ParentEndpoint,
    SessionDirectory,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    },
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

const HELPER_ARGUMENT: &str = "--platform-update-helper";
// The handoff protocol owns descriptors 3 and 4.
const INSTALL_LOCK_FD: libc::c_int = 5;
const INSTALL_LOCK: &str = "install.lock";
const PLAN: &str = "installation-plan.json";
const ACK_BUDGET: Duration = Duration::from_secs(4);
const DRAIN_BUDGET: Duration = Duration::from_secs(18);
const MAX_UNPACKED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 60_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: String,
    filename: String,
    device: u64,
    inode: u64,
    preferences: Preferences,
}

pub(super) fn installed_bundle() -> io::Result<PathBuf> {
    let binary = std::env::current_exe()?.canonicalize()?;
    let (bundle, macos, contents) = (
        binary.ancestors().nth(3),
        binary.ancestors().nth(1),
        binary.ancestors().nth(2),
    );
    match (bundle, macos, contents) {
        (Some(bundle), Some(macos), Some(contents))
            if macos.file_name() == Some(OsStr::new("MacOS"))
                && contents.file_name() == Some(OsStr::new("Contents"))
                && bundle.extension() == Some(OsStr::new("app")) =>
        {
            Ok(bundle.to_path_buf())
        }
        _ => Err(io::ErrorKind::InvalidInput.into()),
    }
}

fn entry_file(directory: &File, create: bool) -> io::Result<File> {
    let name = CString::new(PLAN).expect("fixed filename");
    let flags = libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | libc::O_NONBLOCK
        | if create {
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY
        };
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o777 != 0o600
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

fn save_plan(directory: &File, plan: &Plan) -> io::Result<()> {
    let mut file = entry_file(directory, true)?;
    serde_json::to_writer(&mut file, plan).map_err(io::Error::other)?;
    file.flush()?;
    file.sync_all()?;
    directory.sync_all()
}

fn load_plan(directory: &File) -> io::Result<Plan> {
    let mut bytes = Vec::new();
    entry_file(directory, false)?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn check_bundle_identity(bundle: &Path, version: &str) -> io::Result<()> {
    let require = |key: &str, expected: &OsStr| -> io::Result<()> {
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(bundle.join("Contents/Info.plist"))
            .output()?;
        if !output.status.success()
            || output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout) != expected.as_bytes()
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    };
    require("CFBundleIdentifier", OsStr::new(APP_IDENTIFIER))?;
    require("CFBundleShortVersionString", OsStr::new(version))?;
    let binary = std::env::current_exe()?;
    require(
        "CFBundleExecutable",
        binary.file_name().ok_or(io::ErrorKind::InvalidInput)?,
    )
}

fn check_bundle(bundle: &Path, version: &str) -> io::Result<()> {
    check_bundle_identity(bundle, version)?;
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdin(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

fn lock_file(root: &Path, create: bool) -> io::Result<File> {
    let lock =
        platform_native::filesystem::open_private_file(&root.join(INSTALL_LOCK), create, false)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(lock)
}

pub(super) fn verify_named_lock(root: &Path, lock: &File) -> io::Result<File> {
    let named = lock_file(root, false)?;
    let expected = named.metadata()?;
    let actual = lock.metadata()?;
    if (expected.dev(), expected.ino()) != (actual.dev(), actual.ino()) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(named)
}

pub(super) fn start_lock(root: &Path) -> io::Result<File> {
    let lock = lock_file(root, true)?;
    FileExt::try_lock_shared(&lock)?;
    // A process started before a replacement must never register as the
    // singleton for a different version now installed at this path.
    check_bundle_identity(&installed_bundle()?, APP_VERSION)?;
    Ok(lock)
}

pub(crate) fn lock_before_startup() -> io::Result<Option<File>> {
    if !super::configured() {
        return Ok(None);
    }
    let bundle = installed_bundle()?;
    if bundle.ancestors().any(|path| {
        path.file_name()
            .is_some_and(|name| name.as_bytes().starts_with(b".panda-install-"))
    }) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // The installer can exist only for an initialized, discoverable runtime.
    // The normal startup path handles first launch and invalid workspace data.
    let Ok(paths) = RuntimePaths::discover() else {
        check_bundle_identity(&bundle, APP_VERSION)?;
        return Ok(None);
    };
    let root = update_root(paths.data_root()).map_err(io::Error::other)?;
    start_lock(&root).map(Some)
}

struct ExclusiveInstallLock {
    lock: File,
    restore_shared: bool,
}

impl ExclusiveInstallLock {
    fn promote(lock: File) -> io::Result<Self> {
        if let Err(error) = FileExt::try_lock_exclusive(&lock) {
            // A flock conversion may relinquish the original shared lock
            // before discovering that a second launcher currently holds it.
            FileExt::lock_shared(&lock)?;
            return Err(error);
        }
        Ok(Self {
            lock,
            restore_shared: true,
        })
    }

    fn pass_to_helper(&self, command: &mut Command) -> io::Result<()> {
        // Keep the same open file description. The source cannot overlap any
        // descriptor reserved by the handoff or by this adapter.
        let duplicate = unsafe { libc::fcntl(self.lock.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let held = unsafe { File::from_raw_fd(duplicate) };
        unsafe {
            command.pre_exec(move || {
                // dup2 clears close-on-exec for the explicit child slot.
                if libc::dup2(held.as_raw_fd(), INSTALL_LOCK_FD) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

impl Drop for ExclusiveInstallLock {
    fn drop(&mut self) {
        if self.restore_shared {
            if let Err(error) = FileExt::try_lock_shared(&self.lock) {
                eprintln!(
                    "Platform update could not restore the shared installation lock: {error}"
                );
            }
        }
    }
}

pub(super) async fn install(
    app: AppHandle,
    root: PathBuf,
    preferences: Preferences,
    transport: Transport,
    ready: Ready,
) -> Result<(), String> {
    let lock = app
        .state::<AppState>()
        .platform_update
        .install_lock
        .as_ref()
        .ok_or(CANNOT_INSTALL)?
        .try_clone()
        .map_err(|_| CANNOT_INSTALL)?;
    let task_root = root.clone();
    let (prepared, exclusive) = tokio::task::spawn_blocking(move || {
        transport.verify_ready(&ready).map_err(io::Error::other)?;
        let bundle = installed_bundle()?;
        check_bundle(&bundle, APP_VERSION)?;
        let metadata = fs::symlink_metadata(&bundle)?;
        let session =
            SessionDirectory::create_in(&File::open(&task_root)?).map_err(io::Error::other)?;
        save_plan(
            session.directory(),
            &Plan {
                version: ready.release.version,
                filename: ready.release.filename,
                device: metadata.dev(),
                inode: metadata.ino(),
                preferences,
            },
        )?;
        let log = platform_native::filesystem::open_private_file(
            &task_root.join("installer.log"),
            true,
            false,
        )?;
        log.set_len(0)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(HELPER_ARGUMENT)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log));
        verify_named_lock(&task_root, &lock)?;
        let exclusive = ExclusiveInstallLock::promote(lock)?;
        exclusive.pass_to_helper(&mut command)?;
        let mut endpoint = ParentEndpoint::launch(session, command).map_err(io::Error::other)?;
        endpoint
            .wait_registered(ACK_BUDGET)
            .map_err(io::Error::other)?;
        Ok::<_, io::Error>((endpoint, exclusive))
    })
    .await
    .map_err(|_| CANNOT_INSTALL)?
    .map_err(|error| {
        eprintln!("Platform update helper preparation: {error}");
        CANNOT_INSTALL
    })?;

    let state = app.state::<AppState>();
    if state.shutting_down.load(Ordering::Acquire)
        || state.local_data_cleanup.active()
        || state.handle.update_has_active_work()
    {
        return Err("请等待相关任务完成后再安装".into());
    }
    state
        .platform_update
        .hold_exit
        .store(true, Ordering::Release);
    let shutdown = state.desktop_shutdown.clone();
    if shutdown.arm_handoff(prepared.binding().clone()).is_err() {
        state
            .platform_update
            .hold_exit
            .store(false, Ordering::Release);
        return Err(CANNOT_INSTALL.into());
    }
    desktop_shutdown_native::request_for_cleanup(&app);
    let deadline = Instant::now() + DRAIN_BUDGET;
    let permit = loop {
        let report = shutdown.status();
        if let Some(receipt) = shutdown.take_clean_drain_receipt() {
            break Some(receipt);
        }
        if report.status == Status::Blocked
            || report.status == Status::ReadyForNativeExit && !report.issues.is_empty()
            || Instant::now() >= deadline
        {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let committed = if let Some(receipt) = permit {
        tokio::task::spawn_blocking(move || {
            let mut prepared = prepared;
            let mut exclusive = exclusive;
            let accepted = match prepared.acknowledge_drained(receipt, ACK_BUDGET) {
                Ok(accepted) => accepted,
                Err(error) => {
                    eprintln!("Platform update drain acknowledgement: {error}");
                    return false;
                }
            };
            // An Arm sent to the helper cannot be assumed aborted after a
            // lost reply. Keep the exclusive lock through native exit.
            exclusive.restore_shared = false;
            match prepared.leave_witness_armed(accepted, ACK_BUDGET) {
                Ok(_) => true,
                Err(HandoffError::HandoffCommitUnknown) => {
                    eprintln!("Platform update handoff commit outcome is unknown");
                    true
                }
                Err(error) => {
                    exclusive.restore_shared = true;
                    eprintln!("Platform update handoff: {error}");
                    false
                }
            }
        })
        .await
        .unwrap_or(false)
    } else {
        drop(prepared);
        drop(exclusive);
        false
    };

    if !committed {
        state
            .platform_update
            .hold_exit
            .store(false, Ordering::Release);
        if shutdown.status().status == Status::ReadyForNativeExit {
            state.exit_ready.store(true, Ordering::Release);
            app.exit(0);
        }
        return Err("更新没能完成安全退出，未开始安装".into());
    }
    state.exit_ready.store(true, Ordering::Release);
    app.exit(0);
    Ok(())
}

fn unpack(file: File, location: &Path) -> io::Result<PathBuf> {
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let mut top = None;
    let mut unpacked = 0u64;
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        let path = entry.path()?;
        let mut pieces = path.components();
        let Some(Component::Normal(name)) = pieces.next() else {
            return Err(io::ErrorKind::InvalidData.into());
        };
        let kind = entry.header().entry_type();
        unpacked = unpacked
            .checked_add(entry.size())
            .ok_or(io::ErrorKind::InvalidData)?;
        if index >= MAX_ENTRIES
            || unpacked > MAX_UNPACKED_BYTES
            || Path::new(name).extension() != Some(OsStr::new("app"))
            || !pieces.all(|piece| matches!(piece, Component::Normal(_)))
            || !kind.is_file() && !kind.is_dir()
            || top
                .as_ref()
                .is_some_and(|previous: &PathBuf| previous.as_os_str() != name)
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        top.get_or_insert_with(|| PathBuf::from(name));
        if !entry.unpack_in(location)? {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok(location.join(top.ok_or(io::ErrorKind::InvalidData)?))
}

fn swap(left: &Path, right: &Path) -> io::Result<()> {
    let left = CString::new(left.as_os_str().as_bytes())?;
    let right = CString::new(right.as_os_str().as_bytes())?;
    if unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn reopen(bundle: &Path) -> io::Result<()> {
    let status = Command::new("/usr/bin/open")
        .arg("-n")
        .arg(bundle)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::ErrorKind::Interrupted.into())
    }
}

fn install_after_exit(
    observed: platform_update_handoff::ObservedParentExit,
    lock: File,
) -> io::Result<()> {
    let plan = load_plan(observed.directory())?;
    let bundle = installed_bundle()?;
    let paths = RuntimePaths::discover().map_err(io::Error::other)?;
    let root = update_root(paths.data_root()).map_err(io::Error::other)?;
    let named = verify_named_lock(&root, &lock)?;
    // An independent open must be blocked by the inherited exclusive flock,
    // and the inherited description itself must be able to assert that lock.
    match FileExt::try_lock_shared(&named) {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        _ => return Err(io::ErrorKind::PermissionDenied.into()),
    }
    FileExt::try_lock_exclusive(&lock)?;
    let recorded = fs::symlink_metadata(&bundle)?;
    if recorded.dev() != plan.device || recorded.ino() != plan.inode {
        return Err(io::ErrorKind::InvalidData.into());
    }
    check_bundle(&bundle, APP_VERSION)?;

    let result = |outcome, notes: &str| InstallationResult {
        previous: APP_VERSION.into(),
        version: plan.version.clone(),
        notes: notes.into(),
        outcome,
    };
    let operation = (|| {
        write_json(&root, RESULT_FILE, &result(InstallOutcome::Unknown, ""))?;
        let parent = bundle.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let (transport, _) = transport_for(&plan.preferences).map_err(io::Error::other)?;
        let ready = transport
            .recover(&root, APP_VERSION)
            .map_err(io::Error::other)?
            .filter(|ready| {
                ready.release.version == plan.version && ready.release.filename == plan.filename
            })
            .ok_or(io::ErrorKind::InvalidData)?;
        let candidate = transport.verify_ready(&ready).map_err(io::Error::other)?;
        let stage = tempfile::Builder::new()
            .prefix(".panda-install-")
            .tempdir_in(parent)?;
        let replacement = unpack(candidate, stage.path())?;
        check_bundle(&replacement, &plan.version)?;
        let actual = fs::symlink_metadata(&bundle)?;
        if actual.dev() != recorded.dev() || actual.ino() != recorded.ino() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        swap(&bundle, &replacement)?;
        if let Err(error) = write_json(
            &root,
            RESULT_FILE,
            &result(InstallOutcome::Installed, &ready.release.notes),
        ) {
            if swap(&bundle, &replacement).is_err() {
                let _ = stage.keep();
            }
            return Err(error);
        }
        Ok((stage, replacement))
    })();
    match operation {
        Ok((stage, previous)) => {
            FileExt::unlock(&lock)?;
            if let Err(error) = reopen(&bundle) {
                // No launch was accepted. Keep the known previous copy if we
                // cannot re-enter the same installation lock for restoration.
                if FileExt::try_lock_exclusive(&lock).is_ok() {
                    if previous.exists() && swap(&bundle, &previous).is_ok() {
                        write_json(
                            &root,
                            RESULT_FILE,
                            &result(InstallOutcome::PreviousPreserved, ""),
                        )?;
                        FileExt::unlock(&lock)?;
                        let _ = reopen(&bundle);
                        return Err(error);
                    }
                }
                let _ = stage.keep();
                return Err(error);
            }
            Ok(())
        }
        Err(error) => {
            let actual = fs::symlink_metadata(&bundle).ok();
            if actual
                .is_some_and(|meta| meta.dev() == recorded.dev() && meta.ino() == recorded.ino())
            {
                write_json(
                    &root,
                    RESULT_FILE,
                    &result(InstallOutcome::PreviousPreserved, ""),
                )?;
                FileExt::unlock(&lock)?;
                let _ = reopen(&bundle);
            }
            Err(error)
        }
    }
}

pub(crate) fn dispatch_helper() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(OsStr::new(HELPER_ARGUMENT)) {
        return;
    }
    if arguments.next().is_some()
        || !super::enabled()
        || [3, 4, INSTALL_LOCK_FD]
            .iter()
            .any(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } < 0)
    {
        std::process::exit(2);
    }
    let lock = unsafe { File::from_raw_fd(INSTALL_LOCK_FD) };
    if unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        std::process::exit(2);
    }
    // Only the explicit helper entry owns the launcher descriptors.
    let result = unsafe { observe_parent_exit_from_inherited_descriptors() }
        .map_err(io::Error::other)
        .and_then(|observed| {
            observed.map_or(Ok(()), |observed| install_after_exit(observed, lock))
        });
    if let Err(error) = result {
        eprintln!("Platform update installation: {error}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
