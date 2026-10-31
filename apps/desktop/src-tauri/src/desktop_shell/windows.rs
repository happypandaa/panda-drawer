use platform_core::ApiError;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        },
        UI::{
            Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

pub(crate) fn explorer_command(location: &Path, select_file: bool) -> Result<Command, ApiError> {
    if !location.is_absolute() {
        return Err(ApiError::new("REVEAL_FAILED", "本地目录位置无效"));
    }
    let root = platform_native::windows::system_paths::SystemPaths::current()
        .map_err(|_| ApiError::new("REVEAL_FAILED", "无法找到系统文件管理器"))?;
    let location = platform_native::windows::api_paths::conventional_path(location)?;
    let mut command = Command::new(root.windows.join("explorer.exe"));
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

pub(crate) fn reveal(location: PathBuf, select_file: bool) -> Result<(), ApiError> {
    // Explorer may forward to an existing window. Its exit code is not a
    // delivery receipt, and the user's shell must never be terminated.
    explorer_command(&location, select_file)?
        .spawn()
        .map(|_| ())
        .map_err(|_| ApiError::new("REVEAL_FAILED", "无法打开项目位置"))
}

fn browser_error() -> ApiError {
    ApiError::new(
        "BROWSER_OPEN_FAILED",
        "无法打开浏览器，请检查系统默认浏览器设置。",
    )
}

fn url_argument(url: &str) -> Result<Vec<u16>, ApiError> {
    // The original callers keep their separate HTTPS/public-link policies.
    // This boundary only preserves the one URL and the native terminator.
    if url.contains('\0') {
        return Err(browser_error());
    }
    Ok(url.encode_utf16().chain(Some(0)).collect())
}

fn open_browser(url: &str) -> Result<(), ApiError> {
    let argument = url_argument(url)?;
    std::thread::Builder::new()
        .name("native-browser-open".into())
        .spawn(move || {
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
                .ok()
                .map_err(|_| browser_error())?;
            struct Apartment;
            impl Drop for Apartment {
                fn drop(&mut self) {
                    unsafe { CoUninitialize() };
                }
            }
            let _apartment = Apartment;
            // Keep this STA through native dispatch. The one URL is lpFile,
            // with no command parameters or environment expansion. We never
            // request or wait for the user's browser process handle.
            let mut execute = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
                lpVerb: w!("open"),
                lpFile: PCWSTR(argument.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            unsafe { ShellExecuteExW(&mut execute) }.map_err(|_| browser_error())
        })
        .map_err(|_| browser_error())?
        .join()
        .map_err(|_| browser_error())?
}

pub(crate) fn external_link(url: &tauri::Url) {
    let _ = open_browser(url.as_str());
}
pub(crate) fn node_help(url: String) -> Result<(), ApiError> {
    open_browser(&url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_argument_is_one_literal_utf16_url_without_shell_parameters() {
        let url = "https://example.com/中文?q=a%20b&next=%22quoted%22#part";
        let value = url_argument(url).unwrap();
        assert_eq!(value.last(), Some(&0));
        assert_eq!(String::from_utf16(&value[..value.len() - 1]).unwrap(), url);
        assert!(url_argument("https://example.com/\0https://other.test").is_err());
    }
}
