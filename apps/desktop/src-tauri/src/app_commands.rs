//! Manager-only command adapters for the single application model.
use super::{require_manager, ApiError, AppState};
use serde_json::{json, Value};
use tauri::Manager;

#[tauri::command]
pub(super) async fn app_list(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    super::project_list(window, state).await
}
#[tauri::command]
pub(super) async fn app_create(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: platform_core::CreateProjectInput,
) -> Result<Value, ApiError> {
    super::project_create(window, state, input).await
}
#[tauri::command]
pub(super) async fn app_open(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let app = window.app_handle().clone();
    if app
        .get_webview_window(&super::project_label(&app_id))
        .is_some()
    {
        return Ok(json!(super::open_app_window(app, app_id).await?));
    }
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move { handle.reload_app(&app_id).await })
        .await?
}
#[tauri::command]
pub(super) async fn app_reload(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    expected_window_generation: Option<u64>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let app = window.app_handle().clone();
    let admission = if let Some(generation) = expected_window_generation {
        super::app_reload::validate_window_action(&app, &app_id, generation)?;
        Some(
            state
                .window_lifetimes
                .admit_project("app", &app_id)
                .map_err(|_| ApiError::new("APP_WINDOW_ACTION_EXPIRED", "原应用窗口已关闭。"))?,
        )
    } else {
        None
    };
    let _native_operation = if let Some(admission) = &admission {
        Some(
            admission
                .operation()
                .await
                .map_err(|_| ApiError::new("APP_WINDOW_ACTION_EXPIRED", "原应用窗口已关闭。"))?,
        )
    } else {
        None
    };
    if let Some(generation) = expected_window_generation {
        super::app_reload::validate_window_action(&app, &app_id, generation)?;
    }
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move { handle.reload_app(&app_id).await })
        .await?
}
#[tauri::command]
pub(super) async fn app_configuration(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    super::web_app_configuration(window, state, app_id).await
}
#[tauri::command]
pub(super) async fn app_configuration_save(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    super::web_app_configuration_save(window, state, app_id, input).await
}
#[tauri::command]
pub(super) async fn app_close(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    super::web_app_close(window, state, app_id).await
}
#[tauri::command]
pub(super) async fn app_open_help(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    field_name: String,
) -> Result<(), ApiError> {
    super::web_app_open_help(window, state, app_id, field_name).await
}
#[tauri::command]
pub(super) async fn app_build(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: Option<Value>,
) -> Result<Value, ApiError> {
    super::project_build(window, state, app_id, input).await
}
#[tauri::command]
pub(super) async fn app_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    purpose: Option<String>,
) -> Result<Value, ApiError> {
    super::project_handoff(window, state, app_id, purpose).await
}
#[tauri::command]
pub(super) async fn app_delete_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    super::project_delete_preview(window, state, app_id).await
}
#[tauri::command]
pub(super) async fn app_delete(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    super::project_delete(window, state, app_id, input).await
}
#[tauri::command]
pub(super) async fn app_logs(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease.run(async move { handle.app_logs(&app_id) }).await?
}
#[tauri::command]
pub(super) async fn app_reserve(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    name: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move { handle.reserve_app(name).await })
        .await?
}
#[tauri::command]
pub(super) async fn app_directory_inspect(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move { handle.check_directory(source_path).await })
        .await?
}
#[tauri::command]
pub(super) async fn app_directory_register(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
    expected_revision: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move {
            handle
                .register_directory(source_path, expected_revision)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn app_directory_initialize_and_register(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move { handle.initialize_directory(source_path, input).await })
        .await?
}
#[tauri::command]
pub(super) async fn management_request_resolve(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    request_id: String,
    approved: bool,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let _lease = state.handle.io_lifetime().admit()?;
    state
        .handle
        .resolve_management_request(&request_id, approved)
}
