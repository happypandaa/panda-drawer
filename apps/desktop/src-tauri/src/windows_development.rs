//! Native-manager-only development tools and source candidates.
use super::{require_active_manager, WindowsDesktopState};
use platform_node::{development_tools::DevelopmentTools, ApiError};
use serde_json::Value;
use std::path::PathBuf;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

pub(super) fn tools(app: &tauri::AppHandle) -> Result<DevelopmentTools, ApiError> {
    Ok(DevelopmentTools::new(
        app.path()
            .resource_dir()
            .map_err(|_| ApiError::new("DEVELOPMENT_TOOLS_UNAVAILABLE", "无法定位开发工具资源。"))?
            .join("resources"),
    ))
}
#[tauri::command]
pub(super) async fn development_environment_doctor(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, WindowsDesktopState>,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &state)?;
    let _operation = state.operation_gate.lock().await;
    require_active_manager(&window, &state)?;
    let root = state.runtime.node_app_root().await?;
    Ok(tools(&app)?.doctor(&root).await)
}
#[tauri::command]
pub(super) async fn development_environment_configure(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, WindowsDesktopState>,
    kind: String,
    path: Option<PathBuf>,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &state)?;
    let _operation = state.operation_gate.lock().await;
    require_active_manager(&window, &state)?;
    let root = state.runtime.node_app_root().await?;
    tools(&app)?.configure(&root, &kind, path).await
}
#[tauri::command]
pub(super) async fn development_choose_tool(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, WindowsDesktopState>,
    kind: String,
) -> Result<Option<PathBuf>, ApiError> {
    require_active_manager(&window, &state)?;
    if !matches!(kind.as_str(), "node" | "git") {
        return Err(ApiError::new("INVALID_TOOL_KIND", "请选择Node或Git。"));
    }
    let (send, receive) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(if kind == "node" {
            "选择开发Node的node.exe"
        } else {
            "选择Git for Windows的cmd/git.exe"
        })
        .add_filter("Windows executable", &["exe"])
        .pick_file(move |value| {
            let _ = send.send(
                value
                    .map(|p| p.into_path())
                    .transpose()
                    .map_err(|_| ApiError::new("INVALID_TOOL_PATH", "请选择本机exe文件。")),
            );
        });
    let selection = receive
        .await
        .map_err(|_| ApiError::new("TOOL_PICKER_CANCELLED", "工具选择未完成。"))??;
    require_active_manager(&window, &state)?;
    Ok(selection)
}
