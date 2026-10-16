#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use platform_core::{
    ApiError, BootstrapInfo, CreateProjectInput, RuntimeHandle, RuntimePaths, RuntimeService,
    APP_IDENTIFIER, DISPLAY_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    webview::NewWindowResponse,
    Manager, Url,
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
}

struct StartupFailure(String);

struct AppState {
    info: DesktopInfo,
    handle: RuntimeHandle,
    runtime: Mutex<Option<RuntimeService>>,
    project_windows: tokio::sync::Mutex<()>,
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
    Ok(())
}

#[tauri::command]
fn bootstrap_info(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<DesktopInfo, ApiError> {
    require_manager(&window)?;
    if let Some(failure) = app.try_state::<StartupFailure>() {
        return Err(ApiError::new("WORKSPACE_START_FAILED", &failure.0));
    }
    let state = app.state::<AppState>();
    let mut info = state.info.clone();
    info.runtime = state.handle.info();
    Ok(info)
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PathChoice {
    Import,
    Export,
    Workspace,
}

#[tauri::command]
async fn choose_path(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    kind: PathChoice,
    suggested_name: Option<String>,
) -> Result<Option<String>, ApiError> {
    require_manager(&window)?;
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
        PathChoice::Workspace => dialog
            .set_title("选择独立、为空的 APFS 工作区目录")
            .pick_folder(move |path| {
                let _ = sender.send(path);
            }),
    }
    receiver
        .await
        .map_err(|_| ApiError::new("DIALOG_FAILED", "路径选择未完成"))?
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
    let projects = state.handle.projects().await?;
    for project in projects.as_array().into_iter().flatten() {
        let (Some(id), Some(name)) = (project["id"].as_str(), project["name"].as_str()) else {
            continue;
        };
        if let Some(project_window) = app.get_webview_window(&project_label(id)) {
            // Metadata refresh changes the host title without reloading forms.
            let _ = project_window.set_title(&project_title(name));
        }
    }
    Ok(projects)
}

#[tauri::command]
async fn project_create(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: CreateProjectInput,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.create_project(input).await
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OpenTarget {
    Desktop,
    Browser,
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
    format!("{name} · 本地项目（开发版）")
}

fn is_project_url(url: &Url, base: &Url, id: &str) -> bool {
    url.origin() == base.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && (url.path() == "/_platform/connect" || url.path().starts_with(&format!("/p/{id}/")))
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
    // Creation is serialized, so two close clicks cannot create two containers.
    // Reuse only focuses the current document and never exchanges a new ticket.
    let _creation = state.project_windows.lock().await;
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
    if matches!(target, OpenTarget::Desktop) && !reload.unwrap_or(false) {
        if let Some(existing) = app.get_webview_window(&project_label(&project_id)) {
            existing
                .show()
                .and_then(|_| existing.unminimize())
                .and_then(|_| existing.set_title(&project_title(&project.name)))
                .and_then(|_| existing.set_focus())
                .map_err(|error| ApiError::new("WINDOW_OPEN_FAILED", error.to_string()))?;
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
    if !is_project_url(&launch_url, &base, &project_id) || launch_url.path() != "/_platform/connect"
    {
        return Err(ApiError::new(
            "INVALID_LAUNCH_URL",
            "项目入口不属于当前本地服务",
        ));
    }

    match target {
        OpenTarget::Desktop => {
            if let Some(existing) = app.get_webview_window(&project_label(&project_id)) {
                // Explicit reload discards the current document only after the
                // user asks for it. Normal open still preserves form state.
                existing
                    .navigate(launch_url)
                    .and_then(|_| existing.show())
                    .and_then(|_| existing.set_focus())
                    .map_err(|error| ApiError::new("WINDOW_OPEN_FAILED", error.to_string()))?;
                return Ok(OpenProjectResult {
                    project_id,
                    target,
                    reused: true,
                });
            }
            let allowed_id = project_id.clone();
            let runtime_handle = state.handle.clone();
            tauri::WebviewWindowBuilder::new(
                &app,
                project_label(&project_id),
                tauri::WebviewUrl::External(launch_url),
            )
            .title(project_title(&project.name))
            .inner_size(1080.0, 780.0)
            .min_inner_size(640.0, 480.0)
            .on_navigation(move |url| {
                Url::parse(&runtime_handle.info().base_url)
                    .is_ok_and(|current| is_project_url(url, &current, &allowed_id))
            })
            // M1 grants no remote capabilities and opens no secondary native
            // webviews. External navigation is deferred to the capability stage.
            .on_new_window(|_, _| NewWindowResponse::Deny)
            .build()
            .map_err(|_| ApiError::new("WINDOW_OPEN_FAILED", "无法创建项目窗口，请重新打开项目"))?;
        }
        OpenTarget::Browser => {
            // Fixed executable, separate argument, no shell, no caller-supplied
            // URL. LaunchServices opens only the validated service-issued entry.
            tauri::async_runtime::spawn_blocking(move || {
                let status = Command::new("/usr/bin/open")
                    .arg(launch_url.as_str())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|_| ApiError::new("BROWSER_OPEN_FAILED", "无法启动默认浏览器"))?;
                if !status.success() {
                    return Err(ApiError::new(
                        "BROWSER_OPEN_FAILED",
                        "默认浏览器没有接受打开请求",
                    ));
                }
                Ok(())
            })
            .await
            .map_err(|_| ApiError::new("BROWSER_OPEN_FAILED", "浏览器打开任务未完成"))??;
        }
    }
    Ok(OpenProjectResult {
        project_id,
        target,
        reused: false,
    })
}

#[tauri::command]
async fn project_build(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Option<Value>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state
        .handle
        .build_project_with_input(&project_id, input.unwrap_or_else(|| serde_json::json!({})))
        .await
}

#[tauri::command]
async fn backup_list(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.backup_list(&project_id).await
}

#[tauri::command]
async fn backup_info(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    backup_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.backup_info(&project_id, &backup_id).await
}

#[tauri::command]
async fn backup_create(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.backup_create(&project_id, input).await
}

#[tauri::command]
async fn backup_restore(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.backup_restore(&project_id, input).await
}

#[tauri::command]
async fn maintenance_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.maintenance_status(&project_id).await
}

#[tauri::command]
async fn maintenance_begin(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.maintenance_begin(&project_id, input).await
}

#[tauri::command]
async fn maintenance_end(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.maintenance_end(&project_id, input).await
}

#[tauri::command]
async fn project_export(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.project_export(&project_id, input).await
}

#[tauri::command]
async fn project_copy(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.project_copy(&project_id, input).await
}

#[tauri::command]
async fn project_delete(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.project_delete(&project_id, input).await
}

#[tauri::command]
async fn project_import(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.project_import(input).await
}

#[tauri::command]
async fn workspace_migrate(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    input: Value,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.workspace_migrate(input).await
}

#[tauri::command]
async fn trash_info(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    trash_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.trash_info(&trash_id).await
}

#[tauri::command]
async fn trash_restore(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    trash_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.trash_restore(&trash_id).await
}

#[tauri::command]
async fn trash_purge(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    trash_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.trash_purge(&trash_id).await
}

#[tauri::command]
async fn trash_list(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.trash_list().await
}

#[tauri::command]
async fn workspace_info(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.workspace_info().await
}

#[tauri::command]
async fn task_get(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.task(&task_id).await
}

#[tauri::command]
async fn task_logs(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    task_id: String,
    cursor: u64,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.task_logs(&task_id, cursor).await
}

#[tauri::command]
async fn environment_doctor(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.environment().await
}

#[tauri::command]
async fn environment_node_set(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    node_path: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.set_node_path(node_path).await
}

#[tauri::command]
async fn project_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    state.handle.handoff(&project_id).await
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

fn shutdown_runtime(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Release the mutex before waiting. These callbacks run on the main thread,
    // so the runtime stays alive in this stack frame until shutdown finishes.
    let runtime = {
        state
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    };
    if let Some(runtime) = runtime {
        // Await the service's operation drain here. macOS termination may not
        // permit an asynchronous follow-up after this callback returns.
        if let Err(error) = tauri::async_runtime::block_on(runtime.shutdown()) {
            eprintln!("Runtime shutdown: {error}");
        }
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_manager(app)
        }))
        .invoke_handler(tauri::generate_handler![
            bootstrap_info,
            choose_path,
            project_export,
            project_import,
            project_copy,
            project_delete,
            trash_list,
            trash_info,
            trash_restore,
            trash_purge,
            workspace_info,
            workspace_migrate,
            project_list,
            project_create,
            project_open,
            project_build,
            backup_list,
            backup_info,
            backup_create,
            backup_restore,
            maintenance_status,
            maintenance_begin,
            maintenance_end,
            task_get,
            task_logs,
            environment_doctor,
            environment_node_set,
            project_handoff
        ])
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" | "show-manager" => show_manager(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .setup(|app| {
            if app.config().identifier != APP_IDENTIFIER {
                return Err(
                    "Desktop identifier and development runtime identity do not match".into(),
                );
            }
            let startup = (|| -> Result<AppState, Box<dyn std::error::Error>> {
                let paths = RuntimePaths::initialize()?;
            let lock = paths.acquire_lock()?;
            let InstalledResources {
                info: resources,
                cli_path,
                root: resource_root,
                cli_executable,
            } = resource_info(app)?;
            let runtime = tauri::async_runtime::block_on(RuntimeService::start_with_resources(
                paths,
                lock,
                resource_root,
                cli_executable,
            ))?;
            let info = DesktopInfo {
                runtime: runtime.handle().info(),
                version: app.package_info().version.to_string(),
                resources,
                cli_path,
            };
            eprintln!("M4 development runtime ready: {}", info.runtime.base_url);
            Ok(AppState {
                info,
                handle: runtime.handle(),
                runtime: Mutex::new(Some(runtime)),
                project_windows: tokio::sync::Mutex::new(()),
            })
            })();
            match startup {
                Ok(state) => { app.manage(state); }
                Err(error) => {
                    let message = format!("平台工作区无法启动：{error:#}\n\n请连接目标磁盘或修复上述路径，再退出并重新启动开发版。为保护现有数据，平台没有创建空工作区，也没有切回旧目录。");
                    eprintln!("{message}");
                    app.manage(StartupFailure(message));
                }
            }
            // Create the webview only after namespace validation and state setup.
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|window| window.label == "manager")
                .ok_or("Missing management window configuration")?;
            tauri::WebviewWindowBuilder::from_config(app, config)?
                .on_navigation(|url| is_manager_url(url, cfg!(debug_assertions)))
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .build()?;
            if let Some(failure) = app.try_state::<StartupFailure>() {
                app.dialog().message(&failure.0).title("本地项目 · 工作区无法启动").kind(tauri_plugin_dialog::MessageDialogKind::Error).show(|_| {});
            }
            install_application_menu(app)?;
            let show =
                MenuItem::with_id(app, "show", "打开本地项目（开发版）", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出开发版", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("本地项目 · 开发版")
                .menu(&menu);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "manager" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Unable to start isolated M3 desktop application");

    app.run(|app, event| match event {
        // Cocoa's Dock/AppleEvent quit may deliver Exit without ExitRequested.
        // Both paths synchronously finish the same idempotent shutdown.
        tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => shutdown_runtime(app),
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
