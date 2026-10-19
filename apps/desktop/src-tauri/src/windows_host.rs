//! Windows W1 desktop host. No project runtime or business manager is linked.

use platform_host::{HostReport, APP_IDENTIFIER};
use serde::Serialize;
use tauri::{webview::NewWindowResponse, Manager, Url};

#[derive(Serialize)]
struct HostError {
    code: &'static str,
    message: &'static str,
}

fn is_manager_url(url: &Url, development: bool) -> bool {
    let bundled =
        url.scheme() == "http" && url.host_str() == Some("tauri.localhost") && url.port().is_none();
    let developing = development
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420);
    url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/windows.html"
        && (bundled || developing)
}

#[tauri::command]
async fn host_status(window: tauri::WebviewWindow) -> Result<HostReport, HostError> {
    if window.label() != "manager"
        || !window
            .url()
            .is_ok_and(|url| is_manager_url(&url, cfg!(debug_assertions)))
    {
        return Err(HostError {
            code: "MANAGER_ONLY",
            message: "此检查仅允许本机管理窗口调用。",
        });
    }
    tauri::async_runtime::spawn_blocking(platform_host::inspect_host)
        .await
        .map_err(|_| HostError {
            code: "HOST_CHECK_FAILED",
            message: "只读检查未完成，请刷新后重试。",
        })
}

fn show_manager(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("manager") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_manager(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    if app.config().identifier != APP_IDENTIFIER {
        return Err("Desktop identifier and Windows host identity do not match".into());
    }
    // This host does not initialize RuntimePaths, acquire a workspace
    // lock, start a server, or construct a deferred business manager.
    // WebView2 still requires a cache even in private mode. Use the
    // dedicated disposable cache, never the protected business root.
    let webview_cache = platform_host::create_webview_cache().map_err(std::io::Error::other)?;
    tauri::WebviewWindowBuilder::new(
        app,
        "manager",
        tauri::WebviewUrl::App("windows.html".into()),
    )
    .title("本地项目 · Windows 只读内部版")
    .inner_size(1160.0, 800.0)
    .min_inner_size(880.0, 640.0)
    .data_directory(webview_cache.path().to_path_buf())
    .incognito(true)
    .disable_drag_drop_handler()
    .on_navigation(|url| is_manager_url(url, cfg!(debug_assertions)))
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .on_download(|_, _| false)
    .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny)
    .build()?;
    // Cache allocation is separate from business-file safety. Chromium
    // must be able to rename its own files; no workspace lock is held.
    Ok(())
}

pub fn run() {
    if let Err(error) = platform_host::check_webview_overrides() {
        platform_host::show_startup_error(&error);
        std::process::exit(1);
    }
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_manager(app)
        }))
        .invoke_handler(tauri::generate_handler![host_status])
        .setup(|app| match setup_manager(app) {
            Ok(()) => Ok(()),
            Err(error) => {
                // Tauri turns an error returned by setup into a panic. Show
                // the native failure here before handing control back to it.
                platform_host::show_startup_error(&error.to_string());
                std::process::exit(1);
            }
        })
        .on_window_event(|window, event| {
            if window.label() == "manager"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!());
    if let Err(error) = result {
        platform_host::show_startup_error(&error.to_string());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_fixed_windows_management_page_can_call_the_host() {
        assert!(is_manager_url(
            &Url::parse("http://tauri.localhost/windows.html").unwrap(),
            false
        ));
        assert!(is_manager_url(
            &Url::parse("http://127.0.0.1:1420/windows.html").unwrap(),
            true
        ));
        assert!(!is_manager_url(
            &Url::parse("http://127.0.0.1:1420/windows.html").unwrap(),
            false
        ));
        for value in [
            "https://tauri.localhost/windows.html",
            "http://tauri.localhost:1420/windows.html",
            "http://tauri.localhost.evil.test/windows.html",
            "http://user@tauri.localhost/windows.html",
            "http://localhost:1420/windows.html",
            "http://127.0.0.1:49152/windows.html",
            "http://127.0.0.1:1420/index.html",
            "http://tauri.localhost/index.html",
            "tauri://localhost/windows.html",
            "file:///windows.html",
        ] {
            assert!(
                !is_manager_url(&Url::parse(value).unwrap(), true),
                "{value}"
            );
        }
    }
}
