//! Native-manager-only development tools and source candidates.
use super::{require_active_manager, WindowsDesktopState};
use platform_node::{
    development_tasks::DevelopmentTasks,
    development_tools::DevelopmentTools,
    windows_source_builds::{BuildInput, SourceBuilds},
    windows_sources::{AcquireInput, CaptureInput, SourceManager},
    ApiError,
};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
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

struct DevelopmentState {
    sources: Arc<SourceManager>,
    tasks: Arc<DevelopmentTasks>,
    builds: Arc<SourceBuilds>,
}

fn work_failed() -> ApiError {
    ApiError::new(
        "DEVELOPMENT_OPERATION_INCOMPLETE",
        "开发操作回执未完成。请重新读取任务和保留现场，勿重复提交。",
    )
    .outcome("unknown")
}

// The caller owns operation_gate. Revalidate the admitted, fixed root even when
// a manager already exists; no command creates or migrates the workspace.
async fn ensure_development(
    app: &tauri::AppHandle,
    host: &WindowsDesktopState,
) -> Result<(), ApiError> {
    let root = host.runtime.node_app_root().await?;
    if app.try_state::<DevelopmentState>().is_some() {
        return Ok(());
    }
    let tools = tools(app)?;
    let development = tauri::async_runtime::spawn_blocking(move || {
        let tasks = DevelopmentTasks::new(root.clone())?;
        let sources = SourceManager::new(root.clone(), tools.clone(), tasks.clone())?;
        let builds = SourceBuilds::new(root, tools, sources.clone(), tasks.clone());
        Ok::<_, ApiError>(DevelopmentState {
            sources,
            tasks,
            builds,
        })
    })
    .await
    .map_err(|_| work_failed())??;
    app.manage(development);
    Ok(())
}

#[tauri::command]
pub(super) async fn source_choose_directory(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
) -> Result<Option<PathBuf>, ApiError> {
    require_active_manager(&window, &host)?;
    {
        let _operation = host.operation_gate.lock().await;
        require_active_manager(&window, &host)?;
        ensure_development(&app, &host).await?;
    }
    // A picker is not an accepted source task. It must not block exit while the
    // user considers a directory, and a result after closing has no authority.
    let (send, receive) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择要检查的独立源码目录")
        .pick_folder(move |selection| {
            let _ = send.send(
                selection
                    .map(|path| path.into_path())
                    .transpose()
                    .map_err(|_| ApiError::new("INVALID_SOURCE_PATH", "请选择本机普通源码目录。")),
            );
        });
    let selection = receive
        .await
        .map_err(|_| ApiError::new("SOURCE_PICKER_FAILED", "源码目录选择未完成。"))??;
    require_active_manager(&window, &host)?;
    Ok(selection)
}

#[tauri::command]
pub(super) async fn source_list(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let sources = app.state::<DevelopmentState>().sources.clone();
    tauri::async_runtime::spawn_blocking(move || sources.list())
        .await
        .map_err(|_| work_failed())?
}

#[tauri::command]
pub(super) async fn source_inspect(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    source_path: PathBuf,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let sources = app.state::<DevelopmentState>().sources.clone();
    tauri::async_runtime::spawn_blocking(move || sources.inspect_local(&source_path))
        .await
        .map_err(|_| work_failed())?
}

#[tauri::command]
pub(super) async fn source_capture(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    input: CaptureInput,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let sources = app.state::<DevelopmentState>().sources.clone();
    tauri::async_runtime::spawn_blocking(move || sources.capture(input))
        .await
        .map_err(|_| work_failed())?
}

#[tauri::command]
pub(super) async fn source_acquire(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    input: AcquireInput,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let sources = app.state::<DevelopmentState>().sources.clone();
    tauri::async_runtime::spawn_blocking(move || sources.acquire(input))
        .await
        .map_err(|_| work_failed())?
}

#[tauri::command]
pub(super) async fn source_cancel(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let tasks = app.state::<DevelopmentState>().tasks.clone();
    tasks.cancel(&task_id)
}

#[tauri::command]
pub(super) async fn task_get(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    app.state::<DevelopmentState>().tasks.get(&task_id)
}

#[tauri::command]
pub(super) async fn task_logs(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    task_id: String,
    cursor: u64,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let tasks = app.state::<DevelopmentState>().tasks.clone();
    tauri::async_runtime::spawn_blocking(move || tasks.logs(&task_id, cursor))
        .await
        .map_err(|_| work_failed())?
}

#[tauri::command]
pub(super) async fn source_build_prepare(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
    input: BuildInput,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let builds = app.state::<DevelopmentState>().builds.clone();
    builds.prepare(input).await
}

#[tauri::command]
pub(super) async fn source_build_list(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    host: tauri::State<'_, WindowsDesktopState>,
) -> Result<Value, ApiError> {
    require_active_manager(&window, &host)?;
    let _operation = host.operation_gate.lock().await;
    require_active_manager(&window, &host)?;
    ensure_development(&app, &host).await?;
    let builds = app.state::<DevelopmentState>().builds.clone();
    tauri::async_runtime::spawn_blocking(move || builds.list())
        .await
        .map_err(|_| work_failed())?
}

// Called after closing is set and while operation_gate excludes any admission.
// Join the worker owners before waiting for the durable task set to drain, then
// the host may safely stop Node applications and the shared HTTP runtime.
pub(super) async fn shutdown(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<DevelopmentState>() {
        tokio::join!(state.sources.shutdown(), state.builds.shutdown());
        state.tasks.shutdown().await;
    }
    platform_host::windows_command::wait_for_idle().await;
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
