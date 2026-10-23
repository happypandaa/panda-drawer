//! Independent Windows source candidates. Acquisition never registers a project,
//! installs dependencies, checks out Git objects or executes application code.
use crate::{
    development_tasks::{DevelopmentTasks, TaskContext},
    development_tools::{DevelopmentTools, ToolLease},
    ApiError,
};
use platform_host::{
    windows_command::{self, CommandSpec, CommandTermination},
    WindowsObjectIdentity, WindowsSourceDirectory, WorkspaceDirectory,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::JoinHandle};
use uuid::Uuid;

#[path = "windows_sources/policy.rs"]
mod policy;
#[path = "windows_sources/snapshot.rs"]
mod snapshot;
use snapshot::{Destination, Snapshot, SourceFile};

type Result<T> = std::result::Result<T, ApiError>;
const MAX_FILES: usize = 10_000;
const MAX_ENTRIES: usize = 40_000;
const MAX_BLOB: u64 = 8_000_000;
const MAX_TREE: u64 = 256 * 1024 * 1024;
const MAX_DISK: u64 = 512 * 1024 * 1024;
const MAX_METADATA: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(600);
const CAPTURE_POLICY: &str = "local-source-v1";
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn invalid(message: &str) -> ApiError {
    error("SOURCE_ACQUISITION_INVALID", message)
}
fn fs_error(error: platform_host::WorkspaceError) -> ApiError {
    ApiError::new(error.code, error.message)
}
fn limit() -> ApiError {
    error("SOURCE_ACQUISITION_LIMIT","源码超出Windows受管范围：每文件8,000,000字节、合计256MiB、10,000文件及512MiB操作证据；现场已保留。")
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}
fn prefixed_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|value| {
        value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureInput {
    pub source_path: PathBuf,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireInput {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
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
    #[serde(default)]
    local_source_path: Option<PathBuf>,
    #[serde(default)]
    capture_policy: Option<String>,
    #[serde(default)]
    excluded_paths: Vec<String>,
    created_at: u64,
    completed_at: Option<u64>,
    repository: String,
    shallow: bool,
    git: Option<Value>,
    commit: Option<String>,
    file_count: Option<usize>,
    total_bytes: Option<u64>,
    content_digest: Option<String>,
    inspection: Option<Value>,
    error: Option<ApiError>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlotProof {
    schema_version: u32,
    source_id: String,
    task_id: String,
    directory_identity: WindowsObjectIdentity,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadyProof {
    schema_version: u32,
    source_id: String,
    payload_identity: WindowsObjectIdentity,
    record_identity: WindowsObjectIdentity,
    files: Vec<SourceFile>,
}

/// Keep this entire lease alive while copying/building; root alone does not
/// retain the independently pinned leaf HANDLEs for its complete contents.
pub struct SourceLease {
    pub source_id: String,
    pub root: WindowsSourceDirectory,
    pub content_digest: String,
    pub inspection: Value,
    pub name: String,
    guards: Vec<WindowsSourceDirectory>,
    snapshot: Snapshot,
}
impl SourceLease {
    pub fn verify(&self) -> Result<()> {
        let (_send, cancel) = watch::channel(false);
        let control = Control {
            cancel,
            deadline: Instant::now() + TIMEOUT,
        };
        self.verify_control(&control)
    }
    fn verify_control(&self, control: &Control) -> Result<()> {
        self.snapshot.verify(control, false)?;
        for guard in &self.guards {
            guard.verify_binding().map_err(fs_error)?;
        }
        self.root.verify_binding().map_err(fs_error)
    }
    pub fn copy_to(&self, destination: &WorkspaceDirectory) -> Result<()> {
        self.copy_projection(None, destination, None)
    }
    pub fn copy_subdirectory_to(
        &self,
        relative: &str,
        destination: &WorkspaceDirectory,
    ) -> Result<()> {
        self.copy_projection(Some(relative), destination, None)
    }
    pub fn copy_subdirectory_to_with_cancellation(
        &self,
        relative: &str,
        destination: &WorkspaceDirectory,
        cancellation: watch::Receiver<bool>,
    ) -> Result<()> {
        self.copy_projection(Some(relative), destination, Some(cancellation))
    }
    fn copy_projection(
        &self,
        relative: Option<&str>,
        destination: &WorkspaceDirectory,
        cancellation: Option<watch::Receiver<bool>>,
    ) -> Result<()> {
        let (_keepalive, idle) = watch::channel(false);
        let control = Control {
            cancel: cancellation.unwrap_or(idle),
            deadline: Instant::now() + TIMEOUT,
        };
        control.check()?;
        let _relative = relative
            .map(|path| relative_directory(&self.root, path))
            .transpose()?;
        let target = WindowsSourceDirectory::from_workspace(destination).map_err(fs_error)?;
        if self.root.overlaps(&target) || !destination.list().map_err(fs_error)?.is_empty() {
            return Err(invalid(
                "源码复制目标必须为独立的新建空目录；未覆盖已有文件。",
            ));
        }
        self.verify_control(&control)?;
        let mut output = Destination::new(destination.clone());
        let prefix = relative.map(|value| format!("{value}/"));
        let mut expected = BTreeMap::new();
        let mut copied = 0;
        for (path, file) in &self.snapshot.files {
            control.check()?;
            let path = if let Some(prefix) = &prefix {
                let Some(path) = path.strip_prefix(prefix) else {
                    continue;
                };
                path
            } else {
                path
            };
            output.write(
                path,
                &self.snapshot.read(&file.record.path)?,
                file.record.executable,
            )?;
            let mut projected = file.record.clone();
            projected.path = path.into();
            expected.insert(path, projected);
            copied += 1;
        }
        if copied == 0 {
            return Err(invalid("源码复制范围没有受支持的普通文件。"));
        }
        if output.digest() != snapshot::digest_files(expected.values()) {
            return Err(invalid("源码投影的内容或逻辑文件模式摘要不一致。"));
        }
        self.verify_control(&control)?;
        destination.verify_binding().map_err(fs_error)
    }
}

#[derive(Clone)]
struct Control {
    cancel: watch::Receiver<bool>,
    deadline: Instant,
}
impl Control {
    fn check(&self) -> Result<()> {
        if *self.cancel.borrow() || self.cancel.has_changed().is_err() {
            return Err(error(
                "SOURCE_ACQUISITION_CANCELLED",
                "源码任务已取消；部分文件保留，原目录未修改。",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(error(
                "SOURCE_ACQUISITION_TIMEOUT",
                "源码任务已超时；部分文件保留。",
            ));
        }
        Ok(())
    }
}

pub struct SourceManager {
    data: WorkspaceDirectory,
    tools: DevelopmentTools,
    tasks: Arc<DevelopmentTasks>,
    active: Mutex<BTreeSet<String>>,
    runners: Mutex<Vec<JoinHandle<()>>>,
    submission: Mutex<()>,
    stopping: AtomicBool,
}
impl SourceManager {
    pub fn new(
        data: WorkspaceDirectory,
        tools: DevelopmentTools,
        tasks: Arc<DevelopmentTasks>,
    ) -> Result<Arc<Self>> {
        platform_host::windows_development::area(&data, "sources", false).map_err(fs_error)?;
        Ok(Arc::new(Self {
            data,
            tools,
            tasks,
            active: Mutex::new(BTreeSet::new()),
            runners: Mutex::new(Vec::new()),
            submission: Mutex::new(()),
            stopping: AtomicBool::new(false),
        }))
    }
    fn slot(&self, id: &str) -> Result<WorkspaceDirectory> {
        if !prefixed_id(id, "src_") {
            return Err(error("SOURCE_NOT_FOUND", "未找到该源码候选。"));
        }
        let root = platform_host::windows_development::area(&self.data, "sources", false)
            .map_err(fs_error)?
            .ok_or_else(|| error("SOURCE_NOT_FOUND", "未找到该源码候选。"))?;
        let slot = root.open_directory(id).map_err(fs_error)?;
        let proof: SlotProof = serde_json::from_slice(
            &slot
                .read_bounded(".windows-source-slot.json", 65536)
                .map_err(fs_error)?
                .bytes,
        )?;
        if proof.schema_version != 1
            || proof.source_id != id
            || !prefixed_id(&proof.task_id, "task_")
            || proof.directory_identity != *slot.identity()
        {
            return Err(invalid("源码槽身份或记录无效；未接管替换目录。"));
        }
        for entry in slot.list().map_err(fs_error)? {
            if !matches!(
                (entry.name.as_str(), entry.is_directory),
                (".windows-source-slot.json", false)
                    | ("progress-00000001.json", false)
                    | ("source.json", false)
                    | (".windows-source-ready.json", false)
                    | ("repository", true)
            ) {
                return Err(invalid("源码槽含未知记录，现场已保留。"));
            }
        }
        Ok(slot)
    }
    fn record(&self, id: &str) -> Result<(WorkspaceDirectory, SourceRecord, bool)> {
        let slot = self.slot(id)?;
        let final_record = slot
            .list()
            .map_err(fs_error)?
            .iter()
            .any(|entry| entry.name == "source.json");
        let name = if final_record {
            "source.json"
        } else {
            "progress-00000001.json"
        };
        let record: SourceRecord = serde_json::from_slice(
            &slot
                .read_bounded(name, MAX_METADATA)
                .map_err(fs_error)?
                .bytes,
        )?;
        let proof: SlotProof = serde_json::from_slice(
            &slot
                .read_bounded(".windows-source-slot.json", 65536)
                .map_err(fs_error)?
                .bytes,
        )?;
        if record.schema_version != 1
            || record.source_id != id
            || !prefixed_id(&record.task_id, "task_")
            || record.task_id != proof.task_id
            || record.repository != "repository"
            || !matches!(record.provider.as_str(), "local" | "github" | "gitee")
            || !matches!(
                record.status.as_str(),
                "acquiring" | "ready" | "failed" | "cancelled" | "interrupted"
            )
        {
            return Err(invalid("源码候选记录格式或身份不符。"));
        }
        Ok((slot, record, final_record))
    }
    fn view(&self, id: &str) -> Result<Value> {
        let (slot, mut record, final_record) = self.record(id)?;
        if !final_record
            && (!self.active.lock().unwrap().contains(&record.task_id)
                || self
                    .tasks
                    .get(&record.task_id)
                    .is_ok_and(|task| task["final"] == true))
        {
            record.status = "interrupted".into();
            record.phase = "interrupted".into();
            record.error = Some(error(
                "SOURCE_ACQUISITION_INTERRUPTED",
                "任务未完成，保留全部记录和文件；没有自动重试。",
            ));
        }
        let source_path = if record.status == "ready" {
            let proof: ReadyProof = serde_json::from_slice(
                &slot
                    .read_bounded(".windows-source-ready.json", 8_000_000)
                    .map_err(fs_error)?
                    .bytes,
            )?;
            let payload = slot.open_directory("repository").map_err(fs_error)?;
            let identity = slot
                .read_bounded("source.json", MAX_METADATA)
                .map_err(fs_error)?
                .identity;
            if proof.schema_version != 1
                || proof.source_id != id
                || proof.payload_identity != *payload.identity()
                || proof.record_identity != identity
            {
                return Err(invalid("源码完成记录或目录身份无效。"));
            }
            Some(payload.path().to_owned())
        } else {
            None
        };
        let mut value = serde_json::to_value(&record)?;
        value.as_object_mut().unwrap().remove("schemaVersion");
        value.as_object_mut().unwrap().remove("repository");
        value["sourcePath"] = json!(source_path);
        value["evidencePath"] = json!(slot.path());
        value["executionState"] = json!("not-executed");
        value["registrationAvailable"] = json!(false);
        value["captureNotice"] = if record.provider == "local" {
            json!("按已知路径排除了本机数据、依赖、凭据和临时文件；未知文件中的秘密无法通用识别，不能视为已脱敏。")
        } else {
            Value::Null
        };
        if record.status == "acquiring" {
            if let Ok(task) = self.tasks.get(&record.task_id) {
                value["phase"] = task["phase"].clone();
            }
        }
        Ok(value)
    }
    pub fn list(&self) -> Result<Value> {
        let Some(root) = platform_host::windows_development::area(&self.data, "sources", false)
            .map_err(fs_error)?
        else {
            return Ok(json!([]));
        };
        let mut result = Vec::new();
        for entry in root.list().map_err(fs_error)? {
            if entry.name == ".windows-development.json" {
                continue;
            }
            result.push(self.view(&entry.name).unwrap_or_else(|issue|json!({"sourceId":entry.name,"name":entry.name,"status":"unavailable","phase":"unavailable","error":issue,"sourcePath":null,"evidencePath":root.path().join(&entry.name),"executionState":"not-executed","registrationAvailable":false})));
        }
        Ok(json!(result))
    }
    pub fn candidate(&self, id: &str) -> Result<SourceLease> {
        let (slot, record, final_record) = self.record(id)?;
        if !final_record
            || record.status != "ready"
            || record
                .content_digest
                .as_deref()
                .is_none_or(|v| !valid_digest(v))
        {
            return Err(invalid("源码候选尚未完成；未允许构建。"));
        }
        let source = WindowsSourceDirectory::from_workspace(&slot).map_err(fs_error)?;
        let source = source
            .pin_file("source.json")
            .map_err(fs_error)?
            .pin_file(".windows-source-ready.json")
            .map_err(fs_error)?;
        let record_file = source
            .read_bounded("source.json", MAX_METADATA)
            .map_err(fs_error)?;
        let current: SourceRecord = serde_json::from_slice(&record_file.bytes)?;
        if serde_json::to_value(&current)? != serde_json::to_value(&record)? {
            return Err(invalid("源码记录在校验期间变化。"));
        }
        let proof: ReadyProof = serde_json::from_slice(
            &source
                .read_bounded(".windows-source-ready.json", 8_000_000)
                .map_err(fs_error)?
                .bytes,
        )?;
        let payload = source.open_directory("repository").map_err(fs_error)?;
        if proof.schema_version != 1
            || proof.source_id != id
            || proof.payload_identity != *payload.identity()
            || proof.record_identity != record_file.identity
            || proof.files.len() > MAX_FILES
            || proof.files.iter().any(|file| !valid_digest(&file.sha256))
        {
            return Err(invalid("源码完成记录或目录身份无效。"));
        }
        let (_send, cancel) = watch::channel(false);
        let control = Control {
            cancel,
            deadline: Instant::now() + TIMEOUT,
        };
        let mut actual = snapshot::scan(payload.clone(), &control, false)?;
        let mut expected = BTreeMap::new();
        for file in proof.files {
            policy::validate_tree_path(&file.path)?;
            if expected.insert(file.path.clone(), file).is_some() {
                return Err(invalid("源码完成记录含重复文件。"));
            }
        }
        if expected.len() != actual.files.len() {
            return Err(invalid("源码候选的完整文件树已变化。"));
        }
        for (path, file) in &mut actual.files {
            let expected = expected
                .get(path)
                .ok_or_else(|| invalid("源码候选新增了未确认文件。"))?;
            if file.record.size != expected.size
                || file.record.sha256 != expected.sha256
                || file.record.identity != expected.identity
            {
                return Err(invalid("源码候选文件身份或内容已变化。"));
            }
            // NTFS has no POSIX executable bit. Preserve the verified Git mode
            // as logical provenance used by content digests and later copies.
            file.record.executable = expected.executable;
        }
        let digest = snapshot::digest_files(expected.values());
        if Some(&digest) != record.content_digest.as_ref() {
            return Err(invalid("源码候选摘要不符；未允许构建。"));
        }
        let lease = SourceLease {
            source_id: id.into(),
            root: payload,
            content_digest: digest,
            inspection: record.inspection.unwrap_or(Value::Null),
            name: record.name,
            guards: vec![source],
            snapshot: actual,
        };
        Ok(lease)
    }
    fn local_root(&self, path: &Path) -> Result<WindowsSourceDirectory> {
        let root = WindowsSourceDirectory::open(path).map_err(fs_error)?;
        let data = WindowsSourceDirectory::from_workspace(&self.data).map_err(fs_error)?;
        if root.overlaps(&data) {
            return Err(invalid(
                "请选择独立源码目录，不能使用平台数据/控制目录或其父级。",
            ));
        }
        if let Some(home) = std::env::var_os("USERPROFILE")
            .and_then(|home| WindowsSourceDirectory::open(Path::new(&home)).ok())
        {
            if home.path().starts_with(root.path()) {
                return Err(invalid(
                    "不能接入整个用户目录或其父级；请选择具体源码文件夹。",
                ));
            }
        }
        Ok(root)
    }
    pub fn inspect_local(&self, path: &Path) -> Result<Value> {
        let root = self.local_root(path)?;
        Ok(json!({"sourcePath":root.path(),"inspection":policy::inspect_directory(&root)?}))
    }
    pub fn capture(self: &Arc<Self>, input: CaptureInput) -> Result<Value> {
        let root = self.local_root(&input.source_path)?;
        let name = input.name.unwrap_or_else(|| {
            root.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        self.submit(name, String::new(), "local".into(), Some(root))
    }
    pub fn acquire(self: &Arc<Self>, input: AcquireInput) -> Result<Value> {
        let (url, provider, default_name) = policy::validate_url(&input.url)?;
        let name = input.name.unwrap_or(default_name);
        self.submit(name, url, provider, None)
    }
    fn submit(
        self: &Arc<Self>,
        name: String,
        url: String,
        provider: String,
        local: Option<WindowsSourceDirectory>,
    ) -> Result<Value> {
        if name.trim().is_empty()
            || name.chars().count() > 200
            || name.chars().any(char::is_control)
        {
            return Err(invalid("源码名称须为 1 到 200 个有效字符。"));
        }
        let _submission = self.submission.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(error(
                "SERVICE_STOPPING",
                "平台正在退出，未接受新源码任务。",
            ));
        }
        if !self.active.lock().unwrap().is_empty() {
            return Err(error(
                "SOURCE_ACQUISITION_BUSY",
                "已有源码正在获取，请等待完成或取消。",
            ));
        }
        let id = format!("src_{}", Uuid::new_v4().simple());
        let ctx = self.tasks.start(
            if local.is_some() {
                "sourceCapture"
            } else {
                "sourceAcquire"
            },
            Some(&id),
        )?;
        let setup = (|| -> Result<_> {
            let root = platform_host::windows_development::area(&self.data, "sources", true)
                .map_err(fs_error)?
                .unwrap();
            if root.list().map_err(fs_error)?.len() >= 240 {
                return Err(limit());
            }
            let slot = root.create_directory(&id).map_err(fs_error)?;
            slot.write_new(
                ".windows-source-slot.json",
                &serde_json::to_vec(&SlotProof {
                    schema_version: 1,
                    source_id: id.clone(),
                    task_id: ctx.id().into(),
                    directory_identity: slot.identity().clone(),
                })?,
            )
            .map_err(fs_error)?;
            let record = SourceRecord {
                schema_version: 1,
                source_id: id.clone(),
                task_id: ctx.id().into(),
                name,
                status: "acquiring".into(),
                phase: "queued".into(),
                url,
                provider: provider.clone(),
                local_source_path: local.as_ref().map(|r| r.path().to_owned()),
                capture_policy: local.as_ref().map(|_| CAPTURE_POLICY.into()),
                excluded_paths: Vec::new(),
                created_at: now(),
                completed_at: None,
                repository: "repository".into(),
                shallow: local.is_none(),
                git: None,
                commit: None,
                file_count: None,
                total_bytes: None,
                content_digest: None,
                inspection: None,
                error: None,
            };
            slot.write_new("progress-00000001.json", &serde_json::to_vec(&record)?)
                .map_err(fs_error)?;
            Ok((slot, record))
        })();
        let (slot, record) = match setup {
            Ok(value) => value,
            Err(issue) => {
                let _ = ctx.finish(Err(issue.clone()));
                return Err(issue.detail("sourceId", id).outcome("unknown"));
            }
        };
        self.active.lock().unwrap().insert(ctx.id().into());
        let task_id = ctx.id().to_owned();
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            manager.work(ctx, slot, record, local).await;
        });
        self.runners.lock().unwrap().push(runner);
        self.tasks.get(&task_id)
    }
    async fn work(
        &self,
        ctx: TaskContext,
        slot: WorkspaceDirectory,
        mut record: SourceRecord,
        local: Option<WindowsSourceDirectory>,
    ) {
        let control = Control {
            cancel: ctx.cancellation(),
            deadline: Instant::now() + TIMEOUT,
        };
        let result = async {
            let destination = slot.create_directory("repository").map_err(fs_error)?;
            let mut output = Destination::new(destination);
            let original = if let Some(local) = local {
                record.phase = "inspecting-local-scope".into();
                ctx.phase(&record.phase)?;
                let copy_control = control.clone();
                let plan =
                    tokio::task::spawn_blocking(move || snapshot::scan(local, &copy_control, true))
                        .await
                        .map_err(|_| invalid("本机源码检查未完成。"))??;
                record.excluded_paths = plan.excluded.clone();
                record.phase = "capturing-local".into();
                ctx.phase(&record.phase)?;
                let copy_control = control.clone();
                let (copied, issues, plan) = tokio::task::spawn_blocking(move || -> Result<_> {
                    let issues = policy::local_dependencies(&plan, &copy_control)?;
                    for (index, (path, file)) in plan.files.iter().enumerate() {
                        copy_control.check()?;
                        output.write(path, &plan.read(path)?, file.record.executable)?;
                        if index % 64 == 0 {
                            check_disk(&output.root)?;
                        }
                    }
                    plan.verify(&copy_control, true)?;
                    if output.digest() != plan.digest() {
                        return Err(invalid("本机源码副本摘要不符；未提交。"));
                    }
                    Ok((output, issues, plan))
                })
                .await
                .map_err(|_| invalid("本机源码复制未完成。"))??;
                output = copied;
                let mut inspection = policy::inspect_directory(
                    &WindowsSourceDirectory::from_workspace(&output.root).map_err(fs_error)?,
                )?;
                if !issues.is_empty() {
                    inspection["kind"] = json!("needs-adaptation");
                    inspection["nextStep"] = json!("review-and-adapt-source");
                    inspection["issues"]
                        .as_array_mut()
                        .unwrap()
                        .extend(issues.into_iter().map(|issue| json!(issue)));
                }
                record.inspection = Some(inspection);
                Some(plan)
            } else {
                self.fetch(&ctx, &control, &mut record, &mut output).await?;
                None
            };
            record.phase = "verifying-candidate".into();
            ctx.phase(&record.phase)?;
            control.check()?;
            check_disk(&slot)?;
            let payload = WindowsSourceDirectory::from_workspace(&output.root).map_err(fs_error)?;
            let scan_root = payload.clone();
            let scan_control = control.clone();
            let (actual, _original) = tokio::task::spawn_blocking(move || -> Result<_> {
                let actual = snapshot::scan(scan_root, &scan_control, false)?;
                if let Some(plan) = &original {
                    plan.verify(&scan_control, true)?;
                }
                Ok((actual, original))
            })
            .await
            .map_err(|_| invalid("候选完整核对未完成。"))??;
            if actual.files.len() != output.files.len()
                || actual.files.iter().any(|(path, file)| {
                    output.files.get(path).is_none_or(|expected| {
                        expected.sha256 != file.record.sha256
                            || expected.identity != file.record.identity
                            || expected.size != file.record.size
                    })
                })
            {
                return Err(invalid("候选物化内容核对失败。"));
            }
            record.file_count = Some(output.files.len());
            record.total_bytes = Some(output.files.values().map(|file| file.size).sum());
            record.content_digest = Some(output.digest());
            record.status = "ready".into();
            record.phase = "completed".into();
            record.completed_at = Some(now());
            ctx.check()?;
            control.check()?;
            // Both the complete copied tree and original local inputs remain
            // pinned through the two create-only completion records.
            let record_identity = slot
                .write_new("source.json", &serde_json::to_vec(&record)?)
                .map_err(|issue| fs_error(issue).outcome("unknown"))?;
            slot.write_new(
                ".windows-source-ready.json",
                &serde_json::to_vec(&ReadyProof {
                    schema_version: 1,
                    source_id: record.source_id.clone(),
                    payload_identity: payload.identity().clone(),
                    record_identity,
                    files: output.files.into_values().collect(),
                })?,
            )
            .map_err(|issue| fs_error(issue).outcome("unknown"))?;
            Ok::<_, ApiError>(json!({
                "sourceId":record.source_id,
                "operationOutcome":"committed",
                "executionState":"not-executed",
                "registrationAvailable":false
            }))
        }
        .await;
        let result = match result {
            Ok(value) => Ok(value),
            Err(issue) => {
                record.status = if self.stopping.load(Ordering::Acquire) {
                    "interrupted"
                } else if matches!(
                    issue.code.as_str(),
                    "TASK_CANCELLED" | "SOURCE_ACQUISITION_CANCELLED"
                ) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                record.completed_at = Some(now());
                record.error = Some(issue.clone());
                let saved = serde_json::to_vec(&record)
                    .map_err(ApiError::from)
                    .and_then(|bytes| {
                        slot.write_new("source.json", &bytes)
                            .map(|_| ())
                            .map_err(fs_error)
                    });
                let mut issue = issue
                    .detail("sourceId", record.source_id.clone())
                    .detail("partialFilesRetained", true);
                if saved.is_err()
                    || issue
                        .details
                        .get("operationOutcome")
                        .and_then(Value::as_str)
                        == Some("unknown")
                {
                    issue = issue.outcome("unknown");
                } else {
                    issue = issue.outcome("not_registered");
                }
                Err(issue)
            }
        };
        let _ = ctx.finish(result);
        self.active.lock().unwrap().remove(ctx.id());
    }
    pub async fn shutdown(&self) {
        {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            for id in self.active.lock().unwrap().iter() {
                let _ = self.tasks.cancel(id);
            }
        }
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        for runner in runners {
            let _ = runner.await;
        }
    }
}

fn relative_directory(root: &WindowsSourceDirectory, path: &str) -> Result<WindowsSourceDirectory> {
    policy::validate_tree_path(path)?;
    let mut directory = root.clone();
    for part in path.split('/') {
        let entry = directory
            .source_entries(MAX_ENTRIES)
            .map_err(fs_error)?
            .into_iter()
            .find(|entry| entry.name == part && entry.is_directory && !entry.reparse)
            .ok_or_else(|| {
                error(
                    "SOURCE_MANIFEST_PATH_MISMATCH",
                    "源码声明路径必须与实际普通目录逐段完全一致（包括大小写）；未复制其他目录。",
                )
                .detail("relativePath", path)
            })?;
        directory = directory.pin_source_entry(&entry).map_err(fs_error)?.0;
    }
    Ok(directory)
}
fn optional_bytes(root: &WindowsSourceDirectory, name: &str) -> Result<Option<Vec<u8>>> {
    let Some(entry) = root
        .source_entries(MAX_ENTRIES)
        .map_err(fs_error)?
        .into_iter()
        .find(|entry| entry.name == name)
    else {
        return Ok(None);
    };
    if entry.is_directory || entry.reparse || entry.length > MAX_METADATA as u64 {
        return Err(invalid("公开声明不是普通有界文件。"));
    }
    let (pin, _) = root.pin_source_entry(&entry).map_err(fs_error)?;
    let file = pin.read_bounded(name, MAX_METADATA).map_err(fs_error)?;
    if file.identity != entry.identity {
        return Err(snapshot::changed());
    }
    Ok(Some(file.bytes))
}
fn optional_json(root: &WindowsSourceDirectory, name: &str) -> Result<Option<Value>> {
    optional_bytes(root, name)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(ApiError::from))
        .transpose()
}

fn check_disk(root: &WorkspaceDirectory) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    root.verify_binding().map_err(fs_error)?;
    // Handle-rooted workspaces may exceed MAX_PATH. This read-only Win32 query
    // must use the extended syntax too; the directory binding is checked before
    // and after querying its volume's available space.
    let mut extended = OsString::from(r"\\?\");
    extended.push(root.path());
    let mut path: Vec<_> = extended.encode_wide().collect();
    path.push(b'\\' as u16);
    path.push(0);
    let mut free = 0u64;
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        return Err(error(
            "SOURCE_DISK_SPACE_LOW",
            "磁盘可用空间无法确认；源码任务已停止。",
        )
        .detail(
            "nativeError",
            std::io::Error::last_os_error().raw_os_error(),
        ));
    }
    if free < 64 * 1024 * 1024 {
        return Err(error(
            "SOURCE_DISK_SPACE_LOW",
            "磁盘可用空间少于64MiB；源码任务已停止。",
        )
        .detail("availableBytes", free));
    }
    root.verify_binding().map_err(fs_error)
}

struct GitSession {
    tool: ToolLease,
    repository: WorkspaceDirectory,
    home: WorkspaceDirectory,
    temporary: WorkspaceDirectory,
    template: WorkspaceDirectory,
    _configuration: WindowsSourceDirectory,
}
impl GitSession {
    fn new(manager: &SourceManager, ctx: &TaskContext) -> Result<Self> {
        use std::os::windows::ffi::OsStrExt;
        let tool = manager.tools.lease(&manager.data, "git")?;
        let version = tool
            .version
            .strip_prefix("git version ")
            .unwrap_or(&tool.version);
        if !["2.54.0", "2.56.0"].iter().any(|base| {
            version == *base
                || version
                    .strip_prefix(base)
                    .and_then(|rest| rest.strip_prefix(".windows."))
                    .is_some_and(|rest| {
                        !rest.is_empty()
                            && rest
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || byte == b'.')
                    })
        }) {
            return Err(error(
                "SOURCE_GIT_VERSION_UNSUPPORTED",
                "源码获取仅支持已确认的Git 2.54.0或2.56.0 Windows发行。",
            ));
        }
        let repository_path = ctx.directory().path().join("bare-repository.git");
        if repository_path.as_os_str().encode_wide().count() >= 248
            || repository_path.to_string_lossy().len() > 220
        {
            // Git for Windows canonicalizes the initial repository path before
            // reading core.longpaths. Its early directory operations therefore
            // retain the Win32 directory limit even with an explicit -c option.
            // Explicit discovery also caps the UTF-8 $GIT_DIR at PATH_MAX-40.
            return Err(error(
                "SOURCE_GIT_PATH_UNSUPPORTED",
                "当前任务目录超过Git初始化的Windows路径上限；未下载或执行源码，请通过公开工作区迁移流程选择较短路径。",
            )
            .detail("repositoryPath", json!(repository_path)));
        }
        let home = ctx.directory().create_directory("home").map_err(fs_error)?;
        let temporary = ctx
            .directory()
            .create_directory("temporary")
            .map_err(fs_error)?;
        let template = ctx
            .directory()
            .create_directory("empty-template")
            .map_err(fs_error)?;
        let repository = ctx
            .directory()
            .create_directory("bare-repository.git")
            .map_err(fs_error)?;
        home.write_new("user.npmrc", b"").map_err(fs_error)?;
        home.write_new("global.npmrc", b"").map_err(fs_error)?;
        let configuration = WindowsSourceDirectory::from_workspace(&home)
            .map_err(fs_error)?
            .pin_file("user.npmrc")
            .map_err(fs_error)?
            .pin_file("global.npmrc")
            .map_err(fs_error)?;
        Ok(Self {
            tool,
            repository,
            home,
            temporary,
            template,
            _configuration: configuration,
        })
    }
    async fn execute(
        &self,
        ctx: &TaskContext,
        control: &Control,
        args: Vec<OsString>,
        stdin: Vec<u8>,
        stdout_limit: usize,
    ) -> Result<Vec<u8>> {
        ctx.check()?;
        control.check()?;
        self.tool.verify()?;
        disk_budget(ctx.directory())?;
        let mut environment =
            self.tool
                .environment(self.home.path(), self.temporary.path(), None)?;
        for (key, value) in [
            ("LC_ALL", "C"),
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_ALLOW_PROTOCOL", "https"),
            ("GIT_PROTOCOL_FROM_USER", "0"),
            ("GIT_ATTR_NOSYSTEM", "1"),
            ("GIT_NO_REPLACE_OBJECTS", "1"),
            ("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0"),
            ("SSH_ASKPASS_REQUIRE", "never"),
        ] {
            environment.insert(key.into(), value.into());
        }
        environment.insert(
            "GIT_CEILING_DIRECTORIES".into(),
            self.tool
                .root
                .parent()
                .unwrap_or(&self.tool.root)
                .as_os_str()
                .into(),
        );
        environment.insert(
            "GIT_TEMPLATE_DIR".into(),
            self.template.path().as_os_str().into(),
        );
        let mut command = Vec::<OsString>::new();
        for setting in [
            "credential.helper=",
            "credential.interactive=false",
            "core.askPass=",
            "protocol.allow=never",
            "protocol.https.allow=always",
            "http.followRedirects=false",
            "http.emptyAuth=false",
            "http.proactiveAuth=none",
            "http.delegation=none",
            "http.extraHeader=",
            "http.proxy=",
            "http.sslBackend=openssl",
            "http.sslVerify=true",
            "http.saveCookies=false",
            "core.fsmonitor=false",
            "core.attributesFile=NUL",
            "core.autocrlf=false",
            "core.longpaths=true",
            "gc.auto=0",
            "maintenance.auto=false",
            "fetch.fsckObjects=true",
            "transfer.fsckObjects=true",
            "fetch.unpackLimit=1",
            "core.protectHFS=true",
            "core.protectNTFS=true",
            "submodule.recurse=false",
        ] {
            command.push("-c".into());
            command.push(setting.into());
        }
        command.extend([
            "-c".into(),
            format!("core.hooksPath={}", self.template.path().display()).into(),
        ]);
        command.extend(args);
        let (stop, receiver) = watch::channel(false);
        let mut cancellation = control.cancel.clone();
        let operation = windows_command::run(
            CommandSpec {
                executable: self.tool.executable.clone(),
                args: command,
                // Every operation supplies an absolute --git-dir or explicit
                // init target. The verified tool root keeps CreateProcess's
                // working directory short even when task evidence exceeds it.
                working_directory: self.tool.root.clone(),
                environment,
                stdin,
                timeout: control
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1)),
                stdout_limit,
                stderr_limit: 65536,
            },
            receiver,
        );
        tokio::pin!(operation);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut stop_issue = None;
        let result = loop {
            tokio::select! {
                result = &mut operation => break result,
                result = cancellation.changed(), if stop_issue.is_none() => {
                    if result.is_err() || *cancellation.borrow() {
                        stop.send_replace(true);
                        stop_issue = Some(error("SOURCE_ACQUISITION_CANCELLED", "源码任务已取消；下载或复制现场已保留。"));
                    }
                },
                _ = interval.tick(), if stop_issue.is_none() => {
                    if let Err(issue) = control.check().and_then(|()| disk_budget(ctx.directory())) {
                        stop.send_replace(true);
                        stop_issue = Some(issue);
                    }
                },
            }
        };
        let output = match result {
            Ok(output) => output,
            Err(issue) => {
                // Execution may have transferred its exact Job to native
                // cleanup. Keep this complete tool tree, configuration and
                // repository scope alive until that cleanup has drained.
                windows_command::wait_for_idle().await;
                return Err(issue);
            }
        };
        if !output.stderr.bytes.is_empty() {
            ctx.log(
                "stderr",
                &output.stderr.bytes,
                output.stderr.discarded_bytes != 0,
            )?;
        }
        if let Some(issue) = stop_issue {
            return Err(issue);
        }
        self.tool.verify()?;
        control.check()?;
        disk_budget(ctx.directory())?;
        if output.output_truncated {
            return Err(error(
                "SOURCE_OUTPUT_LIMIT",
                "Git响应超出读取上限；未使用截断的文件树或对象。",
            ));
        }
        match output.termination {
            CommandTermination::Cancelled => {
                return Err(error(
                    "SOURCE_ACQUISITION_CANCELLED",
                    "源码任务已取消；现场已保留。",
                ))
            }
            CommandTermination::TimedOut => {
                return Err(error(
                    "SOURCE_ACQUISITION_TIMEOUT",
                    "源码任务已超时；现场已保留。",
                ))
            }
            _ => {}
        }
        if !output.success() {
            return Err(error(
                "SOURCE_PROCESS_FAILED",
                "受管Git执行未正常完成；请查看此任务日志，现场已保留。",
            )
            .detail("exitCode", output.exit_code)
            .detail("termination", format!("{:?}", output.termination))
            .detail("treeExited", output.tree_exited)
            .detail("stdinComplete", output.stdin_complete));
        }
        Ok(output.stdout.bytes)
    }
    fn arguments(&self, args: &[&str]) -> Vec<OsString> {
        let mut result = vec![format!("--git-dir={}", self.repository.path().display()).into()];
        result.extend(args.iter().map(OsString::from));
        result
    }
    async fn initialize(&self, ctx: &TaskContext, control: &Control) -> Result<()> {
        // A positional `init <directory>` calls chdir before Git reads its
        // longpaths setting. Explicit --git-dir keeps initialization rooted in
        // the already-created, retained directory even beyond MAX_PATH.
        let mut args = self.arguments(&["init", "--bare"]);
        args.push(format!("--template={}", self.template.path().display()).into());
        self.execute(ctx, control, args, Vec::new(), 65536).await?;
        Ok(())
    }
}
impl SourceManager {
    async fn fetch(
        &self,
        ctx: &TaskContext,
        control: &Control,
        record: &mut SourceRecord,
        destination: &mut Destination,
    ) -> Result<()> {
        record.phase = "environment".into();
        ctx.phase(&record.phase)?;
        let git = GitSession::new(self, ctx)?;
        record.git = Some(
            json!({"path":git.tool.executable,"version":git.tool.version.strip_prefix("git version ").unwrap_or(&git.tool.version)}),
        );
        git.initialize(ctx, control).await?;
        record.phase = "downloading".into();
        ctx.phase(&record.phase)?;
        let mut fetch = git.arguments(&[
            "fetch",
            "--depth=1",
            "--no-tags",
            "--no-recurse-submodules",
            "--",
        ]);
        fetch.push(record.url.clone().into());
        fetch.push("HEAD".into());
        git.execute(ctx, control, fetch, Vec::new(), 65536).await?;
        self.materialize_git(ctx, control, record, destination, &git, "FETCH_HEAD")
            .await
    }
    async fn materialize_git(
        &self,
        ctx: &TaskContext,
        control: &Control,
        record: &mut SourceRecord,
        destination: &mut Destination,
        git: &GitSession,
        reference: &str,
    ) -> Result<()> {
        record.phase = "validating-objects".into();
        ctx.phase(&record.phase)?;
        // Pin the completed bare database before even rev-parse/fsck. Every
        // subsequent command is read-only, and each transition rejects new or
        // changed objects as well as substitutions of existing objects.
        let evidence_root = generated_evidence_root(&git.repository)?;
        let evidence = snapshot::pin_evidence(&evidence_root, control)
            .map_err(|issue| issue.detail("evidenceStage", "before-rev-parse"))?;
        let commit = policy::parse_commit(
            &git.execute(
                ctx,
                control,
                git.arguments(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")]),
                Vec::new(),
                65536,
            )
            .await?,
        )?;
        evidence
            .verify(control)
            .map_err(|issue| issue.detail("evidenceStage", "after-rev-parse"))?;
        git.execute(
            ctx,
            control,
            git.arguments(&["fsck", "--full", "--strict", "--no-reflogs"]),
            Vec::new(),
            65536,
        )
        .await?;
        evidence
            .verify(control)
            .map_err(|issue| issue.detail("evidenceStage", "after-fsck"))?;
        let entries = policy::parse_tree(
            &git.execute(
                ctx,
                control,
                git.arguments(&["ls-tree", "-r", "-z", "--long", "--full-tree", &commit]),
                Vec::new(),
                8 * 1024 * 1024,
            )
            .await?,
        )?;
        evidence
            .verify(control)
            .map_err(|issue| issue.detail("evidenceStage", "after-ls-tree"))?;
        record.commit = Some(commit);
        record.phase = "materializing".into();
        ctx.phase(&record.phase)?;
        // The completed bare object database stays pinned during all cat-file
        // reads. No checkout, hook, attribute filter or application script runs.
        let mut index = 0;
        while index < entries.len() {
            let start = index;
            let mut budget = 0usize;
            while index < entries.len() && index - start < 128 {
                let next = entries[index].size as usize + 192;
                if index > start && budget + next > 12 * 1024 * 1024 {
                    break;
                }
                budget += next;
                index += 1;
            }
            let batch = &entries[start..index];
            evidence.verify(control)?;
            let input = batch
                .iter()
                .flat_map(|entry| format!("{}\n", entry.oid).into_bytes())
                .collect();
            let bytes = git
                .execute(
                    ctx,
                    control,
                    git.arguments(&["cat-file", "--batch"]),
                    input,
                    budget + 1,
                )
                .await?;
            let mut offset = 0;
            for entry in batch {
                control.check()?;
                let relative = bytes
                    .get(offset..)
                    .ok_or_else(|| invalid("Git对象响应不完整。"))?;
                let newline = relative
                    .iter()
                    .take(161)
                    .position(|byte| *byte == b'\n')
                    .ok_or_else(|| invalid("Git对象响应头超限。"))?;
                if relative[..newline] != *format!("{} blob {}", entry.oid, entry.size).as_bytes() {
                    return Err(invalid("Git对象响应与已核验文件树不一致。"));
                }
                offset += newline + 1;
                let end = offset.checked_add(entry.size as usize).ok_or_else(limit)?;
                let payload = bytes
                    .get(offset..end)
                    .ok_or_else(|| invalid("Git对象内容不完整。"))?;
                if bytes.get(end) != Some(&b'\n') {
                    return Err(invalid("Git对象分隔符无效。"));
                }
                destination.write(&entry.path, payload, entry.executable)?;
                offset = end + 1;
            }
            if offset != bytes.len() {
                return Err(invalid("Git返回未请求的额外对象内容。"));
            }
            evidence.verify(control)?;
            check_disk(&destination.root)?;
        }
        record.phase = "inspecting".into();
        ctx.phase(&record.phase)?;
        record.inspection = Some(policy::inspect_directory(
            &WindowsSourceDirectory::from_workspace(&destination.root).map_err(fs_error)?,
        )?);
        control.check()?;
        Ok(())
    }
}

fn disk_budget(root: &WorkspaceDirectory) -> Result<()> {
    fn size(
        directory: &WindowsSourceDirectory,
        total: &mut u64,
        count: &mut usize,
        depth: usize,
    ) -> Result<()> {
        if depth > 40 {
            return Err(limit());
        }
        for entry in directory.source_entries(MAX_ENTRIES).map_err(fs_error)? {
            *count += 1;
            *total = total.checked_add(entry.length).ok_or_else(limit)?;
            if *count > MAX_ENTRIES || *total > MAX_DISK || entry.reparse {
                return Err(limit());
            }
            if entry.is_directory {
                size(
                    &directory.open_directory(&entry.name).map_err(fs_error)?,
                    total,
                    count,
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }
    check_disk(root)?;
    let directory = generated_evidence_root(root)?;
    size(&directory, &mut 0, &mut 0, 0)
}

fn generated_evidence_root(root: &WorkspaceDirectory) -> Result<WindowsSourceDirectory> {
    root.verify_binding().map_err(fs_error)?;
    // Git creates ordinary children that inherit the retained private parent.
    // Observe those as tool evidence, keeping the normal no-reparse, owner and
    // no-untrusted-writer checks. No child's ACL is repaired or adopted as a
    // platform-owned immutable candidate.
    let result = WindowsSourceDirectory::open(root.path()).map_err(fs_error)?;
    if result.identity() != root.identity() {
        return Err(invalid("Git操作证据目录身份已变化。"));
    }
    root.verify_binding().map_err(fs_error)?;
    Ok(result)
}

#[cfg(test)]
#[path = "windows_sources/tests.rs"]
mod tests;
