//! Read-only handoff for the directly running project folder.
use super::{require_manager, ApiError, AppState};
use serde_json::Value;
use tauri::Manager;

#[tauri::command]
pub(super) async fn node_app_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let state = app.state::<AppState>();
            let _operation = state.node_windows.instance_operation(&app_id).await;
            state
                .handle
                .node_app_handoff(&state.node_apps, &app_id)
                .await
        })
        .await?
}
