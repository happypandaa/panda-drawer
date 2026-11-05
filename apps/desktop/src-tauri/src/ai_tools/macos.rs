use super::{AiTool, ApiError, DetectedTool, OpenResult};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSBundle, NSString, NSURL};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

struct ToolSpec {
    id: &'static str,
    name: &'static str,
    scheme: &'static str,
    command: Option<&'static str>,
    bundle: Option<&'static str>,
    one_click: bool,
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        id: "codex",
        name: "Codex",
        scheme: "codex",
        command: Some("codex"),
        bundle: Some("com.openai.codex"),
        one_click: true,
    },
    ToolSpec {
        id: "claude-code",
        name: "Claude Code（命令行）",
        scheme: "claude-cli",
        command: Some("claude"),
        bundle: None,
        one_click: true,
    },
    ToolSpec {
        id: "claude",
        name: "Claude",
        scheme: "claude",
        command: None,
        bundle: None,
        one_click: true,
    },
    ToolSpec {
        id: "workbuddy",
        name: "WorkBuddy",
        scheme: "workbuddy",
        command: None,
        bundle: Some("com.tencent.workbuddy.mac"),
        one_click: true,
    },
    ToolSpec {
        id: "cursor",
        name: "Cursor",
        scheme: "cursor",
        command: Some("cursor"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "qoder",
        name: "Qoder",
        scheme: "qoder",
        // The standalone qoder/qodercn CLIs are separate products, not desktop launchers.
        command: None,
        bundle: Some("com.qoder.app"),
        one_click: false,
    },
    ToolSpec {
        id: "vscode",
        name: "VS Code",
        scheme: "vscode",
        command: Some("code"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "trae",
        name: "Trae",
        scheme: "trae",
        command: Some("trae"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "trae-cn",
        name: "Trae CN",
        scheme: "trae-cn",
        command: Some("trae-cn"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "windsurf",
        name: "Windsurf",
        scheme: "windsurf",
        command: Some("windsurf"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "kiro",
        name: "Kiro",
        scheme: "kiro",
        command: Some("kiro"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "codebuddy",
        name: "CodeBuddy",
        scheme: "codebuddy",
        command: Some("codebuddy"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "codebuddy-cn",
        name: "CodeBuddy CN",
        scheme: "codebuddycn",
        command: Some("codebuddycn"),
        bundle: None,
        one_click: false,
    },
    ToolSpec {
        id: "lingma",
        name: "通义灵码",
        scheme: "lingma",
        command: Some("lingma"),
        bundle: None,
        one_click: false,
    },
];

const QODER_CN: ToolSpec = ToolSpec {
    id: "qoder",
    name: "Qoder",
    scheme: "qoder-cn",
    command: None,
    bundle: Some("com.qodercn.app"),
    one_click: false,
};

fn qoder_workbench(application: &Path) -> bool {
    application
        .to_str()
        .and_then(|path| NSBundle::bundleWithPath(&NSString::from_str(path)))
        .and_then(|bundle| bundle.bundleIdentifier())
        .is_some_and(|id| matches!(id.to_string().as_str(), "com.qoder.app" | "com.qodercn.app"))
}

fn project_argument<'a>(tool: &DetectedTool, root: &'a Path) -> Option<&'a std::ffi::OsStr> {
    let workbench =
        tool.info.id == "qoder" && tool.application.as_deref().is_some_and(qoder_workbench);
    (!workbench).then_some(root.as_os_str())
}

fn locate_tool(spec: &ToolSpec) -> DetectedTool {
    if spec.id != "qoder" {
        return locate_spec(spec);
    }
    // Prefer the new desktop workbench, then a registered IDE. Both editions
    // share the existing Qoder preference and produce a single chooser entry.
    select_qoder([locate_spec(spec), locate_spec(&QODER_CN)])
}

fn select_qoder(candidates: [DetectedTool; 2]) -> DetectedTool {
    let selected = candidates
        .iter()
        .position(|tool| tool.application.as_deref().is_some_and(qoder_workbench))
        .or_else(|| candidates.iter().position(|tool| tool.info.installed))
        .unwrap_or(0);
    candidates.into_iter().nth(selected).unwrap()
}

fn locate_spec(spec: &ToolSpec) -> DetectedTool {
    let workspace = NSWorkspace::sharedWorkspace();
    let protocol = NSURL::URLWithString(&NSString::from_str(&format!("{}://", spec.scheme)))
        .and_then(|url| workspace.URLForApplicationToOpenURL(&url));
    let bundle = spec
        .bundle
        .and_then(|id| workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(id)));
    let mut detected_by = Vec::new();
    if protocol.is_some() {
        detected_by.push(format!("protocol:{}", spec.scheme));
    }
    if let Some(id) = spec.bundle.filter(|_| bundle.is_some()) {
        detected_by.push(format!("bundle:{id}"));
    }
    let application = if spec.id == "qoder" {
        bundle.as_ref().or(protocol.as_ref())
    } else {
        protocol.as_ref().or(bundle.as_ref())
    }
    .and_then(|url| url.path())
    .map(|path| PathBuf::from(path.to_string()));
    let application = application.filter(|path| path.is_dir());
    let one_click = spec.one_click && protocol.is_some() && application.is_some();
    DetectedTool {
        info: AiTool {
            id: spec.id.into(),
            name: spec.name.into(),
            installed: application.is_some(),
            detected_by,
            one_click,
            setup_hint: None,
        },
        application,
    }
}

pub(super) async fn locate(id: &str) -> Result<DetectedTool, ApiError> {
    if id == "other" {
        return Ok(other());
    }
    let spec = TOOLS
        .iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| ApiError::new("AI_TOOL_UNKNOWN", "无法识别这个 AI 工具，请重新选择。"))?;
    let mut tool = locate_tool(spec);
    if !tool.info.installed {
        if let Some(name) = spec.command {
            let mut command = Command::new("/bin/zsh");
            command
                .args(["-lc", &format!("command -v {name} >/dev/null 2>&1")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if super::super::native_process::status(command, Duration::from_secs(3))
                .await
                .is_ok_and(|status| status.success())
            {
                tool.info.installed = true;
                tool.info.detected_by.push(format!("command:{name}"));
            }
        }
    }
    Ok(tool)
}

fn other() -> DetectedTool {
    DetectedTool {
        info: AiTool {
            id: "other".into(),
            name: "其他 AI 工具".into(),
            installed: true,
            detected_by: Vec::new(),
            one_click: false,
            setup_hint: None,
        },
        application: None,
    }
}

pub(super) async fn detect() -> Result<Vec<AiTool>, ApiError> {
    // Fixed command names only; no app paths or user text are evaluated by the shell.
    let mut script = String::new();
    for spec in TOOLS {
        if let Some(command) = spec.command {
            script.push_str(&format!(
                "if command -v {command} >/dev/null 2>&1; then printf '%s\\n' '{command}'; fi\n"
            ));
        }
    }
    let mut command = Command::new("/bin/zsh");
    command
        .args(["-lc", &script])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let commands = super::super::native_process::output(command, Duration::from_secs(5))
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| (line.to_owned(), ()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut tools = Vec::new();
    for spec in TOOLS {
        let mut found = locate_tool(spec).info;
        if let Some(command) = spec
            .command
            .filter(|command| commands.contains_key(*command))
        {
            found.installed = true;
            found.detected_by.push(format!("command:{command}"));
            if spec.id == "claude-code" && !found.one_click {
                found.setup_hint =
                    Some("先在终端运行一次 claude 并发送任意内容，即可启用一键打开。".into());
            }
        }
        tools.push(found);
    }
    tools.sort_by_key(|tool| !tool.installed);
    Ok(tools)
}

fn deep_link(tool: &DetectedTool, root: &Path, text: &str) -> Option<tauri::Url> {
    let root = root.to_str().filter(|_| root.is_absolute())?;
    let (base, path_key, prompt_key) = match tool.info.id.as_str() {
        "codex" => ("codex://new", "path", "prompt"),
        "claude-code" => ("claude-cli://open", "cwd", "q"),
        "claude" => ("claude://code/new", "folder", "q"),
        "workbuddy" => ("workbuddy://task", "cwd", "prompt"),
        _ => return None,
    };
    let mut url = tauri::Url::parse(base).ok()?;
    if tool.info.id == "workbuddy" {
        url.query_pairs_mut().append_pair("action", "start");
    }
    url.query_pairs_mut()
        .append_pair(path_key, root)
        .append_pair(prompt_key, text);
    // Deep links may use URI decoders rather than form decoders. Encode spaces
    // as %20; literal plus characters have already been encoded as %2B.
    let query = url.query()?.replace('+', "%20");
    url.set_query(Some(&query));
    Some(url)
}

async fn dispatch(application: Option<&Path>, value: Option<&std::ffi::OsStr>) -> bool {
    let mut command = Command::new("/usr/bin/open");
    if let Some(application) = application {
        command.arg("-a").arg(application);
    }
    if let Some(value) = value {
        command.arg(value);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    super::super::native_process::status(command, Duration::from_secs(5))
        .await
        .is_ok_and(|status| status.success())
}

pub(super) async fn open(tool: &DetectedTool, root: &Path, text: &str) -> OpenResult {
    if tool.info.id != "other" {
        if tool.info.one_click {
            if let Some(url) = deep_link(tool, root, text) {
                if dispatch(
                    tool.application.as_deref(),
                    Some(std::ffi::OsStr::new(url.as_str())),
                )
                .await
                {
                    return OpenResult {
                        tool_id: tool.info.id.clone(),
                        tool_name: tool.info.name.clone(),
                        one_click: true,
                        opened: true,
                        warning: None,
                    };
                }
            }
        } else if tool.application.is_some()
            && tool.info.id != "claude-code"
            && dispatch(
                tool.application.as_deref(),
                // New Qoder workbenches treat file arguments as attachments and
                // reject directories. Launch without one; the guide asks the user
                // to select this project's folder before pasting the prompt.
                project_argument(tool, root),
            )
            .await
        {
            return OpenResult {
                tool_id: tool.info.id.clone(),
                tool_name: tool.info.name.clone(),
                one_click: false,
                opened: true,
                warning: None,
            };
        }
    }
    let opened = dispatch(None, Some(root.as_os_str())).await;
    let warning = if tool.info.id == "other" {
        (!opened).then(|| "项目文件夹未能打开，请按下方路径手动打开，并复制提示词。".into())
    } else if !tool.info.installed {
        Some(format!(
            "没找到 {}，可以重新选择。请手动打开 AI 工具并粘贴提示词。",
            tool.info.name
        ))
    } else {
        Some(format!(
            "未能自动在 {} 中打开，请手动选择项目文件夹并粘贴提示词。",
            tool.info.name
        ))
    };
    OpenResult {
        tool_id: "other".into(),
        tool_name: "其他 AI 工具".into(),
        one_click: false,
        opened,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qoder_fixture(bundle_id: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        // Bundle identity must still work if the user renames the application.
        let application = directory.path().join("Renamed.app");
        std::fs::create_dir_all(application.join("Contents")).unwrap();
        std::fs::write(
            application.join("Contents/Info.plist"),
            format!(
                "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
                 <key>CFBundleIdentifier</key><string>{bundle_id}</string>\
                 <key>CFBundlePackageType</key><string>APPL</string>\
                 </dict></plist>"
            ),
        )
        .unwrap();
        (directory, application)
    }

    fn detected_qoder(spec: &ToolSpec, application: Option<PathBuf>) -> DetectedTool {
        let mut tool = other();
        tool.info.id = spec.id.into();
        tool.info.name = spec.name.into();
        tool.info.one_click = spec.one_click;
        tool.info.installed = application.is_some();
        tool.application = application;
        tool
    }

    #[test]
    fn qoder_editions_do_not_receive_project_directories_as_attachments() {
        let root = Path::new("/tmp/可丢弃 项目/+?#");
        let global = TOOLS.iter().find(|spec| spec.id == "qoder").unwrap();
        for spec in [global, &QODER_CN] {
            let (_directory, application) = qoder_fixture(spec.bundle.unwrap());
            let tool = detected_qoder(spec, Some(application));
            assert_eq!(project_argument(&tool, root), None, "{}", spec.scheme);
            assert!(!tool.info.one_click);
        }
        let (_directory, application) = qoder_fixture("example.qoder.ide");
        let tool = detected_qoder(global, Some(application));
        assert_eq!(project_argument(&tool, root), Some(root.as_os_str()));
    }

    #[test]
    fn qoder_uses_one_name_and_supports_either_edition_with_workbench_priority() {
        let (_global_directory, global) = qoder_fixture("com.qoder.app");
        let (_cn_directory, cn) = qoder_fixture("com.qodercn.app");
        let (_ide_directory, ide) = qoder_fixture("example.qoder.ide");
        let global_spec = TOOLS.iter().find(|spec| spec.id == "qoder").unwrap();
        for (candidates, expected) in [
            ([Some(global.clone()), None], global.clone()),
            ([None, Some(cn.clone())], cn.clone()),
            ([Some(global.clone()), Some(cn.clone())], global.clone()),
            ([Some(ide), Some(cn.clone())], cn),
        ] {
            let [global, cn] = candidates;
            let tool = select_qoder([
                detected_qoder(global_spec, global),
                detected_qoder(&QODER_CN, cn),
            ]);
            assert_eq!(tool.application, Some(expected));
            assert_eq!(tool.info.id, "qoder");
            assert_eq!(tool.info.name, "Qoder");
        }
        let missing = select_qoder([
            detected_qoder(global_spec, None),
            detected_qoder(&QODER_CN, None),
        ]);
        assert!(!missing.info.installed);
    }

    #[test]
    fn handoff_keeps_prompt_and_directory_as_separate_encoded_values() {
        let mut tool = other();
        tool.info.id = "workbuddy".into();
        let root = Path::new("/tmp/可丢弃 项目/#?&prompt=oops");
        let text = "请先阅读\n不要自动修改 & cwd=/wrong";
        let url = deep_link(&tool, root, text).unwrap();
        let values: BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(values["cwd"], root.to_str().unwrap());
        assert_eq!(values["prompt"], text);
        assert_eq!(values["action"], "start");
    }
}
