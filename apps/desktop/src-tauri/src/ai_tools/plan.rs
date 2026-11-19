//! Platform-neutral handoff rules: link building, length limits, tool
//! preference and the fallback order. Native modules only detect tools and
//! perform the individual OS actions described here.
use super::{DetectedTool, OpenResult};
use std::{future::Future, path::Path, time::Duration};

/// What a tool can do with one click, as shown in the chooser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capability {
    /// One link opens the folder, starts a new conversation and fills the prompt.
    Full,
    /// The program opens the folder; a separate chat link fills short prompts.
    FolderPrefill,
    /// The program opens the folder; the user pastes the prompt.
    Folder,
    /// The program starts; the user chooses the folder and pastes the prompt.
    Launch,
    /// Nothing is opened in a tool: file manager plus clipboard.
    Manual,
}

impl Capability {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::FolderPrefill => "folder-prefill",
            Self::Folder => "folder",
            Self::Launch => "launch",
            Self::Manual => "manual",
        }
    }
}

/// Windows passes a protocol URL on a command line (at most 32767 UTF-16
/// units, including the handler path). Stay well below on every system.
pub(crate) const MAX_URL_CHARS: usize = 30_000;
/// WorkBuddy is given time to start before it receives the task link.
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const START_POLL: Duration = Duration::from_millis(500);
pub(crate) const START_SETTLE: Duration = Duration::from_millis(2500);
/// A folder window needs to become active before a chat link targets it.
pub(crate) const CHAT_DELAY_RUNNING: Duration = Duration::from_secs(2);
pub(crate) const CHAT_DELAY_STARTED: Duration = Duration::from_secs(6);

struct LinkSpec {
    base: &'static str,
    fixed: &'static [(&'static str, &'static str)],
    path_key: &'static str,
    prompt_key: &'static str,
    /// Characters the tool accepts before truncating; longer prompts are pasted.
    prompt_limit: usize,
    /// Whether the link is still valid without the prompt parameter.
    prompt_required: bool,
}

fn link_spec(tool_id: &str) -> Option<LinkSpec> {
    Some(match tool_id {
        // https://learn.chatgpt.com/docs/app/commands (ChatGPT desktop keeps codex://)
        "codex" => LinkSpec {
            base: "codex://new",
            fixed: &[],
            path_key: "path",
            prompt_key: "prompt",
            prompt_limit: usize::MAX,
            prompt_required: false,
        },
        // https://support.claude.com/en/articles/14729294: q is truncated near 14000.
        "claude" => LinkSpec {
            base: "claude://code/new",
            fixed: &[],
            path_key: "folder",
            prompt_key: "q",
            prompt_limit: 14_000,
            prompt_required: false,
        },
        // WorkBuddy 5.11.3 requires a prompt and truncates it at 8000 characters.
        "workbuddy" => LinkSpec {
            base: "workbuddy://task",
            fixed: &[("action", "start")],
            path_key: "cwd",
            prompt_key: "prompt",
            prompt_limit: 8_000,
            prompt_required: true,
        },
        _ => return None,
    })
}

/// Deep links may be read by URI decoders rather than form decoders: spaces
/// become %20, and literal plus signs were already encoded as %2B.
fn encoded(base: &str, pairs: &[(&str, &str)]) -> Option<tauri::Url> {
    let mut url = tauri::Url::parse(base).ok()?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    }
    let query = url.query()?.replace('+', "%20");
    url.set_query(Some(&query));
    Some(url)
}

/// A one-click link for a full-handoff tool, and whether it carries the prompt.
/// The folder must be an absolute path in the platform's ordinary spelling.
pub(crate) fn full_link(
    tool_id: &str,
    root: &Path,
    text: &str,
    paste_note: &str,
) -> Option<(tauri::Url, bool)> {
    let spec = link_spec(tool_id)?;
    let root = root.to_str().filter(|_| root.is_absolute())?;
    let build = |prompt: Option<&str>| {
        let mut pairs: Vec<(&str, &str)> = spec.fixed.to_vec();
        pairs.push((spec.path_key, root));
        if let Some(prompt) = prompt {
            pairs.push((spec.prompt_key, prompt));
        }
        encoded(spec.base, &pairs).filter(|url| url.as_str().chars().count() <= MAX_URL_CHARS)
    };
    if text.chars().count() <= spec.prompt_limit {
        if let Some(url) = build(Some(text)) {
            return Some((url, true));
        }
    }
    let fallback = if spec.prompt_required {
        build(Some(paste_note))
    } else {
        build(None)
    };
    fallback.map(|url| (url, false))
}

/// A chat link for tools that open the folder separately. None when the tool
/// has no such link or the prompt is too long for it.
pub(crate) fn chat_link(tool_id: &str, scheme: &str, text: &str) -> Option<tauri::Url> {
    let (url, limit) = match tool_id {
        // https://docs.qoder.com/user-guide/deeplink: new agent chat, never sent;
        // the whole URL must stay within 8000 characters.
        "qoder" => (
            encoded(
                &format!("{scheme}://aicoding.aicoding-deeplink/chat"),
                &[("text", text), ("mode", "agent"), ("isNewChat", "true")],
            )?,
            8_000,
        ),
        // Cursor prompt links apply to the active window and ask for confirmation.
        "cursor" => (
            encoded(
                &format!("{scheme}://anysphere.cursor-deeplink/prompt"),
                &[("text", text)],
            )?,
            8_000,
        ),
        _ => return None,
    };
    (url.as_str().chars().count() <= limit).then_some(url)
}

/// Qoder ships an IDE (opens folders) and a separate desktop app (the user
/// selects a folder). Prefer an IDE, which can open this app's folder itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QoderKind {
    Ide,
    Desktop,
}

pub(crate) fn select_qoder<T>(candidates: Vec<(QoderKind, T)>) -> Option<(QoderKind, T)> {
    let mut candidates = candidates;
    let index = candidates
        .iter()
        .position(|(kind, _)| *kind == QoderKind::Ide)
        .or_else(|| (!candidates.is_empty()).then_some(0))?;
    Some(candidates.swap_remove(index))
}

/// Individual native actions. Each returns whether the OS accepted the request;
/// acceptance never proves what the tool did with it.
pub(crate) trait Native: Sync {
    fn dispatch_url(
        &self,
        application: Option<&Path>,
        url: &str,
    ) -> impl Future<Output = bool> + Send;
    fn launch(
        &self,
        application: &Path,
        folder: Option<&Path>,
    ) -> impl Future<Output = bool> + Send;
    fn reveal(&self, folder: &Path) -> impl Future<Output = bool> + Send;
    /// None when the platform cannot tell.
    fn running(&self, tool: &DetectedTool) -> impl Future<Output = Option<bool>> + Send;
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// Warnings are codes; the manager translates them with the tool name.
pub(crate) mod warning {
    pub(crate) const TOOL_NOT_FOUND: &str = "TOOL_NOT_FOUND";
    pub(crate) const TOOL_OPEN_FAILED: &str = "TOOL_OPEN_FAILED";
    pub(crate) const FOLDER_OPEN_FAILED: &str = "FOLDER_OPEN_FAILED";
    pub(crate) const PROMPT_NOT_FILLED: &str = "PROMPT_NOT_FILLED";
}

fn opened(
    tool: &DetectedTool,
    capability: Capability,
    prefilled: bool,
    trace: Vec<String>,
) -> OpenResult {
    let full = capability == Capability::Full;
    OpenResult {
        tool_id: tool.info.id.clone(),
        tool_name: tool.info.name.clone(),
        one_click: full && prefilled,
        opened: true,
        capability,
        prefilled,
        // A full link without the prompt still needs the paste step.
        warning: (full && !prefilled).then_some(warning::PROMPT_NOT_FILLED),
        trace,
    }
}

/// Open `root` in the detected tool, degrading to the file manager. `root` is
/// the folder in the platform's ordinary spelling; `paste_note` is a short
/// localized placeholder for links that cannot exist without a prompt.
pub(crate) async fn open<N: Native>(
    native: &N,
    tool: &DetectedTool,
    root: &Path,
    text: &str,
    paste_note: &str,
) -> OpenResult {
    let mut trace = Vec::new();
    // Tools such as WorkBuddy silently drop a working folder that does not exist.
    let folder_present = root.is_dir();
    if !folder_present {
        trace.push("folder-missing".into());
    }
    if tool.info.id != "other" && tool.info.installed && folder_present {
        match tool.capability {
            Capability::Full if tool.link_ready => {
                if let Some((url, prefilled)) = full_link(&tool.info.id, root, text, paste_note) {
                    if tool.wait_for_start {
                        wait_for_start(native, tool, &mut trace).await;
                    }
                    if native
                        .dispatch_url(tool.application.as_deref(), url.as_str())
                        .await
                    {
                        trace.push(format!("dispatched-link prefilled={prefilled}"));
                        return opened(tool, Capability::Full, prefilled, trace);
                    }
                    trace.push("link-dispatch-failed".into());
                }
            }
            Capability::FolderPrefill | Capability::Folder => {
                if let Some(application) = tool.application.as_deref() {
                    let was_running = native.running(tool).await;
                    if native.launch(application, Some(root)).await {
                        trace.push("opened-folder".into());
                        let prefilled = tool.capability == Capability::FolderPrefill
                            && chat_after_start(native, tool, text, was_running, &mut trace).await;
                        return opened(tool, tool.capability, prefilled, trace);
                    }
                    trace.push("open-folder-failed".into());
                }
            }
            Capability::Launch => {
                let was_running = native.running(tool).await;
                let launched = match (tool.application.as_deref(), tool.launch_url) {
                    (Some(application), _) => native.launch(application, None).await,
                    (None, Some(url)) => native.dispatch_url(None, url).await,
                    (None, None) => false,
                };
                if launched {
                    trace.push("launched".into());
                    // An unconfirmed chat link (Qoder desktop) is only an attempt;
                    // the guide still asks for the folder and a paste.
                    let prefilled =
                        chat_after_start(native, tool, text, was_running, &mut trace).await;
                    return opened(tool, Capability::Launch, prefilled, trace);
                }
                trace.push("launch-failed".into());
            }
            _ => trace.push("not-one-click".into()),
        }
    }
    let revealed = native.reveal(root).await;
    trace.push(format!("revealed-folder={revealed}"));
    let warning = if !folder_present {
        Some(warning::FOLDER_OPEN_FAILED)
    } else if tool.info.id == "other" {
        (!revealed).then_some(warning::FOLDER_OPEN_FAILED)
    } else if !tool.info.installed {
        Some(warning::TOOL_NOT_FOUND)
    } else {
        Some(warning::TOOL_OPEN_FAILED)
    };
    OpenResult {
        tool_id: "other".into(),
        tool_name: "other".into(),
        one_click: false,
        opened: revealed,
        capability: Capability::Manual,
        prefilled: false,
        warning,
        trace,
    }
}

/// Send the tool's separate chat link after its window had time to appear.
async fn chat_after_start<N: Native>(
    native: &N,
    tool: &DetectedTool,
    text: &str,
    was_running: Option<bool>,
    trace: &mut Vec<String>,
) -> bool {
    let Some(url) = tool
        .chat_scheme
        .and_then(|scheme| chat_link(&tool.info.id, scheme, text))
    else {
        trace.push("chat-link-skipped".into());
        return false;
    };
    native
        .sleep(if was_running == Some(true) {
            CHAT_DELAY_RUNNING
        } else {
            CHAT_DELAY_STARTED
        })
        .await;
    let sent = native
        .dispatch_url(tool.application.as_deref(), url.as_str())
        .await;
    trace.push(format!("chat-link prefilled={sent}"));
    sent
}

async fn wait_for_start<N: Native>(native: &N, tool: &DetectedTool, trace: &mut Vec<String>) {
    match native.running(tool).await {
        Some(false) => {}
        state => {
            trace.push(format!("already-running={state:?}"));
            return;
        }
    }
    let Some(application) = tool.application.as_deref() else {
        trace.push("start-skipped-no-application".into());
        return;
    };
    if !native.launch(application, None).await {
        trace.push("start-failed".into());
        return;
    }
    let mut waited = Duration::ZERO;
    while waited < START_TIMEOUT {
        native.sleep(START_POLL).await;
        waited += START_POLL;
        if native.running(tool).await == Some(true) {
            native.sleep(START_SETTLE).await;
            trace.push(format!("started-after={}ms", waited.as_millis()));
            return;
        }
    }
    trace.push(format!("start-timeout={}ms", waited.as_millis()));
}

/// Text sent from an app page is written by the app, so the platform adds the
/// app folder there. The user is never asked to select the folder in the tool:
/// the AI reads and writes it by path, and only when it cannot does it explain
/// how to open the folder in its own tool. Platform prompts carry the same
/// sentence (scripts/project-ai-requirements.json; the repair and evaluation
/// prompts ask what the AI can read), so no tool gets it twice.
pub(crate) fn with_workspace_line(entry: &str, root: &Path, text: &str, english: bool) -> String {
    if entry != "app-page" {
        return text.to_owned();
    }
    let folder = root.display();
    if english {
        format!(
            "{text}\n\nThis app's folder is {folder}. If it is not your current working folder, read and write it by its path. If you cannot, first tell me how to open it in your tool."
        )
    } else {
        format!(
            "{text}\n\n这个应用的文件夹是 {folder}。它不是你当前的工作文件夹也没关系，按路径读写；读写不了时，先告诉我在你这个工具里怎么打开它。"
        )
    }
}

/// One log line per dispatch, without the prompt text.
pub(crate) fn log_line(entry: &str, tool_id: &str, root: &Path, result: &OpenResult) -> String {
    format!(
        "entry={entry} selected={tool_id} result={} capability={} opened={} prefilled={} warning={} folder={:?} steps=[{}]",
        result.tool_id,
        result.capability.as_str(),
        result.opened,
        result.prefilled,
        result.warning.unwrap_or("-"),
        root,
        result.trace.join(", "),
    )
}

/// Entry labels come from the manager; keep only a short lowercase token.
pub(crate) fn entry_label(value: Option<&str>) -> String {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        })
        .unwrap_or("unknown")
        .to_owned()
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
