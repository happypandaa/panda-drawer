//! Manager-only AI handoff. Paths always come from the current registered app.
use super::{require_manager, ApiError, AppState};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::Manager;

#[cfg(target_os = "macos")]
#[path = "ai_tools/macos.rs"]
mod platform;
#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    pub(super) async fn detect() -> Result<Vec<AiTool>, ApiError> {
        Err(unavailable())
    }
    pub(super) async fn locate(_: &str) -> Result<DetectedTool, ApiError> {
        Err(unavailable())
    }
    pub(super) async fn open(_: &DetectedTool, _: &Path, _: &str) -> OpenResult {
        OpenResult {
            tool_id: "other".into(),
            tool_name: "其他 AI 工具".into(),
            one_click: false,
            opened: false,
            warning: Some("此平台暂不支持自动交接，请复制提示词后手动打开 AI 工具。".into()),
        }
    }
    fn unavailable() -> ApiError {
        ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "此平台暂不支持 AI 工具检测和自动交接。",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_hint: Option<String>,
}

struct DetectedTool {
    info: AiTool,
    application: Option<PathBuf>,
}

struct OpenResult {
    tool_id: String,
    tool_name: String,
    one_click: bool,
    opened: bool,
    warning: Option<String>,
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
    json!({"toolId": result.tool_id, "toolName": result.tool_name,
        "oneClick": result.one_click, "opened": result.opened,
        "warning": result.warning, "text": handoff["text"],
        "projectRoot": handoff["projectRoot"], "instanceId": handoff["instanceId"], "appName": name})
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
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    if let Some(source_path) = source_path {
        if project_id.is_some() || app_id.is_some() {
            return Err(ApiError::new(
                "HANDOFF_APP_INVALID",
                "请使用应用或文件夹中的一个交接目标。",
            ));
        }
        let text =
            text.ok_or_else(|| ApiError::new("HANDOFF_TEXT_INVALID", "缺少文件夹处理说明。"))?;
        return ai_directory_handoff(window, state, tool_id, source_path, text).await;
    }
    // Only the manager passes a text: the create prompt or what an app page asked
    // the platform to hand over. The folder always comes from the registration.
    let text = match text {
        Some(text) if text.trim().is_empty() || text.chars().count() > 2000 => {
            return Err(ApiError::new(
                "HANDOFF_TEXT_INVALID",
                "交给 AI 的文字不能为空，也不能超过 2000 字。",
            ))
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
                _ => {
                    return Err(ApiError::new(
                        "HANDOFF_APP_INVALID",
                        "请选择一个要交给 AI 的应用。",
                    ))
                }
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
            let (text, root, instance) = prepared(&handoff)?;
            let lease = state.handle.editor_handoff_lease(&id, &root, instance)?;
            let tool = platform::locate(&tool_id).await?;
            lease.validate()?;
            if let Some(generation) = expected_window_generation {
                super::app_reload::validate_window_action(&app, &id, generation)?;
            }
            let name = app_name(&root).unwrap_or_else(|| "应用".into());
            let result = platform::open(&tool, &root, text).await;
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
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    if text.trim().is_empty() || text.chars().count() > 20000 {
        return Err(ApiError::new(
            "HANDOFF_TEXT_INVALID",
            "交给 AI 的说明不能为空或超过长度限制。",
        ));
    }
    let info = state.handle.info();
    let mut root = PathBuf::from(source_path);
    // A failed first download may precede creation of the source container.
    // Open the existing selected workspace so the AI can reserve its own app
    // directory; an arbitrary missing author folder never falls back elsewhere.
    if root == info.workspace_root.join("project-sources")
        && matches!(std::fs::symlink_metadata(&root), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        root = info.workspace_root;
    }
    let instance = info.instance_id;
    platform_core::projects::check_no_symlinks(&root, false)?;
    let identity = super::desktop_path_io::directory_identity(&root)?
        .ok_or_else(|| ApiError::new("HANDOFF_DIRECTORY_MISSING", "项目文件夹不存在。"))?;
    if !root.is_absolute() {
        return Err(ApiError::new(
            "HANDOFF_DIRECTORY_INVALID",
            "请选择完整的项目文件夹路径。",
        ));
    }
    let lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    lease
        .run(async move {
            let tool = platform::locate(&tool_id).await?;
            handle.io_lifetime().check_open()?;
            if handle.info().instance_id != instance {
                return Err(ApiError::new("HANDOFF_CHANGED", "平台工作区已变化，请重新交接。"));
            }
            platform_core::projects::check_no_symlinks(&root, false)?;
            if super::desktop_path_io::directory_identity(&root)? != Some(identity) {
                return Err(ApiError::new(
                    "HANDOFF_CHANGED",
                    "项目文件夹已变化，请重新交接。",
                ));
            }
            let text = platform_core::ai_prompt_rules::localize_handoff(
                &text,
                crate::ui_locale::current(),
            );
            let handoff =
                json!({"text":text,"projectRoot":root,"instanceId":handle.info().instance_id});
            let name = root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("应用")
                .to_owned();
            let result = platform::open(&tool, &root, &text).await;
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
            .starts_with("I want to make a new app"));
        assert_eq!(handoff["projectRoot"], "/disposable/中文/{folder}");
        // Even a full recognizable scaffold belongs to the app when it supplies text.
        prepare_handoff_text(&mut handoff, Some(&source), "en");
        assert_eq!(handoff["text"], source);
    }
}
