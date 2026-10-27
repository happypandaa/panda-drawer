//! Native-manager-only, explicit rollback of a managed program pointer.
use super::{require_manager, ApiError, AppState};
use platform_core::node_apps::RollbackNodeProgramInput;
use serde_json::Value;
use std::sync::atomic::Ordering;
use tauri::Manager;

fn require_closed(app: &tauri::AppHandle, state: &AppState, id: &str) -> Result<(), ApiError> {
    if state.shutting_down.load(Ordering::Acquire) {
        return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
    }
    state.handle.node_install_require_idle()?;
    if app.get_webview_window(&format!("node-app-{id}")).is_some() {
        return Err(ApiError::new(
            "NODE_PROGRAM_ROLLBACK_WINDOW_OPEN",
            "请先正常关闭应用窗口，等待服务停止后再退回程序。",
        ));
    }
    Ok(())
}

#[tauri::command]
pub(super) async fn node_program_rollback_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let _operation = state.node_windows.operations.lock().await;
            require_closed(&io_app, &state, &app_id)?;
            state
                .node_apps
                .rollback_preview(&app_id, state.handle.store())
                .await
        })
        .await?
}

#[tauri::command]
pub(super) async fn node_program_rollback_apply(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: RollbackNodeProgramInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let _operation = state.node_windows.operations.lock().await;
            require_closed(&io_app, &state, &app_id)?;
            state
                .node_apps
                .rollback_apply(&app_id, input, state.handle.store())
                .await
        })
        .await?
}
