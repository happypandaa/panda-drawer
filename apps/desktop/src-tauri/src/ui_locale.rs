//! The management language is an explicit preference, independent of the OS.
use platform_core::ApiError;
use platform_native::filesystem as native_fs;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
use tauri::Manager;

static LANGUAGE: AtomicUsize = AtomicUsize::new(0);
const SUPPORTED_LOCALES: &[&str] = &["zh-CN", "en"];
static PREFERENCE_WRITE: Mutex<()> = Mutex::new(());
const PREFERENCE_FILE: &str = "ui-language.json";

#[derive(Deserialize, Serialize)]
struct Preference {
    locale: String,
}

pub(super) fn current() -> &'static str {
    SUPPORTED_LOCALES[LANGUAGE.load(Ordering::Relaxed)]
}

/// Read-only startup lookup, including when the workspace cannot start. The
/// app-local data path is the existing fixed application-identifier directory.
pub(super) fn initialize(app: &tauri::AppHandle) {
    let loaded = (|| -> Option<String> {
        let root = app.path().app_local_data_dir().ok()?;
        require_root(&root).ok()?;
        let file = native_fs::open_read(
            root.join(PREFERENCE_FILE),
            native_fs::ReadOptions::NoLinksNonblockingCloseOnExec,
        )
        .ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > 128 {
            return None;
        }
        let mut source = String::new();
        file.take(129).read_to_string(&mut source).ok()?;
        if source.len() > 128 {
            return None;
        }
        serde_json::from_str::<Preference>(&source)
            .ok()
            .map(|value| value.locale)
    })();
    let language = SUPPORTED_LOCALES
        .iter()
        .position(|candidate| Some(*candidate) == loaded.as_deref())
        .unwrap_or(0);
    LANGUAGE.store(language, Ordering::Relaxed);
}

fn require_root(root: &Path) -> std::io::Result<()> {
    for component in native_fs::path_components(root) {
        if native_fs::is_reparse(&std::fs::symlink_metadata(component)?) {
            return Err(std::io::Error::other(
                "Language preference directory cannot be a link",
            ));
        }
    }
    if !native_fs::directory_is_private(root)? {
        return Err(std::io::Error::other(
            "Language preference directory must be private",
        ));
    }
    Ok(())
}

fn persist(root: &Path, locale: &str) -> std::io::Result<()> {
    require_root(root)?;
    let destination = root.join(PREFERENCE_FILE);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        if !metadata.is_file() || native_fs::is_reparse(&metadata) {
            return Err(std::io::Error::other(
                "Refusing to replace a non-file language preference",
            ));
        }
    }
    let sequence = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let temporary = root.join(format!(
        ".ui-language-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = native_fs::create_new_private_file(&temporary, true)?;
        let preference = serde_json::to_vec(&Preference {
            locale: locale.to_owned(),
        })
        .map_err(std::io::Error::other)?;
        file.write_all(&preference)?;
        file.sync_all()?;
        drop(file);
        native_fs::replace_file(&temporary, &destination)?;
        native_fs::sync_directory(root)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[tauri::command]
pub(super) fn get_ui_locale(window: tauri::WebviewWindow) -> Result<&'static str, ApiError> {
    super::require_manager(&window)?;
    Ok(current())
}

#[tauri::command]
pub(super) fn set_ui_locale(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    locale: String,
) -> Result<&'static str, ApiError> {
    super::require_manager(&window)?;
    let language = SUPPORTED_LOCALES
        .iter()
        .position(|candidate| *candidate == locale)
        .ok_or_else(|| ApiError::new("UI_LOCALE_UNSUPPORTED", "Unsupported interface language"))?;
    let state = app.state::<super::AppState>();
    let _lease = state.handle.io_lifetime().admit()?;
    let _write = PREFERENCE_WRITE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    persist(&state.info.runtime.data_root, &locale)
        .map_err(|_| ApiError::new("UI_LOCALE_SAVE_FAILED", text("语言设置未能保存，请重试。")))?;
    LANGUAGE.store(language, Ordering::Relaxed);
    super::install_application_menu(&app)
        .map_err(|error| ApiError::new("UI_LOCALE_MENU_FAILED", error.to_string()))?;
    super::refresh_native_language(&app)
        .map_err(|error| ApiError::new("UI_LOCALE_MENU_FAILED", error.to_string()))?;
    Ok(current())
}

pub(super) fn text(source: &str) -> &str {
    match (current(), source) {
        ("en", "语言设置未能保存，请重试。") => "Could not save the language preference. Please try again.",
        ("en", "熊猫小抽屉") => "Panda Drawer",
        ("en", "按你的想法改") => "Change it your way",
        ("en", "重新加载") => "Reload",
        ("en", "启动失败") => "Could not start the app",
        ("en", "请查看应用日志，修好后重新加载。") => "Check the app logs, fix the issue, then reload.",
        ("en", "后端启动检查接口没有返回成功结果。") => "The backend startup check did not succeed.",
        ("en", "后端启动检查的回复格式无法识别。") => "The backend startup check returned an unrecognized response.",
        ("en", "后端启动检查的回复超过了大小限制。") => "The backend startup check response exceeded the size limit.",
        ("en", "后端报告准备完成，但平台无法连接它的本机服务。") => "The backend reported that it was ready, but the platform could not connect to its local service.",
        ("en", "后端未在 10 秒内完成本机健康检查。") => "The backend did not complete its local health check within 10 seconds.",
        ("en", "后端健康检查的协议版本、启动身份或状态不符合要求。") => "The backend health check returned an invalid protocol version, startup identity, or status.",
        ("en", "后端报告的启动身份、监听地址或端口不符合要求。") => "The backend reported an invalid startup identity, listening address, or port.",
        ("en", "后端的启动消息不符合平台控制协议，请检查是否把普通日志写到了标准输出。") => "The backend startup message did not follow the platform control protocol. Check whether ordinary logs were written to standard output.",
        ("en", "后端在完成启动前关闭了控制通道或退出。") => "The backend closed its control channel or exited before startup completed.",
        ("en", "后端未在 30 秒内完成启动。") => "The backend did not finish starting within 30 seconds.",
        ("en", "后端未能完成启动，请查看本次运行日志中的具体原因。") => "The backend could not start. Check the current run log for the cause.",
        ("en", "发现旧数据，请先用旧版清理本机数据") => "Old app data was found. Use the previous version to clear local data first.",
        ("en", "熊猫小抽屉 · 开发版") => "Panda Drawer · Development",
        ("en", "熊猫小抽屉（开发版）") => "Panda Drawer · Development",
        ("en", "退出开发版") => "Quit Development App",
        ("en", "打开熊猫小抽屉（开发版）") => "Open Panda Drawer (Development)",
        ("en", "关于熊猫小抽屉") => "About Panda Drawer",
        ("en", "隐藏熊猫小抽屉") => "Hide Panda Drawer",
        ("en", "隐藏其他") => "Hide Others",
        ("en", "显示全部") => "Show All",
        ("en", "编辑") => "Edit",
        ("en", "撤销") => "Undo",
        ("en", "重做") => "Redo",
        ("en", "剪切") => "Cut",
        ("en", "复制") => "Copy",
        ("en", "粘贴") => "Paste",
        ("en", "全选") => "Select All",
        ("en", "项目管理") => "Manage Apps",
        ("en", "窗口") => "Window",
        ("en", "最小化") => "Minimize",
        ("en", "缩放") => "Zoom",
        ("en", "关闭窗口") => "Close Window",
        ("en", "选择独立、为空的工作区目录") => "Choose a separate, empty workspace folder",
        ("en", "选择本地项目文件夹（空目录或已有项目）") => "Choose a local project folder (empty or existing project)",
        ("en", "选择要检查的项目源码目录") => "Choose the project source folder to inspect",
        ("en", "选择一个空文件夹作为 AI 的工作目录") => "Choose an empty folder for your AI to work in",
        ("en", "保存项目附件") => "Save Project Attachment",
        ("en", "保存项目下载") => "Save Project Download",
        ("en", "保存下载") => "Save Download",
        ("en", "下载未保存：") => "Download was not saved: ",
        ("en", "将在默认浏览器打开：") => "Open in your default browser:",
        ("en", "项目窗口仍保留当前内容。") => "The app window will keep its current content.",
        ("en", "打开外部链接") => "Open External Link",
        ("en", "打开浏览器") => "Open Browser",
        ("en", "确定") => "OK",
        ("en", "取消") => "Cancel",
        ("en", "应用") => "App",
        ("en", "应用 · 正在关闭") => "App · Closing",
        ("en", "应用 · 正在重启") => "App · Restarting",
        ("en", "应用关闭状态") => "App Closing Status",
        ("en", "应用关闭未完整完成，请在‘我的项目 → 详情 → 运行状态’中核对并重试停止；平台不会自动重新启动。") => "The app has not fully closed. Check its runtime status in My Apps → Details and retry stopping it. It will not restart automatically.",
        ("en", "应用关闭尚未完成。请等待文件操作结束，并在应用详情中核对或重试关闭；数据库释放前不会显示已关闭。") => "The app has not fully closed. Wait for file operations to finish, then check or retry closing it in the app details. It will be marked closed once its database is released.",
        ("en", "项目窗口关闭状态") => "App Window Closing Status",
        ("en", "退出尚未完成，部分窗口、后台服务或文件工作仍待确认。请等待相关操作结束后再次通过菜单退出。平台没有强制释放现场。") => "Quitting is still in progress. Some windows, background services, or file operations have not finished. Wait for them, then choose Quit again from the menu. Current work has been preserved.",
        ("en", "正在保护现有工作") => "Protecting Current Work",
        ("en", "熊猫小抽屉 · 工作区无法启动") => "Panda Drawer · Workspace Could Not Start",
        ("en", "平台工作区无法启动：") => "The workspace could not start: ",
        ("en", "请连接目标磁盘或修复上述路径，再退出并重新启动开发版。为保护现有数据，平台没有创建空工作区，也没有切回旧目录。") => "Connect the target disk or repair the path above, then quit and restart the development app. Existing data has been preserved; no empty workspace was created and no previous folder was selected.",
        ("en", "无法使用 Microsoft Edge WebView2 运行时。请安装当前用户可用的 WebView2 Evergreen 运行时，然后重新启动熊猫小抽屉。\n\n官方下载：https://developer.microsoft.com/microsoft-edge/webview2/") => "Microsoft Edge WebView2 is unavailable. Install the WebView2 Evergreen Runtime for the current user, then restart Panda Drawer.\n\nOfficial download: https://developer.microsoft.com/microsoft-edge/webview2/",
        ("en", "熊猫小抽屉 · WebView2 运行时不可用") => "Panda Drawer · WebView2 Runtime Unavailable",
        ("en", "熊猫小抽屉 · 浏览器目录无法核实") => "Panda Drawer · Browser Folder Could Not Be Verified",
        ("en", "熊猫小抽屉 · 本机数据清理结果") => "Panda Drawer · Local Data Cleanup Result",
        ("en", "\n\n尚未清理的路径：\n") => "\n\nPaths not yet cleaned:\n",
        ("en", "\n\n关闭此结果窗口后退出软件。") => "\n\nClose this result window to quit the app.",
        ("en", "本软件管理的本机项目数据和代码已清理。外部原始源码及清理范围之外的独立导出文件保留。请退出软件后卸载。") => "Managed local project data and code have been removed. External original source and completed exports outside the cleanup scope have been preserved. Quit the app before uninstalling.",
        ("en", "清理范围或关闭条件未通过核对，没有开始删除。请退出后检查，再重新确认。") => "Cleanup scope or shutdown checks did not pass. Nothing was deleted. Quit, inspect the issue, then confirm again.",
        ("en", "清理未全部完成，部分内容可能已经永久删除。请核对剩余目录和错误信息，不会自动重试。") => "Cleanup is incomplete and some content may already be permanently deleted. Check the remaining folders and error details. Cleanup will not retry automatically.",
        ("en", "尚未确认所有窗口、服务和文件工作已停止，没有开始删除。请退出后检查保留现场。") => "Some windows, services, or file operations have not been confirmed stopped. Nothing was deleted. Quit and inspect the preserved state.",
        ("en", "关闭过程仍有未解决的问题，没有开始删除。") => "Shutdown still has unresolved issues. Nothing was deleted.",
        ("en", "清理计划不可用，没有开始删除。") => "The cleanup plan is unavailable. Nothing was deleted.",
        ("en", "清理工作意外结束，部分数据可能已经删除。") => "Cleanup ended unexpectedly. Some data may already have been deleted.",
        ("en", "清理工作未正常结束，尚未授予退出许可；请保留剩余现场。") => "Cleanup did not finish normally. Quitting has not been authorized; please preserve the remaining state.",
        _ => source,
    }
}

pub(super) fn app_failure_message(source: &str) -> String {
    if current() == "en" {
        if let Some(status) = source
            .strip_prefix("后端启动检查接口返回了 HTTP ")
            .and_then(|value| value.strip_suffix("，需要返回 HTTP 200。"))
            .and_then(|value| value.parse::<u16>().ok())
        {
            return format!(
                "The backend startup check returned HTTP {status}; HTTP 200 is required."
            );
        }
    }
    text(source).to_owned()
}
