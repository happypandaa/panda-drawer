mod installation;

use super::{
    transport::{Ready, Transport},
    transport_for, update_root, write_json, InstallOutcome, InstallationResult, Preferences,
    CANNOT_INSTALL, RESULT_FILE,
};
use crate::{
    desktop_shutdown::{CleanDrainReceipt, Status},
    desktop_shutdown_native, AppState,
};
use installation::{pin_file, Installation};
use platform_core::{build_profile::APP_VERSION, RuntimePaths};
use platform_native::windows::files;
use platform_update_handoff::{ObservedParentExit, ParentEndpoint, SessionDirectory};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, Read, Seek},
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

const LOCK: &str = "install.lock";
const GATE: &str = "handoff.lock";
const HELPER: &str = "--platform-update-helper";
const ACK: Duration = Duration::from_secs(8);

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    volume: u64,
    id: [u8; 16],
}
impl Identity {
    fn of(file: &File) -> io::Result<Self> {
        let id = files::identity(file)?;
        Ok(Self {
            volume: id.volume_serial,
            id: id.file_id,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: String,
    filename: String,
    directory: Identity,
    executable: Identity,
    preferences: Preferences,
    english: bool,
}

pub(crate) fn installed() -> bool {
    std::env::current_exe()
        .and_then(|path| Installation::inspect(&path, APP_VERSION))
        .is_ok()
}

// Byte-range locking gates concurrent launches. Denying share-delete on the same
// file also keeps the name bound to the same underlying object for the lock's lifetime.
fn open_lock_file(root: &Path, name: &str, create: bool) -> io::Result<File> {
    use ::windows::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let _scope = files::pin_directory(root)?;
    let path = root.join(name);
    let checked = files::open_private_file(&path, create, false)?;
    let pin = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    if Identity::of(&pin)? != Identity::of(&checked)? || files::identity(&pin)?.links != 1 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(pin)
}

fn open_lock(root: &Path, create: bool) -> io::Result<File> {
    open_lock_file(root, LOCK, create)
}

pub(crate) fn verify_named_lock(root: &Path, lock: &File) -> io::Result<()> {
    if Identity::of(lock)? != Identity::of(&open_lock(root, false)?)? {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

pub(crate) fn start_lock(root: &Path) -> io::Result<File> {
    // A registered helper holds this gate before the parent starts draining.
    // No new launch may slip between that parent's exit and the NSIS launch.
    let admission = open_lock_file(root, GATE, true)?;
    admission.try_lock_shared()?;
    let lock = open_lock(root, true)?;
    if lock.try_lock().is_ok() {
        // Only a launch with no live desktop may collect old helper images.
        // Another launch can legitimately be writing a new session.
        platform_update_handoff::cleanup_sessions(&files::directory_file(root)?)?;
        lock.unlock()?;
    }
    lock.try_lock_shared()?;
    Installation::inspect(&std::env::current_exe()?, APP_VERSION)?;
    Ok(lock)
}

pub(crate) fn lock_before_startup() -> io::Result<Option<File>> {
    if !super::configured() {
        return Ok(None);
    }
    // First launch can precede runtime initialization. Still check the installed
    // identity before allowing this process to register as the desktop singleton.
    Installation::inspect(&std::env::current_exe()?, APP_VERSION)?;
    let Ok(paths) = RuntimePaths::discover() else {
        return Ok(None);
    };
    start_lock(&update_root(paths.data_root()).map_err(io::Error::other)?).map(Some)
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
    let endpoint = tokio::task::spawn_blocking(move || -> io::Result<_> {
        verify_named_lock(&root, &lock)?;
        let installed = Installation::inspect(&std::env::current_exe()?, APP_VERSION)?;
        transport.verify_ready(&ready).map_err(io::Error::other)?;
        let session = SessionDirectory::create_in(&files::directory_file(&root)?)?;
        let plan = Plan {
            version: ready.release.version,
            filename: ready.release.filename,
            preferences,
            english: crate::ui_locale::current() == "en",
            directory: Identity::of(&installed.directory_pin.file)?,
            executable: Identity::of(&installed.executable_pin)?,
        };
        let mut endpoint = ParentEndpoint::<CleanDrainReceipt>::launch_with_plan(
            session,
            &serde_json::to_vec(&plan)?,
        )?;
        endpoint.wait_registered(ACK)?;
        Ok(endpoint)
    })
    .await
    .map_err(|_| CANNOT_INSTALL)?
    .map_err(|error| {
        eprintln!("Windows update preparation: {error}");
        CANNOT_INSTALL
    })?;

    let state = app.state::<AppState>();
    if state.shutting_down.load(Ordering::Acquire)
        || state.local_data_cleanup.active()
        || state.handle.update_has_active_work()
    {
        return Err("请等待相关任务完成后再安装".into());
    }
    let shutdown = state.desktop_shutdown.clone();
    state
        .platform_update
        .hold_exit
        .store(true, Ordering::Release);
    if shutdown.arm_handoff(endpoint.binding().clone()).is_err() {
        state
            .platform_update
            .hold_exit
            .store(false, Ordering::Release);
        return Err(CANNOT_INSTALL.into());
    }
    desktop_shutdown_native::request_for_cleanup(&app);
    let deadline = Instant::now() + Duration::from_secs(18);
    let receipt = loop {
        if let Some(receipt) = shutdown.take_clean_drain_receipt() {
            break Some(receipt);
        }
        let status = shutdown.status();
        if status.status == Status::Blocked
            || !status.issues.is_empty()
            || Instant::now() >= deadline
        {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let committed = if let Some(receipt) = receipt {
        tokio::task::spawn_blocking(move || match endpoint.commit(receipt, ACK) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("Windows update handoff: {error}");
                error.kind() == io::ErrorKind::ConnectionAborted
            }
        })
        .await
        .unwrap_or(false)
    } else {
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

struct PackageCopy {
    path: PathBuf,
    identity: files::FileIdentity,
}
impl Drop for PackageCopy {
    fn drop(&mut self) {
        let _ = files::remove_file(&self.path, &self.identity);
    }
}

fn prepare_installer(
    session: &Path,
    transport: &Transport,
    ready: &Ready,
) -> io::Result<(PackageCopy, File)> {
    // Keep the cached input immutable across the final signature check and copy.
    let _source_pin = pin_file(&ready.path)?;
    let mut source = transport.verify_ready(ready).map_err(io::Error::other)?;
    let path = session.join("update-installer.exe");
    let mut destination = files::create_private_file(&path)?;
    let package = PackageCopy {
        identity: files::identity(&destination)?,
        path,
    };
    io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    drop(destination);
    let mut pin = pin_file(&package.path)?;
    if !package.identity.same_object(&files::identity(&pin)?)
        || pin.metadata()?.len() != source.metadata()?.len()
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    // There is a close/reopen between writing and acquiring the execution pin.
    // Compare the now-immutable NSIS bytes with the still-pinned verified input.
    source.rewind()?;
    let (mut expected, mut actual) = ([0u8; 65536], [0u8; 65536]);
    loop {
        let count = source.read(&mut expected)?;
        if count == 0 {
            break;
        }
        pin.read_exact(&mut actual[..count])?;
        if actual[..count] != expected[..count] {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok((package, pin))
}

struct RegisteredInstall {
    root: PathBuf,
    _root_pin: files::PinnedDirectory,
    gate: File,
    plan: Plan,
    installed: Installation,
}

fn register_install(
    directory: &File,
    original: &Path,
    bytes: &[u8],
) -> io::Result<RegisteredInstall> {
    let paths = RuntimePaths::discover().map_err(io::Error::other)?;
    let root = update_root(paths.data_root()).map_err(io::Error::other)?;
    let root_pin = files::pin_directory(&root)?;
    let (session, _scope) = files::directory_scope(directory)?;
    let parent = session.parent().ok_or(io::ErrorKind::InvalidData)?;
    if Identity::of(&files::directory_file(parent)?)? != Identity::of(&root_pin.file)? {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let plan: Plan = serde_json::from_slice(bytes)?;
    let installed = Installation::inspect(original, APP_VERSION)?;
    if Identity::of(&installed.directory_pin.file)? != plan.directory
        || Identity::of(&installed.executable_pin)? != plan.executable
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let gate = open_lock_file(&root, GATE, false)?;
    gate.try_lock()?;
    Ok(RegisteredInstall {
        root,
        _root_pin: root_pin,
        gate,
        plan,
        installed,
    })
}

fn install_after_exit(
    (observed, registered): (ObservedParentExit, RegisteredInstall),
) -> io::Result<()> {
    let RegisteredInstall {
        root,
        _root_pin,
        gate,
        plan,
        installed,
    } = registered;
    let (session, _session_scope) = files::directory_scope(observed.directory())?;
    let result = |outcome, notes: &str| InstallationResult {
        previous: APP_VERSION.into(),
        version: plan.version.clone(),
        notes: notes.to_owned(),
        outcome,
    };
    let mut started = false;
    let operation = (|| -> io::Result<Installation> {
        // An instance that already passed admission must also have exited.
        // Keep this lifetime lock until the replacement's identity is checked.
        let lock = open_lock(&root, false)?;
        lock.try_lock()?;
        let (transport, _) = transport_for(&plan.preferences).map_err(io::Error::other)?;
        let ready = transport
            .recover(&root, APP_VERSION)
            .map_err(io::Error::other)?
            .filter(|r| {
                r.release.version == plan.version
                    && r.release.filename == plan.filename
                    && r.release.target == "windows-x86_64"
            })
            .ok_or(io::ErrorKind::InvalidData)?;
        let (package, pin) = prepare_installer(&session, &transport, &ready)?;
        let mut installer = Command::new(&package.path);
        installer
            .args(["/S", "/UPDATE"])
            .raw_arg(installed.nsis_directory_arg())
            .current_dir(&session)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // NSIS needs write access to the old executable. The parent process is
        // already gone; retain the verified installation directory and update lock.
        drop(installed.executable_pin);
        write_json(&root, RESULT_FILE, &result(InstallOutcome::Unknown, ""))?;
        let mut child = installer.spawn()?;
        started = true;
        let status = child.wait()?;
        drop(pin);
        drop(package);
        if !status.success() {
            return Err(io::Error::other(format!("NSIS exited with {status}")));
        }
        let replacement = Installation::inspect(observed.parent_executable(), &plan.version)?;
        if Identity::of(&replacement.directory_pin.file)? != plan.directory {
            return Err(io::ErrorKind::InvalidData.into());
        }
        write_json(
            &root,
            RESULT_FILE,
            &result(InstallOutcome::Installed, &ready.release.notes),
        )?;
        Ok(replacement)
    })();
    let (reopen, status) = match operation {
        Ok(replacement) => (Some(replacement), Ok(())),
        Err(error) => {
            // Once NSIS started it may have partially changed the installation.
            // Only assert preservation when we never launched it and the old image is intact.
            let original = observed.parent_executable();
            let verified = [APP_VERSION, &plan.version]
                .into_iter()
                .find_map(|version| Installation::inspect(original, version).ok())
                .filter(|app| {
                    Identity::of(&app.directory_pin.file).ok().as_ref() == Some(&plan.directory)
                });
            let preserved = !started
                && verified.as_ref().is_some_and(|app| {
                    Identity::of(&app.executable_pin).ok().as_ref() == Some(&plan.executable)
                });
            let outcome = if preserved {
                InstallOutcome::PreviousPreserved
            } else {
                InstallOutcome::Unknown
            };
            write_json(&root, RESULT_FILE, &result(outcome, ""))?;
            (verified, Err(error))
        }
    };
    // Both startup locks must be released before launching either version.
    // Keep the actual executable and directory pinned through CreateProcess.
    drop(gate);
    if let Some(app) = reopen {
        if let Err(error) = Command::new(&app.executable)
            .current_dir(&app.directory)
            .spawn()
        {
            show_reopen_failure(plan.english);
            return Err(error);
        }
    } else {
        show_reopen_failure(plan.english);
    }
    status
}

fn show_reopen_failure(english: bool) {
    use ::windows::{
        core::w,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };
    let (message, title) = if english {
        (w!("The application could not reopen after the update. Open the installed app again or use its installer to repair it."),
         w!("Application update"))
    } else {
        (
            w!("更新后无法重新打开应用。请再次打开已安装的应用，或使用安装包修复。"),
            w!("应用更新"),
        )
    };
    unsafe {
        MessageBoxW(None, message, title, MB_OK | MB_ICONERROR);
    }
}

pub(crate) fn dispatch_helper() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(HELPER)) {
        return;
    }
    if args.next().is_some() || !super::configured() {
        std::process::exit(2);
    }
    let result = platform_update_handoff::observe_parent_exit_with(register_install)
        .and_then(install_after_exit);
    if let Err(error) = result {
        eprintln!("Windows update helper: {error}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_and_installation_exclude_each_other_and_pin_the_registered_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("update");
        files::create_private_directory(&path).unwrap();
        let startup = open_lock(&path, true).unwrap();
        startup.try_lock_shared().unwrap();
        let installer = open_lock(&path, false).unwrap();
        assert!(installer.try_lock().is_err());
        assert!(std::fs::rename(path.join(LOCK), path.join("changed.lock")).is_err());
        drop(startup);
        installer.try_lock().unwrap();
        let next = open_lock(&path, false).unwrap();
        assert!(next.try_lock_shared().is_err());
        drop(installer);
        next.try_lock_shared().unwrap();
    }

    #[test]
    fn verified_execution_pin_blocks_writers_and_replacement_until_spawn_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("package.exe");
        std::fs::write(&path, b"disposable fixture").unwrap();
        let pin = pin_file(&path).unwrap();
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(pin);
        std::fs::write(&path, b"replacement").unwrap();
    }
}
