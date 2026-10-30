//! Manager-only browser initialization. Project store removal remains separate.
use platform_core::ApiError;
use tauri::{AppHandle, Url, WebviewWindowBuilder, Wry};

#[cfg(windows)]
#[path = "desktop_webview_profile/windows.rs"]
mod windows;

type ManagerBuilder<'a> = WebviewWindowBuilder<'a, Wry, AppHandle>;

pub(super) fn runtime_available(app: &AppHandle) -> bool {
    #[cfg(windows)]
    {
        windows::runtime_available(app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        true
    }
}

pub(super) fn initialize(app: &AppHandle, data_root: &std::path::Path) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        windows::initialize(app, data_root)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, data_root);
        Ok(())
    }
}

pub(super) fn manager_builder<'a>(
    app: &AppHandle,
    builder: ManagerBuilder<'a>,
) -> Result<ManagerBuilder<'a>, ApiError> {
    #[cfg(windows)]
    {
        windows::manager_builder(app, builder)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(builder.data_store_identifier(platform_core::webview_storage::manager_identifier()))
    }
}

/// macOS still clears and verifies its shared default store. Windows has no
/// such WebKit store: readiness means the explicit manager profile is bound.
pub(super) async fn initialize_storage(app: &AppHandle) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        windows::verify_initialized(app)
    }
    #[cfg(not(windows))]
    {
        super::native_stores::clear_default_and_verify(app).await
    }
}

pub(super) fn build_failure_manager(app: &AppHandle) -> Result<(), ApiError> {
    match super::build_manager_window(app) {
        // No initialized data root means Windows must not construct a default
        // profile as a fallback. The existing native startup dialog still runs.
        #[cfg(windows)]
        Err(error) if error.code == "MANAGER_PROFILE_NOT_INITIALIZED" => Ok(()),
        result => result,
    }
}

pub(super) fn is_manager_url(url: &Url, development: bool) -> bool {
    #[cfg(windows)]
    let bundled = windows::is_bundled_url(url);
    #[cfg(not(windows))]
    let bundled = url.scheme() == "tauri" && url.host_str() == Some("localhost");
    let developing = development
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420);
    url.username().is_empty() && url.password().is_none() && (bundled || developing)
}
