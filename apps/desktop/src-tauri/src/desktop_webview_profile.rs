//! Manager-only browser initialization. Project store removal remains separate.
use platform_core::ApiError;
use tauri::{AppHandle, Url, WebviewWindowBuilder, Wry};

#[cfg(windows)]
#[path = "desktop_webview_profile/windows.rs"]
mod windows;

type ManagerBuilder<'a> = WebviewWindowBuilder<'a, Wry, AppHandle>;

#[cfg(windows)]
pub(super) async fn close_and_clear_manager(app: &AppHandle) -> Result<(), ApiError> {
    windows::close_and_clear_manager(app).await
}
pub(super) fn manager_closed_for_cleanup(app: &AppHandle) -> bool {
    #[cfg(windows)]
    {
        windows::manager_closed_for_cleanup(app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
}

pub(super) fn manager_from_config<'a>(
    app: &'a AppHandle,
    config: &tauri::utils::config::WindowConfig,
) -> Result<ManagerBuilder<'a>, ApiError> {
    #[cfg(windows)]
    {
        windows::manager_from_config(app, config)
    }
    #[cfg(not(windows))]
    {
        WebviewWindowBuilder::from_config(app, config)
            .map_err(|e| ApiError::new("MANAGER_WINDOW_OPEN_FAILED", e.to_string()))
    }
}

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

pub(super) fn build_manager(app: &AppHandle, builder: ManagerBuilder<'_>) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        windows::build_manager(app, builder)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        builder
            .build()
            .map_err(|e| ApiError::new("MANAGER_WINDOW_OPEN_FAILED", e.to_string()))?;
        Ok(())
    }
}

pub(super) async fn finish_startup(app: AppHandle, result: Result<(), ApiError>) {
    #[cfg(windows)]
    {
        windows::finish_startup(app, result);
    }
    #[cfg(not(windows))]
    {
        use tauri::Manager;
        let handle = app.state::<super::AppState>().handle.clone();
        match result {
            Ok(()) => handle.complete_native_startup(),
            Err(error) => handle.fail_native_startup(error),
        }
        let owner = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if let Err(error) = super::build_manager_window(&owner) {
                eprintln!("Manager window: {}", error.code);
            }
        }) {
            eprintln!("Manager UI dispatch: {error}");
        }
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
    #[cfg(windows)]
    {
        windows::build_failure_manager(app)
    }
    #[cfg(not(windows))]
    super::build_manager_window(app)
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
