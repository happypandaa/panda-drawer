//! Bounded Windows static host and manager-only, independently owned Node apps.

#[path = "windows_node.rs"]
mod windows_node;

use platform_host::{
    windows_projects::BundledProject,
    windows_runtime::{ProjectStatus, WindowsProjectRuntime},
    HostReport, APP_IDENTIFIER,
};
use platform_project_format::{ApiError, CreateProjectInput, ProjectInfo};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{webview::NewWindowResponse, Manager, Url};

struct WindowsDesktopState {
    runtime: Arc<WindowsProjectRuntime>,
    operation_gate: tokio::sync::Mutex<()>,
    closing: AtomicBool,
    exit_ready: AtomicBool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedBundle {
    schema_version: u32,
    files: Vec<(String, Vec<u8>)>,
}

fn bundled_project() -> Result<BundledProject, Box<dyn std::error::Error>> {
    let bundle: EmbeddedBundle = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/windows/embedded-project-resources.json"
    )))?;
    if bundle.schema_version != 1 {
        return Err("Unsupported embedded Windows project resource version".into());
    }
    Ok(BundledProject {
        files: bundle.files,
    })
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
async fn host_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WindowsDesktopState>,
) -> Result<HostReport, ApiError> {
    require_active_manager(&window, &state)?;
    let _diagnostic = state.operation_gate.lock().await;
    if state.runtime.status().await.root_status == "ready" {
        return Err(ApiError::new(
            "HOST_DIAGNOSTIC_BUSY",
            "项目工作区已由当前服务持有，请查看项目状态；本次没有另行打开数据根进行诊断。",
        ));
    }
    tauri::async_runtime::spawn_blocking(platform_host::inspect_host)
        .await
        .map_err(|_| ApiError::new("HOST_CHECK_FAILED", "只读检查未完成，请刷新后重试。"))
}

fn require_manager(window: &tauri::WebviewWindow) -> Result<(), ApiError> {
    if window.label() != "manager"
        || !window
            .url()
            .is_ok_and(|url| is_manager_url(&url, cfg!(debug_assertions)))
    {
        return Err(ApiError::new(
            "MANAGER_ONLY",
            "此操作仅允许本机管理窗口调用。",
        ));
    }
    Ok(())
}

fn require_active_manager(
    window: &tauri::WebviewWindow,
    state: &WindowsDesktopState,
) -> Result<(), ApiError> {
    require_manager(window)?;
    if state.closing.load(Ordering::Acquire) {
        return Err(
            ApiError::new("SERVICE_STOPPING", "平台正在等待当前操作结束后退出。")
                .outcome("not_started"),
        );
    }
    Ok(())
}

#[tauri::command]
async fn project_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WindowsDesktopState>,
) -> Result<ProjectStatus, ApiError> {
    require_active_manager(&window, &state)?;
    Ok(state.runtime.status().await)
}

#[tauri::command]
async fn project_create(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WindowsDesktopState>,
    input: CreateProjectInput,
) -> Result<ProjectInfo, ApiError> {
    require_active_manager(&window, &state)?;
    let _creation = state.operation_gate.lock().await;
    state.runtime.create(input).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenProjectResult {
    project_id: String,
    target: &'static str,
    reused: bool,
}

fn is_project_url(url: &Url, base: &Url, id: &str) -> bool {
    base.scheme() == "http"
        && base.host_str() == Some("127.0.0.1")
        && base.port().is_some()
        && url.origin() == base.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && (url.path() == "/_platform/connect" || url.path().starts_with(&format!("/p/{id}/")))
}

fn validated_launch_url(base: &str, launch: &str, id: &str) -> Result<Url, ApiError> {
    let base =
        Url::parse(base).map_err(|_| ApiError::new("INVALID_RUNTIME_URL", "本地服务地址无效。"))?;
    let launch = Url::parse(launch)
        .map_err(|_| ApiError::new("INVALID_LAUNCH_URL", "项目入口地址无效。"))?;
    if !is_project_url(&launch, &base, id) || launch.path() != "/_platform/connect" {
        return Err(ApiError::new(
            "INVALID_LAUNCH_URL",
            "项目入口不属于当前本地服务。",
        ));
    }
    Ok(launch)
}

#[tauri::command]
async fn project_open(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, WindowsDesktopState>,
    project_id: String,
) -> Result<OpenProjectResult, ApiError> {
    require_active_manager(&window, &state)?;
    let _creation = state.operation_gate.lock().await;
    let status = state.runtime.status().await;
    if status.root_status != "ready" {
        return Err(ApiError::new("WORKSPACE_UNAVAILABLE", status.message));
    }
    let project = status
        .projects
        .into_iter()
        .find(|project| project.id == project_id)
        .filter(|project| project.state == "ready")
        .ok_or_else(|| {
            ApiError::new(
                "PROJECT_NOT_READY",
                "项目当前无法打开，请刷新状态后查看原因。",
            )
        })?;
    let label = format!("project-{project_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        existing
            .show()
            .and_then(|_| existing.unminimize())
            .and_then(|_| existing.set_focus())
            .map_err(|error| ApiError::new("WINDOW_OPEN_FAILED", error.to_string()))?;
        return Ok(OpenProjectResult {
            project_id,
            target: "desktop",
            reused: true,
        });
    }
    let launch = state.runtime.launch(&project_id).await?;
    let launch_url =
        validated_launch_url(state.runtime.base_url(), &launch.launch_url, &project_id)?;
    let base = Url::parse(state.runtime.base_url())
        .map_err(|_| ApiError::new("INVALID_RUNTIME_URL", "本地服务地址无效。"))?;
    platform_host::check_webview_overrides()
        .map_err(|message| ApiError::new("WEBVIEW_POLICY_BLOCKED", message))?;
    let cache = platform_host::create_webview_cache()
        .map_err(|message| ApiError::new("WEBVIEW_CACHE_FAILED", message))?;
    let allowed_id = project_id.clone();
    tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::External(launch_url))
        .title(format!("{} · Windows 只读样本", project.name))
        .inner_size(1080.0, 780.0)
        .min_inner_size(640.0, 480.0)
        .data_directory(cache.path().to_path_buf())
        .incognito(true)
        .disable_drag_drop_handler()
        .on_navigation(move |url| is_project_url(url, &base, &allowed_id))
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny)
        .build()
        .map_err(|_| ApiError::new("WINDOW_OPEN_FAILED", "无法创建项目窗口，请重新打开项目。"))?;
    Ok(OpenProjectResult {
        project_id,
        target: "desktop",
        reused: false,
    })
}

#[tauri::command]
async fn project_open_browser(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, WindowsDesktopState>,
    project_id: String,
) -> Result<OpenProjectResult, ApiError> {
    require_active_manager(&window, &state)?;
    let launch = state.runtime.launch(&project_id).await?;
    let url = validated_launch_url(state.runtime.base_url(), &launch.launch_url, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || platform_host::open_project_browser(url.as_str()))
        .await
        .map_err(|_| ApiError::new("BROWSER_OPEN_FAILED", "浏览器打开任务未完成。"))??;
    Ok(OpenProjectResult {
        project_id,
        target: "browser",
        reused: false,
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
    // The bounded Windows runtime opens only this stage's new-project path.
    // Deferred recovery, lifecycle, build and cleanup managers remain absent.
    let runtime = tauri::async_runtime::block_on(WindowsProjectRuntime::start(bundled_project()?))?;
    app.manage(WindowsDesktopState {
        runtime,
        operation_gate: tokio::sync::Mutex::new(()),
        closing: AtomicBool::new(false),
        exit_ready: AtomicBool::new(false),
    });
    // WebView2 still requires a cache even in private mode. Use the
    // dedicated disposable cache, never the protected business root.
    let webview_cache = platform_host::create_webview_cache().map_err(std::io::Error::other)?;
    tauri::WebviewWindowBuilder::new(
        app,
        "manager",
        tauri::WebviewUrl::App("windows.html".into()),
    )
    .title("熊猫应用小抽屉 · Windows 项目内部版")
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_manager(app)
        }))
        .invoke_handler(tauri::generate_handler![
            host_status,
            project_status,
            project_create,
            project_open,
            project_open_browser,
            windows_node::node_app_choose_source,
            windows_node::node_app_inspect,
            windows_node::node_app_register,
            windows_node::node_app_list,
            windows_node::node_app_configuration,
            windows_node::node_app_configuration_save,
            windows_node::node_app_start,
            windows_node::node_app_stop,
            windows_node::node_app_restart,
            windows_node::node_app_status,
            windows_node::node_app_open,
            windows_node::node_app_open_help
        ])
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
            windows_node::window_event(window, event);
            if window.label() == "manager" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    // Retain the window while the unified exit path drains a
                    // pending creation; closing the form is not cancellation.
                    api.prevent_close();
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!());
    match result {
        Ok(app) => app.run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if let Some(state) = app.try_state::<WindowsDesktopState>() {
                    if !state.exit_ready.load(Ordering::Acquire) {
                        api.prevent_exit();
                        if !state.closing.swap(true, Ordering::AcqRel) {
                            let runtime = state.runtime.clone();
                            let app = app.clone();
                            tauri::async_runtime::spawn(async move {
                                let state = app.state::<WindowsDesktopState>();
                                let _operation = state.operation_gate.lock().await;
                                if let Err(error) = windows_node::shutdown(&app).await {
                                    state.closing.store(false, Ordering::Release);
                                    show_manager(&app);
                                    use tauri_plugin_dialog::DialogExt;
                                    app.dialog()
                                        .message(format!(
                                            "退出尚未完成：{}。请核对运行状态后重试退出。",
                                            error.message
                                        ))
                                        .title("应用退出状态")
                                        .show(|_| {});
                                    return;
                                }
                                runtime.shutdown().await;
                                if let Some(state) = app.try_state::<WindowsDesktopState>() {
                                    state.exit_ready.store(true, Ordering::Release);
                                }
                                app.exit(0);
                            });
                        }
                    }
                }
            }
            tauri::RunEvent::Exit => {
                if let Some(state) = app.try_state::<WindowsDesktopState>() {
                    state.runtime.stop();
                }
            }
            _ => {}
        }),
        Err(error) => {
            platform_host::show_startup_error(&error.to_string());
            std::process::exit(1);
        }
    };
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

    #[test]
    fn project_navigation_stays_within_the_current_loopback_origin_and_project() {
        let base = Url::parse("http://127.0.0.1:49152").unwrap();
        for value in [
            "http://127.0.0.1:49152/_platform/connect#ticket=synthetic",
            "http://127.0.0.1:49152/p/prj_fixture/",
            "http://127.0.0.1:49152/p/prj_fixture/_builds/starter_fixture/assets/app.js",
        ] {
            assert!(
                is_project_url(&Url::parse(value).unwrap(), &base, "prj_fixture"),
                "{value}"
            );
        }
        for value in [
            "http://127.0.0.1:49153/p/prj_fixture/",
            "http://localhost:49152/p/prj_fixture/",
            "http://127.0.0.1:49152/p/prj_other/",
            "http://127.0.0.1:49152/p/prj_fixture_suffix/",
            "http://127.0.0.1:49152/p/prj_fixture/../../api/v1/",
            "http://user@127.0.0.1:49152/p/prj_fixture/",
            "http://tauri.localhost/windows.html",
            "https://example.com/",
            "file:///C:/fixture/index.html",
        ] {
            assert!(
                !is_project_url(&Url::parse(value).unwrap(), &base, "prj_fixture"),
                "{value}"
            );
        }
    }

    #[test]
    fn launch_opening_accepts_only_the_service_connection_entry() {
        let base = "http://127.0.0.1:49152";
        assert!(validated_launch_url(
            base,
            "http://127.0.0.1:49152/_platform/connect#ticket=synthetic",
            "prj_fixture"
        )
        .is_ok());
        assert!(
            validated_launch_url(base, "http://127.0.0.1:49152/p/prj_fixture/", "prj_fixture")
                .is_err()
        );
        assert!(validated_launch_url(
            base,
            "http://127.0.0.1:49153/_platform/connect",
            "prj_fixture"
        )
        .is_err());
    }
}
