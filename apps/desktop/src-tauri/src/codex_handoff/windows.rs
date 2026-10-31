//! Current-user MSIX activation, never an ambient protocol or guessed exe.
//! Observed signed OpenAI.Codex 26.1027.3384.0: AppxManifest's App identity and
//! OpenProjectInCodex Explorer verb use family!App and `--open-project` (the
//! latter is present in its shipped windows-account.node UTF-16 strings).
//! Package identity/distribution: https://learn.chatgpt.com/docs/enterprise/windows-deployment
//! Explicit OS activation contract: https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-iapplicationactivationmanager-activateapplication
use platform_core::ApiError;
use platform_native::windows::{api_paths::conventional_path, files as native};
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{mpsc, OnceLock},
    time::{Duration, Instant},
};
use windows::{
    core::{HSTRING, PCWSTR},
    ApplicationModel::{Package, PackageSignatureKind},
    Management::Deployment::PackageManager,
    Win32::{
        System::{
            Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER},
            WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
        },
        UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, AO_NOERRORUI},
    },
};
const NAME: &str = "OpenAI.Codex";
const FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";
const PUBLISHER: &str = "CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B";
const APPLICATION: &str = "OpenAI.Codex_2p2nqsd0c76g0!App";
type Result<T> = std::result::Result<T, ApiError>;
fn not_installed() -> ApiError {
    ApiError::new(
        "CODEX_NOT_INSTALLED",
        "未找到已安装的 Codex 桌面应用。可手动打开 Codex、选择下方项目目录，再粘贴接手说明。",
    )
}
fn invalid() -> ApiError {
    ApiError::new(
        "CODEX_INSTALLATION_INVALID",
        "无法验证 Codex 安装，请检查应用是否完整。接手说明已保留，可手动选择项目目录。",
    )
}
fn open_failed() -> ApiError {
    ApiError::new(
        "CODEX_OPEN_FAILED",
        "未能确认 Codex 已打开，请手动选择下方项目目录并粘贴说明。",
    )
}
fn path_invalid() -> ApiError {
    ApiError::new("CODEX_PATH_INVALID", "项目目录必须是可读取的绝对路径")
}

fn package_authorized(
    name: &str,
    family: &str,
    publisher: &str,
    store_signed: bool,
    healthy: bool,
) -> bool {
    name == NAME && family == FAMILY && publisher == PUBLISHER && store_signed && healthy
}
fn find_app(verify_contents: bool) -> Result<(Package, PathBuf)> {
    let manager = PackageManager::new().map_err(|_| invalid())?;
    // Empty SID means the calling user, without enumerating other users or
    // installing/provisioning packages or consulting a user-writable PATH.
    let packages = manager
        .FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(FAMILY))
        .map_err(|_| invalid())?;
    let mut selected = None;
    for package in packages {
        if package.IsFramework().map_err(|_| invalid())?
            || package.IsResourcePackage().map_err(|_| invalid())?
        {
            continue;
        }
        let id = package.Id().map_err(|_| invalid())?;
        if !package_authorized(
            &id.Name().map_err(|_| invalid())?.to_string(),
            &id.FamilyName().map_err(|_| invalid())?.to_string(),
            &id.Publisher().map_err(|_| invalid())?.to_string(),
            package.SignatureKind().map_err(|_| invalid())? == PackageSignatureKind::Store,
            package
                .Status()
                .and_then(|status| status.VerifyIsOK())
                .map_err(|_| invalid())?,
        ) {
            return Err(invalid());
        }
        let version = id.Version().map_err(|_| invalid())?;
        let version = (
            version.Major,
            version.Minor,
            version.Build,
            version.Revision,
        );
        if selected.as_ref().is_none_or(|(prior, _)| version > *prior) {
            selected = Some((version, package));
        }
    }
    let (_, package) = selected.ok_or_else(not_installed)?;
    if verify_contents {
        let verification = package
            .VerifyContentIntegrityAsync()
            .map_err(|_| invalid())?;
        let deadline = Instant::now() + Duration::from_secs(20);
        // WinRT AsyncStatus::Started is zero. A timeout never proceeds to activation;
        // cancelling verification is not reported as editor/process termination.
        while verification.Status().map_err(|_| invalid())?.0 == 0 {
            if Instant::now() >= deadline {
                let _ = verification.Cancel();
                return Err(invalid());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !verification.GetResults().map_err(|_| invalid())? {
            return Err(invalid());
        }
    }
    let path = package.InstalledPath().map_err(|_| invalid())?;
    let path = PathBuf::from(OsString::from_wide(&path));
    if !path.is_absolute() {
        return Err(invalid());
    }
    Ok((package, path))
}

/// Windows command-line quoting, passed directly to app activation, not cmd.
/// The selected project is exactly one argument, including trailing slashes.
fn arguments(root: &Path) -> Result<Vec<u16>> {
    root.to_str()
        .filter(|_| root.is_absolute())
        .ok_or_else(path_invalid)?;
    let root = conventional_path(root).map_err(|_| path_invalid())?;
    let units = root.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(path_invalid());
    }
    let mut command = "--open-project \"".encode_utf16().collect::<Vec<_>>();
    let mut slashes = 0;
    for unit in units {
        if unit == 92 {
            slashes += 1;
        } else if unit == 34 {
            command.extend(std::iter::repeat_n(92, slashes * 2 + 1));
            command.push(unit);
            slashes = 0;
        } else {
            command.extend(std::iter::repeat_n(92, slashes));
            slashes = 0;
            command.push(unit);
        }
    }
    command.extend(std::iter::repeat_n(92, slashes * 2));
    command.extend([34, 0]);
    Ok(command)
}
fn activate(app: &Path, root: &Path) -> Result<()> {
    let arguments = arguments(root)?;
    let (_package, registered) = find_app(false)?;
    let registered = native::pin_directory(&registered).map_err(|_| invalid())?;
    let supplied = native::pin_directory(app).map_err(|_| invalid())?;
    if !native::identity(&registered.file)
        .map_err(|_| invalid())?
        .same_object(&native::identity(&supplied.file).map_err(|_| invalid())?)
    {
        return Err(invalid());
    }
    let manager: IApplicationActivationManager =
        unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER) }
            .map_err(|_| open_failed())?;
    let application = APPLICATION.encode_utf16().chain([0]).collect::<Vec<_>>();
    let pid = unsafe {
        manager.ActivateApplication(
            PCWSTR(application.as_ptr()),
            PCWSTR(arguments.as_ptr()),
            AO_NOERRORUI,
        )
    }
    .map_err(|_| open_failed())?;
    if pid == 0 {
        return Err(open_failed());
    }
    // S_OK/PID proves accepted OS dispatch, not a loaded project or a new chat.
    Ok(())
}

enum Request {
    Installed(tokio::sync::oneshot::Sender<Result<PathBuf>>),
    Open(PathBuf, PathBuf, tokio::sync::oneshot::Sender<Result<()>>),
}
static WORKER: OnceLock<std::result::Result<mpsc::Sender<Request>, String>> = OnceLock::new();
fn worker() -> Result<mpsc::Sender<Request>> {
    WORKER
        .get_or_init(|| {
            let (send, receive) = mpsc::channel::<Request>();
            std::thread::Builder::new()
                .name("codex-installed-activation".into())
                .spawn(move || {
                    let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
                    for request in receive {
                        match request {
                            Request::Installed(reply) => {
                                if reply.is_closed() {
                                    continue;
                                }
                                let result = if initialized.is_ok() {
                                    find_app(true).map(|(_, path)| path)
                                } else {
                                    Err(invalid())
                                };
                                let _ = reply.send(result);
                            }
                            Request::Open(app, root, reply) => {
                                if reply.is_closed() {
                                    continue;
                                }
                                let result = if initialized.is_ok() {
                                    activate(&app, &root)
                                } else {
                                    Err(open_failed())
                                };
                                let _ = reply.send(result);
                            }
                        }
                    }
                    if initialized.is_ok() {
                        unsafe { RoUninitialize() };
                    }
                })
                .map_err(|error| error.to_string())?;
            // A single process-lifetime MTA avoids apartment shutdown racing other
            // requests. All WinRT/COM interfaces remain on this one worker thread.
            Ok(send)
        })
        .as_ref()
        .cloned()
        .map_err(|_| invalid())
}
pub async fn installed_app() -> Result<PathBuf> {
    let (reply, receive) = tokio::sync::oneshot::channel();
    worker()?
        .send(Request::Installed(reply))
        .map_err(|_| invalid())?;
    receive.await.map_err(|_| invalid())?
}
pub async fn open(app: &Path, root: &Path) -> Result<()> {
    // Reject malformed input before queueing any editor/system action.
    let _ = arguments(root)?;
    let (reply, receive) = tokio::sync::oneshot::channel();
    worker()?
        .send(Request::Open(app.into(), root.into(), reply))
        .map_err(|_| open_failed())?;
    receive.await.map_err(|_| open_failed())?
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
