use super::{require_manager, ApiError, AppState};
use serde_json::{json, Value};
use tauri::Manager;

#[tauri::command]
pub(super) async fn source_import_location(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let _lease = state.handle.io_lifetime().admit()?;
    Ok(json!({"projectDirectory":state.handle.info().workspace_root.join("project-sources")}))
}

#[tauri::command]
pub(super) async fn source_import_recover(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let state = app.state::<AppState>();
            // Recovery coordinates shared source transactions. Runtime stops
            // use instance locks and never wait for this shared mutex.
            let _operation = state.node_windows.operations.lock().await;
            state
                .handle
                .recover_source_imports(state.node_apps.clone())
                .await
        })
        .await?
}

#[tauri::command]
pub(super) async fn source_import_publish(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_task_id: String,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let state = app.state::<AppState>();
            let _admission = state.node_apps.admit_operation(&app_id, "发布源码")?;
            let _operation = state.node_windows.instance_operation(&app_id).await;
            state
                .handle
                .source_import_publish(state.node_apps.clone(), source_task_id, app_id)
                .await
        })
        .await?
}

#[tauri::command]
pub(super) async fn source_import_build_info(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            app.state::<AppState>()
                .handle
                .source_import_build_info(&source_task_id)
                .await
        })
        .await?
}
