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
    purpose: Option<String>,
    text: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
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
            match (project_id, app_id) {
                (Some(id), None) => {
                    let mut handoff = if purpose.as_deref() == Some("create") {
                        state.handle.create_handoff(&id).await?
                    } else {
                        state.handle.handoff(&id).await?
                    };
                    prepare_handoff_text(
                        &mut handoff,
                        text.as_deref(),
                        crate::ui_locale::current(),
                    );
                    let (text, root, instance) = prepared(&handoff)?;
                    let lease = state.handle.editor_handoff_lease(&id, &root, instance)?;
                    let tool = platform::locate(&tool_id).await?;
                    lease.validate()?;
                    let name = root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("应用");
                    let name = app_name(&root).unwrap_or_else(|| name.into());
                    let result = platform::open(&tool, &root, text).await;
                    Ok(response(result, &handoff, &name))
                }
                (None, Some(_)) if text.is_some() => Err(ApiError::new(
                    "HANDOFF_TEXT_INVALID",
                    "只有网页应用可以请求打开 AI 工具。",
                )),
                (None, Some(id)) => {
                    let _instance = state.node_windows.instance_operation(&id).await;
                    let _operation = state.node_apps.admit_operation(&id, "交给 AI 修改")?;
                    let mut handoff = if purpose.as_deref() == Some("create") {
                        state
                            .handle
                            .node_app_create_handoff(&state.node_apps, &id)
                            .await?
                    } else {
                        state.handle.node_app_handoff(&state.node_apps, &id).await?
                    };
                    prepare_handoff_text(&mut handoff, None, crate::ui_locale::current());
                    let (text, root, _) = prepared(&handoff)?;
                    let detail = state.node_apps.status(&id).await?;
                    let tool = platform::locate(&tool_id).await?;
                    if state.node_apps.project_source(&id).await? != root {
                        return Err(ApiError::new(
                            "HANDOFF_CHANGED",
                            "项目文件夹已变化，请重新交接。",
                        ));
                    }
                    let result = platform::open(&tool, &root, text).await;
                    Ok(response(
                        result,
                        &handoff,
                        detail["name"].as_str().unwrap_or("应用"),
                    ))
                }
                _ => Err(ApiError::new(
                    "HANDOFF_APP_INVALID",
                    "请选择一个要交给 AI 的应用。",
                )),
            }
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
