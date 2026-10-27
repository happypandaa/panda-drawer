//! Explicit, non-executing source acquisition. A source slot is not a project or
//! an installed application. Candidate paths are relative to its workspace; the
//! optional local origin is provenance only and is never followed on recovery.
use crate::{
    builds::{atomic_json, BuildManager},
    environment::ExecutableInfo,
    git_environment::{GitEnvironmentManager, GitToolchain},
    node_app_manifest::{TrialManifest, TrialRuntime},
    node_development_build::NodeDevelopmentBuildOutput,
    projects::{check_no_symlinks, checked_relative, valid_id, validate_manifest},
    ApiError, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
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
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
    task::JoinHandle,
    time::Instant,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

#[path = "source_acquisition_process.rs"]
mod process;
#[path = "source_publication.rs"]
mod publication;
#[cfg(test)]
use process::SourceHooks;
use process::{execute, materialize, resolve_source_git, SourceScope};

const TIMEOUT: Duration = Duration::from_secs(600);
const MAX_FILES: usize = 10_000;
const MAX_BLOB: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 256 * 1024 * 1024;
const MAX_DISK: u64 = 512 * 1024 * 1024;
const MIN_FREE: u64 = 64 * 1024 * 1024;
const MAX_METADATA: u64 = 256 * 1024;
const MAX_TREE_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;

enum CaptureInput {
    External(PathBuf),
    Built(NodeDevelopmentBuildOutput, Option<String>),
}
impl CaptureInput {
    fn path(&self) -> &Path {
        match self {
            Self::External(path) => path,
            Self::Built(output, _) => &output.program_root,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireSourceInput {
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureLocalSourceInput {
    pub source_path: PathBuf,
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
    #[serde(default)]
    pub requires_build: bool,
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
    #[serde(default)]
    branch: Option<String>,
    provider: String,
    #[serde(default)]
    local_source_path: Option<PathBuf>,
    #[serde(default)]
    capture_policy: Option<String>,
    #[serde(default)]
    build_source_digest: Option<String>,
    #[serde(default)]
    excluded_paths: Vec<String>,
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
    #[serde(skip)]
    legacy_build_inspection: bool,
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
    pub branch: Option<String>,
    pub provider: String,
    pub local_source_path: Option<PathBuf>,
    pub capture_policy: Option<String>,
    pub build_source_digest: Option<String>,
    pub capture_notice: Option<String>,
    pub excluded_paths: Vec<String>,
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
    active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    #[cfg(test)]
    hooks: Arc<SourceHooks>,
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
            active: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            hooks: Arc::new(SourceHooks::default()),
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
        let value: serde_json::Value =
            serde_json::from_slice(&read_small(&self.slot(id)?.join("source.json"))?)
                .map_err(|_| invalid("源码候选记录无效，请保留现场。"))?;
        let legacy_build_inspection = value["inspection"].get("requiresBuild").is_none();
        let mut record: SourceRecord =
            serde_json::from_value(value).map_err(|_| invalid("源码候选记录无效，请保留现场。"))?;
        record.legacy_build_inspection = legacy_build_inspection;
        if record.schema_version != 1
            || record.source_id != id
            || !valid_id(&record.task_id)
            || record.repository != "repository"
            || record
                .branch
                .as_deref()
                .is_some_and(|branch| validate_branch(branch).is_err())
            || (record.provider == "local" && record.branch.is_some())
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
        let built = record.capture_policy.as_deref() == Some("node-development-build-v1");
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
        let mut inspection = record.inspection;
        if record.build_source_digest.is_some() {
            if let Some(inspection) = &mut inspection {
                inspection.requires_build = false;
            }
        } else if record.legacy_build_inspection {
            if let (Some(inspection), Some(root)) = (&mut inspection, &source_path) {
                if inspection.kind == "node-service-source" {
                    // Old metadata did not describe build authorization. Derive
                    // it from a verified immutable public manifest, read-only.
                    // New records avoid this additional full tree verification.
                    let path = root.join("local-app.json");
                    let bytes = read_small(&path)?;
                    if record.content_digest.as_deref() != Some(directory_digest(root)?.as_str())
                        || read_small(&path)? != bytes
                    {
                        return Err(error(
                            "SOURCE_CHANGED",
                            "候选源码已经改变，请重新获取后核对构建方式。",
                        ));
                    }
                    let manifest = TrialManifest::parse(&bytes)
                        .map_err(|_| invalid("应用声明无效，无法核对旧来源的构建方式。"))?;
                    inspection.requires_build =
                        manifest.development.as_ref().is_some_and(|declaration| {
                            matches!(
                                declaration.build,
                                crate::node_app_manifest::NodeDevelopmentBuild::Npm { .. }
                            )
                        });
                }
            }
        }
        Ok(SourceCandidate {
            source_id: record.source_id,
            task_id: record.task_id,
            name: record.name,
            status: record.status,
            phase: record.phase,
            url: record.url,
            capture_notice: (record.provider == "local").then(|| if built {
                "开发材料已在独立目录准备，未启动应用。已排除 Git、依赖及已知数据和凭据目录，不保证识别源码中的秘密。".into()
            } else {
                "按已知路径排除了本机数据、依赖、凭据和临时文件；未知文件中的秘密无法通用识别，不能视为已脱敏。".into()
            }),
            provider: record.provider,
            branch: record.branch,
            local_source_path: record.local_source_path,
            capture_policy: record.capture_policy,
            build_source_digest: record.build_source_digest,
            excluded_paths: record.excluded_paths,
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
            inspection,
            error: record.error,
            execution_state: if built { "build-prepared-program-not-started" } else { "not-executed" }.into(),
            registration_available: false,
        })
    }
    pub fn info(&self, id: &str) -> Result<SourceCandidate, ApiError> {
        self.view(self.read_record(id)?)
    }
    pub(crate) fn git_environment(&self) -> GitEnvironmentManager {
        self.git.clone()
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
        self.verify_ready_contents(id, expected_digest)
    }
    /// The caller owns the exact source task lease for the entire use/copy of
    /// this verified candidate. Do not attempt a second transient reservation.
    pub(crate) fn verify_ready_leased(
        &self,
        id: &str,
        expected_digest: &str,
        task_id: &str,
    ) -> Result<SourceCandidate, ApiError> {
        if self.store.active_task(&format!("source:{id}")).as_deref() != Some(task_id) {
            return Err(error(
                "SOURCE_LEASE_REQUIRED",
                "操作尚未持有此源码候选的专属使用权限。",
            ));
        }
        self.verify_ready_contents(id, expected_digest)
    }
    fn verify_ready_contents(
        &self,
        id: &str,
        expected_digest: &str,
    ) -> Result<SourceCandidate, ApiError> {
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
                "候选源码已经改变，未执行接入。请重新获取或明确处理修改。",
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
            branch: None,
            provider: String::new(),
            local_source_path: None,
            capture_policy: None,
            build_source_digest: None,
            capture_notice: None,
            excluded_paths: Vec::new(),
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
        if let Some(branch) = &input.branch {
            validate_branch(branch)?;
        }
        let name = input.name.unwrap_or(repository_name);
        self.start_source(name, url, input.branch, provider, None, stop)
    }
    pub async fn capture(
        self: &Arc<Self>,
        input: CaptureLocalSourceInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let source = validate_local_root(&input.source_path, &self.paths)?;
        let name = input.name.unwrap_or_else(|| {
            source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        self.start_source(
            name,
            String::new(),
            None,
            "local".into(),
            Some(CaptureInput::External(source)),
            stop,
        )
    }
    /// This internal entry accepts only a token produced by the isolated Node
    /// builder. It never relaxes public local-directory capture restrictions.
    pub(crate) async fn capture_built_output(
        self: &Arc<Self>,
        output: NodeDevelopmentBuildOutput,
        name: Option<String>,
        commit: Option<String>,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        output.verify()?;
        if commit.as_deref().is_some_and(|value| {
            ![40, 64].contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err(invalid("构建来源提交号无效。"));
        }
        let name = name.unwrap_or_else(|| "Node 开发构建".into());
        self.start_source(
            name,
            String::new(),
            None,
            "local".into(),
            Some(CaptureInput::Built(output, commit)),
            stop,
        )
    }
    fn start_source(
        self: &Arc<Self>,
        name: String,
        url: String,
        branch: Option<String>,
        provider: String,
        local_input: Option<CaptureInput>,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
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
        let local = local_input.is_some();
        let built = matches!(local_input, Some(CaptureInput::Built(_, _)));
        let commit = match &local_input {
            Some(CaptureInput::Built(_, commit)) => commit.clone(),
            _ => None,
        };
        let build_source_digest = match &local_input {
            Some(CaptureInput::Built(output, _)) => Some(output.source_digest.clone()),
            _ => None,
        };
        let task = self.tasks.create_workspace_operation(if local {
            "sourceCapture"
        } else {
            "sourceAcquire"
        })?;
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
            legacy_build_inspection: false,
            schema_version: 1,
            source_id: id.clone(),
            task_id: task.task_id.clone(),
            name: name.trim().into(),
            status: "acquiring".into(),
            phase: "queued".into(),
            url,
            branch,
            provider,
            local_source_path: local_input.as_ref().map(|input| input.path().to_path_buf()),
            capture_policy: local.then(|| {
                if built {
                    "node-development-build-v1".into()
                } else {
                    CAPTURE_POLICY.into()
                }
            }),
            build_source_digest,
            excluded_paths: Vec::new(),
            created_at: now(),
            completed_at: None,
            repository: "repository".into(),
            shallow: !local,
            git: None,
            commit,
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
            self.tasks.append(&task.task_id, format!("Source candidate: {id}\nProvider: {}\nPublic URL: {}\nWorkspace path: .platform/sources/{id}\nNo project code is executed.\n", record.provider, record.url).as_bytes())?;
            fs::File::open(self.root().join(&id)).map_err(ApiError::from)
        })();
        let anchor = match prepared {
            Ok(anchor) => anchor,
            Err(error) => {
                self.tasks.finish(&task.task_id, Err(error.clone()))?;
                return Err(error);
            }
        };
        let scope = Arc::new(SourceScope::new(
            lease,
            self.active.clone(),
            self.tasks.clone(),
            task.task_id.clone(),
            self.root().join(&id),
            anchor,
        ));
        let (sender, cancel) = watch::channel(false);
        self.active
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), sender);
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            manager.run(record, stop, cancel, local_input, scope).await;
        });
        let mut runners = self.runners.lock().unwrap();
        // Preserve completed outcomes for the shutdown join; panic is not success.
        runners.push(runner);
        Ok(task)
    }
    pub fn cancel(&self, task_id: &str) -> Result<TaskRecord, ApiError> {
        let task = self.tasks.task(task_id)?;
        if !matches!(task.task_type.as_str(), "sourceAcquire" | "sourceCapture") {
            return Err(error("SOURCE_TASK_INVALID", "该任务不是源码获取任务。"));
        }
        if let Some(cancel) = self.active.lock().unwrap().get(task_id) {
            let _ = cancel.send(true);
        }
        Ok(task)
    }
    pub(crate) fn request_stop(&self) {
        {
            let _submission = self.submission.lock().unwrap();
            self.stopping.store(true, Ordering::Release);
            for cancel in self.active.lock().unwrap().values() {
                let _ = cancel.send(true);
            }
        }
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        let issues = crate::shutdown::join_all("sourceAcquisition", runners).await;
        issues
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
        local_input: Option<CaptureInput>,
        scope: Arc<SourceScope>,
    ) {
        let mut control = RunControl {
            stop,
            cancel,
            deadline: Instant::now() + TIMEOUT,
            monitor: self.root().join(&record.source_id),
            processes: self.store.processes(),
            scope: Some(scope.clone()),
            #[cfg(test)]
            hooks: self.hooks.clone(),
        };
        let result = if record.provider == "local" {
            self.capture_local(&mut record, &mut control, local_input.as_ref())
                .await
        } else {
            self.acquire(&mut record, &mut control).await
        }
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
                scope
                    .check_identity()
                    .and_then(|()| self.save_record(&record))
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
                if error.details["processResourceClosed"] == false {
                    record.phase = "process-unconfirmed".into();
                }
                record.error = Some(error.clone());
                if let Err(persist) = scope
                    .check_identity()
                    .and_then(|()| self.save_record(&record))
                {
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
                let unknown = error.details["processResourceClosed"] == false
                    || error.details["candidateOwnershipConfirmed"] == false;
                let issue = error.detail("sourceId", record.source_id.clone());
                Err(if unknown {
                    issue.outcome("unknown")
                } else {
                    issue.outcome("not_registered")
                })
            }
        };
        let _ = self.tasks.append(
            &record.task_id,
            format!("Result: {}\n", record.status).as_bytes(),
        );
        let _ = self.tasks.finish(&record.task_id, result);
        drop(control);
        drop(scope);
    }
    async fn capture_local(
        &self,
        record: &mut SourceRecord,
        control: &mut RunControl,
        input: Option<&CaptureInput>,
    ) -> Result<(), ApiError> {
        control.check()?;
        self.phase(record, "inspecting-local-scope")?;
        let original = record
            .local_source_path
            .clone()
            .ok_or_else(|| invalid("缺少本机来源路径。"))?;
        let trusted = match input {
            Some(CaptureInput::Built(output, _)) => {
                if output.program_root != original {
                    return Err(invalid("构建产物位置已变化。"));
                }
                output.verify()?;
                Some(output)
            }
            _ => None,
        };
        let source = if trusted.is_some() {
            original
        } else {
            validate_local_root(&original, &self.paths)?
        };
        let slot = self.slot(&record.source_id)?;
        let stage = slot.join(".acquiring");
        new_dir(&stage)?;
        let candidate = stage.join("repository");
        new_dir(&candidate)?;
        let scan_control = control.clone();
        let scan_source = source.clone();
        let plan = tokio::task::spawn_blocking(move || local_plan(&scan_source, &scan_control))
            .await
            .map_err(|_| invalid("本机源码检查未完成；现场保留。"))??;
        record.excluded_paths = plan.excluded.clone();
        control.check()?;
        self.phase(record, "capturing-local")?;
        let copy_control = control.clone();
        let copy_candidate = candidate.clone();
        let copied = tokio::task::spawn_blocking(move || {
            capture_plan(&source, &copy_candidate, plan, &copy_control)
        })
        .await
        .map_err(|_| invalid("本机源码复制未完成；现场保留。"))??;
        record.file_count = Some(copied.file_count);
        record.total_bytes = Some(copied.total_bytes);
        record.content_digest = Some(copied.digest);
        if let Some(output) = trusted {
            output.verify()?;
        }
        control.check()?;
        self.phase(record, "inspecting")?;
        let mut inspection = inspect_directory(&candidate)?;
        if trusted.is_some() {
            inspection.execution_state = "build-prepared-program-not-started".into();
        }
        if !copied.issues.is_empty() {
            inspection.kind = "needs-adaptation".into();
            inspection.next_step = "review-and-adapt-source".into();
            inspection.issues.extend(copied.issues);
        }
        record.inspection = Some(inspection);
        control.check()?;
        check_disk(&slot)?;
        let published = slot.join("repository");
        if published.try_exists()? {
            return Err(invalid("候选目录已经存在，未覆盖。"));
        }
        fs::rename(candidate, published)?;
        fs::File::open(slot)?.sync_all()?;
        Ok(())
    }
    async fn acquire(
        &self,
        record: &mut SourceRecord,
        control: &mut RunControl,
    ) -> Result<(), ApiError> {
        control.check()?;
        self.phase(record, "environment")?;
        let toolchain = resolve_source_git(&self.git, control).await?;
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
        control.check()?;
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
            .arg(format!("--template={}", template.display()));
        if let Some(branch) = &record.branch {
            clone.arg("--branch").arg(branch);
        }
        clone.arg("--").arg(&record.url).arg(&repository);
        execute(clone, control, MAX_OUTPUT).await?;
        if let Some(branch) = &record.branch {
            // --branch can also name a tag. Accept only an actual local branch
            // created from the explicitly requested remote branch, never a
            // detached tag/commit or a fallback to the remote default branch.
            let mut symbolic = isolated_command(&toolchain, &repository, &home, &template);
            symbolic.args(["symbolic-ref", "--quiet", "HEAD"]);
            let actual = execute(symbolic, control, MAX_OUTPUT)
                .await
                .map_err(|issue| {
                    if issue.code == "SOURCE_GIT_FAILED" {
                        error(
                            "SOURCE_BRANCH_UNAVAILABLE",
                            "未获取到指定分支，请核对公开仓库中的分支名称。",
                        )
                    } else {
                        issue
                    }
                })?;
            if actual != format!("refs/heads/{branch}\n").as_bytes() {
                return Err(error(
                    "SOURCE_BRANCH_UNAVAILABLE",
                    "获取结果与指定分支不一致，未发布源码候选。",
                ));
            }
        }
        control.check()?;
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
        control.check()?;
        self.phase(record, "materializing")?;
        record.content_digest =
            Some(materialize(&toolchain, &repository, &home, &template, &entries, control).await?);
        // Populate only Git's index. No checkout, attributes filters or project
        // scripts run, and later status sees the materialized tree accurately.
        let mut index = isolated_command(&toolchain, &repository, &home, &template);
        index.args(["read-tree", &commit]);
        execute(index, control, MAX_OUTPUT).await?;
        control.check()?;
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

const CAPTURE_POLICY: &str = "local-source-v1";
fn validate_local_root(path: &Path, paths: &RuntimePaths) -> Result<PathBuf, ApiError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid("请选择具体源码文件夹的绝对路径。"));
    }
    check_no_symlinks(path, false)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(invalid("请选择普通源码目录。"));
    }
    let root = path.canonicalize()?;
    let data = paths.data_root().canonicalize()?;
    let control = paths.workspace_root().join(".platform");
    if root.starts_with(&data)
        || data.starts_with(&root)
        || root.starts_with(&control)
        || control.starts_with(&root)
        || std::env::var_os("HOME")
            .and_then(|home| PathBuf::from(home).canonicalize().ok())
            .is_some_and(|home| home.starts_with(&root))
    {
        return Err(invalid(
            "不能接入平台数据/控制目录、其父级或整个用户目录；请选择独立的具体源码文件夹。",
        ));
    }
    Ok(root)
}
fn local_excluded(path: &str, output: Option<&str>) -> bool {
    let folded = path.to_ascii_lowercase();
    if output.is_some_and(|output| {
        let output = output.to_ascii_lowercase();
        folded == output || folded.starts_with(&format!("{output}/"))
    }) {
        return true;
    }
    if ["user", "work"]
        .iter()
        .any(|name| folded == *name || folded.starts_with(&format!("{name}/")))
    {
        return true;
    }
    folded.split('/').any(|name| {
        matches!(
            name,
            ".git"
                | ".hg"
                | ".svn"
                | ".platform"
                | "node_modules"
                | "data"
                | ".data"
                | "attachments"
                | "config"
                | "configuration"
                | "credentials"
                | "secrets"
                | "tokens"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".npmrc"
                | ".netrc"
                | ".git-credentials"
                | ".pypirc"
                | "id_rsa"
                | "id_ed25519"
                | "id_ecdsa"
                | "id_dsa"
                | "cache"
                | ".cache"
                | ".vite"
                | ".next"
                | ".nuxt"
                | "coverage"
                | "tmp"
                | ".tmp"
                | "temp"
                | "runs"
                | "backups"
                | "trash"
                | ".ds_store"
        ) || (name.starts_with(".env") && name != ".env.example")
            || [
                ".pem",
                ".key",
                ".p12",
                ".pfx",
                ".sqlite",
                ".sqlite3",
                ".db",
                ".sqlite-wal",
                ".sqlite-shm",
                ".sqlite3-wal",
                ".sqlite3-shm",
                ".db-wal",
                ".db-shm",
                "-wal",
                "-shm",
                "-journal",
            ]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalFile {
    size: u64,
    executable: bool,
    digest: [u8; 32],
}
#[derive(Debug, PartialEq, Eq)]
struct LocalPlan {
    identity: (u64, u64),
    files: BTreeMap<String, LocalFile>,
    excluded: Vec<String>,
}
impl LocalPlan {
    fn digest(&self) -> String {
        let mut tree = Sha256::new();
        for (path, file) in &self.files {
            tree.update(path.as_bytes());
            tree.update([0]);
            tree.update(file.size.to_le_bytes());
            tree.update([u8::from(file.executable)]);
            tree.update(file.digest);
        }
        format!("{:x}", tree.finalize())
    }
}
fn capture_read(path: &Path, control: &RunControl) -> Result<(Vec<u8>, bool), ApiError> {
    control.check()?;
    ordinary_file(path)?;
    let before = fs::symlink_metadata(path)?;
    if before.len() > MAX_BLOB {
        return Err(limit());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let same = |metadata: &fs::Metadata| {
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.dev() == before.dev()
            && metadata.ino() == before.ino()
            && metadata.len() == before.len()
            && metadata.mode() == before.mode()
            && metadata.mtime() == before.mtime()
            && metadata.mtime_nsec() == before.mtime_nsec()
            && metadata.ctime() == before.ctime()
            && metadata.ctime_nsec() == before.ctime_nsec()
    };
    if !same(&file.metadata()?) {
        return Err(local_changed());
    }
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        control.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > MAX_BLOB {
            return Err(limit());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if bytes.len() as u64 != before.len()
        || !same(&file.metadata()?)
        || !same(&fs::symlink_metadata(path)?)
    {
        return Err(local_changed());
    }
    Ok((bytes, before.mode() & 0o111 != 0))
}
fn local_changed() -> ApiError {
    error(
        "SOURCE_CHANGED",
        "本机源码在复制时发生变化；未发布候选，请停止编辑后重新接入。",
    )
}
fn local_plan(root: &Path, control: &RunControl) -> Result<LocalPlan, ApiError> {
    check_no_symlinks(root, false)?;
    let identity = fs::symlink_metadata(root)?;
    let output = if root.join("project.json").try_exists()? {
        let bytes = read_small(&root.join("project.json"))?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|manifest| manifest["outputDir"].as_str().map(str::to_owned))
            .filter(|path| validate_tree_path(path).is_ok())
    } else {
        None
    };
    let mut plan = LocalPlan {
        identity: (identity.dev(), identity.ino()),
        files: BTreeMap::new(),
        excluded: Vec::new(),
    };
    fn visit(
        root: &Path,
        relative: &Path,
        output: Option<&str>,
        plan: &mut LocalPlan,
        control: &RunControl,
        count: &mut usize,
        total: &mut u64,
    ) -> Result<(), ApiError> {
        control.check()?;
        let directory = root.join(relative);
        check_no_symlinks(&directory, false)?;
        let mut entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            control.check()?;
            *count += 1;
            if *count > MAX_FILES * 4 {
                return Err(limit());
            }
            let path = relative.join(entry.file_name());
            let label = path
                .to_str()
                .ok_or_else(|| invalid("本机源码路径不是UTF-8。"))?
                .to_owned();
            // Do not stat, recurse into or open an excluded entry, including an
            // excluded symlink. Only its name is part of the visible scope report.
            if local_excluded(&label, output) {
                if label.len() > 1024
                    || plan.excluded.len() >= 1000
                    || plan.excluded.iter().map(String::len).sum::<usize>() + label.len()
                        > 64 * 1024
                {
                    return Err(limit());
                }
                plan.excluded.push(label);
                continue;
            }
            validate_tree_path(&label)?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                visit(root, &path, output, plan, control, count, total)?;
            } else if metadata.is_file() && metadata.nlink() == 1 {
                if plan.files.len() >= MAX_FILES || metadata.len() > MAX_BLOB {
                    return Err(limit());
                }
                *total = total.checked_add(metadata.len()).ok_or_else(limit)?;
                if *total > MAX_TREE {
                    return Err(limit());
                }
                let (bytes, executable) = capture_read(&entry.path(), control)?;
                plan.files.insert(
                    label,
                    LocalFile {
                        size: bytes.len() as u64,
                        executable,
                        digest: Sha256::digest(&bytes).into(),
                    },
                );
            } else {
                return Err(invalid(
                    "本机源码包含未排除的链接、硬链接或特殊文件；未复制。",
                ));
            }
        }
        Ok(())
    }
    visit(
        root,
        Path::new(""),
        output.as_deref(),
        &mut plan,
        control,
        &mut 0,
        &mut 0,
    )?;
    // Reuse the acquisition path collision policy, without executing Git.
    let rows: Vec<u8> = plan
        .files
        .iter()
        .flat_map(|(path, file)| {
            format!(
                "{} blob {} {}\t{}\0",
                if file.executable { "100755" } else { "100644" },
                "1".repeat(40),
                file.size,
                path
            )
            .into_bytes()
        })
        .collect();
    parse_tree(&rows)?;
    Ok(plan)
}
struct CapturedLocal {
    digest: String,
    file_count: usize,
    total_bytes: u64,
    issues: Vec<ApiError>,
}
fn capture_plan(
    source: &Path,
    candidate: &Path,
    plan: LocalPlan,
    control: &RunControl,
) -> Result<CapturedLocal, ApiError> {
    let digest = plan.digest();
    for (index, (relative, expected)) in plan.files.iter().enumerate() {
        control.check()?;
        let (bytes, executable) = capture_read(&source.join(relative), control)?;
        if bytes.len() as u64 != expected.size
            || executable != expected.executable
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != expected.digest
        {
            return Err(local_changed());
        }
        let destination = candidate.join(relative);
        private_dir(
            destination
                .parent()
                .ok_or_else(|| invalid("候选路径无父目录。"))?,
        )?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if executable { 0o700 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW)
            .open(destination)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        // A full evidence-tree scan per file would make a 10,000-file capture
        // quadratic. Space is still checked while copying and before publish.
        if index % 64 == 0 {
            check_disk(&control.monitor)?;
        }
    }
    if local_plan(source, control)? != plan || directory_digest(candidate)? != digest {
        return Err(local_changed());
    }
    let mut issues = validate_local_dependencies(candidate, &plan, control)?;
    if plan.excluded.iter().any(|path| {
        path.split('/')
            .any(|name| name.eq_ignore_ascii_case("config"))
    }) {
        issues.push(invalid(
            "config 目录按本机配置保护规则排除；若其中是运行代码，请调整源码布局后重新接入。",
        ));
    }
    control.check()?;
    fs::File::open(candidate)?.sync_all()?;
    Ok(CapturedLocal {
        digest,
        file_count: plan.files.len(),
        total_bytes: plan.files.values().map(|file| file.size).sum(),
        issues,
    })
}
fn validate_local_dependencies(
    root: &Path,
    plan: &LocalPlan,
    control: &RunControl,
) -> Result<Vec<ApiError>, ApiError> {
    fn target(base: &Path, spec: &str, plan: &LocalPlan, pattern: bool) -> Result<(), ApiError> {
        if spec.is_empty()
            || spec.starts_with(['/', '~'])
            || spec.contains(['\\', '%', ':'])
            || spec.chars().any(char::is_control)
        {
            return Err(invalid(
                "本机依赖路径不受支持或越界；请将依赖材料放进所选源码目录。",
            ));
        }
        let mut path = base.to_path_buf();
        let mut wildcard = false;
        for part in spec.split('/') {
            match part {
                "" | "." => (),
                ".." => {
                    if wildcard || !path.pop() {
                        return Err(invalid("本机依赖指向所选源码以外；未接入。"));
                    }
                }
                part => {
                    if pattern && part.contains(['*', '?', '[', '{']) {
                        wildcard = true;
                    } else if !wildcard {
                        path.push(part);
                    }
                }
            }
        }
        let label = path.to_str().ok_or_else(|| invalid("依赖路径无效。"))?;
        if !label.is_empty()
            && !plan
                .files
                .keys()
                .any(|file| file == label || file.starts_with(&format!("{label}/")))
        {
            return Err(invalid("本机依赖材料不存在或已被保护规则排除；未接入。"));
        }
        Ok(())
    }
    fn spec(base: &Path, value: &str, plan: &LocalPlan, force: bool) -> Result<(), ApiError> {
        let lower = value.to_ascii_lowercase();
        if lower.starts_with("file:") || lower.starts_with("link:") {
            target(base, &value[5..], plan, false)
        } else if force
            || value.starts_with(['.', '/'])
            || value == "~"
            || value.starts_with("~/")
            || value.contains('\\')
            || (value.len() > 1 && value.as_bytes()[1] == b':')
        {
            target(base, value, plan, false)
        } else {
            Ok(())
        }
    }
    fn dependency_values(
        value: &serde_json::Value,
        base: &Path,
        plan: &LocalPlan,
    ) -> Result<(), ApiError> {
        if let Some(value) = value.as_str() {
            spec(base, value, plan, false)?;
        } else if let Some(values) = value.as_object() {
            for value in values.values() {
                dependency_values(value, base, plan)?;
            }
        }
        Ok(())
    }
    fn walk(value: &serde_json::Value, base: &Path, plan: &LocalPlan) -> Result<(), ApiError> {
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "dependencies"
                        | "devDependencies"
                        | "optionalDependencies"
                        | "peerDependencies"
                        | "overrides"
                ) {
                    dependency_values(value, base, plan)?;
                }
                if matches!(key.as_str(), "resolved" | "version") {
                    if let Some(value) = value.as_str() {
                        spec(
                            base,
                            value,
                            plan,
                            key == "resolved"
                                && object.get("link").and_then(serde_json::Value::as_bool)
                                    == Some(true),
                        )?;
                    }
                }
                if key == "workspaces" {
                    let entries = value
                        .as_array()
                        .or_else(|| value.get("packages").and_then(serde_json::Value::as_array));
                    if let Some(entries) = entries {
                        for value in entries.iter().filter_map(serde_json::Value::as_str) {
                            target(base, value, plan, true)?;
                        }
                    }
                }
                walk(value, base, plan)?;
            }
        } else if let Some(array) = value.as_array() {
            for value in array {
                walk(value, base, plan)?;
            }
        }
        Ok(())
    }
    let mut uninspectable = false;
    for relative in plan.files.keys() {
        if !matches!(
            Path::new(relative)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("package.json" | "package-lock.json" | "npm-shrinkwrap.json")
        ) {
            continue;
        }
        control.check()?;
        match read_small(&root.join(relative)).and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(ApiError::from)
        }) {
            Ok(value) => walk(
                &value,
                Path::new(relative).parent().unwrap_or(Path::new("")),
                plan,
            )?,
            Err(_) => uninspectable = true,
        }
    }
    // Keep persisted metadata bounded even when many nested declarations have
    // the same problem. The original files remain in the candidate for review.
    Ok(if uninspectable {
        vec![invalid(
            "存在无法检查的 npm 声明文件；源码已保留，但需要适配后才能安装。",
        )]
    } else {
        Vec::new()
    })
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
fn validate_branch(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > 128
        || value == "HEAD"
        || value.starts_with("refs/")
        || value.contains("..")
        || value.contains("@{")
        || value.ends_with('.')
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
        || value
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || ([40, 64].contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(error(
            "SOURCE_BRANCH_INVALID",
            "请输入普通分支名称，例如 main 或 codex/tts-providers。",
        ));
    }
    Ok(())
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
        requires_build: false,
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
                result.requires_build = manifest.development.as_ref().is_some_and(|declaration| {
                    matches!(
                        declaration.build,
                        crate::node_app_manifest::NodeDevelopmentBuild::Npm { .. }
                    )
                });
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

#[derive(Clone)]
struct RunControl {
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    monitor: PathBuf,
    processes: crate::owned_process::OwnedProcessRegistry,
    scope: Option<Arc<SourceScope>>,
    #[cfg(test)]
    hooks: Arc<SourceHooks>,
}
impl RunControl {
    fn check(&self) -> Result<(), ApiError> {
        if let Some(scope) = &self.scope {
            scope.check_identity()?;
        }
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
#[derive(Clone, Debug)]
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
#[cfg(test)]
#[path = "source_acquisition_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "source_acquisition_owned_process_tests.rs"]
mod owned_process_tests;
