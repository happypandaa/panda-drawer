//! Native dispatch after the caller's existing location or URL checks.
#[cfg(target_os = "macos")]
#[path = "desktop_shell/macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "desktop_shell/windows.rs"]
mod platform;

#[cfg(windows)]
pub(super) use platform::explorer_command;
pub(super) use platform::{external_link, node_help, reveal};

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use platform_core::ApiError;
    use std::path::PathBuf;
    pub(crate) fn reveal(_: PathBuf, _: bool) -> Result<(), ApiError> {
        Err(ApiError::new("REVEAL_FAILED", "无法打开项目位置"))
    }
    pub(crate) fn external_link(_: &tauri::Url) {}
    pub(crate) fn node_help(_: String) -> Result<(), ApiError> {
        Err(ApiError::new(
            "BROWSER_OPEN_FAILED",
            "无法打开浏览器，请检查系统默认浏览器设置。",
        ))
    }
}
