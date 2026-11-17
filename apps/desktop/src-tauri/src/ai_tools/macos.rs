//! macOS tool detection (Launch Services, bundle identifiers, login-shell
//! commands) and native actions (`/usr/bin/open`). Handoff rules live in plan.rs.
use super::{plan, plan::Capability, AiTool, ApiError, DetectedTool, OpenResult};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSBundle, NSString, NSURL};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

struct ToolSpec {
    id: &'static str,
    name: &'static str,
    scheme: Option<&'static str>,
    command: Option<&'static str>,
    bundles: &'static [&'static str],
    /// Application names looked up in /Applications and ~/Applications.
    names: &'static [&'static str],
    capability: Capability,
    chat_scheme: Option<&'static str>,
    wait_for_start: bool,
}

const fn spec(id: &'static str, name: &'static str, capability: Capability) -> ToolSpec {
    ToolSpec {
        id,
        name,
        scheme: None,
        command: None,
        bundles: &[],
        names: &[],
        capability,
        chat_scheme: None,
        wait_for_start: false,
    }
}

const TOOLS: &[ToolSpec] = &[
    // ChatGPT.app replaced Codex.app and keeps its bundle ID and codex:// links.
    ToolSpec {
        scheme: Some("codex"),
        bundles: &["com.openai.codex"],
        ..spec("codex", "ChatGPT（Codex）", Capability::Full)
    },
    ToolSpec {
        scheme: Some("claude"),
        bundles: &["com.anthropic.claudefordesktop"],
        ..spec("claude", "Claude 桌面版", Capability::Full)
    },
    ToolSpec {
        scheme: Some("workbuddy"),
        bundles: &["com.tencent.workbuddy.mac"],
        wait_for_start: true,
        ..spec("workbuddy", "WorkBuddy", Capability::Full)
    },
    // Qoder is resolved by `locate_qoder`; this entry carries its chooser identity.
    spec("qoder", "Qoder", Capability::Folder),
    ToolSpec {
        scheme: Some("cursor"),
        command: Some("cursor"),
        chat_scheme: Some("cursor"),
        ..spec("cursor", "Cursor", Capability::FolderPrefill)
    },
    ToolSpec {
        scheme: Some("vscode"),
        command: Some("code"),
        ..spec("vscode", "VS Code", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("trae"),
        command: Some("trae"),
        ..spec("trae", "Trae", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("trae-cn"),
        command: Some("trae-cn"),
        ..spec("trae-cn", "Trae CN", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("windsurf"),
        command: Some("windsurf"),
        ..spec("windsurf", "Windsurf", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("kiro"),
        command: Some("kiro"),
        ..spec("kiro", "Kiro", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("codebuddy"),
        command: Some("codebuddy"),
        ..spec("codebuddy", "CodeBuddy", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("codebuddycn"),
        command: Some("codebuddycn"),
        ..spec("codebuddy-cn", "CodeBuddy CN", Capability::Folder)
    },
    ToolSpec {
        scheme: Some("lingma"),
        command: Some("lingma"),
        ..spec("lingma", "通义灵码", Capability::Folder)
    },
];

/// Qoder desktop apps (separate from the IDE) take no folder argument.
const QODER_DESKTOP_BUNDLES: &[&str] = &["com.qoder.app", "com.qodercn.app"];

fn bundle_id(application: &Path) -> Option<String> {
    application
        .to_str()
        .and_then(|path| NSBundle::bundleWithPath(&NSString::from_str(path)))
        .and_then(|bundle| bundle.bundleIdentifier())
        .map(|id| id.to_string())
}

/// An application Launch Services still remembers may have been deleted or
/// moved to the Trash; neither counts as installed.
fn usable_application(path: &Path) -> bool {
    path.is_dir()
        && !path
            .components()
            .any(|part| matches!(part.as_os_str().to_str(), Some(".Trash" | ".Trashes")))
}

fn url_path(url: Option<objc2::rc::Retained<NSURL>>) -> Option<PathBuf> {
    url.and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .filter(|path| usable_application(path))
}

fn protocol_application(scheme: &str) -> Option<PathBuf> {
    let workspace = NSWorkspace::sharedWorkspace();
    url_path(
        NSURL::URLWithString(&NSString::from_str(&format!("{scheme}://")))
            .and_then(|url| workspace.URLForApplicationToOpenURL(&url)),
    )
}

fn bundle_application(id: &str) -> Option<PathBuf> {
    url_path(
        NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(id)),
    )
}

fn named_application(name: &str) -> Option<PathBuf> {
    let mut directories = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join("Applications"));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_dir())
}

fn locate_spec(spec: &ToolSpec) -> DetectedTool {
    let mut tool = DetectedTool::new(spec.id, spec.name, spec.capability);
    let protocol = spec.scheme.and_then(protocol_application);
    if let (Some(scheme), Some(_)) = (spec.scheme, &protocol) {
        tool.info.detected_by.push(format!("protocol:{scheme}"));
    }
    let bundle = spec.bundles.iter().find_map(|id| {
        bundle_application(id).inspect(|_| tool.info.detected_by.push(format!("bundle:{id}")))
    });
    let named = spec.names.iter().find_map(|name| named_application(name));
    tool.link_ready = protocol.is_some();
    tool.application = protocol.or(bundle).or(named);
    tool.info.installed = tool.application.is_some();
    tool.process = tool.application.as_deref().and_then(bundle_id);
    tool.chat_scheme = spec.chat_scheme;
    tool.wait_for_start = spec.wait_for_start;
    tool.finish()
}

fn locate_qoder() -> DetectedTool {
    let mut candidates = Vec::new();
    // The IDE registers the documented qoder:// handler. Qoder CN.app (desktop,
    // com.qodercn.app) owns qoder-cn://. A handler owned by a desktop bundle is
    // not an IDE and never receives a folder argument.
    for (scheme, names) in [
        ("qoder", &["Qoder IDE.app"][..]),
        ("qoder-cn", &["Qoder CN IDE.app"][..]),
    ] {
        let protocol = protocol_application(scheme);
        let linked = protocol.is_some();
        let application =
            protocol.or_else(|| names.iter().find_map(|name| named_application(name)));
        if let Some(application) = application {
            let bundle = bundle_id(&application);
            let kind = if bundle
                .as_deref()
                .is_some_and(|id| QODER_DESKTOP_BUNDLES.contains(&id))
            {
                plan::QoderKind::Desktop
            } else {
                plan::QoderKind::Ide
            };
            // An IDE found only by name is assumed to use its edition's prefix.
            let chat = (linked || kind == plan::QoderKind::Ide).then_some(scheme);
            candidates.push((kind, (application, bundle, chat)));
        }
    }
    for id in QODER_DESKTOP_BUNDLES {
        if let Some(application) = bundle_application(id) {
            let chat = ["qoder-cn", "qoder"].into_iter().find(|scheme| {
                protocol_application(scheme).as_deref() == Some(application.as_path())
            });
            candidates.push((
                plan::QoderKind::Desktop,
                (application, Some(id.to_string()), chat),
            ));
        }
    }
    let Some((kind, (application, bundle, chat))) = plan::select_qoder(candidates) else {
        return DetectedTool::new("qoder", "Qoder", Capability::Folder).finish();
    };
    let capability = match kind {
        plan::QoderKind::Ide => Capability::FolderPrefill,
        plan::QoderKind::Desktop => Capability::Launch,
    };
    let mut tool = DetectedTool::new("qoder", "Qoder", capability);
    tool.info.installed = true;
    tool.info.detected_by.push(match (&bundle, chat) {
        (Some(id), _) if kind == plan::QoderKind::Desktop => format!("bundle:{id}"),
        (_, Some(scheme)) => format!("protocol:{scheme}"),
        _ => "application".into(),
    });
    // Desktop: an unconfirmed attempt (docs cover only the IDE); see report.md.
    tool.chat_scheme = chat;
    tool.application = Some(application);
    tool.process = bundle;
    tool.finish()
}

fn locate_tool(spec: &ToolSpec) -> DetectedTool {
    if spec.id == "qoder" {
        locate_qoder()
    } else {
        locate_spec(spec)
    }
}

/// Folders where command-line AI tools are commonly installed. Checked
/// directly, without a shell, so a missing shell setup cannot hide them.
fn command_directories() -> Vec<PathBuf> {
    let mut directories = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for relative in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            "Library/pnpm",
            ".yarn/bin",
        ] {
            directories.push(home.join(relative));
        }
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            directories.extend(versions.flatten().map(|version| version.path().join("bin")));
        }
    }
    directories
}

fn executable_in(directories: &[PathBuf], name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // metadata follows links, so a link left behind by an uninstall does not count.
    directories.iter().any(|directory| {
        std::fs::metadata(directory.join(name))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

/// The user's own zsh or bash; other shells use zsh with the user's files.
fn user_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| {
            shell.is_absolute()
                && shell.is_file()
                && shell
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| matches!(name, "zsh" | "bash"))
        })
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

/// Which of these fixed command names exist. A login shell alone does not
/// read ~/.zshrc, where nvm and npm setups usually put their PATH, so an
/// interactive login shell is asked first and a plain login shell only if
/// that one cannot finish. No app paths or user text reach the shell.
async fn found_commands(names: &[&'static str]) -> BTreeSet<String> {
    let directories = command_directories();
    let mut found: BTreeSet<String> = names
        .iter()
        .filter(|name| executable_in(&directories, name))
        .map(|name| name.to_string())
        .collect();
    let remaining: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !found.contains(*name))
        .collect();
    if remaining.is_empty() {
        return found;
    }
    let script: String = remaining
        .iter()
        .map(|command| {
            format!(
                "if command -v {command} >/dev/null 2>&1; then printf '%s\\n' '{command}'; fi\n"
            )
        })
        .collect();
    let shell = user_shell();
    for flags in ["-ilc", "-lc"] {
        let mut command = Command::new(&shell);
        command
            .args([flags, script.as_str()])
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(output) =
            super::super::native_process::output(command, Duration::from_secs(5)).await
        {
            // Startup files may print their own lines; only exact names count.
            found.extend(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|line| remaining.contains(line))
                    .map(str::to_owned),
            );
            break;
        }
    }
    found
}

pub(super) async fn locate(id: &str) -> Result<DetectedTool, ApiError> {
    if id == "other" {
        return Ok(DetectedTool::other());
    }
    let spec = TOOLS
        .iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| ApiError::new("AI_TOOL_UNKNOWN", "无法识别这个 AI 工具，请重新选择。"))?;
    let mut tool = locate_tool(spec);
    if !tool.info.installed {
        if let Some(name) = spec.command {
            if found_commands(&[name]).await.contains(name) {
                tool.info.installed = true;
                tool.info.detected_by.push(format!("command:{name}"));
            }
        }
    }
    Ok(tool.finish())
}

pub(super) async fn detect() -> Result<Vec<AiTool>, ApiError> {
    let names: Vec<&'static str> = TOOLS.iter().filter_map(|spec| spec.command).collect();
    let commands = found_commands(&names).await;
    let mut tools = Vec::new();
    for spec in TOOLS {
        let mut tool = locate_tool(spec);
        if let Some(command) = spec.command.filter(|command| commands.contains(*command)) {
            tool.info.installed = true;
            tool.info.detected_by.push(format!("command:{command}"));
        }
        tools.push(tool.finish().info);
    }
    tools.sort_by_key(|tool| !tool.installed);
    Ok(tools)
}

struct Mac;

async fn run_open(application: Option<&Path>, value: Option<&std::ffi::OsStr>) -> bool {
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

impl plan::Native for Mac {
    async fn dispatch_url(&self, application: Option<&Path>, url: &str) -> bool {
        run_open(application, Some(std::ffi::OsStr::new(url))).await
    }
    async fn launch(&self, application: &Path, folder: Option<&Path>) -> bool {
        run_open(Some(application), folder.map(Path::as_os_str)).await
    }
    async fn reveal(&self, folder: &Path) -> bool {
        run_open(None, Some(folder.as_os_str())).await
    }
    async fn running(&self, tool: &DetectedTool) -> Option<bool> {
        let id = tool.process.as_deref()?;
        let applications =
            NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(id));
        Some(
            applications
                .iter()
                .any(|application| application.isFinishedLaunching()),
        )
    }
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

pub(super) async fn open(tool: &DetectedTool, root: &Path, text: &str) -> OpenResult {
    plan::open(&Mac, tool, root, text, super::paste_note()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn application_fixture(bundle_id: &str) -> (tempfile::TempDir, PathBuf) {
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

    #[test]
    fn bundle_identity_is_read_from_the_application_not_its_name() {
        let (_directory, application) = application_fixture("com.qodercn.app");
        assert_eq!(bundle_id(&application).as_deref(), Some("com.qodercn.app"));
        assert!(QODER_DESKTOP_BUNDLES.contains(&bundle_id(&application).unwrap().as_str()));
        let (_directory, ide) = application_fixture("example.qoder.ide");
        assert!(!QODER_DESKTOP_BUNDLES.contains(&bundle_id(&ide).unwrap().as_str()));
    }

    #[test]
    fn only_present_executable_commands_count() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().to_path_buf();
        std::fs::write(bin.join("claude"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(bin.join("codex"), "not executable").unwrap();
        std::os::unix::fs::symlink(bin.join("removed-app/bin/code"), bin.join("code")).unwrap();
        let directories = [PathBuf::from("/nonexistent-panda-bin"), bin];
        assert!(executable_in(&directories, "claude"));
        assert!(
            !executable_in(&directories, "codex"),
            "a file without the executable bit"
        );
        assert!(
            !executable_in(&directories, "code"),
            "a link left behind by an uninstall"
        );
        assert!(!executable_in(&directories, "cursor"));
    }

    #[test]
    fn applications_in_the_trash_are_not_installed() {
        let directory = tempfile::tempdir().unwrap();
        let kept = directory.path().join("Applications/Tool.app");
        let trashed = directory.path().join(".Trash/Tool.app");
        let volume = directory.path().join("Volumes/Disk/.Trashes/501/Tool.app");
        for path in [&kept, &trashed, &volume] {
            std::fs::create_dir_all(path).unwrap();
        }
        assert!(usable_application(&kept));
        assert!(!usable_application(&trashed));
        assert!(!usable_application(&volume));
        assert!(!usable_application(
            &directory.path().join("Applications/Gone.app")
        ));
    }

    #[test]
    fn every_chooser_tool_has_one_macos_entry() {
        let ids: BTreeSet<_> = TOOLS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids.len(), TOOLS.len());
        for id in ["codex", "claude", "workbuddy", "qoder", "trae", "trae-cn"] {
            assert!(ids.contains(id), "{id}");
        }
        // Command-line tools and tools that cannot receive a folder are not offered.
        for id in ["claude-code", "deepseek-harness"] {
            assert!(!ids.contains(id), "{id}");
        }
        let workbuddy = TOOLS.iter().find(|spec| spec.id == "workbuddy").unwrap();
        assert!(workbuddy.wait_for_start && workbuddy.capability == Capability::Full);
    }
}
