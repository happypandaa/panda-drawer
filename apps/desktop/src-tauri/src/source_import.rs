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
pub(super) async fn node_git_import_start(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: platform_core::node_git_import::StartNodeGitImportInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let state = app.state::<AppState>();
            state
                .handle
                .node_git_import_start(state.node_apps.clone(), input)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_git_import_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    import_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let _lease = state.handle.io_lifetime().admit()?;
    state.handle.node_git_import_status(&import_id)
}
#[tauri::command]
pub(super) async fn node_git_import_cancel(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let _lease = state.handle.io_lifetime().admit_cleanup()?;
    state.handle.node_git_import_cancel(&task_id)
}
#[tauri::command]
pub(super) async fn node_git_import_retry_cleanup(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit_cleanup()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let state = app.state::<AppState>();
            state
                .handle
                .node_git_import_retry_cleanup(state.node_apps.clone(), &task_id)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn source_import_recover(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit_cleanup()?;
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let state = app.state::<AppState>();
            state
                .handle
                .recover_source_imports(state.node_apps.clone())
                .await
        })
        .await?
}
