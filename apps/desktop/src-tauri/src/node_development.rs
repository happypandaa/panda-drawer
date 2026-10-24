//! Only the native manager can edit developer sources or dispatch an editor.
use super::{codex_handoff, local_directory, require_manager, ApiError, AppState};
use platform_core::NodeGitAction;
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Manager;

#[tauri::command]
pub(super) async fn node_development_info(
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
            state
                .handle
                .node_development_info(&state.node_apps, &app_id)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_bind(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let _operation = state.node_windows.operations.lock().await;
            state.handle.node_install_require_idle()?;
            state
                .node_apps
                .bind_development_source(&app_id, PathBuf::from(source_path))
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_reveal(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<(), ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let _operation = state.node_windows.operations.lock().await;
            let lease = state
                .handle
                .node_development_lease(&state.node_apps, &app_id, false)
                .await?;
            lease.validate()?;
            local_directory::open(&lease.source.source_root, false).await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_git(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    operation: String,
    input: Option<Value>,
    limit: Option<u32>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let _operation = state.node_windows.operations.lock().await;
            let value = input.unwrap_or(Value::Null);
            let parse =
                |error: serde_json::Error| ApiError::new("INVALID_INPUT", error.to_string());
            let action = match operation.as_str() {
                "status" => NodeGitAction::Status,
                "versions" => NodeGitAction::Versions(limit.unwrap_or(50)),
                "initialize" => {
                    NodeGitAction::Initialize(serde_json::from_value(value).map_err(parse)?)
                }
                "save" => NodeGitAction::Save(serde_json::from_value(value).map_err(parse)?),
                "branch" => NodeGitAction::Branch(serde_json::from_value(value).map_err(parse)?),
                "restore-preview" => {
                    NodeGitAction::RestorePreview(serde_json::from_value(value).map_err(parse)?)
                }
                "restore-apply" => {
                    NodeGitAction::RestoreApply(serde_json::from_value(value).map_err(parse)?)
                }
                "restore-status" => NodeGitAction::RestoreStatus,
                "restore-recover" => {
                    NodeGitAction::RestoreRecover(serde_json::from_value(value).map_err(parse)?)
                }
                _ => return Err(ApiError::new("INVALID_INPUT", "未知源码版本操作。")),
            };
            state
                .handle
                .node_development_git(&state.node_apps, &app_id, action)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_handoff(
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
            state
                .handle
                .node_development_handoff(&state.node_apps, &app_id)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_open_codex(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    expected_project_root: String,
    expected_instance_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let _operation = state.node_windows.operations.lock().await;
            let lease = state
                .handle
                .node_development_lease(&state.node_apps, &app_id, true)
                .await?;
            if lease.instance_id != expected_instance_id
                || lease.source.source_root != std::path::Path::new(&expected_project_root)
            {
                return Err(ApiError::new(
                    "HANDOFF_CHANGED",
                    "项目位置或平台已变化，请重新准备说明。",
                ));
            }
            let app = codex_handoff::installed_app().await?;
            lease.validate()?;
            codex_handoff::open(&app, &lease.source.source_root).await?;
            Ok(json!({"projectRoot":lease.source.source_root,"requested":true}))
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_build(
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
            state
                .handle
                .node_development_build(state.node_apps.clone(), app_id)
                .await
        })
        .await?
}
#[tauri::command]
pub(super) async fn node_development_cancel(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.node_development_cancel(&task_id)
        })
        .await?
}
