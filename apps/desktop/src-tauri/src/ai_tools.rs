//! Manager-only AI handoff. Paths always come from the current registered app.
use super::{require_manager, ApiError, AppState};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::Manager;

mod plan;
#[cfg(windows)]
#[path = "ai_tools/windows_clipboard.rs"]
mod windows_clipboard;

#[cfg(target_os = "macos")]
#[path = "ai_tools/macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "ai_tools/windows.rs"]
mod platform;
#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::*;
    pub(super) async fn detect() -> Result<Vec<AiTool>, ApiError> {
        Err(unavailable())
    }
    pub(super) async fn locate(_: &str) -> Result<DetectedTool, ApiError> {
        Err(unavailable())
    }
    pub(super) async fn open(tool: &DetectedTool, _: &Path, _: &str) -> OpenResult {
        OpenResult {
            tool_id: "other".into(),
            tool_name: tool.info.name.clone(),
            one_click: false,
            opened: false,
            capability: plan::Capability::Manual,
            prefilled: false,
            warning: Some(plan::warning::TOOL_OPEN_FAILED),
            trace: vec!["unsupported-platform".into()],
        }
    }
    fn unavailable() -> ApiError {
        ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "此平台暂不支持检测和打开 AI 工具。",
        )
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AiTool {
    id: String,
    name: String,
    installed: bool,
    detected_by: Vec<String>,
    one_click: bool,
    /// What one click achieves with this tool on this computer.
    capability: &'static str,
    /// A code the manager translates, never display text.
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_hint: Option<&'static str>,
}

/// A located tool and how this platform hands a folder and prompt to it.
pub(super) struct DetectedTool {
    info: AiTool,
    application: Option<PathBuf>,
    capability: plan::Capability,
    /// The tool's handoff protocol is registered with the OS.
    link_ready: bool,
    /// Protocol of a separate chat link (Qoder IDE, Cursor).
    chat_scheme: Option<&'static str>,
    /// Link that starts the tool when no program path is known.
    launch_url: Option<&'static str>,
    /// Start the tool and wait before sending its task link (WorkBuddy).
    wait_for_start: bool,
    /// Bundle identifier (macOS) or executable path (Windows) for running checks.
    process: Option<String>,
}

impl DetectedTool {
    fn new(id: &str, name: &str, capability: plan::Capability) -> Self {
        Self {
            info: AiTool {
                id: id.into(),
                name: name.into(),
                installed: false,
                detected_by: Vec::new(),
                one_click: false,
                capability: capability.as_str(),
                setup_hint: None,
            },
            application: None,
            capability,
            link_ready: false,
            chat_scheme: None,
            launch_url: None,
            wait_for_start: false,
            process: None,
        }
    }

    fn other() -> Self {
        let mut tool = Self::new("other", "other", plan::Capability::Manual);
        tool.info.installed = true;
        tool
    }

    /// Recompute the summary fields after detection changed the tool.
    fn finish(mut self) -> Self {
        let full = self.capability == plan::Capability::Full;
        self.info.one_click = full && self.link_ready;
        self.info.capability = if full && !self.link_ready {
            plan::Capability::Manual.as_str()
        } else {
            self.capability.as_str()
        };
        self
    }
}

pub(super) struct OpenResult {
    tool_id: String,
    tool_name: String,
    one_click: bool,
    opened: bool,
    capability: plan::Capability,
    prefilled: bool,
    warning: Option<&'static str>,
    /// Steps for the platform log; never contains the prompt.
    trace: Vec<String>,
}

#[tauri::command]
pub(super) async fn detect_ai_tools(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AiTool>, ApiError> {
    require_manager(&window)?;
    state
        .handle
        .io_lifetime()
        .admit()?
        .run(platform::detect())
        .await?
}

#[tauri::command]
pub(super) fn ai_handoff_show_manager(window: tauri::WebviewWindow) -> Result<(), ApiError> {
    require_manager(&window)?;
    super::show_manager(window.app_handle());
    Ok(())
}

fn prepared(handoff: &Value) -> Result<(&str, PathBuf, &str), ApiError> {
    let text = handoff["text"].as_str();
    let root = handoff["projectRoot"].as_str();
    let instance = handoff["instanceId"].as_str();
    match (text, root, instance) {
        (Some(text), Some(root), Some(instance)) if Path::new(root).is_absolute() => {
            Ok((text, PathBuf::from(root), instance))
        }
        _ => Err(ApiError::new(
            "HANDOFF_UNAVAILABLE",
            "暂时无法准备应用说明。",
        )),
    }
}

fn response(result: OpenResult, handoff: &Value, name: &str) -> Value {
    manager_handoff(
        json!({"toolId": result.tool_id, "toolName": result.tool_name,
        "oneClick": result.one_click, "opened": result.opened,
        "capability": result.capability.as_str(), "prefilled": result.prefilled,
        "warningCode": result.warning, "text": handoff["text"],
        "projectRoot": handoff["projectRoot"], "instanceId": handoff["instanceId"], "appName": name}),
    )
}

/// The manager also compares the registered projectRoot when checking for stale results.
pub(super) fn manager_handoff(mut handoff: Value) -> Value {
    if let Some(root) = handoff["projectRoot"].as_str() {
        handoff["displayProjectRoot"] = json!(platform_core::resources::handoff_display_path(
            Path::new(root)
        ));
    }
    handoff
}

/// Short placeholder for task links that cannot exist without a prompt.
fn paste_note() -> &'static str {
    if crate::ui_locale::current() == "en" {
        "Paste the full prompt from Panda Drawer, then review it before sending."
    } else {
        "请粘贴熊猫小抽屉里的完整提示词，检查后再发送。"
    }
}

/// Open and record one dispatch. The log keeps the tool, entry, folder and
/// outcome so a user can report what happened; it never keeps the prompt.
async fn dispatch(
    app: &tauri::AppHandle,
    entry: &str,
    tool: &DetectedTool,
    root: &Path,
    handoff: &mut Value,
) -> OpenResult {
    let text = plan::with_workspace_line(
        entry,
        root,
        handoff["text"].as_str().unwrap_or_default(),
        crate::ui_locale::current() == "en",
    );
    // The copied text is exactly what the tool received.
    handoff["text"] = json!(text);
    let result = platform::open(tool, root, &text).await;
    handoff_log::record(app, &plan::log_line(entry, &tool.info.id, root, &result));
    result
}

mod handoff_log {
    use std::io::Write;
    use tauri::Manager;
    const FILE: &str = "ai-handoff.log";
    const LIMIT: u64 = 256 * 1024;

    /// Best effort: logging never changes a handoff outcome.
    pub(super) fn record(app: &tauri::AppHandle, line: &str) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or_default();
        let line = format!("{seconds} ai-handoff {line}");
        eprintln!("{line}");
        let Ok(directory) = app.path().app_local_data_dir() else {
            return;
        };
        if !directory.is_dir() {
            return;
        }
        let path = directory.join(FILE);
        if std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| !metadata.file_type().is_file() || metadata.len() > LIMIT)
        {
            let _ = std::fs::rename(&path, directory.join(format!("{FILE}.1")));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn prepare_handoff_text(handoff: &mut Value, supplied: Option<&str>, locale: &str) {
    if let Some(text) = supplied {
        handoff["text"] = json!(text);
    } else if let Some(text) = handoff["text"].as_str() {
        handoff["text"] = json!(platform_core::ai_prompt_rules::localize_handoff(
            text, locale
        ));
    }
}

const APP_TEXT_LIMIT: usize = 2_000;
const MANAGER_TEXT_LIMIT: usize = 20_000;

#[cfg(windows)]
#[tauri::command]
pub(super) async fn copy_ai_handoff_text(
    window: tauri::WebviewWindow,
    text: String,
) -> Result<(), ApiError> {
    require_manager(&window)?;
    let value = windows_clipboard::unicode_text(&text)?;
    tauri::async_runtime::spawn_blocking(move || windows_clipboard::write(&window, &value))
        .await
        .map_err(|_| windows_clipboard::unavailable())?
}

#[tauri::command]
pub(super) async fn open_ai_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    tool_id: String,
    project_id: Option<String>,
    app_id: Option<String>,
    source_path: Option<String>,
    expected_window_generation: Option<u64>,
    purpose: Option<String>,
    text: Option<String>,
    entry: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let entry = plan::entry_label(entry.as_deref());
    if let Some(source_path) = source_path {
        if project_id.is_some() || app_id.is_some() {
            return Err(ApiError::new(
                "HANDOFF_APP_INVALID",
                "请选一个应用或文件夹。",
            ));
        }
        let text =
            text.ok_or_else(|| ApiError::new("HANDOFF_TEXT_INVALID", "缺少文件夹处理说明。"))?;
        return directory_handoff(window, state, tool_id, source_path, text, entry).await;
    }
    // Only the manager passes a text: the create prompt, a repair explanation it
    // built, or what an app page asked the platform to hand over. The folder
    // always comes from the registration.
    let repair = purpose.as_deref() == Some("repair");
    let limit = if repair {
        MANAGER_TEXT_LIMIT
    } else {
        APP_TEXT_LIMIT
    };
    let text = match text {
        Some(text) if text.trim().is_empty() || text.chars().count() > limit => {
            return Err(ApiError::new(
                "HANDOFF_TEXT_INVALID",
                "提示词是空的，或者太长了。",
            ));
        }
        None if repair => {
            return Err(ApiError::new(
                "HANDOFF_TEXT_INVALID",
                "提示词是空的，或者太长了。",
            ));
        }
        other => other,
    };
    let io_lease = state.handle.io_lifetime().admit()?;
    let app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = app.state::<AppState>();
            let id = match (project_id, app_id) {
                (Some(id), None) | (None, Some(id)) => id,
                _ => return Err(ApiError::new("HANDOFF_APP_INVALID", "请先选一个应用。")),
            };
            if let Some(generation) = expected_window_generation {
                super::app_reload::validate_window_action(&app, &id, generation)?;
            }
            let mut handoff = if purpose.as_deref() == Some("create") {
                state.handle.create_handoff(&id).await?
            } else {
                state.handle.handoff(&id).await?
            };
            prepare_handoff_text(&mut handoff, text.as_deref(), crate::ui_locale::current());
            let (_, root, instance) = prepared(&handoff)?;
            let lease = state.handle.editor_handoff_lease(&id, &root, instance)?;
            let tool = platform::locate(&tool_id).await?;
            lease.validate()?;
            if let Some(generation) = expected_window_generation {
                super::app_reload::validate_window_action(&app, &id, generation)?;
            }
            let name = app_name(&root).unwrap_or_else(|| "应用".into());
            let result = dispatch(&app, &entry, &tool, &root, &mut handoff).await;
            Ok(response(result, &handoff, &name))
        })
        .await?
}

#[tauri::command]
pub(super) async fn ai_directory_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    tool_id: String,
    source_path: String,
    text: String,
    entry: Option<String>,
) -> Result<Value, ApiError> {
    let entry = plan::entry_label(entry.as_deref());
    directory_handoff(window, state, tool_id, source_path, text, entry).await
}

async fn directory_handoff(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    tool_id: String,
    source_path: String,
    text: String,
    entry: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    if text.trim().is_empty() || text.chars().count() > MANAGER_TEXT_LIMIT {
        return Err(ApiError::new(
            "HANDOFF_TEXT_INVALID",
            "提示词是空的，或者太长了。",
        ));
    }
    let info = state.handle.info();
    // Every caller now supplies the app's own folder: the selected author
    // folder or a platform reservation. Parent download folders are refused.
    let root = PathBuf::from(source_path);
    if !root.is_absolute() {
        return Err(ApiError::new(
            "HANDOFF_DIRECTORY_INVALID",
            "请选择完整的应用文件夹路径。",
        ));
    }
    if root == info.workspace_root || root == info.workspace_root.join("project-sources") {
        return Err(ApiError::new(
            "HANDOFF_DIRECTORY_INVALID",
            "请选择这个应用自己的文件夹。",
        ));
    }
    let instance = info.instance_id;
    platform_core::projects::check_no_symlinks(&root, false)?;
    let identity = super::desktop_path_io::directory_identity(&root)?
        .ok_or_else(|| ApiError::new("HANDOFF_DIRECTORY_MISSING", "找不到应用文件夹。"))?;
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    let app = window.app_handle().clone();
    lease
        .run(async move {
            let tool = platform::locate(&tool_id).await?;
            handle.io_lifetime().check_open()?;
            if handle.info().instance_id != instance {
                return Err(ApiError::new(
                    "HANDOFF_CHANGED",
                    "存放位置变了，请再点一次。",
                ));
            }
            platform_core::projects::check_no_symlinks(&root, false)?;
            if super::desktop_path_io::directory_identity(&root)? != Some(identity) {
                return Err(ApiError::new(
                    "HANDOFF_CHANGED",
                    "应用文件夹变了，请再点一次。",
                ));
            }
            let text = platform_core::ai_prompt_rules::localize_handoff(
                &text,
                crate::ui_locale::current(),
            );
            let mut handoff =
                json!({"text":text,"projectRoot":root,"instanceId":handle.info().instance_id});
            let name = root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("应用")
                .to_owned();
            let result = dispatch(&app, &entry, &tool, &root, &mut handoff).await;
            Ok(response(result, &handoff, &name))
        })
        .await?
}

fn app_name(root: &Path) -> Option<String> {
    platform_core::panda_manifest::PandaManifest::load(root)
        .ok()
        .map(|manifest| manifest.app.name)
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use platform_core::ai_prompt_rules::{render, TEMPLATE_CREATE};

    #[cfg(windows)]
    #[test]
    fn manager_and_sdk_handoffs_display_the_folder_without_altering_the_registration_or_supplied_text(
    ) {
        let supplied = r"原文保留 \\?\C:\example 和 {projectRoot}";
        for (raw, shown) in [
            (r"\\?\C:\项目 有空格", r"C:\项目 有空格"),
            (r"\\?\UNC\server\share\项目", r"\\server\share\项目"),
            (r"\\?\C:\special.", r"\\?\C:\special."),
        ] {
            let original = json!({"projectRoot":raw,"text":supplied,"instanceId":"fixture"});
            let (_, registered, _) = prepared(&original).unwrap();
            assert_eq!(registered, Path::new(raw));
            let manager = manager_handoff(original);
            assert_eq!(manager["projectRoot"], raw);
            assert_eq!(manager["displayProjectRoot"], shown);
            assert_eq!(manager["text"], supplied);
            for (english, label) in [
                (true, "This app's folder is "),
                (false, "这个应用的文件夹是 "),
            ] {
                let prompt = plan::with_workspace_line("app-page", &registered, supplied, english);
                assert!(prompt.starts_with(&format!("{supplied}\n\n{label}{shown}")));
            }
        }
    }

    #[test]
    fn native_open_translates_generated_text_but_preserves_app_supplied_text() {
        let source = render(
            TEMPLATE_CREATE,
            &[
                ("nameLine", "应用：中文应用\n"),
                ("descriptionLine", ""),
                ("projectRoot", "/disposable/中文/{folder}"),
                ("skillPath", "/disposable/SKILL.md"),
                ("cliPath", "/disposable/panda-cli"),
            ],
        );
        let mut handoff = json!({"text": source, "projectRoot": "/disposable/中文/{folder}"});
        prepare_handoff_text(&mut handoff, None, "en");
        assert!(handoff["text"]
            .as_str()
            .unwrap()
            .starts_with("Application: 中文应用\nI want to make this new app in Panda Drawer."));
        assert_eq!(handoff["projectRoot"], "/disposable/中文/{folder}");
        // Even a full recognizable scaffold belongs to the app when it supplies text.
        prepare_handoff_text(&mut handoff, Some(&source), "en");
        assert_eq!(handoff["text"], source);
    }
}
