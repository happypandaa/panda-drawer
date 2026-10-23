//! Node commands share the native manager boundary and the admitted Windows root.
use super::{require_active_manager, show_manager, WindowsDesktopState};
use platform_node::{
    node_app_configuration::SaveConfigurationRequest, node_apps::NodeApps, ApiError,
};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[path = "node_windows.rs"]
mod node_windows;
#[path = "window_sizes.rs"]
mod window_sizes;

struct AppState {
    node_apps: Arc<NodeApps>,
    node_windows: node_windows::NodeWindows,
    window_sizes: window_sizes::WindowSizes,
    shutting_down: AtomicBool,
}
fn project_title(name: &str) -> String {
    format!("{name} · 熊猫应用小抽屉")
}

// Called only while the desktop operation gate is held. Merely listing Node
// applications never creates a workspace, registration, config or credential.
async fn ensure_nodes(app: &tauri::AppHandle, host: &WindowsDesktopState) -> Result<(), ApiError> {
    let root = host.runtime.node_app_root().await?;
    if app.try_state::<AppState>().is_some() {
        return Ok(());
    }
    let runtime = app
        .path()
        .resource_dir()
        .map_err(|_| {
            ApiError::new(
                "NODE_RUNTIME_UNAVAILABLE",
                "无法定位内置 Node，请重新准备 Windows 资源。",
            )
        })?
        .join("resources/node-runtime/node.exe");
    let sizes = window_sizes::WindowSizes::new(root.path());
    let node_apps = Arc::new(NodeApps::new_windows(root, runtime)?);
    app.manage(AppState {
        node_apps,
        node_windows: node_windows::NodeWindows::default(),
        window_sizes: sizes,
        shutting_down: AtomicBool::new(false),
    });
    let monitor = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            let Some(state) = monitor.try_state::<AppState>() else {
                break;
            };
            if state.shutting_down.load(Ordering::Acquire) {
                break;
            }
            state.node_apps.monitor_tick().await;
            node_windows::reconcile(&monitor).await;
        }
    });
    Ok(())
}

#[tauri::command]
pub(super) async fn node_app_choose_source(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
) -> Result<Option<String>, ApiError> {
    require_active_manager(&window, &host)?;
    {
        let _operation = host.operation_gate.lock().await;
        require_active_manager(&window, &host)?;
        ensure_nodes(&app, &host).await?;
    }
    // A native picker may remain open indefinitely. It does not mutate the
    // workspace and must not hold the gate needed for platform shutdown.
    let (send, recv) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择独立的 Node 应用源码目录")
        .pick_folder(move |selection| {
            let _ = send.send(
                selection
                    .map(|selection| {
                        selection
                            .into_path()
                            .map(|path| path.to_string_lossy().into_owned())
                            .map_err(|_| {
                                ApiError::new("INVALID_SOURCE_PATH", "请选择本机普通目录。")
                            })
                    })
                    .transpose(),
            );
        });
    let selection = recv
        .await
        .map_err(|_| ApiError::new("SOURCE_PICKER_FAILED", "目录选择未完成，请重试。"))??;
    require_active_manager(&window, &host)?;
    Ok(selection)
}

#[tauri::command]
pub(super) async fn node_app_inspect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    app.state::<AppState>()
        .node_apps
        .inspect(&PathBuf::from(source_path))
}

#[tauri::command]
pub(super) async fn node_app_register(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    source_path: String,
    expected_revision: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    app.state::<AppState>()
        .node_apps
        .register(PathBuf::from(source_path), &expected_revision)
        .await
}

#[tauri::command]
pub(super) async fn node_app_list(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
) -> Result<Vec<Value>, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    app.state::<AppState>().node_apps.list().await
}

#[tauri::command]
pub(super) async fn node_app_configuration_save(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
    input: SaveConfigurationRequest,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    app.state::<AppState>().node_apps.save(&app_id, input).await
}

#[tauri::command]
pub(super) async fn node_app_configuration(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    app.state::<AppState>()
        .node_apps
        .configuration(&app_id)
        .await
}

#[tauri::command]
pub(super) async fn node_app_status(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    app.state::<AppState>().node_apps.status(&app_id).await
}

#[tauri::command]
pub(super) async fn node_app_start(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    node_windows::open(&app, &app.state::<AppState>(), &app_id).await
}

#[tauri::command]
pub(super) async fn node_app_stop(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    node_windows::close(&app, &app.state::<AppState>(), &app_id).await
}

#[tauri::command]
pub(super) async fn node_app_restart(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    node_windows::restart(&app, &app.state::<AppState>(), &app_id).await
}

#[tauri::command]
pub(super) async fn node_app_open(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
) -> Result<(), ApiError> {
    node_app_start(window, app, host, app_id).await.map(|_| ())
}

fn validated_help_url(value: &str) -> Result<tauri::Url, ApiError> {
    let url = tauri::Url::parse(value)
        .map_err(|_| ApiError::new("INVALID_HELP_URL", "帮助链接无效。"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::new(
            "INVALID_HELP_URL",
            "帮助链接必须为不含凭据的 HTTPS 地址。",
        ));
    }
    Ok(url)
}
fn open_help(value: &str) -> Result<(), ApiError> {
    let url = validated_help_url(value)?;
    let action: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = url.as_str().encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            action.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if result as isize <= 32 {
        return Err(ApiError::new(
            "BROWSER_OPEN_FAILED",
            "无法打开帮助链接，请检查默认浏览器设置。",
        ));
    }
    Ok(())
}

#[tauri::command]
pub(super) async fn node_app_open_help(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    app_id: String,
    field_name: String,
) -> Result<(), ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_nodes(&app, &host).await?;
    let url = app
        .state::<AppState>()
        .node_apps
        .help_url(&app_id, &field_name)
        .await?;
    tauri::async_runtime::spawn_blocking(move || open_help(&url))
        .await
        .map_err(|_| ApiError::new("BROWSER_OPEN_FAILED", "帮助链接打开任务未完成。"))?
}

pub(super) fn window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let Some(state) = window.app_handle().try_state::<AppState>() {
        state.window_sizes.on_event(window, event);
    }
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        if node_windows::close_requested(window) {
            api.prevent_close();
        }
    }
    if matches!(event, tauri::WindowEvent::Destroyed) {
        node_windows::destroyed(window);
    }
}
pub(super) async fn shutdown(app: &tauri::AppHandle) -> Result<(), ApiError> {
    if let Some(state) = app.try_state::<AppState>() {
        node_windows::shutdown(app, &state).await?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn help_opener_never_accepts_shell_targets_or_embedded_credentials() {
        assert!(validated_help_url("https://example.test/help?a=b#section").is_ok());
        for value in [
            "file:///C:/sample.exe",
            "cmd.exe /c something",
            "http://example.test/",
            "https://user:secret@example.test/",
            "https://example.test/\n",
        ] {
            assert!(validated_help_url(value).is_err(), "{value}");
        }
    }
}
