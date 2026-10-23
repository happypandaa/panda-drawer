//! Explicitly acknowledged builds of independent source candidates. These are
//! trusted scripts running as the user; this module is not a code sandbox.
use crate::{
    development_tasks::{DevelopmentTasks, TaskContext},
    development_tools::{DevelopmentTools, ToolLease},
    windows_sources::{SourceLease, SourceManager},
    ApiError,
};
use platform_host::{
    windows_command::{self, CommandSpec, CommandTermination},
    WindowsObjectIdentity, WindowsSourceDirectory, WorkspaceDirectory,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{watch, Semaphore},
    task::JoinHandle,
};
type Result<T> = std::result::Result<T, ApiError>;
const TIMEOUT: Duration = Duration::from_secs(600);
const DISK_LIMIT: u64 = 512 * 1024 * 1024;
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message).outcome("not_committed")
}
fn fs(e: platform_host::WorkspaceError) -> ApiError {
    error(e.code, &e.message)
}
fn invalid(message: &str) -> ApiError {
    error("SOURCE_BUILD_INVALID", message)
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildInput {
    pub source_id: String,
    pub expected_digest: String,
    pub acknowledge_scripts: bool,
}
pub struct SourceBuilds {
    data: WorkspaceDirectory,
    tools: DevelopmentTools,
    sources: Arc<SourceManager>,
    tasks: Arc<DevelopmentTasks>,
    stopping: AtomicBool,
    submission: Mutex<()>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    capacity: Arc<Semaphore>,
}
impl SourceBuilds {
    pub fn new(
        data: WorkspaceDirectory,
        tools: DevelopmentTools,
        sources: Arc<SourceManager>,
        tasks: Arc<DevelopmentTasks>,
    ) -> Arc<Self> {
        Arc::new(Self {
            data,
            tools,
            sources,
            tasks,
            stopping: AtomicBool::new(false),
            submission: Mutex::new(()),
            runners: Mutex::new(Vec::new()),
            capacity: Arc::new(Semaphore::new(1)),
        })
    }
    pub fn list(&self) -> Result<Value> {
        Ok(json!(self
            .tasks
            .list()
            .as_array()
            .ok_or_else(|| invalid("任务列表格式无效。"))?
            .iter()
            .filter(|task| task["type"] == "sourceBuild")
            .collect::<Vec<_>>()))
    }
    pub async fn prepare(self: &Arc<Self>, input: BuildInput) -> Result<Value> {
        if !input.acknowledge_scripts {
            return Err(error(
                "SOURCE_BUILD_ACK_REQUIRED",
                "请先确认允许在独立候选中运行依赖与项目构建脚本。",
            ));
        }
        if input.source_id.len() != 36
            || !input.source_id.starts_with("src_")
            || !input.source_id[4..].bytes().all(|b| b.is_ascii_hexdigit())
            || input.expected_digest.len() != 64
            || !input
                .expected_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("源码身份或摘要无效，请刷新源码列表。"));
        }
        let _submission = self.submission.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(error("SERVICE_STOPPING", "平台正在退出，未接受候选构建。"));
        }
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| {
            error(
                "SOURCE_BUILD_BUSY",
                "另一个候选构建尚未结束，请先查看该任务。",
            )
        })?;
        let task = self.tasks.start("sourceBuild", Some(&input.source_id))?;
        let accepted = self.tasks.get(task.id())?;
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            let completed = manager.run(&task, input).await;
            let result = match &completed {
                Ok(candidate) => Ok(candidate.value.clone()),
                Err(issue) => Err(issue
                    .clone()
                    .detail("evidencePath", json!(task.directory().path()))),
            };
            let _ = task.finish(result);
            // Keep source, tool and every artifact leaf binding through the
            // final durable receipt, including a persistence failure.
            drop(completed);
            drop(permit);
        });
        let mut runners = self.runners.lock().unwrap();
        runners.retain(|runner| !runner.is_finished());
        runners.push(runner);
        Ok(accepted)
    }
    pub async fn shutdown(&self) {
        let runners = {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            if let Some(tasks) = self.tasks.list().as_array() {
                for task in tasks {
                    if task["type"] == "sourceBuild" && task["final"] == false {
                        if let Some(id) = task["taskId"].as_str() {
                            let _ = self.tasks.cancel(id);
                        }
                    }
                }
            }
            std::mem::take(&mut *self.runners.lock().unwrap())
        };
        for runner in runners {
            let _ = runner.await;
        }
    }
    async fn run(&self, task: &TaskContext, input: BuildInput) -> Result<BuiltCandidate> {
        task.phase("checking-source")?;
        let sources = self.sources.clone();
        let data = self.data.clone();
        let tools = self.tools.clone();
        let directory = task.directory().clone();
        let cancel = task.cancellation();
        let prepared = tokio::task::spawn_blocking(move || {
            Prepared::new(sources, data, tools, directory, input, cancel)
        })
        .await
        .map_err(|_| error("SOURCE_BUILD_INTERRUPTED", "候选准备被中断，现场已保留。"))??;
        task.check()?;
        let deadline = Instant::now() + TIMEOUT;
        task.phase("installing-dependencies")?;
        self.command(task, &prepared, vec!["ci".into()], deadline)
            .await?;
        let prepared = checked(prepared).await?;
        task.phase("building-candidate")?;
        self.command(
            task,
            &prepared,
            vec![
                "run".into(),
                prepared.script.clone().into(),
                "--".into(),
                "--outDir".into(),
                prepared.artifact.path().as_os_str().into(),
            ],
            deadline,
        )
        .await?;
        task.phase("checking-artifact")?;
        task.check()?;
        let prepared = checked(prepared).await?;
        task.check()?;
        let artifact = ordinary(&prepared.artifact)?;
        let entry = prepared.entry.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let snapshot = inventory(&artifact, false)?;
            validate_artifact(&artifact, &snapshot, &entry)?;
            Ok::<_, ApiError>(snapshot)
        })
        .await
        .map_err(|_| error("SOURCE_BUILD_INTERRUPTED", "候选产物检查被中断。"))??;
        if !snapshot.files.contains_key(&prepared.entry) {
            return Err(error(
                "SOURCE_BUILD_ENTRY_MISSING",
                "构建未生成清单要求的网页入口，候选现场已保留。",
            ));
        }
        task.check()?;
        task.directory().write_new("candidate-ready.json", &serde_json::to_vec(&json!({"schemaVersion":1,"sourceId":prepared.source.source_id,"sourceDigest":prepared.source.content_digest,"artifactIdentity":prepared.artifact.identity(),"artifactDigest":snapshot.digest,"files":snapshot.files.iter().map(|(path,file)|json!({"path":path,"identity":file.identity,"sha256":file.digest,"bytes":file.bytes})).collect::<Vec<_>>(),"installed":false,"published":false}))?).map_err(fs)?;
        // Saving readiness is the acceptance point. A cancellation arriving
        // afterward cannot turn this retained ready candidate into a rollback.
        let value = json!({"sourceId":prepared.source.source_id,"sourceDigest":prepared.source.content_digest,"candidatePath":prepared.candidate.path(),"artifactPath":prepared.artifact.path(),"artifactDigest":snapshot.digest,"operationOutcome":"candidate-ready","installed":false,"published":false});
        Ok(BuiltCandidate {
            value,
            _prepared: prepared,
            _artifact: snapshot,
        })
    }
    async fn command(
        &self,
        task: &TaskContext,
        prepared: &Prepared,
        args: Vec<OsString>,
        deadline: Instant,
    ) -> Result<()> {
        task.check()?;
        let node = prepared.node.clone();
        let git = prepared.git.clone();
        tokio::task::spawn_blocking(move || {
            node.verify()?;
            git.verify()
        })
        .await
        .map_err(|_| error("SOURCE_BUILD_INTERRUPTED", "开发工具复核被中断。"))??;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| error("SOURCE_BUILD_TIMEOUT", "候选构建已超时，现场已保留。"))?;
        let mut arguments = vec![
            prepared
                .node
                .npm_cli
                .as_ref()
                .ok_or_else(|| invalid("npm入口缺失。"))?
                .as_os_str()
                .into(),
            "--prefix".into(),
            ".".into(),
            "--global=false".into(),
        ];
        arguments.extend(args);
        let mut environment = prepared.git.environment(
            prepared.home.path(),
            prepared.temporary.path(),
            Some(&prepared.node),
        )?;
        environment.insert(
            "GIT_CEILING_DIRECTORIES".into(),
            prepared.candidate.path().as_os_str().into(),
        );
        environment.insert("NPM_CONFIG_PROGRESS".into(), "false".into());
        let (send, receive) = watch::channel(false);
        let mut cancel = task.cancellation();
        let run = windows_command::run(
            CommandSpec {
                executable: prepared.node.executable.clone(),
                args: arguments,
                working_directory: prepared.candidate.path().into(),
                environment,
                stdin: Vec::new(),
                timeout: remaining,
                stdout_limit: 1024 * 1024,
                stderr_limit: 1024 * 1024,
            },
            receive,
        );
        tokio::pin!(run);
        let mut monitor = tokio::time::interval(Duration::from_millis(500));
        monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut issue = if *cancel.borrow() {
            let _ = send.send(true);
            Some(error(
                "SOURCE_BUILD_CANCELLED",
                "候选构建已取消，进程树停止后保留现场。",
            ))
        } else {
            None
        };
        let mut scan: Option<JoinHandle<Result<()>>> = None;
        let output = loop {
            tokio::select! {
            result=&mut run => break match result {
                Ok(output)=>output,
                Err(issue)=>{
                    // The executor can retain a Job on its cleanup thread.
                    // Keep the entire tool/source lease until that owner drains.
                    windows_command::wait_for_idle().await;
                    if let Some(scan)=scan.take(){let _=scan.await;}
                    return Err(issue);
                }
            },
                _=cancel.changed(), if issue.is_none() => { let _=send.send(true); issue=Some(error("SOURCE_BUILD_CANCELLED","候选构建已取消，进程树停止后保留现场。")); },
                _=monitor.tick(), if issue.is_none() && scan.is_none() => {
                    let root=task.directory().clone();
                    scan=Some(tokio::task::spawn_blocking(move||budget(&root)));
                }
                result=async{scan.as_mut().unwrap().await}, if scan.is_some() => {
                    scan=None;
                    let check=result.unwrap_or_else(|_|Err(error("SOURCE_BUILD_INTERRUPTED","候选容量检查被中断。")));
                    if let Err(failure)=check {let _=send.send(true);issue=Some(failure);}
                }
            }
        };
        if let Some(scan) = scan {
            if let Err(failure) = scan
                .await
                .unwrap_or_else(|_| Err(error("SOURCE_BUILD_INTERRUPTED", "候选容量检查被中断。")))
            {
                issue.get_or_insert(failure);
            }
        }
        task.log(
            "stdout",
            &output.stdout.bytes,
            output.stdout.discarded_bytes > 0,
        )?;
        task.log(
            "stderr",
            &output.stderr.bytes,
            output.stderr.discarded_bytes > 0,
        )?;
        if let Some(issue) = issue {
            return Err(issue);
        }
        if output.termination == CommandTermination::TimedOut {
            return Err(error(
                "SOURCE_BUILD_TIMEOUT",
                "候选构建已超时，进程树已停止，现场已保留。",
            ));
        }
        if !output.success() {
            return Err(error(
                "SOURCE_BUILD_PROCESS_FAILED",
                "依赖安装或构建未完整成功，请查看任务日志；现场已保留。",
            ));
        }
        task.check()?;
        Ok(())
    }
}
struct BuiltCandidate {
    value: Value,
    _prepared: Prepared,
    _artifact: Inventory,
}
struct Prepared {
    source: SourceLease,
    node: Arc<ToolLease>,
    git: Arc<ToolLease>,
    candidate: WorkspaceDirectory,
    artifact: WorkspaceDirectory,
    home: WorkspaceDirectory,
    temporary: WorkspaceDirectory,
    script: String,
    entry: String,
    baseline: Inventory,
    _configs: Vec<WindowsSourceDirectory>,
}
impl Prepared {
    fn new(
        sources: Arc<SourceManager>,
        data: WorkspaceDirectory,
        tools: DevelopmentTools,
        directory: WorkspaceDirectory,
        input: BuildInput,
        cancel: watch::Receiver<bool>,
    ) -> Result<Self> {
        let source = sources.candidate(&input.source_id)?;
        if source.content_digest != input.expected_digest {
            return Err(error(
                "SOURCE_BUILD_SOURCE_CHANGED",
                "源码摘要变化，请刷新源码并重新确认运行脚本。",
            ));
        }
        if source.inspection["kind"] != "project-v1-source" {
            return Err(invalid("仅支持已检查的平台网页源码候选。"));
        }
        let manifest: platform_project_format::ProjectManifest = serde_json::from_slice(
            &source
                .root
                .read_bounded("project.json", 256 * 1024)
                .map_err(fs)?
                .bytes,
        )?;
        for path in [&manifest.source_dir, &manifest.output_dir, &manifest.entry] {
            safe_path(path)?;
        }
        if overlaps(&manifest.source_dir, &manifest.output_dir)
            || !manifest
                .entry
                .starts_with(&format!("{}/", manifest.output_dir))
        {
            return Err(invalid("源码、产物或网页入口声明无效。"));
        }
        let script = manifest.build["script"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && !s.starts_with('-')
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
            })
            .ok_or_else(|| invalid("构建脚本名称无效。"))?
            .to_owned();
        let node = Arc::new(tools.lease(&data, "node")?);
        let git = Arc::new(tools.lease(&data, "git")?);
        let candidate = directory.create_directory("candidate").map_err(fs)?;
        let artifact = directory.create_directory("artifact").map_err(fs)?;
        let home = directory.create_directory("home").map_err(fs)?;
        let temporary = directory.create_directory("temporary").map_err(fs)?;
        home.write_new("user.npmrc", b"").map_err(fs)?;
        home.write_new("global.npmrc", b"").map_err(fs)?;
        let configroot = WindowsSourceDirectory::from_workspace(&home).map_err(fs)?;
        let configs = configroot
            .source_entries(20)
            .map_err(fs)?
            .iter()
            .map(|entry| configroot.pin_source_entry(entry).map(|p| p.0).map_err(fs))
            .collect::<Result<Vec<_>>>()?;
        source.copy_subdirectory_to_with_cancellation(&manifest.source_dir, &candidate, cancel)?;
        let root = WindowsSourceDirectory::from_workspace(&candidate).map_err(fs)?;
        let baseline = inventory(&root, true)?;
        validate_materials(&root, &baseline, &script)?;
        budget(&directory)?;
        Ok(Self {
            source,
            node,
            git,
            candidate,
            artifact,
            home,
            temporary,
            script,
            entry: manifest.entry[manifest.output_dir.len() + 1..].into(),
            baseline,
            _configs: configs,
        })
    }
}
async fn checked(prepared: Prepared) -> Result<Prepared> {
    tokio::task::spawn_blocking(move || {
        prepared.source.verify()?;
        prepared.node.verify()?;
        prepared.git.verify()?;
        let root = ordinary(&prepared.candidate)?;
        let after = inventory(&root, true)?;
        if prepared.baseline.digest != after.digest
            || prepared.baseline.files.iter().any(|(path, before)| {
                after
                    .files
                    .get(path)
                    .is_none_or(|after| before.identity != after.identity)
            })
        {
            return Err(error(
                "SOURCE_BUILD_INPUT_CHANGED",
                "安装或构建修改了候选输入，未接受产物；现场已保留。",
            ));
        }
        // Installed workspace junctions are rejected until their HANDLE-based
        // target validation is implemented; an unresolved link is never followed.
        disk_tree(&root, &mut 0, &mut 0)?;
        Ok(prepared)
    })
    .await
    .map_err(|_| error("SOURCE_BUILD_INTERRUPTED", "候选复核被中断。"))?
}
fn safe_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 240
        || path.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || p.ends_with(['.', ' '])
                || p.chars()
                    .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
        })
    {
        return Err(invalid("项目路径必须为无别名的相对路径。"));
    }
    Ok(())
}
fn overlaps(left: &str, right: &str) -> bool {
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    left == right
        || left.starts_with(&format!("{right}/"))
        || right.starts_with(&format!("{left}/"))
}
fn generated(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "node_modules"
            | ".cache"
            | ".vite"
            | ".next"
            | ".nuxt"
            | "coverage"
            | "tmp"
            | ".tmp"
            | "temp"
    )
}
// npm creates files with safe inherited ACLs, not the platform's protected-ACL
// object format. Keep the admitted private root bound, and read its ordinary
// descendants without repairing or taking over their ACLs.
fn ordinary(root: &WorkspaceDirectory) -> Result<WindowsSourceDirectory> {
    root.verify_binding().map_err(fs)?;
    let source = WindowsSourceDirectory::open(root.path()).map_err(fs)?;
    if source.identity() != root.identity() {
        return Err(invalid("候选根目录身份变化。"));
    }
    Ok(source)
}
#[derive(Clone)]
struct InputFile {
    identity: WindowsObjectIdentity,
    digest: String,
    bytes: u64,
}
struct Inventory {
    files: BTreeMap<String, InputFile>,
    digest: String,
    _guards: Vec<WindowsSourceDirectory>,
}
fn inventory(root: &WindowsSourceDirectory, inputs: bool) -> Result<Inventory> {
    struct Scan {
        files: BTreeMap<String, InputFile>,
        total: u64,
        hash: Sha256,
        guards: Vec<WindowsSourceDirectory>,
    }
    fn visit(
        root: &WindowsSourceDirectory,
        prefix: &str,
        inputs: bool,
        scan: &mut Scan,
    ) -> Result<()> {
        let mut entries = root.source_entries(40_000).map_err(fs)?;
        entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        for entry in entries {
            if inputs && generated(&entry.name) {
                continue;
            }
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            safe_path(&path)?;
            let (pin, _) = root.pin_source_entry(&entry).map_err(fs)?;
            if entry.is_directory {
                visit(&pin, &path, inputs, scan)?;
            } else {
                scan.total = scan
                    .total
                    .checked_add(entry.length)
                    .ok_or_else(|| invalid("候选内容超限。"))?;
                if scan.files.len() >= 10_000
                    || scan.total > 256 * 1024 * 1024
                    || entry.length > 8_000_000
                {
                    return Err(error(
                        "SOURCE_BUILD_LIMIT",
                        "候选文件或产物超过10,000文件、256MiB或单文件8,000,000字节限制。",
                    ));
                }
                let file = root.read_bounded(&entry.name, 8_000_000).map_err(fs)?;
                if file.identity != entry.identity {
                    return Err(error(
                        "SOURCE_BUILD_INPUT_CHANGED",
                        "候选读取期间身份变化。",
                    ));
                }
                platform_project_format::update_file_header(&mut scan.hash, &path, entry.length);
                scan.hash.update(&file.bytes);
                scan.files.insert(
                    path,
                    InputFile {
                        identity: file.identity,
                        digest: format!("{:x}", Sha256::digest(&file.bytes)),
                        bytes: entry.length,
                    },
                );
            }
            pin.verify_binding().map_err(fs)?;
            if !inputs {
                scan.guards.push(pin);
            }
        }
        root.verify_binding().map_err(fs)
    }
    let mut scan = Scan {
        files: BTreeMap::new(),
        total: 0,
        hash: Sha256::new(),
        guards: Vec::new(),
    };
    visit(root, "", inputs, &mut scan)?;
    Ok(Inventory {
        files: scan.files,
        digest: format!("{:x}", scan.hash.finalize()),
        _guards: scan.guards,
    })
}
fn disk_tree(root: &WindowsSourceDirectory, bytes: &mut u64, entries: &mut usize) -> Result<()> {
    disk_tree_at(root, bytes, entries, 0)
}
fn disk_tree_at(
    root: &WindowsSourceDirectory,
    bytes: &mut u64,
    entries: &mut usize,
    depth: usize,
) -> Result<()> {
    if depth > 64 {
        return Err(error(
            "SOURCE_BUILD_LIMIT",
            "候选目录超过64层，现场已保留。",
        ));
    }
    for entry in root.source_entries(40_000).map_err(fs)? {
        *entries += 1;
        *bytes = bytes
            .checked_add(entry.length)
            .ok_or_else(|| invalid("候选容量超限。"))?;
        if *entries > 100_000 || *bytes > DISK_LIMIT {
            return Err(error(
                "SOURCE_BUILD_LIMIT",
                "候选任务超过512MiB或100,000条目录证据限制，现场已保留。",
            ));
        }
        if entry.reparse {
            return Err(error(
                "SOURCE_BUILD_LINK_UNSUPPORTED",
                "此Windows候选暂不支持依赖链接或目录联接；未跟随链接，现场已保留。",
            ));
        }
        if entry.is_directory {
            let child = root.open_directory(&entry.name).map_err(fs)?;
            if child.identity() != &entry.identity {
                return Err(invalid("候选目录身份变化。"));
            }
            disk_tree_at(&child, bytes, entries, depth + 1)?;
        }
    }
    root.verify_binding().map_err(fs)
}
fn budget(root: &WorkspaceDirectory) -> Result<()> {
    let source = ordinary(root)?;
    disk_tree(&source, &mut 0, &mut 0)?;
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = OsString::from(format!("\\\\?\\{}", root.path().display()))
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut available = 0;
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
        || available < 64 * 1024 * 1024
    {
        return Err(error(
            "SOURCE_BUILD_SPACE_UNAVAILABLE",
            "任务容量或至少64MiB剩余空间无法确认，现场已保留。",
        ));
    }
    root.verify_binding().map_err(fs)
}
fn read_json(root: &WindowsSourceDirectory, path: &str) -> Result<Value> {
    let mut parts = path.rsplitn(2, '/');
    let name = parts.next().unwrap();
    let directory = if let Some(prefix) = parts.next() {
        let mut directory = root.clone();
        for name in prefix.split('/') {
            directory = directory.open_directory(name).map_err(fs)?;
        }
        directory
    } else {
        root.clone()
    };
    Ok(serde_json::from_slice(
        &directory.read_bounded(name, 256 * 1024).map_err(fs)?.bytes,
    )?)
}
// Match main's static webpage checks, using the already pinned NTFS tree.
fn validate_artifact(
    root: &WindowsSourceDirectory,
    snapshot: &Inventory,
    entry: &str,
) -> Result<()> {
    let html = |path: &str| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            == "text/html"
    };
    let invalid = |message: &str| error("BUILD_ARTIFACT_INVALID", message);
    if !snapshot.files.contains_key(entry) || !html(entry) {
        return Err(invalid("构建未生成清单要求的HTML入口。"));
    }
    let html_refs = regex::Regex::new(r#"(?i)(?:src|href)\s*=\s*["']([^"']+)["']"#).unwrap();
    let css_refs = regex::Regex::new(r#"(?i)url\(\s*["']?([^)'"\s]+)"#).unwrap();
    let js_refs = regex::Regex::new(r#"(?:from\s*|import\s*\(\s*)["'](\.[^"']+)["']"#).unwrap();
    let base_tag = regex::Regex::new(r"(?i)<base(?:\s|>)").unwrap();
    for (path, expected) in &snapshot.files {
        let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let is_html = html(path);
        if !is_html && !matches!(extension.as_str(), "css" | "js" | "mjs") {
            continue;
        }
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let mut directory = root.clone();
        for name in parent.split('/').filter(|p| !p.is_empty()) {
            directory = directory.open_directory(name).map_err(fs)?;
        }
        let file = directory.read_bounded(name, 8_000_000).map_err(fs)?;
        if file.identity != expected.identity
            || format!("{:x}", Sha256::digest(&file.bytes)) != expected.digest
        {
            return Err(invalid("产物在校验期间变化。"));
        }
        let text = std::str::from_utf8(&file.bytes)
            .map_err(|_| invalid("HTML/CSS/JavaScript产物必须为UTF-8。"))?;
        if is_html && base_tag.is_match(text) {
            return Err(invalid("网页产物不能包含base标签。"));
        }
        let refs = if is_html {
            &html_refs
        } else if extension == "css" {
            &css_refs
        } else {
            &js_refs
        };
        for capture in refs.captures_iter(text) {
            let reference = &capture[1];
            if reference.starts_with('#')
                || ["data:", "blob:", "http:", "https:", "mailto:", "tel:"]
                    .iter()
                    .any(|prefix| reference.starts_with(prefix))
            {
                continue;
            }
            let reference = reference.split(['?', '#']).next().unwrap();
            if reference.is_empty() {
                continue;
            }
            let target = local_path(parent, reference)
                .map_err(|_| invalid("网页资源必须使用产物目录内的相对路径。"))?;
            if !snapshot.files.contains_key(&target) {
                return Err(invalid(&format!(
                    "产物{path}引用了缺失的本机资源{target}。"
                )));
            }
        }
    }
    Ok(())
}
fn validate_materials(
    root: &WindowsSourceDirectory,
    inventory: &Inventory,
    script: &str,
) -> Result<()> {
    if inventory.files.contains_key("npm-shrinkwrap.json") {
        return Err(invalid(
            "根npm-shrinkwrap.json会覆盖已验证的锁文件，请先适配源码。",
        ));
    }
    let package = read_json(root, "package.json")?;
    let lock = read_json(root, "package-lock.json")?;
    validate_lock(&package, &lock)?;
    if package["scripts"][script]
        .as_str()
        .is_none_or(str::is_empty)
    {
        return Err(invalid("package.json未声明对应的构建脚本。"));
    }
    for path in inventory.files.keys().filter(|p| {
        matches!(
            p.rsplit('/').next(),
            Some("package.json" | "package-lock.json" | "npm-shrinkwrap.json")
        )
    }) {
        let json = read_json(root, path)?;
        let base = path.rsplit_once('/').map(|p| p.0).unwrap_or("");
        validate_dependencies(&json, base, inventory)?;
    }
    Ok(())
}
fn validate_lock(package: &Value, lock: &Value) -> Result<()> {
    if !matches!(lock["lockfileVersion"].as_u64(), Some(2 | 3))
        || !lock.pointer("/packages/").is_some_and(Value::is_object)
    {
        return Err(error(
            "LOCKFILE_INVALID",
            "请使用含根包声明的npm lockfileVersion 2或3锁文件。",
        ));
    }
    for key in ["dependencies", "devDependencies", "optionalDependencies"] {
        let empty = json!({});
        if package.get(key).unwrap_or(&empty) != lock["packages"][""].get(key).unwrap_or(&empty) {
            return Err(error(
                "LOCKFILE_MISMATCH",
                "package.json与锁文件的根依赖声明不一致，请先更新锁文件。",
            ));
        }
    }
    Ok(())
}
fn local_path(base: &str, path: &str) -> Result<String> {
    if path.is_empty()
        || path.starts_with(['/', '~'])
        || path.contains(['\\', '%', ':', '?', '#', '{', '['])
        || path.chars().any(char::is_control)
    {
        return Err(invalid("本机依赖必须使用候选源码内的普通相对路径。"));
    }
    let mut parts: Vec<_> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("本机依赖不能离开sourceDir。"));
                }
            }
            value => parts.push(value),
        }
    }
    let resolved = parts.join("/");
    safe_path(&resolved)?;
    Ok(resolved)
}
fn local_target(base: &str, path: &str, inventory: &Inventory) -> Result<()> {
    let resolved = local_path(base, path)?;
    if !inventory
        .files
        .keys()
        .any(|p| p == &resolved || p.starts_with(&format!("{resolved}/")))
    {
        return Err(invalid("本机依赖材料未包含在候选sourceDir中。"));
    }
    Ok(())
}
fn validate_dependencies(value: &Value, base: &str, inventory: &Inventory) -> Result<()> {
    fn spec(value: &str, base: &str, inventory: &Inventory, force: bool) -> Result<()> {
        let lower = value.to_ascii_lowercase();
        if lower.starts_with("git+file:") {
            return Err(invalid(
                "本机Git依赖不能离开候选源码目录；请改用源码内的普通依赖材料。",
            ));
        }
        if lower.starts_with("file:") || lower.starts_with("link:") {
            local_target(base, &value[5..], inventory)
        } else if force
            || value.starts_with(['.', '/'])
            || value.starts_with("~/")
            || value.contains('\\')
            || (value.len() > 1 && value.as_bytes()[1] == b':')
        {
            local_target(base, value, inventory)
        } else {
            Ok(())
        }
    }
    fn package(value: &Value, base: &str, inventory: &Inventory) -> Result<()> {
        if value.get("workspaces").is_some() {
            return Err(error(
                "SOURCE_BUILD_LINK_UNSUPPORTED",
                "此Windows候选暂不支持npm workspace目录联接，请先使用普通包源码。",
            ));
        }
        fn deps(value: &Value, base: &str, inventory: &Inventory) -> Result<()> {
            if let Some(value) = value.as_str() {
                spec(value, base, inventory, false)?;
            } else if let Some(object) = value.as_object() {
                for value in object.values() {
                    deps(value, base, inventory)?;
                }
            }
            Ok(())
        }
        for key in [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
            "overrides",
        ] {
            if let Some(value) = value.get(key) {
                deps(value, base, inventory)?;
            }
        }
        Ok(())
    }
    package(value, base, inventory)?;
    if let Some(packages) = value["packages"].as_object() {
        for (name, value) in packages {
            let location = if name.is_empty() {
                base.to_owned()
            } else {
                local_path(base, name)?
            };
            package(value, &location, inventory)?;
            if let Some(resolved) = value["resolved"].as_str() {
                spec(resolved, base, inventory, value["link"] == true)?;
            }
        }
    }
    // v2's recursive legacy graph interprets local resolved/version entries
    // relative to the lockfile, whereas packages entries have their own bases.
    fn nested(value: &Value, base: &str, inventory: &Inventory) -> Result<()> {
        if let Some(values) = value.as_object() {
            for (key, value) in values {
                if matches!(key.as_str(), "resolved" | "version") {
                    if let Some(value) = value.as_str() {
                        spec(value, base, inventory, false)?;
                    }
                }
                nested(value, base, inventory)?;
            }
        } else if let Some(values) = value.as_array() {
            for value in values {
                nested(value, base, inventory)?;
            }
        }
        Ok(())
    }
    nested(&value["dependencies"], base, inventory)?;
    Ok(())
}

#[cfg(test)]
#[path = "windows_source_builds_tests.rs"]
mod tests;
