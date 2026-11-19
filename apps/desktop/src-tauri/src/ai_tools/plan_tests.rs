//! Pure link/fallback tests. No AI tool, file manager or URL is opened: every
//! native action goes to a recording fake.
use super::*;
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::Mutex,
};

fn values(url: &tauri::Url) -> BTreeMap<String, String> {
    url.query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

/// A real disposable folder whose name needs every kind of URI escaping.
fn disposable_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("可丢弃 项目").join("a#b&c+d=e%20g");
        std::fs::create_dir_all(&root).unwrap();
        (directory, root)
    })
    .1
    .clone()
}

const TEXT: &str = "请先阅读 README\n不要自动修改 & cwd=/wrong + 100% #标题 ?问";

#[test]
fn full_links_keep_folder_and_prompt_as_separate_uri_encoded_values() {
    let root = disposable_root();
    for (tool, base, folder, prompt) in [
        ("codex", "codex://new", "path", "prompt"),
        ("claude", "claude://code/new", "folder", "q"),
        ("workbuddy", "workbuddy://task", "cwd", "prompt"),
    ] {
        let (url, prefilled) = full_link(tool, &root, TEXT, "note").unwrap();
        assert!(prefilled, "{tool}");
        assert!(
            url.as_str().starts_with(&format!("{base}?")),
            "{tool}: {url}"
        );
        let query = url.query().unwrap();
        // URI decoders must see spaces as %20; a literal plus stays %2B.
        assert!(!query.contains('+'), "{tool}: {query}");
        assert!(
            query.contains("%20") && query.contains("%2B"),
            "{tool}: {query}"
        );
        assert!(
            !query.contains(' ') && !query.contains('#'),
            "{tool}: {query}"
        );
        let pairs = values(&url);
        assert_eq!(pairs[folder], root.to_str().unwrap(), "{tool}");
        assert_eq!(pairs[prompt], TEXT, "{tool}");
        assert_eq!(
            pairs.get("action").map(String::as_str),
            (tool == "workbuddy").then_some("start")
        );
        assert_eq!(
            pairs.len(),
            if tool == "workbuddy" { 3 } else { 2 },
            "{tool}"
        );
    }
    assert!(full_link("vscode", &root, TEXT, "note").is_none());
    assert!(full_link("codex", Path::new("relative/folder"), TEXT, "note").is_none());
}

#[test]
fn chinese_paths_round_trip_through_percent_encoding() {
    let root = disposable_root();
    let (url, _) = full_link("workbuddy", &root, "中文提示", "note").unwrap();
    assert!(url.as_str().is_ascii(), "{url}");
    assert!(
        url.as_str().contains("%E5%8F%AF%E4%B8%A2%E5%BC%83"),
        "{url}"
    );
    assert_eq!(values(&url)["cwd"], root.to_str().unwrap());
}

#[test]
fn prompts_longer_than_a_tool_accepts_are_pasted_instead_of_truncated() {
    let root = disposable_root();
    // ASCII keeps the URL short, so only the tool's own prompt limit applies.
    let long = "a".repeat(14_001);
    let (url, prefilled) = full_link("claude", &root, &long, "note").unwrap();
    assert!(!prefilled);
    assert!(!values(&url).contains_key("q"));
    assert_eq!(values(&url)["folder"], root.to_str().unwrap());
    // Claude desktop accepts prompts up to about 14000 characters.
    assert!(
        full_link("claude", &root, &"a".repeat(5_001), "note")
            .unwrap()
            .1
    );
    // About 3300 Chinese characters already reach the shared URL bound.
    assert!(
        !full_link("claude", &root, &"字".repeat(3_400), "note")
            .unwrap()
            .1
    );
    // WorkBuddy needs a prompt parameter, so it receives the short paste note.
    let (url, prefilled) = full_link("workbuddy", &root, &"字".repeat(8_001), "请粘贴").unwrap();
    assert!(!prefilled);
    assert_eq!(values(&url)["prompt"], "请粘贴");
    // Codex has no documented prompt limit; the URL itself is bounded.
    let (url, prefilled) = full_link("codex", &root, &"字".repeat(4_000), "note").unwrap();
    assert!(!prefilled);
    assert!(url.as_str().len() <= MAX_URL_CHARS);
    assert!(!values(&url).contains_key("prompt"));
}

#[test]
fn chat_links_respect_their_url_limits_and_carry_no_folder() {
    let url = chat_link("qoder", "qoder", TEXT).unwrap();
    assert!(url
        .as_str()
        .starts_with("qoder://aicoding.aicoding-deeplink/chat?"));
    let pairs = values(&url);
    assert_eq!(pairs["text"], TEXT);
    assert_eq!(pairs["mode"], "agent");
    assert_eq!(pairs["isNewChat"], "true");
    assert!(chat_link("qoder", "qoder-cn", TEXT)
        .unwrap()
        .as_str()
        .starts_with("qoder-cn://"));
    assert_eq!(
        values(&chat_link("cursor", "cursor", TEXT).unwrap())["text"],
        TEXT
    );
    // 1000 Chinese characters encode to 9000 URL characters.
    assert!(chat_link("qoder", "qoder", &"字".repeat(1_000)).is_none());
    assert!(chat_link("vscode", "vscode", TEXT).is_none());
}

#[test]
fn qoder_prefers_an_ide_that_can_open_the_folder() {
    assert_eq!(
        select_qoder(vec![
            (QoderKind::Desktop, "desktop"),
            (QoderKind::Ide, "ide")
        ]),
        Some((QoderKind::Ide, "ide"))
    );
    assert_eq!(
        select_qoder(vec![(QoderKind::Desktop, "cn-desktop")]),
        Some((QoderKind::Desktop, "cn-desktop"))
    );
    assert_eq!(select_qoder::<&str>(Vec::new()), None);
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    running: Mutex<VecDeque<Option<bool>>>,
    dispatch_ok: bool,
    launch_ok: bool,
    reveal_ok: bool,
}
impl Fake {
    fn new() -> Self {
        Self {
            dispatch_ok: true,
            launch_ok: true,
            reveal_ok: true,
            ..Default::default()
        }
    }
    fn running(self, states: &[Option<bool>]) -> Self {
        *self.running.lock().unwrap() = states.iter().copied().collect();
        self
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn push(&self, value: String) {
        self.calls.lock().unwrap().push(value);
    }
}
impl Native for Fake {
    async fn dispatch_url(&self, application: Option<&Path>, url: &str) -> bool {
        let scheme = url.split(':').next().unwrap_or_default();
        self.push(format!("url:{scheme}:{}", application.is_some()));
        self.dispatch_ok
    }
    async fn launch(&self, _: &Path, folder: Option<&Path>) -> bool {
        self.push(format!("launch:{}", folder.is_some()));
        self.launch_ok
    }
    async fn reveal(&self, _: &Path) -> bool {
        self.push("reveal".into());
        self.reveal_ok
    }
    async fn running(&self, _: &DetectedTool) -> Option<bool> {
        self.push("running?".into());
        self.running
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Some(true))
    }
    async fn sleep(&self, duration: Duration) {
        self.push(format!("sleep:{}", duration.as_millis()));
    }
}

fn tool(id: &str, capability: Capability) -> DetectedTool {
    let mut tool = DetectedTool::new(id, id, capability);
    tool.info.installed = true;
    tool.link_ready = true;
    tool.application = Some(PathBuf::from("/disposable/Tool.app"));
    tool.wait_for_start = id == "workbuddy";
    tool.finish()
}

#[tokio::test]
async fn workbuddy_is_started_and_awaited_before_the_task_link() {
    let fake = Fake::new().running(&[Some(false), Some(false), Some(true)]);
    let result = open(
        &fake,
        &tool("workbuddy", Capability::Full),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    assert_eq!(
        fake.calls(),
        [
            "running?",
            "launch:false",
            "sleep:500",
            "running?",
            "sleep:500",
            "running?",
            "sleep:2500",
            "url:workbuddy:true"
        ]
    );
    assert!(result.one_click && result.opened && result.prefilled);
    assert_eq!(result.warning, None);
    assert!(
        result
            .trace
            .iter()
            .any(|step| step == "started-after=1000ms"),
        "{:?}",
        result.trace
    );
}

#[tokio::test]
async fn a_running_workbuddy_receives_the_link_directly() {
    let fake = Fake::new().running(&[Some(true)]);
    let result = open(
        &fake,
        &tool("workbuddy", Capability::Full),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    assert_eq!(fake.calls(), ["running?", "url:workbuddy:true"]);
    assert!(result.one_click);
}

#[tokio::test]
async fn a_workbuddy_that_never_starts_is_still_given_the_link_once() {
    let fake = Fake::new().running(&[Some(false); 50]);
    let result = open(
        &fake,
        &tool("workbuddy", Capability::Full),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    let calls = fake.calls();
    assert_eq!(
        calls.iter().filter(|call| call.starts_with("url:")).count(),
        1
    );
    assert_eq!(calls.iter().filter(|call| *call == "sleep:500").count(), 40);
    assert!(result
        .trace
        .iter()
        .any(|step| step == "start-timeout=20000ms"));
}

#[tokio::test]
async fn failed_links_fall_back_to_the_file_manager_with_a_warning_code() {
    let fake = Fake {
        dispatch_ok: false,
        ..Fake::new()
    };
    let result = open(
        &fake,
        &tool("codex", Capability::Full),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    assert_eq!(fake.calls(), ["url:codex:true", "reveal"]);
    assert_eq!(result.tool_id, "other");
    assert_eq!(result.capability, Capability::Manual);
    assert_eq!(result.warning, Some(warning::TOOL_OPEN_FAILED));
    assert!(result.opened && !result.one_click);
}

#[tokio::test]
async fn missing_and_unregistered_tools_explain_what_to_do() {
    let mut missing = tool("claude", Capability::Full);
    missing.info.installed = false;
    let fake = Fake::new();
    assert_eq!(
        open(&fake, &missing, &disposable_root(), TEXT, "note")
            .await
            .warning,
        Some(warning::TOOL_NOT_FOUND)
    );
    assert_eq!(fake.calls(), ["reveal"]);
    let mut unlinked = tool("claude", Capability::Full);
    unlinked.link_ready = false;
    let unlinked = unlinked.finish();
    assert_eq!(unlinked.info.setup_hint, None);
    assert!(!unlinked.info.one_click);
    assert_eq!(unlinked.info.capability, "manual");
    assert_eq!(
        open(&Fake::new(), &unlinked, &disposable_root(), TEXT, "note")
            .await
            .warning,
        Some(warning::TOOL_OPEN_FAILED)
    );
    let fake = Fake {
        reveal_ok: false,
        ..Fake::new()
    };
    let other = open(
        &fake,
        &DetectedTool::other(),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    assert_eq!(
        (other.opened, other.warning),
        (false, Some(warning::FOLDER_OPEN_FAILED))
    );
}

#[tokio::test]
async fn long_prompts_open_the_folder_and_ask_for_a_paste() {
    let result = open(
        &Fake::new(),
        &tool("claude", Capability::Full),
        &disposable_root(),
        &"a".repeat(14_001),
        "note",
    )
    .await;
    assert!(result.opened && !result.prefilled && !result.one_click);
    assert_eq!(result.warning, Some(warning::PROMPT_NOT_FILLED));
}

#[tokio::test]
async fn folder_tools_open_the_folder_and_short_chat_links_follow_the_new_window() {
    let fake = Fake::new().running(&[Some(false)]);
    let mut qoder = tool("qoder", Capability::FolderPrefill);
    qoder.chat_scheme = Some("qoder");
    let result = open(&fake, &qoder, &disposable_root(), "短提示", "note").await;
    assert_eq!(
        fake.calls(),
        ["running?", "launch:true", "sleep:6000", "url:qoder:true"]
    );
    assert!(result.prefilled && !result.one_click);
    assert_eq!(result.capability, Capability::FolderPrefill);
    let fake = Fake::new();
    let result = open(
        &fake,
        &qoder,
        &disposable_root(),
        &"字".repeat(2_000),
        "note",
    )
    .await;
    assert_eq!(fake.calls(), ["running?", "launch:true"]);
    assert!(!result.prefilled && result.warning.is_none());
    let fake = Fake::new();
    let result = open(
        &fake,
        &tool("vscode", Capability::Folder),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    assert_eq!(fake.calls(), ["running?", "launch:true"]);
    assert_eq!(
        (result.tool_id.as_str(), result.capability),
        ("vscode", Capability::Folder)
    );
}

#[tokio::test]
async fn launch_tools_start_without_a_folder_argument() {
    let fake = Fake::new();
    let result = open(
        &fake,
        &tool("launch-only", Capability::Launch),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    assert_eq!(fake.calls(), ["running?", "launch:false"]);
    assert_eq!(result.capability, Capability::Launch);
    assert!(!result.prefilled);
    let mut protocol_only = tool("launch-only", Capability::Launch);
    protocol_only.application = None;
    protocol_only.launch_url = Some("example://open");
    let fake = Fake::new();
    open(&fake, &protocol_only, &disposable_root(), TEXT, "note").await;
    assert_eq!(fake.calls(), ["running?", "url:example:false"]);
    // Qoder desktop gets an unconfirmed chat-link attempt after it starts.
    let mut desktop = tool("qoder", Capability::Launch);
    desktop.chat_scheme = Some("qoder-cn");
    let fake = Fake::new().running(&[Some(false)]);
    let result = open(&fake, &desktop, &disposable_root(), "短提示", "note").await;
    assert_eq!(
        fake.calls(),
        [
            "running?",
            "launch:false",
            "sleep:6000",
            "url:qoder-cn:true"
        ]
    );
    assert!(result.prefilled && !result.one_click && result.warning.is_none());
}

#[tokio::test]
async fn a_missing_folder_is_never_sent_to_a_tool() {
    let fake = Fake::new();
    let missing = disposable_root().join("removed-before-dispatch");
    let result = open(
        &fake,
        &tool("workbuddy", Capability::Full),
        &missing,
        TEXT,
        "note",
    )
    .await;
    assert_eq!(fake.calls(), ["reveal"]);
    assert_eq!(result.warning, Some(warning::FOLDER_OPEN_FAILED));
    assert!(result.trace.iter().any(|step| step == "folder-missing"));
}

#[tokio::test]
async fn the_log_line_records_the_outcome_but_never_the_prompt() {
    let result = open(
        &Fake::new(),
        &tool("codex", Capability::Full),
        &disposable_root(),
        TEXT,
        "note",
    )
    .await;
    let line = log_line("detail", "codex", &disposable_root(), &result);
    assert!(
        line.contains("entry=detail")
            && line.contains("result=codex")
            && line.contains("prefilled=true"),
        "{line}"
    );
    assert!(
        !line.contains("请先阅读") && !line.contains("cwd=/wrong"),
        "{line}"
    );
    assert_eq!(entry_label(Some("app-page")), "app-page");
    for unsafe_label in [Some(""), Some("Detail"), Some("a b"), Some("../x"), None] {
        assert_eq!(entry_label(unsafe_label), "unknown");
    }
}

#[test]
fn app_page_prompts_name_the_folder_and_platform_prompts_keep_the_text() {
    let root = disposable_root();
    let text = with_workspace_line("app-page", &root, "原文", false);
    assert!(text.starts_with("原文\n\n这个应用的文件夹是 "));
    assert!(text.contains(&root.display().to_string()) && text.contains("按路径读写"));
    assert!(with_workspace_line("app-page", &root, "Text", true).contains("by its path"));
    for platform_entry in ["detail", "titlebar", "create", "repair", "unknown"] {
        assert_eq!(
            with_workspace_line(platform_entry, &root, "原文", false),
            "原文"
        );
    }
}
