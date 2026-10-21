//! Explicit, non-executing source acquisition. A source slot is not a project or
//! an installed application. All persisted paths are relative to its workspace.
use crate::{
    builds::{atomic_json, BuildManager},
    environment::ExecutableInfo,
    git_environment::{GitEnvironmentManager, GitToolchain},
    node_app_manifest::{TrialManifest, TrialRuntime},
    projects::{check_no_symlinks, checked_relative, valid_id, validate_manifest},
    ApiError, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::watch,
    task::JoinHandle,
    time::Instant,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(600);
const MAX_FILES: usize = 10_000;
const MAX_BLOB: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 256 * 1024 * 1024;
const MAX_DISK: u64 = 512 * 1024 * 1024;
const MIN_FREE: u64 = 64 * 1024 * 1024;
const MAX_METADATA: u64 = 256 * 1024;
const MAX_TREE_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireSourceInput {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceInspection {
    pub kind: String,
    pub name: Option<String>,
    pub next_step: String,
    pub execution_state: String,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSourceInspection {
    pub source_path: PathBuf,
    pub inspection: SourceInspection,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceRecord {
    schema_version: u32,
    source_id: String,
    task_id: String,
    name: String,
    status: String,
    phase: String,
    url: String,
    provider: String,
    created_at: u64,
    completed_at: Option<u64>,
    repository: String,
    shallow: bool,
    git: Option<ExecutableInfo>,
    commit: Option<String>,
    file_count: Option<usize>,
    total_bytes: Option<u64>,
    content_digest: Option<String>,
    inspection: Option<SourceInspection>,
    error: Option<ApiError>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCandidate {
    pub source_id: String,
    pub task_id: String,
    pub name: String,
    pub status: String,
    pub phase: String,
    pub url: String,
    pub provider: String,
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub source_path: Option<PathBuf>,
    pub evidence_path: PathBuf,
    pub shallow: bool,
    pub git: Option<ExecutableInfo>,
    pub commit: Option<String>,
    pub file_count: Option<usize>,
    pub total_bytes: Option<u64>,
    pub content_digest: Option<String>,
    pub inspection: Option<SourceInspection>,
    pub error: Option<ApiError>,
    pub execution_state: String,
    pub registration_available: bool,
}

pub struct SourceAcquisitionManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    git: GitEnvironmentManager,
    active: Mutex<HashMap<String, watch::Sender<bool>>>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    submission: Mutex<()>,
    stopping: AtomicBool,
}
impl SourceAcquisitionManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        git: GitEnvironmentManager,
    ) -> Result<Arc<Self>, ApiError> {
        let manager = Arc::new(Self {
            paths,
            store,
            tasks,
            git,
            active: Mutex::new(HashMap::new()),
            runners: Mutex::new(Vec::new()),
            submission: Mutex::new(()),
            stopping: AtomicBool::new(false),
        });
        // This area is intentionally not a cache. workspace::exclusions does not
        // exclude it, so public workspace migration copies records and payloads.
        manager.recover()?;
        Ok(manager)
    }
    fn root(&self) -> PathBuf {
        self.paths.workspace_root().join(".platform/sources")
    }
    fn slot(&self, id: &str) -> Result<PathBuf, ApiError> {
        if !source_id(id) {
            return Err(error("SOURCE_NOT_FOUND", "未找到该源码候选。"));
        }
        let path = self.root().join(id);
        check_no_symlinks(&path, false)?;
        if !fs::metadata(&path)?.is_dir() {
            return Err(invalid("源码候选目录无效。"));
        }
        Ok(path)
    }
    fn read_record(&self, id: &str) -> Result<SourceRecord, ApiError> {
        let record: SourceRecord =
            serde_json::from_slice(&read_small(&self.slot(id)?.join("source.json"))?)
                .map_err(|_| invalid("源码候选记录无效，请保留现场。"))?;
        if record.schema_version != 1
            || record.source_id != id
            || !valid_id(&record.task_id)
            || record.repository != "repository"
            || !matches!(
                record.status.as_str(),
                "acquiring" | "ready" | "failed" | "cancelled" | "interrupted"
            )
        {
            return Err(invalid("源码候选记录身份不符，请保留现场。"));
        }
        Ok(record)
    }
    fn save_record(&self, record: &SourceRecord) -> Result<(), ApiError> {
        let path = self.slot(&record.source_id)?.join("source.json");
        if path.try_exists()? {
            ordinary_file(&path)?;
        }
        atomic_json(&path, record)
    }
    fn view(&self, record: SourceRecord) -> Result<SourceCandidate, ApiError> {
        let slot = self.slot(&record.source_id)?;
        let source_path = if record.status == "ready" {
            let payload = slot.join("repository");
            check_no_symlinks(&payload, false)?;
            if !fs::metadata(&payload)?.is_dir() {
                return Err(invalid("已获取的源码目录不可用，请保留候选记录。"));
            }
            Some(payload)
        } else {
            None
        };
        Ok(SourceCandidate {
            source_id: record.source_id,
            task_id: record.task_id,
            name: record.name,
            status: record.status,
            phase: record.phase,
            url: record.url,
            provider: record.provider,
            created_at: record.created_at,
            completed_at: record.completed_at,
            source_path,
            evidence_path: slot,
            shallow: record.shallow,
            git: record.git,
            commit: record.commit,
            file_count: record.file_count,
            total_bytes: record.total_bytes,
            content_digest: record.content_digest,
            inspection: record.inspection,
            error: record.error,
            execution_state: "not-executed".into(),
            registration_available: false,
        })
    }
    pub fn info(&self, id: &str) -> Result<SourceCandidate, ApiError> {
        self.view(self.read_record(id)?)
    }
    /// Recheck downloaded bytes before any later operation. Inspection alone is
    /// deliberately not an authorization to execute a mutable source directory.
    pub fn verify_ready(
        &self,
        id: &str,
        expected_digest: &str,
    ) -> Result<SourceCandidate, ApiError> {
        self.store.check_workspace_available()?;
        let _lease = self
            .store
            .reserve_transient_operation(&format!("source:{id}"), "source-verify")?;
        let record = self.read_record(id)?;
        if record.status != "ready" || record.content_digest.as_deref() != Some(expected_digest) {
            return Err(error(
                "SOURCE_CHANGED",
                "源码状态或摘要已变化，请重新获取并核对。",
            ));
        }
        let candidate = self.view(record)?;
        let path = candidate
            .source_path
            .as_ref()
            .ok_or_else(|| invalid("源码目录不可用。"))?;
        if directory_digest(path)? != expected_digest {
            return Err(error(
                "SOURCE_CHANGED",
                "下载后的源码已经改变，未执行接入。请重新获取或明确处理修改。",
            ));
        }
        Ok(candidate)
    }
    pub fn list(&self) -> Result<Vec<SourceCandidate>, ApiError> {
        let root = self.root();
        check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            return Ok(Vec::new());
        }
        check_no_symlinks(&root, false)?;
        let mut records = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if source_id(&id) {
                records.push(self.info(&id).unwrap_or_else(|_| self.unavailable(&id)));
            }
        }
        records.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then(a.source_id.cmp(&b.source_id))
        });
        Ok(records)
    }
    fn unavailable(&self, id: &str) -> SourceCandidate {
        SourceCandidate {
            source_id: id.into(),
            task_id: String::new(),
            name: id.into(),
            status: "unavailable".into(),
            phase: "record-unavailable".into(),
            url: String::new(),
            provider: String::new(),
            created_at: 0,
            completed_at: None,
            source_path: None,
            evidence_path: self.root().join(id),
            shallow: true,
            git: None,
            commit: None,
            file_count: None,
            total_bytes: None,
            content_digest: None,
            inspection: None,
            error: Some(invalid(
                "源码候选记录或路径不可用，已保留现场；其他候选仍可查看。",
            )),
            execution_state: "not-executed".into(),
            registration_available: false,
        }
    }
    pub fn inspect_local(&self, path: PathBuf) -> Result<LocalSourceInspection, ApiError> {
        self.store.check_workspace_available()?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        Ok(LocalSourceInspection {
            inspection: inspect_directory(&path)?,
            source_path: path,
        })
    }
    pub async fn submit(
        self: &Arc<Self>,
        input: AcquireSourceInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let (url, provider, repository_name) = validate_url(&input.url)?;
        let name = input.name.unwrap_or(repository_name);
        if name.trim().is_empty()
            || name.chars().count() > 200
            || name.chars().any(char::is_control)
        {
            return Err(invalid("源码名称须为 1 到 200 个有效字符。"));
        }
        let _submission = self.submission.lock().unwrap();
        self.store.check_workspace_available()?;
        if self.stopping.load(Ordering::Acquire) || *stop.borrow() || stop.has_changed().is_err() {
            return Err(interrupted());
        }
        if !self.active.lock().unwrap().is_empty() {
            return Err(error(
                "SOURCE_ACQUISITION_BUSY",
                "已有源码正在获取，请等待完成或取消。",
            ));
        }
        let task = self.tasks.create_workspace_operation("sourceAcquire")?;
        let id = format!("src_{}", Uuid::new_v4().simple());
        // This lease is deliberately not a project registration/reservation. The
        // migration gate waits for it through has_active_tasks().
        let lease = match self
            .store
            .reserve_task(&format!("source:{id}"), &task.task_id)
        {
            Ok(lease) => lease,
            Err(error) => {
                self.tasks.finish(&task.task_id, Err(error.clone()))?;
                return Err(error);
            }
        };
        let record = SourceRecord {
            schema_version: 1,
            source_id: id.clone(),
            task_id: task.task_id.clone(),
            name: name.trim().into(),
            status: "acquiring".into(),
            phase: "queued".into(),
            url,
            provider,
            created_at: now(),
            completed_at: None,
            repository: "repository".into(),
            shallow: true,
            git: None,
            commit: None,
            file_count: None,
            total_bytes: None,
            content_digest: None,
            inspection: None,
            error: None,
        };
        let prepared = (|| {
            private_dir(&self.root())?;
            new_dir(&self.root().join(&id))?;
            self.save_record(&record)?;
            self.tasks.append(&task.task_id, format!("Source candidate: {id}\nPublic URL: {}\nWorkspace path: .platform/sources/{id}\nNo project code is executed.\n", record.url).as_bytes())
        })();
        if let Err(error) = prepared {
            self.tasks.finish(&task.task_id, Err(error.clone()))?;
            return Err(error);
        }
        let (sender, cancel) = watch::channel(false);
        self.active
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), sender);
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            let _lease = lease;
            manager.run(record, stop, cancel).await;
        });
        let mut runners = self.runners.lock().unwrap();
        runners.retain(|runner| !runner.is_finished());
        runners.push(runner);
        Ok(task)
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord, ApiError> {
        let task = self.tasks.task(task_id)?;
        if task.task_type != "sourceAcquire" {
            return Err(error("SOURCE_TASK_INVALID", "该任务不是源码获取任务。"));
        }
        if let Some(cancel) = self.active.lock().unwrap().get(task_id) {
            let _ = cancel.send(true);
        }
        Ok(task)
    }
    pub async fn shutdown(&self) {
        {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            for cancel in self.active.lock().unwrap().values() {
                let _ = cancel.send(true);
            }
        }
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        for runner in runners {
            let _ = runner.await;
        }
    }
    fn recover(&self) -> Result<(), ApiError> {
        let root = self.root();
        check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            return Ok(());
        }
        check_no_symlinks(&root, false)?;
        for entry in fs::read_dir(root)? {
            let id = entry?.file_name().to_string_lossy().into_owned();
            if !source_id(&id) {
                continue;
            }
            let Ok(mut record) = self.read_record(&id) else {
                continue;
            };
            if record.status == "acquiring" {
                record.status = "interrupted".into();
                record.error = Some(interrupted());
                record.completed_at = Some(now());
                self.save_record(&record)?;
                if self.tasks.task(&record.task_id).is_ok() {
                    self.tasks.reconcile_operation_result(&record.task_id, serde_json::json!({"sourceId":id,"operationOutcome":"interrupted","partialFilesRetained":true,"recovered":true}))?;
                }
            } else if record.status == "ready" && self.tasks.task(&record.task_id).is_ok() {
                self.tasks.reconcile_operation_result(&record.task_id, serde_json::json!({"sourceId":id,"operationOutcome":"committed","recovered":true}))?;
            }
        }
        Ok(())
    }
    fn phase(&self, record: &mut SourceRecord, phase: &str) -> Result<(), ApiError> {
        record.phase = phase.into();
        self.save_record(record)?;
        self.tasks.phase(&record.task_id, phase)?;
        self.tasks
            .append(&record.task_id, format!("Phase: {phase}\n").as_bytes())
    }
    async fn run(
        &self,
        mut record: SourceRecord,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) {
        let mut control = RunControl {
            stop,
            cancel,
            deadline: Instant::now() + TIMEOUT,
            monitor: self.root().join(&record.source_id),
        };
        let result = self
            .acquire(&mut record, &mut control)
            .await
            .map_err(|problem| {
                if self.stopping.load(Ordering::Acquire)
                    && problem.code == "SOURCE_ACQUISITION_CANCELLED"
                {
                    interrupted()
                } else {
                    problem
                }
            });
        let result = match result {
            Ok(()) => {
                record.status = "ready".into();
                record.phase = "completed".into();
                record.completed_at = Some(now());
                self.save_record(&record)
                    .and_then(|()| self.view(record.clone()))
                    .and_then(|candidate| serde_json::to_value(candidate).map_err(ApiError::from))
            }
            Err(error) => {
                record.status = match error.code.as_str() {
                    "SOURCE_ACQUISITION_CANCELLED" => "cancelled",
                    "SOURCE_ACQUISITION_INTERRUPTED" => "interrupted",
                    _ => "failed",
                }
                .into();
                record.completed_at = Some(now());
                record.error = Some(error.clone());
                if let Err(persist) = self.save_record(&record) {
                    let _ = self.tasks.append(
                        &record.task_id,
                        format!(
                            "Source outcome record could not be saved: {}\n",
                            persist.code
                        )
                        .as_bytes(),
                    );
                }
                // Partial payloads are retained as operation evidence. A retry is
                // a new slot; no previous or unknown directory is deleted.
                Err(error
                    .detail("sourceId", record.source_id.clone())
                    .outcome("not_registered"))
            }
        };
        let _ = self.tasks.append(
            &record.task_id,
            format!("Result: {}\n", record.status).as_bytes(),
        );
        let _ = self.tasks.finish(&record.task_id, result);
        self.active.lock().unwrap().remove(&record.task_id);
    }
    async fn acquire(
        &self,
        record: &mut SourceRecord,
        control: &mut RunControl,
    ) -> Result<(), ApiError> {
        self.phase(record, "environment")?;
        let toolchain = supervise(control, self.git.resolve()).await?;
        if !supported_version(&toolchain.git.version) {
            return Err(error(
                "SOURCE_GIT_VERSION_UNSUPPORTED",
                "源码获取目前仅支持已核验的 Git 2.54.0 或 2.56.0。",
            ));
        }
        record.git = Some(toolchain.git.clone());
        self.save_record(record)?;
        let slot = self.slot(&record.source_id)?;
        let stage = slot.join(".acquiring");
        new_dir(&stage)?;
        let home = stage.join("home");
        new_dir(&home)?;
        let template = stage.join("empty-template");
        new_dir(&template)?;
        let repository = stage.join("repository");
        self.phase(record, "downloading")?;
        let mut clone = isolated_command(&toolchain, &home, &home, &template);
        clone
            .args([
                "clone",
                "--no-checkout",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                "--no-local",
                "--no-recurse-submodules",
            ])
            .arg(format!("--template={}", template.display()))
            .arg("--")
            .arg(&record.url)
            .arg(&repository);
        execute(clone, control, MAX_OUTPUT).await?;
        self.phase(record, "validating-objects")?;
        let mut head = isolated_command(&toolchain, &repository, &home, &template);
        head.args(["rev-parse", "--verify", "HEAD^{commit}"]);
        let commit = parse_commit(&execute(head, control, MAX_OUTPUT).await?)?;
        let mut fsck = isolated_command(&toolchain, &repository, &home, &template);
        fsck.args(["fsck", "--full", "--strict", "--no-reflogs"]);
        execute(fsck, control, MAX_OUTPUT).await?;
        let mut tree = isolated_command(&toolchain, &repository, &home, &template);
        tree.args(["ls-tree", "-r", "-z", "--long", &commit]);
        let entries = parse_tree(&execute(tree, control, MAX_TREE_OUTPUT).await?)?;
        record.commit = Some(commit.clone());
        record.file_count = Some(entries.len());
        record.total_bytes = Some(entries.iter().map(|entry| entry.size).sum());
        self.phase(record, "materializing")?;
        record.content_digest =
            Some(materialize(&toolchain, &repository, &home, &template, &entries, control).await?);
        // Populate only Git's index. No checkout, attributes filters or project
        // scripts run, and later status sees the materialized tree accurately.
        let mut index = isolated_command(&toolchain, &repository, &home, &template);
        index.args(["read-tree", &commit]);
        execute(index, control, MAX_OUTPUT).await?;
        self.phase(record, "inspecting")?;
        control.check()?;
        record.inspection = Some(inspect_directory(&repository)?);
        check_disk(&slot)?;
        control.check()?;
        let published = slot.join("repository");
        if published.try_exists()? {
            return Err(invalid("候选目录已经存在，未覆盖。"));
        }
        fs::rename(&repository, &published)?;
        fs::File::open(&slot)?.sync_all()?;
        Ok(())
    }
}

fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn invalid(message: &str) -> ApiError {
    error("SOURCE_INVALID", message)
}
fn interrupted() -> ApiError {
    error(
        "SOURCE_ACQUISITION_INTERRUPTED",
        "服务已停止，源码获取中断；已保留本次现场，不会自动重试。",
    )
}
fn cancelled() -> ApiError {
    error(
        "SOURCE_ACQUISITION_CANCELLED",
        "源码获取已取消，已保留本次现场。",
    )
}
fn limit() -> ApiError {
    error(
        "SOURCE_LIMIT_EXCEEDED",
        "仓库超过本次源码获取的文件、大小、路径或磁盘预算。",
    )
}
fn process_failed() -> ApiError {
    error(
        "SOURCE_GIT_FAILED",
        "源码获取未成功；请检查公开仓库地址和网络，或重新发起获取。",
    )
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn source_id(id: &str) -> bool {
    id.len() == 36 && id.starts_with("src_") && id[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn new_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    if !path.try_exists()? {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("预期为普通目录。"));
    }
    Ok(())
}
fn ordinary_file(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("仅支持普通且无链接的文件。"));
    }
    Ok(())
}
fn read_small(path: &Path) -> Result<Vec<u8>, ApiError> {
    ordinary_file(path)?;
    let before = fs::symlink_metadata(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.nlink() != 1
        || opened.dev() != before.dev()
        || opened.ino() != before.ino()
    {
        return Err(invalid("声明文件在读取时发生变化。"));
    }
    if opened.len() > MAX_METADATA {
        return Err(limit());
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    let after = fs::symlink_metadata(path)?;
    if after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != bytes.len() as u64
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
    {
        return Err(invalid("声明文件在读取时发生变化。"));
    }
    Ok(bytes)
}

fn directory_digest(root: &Path) -> Result<String, ApiError> {
    fn collect(
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<String, PathBuf>,
        depth: usize,
    ) -> Result<(), ApiError> {
        if depth > 32 {
            return Err(limit());
        }
        check_no_symlinks(path, false)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if path == root && entry.file_name() == ".git" {
                continue;
            }
            let full = entry.path();
            let metadata = fs::symlink_metadata(&full)?;
            if metadata.is_dir() {
                collect(root, &full, files, depth + 1)?;
            } else {
                ordinary_file(&full)?;
                let relative = full
                    .strip_prefix(root)
                    .map_err(|_| invalid("源码路径越界。"))?
                    .to_str()
                    .ok_or_else(|| invalid("源码路径不是 UTF-8。"))?
                    .to_owned();
                validate_tree_path(&relative)?;
                if metadata.len() > MAX_BLOB || files.len() >= MAX_FILES {
                    return Err(limit());
                }
                files.insert(relative, full);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(root, root, &mut files, 0)?;
    let mut tree = Sha256::new();
    let mut total = 0u64;
    for (relative, path) in files {
        let before = fs::symlink_metadata(&path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let opened = file.metadata()?;
        if !opened.is_file()
            || opened.nlink() != 1
            || opened.dev() != before.dev()
            || opened.ino() != before.ino()
        {
            return Err(invalid("源码在校验时改变。"));
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0u8; 65536];
        let mut size = 0u64;
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            total += n as u64;
            if size > MAX_BLOB || total > MAX_TREE {
                return Err(limit());
            }
            digest.update(&buffer[..n]);
        }
        let after = fs::symlink_metadata(&path)?;
        if after.dev() != before.dev()
            || after.ino() != before.ino()
            || after.len() != size
            || after.mode() != before.mode()
            || after.mtime() != before.mtime()
            || after.mtime_nsec() != before.mtime_nsec()
        {
            return Err(invalid("源码在校验时改变。"));
        }
        tree.update(relative.as_bytes());
        tree.update([0]);
        tree.update(size.to_le_bytes());
        tree.update([u8::from(before.mode() & 0o111 != 0)]);
        tree.update(digest.finalize());
    }
    Ok(format!("{:x}", tree.finalize()))
}
fn validate_url(value: &str) -> Result<(String, String, String), ApiError> {
    let bad = || {
        error(
            "SOURCE_URL_INVALID",
            "请输入标准 GitHub 或 Gitee 的公开 HTTPS 仓库地址，不包含凭据、端口、查询或额外路径。",
        )
    };
    let (host, provider, path) = if let Some(path) = value.strip_prefix("https://github.com/") {
        ("github.com", "github", path)
    } else if let Some(path) = value.strip_prefix("https://gitee.com/") {
        ("gitee.com", "gitee", path)
    } else {
        return Err(bad());
    };
    if value.len() > 1024 {
        return Err(bad());
    }
    let mut parts = path.strip_suffix('/').unwrap_or(path).split('/');
    let owner = parts.next().ok_or_else(bad)?;
    let repo = parts.next().ok_or_else(bad)?;
    let safe = |s: &str| {
        (1..=255).contains(&s.len())
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if parts.next().is_some() || !safe(owner) || !safe(repo) {
        return Err(bad());
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !safe(repo) {
        return Err(bad());
    }
    Ok((
        format!("https://{host}/{owner}/{repo}.git"),
        provider.into(),
        repo.into(),
    ))
}
fn supported_version(version: &str) -> bool {
    ["2.54.0", "2.56.0"].iter().any(|base| {
        version == *base
            || version
                .strip_prefix(base)
                .and_then(|s| s.strip_prefix(" (Apple Git-")?.strip_suffix(')'))
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
    })
}
fn inspect_directory(path: &Path) -> Result<SourceInspection, ApiError> {
    if !path.is_absolute() {
        return Err(invalid("请选择绝对路径的源码目录。"));
    }
    check_no_symlinks(path, false)?;
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("请选择普通源码目录。"));
    }
    let mut result = SourceInspection {
        kind: "needs-adaptation".into(),
        name: None,
        next_step: "review-and-adapt-source".into(),
        execution_state: "not-executed".into(),
        issues: Vec::new(),
    };
    let project = path.join("project.json");
    let app = path.join("local-app.json");
    if project.try_exists()? && app.try_exists()? {
        result
            .issues
            .push(invalid("目录同时包含两种项目声明，需明确接入类型。"));
        return Ok(result);
    }
    if project.try_exists()? {
        let parsed = (|| {
            let manifest: ProjectManifest = serde_json::from_slice(&read_small(&project)?)
                .map_err(|_| invalid("project.json 不是有效项目声明。"))?;
            if !valid_id(&manifest.id) {
                return Err(invalid("项目声明 ID 无效。"));
            }
            validate_manifest(&manifest, &manifest.id, path)?;
            let source = checked_relative(path, &manifest.source_dir, false)?;
            for name in ["package.json", "package-lock.json"] {
                let _: serde_json::Value = serde_json::from_slice(&read_small(&source.join(name))?)
                    .map_err(|_| invalid("项目缺少有效的 npm 构建材料。"))?;
            }
            Ok::<_, ApiError>(manifest)
        })();
        match parsed {
            Ok(manifest) => {
                result.kind = "project-v1-source".into();
                result.name = Some(manifest.name);
                result.next_step = "review-project-source-before-integration".into();
            }
            Err(problem) => result
                .issues
                .push(error("SOURCE_DECLARATION_INVALID", &problem.message)),
        }
    } else if app.try_exists()? {
        let parsed = read_small(&app).and_then(|bytes| {
            let manifest = TrialManifest::parse(&bytes)
                .map_err(|_| invalid("应用声明不是有效的受支持格式。"))?;
            manifest
                .inspect(path)
                .map_err(|_| invalid("应用声明所需文件不完整。"))?;
            Ok(manifest)
        });
        match parsed {
            Ok(manifest) if manifest.runtime == TrialRuntime::NodeService => {
                result.kind = "node-service-source".into();
                result.name = Some(manifest.app.name);
                result.next_step = "prepare-node-runtime-before-registration".into();
            }
            Ok(_) => result
                .issues
                .push(invalid("检测到应用包声明；本步不提供成品安装。")),
            Err(_) => result
                .issues
                .push(invalid("应用声明或所声明的文件不完整，需要适配后再接入。")),
        }
    } else {
        // Never inspect .env, configuration/credentials, database contents or an
        // arbitrary recursive local tree. These two public declarations suffice.
        for name in ["package.json", "package-lock.json"] {
            let file = path.join(name);
            if file.try_exists()?
                && read_small(&file)
                    .and_then(|bytes| {
                        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(ApiError::from)
                    })
                    .is_err()
            {
                result.issues.push(invalid("npm 声明文件无效或超限。"));
            }
        }
        result.issues.push(invalid(
            "尚无受支持的项目声明；源码获取不自动推断安装或启动方式。",
        ));
    }
    Ok(result)
}

struct RunControl {
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    monitor: PathBuf,
}
impl RunControl {
    fn check(&self) -> Result<(), ApiError> {
        if *self.stop.borrow() || self.stop.has_changed().is_err() {
            return Err(interrupted());
        }
        if *self.cancel.borrow() {
            return Err(cancelled());
        }
        if Instant::now() >= self.deadline {
            return Err(error(
                "SOURCE_ACQUISITION_TIMEOUT",
                "源码获取超过 10 分钟预算；已保留现场，可稍后重试。",
            ));
        }
        Ok(())
    }
}
async fn signal(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
async fn supervise<T, F: std::future::Future<Output = Result<T, ApiError>>>(
    control: &mut RunControl,
    future: F,
) -> Result<T, ApiError> {
    control.check()?;
    tokio::pin!(future);
    let mut stop = control.stop.clone();
    let mut cancel = control.cancel.clone();
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            biased;
            _=signal(&mut stop)=>return Err(interrupted()),
            _=signal(&mut cancel)=>return Err(cancelled()),
            _=tokio::time::sleep_until(control.deadline)=>return Err(error("SOURCE_ACQUISITION_TIMEOUT","源码获取超过 10 分钟预算；已保留现场，可稍后重试。")),
            result=&mut future=>return result,
            _=interval.tick()=>{control.check()?;check_disk(&control.monitor)?;}
        }
    }
}
fn check_disk(root: &Path) -> Result<(), ApiError> {
    fn scan(path: &Path, bytes: &mut u64, count: &mut usize) -> Result<(), ApiError> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => continue,
                Err(problem) => return Err(problem.into()),
            };
            *count += 1;
            if *count > 60_000 {
                return Err(limit());
            }
            if metadata.is_dir() {
                scan(&entry.path(), bytes, count)?;
            } else if metadata.is_file() && metadata.nlink() == 1 {
                *bytes = bytes.checked_add(metadata.len()).ok_or_else(limit)?;
                if *bytes > MAX_DISK {
                    return Err(limit());
                }
            } else {
                return Err(invalid("获取目录包含不支持的链接或特殊文件。"));
            }
        }
        Ok(())
    }
    check_no_symlinks(root, false)?;
    scan(root, &mut 0, &mut 0)?;
    if fs2::available_space(root)? < MIN_FREE {
        return Err(error(
            "SOURCE_SPACE_INSUFFICIENT",
            "磁盘可用空间不足，源码获取已停止。",
        ));
    }
    Ok(())
}
fn isolated_command(tools: &GitToolchain, cwd: &Path, home: &Path, template: &Path) -> Command {
    let mut command = Command::new(&tools.git.path);
    command
        .env_clear()
        .current_dir(cwd)
        .env("PATH", &tools.path)
        .env("LC_ALL", "C")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap_or(cwd))
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0")
        .env("GIT_ALLOW_PROTOCOL", "https")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("GIT_SSH", "/usr/bin/false")
        .env("GIT_SSH_COMMAND", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_TEMPLATE_DIR", template)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    if let Some(path) = &tools.exec_path {
        command.env("GIT_EXEC_PATH", path);
    }
    for setting in [
        "credential.helper=",
        "credential.interactive=false",
        "core.askPass=/usr/bin/false",
        "protocol.allow=never",
        "protocol.https.allow=always",
        "http.followRedirects=false",
        "http.emptyAuth=false",
        "http.proactiveAuth=none",
        "http.delegation=none",
        "http.extraHeader=",
        "http.proxy=",
        "http.sslVerify=true",
        "http.saveCookies=false",
        "core.fsmonitor=false",
        "core.attributesFile=/dev/null",
        "gc.auto=0",
        "maintenance.auto=false",
        "fetch.fsckObjects=true",
        "transfer.fsckObjects=true",
        "fetch.unpackLimit=1",
        "core.protectHFS=true",
        "core.protectNTFS=true",
        "submodule.recurse=false",
    ] {
        command.arg("-c").arg(setting);
    }
    command
        .arg("-c")
        .arg(format!("core.hooksPath={}", template.display()));
    command
}
struct ChildGroup {
    child: Child,
    pid: Option<u32>,
}
impl ChildGroup {
    fn spawn(command: &mut Command) -> Result<Self, ApiError> {
        let child = command.spawn().map_err(|_| process_failed())?;
        let pid = child.id();
        Ok(Self { child, pid })
    }
    fn kill_group(&self) {
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
    async fn finish(&mut self) {
        self.kill_group();
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await;
        self.pid = None;
    }
}
impl Drop for ChildGroup {
    fn drop(&mut self) {
        self.kill_group();
    }
}
async fn bounded(
    mut reader: impl AsyncRead + Unpin,
    maximum: usize,
    retain: bool,
) -> Result<Vec<u8>, ApiError> {
    let mut result = Vec::new();
    let mut count = 0usize;
    // Keep I/O buffers out of nested async state machines on worker stacks.
    let mut buffer = vec![0u8; 8192];
    loop {
        let n = reader
            .read(&mut buffer)
            .await
            .map_err(|_| process_failed())?;
        if n == 0 {
            break;
        }
        count = count.checked_add(n).ok_or_else(limit)?;
        if count > maximum {
            return Err(limit());
        }
        if retain {
            result.extend_from_slice(&buffer[..n]);
        }
    }
    Ok(result)
}
async fn execute(
    mut command: Command,
    control: &mut RunControl,
    maximum: usize,
) -> Result<Vec<u8>, ApiError> {
    control.check()?;
    let mut group = ChildGroup::spawn(&mut command)?;
    let stdout = group.child.stdout.take().unwrap();
    let stderr = group.child.stderr.take().unwrap();
    let result = {
        let operation = async {
            let (out, _, status) = tokio::try_join!(
                bounded(stdout, maximum, true),
                bounded(stderr, MAX_OUTPUT, false),
                async { group.child.wait().await.map_err(|_| process_failed()) }
            )?;
            if !status.success() {
                return Err(process_failed());
            }
            Ok(out)
        };
        supervise(control, operation).await
    };
    group.finish().await;
    result
}
fn parse_commit(bytes: &[u8]) -> Result<String, ApiError> {
    let s = std::str::from_utf8(bytes).map_err(|_| invalid("Git 未返回有效提交。"))?;
    let oid = s
        .strip_suffix('\n')
        .ok_or_else(|| invalid("Git 未返回有效提交。"))?;
    if !valid_oid(oid) {
        return Err(invalid("Git 未返回有效提交。"));
    }
    Ok(oid.into())
}
fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && !oid.bytes().all(|b| b == b'0')
}
#[derive(Debug)]
struct TreeEntry {
    path: String,
    oid: String,
    size: u64,
    executable: bool,
}
fn parse_tree(bytes: &[u8]) -> Result<Vec<TreeEntry>, ApiError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(invalid("Git 文件树不完整。"));
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut ancestors = BTreeMap::<String, String>::new();
    let mut total = 0u64;
    for record in bytes[..bytes.len() - 1].split(|b| *b == 0) {
        let record = std::str::from_utf8(record).map_err(|_| invalid("仅支持 UTF-8 文件路径。"))?;
        let (header, path) = record
            .split_once('\t')
            .ok_or_else(|| invalid("Git 文件树无效。"))?;
        let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 4
            || !matches!(fields[0], "100644" | "100755")
            || fields[1] != "blob"
            || !valid_oid(fields[2])
        {
            return Err(invalid("本步不获取符号链接、子模块或非普通文件。"));
        }
        let size = fields[3]
            .parse::<u64>()
            .map_err(|_| invalid("Git 文件大小无效。"))?;
        if size > MAX_BLOB {
            return Err(limit());
        }
        total = total.checked_add(size).ok_or_else(limit)?;
        if total > MAX_TREE || result.len() >= MAX_FILES {
            return Err(limit());
        }
        validate_tree_path(path)?;
        let mut prefix = String::new();
        for component in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let normalized = prefix
                .nfd()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if let Some(previous) = ancestors.insert(normalized.clone(), prefix.clone()) {
                if previous != prefix {
                    return Err(invalid("仓库路径在本机存在大小写或 Unicode 冲突。"));
                }
            }
            if prefix != path && seen.contains(&normalized) {
                return Err(invalid("仓库文件与目录路径冲突。"));
            }
        }
        let normalized = path.nfd().flat_map(char::to_lowercase).collect::<String>();
        if !seen.insert(normalized.clone())
            || ancestors
                .range(format!("{normalized}/")..)
                .next()
                .is_some_and(|(p, _)| p.starts_with(&format!("{normalized}/")))
        {
            return Err(invalid("仓库文件路径重复或冲突。"));
        }
        result.push(TreeEntry {
            path: path.into(),
            oid: fields[2].into(),
            size,
            executable: fields[0] == "100755",
        });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}
fn validate_tree_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.len() > 1024
        || path.split('/').count() > 32
        || path.chars().any(|c| c.is_control() || c == '\\')
        || Path::new(path)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(invalid("仓库包含不安全或超限路径。"));
    }
    for part in path.split('/') {
        let folded = part
            .nfd()
            .filter(|c| !matches!(c, '\u{200c}' | '\u{200d}' | '\u{feff}'))
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || folded == ".git"
            || folded.starts_with("git~")
            || folded.contains(':')
        {
            return Err(invalid("仓库包含保留或不安全路径。"));
        }
    }
    Ok(())
}
async fn materialize(
    tools: &GitToolchain,
    repository: &Path,
    home: &Path,
    template: &Path,
    entries: &[TreeEntry],
    control: &mut RunControl,
) -> Result<String, ApiError> {
    let mut command = isolated_command(tools, repository, home, template);
    command.args(["cat-file", "--batch"]).stdin(Stdio::piped());
    let mut group = ChildGroup::spawn(&mut command)?;
    let mut input = group.child.stdin.take().unwrap();
    let mut output = BufReader::new(group.child.stdout.take().unwrap());
    let stderr = group.child.stderr.take().unwrap();
    let result = {
        let operation = async {
            let blobs = async {
                let mut tree = Sha256::new();
                let mut buffer = vec![0u8; 65536];
                for entry in entries {
                    input
                        .write_all(format!("{}\n", entry.oid).as_bytes())
                        .await
                        .map_err(|_| process_failed())?;
                    input.flush().await.map_err(|_| process_failed())?;
                    let mut header = Vec::new();
                    loop {
                        if header.len() >= 160 {
                            return Err(invalid("Git blob 响应头超限。"));
                        }
                        let byte = output.read_u8().await.map_err(|_| process_failed())?;
                        if byte == b'\n' {
                            break;
                        }
                        header.push(byte);
                    }
                    if header != format!("{} blob {}", entry.oid, entry.size).as_bytes() {
                        return Err(invalid("Git blob 与已检查文件树不一致。"));
                    }
                    let path = repository.join(&entry.path);
                    private_dir(path.parent().unwrap())?;
                    let file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(if entry.executable { 0o755 } else { 0o644 })
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(&path)?;
                    let mut file = tokio::fs::File::from_std(file);
                    let mut remaining = entry.size;
                    let mut digest = Sha256::new();
                    while remaining > 0 {
                        let n = remaining.min(buffer.len() as u64) as usize;
                        output
                            .read_exact(&mut buffer[..n])
                            .await
                            .map_err(|_| process_failed())?;
                        file.write_all(&buffer[..n]).await?;
                        digest.update(&buffer[..n]);
                        remaining -= n as u64;
                    }
                    file.sync_all().await?;
                    if output.read_u8().await.map_err(|_| process_failed())? != b'\n' {
                        return Err(invalid("Git blob 响应分隔无效。"));
                    }
                    tree.update(entry.path.as_bytes());
                    tree.update([0]);
                    tree.update(entry.size.to_le_bytes());
                    tree.update([u8::from(entry.executable)]);
                    tree.update(digest.finalize());
                }
                input.shutdown().await.map_err(|_| process_failed())?;
                drop(input);
                let trailing = bounded(output, 1, true).await?;
                if !trailing.is_empty() {
                    return Err(invalid("Git 返回了未请求的文件内容。"));
                }
                Ok::<_, ApiError>(format!("{:x}", tree.finalize()))
            };
            let (digest, _, status) =
                tokio::try_join!(blobs, bounded(stderr, MAX_OUTPUT, false), async {
                    group.child.wait().await.map_err(|_| process_failed())
                })?;
            if !status.success() {
                return Err(process_failed());
            }
            Ok(digest)
        };
        supervise(control, operation).await
    };
    group.finish().await;
    result
}

#[cfg(test)]
#[path = "source_acquisition_tests.rs"]
mod tests;
