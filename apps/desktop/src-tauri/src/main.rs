#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browser_download;
mod business_window_lifetime;
mod codex_handoff;
mod desktop_monitor;
mod desktop_partial_startup;
mod desktop_shutdown;
mod desktop_shutdown_native;
mod engine_downloads;
mod local_data_cleanup;
mod local_directory;
mod native_process;
mod native_stores;
mod node_development;
mod node_windows;
mod project_onboarding;
mod project_window_cleanup;
mod source_import;
mod window_sizes;

use platform_core::{
    ApiError, BootstrapInfo, CreateProjectInput, RuntimeHandle, RuntimePaths, RuntimeService,
    DISPLAY_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    webview::NewWindowResponse,
    Emitter, Manager, Url,
};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Serialize)]
struct ResourceInfo {
    sdk: bool,
    template: bool,
    readme: bool,
    cli: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopInfo {
    #[serde(flatten)]
    runtime: BootstrapInfo,
    version: String,
    resources: ResourceInfo,
    cli_path: Option<String>,
    platform: Value,
}

struct StartupFailure {
    message: String,
    // Runtime startup can succeed before NodeApps initialization fails.
    runtime: Option<std::sync::Arc<RuntimeService>>,
    shutdown: std::sync::Arc<desktop_shutdown::DesktopShutdown>,
    exit_ready: std::sync::atomic::AtomicBool,
}

struct AppState {
    info: DesktopInfo,
    handle: RuntimeHandle,
    runtime: std::sync::Arc<RuntimeService>,
    desktop_shutdown: std::sync::Arc<desktop_shutdown::DesktopShutdown>,
    local_data_cleanup: local_data_cleanup::LocalDataCleanup,
    window_lifetimes: business_window_lifetime::WindowLifetimes,
    engine_downloads: std::sync::Arc<engine_downloads::EngineDownloads>,
    project_windows: std::sync::Arc<tokio::sync::Mutex<()>>,
    node_apps: std::sync::Arc<platform_core::node_apps::NodeApps>,
    node_monitor: desktop_monitor::OwnedMonitor,
    node_windows: node_windows::NodeWindows,
    window_sizes: window_sizes::WindowSizes,
    shutting_down: std::sync::atomic::AtomicBool,
    exit_ready: std::sync::atomic::AtomicBool,
}

fn is_manager_url(url: &Url, development: bool) -> bool {
    let bundled = url.scheme() == "tauri" && url.host_str() == Some("localhost");
    let developing = development
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420);
    url.username().is_empty() && url.password().is_none() && (bundled || developing)
}

fn require_manager(window: &tauri::WebviewWindow) -> Result<(), ApiError> {
    let url = window
        .url()
        .map_err(|_| ApiError::new("MANAGER_ONLY", "无法验证管理窗口身份"))?;
    // Custom commands are checked here in addition to capabilities. No project
    // webview, including one that navigates to a bundled URL, has native access.
    if window.label() != "manager" || !is_manager_url(&url, cfg!(debug_assertions)) {
        return Err(ApiError::new(
            "MANAGER_ONLY",
            "此操作仅允许本机平台管理窗口调用",
        ));
    }
    if let Some(state) = window.app_handle().try_state::<AppState>() {
        state.handle.check_native_ready()?;
    }
    Ok(())
}

#[tauri::command]
fn bootstrap_info(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<DesktopInfo, ApiError> {
    require_manager(&window)?;
    if let Some(failure) = app.try_state::<StartupFailure>() {
        return Err(ApiError::new("WORKSPACE_START_FAILED", &failure.message));
    }
    let state = app.state::<AppState>();
    let _io_lease = state.handle.io_lifetime().admit()?;
    let mut info = state.info.clone();
    info.runtime = state.handle.info();
    info.platform = state.handle.platform_info();
    Ok(info)
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PathChoice {
    Import,
    Export,
    Workspace,
    Icon,
    Nodeapp,
    Source,
}

#[tauri::command]
async fn choose_path(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    kind: PathChoice,
    suggested_name: Option<String>,
) -> Result<Option<String>, ApiError> {
    require_manager(&window)?;
    let io_lifetime = app.state::<AppState>().handle.io_lifetime();
    io_lifetime.check_open()?;
    let dialog = app.dialog().file().set_parent(&window);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    match kind {
        PathChoice::Import => dialog
            .set_title("选择完整项目 ZIP")
            .add_filter("完整项目归档", &["zip"])
            .pick_file(move |path| {
                let _ = sender.send(path);
            }),
        PathChoice::Export => dialog
            .set_title("导出完整项目 ZIP")
            .set_file_name(suggested_name.unwrap_or_else(|| "project.zip".to_owned()))
            .add_filter("完整项目归档", &["zip"])
            .save_file(move |path| {
                let _ = sender.send(path);
            }),
        PathChoice::Icon => dialog
            .set_title("选择项目 PNG 图标")
            .add_filter("PNG 图标", &["png"])
            .pick_file(move |path| {
                let _ = sender.send(path);
            }),
        PathChoice::Workspace => dialog
            .set_title("选择独立、为空的 APFS 工作区目录")
            .pick_folder(move |path| {
                let _ = sender.send(path);
            }),
        PathChoice::Nodeapp => dialog
            .set_title("选择本地项目文件夹（空目录或已有项目）")
            .pick_folder(move |path| {
                let _ = sender.send(path);
            }),
        PathChoice::Source => {
            dialog
                .set_title("选择要检查的项目源码目录")
                .pick_folder(move |path| {
                    let _ = sender.send(path);
                })
        }
    }
    let selected = receiver
        .await
        .map_err(|_| ApiError::new("DIALOG_FAILED", "路径选择未完成"))?;
    io_lifetime.check_open()?;
    selected
        .map(|path| {
            path.into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| ApiError::new("DIALOG_FAILED", error.to_string()))
        })
        .transpose()
}

#[tauri::command]
async fn project_list(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let projects = state.handle.projects().await?;
            for project in projects.as_array().into_iter().flatten() {
                let (Some(id), Some(name)) = (project["id"].as_str(), project["name"].as_str())
                else {
                    continue;
                };
                if let Some(project_window) = app.get_webview_window(&project_label(id)) {
                    // Metadata refresh changes the host title without reloading forms.
                    let _ = project_window.set_title(&project_title(name));
                }
            }
            Ok(projects)
        })
        .await?
}

#[tauri::command]
async fn project_metadata(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_metadata(&project_id).await
        })
        .await?
}
#[tauri::command]
async fn project_metadata_set(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let result = state
                .handle
                .project_metadata_set(&project_id, input)
                .await?;
            if let Some(project) = app.get_webview_window(&project_label(&project_id)) {
                if let Some(name) = result["name"].as_str() {
                    let _ = project.set_title(&project_title(name));
                }
            }
            Ok(result)
        })
        .await?
}
#[tauri::command]
async fn project_icon_import(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    source_path: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_icon_import(&project_id, std::path::Path::new(&source_path), input)
                .await
        })
        .await?
}
#[tauri::command]
async fn storage_info(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.storage_info(&project_id).await
        })
        .await?
}
#[tauri::command]
async fn storage_scan(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.storage_scan(&project_id).await
        })
        .await?
}
#[tauri::command]
async fn storage_files(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.storage_files(&project_id, input).await
        })
        .await?
}
#[tauri::command]
async fn storage_cleanup(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.storage_cleanup(&project_id, input).await
        })
        .await?
}
#[tauri::command]
async fn workspace_storage(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.workspace_storage().await
        })
        .await?
}
#[tauri::command]
async fn workspace_cleanup_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.workspace_cleanup_preview().await
        })
        .await?
}
#[tauri::command]
async fn workspace_cleanup(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.workspace_cleanup(input).await
        })
        .await?
}
#[tauri::command]
async fn storage_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    path: String,
    document_epoch: String,
) -> Result<Value, ApiError> {
    use base64::Engine;
    use std::io::Read;
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease.run(async move {
        let state = io_app.state::<AppState>();
    let (file, lease, info) = state
        .handle
        .attachment_read(&project_id, &path, &document_epoch)
        .await?;
    let mime = info["mediaType"].as_str().unwrap_or("").to_owned();
    if ![
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "audio/mpeg",
        "audio/wav",
        "audio/mp4",
        "audio/ogg",
        "audio/webm",
        "video/mp4",
        "video/webm",
    ]
    .contains(&mime.as_str())
    {
        return Err(ApiError::new(
            "PREVIEW_UNSUPPORTED",
            "此格式提供下载，不在管理页执行",
        ));
    }
    if info["size"].as_u64().unwrap_or(u64::MAX) > 16_000_000 {
        return Err(ApiError::new(
            "PREVIEW_TOO_LARGE",
            "预览上限为内部测试参数 16 MB；请下载完整文件",
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {let _lease=lease;let mut bytes=Vec::new();file.take(16_000_001).read_to_end(&mut bytes)?;if bytes.len()>16_000_000{return Err(ApiError::new("PREVIEW_TOO_LARGE","文件在读取期间超出预览上限"));} Ok(serde_json::json!({"dataUrl":format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(&bytes)),"mimeType":mime,"sizeBytes":bytes.len()}))}).await.map_err(|_|ApiError::new("PREVIEW_FAILED","预览读取未完成"))?

    }).await?
}
#[tauri::command]
async fn storage_download(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_id: String,
    path: String,
    document_epoch: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("attachment")
        .to_owned();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let (global_lease, project_lease) =
        state.engine_downloads.admit_project_io(&project_id, || {
            let lease = state.handle.io_lifetime().admit()?;
            state
                .handle
                .store()
                .project(&project_id)?
                .validate_epoch(Some(&document_epoch))?;
            Ok(lease)
        })?;
    let key = project_id.clone();
    let deferred = state.engine_downloads.defer_project(
        &key,
        project_lease.hold(global_lease.hold((project_id, path, document_epoch))),
    )?;
    drop(key);
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title("保存项目附件")
        .set_file_name(name)
        .save_file(move |value| {
            let selected = match value {
                Some(value) => deferred.claim().map(|intent| Some((value, intent))),
                None => Ok(None),
            };
            let _ = sender.send(selected);
        });
    let selected = receiver
        .await
        .map_err(|_| ApiError::new("DIALOG_FAILED", "保存选择未完成"))??;
    let Some((destination, intent)) = selected else {
        return Ok(Value::Null);
    };
    let destination = destination
        .into_path()
        .map_err(|e| ApiError::new("INVALID_PATH", e.to_string()))?;
    // Selection is not an I/O lease. Recheck admission only when actual copying
    // can start, then let the owned task outlive a lost native reply.
    let io_lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    io_lease
        .run(async move {
            handle.validate_download_destination(&destination)?;
            let (id, path, epoch) = intent.resource().resource();
            let guard = handle.native_project_io(id, epoch, "native-attachment-copy")?;
            let file = match guard.attachment_file(path) {
                Ok(file) => file,
                Err(error) => {
                    guard.finish()?;
                    return Err(error);
                }
            };
            tauri::async_runtime::spawn_blocking(move || {
                let result = handle.publish_native_file(&guard, file, &destination);
                let completion = guard.finish();
                drop(intent);
                result?;
                completion?;
                Ok(serde_json::json!({"savedPath":destination}))
            })
            .await
            .map_err(|_| ApiError::new("DOWNLOAD_FAILED", "附件保存未完成"))?
        })
        .await?
}
#[tauri::command]
async fn project_reveal(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    target: String,
) -> Result<(), ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let location = state.handle.project_location(&project_id, &target).await?;
            let mut command = Command::new("/usr/bin/open");
            if target == "readme" {
                command.arg("-R");
            }
            command.arg(location);
            let status = command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if !status.success() {
                return Err(ApiError::new("REVEAL_FAILED", "无法打开项目位置"));
            }
            Ok(())
        })
        .await?
}

#[tauri::command]
async fn project_create(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: CreateProjectInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.create_project(input).await
        })
        .await?
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OpenTarget {
    Desktop,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenProjectResult {
    project_id: String,
    target: OpenTarget,
    reused: bool,
}

fn project_label(id: &str) -> String {
    format!("project-{id}")
}

fn project_title(name: &str) -> String {
    format!("{name} · 熊猫小抽屉（开发版）")
}

fn is_project_url(url: &Url, base: &Url, id: &str) -> bool {
    url.origin() == base.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && (url.path() == "/_platform/connect" || url.path().starts_with(&format!("/p/{id}/")))
}

fn external_link_allowed(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.as_str().len() <= 4096
        && host.contains('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && !host.ends_with(".local")
        && !host.ends_with(".localhost")
}
fn request_external_link(app: &tauri::AppHandle, url: Url) {
    if !external_link_allowed(&url) {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let io_lifetime = state.handle.io_lifetime();
    if io_lifetime.check_open().is_err() {
        return;
    }
    app.dialog()
        .message(format!(
            "将在默认浏览器打开：\n{}\n\n项目窗口仍保留当前内容。",
            url.as_str()
        ))
        .title("打开外部链接")
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
            "打开浏览器".into(),
            "取消".into(),
        ))
        .show(move |approved| {
            if !approved {
                return;
            }
            let Ok(lease) = io_lifetime.admit() else {
                return;
            };
            let work = lease.hold(move || {
                let _ = Command::new("/usr/bin/open")
                    .arg(url.as_str())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            });
            tauri::async_runtime::spawn_blocking(move || work.run_blocking());
        });
}

#[tauri::command]
async fn project_open(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_id: String,
    target: OpenTarget,
    reload: Option<bool>,
) -> Result<OpenProjectResult, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            // Creation is serialized, so two close clicks cannot create two containers.
            // Reuse only focuses the current document and never exchanges a new ticket.
            window_creation_allowed(&state)?;
            let maintenance = state.handle.maintenance_status(&project_id).await?;
            if maintenance["recoveryRequired"] == true {
                return Err(ApiError::new(
                    "RECOVERY_REQUIRED",
                    "此项目有待处理的恢复操作，请先查看备份与维护。",
                ));
            }
            if maintenance["mode"] == "maintenance" {
                return Err(ApiError::new(
                    "PROJECT_MAINTENANCE",
                    "项目仍处于维护。完成检查并显式结束维护后，再重新加载项目。",
                ));
            }
            let project = state.handle.store().info(&project_id).await?;
            if project.state != "ready" {
                return Err(ApiError::new(
                    "PROJECT_NOT_READY",
                    match project.state.as_str() {
                        "unbuilt" => "此项目还没有网页产物。请先构建项目后再打开。",
                        "missing" => "项目文件缺失，请查看项目详情。",
                        _ => "此项目暂时无法打开，请查看项目详情。",
                    },
                ));
            }
            let resolved = state.handle.store().project(&project_id)?;
            let project_read = resolved.operation_gate.clone().read_owned().await;
            resolved.validate_epoch(None)?;
            let native_admission = state
                .window_lifetimes
                .admit_project("web", &project_id)
                .map_err(|issue| ApiError::new(issue.code, "项目窗口正在关闭，请稍后重试。"))?;
            let _project_creation = native_admission
                .operation()
                .await
                .map_err(|issue| ApiError::new(issue.code, "项目窗口正在关闭。"))?;
            window_creation_allowed(&state)?;
            if matches!(target, OpenTarget::Desktop) && !reload.unwrap_or(false) {
                if let Some(existing) = app.get_webview_window(&project_label(&project_id)) {
                    let label = existing.label().to_owned();
                    let title = project_title(&project.name);
                    let registration = state.project_windows.clone().lock_owned().await;
                    native_ui(&app, move |app, _| {
                        let _registration = registration;
                        let _read = project_read;
                        resolved.validate_epoch(None)?;
                        native_admission
                            .check()
                            .map_err(|issue| ApiError::new(issue.code, "项目窗口正在关闭。"))?;
                        app.get_webview_window(&label)
                            .ok_or_else(|| ApiError::new("WINDOW_OPEN_FAILED", "项目窗口已关闭"))?
                            .show()
                            .and_then(|_| existing.unminimize())
                            .and_then(|_| existing.set_title(&title))
                            .and_then(|_| existing.set_focus())
                            .map_err(|_| ApiError::new("WINDOW_OPEN_FAILED", "无法显示项目窗口"))
                    })
                    .await?;
                    return Ok(OpenProjectResult {
                        project_id,
                        target,
                        reused: true,
                    });
                }
            }

            let launch = state.handle.launch(&project_id).await?;
            let base = Url::parse(&state.handle.info().base_url)
                .map_err(|_| ApiError::new("INVALID_RUNTIME_URL", "本地服务地址无效"))?;
            let launch_url = Url::parse(&launch.launch_url)
                .map_err(|_| ApiError::new("INVALID_LAUNCH_URL", "项目入口地址无效"))?;
            if !is_project_url(&launch_url, &base, &project_id)
                || launch_url.path() != "/_platform/connect"
            {
                return Err(ApiError::new(
                    "INVALID_LAUNCH_URL",
                    "项目入口不属于当前本地服务",
                ));
            }

            window_creation_allowed(&state)?;
            match target {
                OpenTarget::Desktop => {
                    if app
                        .get_webview_window(&project_label(&project_id))
                        .is_some()
                    {
                        // Explicit reload discards the current document only after the
                        // user asks for it. Normal open still preserves form state.
                        state
                            .engine_downloads
                            .seal_project_and_drain(&project_id)
                            .await?;
                        project_window_cleanup::destroy_web_for_reload(&app, &project_id).await?;
                        state.engine_downloads.forget_project(&project_id)?;
                    }
                    let window_config = platform_core::app_windows::AppWindowConfig::from_value(
                        project
                            .manifest
                            .as_ref()
                            .and_then(|manifest| manifest.extra.get("window")),
                    )
                    .unwrap_or_else(|error| {
                        eprintln!("Project window declaration ignored: {}", error.code);
                        Default::default()
                    });
                    let id = project_id.clone();
                    let name = project.name.clone();
                    let epoch = launch.document_epoch;
                    let registration = state.project_windows.clone().lock_owned().await;
                    native_ui(&app, move |app, state| {
                        let _registration = registration;
                        let _read = project_read;
                        resolved.validate_epoch(Some(&epoch))?;
                        native_admission
                            .check()
                            .map_err(|issue| ApiError::new(issue.code, "项目窗口正在关闭。"))?;
                        build_project_window(
                            app,
                            state,
                            ProjectWindowInput {
                                project_id: id,
                                name,
                                launch_url,
                                window_config,
                                document_epoch: epoch,
                                native_admission,
                            },
                        )
                    })
                    .await?;
                }
            }
            Ok(OpenProjectResult {
                project_id,
                target,
                reused: false,
            })
        })
        .await?
}

fn window_creation_allowed(state: &AppState) -> Result<(), ApiError> {
    state.handle.check_native_ready()?;
    if state
        .shutting_down
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(ApiError::new(
            "DESKTOP_STOPPING",
            "平台正在退出，暂时不能打开新窗口",
        ));
    }
    state.handle.io_lifetime().check_open()
}
async fn native_ui<T: Send + 'static>(
    app: &tauri::AppHandle,
    work: impl FnOnce(&tauri::AppHandle, &AppState) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let owner = app.clone();
    app.run_on_main_thread(move || {
        let state = owner.state::<AppState>();
        let result = window_creation_allowed(&state).and_then(|()| work(&owner, &state));
        let _ = send.send(result);
    })
    .map_err(|_| {
        ApiError::new(
            "WINDOW_UI_DISPATCH_FAILED",
            "窗口操作尚未完成，已保留原状态",
        )
    })?;
    receive
        .await
        .map_err(|_| ApiError::new("WINDOW_UI_REPLY_LOST", "未收到窗口操作回执，已保留原状态"))?
}
fn observe_project_window(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    id: String,
    receipt: std::sync::Arc<business_window_lifetime::Receipt>,
    admission: business_window_lifetime::ProjectAdmission,
) {
    let app = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            // Ordinary close cannot destroy WebKit before its real download
            // Finished callbacks. Global shutdown uses trusted destroy instead.
            api.prevent_close();
            project_window_cleanup::request_web_close(
                &app,
                &id,
                receipt.clone(),
                admission.clone(),
            );
        }
        tauri::WindowEvent::Destroyed => {
            let state = app.state::<AppState>();
            state.window_lifetimes.destroyed(&receipt, || {
                if !state
                    .shutting_down
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    if let Ok(_lease) = state.handle.io_lifetime().admit_cleanup() {
                        state.window_sizes.flush();
                    }
                }
                state.window_sizes.untrack(&receipt.label);
            });
        }
        _ => {}
    });
}
struct ProjectWindowInput {
    project_id: String,
    name: String,
    launch_url: Url,
    window_config: platform_core::app_windows::AppWindowConfig,
    document_epoch: String,
    native_admission: business_window_lifetime::ProjectAdmission,
}
fn build_project_window(
    app: &tauri::AppHandle,
    state: &AppState,
    input: ProjectWindowInput,
) -> Result<(), ApiError> {
    let ProjectWindowInput {
        project_id,
        name,
        launch_url,
        window_config,
        document_epoch,
        native_admission,
    } = input;
    let allowed_id = project_id.clone();
    let runtime_handle = state.handle.clone();
    let permission_handle = state.handle.clone();
    let permission_id = project_id.clone();
    let navigation_app = app.clone();
    let new_window_app = app.clone();
    let window_identity = platform_core::app_windows::WindowIdentity::project(
        &state.handle.info().workspace_root,
        &project_id,
    );
    let size = state
        .window_sizes
        .initial(app, &window_identity, window_config);
    let store_identifier = state.handle.webview_stores().record(
        platform_core::project_cleanup::ProjectKind::Web,
        &project_id,
    )?;
    let receipt = state
        .window_lifetimes
        .reserve_project(&project_label(&project_id), &native_admission)
        .map_err(|_| ApiError::new("WINDOW_GENERATION_STILL_OWNED", "上次项目窗口尚未确认关闭"))?;
    let project_window = tauri::WebviewWindowBuilder::new(
        app,
        project_label(&project_id),
        tauri::WebviewUrl::External(launch_url),
    )
    .data_store_identifier(store_identifier)
    .title(project_title(&name))
    .inner_size(size.width, size.height)
    .min_inner_size(
        platform_core::app_windows::MIN_WIDTH,
        platform_core::app_windows::MIN_HEIGHT,
    )
    .on_navigation(move |url| {
        let local = Url::parse(&runtime_handle.info().base_url)
            .is_ok_and(|current| is_project_url(url, &current, &allowed_id));
        if !local {
            request_external_link(&navigation_app, url.clone());
        }
        local
    })
    .on_download(browser_download::handler(
        app.clone(),
        state.handle.clone(),
        project_id.clone(),
        document_epoch,
        state.engine_downloads.clone(),
    ))
    .disable_drag_drop_handler()
    .on_permission_request(move |webview, kind| {
        use tauri::webview::{PermissionKind, PermissionResponse};
        let Some(device) = (match kind {
            PermissionKind::Camera => Some("camera"),
            PermissionKind::Microphone => Some("microphone"),
            _ => None,
        }) else {
            return PermissionResponse::Deny;
        };
        let Ok(_io_lease) = permission_handle.io_lifetime().admit() else {
            return PermissionResponse::Deny;
        };
        let info = permission_handle.info();
        let allowed = webview
            .url()
            .ok()
            .zip(Url::parse(&info.base_url).ok())
            .is_some_and(|(url, base)| is_project_url(&url, &base, &permission_id))
            && permission_handle.device_permission_declared(&permission_id, device);
        if allowed {
            PermissionResponse::Default
        } else {
            PermissionResponse::Deny
        }
    })
    // Project windows have browser APIs, never native management access.
    .on_new_window(move |url, _| {
        request_external_link(&new_window_app, url);
        NewWindowResponse::Deny
    })
    .build()
    .map_err(|_| {
        receipt.failed();
        ApiError::new("WINDOW_OPEN_FAILED", "无法创建项目窗口，关闭状态须保留核对")
    })?;
    state
        .window_sizes
        .track(&project_window, window_identity, window_config);
    observe_project_window(app, &project_window, project_id, receipt, native_admission);
    Ok(())
}

#[tauri::command]
async fn project_build(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Option<Value>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .build_project_with_input(
                    &project_id,
                    input.unwrap_or_else(|| serde_json::json!({})),
                )
                .await
        })
        .await?
}

#[tauri::command]
async fn maintenance_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.maintenance_status(&project_id).await
        })
        .await?
}

#[tauri::command]
async fn project_export(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_export(&project_id, input).await
        })
        .await?
}

#[tauri::command]
async fn project_copy(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_copy(&project_id, input).await
        })
        .await?
}

#[tauri::command]
async fn project_delete_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            io_app
                .state::<AppState>()
                .handle
                .project_delete_preview(&project_id)
                .await
        })
        .await?
}

#[tauri::command]
async fn project_delete(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_delete(&project_id, input).await
        })
        .await?
}

#[tauri::command]
async fn project_import(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_import(input).await
        })
        .await?
}

#[tauri::command]
async fn project_import_cleanup(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_import_cleanup(task_id).await
        })
        .await?
}

#[tauri::command]
async fn workspace_migrate(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.workspace_migrate(input).await
        })
        .await?
}

#[tauri::command]
async fn project_git_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_git_status(&id).await
        })
        .await?
}
#[tauri::command]
async fn project_git_versions(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    limit: u32,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_git_versions(&id, limit).await
        })
        .await?
}
#[tauri::command]
async fn project_git_source_restore_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitSourceRestorePreviewInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_source_restore_preview(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_source_restore_apply(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitSourceRestoreApplyInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_source_restore_apply(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_source_restore_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_git_source_restore_status(&id).await
        })
        .await?
}
#[tauri::command]
async fn project_git_source_restore_recover(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitSourceRestoreRecoverInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_source_restore_recover(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_data_restore_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitDataRestorePreviewInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_data_restore_preview(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_data_restore_apply(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitDataRestoreApplyInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_data_restore_apply(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_data_restore_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_git_data_restore_status(&id).await
        })
        .await?
}
#[tauri::command]
async fn project_git_data_restore_recover(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitDataRestoreRecoverInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_data_restore_recover(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_data_save_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_git_data_save_status(&id).await
        })
        .await?
}
#[tauri::command]
async fn project_git_data_save_finalize(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    input: platform_core::project_git::GitDataSaveFinalizeInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state
                .handle
                .project_git_data_save_finalize(&id, input)
                .await
        })
        .await?
}
#[tauri::command]
async fn project_git_cancel(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    id: String,
    task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.project_git_cancel(&id, &task_id).await
        })
        .await?
}
// These commands deliberately belong only to the native manager. Node pages
// receive neither this command surface nor the private backend credentials.
#[tauri::command]
async fn node_app_directory_inspect(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let apps = state.node_apps.clone();
            tokio::task::spawn_blocking(move || apps.inspect_directory(&PathBuf::from(source_path)))
                .await
                .map_err(|_| {
                    ApiError::new(
                        "LOCAL_DIRECTORY_CHECK_FAILED",
                        "目录检查未完成，请重新检查。",
                    )
                })?
        })
        .await?
}
#[tauri::command]
async fn node_app_directory_initialize(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    source_path: String,
    input: platform_core::node_apps::DirectoryInitializeInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            if state
                .shutting_down
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(ApiError::new(
                    "SERVICE_STOPPING",
                    "平台正在退出，请稍后重新检查目录。",
                ));
            }
            let apps = state.node_apps.clone();
            tokio::task::spawn_blocking(move || {
                if app
                    .state::<AppState>()
                    .shutting_down
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    return Err(ApiError::new(
                        "SERVICE_STOPPING",
                        "平台正在退出，尚未开始初始化，请稍后重新检查目录。",
                    )
                    .outcome("not_started"));
                }
                apps.initialize_directory(&PathBuf::from(source_path), input)
            })
            .await
            .map_err(|_| {
                ApiError::new(
                    "LOCAL_DIRECTORY_INITIALIZE_INTERRUPTED",
                    "初始化结果未能确认，请保留目录中的文件并重新检查；不会自动重复初始化。",
                )
                .outcome("unknown")
            })?
        })
        .await?
}
#[tauri::command]
async fn node_app_directory_initialize_and_register(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    source_path: String,
    input: platform_core::node_apps::DirectoryInitializeInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            if state
                .shutting_down
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(ApiError::new(
                    "SERVICE_STOPPING",
                    "平台正在退出，尚未生成或添加项目，请稍后重新检查。",
                )
                .outcome("not_started"));
            }
            let apps = state.node_apps.clone();
            let runtime = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                if app
                    .state::<AppState>()
                    .shutting_down
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    return Err(ApiError::new(
                        "SERVICE_STOPPING",
                        "平台正在退出，尚未生成或添加项目，请稍后重新检查。",
                    )
                    .outcome("not_started"));
                }
                runtime.block_on(
                    apps.initialize_and_register_directory(PathBuf::from(source_path), input),
                )
            })
            .await
            .map_err(|_| {
                ApiError::new(
            "LOCAL_DIRECTORY_CREATE_INTERRUPTED",
            "生成并添加的结果未能确认，请保留目录文件并重新检查登记状态；不会自动重复生成或添加。",
        )
        .outcome("unknown")
            })?
        })
        .await?
}
#[tauri::command]
async fn node_app_directory_register(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
    expected_revision: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            if state
                .shutting_down
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(ApiError::new(
                    "SERVICE_STOPPING",
                    "平台正在退出，请稍后重新检查目录。",
                ));
            }
            state
                .node_apps
                .register_directory(PathBuf::from(source_path), expected_revision)
                .await
        })
        .await?
}
#[tauri::command]
async fn node_app_inspect(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.node_apps.inspect(&PathBuf::from(source_path))
        })
        .await?
}
#[tauri::command]
async fn node_app_register(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.node_apps.register(PathBuf::from(source_path)).await
        })
        .await?
}
#[tauri::command]
async fn node_app_list(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.node_apps.list().await
        })
        .await?
}
#[tauri::command]
async fn node_app_delete_preview(
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
            state.node_apps.delete_preview(&app_id).await
        })
        .await?
}
#[tauri::command]
async fn node_app_delete_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    task_id: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let _lease = state.handle.io_lifetime().admit_cleanup()?;
    match task_id {
        Some(task_id) => state.node_apps.deletion_task_status(&task_id, &app_id),
        None => state.node_apps.deletion_operation_status(&app_id),
    }
}
#[tauri::command]
async fn node_app_delete_tasks(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, ApiError> {
    require_manager(&window)?;
    let _lease = state.handle.io_lifetime().admit_cleanup()?;
    state.node_apps.deletion_tasks()
}
#[tauri::command]
async fn node_app_delete_submit(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: platform_core::node_apps::DeleteNodeAppInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let owner = window.app_handle().clone();
    lease
        .run(async move {
            let state = owner.state::<AppState>();
            node_windows::submit_delete(&app, &state, &app_id, input).await
        })
        .await?
}
#[tauri::command]
async fn node_app_delete_retry(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    task_id: String,
    app_id: String,
    observed_revision: u64,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit()?;
    let owner = window.app_handle().clone();
    lease
        .run(async move {
            let state = owner.state::<AppState>();
            node_windows::retry_delete(&app, &state, &task_id, &app_id, observed_revision).await
        })
        .await?
}
#[tauri::command]
async fn node_app_delete_abandon(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
    app_id: String,
    observed_revision: u64,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let lease = state.handle.io_lifetime().admit_cleanup()?;
    let owner = window.app_handle().clone();
    lease
        .run(async move {
            let state = owner.state::<AppState>();
            state.node_apps.set_deletion_store(state.handle.store())?;
            state
                .node_apps
                .abandon_deletion(&task_id, &app_id, observed_revision)
                .await
        })
        .await?
}
#[tauri::command]
async fn node_app_delete(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: platform_core::node_apps::DeleteNodeAppInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            node_windows::delete(&app, &state, &app_id, input).await
        })
        .await?
}
#[tauri::command]
async fn node_app_configuration(
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
            state.node_apps.configuration(&app_id).await
        })
        .await?
}
#[tauri::command]
async fn node_app_configuration_save(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    input: platform_core::node_app_configuration::SaveConfigurationRequest,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.node_apps.save(&app_id, input).await
        })
        .await?
}
#[tauri::command]
async fn node_app_start(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            node_windows::open(&app, &state, &app_id).await
        })
        .await?
}
#[tauri::command]
async fn node_app_stop(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            node_windows::close(&app, &state, &app_id).await
        })
        .await?
}
#[tauri::command]
async fn node_app_restart(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            node_windows::restart(&app, &state, &app_id).await
        })
        .await?
}
#[tauri::command]
async fn node_app_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.node_apps.status(&app_id).await
        })
        .await?
}
fn open_node_url(url: String) -> Result<(), ApiError> {
    Command::new("/usr/bin/open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|_| {
            ApiError::new(
                "BROWSER_OPEN_FAILED",
                "无法打开浏览器，请检查系统默认浏览器设置。",
            )
        })
}
#[tauri::command]
async fn node_app_open(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
) -> Result<(), ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            node_windows::open(&app, &state, &app_id).await.map(|_| ())
        })
        .await?
}
#[tauri::command]
async fn node_app_open_help(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    app_id: String,
    field_name: String,
) -> Result<(), ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            open_node_url(state.node_apps.help_url(&app_id, &field_name).await?)
        })
        .await?
}

#[tauri::command]
async fn workspace_info(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.workspace_info().await
        })
        .await?
}

#[tauri::command]
async fn task_get(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
    project_id: Option<String>,
    task_type: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit_cleanup()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            match (project_id.as_deref(), task_type.as_deref()) {
                (Some(project_id), Some(task_type)) => {
                    state
                        .handle
                        .task_for_project(&task_id, project_id, task_type)
                        .await
                }
                (None, None) => state.handle.task(&task_id).await,
                _ => Err(ApiError::new(
                    "INVALID_INPUT",
                    "任务查询的项目身份和类型必须同时提供。",
                )),
            }
        })
        .await?
}

#[tauri::command]
async fn environment_doctor(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.environment().await
        })
        .await?
}

#[tauri::command]
async fn environment_node_set(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    node_path: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.set_node_path(node_path).await
        })
        .await?
}

#[tauri::command]
async fn environment_git_doctor(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.git_environment().await
        })
        .await?
}

#[tauri::command]
async fn environment_git_set(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    git_path: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let path = match git_path {
                Value::Null => None,
                Value::String(path) => Some(path),
                _ => {
                    return Err(ApiError::new(
                        "INVALID_GIT_PATH",
                        "gitPath must be an absolute path string or null",
                    ))
                }
            };
            state.handle.set_git_path(path).await
        })
        .await?
}

#[tauri::command]
async fn environment_git_remote_check(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: platform_core::git_remote::GitRemoteCheckInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.check_git_remote(input).await
        })
        .await?
}

#[tauri::command]
async fn project_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            state.handle.handoff(&project_id).await
        })
        .await?
}

#[tauri::command]
async fn project_open_codex(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    expected_project_root: String,
    expected_instance_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let lease = state.handle.editor_handoff_lease(
                &project_id,
                std::path::Path::new(&expected_project_root),
                &expected_instance_id,
            )?;
            let app = codex_handoff::installed_app().await?;
            // Recheck after signature validation: closing/migration may have started.
            lease.validate()?;
            codex_handoff::open(&app, &lease.project_root).await?;
            Ok(serde_json::json!({"projectRoot":lease.project_root,"requested":true}))
        })
        .await?
}

struct InstalledResources {
    info: ResourceInfo,
    cli_path: Option<String>,
    root: PathBuf,
    cli_executable: PathBuf,
}

fn resource_info(app: &tauri::App) -> Result<InstalledResources, Box<dyn std::error::Error>> {
    let mut resources = app.path().resource_dir()?.join("resources");
    if cfg!(debug_assertions) && !resources.join("README.md").is_file() {
        resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    }
    let installed_cli = std::env::current_exe()?
        .parent()
        .ok_or("Missing executable directory")?
        .join("local-projects-dev");
    let cli = if installed_cli.is_file() {
        installed_cli
    } else if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/local-projects-dev-aarch64-apple-darwin")
    } else {
        installed_cli
    };
    let cli_path = cli.is_file().then(|| cli.to_string_lossy().into_owned());
    Ok(InstalledResources {
        info: ResourceInfo {
            sdk: resources
                .join("sdk/releases/local-projects-sdk-0.1.0.tgz")
                .is_file(),
            template: resources.join("template/app/package.json").is_file(),
            readme: std::fs::read_to_string(resources.join("README.md")).is_ok(),
            cli: cli_path.is_some(),
        },
        cli_path,
        root: resources,
        cli_executable: cli,
    })
}

fn show_manager(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("manager") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn install_application_menu(app: &tauri::App) -> tauri::Result<()> {
    // macOS's predefined Quit calls NSApplication.terminate: directly, which
    // bypasses ExitRequested. Our ordinary menu item uses app.exit instead.
    let quit = MenuItem::with_id(app, "quit", "退出开发版", true, Some("CmdOrCtrl+Q"))?;
    let application = Submenu::with_items(
        app,
        DISPLAY_NAME,
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let project_manager = MenuItem::with_id(
        app,
        "show-manager",
        "项目管理",
        true,
        Some("CmdOrCtrl+Shift+H"),
    )?;
    let window = Submenu::with_items(
        app,
        "窗口",
        true,
        &[
            &project_manager,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    app.set_menu(Menu::with_items(app, &[&application, &edit, &window])?)?;
    Ok(())
}

fn request_safe_shutdown(app: &tauri::AppHandle) {
    desktop_shutdown_native::request(app);
}

fn install_project_cleanup_hook(app: &tauri::AppHandle) {
    let owner = app.clone();
    app.state::<AppState>()
        .handle
        .set_project_cleanup_hook(std::sync::Arc::new(move |stage, notice| {
            let app = owner.clone();
            Box::pin(async move {
                let state = app.try_state::<AppState>().ok_or_else(|| {
                    ApiError::new(
                        "PROJECT_CLEANUP_NOT_READY",
                        "项目清理尚未就绪，请稍后重试。",
                    )
                })?;
                match stage {
                    platform_core::project_cleanup::NativeCleanupStage::Prepare => {
                        state
                            .window_sizes
                            .forget_project(notice.kind.as_str(), &notice.id);
                        project_window_cleanup::prepare(&app, &notice).await
                    }
                    platform_core::project_cleanup::NativeCleanupStage::Purge => {
                        project_window_cleanup::purge(&app, &notice).await?;
                        let ledger = state.handle.webview_stores();
                        if ledger.contains(notice.kind, &notice.id)? {
                            let identifier = platform_core::webview_storage::project_identifier(
                                notice.kind,
                                &notice.id,
                            )?;
                            native_stores::purge_identifier(&app, identifier).await?;
                            ledger.forget_confirmed(notice.kind, &notice.id)?;
                        }
                        Ok(())
                    }
                    platform_core::project_cleanup::NativeCleanupStage::Committed => {
                        if notice.kind == platform_core::project_cleanup::ProjectKind::Node {
                            state.handle.forget_deleted_node_import(&notice.id);
                        }
                        project_window_cleanup::committed(&app, &notice).await?;
                        let mut payload = serde_json::to_value(notice).map_err(|error| {
                            ApiError::new("PROJECT_CLEANUP_NOTIFICATION_FAILED", error.to_string())
                        })?;
                        payload["instanceId"] = serde_json::json!(state.handle.info().instance_id);
                        app.emit_to("manager", "panda-project-deleted", payload)
                            .map_err(|error| {
                                ApiError::new(
                                    "PROJECT_CLEANUP_NOTIFICATION_FAILED",
                                    error.to_string(),
                                )
                            })
                    }
                }
            })
        }));
}

fn build_manager_window(app: &tauri::AppHandle) -> Result<(), ApiError> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "manager")
        .ok_or_else(|| ApiError::new("MANAGER_WINDOW_CONFIG_MISSING", "管理窗口配置缺失。"))?;
    tauri::WebviewWindowBuilder::from_config(app, config)
        .map_err(|e| ApiError::new("MANAGER_WINDOW_OPEN_FAILED", e.to_string()))?
        .data_store_identifier(platform_core::webview_storage::manager_identifier())
        .on_navigation(|url| is_manager_url(url, cfg!(debug_assertions)))
        .on_download(|_, _| false)
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .build()
        .map_err(|e| ApiError::new("MANAGER_WINDOW_OPEN_FAILED", e.to_string()))?;
    Ok(())
}
async fn initialize_native_storage(app: tauri::AppHandle) {
    let handle = app.state::<AppState>().handle.clone();
    let Ok(lease) = handle.io_lifetime().admit_cleanup() else {
        return;
    };
    let _ = lease
        .run(async move {
            // Setup has returned to the native event loop. WK callbacks must never
            // be awaited by setup's main-thread block_on.
            match native_stores::clear_default_and_verify(&app).await {
                Ok(()) => {
                    if let Err(error) = app.state::<AppState>().node_apps.resume_deletions().await {
                        handle.fail_native_startup(error);
                    } else {
                        handle.complete_native_startup();
                    }
                }
                Err(error) => handle.fail_native_startup(error),
            }
            let owner = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                if let Err(error) = build_manager_window(&owner) {
                    eprintln!("Manager window: {}", error.code);
                }
            }) {
                eprintln!("Manager UI dispatch: {error}");
            }
        })
        .await;
}

fn main() {
    // Reject before Tauri plugins, setup and WebView creation. In particular,
    // do not turn an unopened Windows stage into a workspace-repair prompt.
    if let Err(error) = platform_core::platform_support::require_runtime() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_manager(app)
        }))
        .invoke_handler(tauri::generate_handler![
            local_data_cleanup::local_data_cleanup_preview,
            local_data_cleanup::local_data_cleanup_start,
            local_data_cleanup::local_data_cleanup_status,
            local_data_cleanup::local_data_cleanup_quit,
            bootstrap_info,

            project_onboarding::source_review_context,project_onboarding::source_review_open_codex,project_onboarding::source_review_capabilities,
            source_import::source_import_location,source_import::source_import_recover,
            source_import::node_git_import_start,source_import::node_git_import_status,source_import::node_git_import_cancel,source_import::node_git_import_retry_cleanup,
            project_git_status,project_git_versions,project_git_source_restore_preview,project_git_source_restore_apply,project_git_source_restore_status,project_git_source_restore_recover,project_git_data_restore_preview,project_git_data_restore_apply,project_git_data_restore_status,project_git_data_restore_recover,project_git_data_save_status,project_git_data_save_finalize,project_git_cancel,
            node_app_inspect,node_app_register,node_app_directory_inspect,node_app_directory_initialize,node_app_directory_initialize_and_register,node_app_directory_register,node_app_list,node_app_configuration,node_app_configuration_save,node_app_delete_preview,node_app_delete_status,node_app_delete_tasks,node_app_delete_submit,node_app_delete_retry,node_app_delete_abandon,node_app_delete,
            node_app_start,node_app_stop,node_app_restart,node_app_status,node_app_open,node_app_open_help,
            local_directory::open_project_folder,
            choose_path,
            project_export,
            project_import,project_import_cleanup,
            project_copy,
            project_delete,
            project_delete_preview,
            workspace_info,
            workspace_migrate,
            project_list,
            project_create,
            project_metadata,project_metadata_set,project_icon_import,project_reveal,
            storage_info,storage_scan,storage_files,storage_cleanup,storage_preview,storage_download,workspace_storage,workspace_cleanup_preview,workspace_cleanup,
            project_open,
            project_build,
            maintenance_status,
            task_get,
            environment_doctor,
            environment_node_set,
            environment_git_doctor,
            environment_git_set,
            environment_git_remote_check,
            project_handoff,
            project_open_codex,
            node_development::node_app_handoff
        ])
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" | "show-manager" => show_manager(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .setup(|app| {
            platform_core::build_profile::validate_desktop_identity(
                &app.config().identifier,
                &app.package_info().version.to_string(),
            )?;
            platform_core::platform_support::require_runtime()?;
            let mut started_runtime = None;
            let startup = (|| -> Result<AppState, Box<dyn std::error::Error>> {
                let paths = RuntimePaths::initialize()?;
            let lock = paths.acquire_lock()?;
            let InstalledResources {
                info: resources,
                cli_path,
                root: resource_root,
                cli_executable,
            } = resource_info(app)?;
            let node_data_root = paths.data_root().to_path_buf();
            let runtime_node = resource_root.join("node-runtime/bin/node");
            let runtime = tauri::async_runtime::block_on(RuntimeService::start_desktop_with_resources(
                paths,
                lock,
                resource_root,
                cli_executable,
            ))?;
            let runtime = std::sync::Arc::new(runtime);
            let window_sizes = window_sizes::WindowSizes::new(runtime.handle().window_size_store());
            started_runtime = Some(runtime.clone());
            let node_apps = std::sync::Arc::new(platform_core::node_apps::NodeApps::with_runtime(
                node_data_root, runtime_node, &runtime.handle(),
            )?);
            // Reconcile only obsolete run markers before publishing commands.
            // Unknown records remain visible and never authorize PID-only stop.
            tauri::async_runtime::block_on(node_apps.reconcile_run_markers())?;
            // Durable deletion consent survives a host restart. Restore its
            // gates and resume proven cleanup before publishing commands.
            let monitor = std::sync::Arc::downgrade(&node_apps);
            let node_monitor_app = app.handle().clone();
            let monitor_io = runtime.handle().io_lifetime();
            let node_monitor = desktop_monitor::OwnedMonitor::start(
                tauri::async_runtime::handle().inner().clone(), move |mut stop| async move {
                loop {
                    if !desktop_monitor::next_tick(&mut stop, std::time::Duration::from_secs(3)).await { break; }
                    let Some(manager) = monitor.upgrade() else { break; };
                    let Ok(io_lease) = monitor_io.admit() else { break; };
                    io_lease.hold(Box::pin(async {
                        manager.monitor_tick().await;
                        node_windows::reconcile(&node_monitor_app).await;
                    })).await;
                }
            });
            let info = DesktopInfo {
                runtime: runtime.handle().info(),
                version: app.package_info().version.to_string(),
                resources,
                cli_path,
                platform: runtime.handle().platform_info(),
            };
            eprintln!("M5 development runtime ready: {}", info.runtime.base_url);
            Ok(AppState {
                info,
                handle: runtime.handle(),
                runtime,
                desktop_shutdown: std::sync::Arc::new(desktop_shutdown::DesktopShutdown::default()),
                local_data_cleanup: local_data_cleanup::LocalDataCleanup::default(),
                window_lifetimes: business_window_lifetime::WindowLifetimes::default(),
                engine_downloads: std::sync::Arc::new(engine_downloads::EngineDownloads::default()),
                project_windows: std::sync::Arc::new(tokio::sync::Mutex::new(())),
                node_apps,
                node_monitor,
                node_windows: node_windows::NodeWindows::default(),
                window_sizes,
                shutting_down: std::sync::atomic::AtomicBool::new(false),
                exit_ready: std::sync::atomic::AtomicBool::new(false),
            })
            })();
            match startup {
                Ok(state) => {
                    app.manage(state);
                    install_project_cleanup_hook(app.handle());
                    tauri::async_runtime::spawn(initialize_native_storage(app.handle().clone()));
                }
                Err(error) => {
                    let message = format!("平台工作区无法启动：{error:#}\n\n请连接目标磁盘或修复上述路径，再退出并重新启动开发版。为保护现有数据，平台没有创建空工作区，也没有切回旧目录。");
                    eprintln!("{message}");
                    app.manage(StartupFailure {
                        message,
                        runtime: started_runtime,
                        shutdown: std::sync::Arc::new(desktop_shutdown::DesktopShutdown::default()),
                        exit_ready: std::sync::atomic::AtomicBool::new(false),
                    });
                    build_manager_window(app.handle())?;
                }
            }
            if let Some(failure) = app.try_state::<StartupFailure>() {
                app.dialog().message(&failure.message).title("熊猫小抽屉 · 工作区无法启动").kind(tauri_plugin_dialog::MessageDialogKind::Error).show(|_| {});
            }
            install_application_menu(app)?;
            let show =
                MenuItem::with_id(app, "show", "打开熊猫小抽屉（开发版）", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出开发版", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("熊猫小抽屉 · 开发版")
                .menu(&menu);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let Some(state) = window.app_handle().try_state::<AppState>() {
                if !matches!(event, tauri::WindowEvent::Destroyed) {
                    if let Ok(_io_lease) = state.handle.io_lifetime().admit_cleanup() { state.window_sizes.on_event(window, event); }
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "manager" {
                    api.prevent_close();
                    if window.app_handle().try_state::<AppState>().is_some_and(|state| state.local_data_cleanup.active()) {
                        return;
                    }
                    // On macOS, closing the manager keeps the application and
                    // its managed services in the Dock. Reopen restores this
                    // same window; only an explicit quit runs safe shutdown.
                    #[cfg(target_os = "macos")]
                    if let Err(error) = window.hide() {
                        eprintln!("Unable to hide management window: {error}");
                    }
                    #[cfg(not(target_os = "macos"))]
                    window.app_handle().exit(0);
                }
            }

        })
        .build(tauri::generate_context!())
        .expect("Unable to start isolated M5 desktop application");

    app.run(|app, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            let wait_for_running = app
                .try_state::<AppState>()
                .is_some_and(|state| !state.exit_ready.load(std::sync::atomic::Ordering::Acquire));
            let wait_for_partial = app.try_state::<StartupFailure>().is_some_and(|failure| {
                failure.runtime.is_some()
                    && !failure
                        .exit_ready
                        .load(std::sync::atomic::Ordering::Acquire)
            });
            if wait_for_running || wait_for_partial {
                api.prevent_exit();
                request_safe_shutdown(app);
            }
        }
        // Cocoa/forced termination can bypass the preventable request. Keep a
        // owned cleanup fallback without blocking the event loop or claiming crash guarantees.
        tauri::RunEvent::Exit => desktop_shutdown_native::forced_exit(app),
        tauri::RunEvent::Reopen { .. } => show_manager(app),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_urls_never_include_project_runtime() {
        assert!(is_manager_url(
            &Url::parse("tauri://localhost/index.html").unwrap(),
            false
        ));
        assert!(is_manager_url(
            &Url::parse("http://127.0.0.1:1420/").unwrap(),
            true
        ));
        for value in [
            "http://127.0.0.1:49152/p/prj_a/",
            "http://localhost:1420/",
            "http://127.0.0.1.evil.test:1420/",
            "https://example.com/",
            "tauri://other/index.html",
            "tauri://user@localhost/index.html",
        ] {
            assert!(
                !is_manager_url(&Url::parse(value).unwrap(), true),
                "{value}"
            );
        }
        assert!(!is_manager_url(
            &Url::parse("http://127.0.0.1:1420/").unwrap(),
            false
        ));
    }

    #[test]
    fn project_navigation_stays_within_its_origin_and_id() {
        let base = Url::parse("http://127.0.0.1:49152").unwrap();
        for value in [
            "http://127.0.0.1:49152/_platform/connect#ticket=test",
            "http://127.0.0.1:49152/p/prj_a/",
            "http://127.0.0.1:49152/p/prj_a/page?view=history#item",
        ] {
            assert!(
                is_project_url(&Url::parse(value).unwrap(), &base, "prj_a"),
                "{value}"
            );
        }
        for value in [
            "http://127.0.0.1:49153/p/prj_a/",
            "http://localhost:49152/p/prj_a/",
            "http://127.0.0.1:49152/p/prj_b/",
            "http://127.0.0.1:49152/p/prj_abc/",
            "http://127.0.0.1:49152/p/prj_a/../prj_b/",
            "http://127.0.0.1:49152/api/v1/projects",
            "http://user:password@127.0.0.1:49152/p/prj_a/",
            "https://example.com/",
            "tauri://localhost/",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(
                !is_project_url(&Url::parse(value).unwrap(), &base, "prj_a"),
                "{value}"
            );
        }
    }
}
