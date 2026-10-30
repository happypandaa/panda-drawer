//! Native file-manager dispatch without a shell or development operations.
use super::{require_manager, AppState};
use platform_core::ApiError;
#[cfg(target_os = "macos")]
use std::time::Duration;
use std::{path::Path, process::Stdio};
use tokio::process::Command;

#[tauri::command]
pub(super) async fn open_project_folder(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    io_lease
        .run(async move {
            let location = Path::new(&path);
            if !location.is_absolute() {
                return Err(ApiError::new("REVEAL_FAILED", "本地目录位置无效"));
            }
            let metadata = tokio::fs::metadata(location).await.map_err(|error| {
                ApiError::new(
                    "REVEAL_FAILED",
                    if error.kind() == std::io::ErrorKind::NotFound {
                        "项目文件夹不存在，请核对显示的路径"
                    } else {
                        "无法访问项目文件夹，请检查目录权限"
                    },
                )
            })?;
            if !metadata.is_dir() {
                return Err(ApiError::new("REVEAL_FAILED", "项目位置不是文件夹"));
            }
            open(location, false).await
        })
        .await?
}

#[cfg(target_os = "macos")]
fn finder_command(location: &Path, select_file: bool) -> Result<Command, ApiError> {
    if !location.is_absolute() {
        return Err(ApiError::new("REVEAL_FAILED", "本地目录位置无效"));
    }
    let mut command = Command::new("/usr/bin/open");
    if select_file {
        command.arg("-R");
    } else {
        // A directory must always open in Finder, even if it has an app-like
        // extension. Paths never choose an executable or become shell input.
        command.args(["-a", "Finder"]);
    }
    command
        .arg(location)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

#[cfg(windows)]
fn explorer_command(location: &Path, select_file: bool) -> Result<Command, ApiError> {
    if !location.is_absolute() {
        return Err(ApiError::new("REVEAL_FAILED", "本地目录位置无效"));
    }
    let system_root = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or_else(|| ApiError::new("REVEAL_FAILED", "无法找到系统文件管理器"))?;
    let mut command = Command::new(system_root.join("explorer.exe"));
    if select_file {
        let mut argument = std::ffi::OsString::from("/select,");
        argument.push(location);
        command.arg(argument);
    } else {
        command.arg(location);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

pub(super) async fn open(location: &Path, select_file: bool) -> Result<(), ApiError> {
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    return Err(ApiError::new(
        "REVEAL_UNSUPPORTED",
        "此系统暂不支持打开本地目录",
    ));
    #[cfg(target_os = "macos")]
    match super::native_process::status(
        finder_command(location, select_file)?,
        Duration::from_secs(5),
    )
    .await
    {
        Ok(status) if status.success() => Ok(()),
        _ => Err(ApiError::new(
            "REVEAL_FAILED",
            "无法打开本地目录，请检查目录是否仍可访问",
        )),
    }
    #[cfg(windows)]
    {
        // Explorer forwards to an existing shell and its exit code is not a
        // delivery acknowledgement. Only report failure to dispatch; do not
        // wait for or terminate the user's file-manager process.
        explorer_command(location, select_file)?
            .spawn()
            .map(|_| ())
            .map_err(|_| ApiError::new("REVEAL_FAILED", "无法打开本地目录，请检查目录是否仍可访问"))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn directory_is_one_literal_finder_argument() {
        let location = Path::new("/tmp/可丢弃 $(echo test) '#?&.app");
        let command = finder_command(location, false).unwrap();
        let command = command.as_std();
        assert_eq!(command.get_program(), "/usr/bin/open");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-a", "Finder", location.to_str().unwrap()]
        );
        let command = finder_command(location, true).unwrap();
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            ["-R", location.to_str().unwrap()]
        );
        assert!(finder_command(Path::new("relative"), false).is_err());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn explorer_receives_one_literal_path_and_no_shell() {
        let location = Path::new(r"C:\可丢弃 项目\$(echo test) '#&,comma.app");
        let command = explorer_command(location, false).unwrap();
        let system_root = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap());
        assert_eq!(
            command.as_std().get_program(),
            system_root.join("explorer.exe")
        );
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            [location.as_os_str()]
        );
        let command = explorer_command(location, true).unwrap();
        let mut argument = OsString::from("/select,");
        argument.push(location);
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            [argument.as_os_str()]
        );
        for relative in ["relative", r"C:relative", r"\rooted"] {
            assert!(explorer_command(Path::new(relative), false).is_err());
        }
    }
}
