//! Local build orchestration. All limits here are internal test parameters, not SDK promises.
use crate::{
    environment::{EnvironmentManager, Toolchain},
    git_environment::GitEnvironmentManager,
    project_git::BuildGitGuard,
    projects::{check_no_symlinks, checked_relative},
    tasks::{TaskRecord, TaskStatus},
    ApiError, ProjectHandle, ProjectManifest, ProjectStore, RuntimePaths,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
};
use uuid::Uuid;
const BUILD_QUEUE: usize = 8;
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);
const LOG_HEAD: usize = 2_000_000;
const LOG_TAIL: usize = 8_000_000;
const LOG_PAGE: usize = 64_000;
const DIGEST_RULE: u32 = 1;

#[path = "build_result.rs"]
mod build_result;
#[path = "build_candidate.rs"]
mod candidate;
pub use build_result::BuildResultRecovery;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildEvent {
    pub project_id: String,
    pub task_id: String,
    pub status: TaskStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildProvenance {
    pub schema_version: u32,
    pub build_id: String,
    pub task_id: String,
    pub completed_at: u64,
    pub digest_rule_version: u32,
    pub input_digest: String,
    pub artifact_digest: String,
    pub node: crate::environment::ExecutableInfo,
    pub npm: crate::environment::ExecutableInfo,
    pub sdk_version: Option<String>,
    pub entry_relative: String,
}
/// Built once while packaging the platform, never a user build task.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StarterDistribution {
    pub schema_version: u32,
    pub origin: String,
    pub template_version: String,
    pub sdk_version: String,
    pub source_digest: String,
    pub artifact_digest: String,
    pub entry_relative: String,
    pub built_at: u64,
    pub node_version: String,
    pub npm_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StarterProvenance {
    pub schema_version: u32,
    pub origin: String,
    pub build_id: String,
    pub installed_at: u64,
    pub digest_rule_version: u32,
    pub input_digest: String,
    pub distribution: StarterDistribution,
}
#[derive(Clone, Debug)]
pub struct ArtifactGeneration {
    pub build_id: String,
    pub root: PathBuf,
    pub entry_relative: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLogs {
    pub task_id: String,
    pub cursor: u64,
    pub next_cursor: u64,
    pub text: String,
    pub truncated: bool,
    pub gaps: Vec<LogGap>,
    pub eof: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct LogGap {
    pub from: u64,
    pub to: u64,
}
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LogMeta {
    total: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    project_id: String,
    provenance: BuildProvenance,
    previous_digest: Option<String>,
}
struct Registry {
    records: HashMap<String, TaskRecord>,
    legacy_build_times: HashMap<String, Option<SystemTime>>,
    active: HashSet<String>,
    leases: HashMap<String, crate::projects::ProjectTaskLease>,
}
pub struct BuildManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    base_url: String,
    environment: EnvironmentManager,
    git_environment: GitEnvironmentManager,
    git_stop: watch::Sender<bool>,
    registry: Mutex<Registry>,
    sender: mpsc::Sender<String>,
    runner: Mutex<Option<JoinHandle<()>>>,
    stopping: AtomicBool,
    events: broadcast::Sender<BuildEvent>,
    log_lock: Arc<Mutex<()>>,
}
impl BuildManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        base_url: String,
    ) -> Result<Arc<Self>, ApiError> {
        Self::with_managed_node(paths, store, base_url, None)
    }
    pub fn with_managed_node(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        base_url: String,
        managed_node: Option<PathBuf>,
    ) -> Result<Arc<Self>, ApiError> {
        Self::with_managed_node_and_git(paths, store, base_url, managed_node, None)
    }
    pub fn with_managed_node_and_git(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        base_url: String,
        managed_node: Option<PathBuf>,
        managed_git: Option<PathBuf>,
    ) -> Result<Arc<Self>, ApiError> {
        let tasks = paths.workspace_root().join(".platform/tasks");
        ensure_dir(&tasks)?;
        ensure_dir(
            &paths
                .workspace_root()
                .join(".platform/project-state/builds"),
        )?;
        let mut records = HashMap::new();
        let mut legacy_build_times = HashMap::new();
        for entry in fs::read_dir(&tasks)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            check_no_symlinks(&path, false)?;
            let record_path = path.join("task.json");
            if !record_path.is_file() {
                continue;
            }
            let mut record: TaskRecord = serde_json::from_slice(&fs::read(&record_path)?)?;
            if !valid_task_id(&record.task_id)
                || path.file_name().and_then(|x| x.to_str()) != Some(record.task_id.as_str())
            {
                return Err(ApiError::new(
                    "TASK_RECORD_INVALID",
                    "Task record identity does not match its directory",
                ));
            }
            if record.task_type == "build" {
                // Startup recovery rewrites task.json. Capture the old mtime
                // first; it is only a labelled fallback, never submission order.
                legacy_build_times
                    .insert(record.task_id.clone(), build_result::modified(&record_path));
            }
            if !record.status.is_final() {
                record.status = TaskStatus::Interrupted;
                record.error = Some(ApiError::new(
                    "TASK_INTERRUPTED",
                    "Platform exited before the task finished; no automatic retry was performed",
                ));
                atomic_json(&record_path, &record)?;
            }
            records.insert(record.task_id.clone(), record);
        }
        recover_publications(&paths, &store)?;
        reconcile_recovered_tasks(&paths, &store, &base_url, &mut records)?;
        let (sender, mut receiver) = mpsc::channel::<String>(BUILD_QUEUE);
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            environment: match managed_node {
                Some(root) => {
                    EnvironmentManager::with_managed_node(paths.data_root().to_path_buf(), root)
                }
                None => EnvironmentManager::new(paths.data_root().to_path_buf()),
            },
            git_environment: GitEnvironmentManager::with_processes(
                paths.data_root().to_path_buf(),
                managed_git,
                store.processes(),
            ),
            git_stop: watch::channel(false).0,
            paths,
            store,
            base_url,
            registry: Mutex::new(Registry {
                records,
                legacy_build_times,
                active: HashSet::new(),
                leases: HashMap::new(),
            }),
            sender,
            runner: Mutex::new(None),
            stopping: AtomicBool::new(false),
            events,
            log_lock: Arc::new(Mutex::new(())),
        });
        let weak = Arc::downgrade(&manager);
        let handle = tokio::spawn(async move {
            loop {
                let id = tokio::select! {id=receiver.recv()=>match id{Some(id)=>id,None=>break},_=tokio::time::sleep(Duration::from_millis(100))=>{if weak.upgrade().is_none_or(|manager|manager.stopping.load(Ordering::Acquire)){break;}continue;}};
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                if manager.stopping.load(Ordering::Acquire) {
                    break;
                }
                manager.run(&id).await;
            }
        });
        *manager.runner.lock().unwrap() = Some(handle);
        Ok(manager)
    }
    pub fn environment(&self) -> &EnvironmentManager {
        &self.environment
    }
    pub fn subscribe(&self) -> broadcast::Receiver<BuildEvent> {
        self.events.subscribe()
    }
    pub async fn submit(self: &Arc<Self>, project_id: &str) -> Result<TaskRecord, ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        self.submit_with_maintenance(project_id, None).await
    }
    pub async fn submit_with_maintenance(
        self: &Arc<Self>,
        project_id: &str,
        maintenance_id: Option<&str>,
    ) -> Result<TaskRecord, ApiError> {
        let task_id = format!("task_{}", Uuid::new_v4().simple());
        let lease = self.store.reserve_development_task(project_id, &task_id)?;
        self.store
            .control(project_id)?
            .check(false, maintenance_id, false)?;
        self.store.project(project_id)?.check()?;
        if self.registry.lock().unwrap().active.contains(project_id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "This project already has an unfinished build",
            ));
        }
        // Reject missing build environments before accepting a task; opening artifacts is independent.
        self.environment.resolve().await?;
        let mut registry = self.registry.lock().unwrap();
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        if registry.active.contains(project_id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "This project already has an unfinished build",
            ));
        }
        let permit = self.sender.try_reserve().map_err(|_| {
            ApiError::new(
                "BUILD_QUEUE_FULL",
                "Internal build queue is full; try again after a task finishes",
            )
        })?;
        let record = TaskRecord {
            task_id: task_id.clone(),
            project_id: Some(project_id.into()),
            task_type: "build".into(),
            status: TaskStatus::Queued,
            phase: "queued".into(),
            result: None,
            error: None,
        };
        ensure_dir(&self.task_dir(&record.task_id))?;
        // The admission record starts final: if pointer persistence fails (or
        // the process exits before acceptance), no queued orphan is retained.
        let mut pending = record.clone();
        pending.status = TaskStatus::Failed;
        pending.phase = "not-started".into();
        pending.error = Some(
            ApiError::new(
                "BUILD_SUBMISSION_INCOMPLETE",
                "构建提交尚未完成，未启动构建；请核对保留记录后明确重试。",
            )
            .outcome("not_started"),
        );
        atomic_json(&self.task_dir(&record.task_id).join("task.json"), &pending)?;
        let admission =
            build_result::remember(self.paths.workspace_root(), &record).and_then(|()| {
                atomic_json(&self.task_dir(&record.task_id).join("task.json"), &record).map_err(
                    |error| {
                        ApiError::new(
                            "BUILD_RESULT_PERSISTENCE_FAILED",
                            "无法保存本次构建任务，构建尚未启动；请保留任务记录并检查存储状态。",
                        )
                        .detail("taskPersistenceError", json!(error))
                    },
                )
            });
        if let Err(error) = admission {
            // No queue send or retained lease until both records are durable.
            // Even if the pointer rename happened before its fsync failed, it
            // must resolve to a final failure rather than a queued orphan.
            let error = error
                .detail("taskId", record.task_id.clone())
                .outcome("not_started");
            let mut failed = record;
            failed.status = TaskStatus::Failed;
            failed.phase = "not-started".into();
            failed.error = Some(error.clone());
            let saved = atomic_json(&self.task_dir(&failed.task_id).join("task.json"), &failed);
            registry.legacy_build_times.insert(
                failed.task_id.clone(),
                build_result::modified(&self.task_dir(&failed.task_id).join("task.json")),
            );
            registry.records.insert(failed.task_id.clone(), failed);
            return Err(match saved {
                Ok(()) => error,
                Err(persistence) => error.detail("taskPersistenceError", json!(persistence)),
            });
        }
        registry.active.insert(project_id.into());
        registry.leases.insert(task_id, lease);
        registry
            .records
            .insert(record.task_id.clone(), record.clone());
        permit.send(record.task_id.clone());
        Ok(record)
    }
    pub(crate) fn create_operation(
        &self,
        project_id: &str,
        kind: &str,
    ) -> Result<(TaskRecord, crate::projects::ProjectTaskLease), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let task_id = format!("task_{}", Uuid::new_v4().simple());
        let lease = self.store.reserve_development_task(project_id, &task_id)?;
        self.record_operation(project_id, kind, task_id, lease)
    }
    /// Permanent deletion may explicitly retry its own quarantine, but never
    /// bypasses pending source-restore protection or the project operation gate.
    pub(crate) fn create_delete_operation(
        &self,
        project_id: &str,
    ) -> Result<(TaskRecord, crate::projects::ProjectTaskLease), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let task_id = format!("task_{}", Uuid::new_v4().simple());
        let lease = self.store.reserve_delete_task(project_id, &task_id)?;
        self.record_operation(project_id, "delete", task_id, lease)
    }
    pub(crate) fn create_source_restore_recovery_operation(
        &self,
        project_id: &str,
        kind: &str,
        permit: &crate::source_restore_guard::SourceRestoreRecoveryPermit,
    ) -> Result<(TaskRecord, crate::projects::ProjectTaskLease), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        if !matches!(kind, "gitSourceRestoreContinue" | "gitSourceRestoreUndo") {
            return Err(ApiError::new(
                "INVALID_TASK_TYPE",
                "Expected an explicit source restore recovery operation",
            ));
        }
        let task_id = format!("task_{}", Uuid::new_v4().simple());
        let lease = self
            .store
            .reserve_source_restore_recovery_task(project_id, &task_id, permit)?;
        self.record_operation(project_id, kind, task_id, lease)
    }
    pub(crate) fn create_data_restore_recovery_operation(
        &self,
        project_id: &str,
        kind: &str,
        permit: &crate::data_restore_guard::DataRestoreRecoveryPermit,
    ) -> Result<(TaskRecord, crate::projects::ProjectTaskLease), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        if !matches!(kind, "gitDataRestoreContinue" | "gitDataRestoreUndo") {
            return Err(ApiError::new(
                "INVALID_TASK_TYPE",
                "Expected data restore recovery",
            ));
        }
        let task_id = format!("task_{}", Uuid::new_v4().simple());
        let lease = self
            .store
            .reserve_data_restore_recovery_task(project_id, &task_id, permit)?;
        self.record_operation(project_id, kind, task_id, lease)
    }
    pub(crate) fn create_data_save_finalization_operation(
        &self,
        project_id: &str,
        input: &crate::project_git::GitDataSaveFinalizeInput,
    ) -> Result<(TaskRecord, crate::projects::ProjectTaskLease), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let task_id = format!("task_{}", Uuid::new_v4().simple());
        let lease = self
            .store
            .reserve_data_save_finalization_task(project_id, &task_id, input)?;
        self.record_operation(project_id, "gitDataSaveFinalize", task_id, lease)
    }
    fn record_operation(
        &self,
        project_id: &str,
        kind: &str,
        task_id: String,
        lease: crate::projects::ProjectTaskLease,
    ) -> Result<(TaskRecord, crate::projects::ProjectTaskLease), ApiError> {
        let record = TaskRecord {
            task_id,
            project_id: Some(project_id.into()),
            task_type: kind.into(),
            status: TaskStatus::Queued,
            phase: "queued".into(),
            result: None,
            error: None,
        };
        ensure_dir(&self.task_dir(&record.task_id))?;
        atomic_json(&self.task_dir(&record.task_id).join("task.json"), &record)?;
        self.registry
            .lock()
            .unwrap()
            .records
            .insert(record.task_id.clone(), record.clone());
        Ok((record, lease))
    }
    pub(crate) fn create_readonly_operation(
        &self,
        project_id: &str,
        kind: &str,
    ) -> Result<TaskRecord, ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        self.store.check_workspace_available()?;
        if !crate::projects::valid_id(project_id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let record = TaskRecord {
            task_id: format!("task_{}", Uuid::new_v4().simple()),
            project_id: Some(project_id.into()),
            task_type: kind.into(),
            status: TaskStatus::Queued,
            phase: "queued".into(),
            result: None,
            error: None,
        };
        ensure_dir(&self.task_dir(&record.task_id))?;
        atomic_json(&self.task_dir(&record.task_id).join("task.json"), &record)?;
        self.registry
            .lock()
            .unwrap()
            .records
            .insert(record.task_id.clone(), record.clone());
        Ok(record)
    }
    /// Workspace management holds its own exclusive gate and has no project lease.
    pub(crate) fn create_workspace_operation(&self, kind: &str) -> Result<TaskRecord, ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let record = TaskRecord {
            task_id: format!("task_{}", Uuid::new_v4().simple()),
            project_id: None,
            task_type: kind.into(),
            status: TaskStatus::Queued,
            phase: "queued".into(),
            result: None,
            error: None,
        };
        ensure_dir(&self.task_dir(&record.task_id))?;
        atomic_json(&self.task_dir(&record.task_id).join("task.json"), &record)?;
        self.registry
            .lock()
            .unwrap()
            .records
            .insert(record.task_id.clone(), record.clone());
        Ok(record)
    }
    pub(crate) fn finish_partial(
        &self,
        id: &str,
        result: Value,
        error: ApiError,
    ) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry
            .records
            .get_mut(id)
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task not found"))?;
        record.status = if error.code == "NODE_INSTALL_INTERRUPTED" {
            TaskStatus::Interrupted
        } else {
            TaskStatus::Failed
        };
        record.phase = if record.status == TaskStatus::Interrupted {
            "interrupted"
        } else {
            "failed"
        }
        .into();
        record.result = Some(result);
        record.error = Some(error);
        atomic_json(&self.task_dir(id).join("task.json"), record)
    }
    pub(crate) fn reconcile_operation(&self, id: &str, outcome: &str) -> Result<(), ApiError> {
        self.reconcile_operation_result(id, json!({"operationOutcome":outcome,"recovered":true}))
    }
    pub(crate) fn reconcile_operation_result(
        &self,
        id: &str,
        result: Value,
    ) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        if let Some(record) = registry.records.get_mut(id) {
            if record.status == TaskStatus::Interrupted {
                record.result = Some(result);
                atomic_json(&self.task_dir(id).join("task.json"), record)?;
            }
        }
        Ok(())
    }
    pub fn task(&self, id: &str) -> Result<TaskRecord, ApiError> {
        self.registry
            .lock()
            .unwrap()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))
    }
    /// Restores a build result independently of storage and maintenance tasks.
    /// Legacy mtime selection is explicitly uncertain about submission order.
    pub fn latest_build(&self, project_id: &str) -> BuildResultRecovery {
        let registry = self.registry.lock().unwrap();
        build_result::recover(
            self.paths.workspace_root(),
            project_id,
            &registry.records,
            &registry.legacy_build_times,
        )
    }
    pub fn logs(&self, id: &str, cursor: u64) -> Result<TaskLogs, ApiError> {
        let task = self.task(id)?;
        let _log_guard = self.log_lock.lock().unwrap();
        let dir = self.task_dir(id);
        let meta = read_optional::<LogMeta>(&dir.join("log.json"))?.unwrap_or_default();
        let head = read_optional_bytes(&dir.join("head.log"))?;
        let tail = read_optional_bytes(&dir.join("tail.log"))?;
        let head_end = head.len() as u64;
        let tail_start = meta.total.saturating_sub(tail.len() as u64);
        let mut cursor = cursor.min(meta.total);
        let start = cursor;
        let mut data = Vec::new();
        let mut gaps = vec![];
        if cursor < head_end {
            let end = (cursor as usize + LOG_PAGE).min(head.len());
            data.extend_from_slice(&head[cursor as usize..end]);
            cursor = end as u64;
        }
        if data.len() < LOG_PAGE && cursor < tail_start {
            gaps.push(LogGap {
                from: cursor,
                to: tail_start,
            });
            cursor = tail_start;
        }
        if data.len() < LOG_PAGE && cursor < meta.total {
            let start = (cursor - tail_start) as usize;
            let end = (start + LOG_PAGE - data.len()).min(tail.len());
            data.extend_from_slice(&tail[start..end]);
            cursor = tail_start + end as u64;
        }
        Ok(TaskLogs {
            task_id: id.into(),
            cursor: start,
            next_cursor: cursor,
            text: String::from_utf8_lossy(&data).into_owned(),
            truncated: tail_start > head_end,
            gaps,
            eof: task.status.is_final() && cursor >= meta.total,
        })
    }
    pub async fn info(&self, id: &str) -> Result<Value, ApiError> {
        let project = self.store.project(id)?;
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || build_info(&paths, &project))
            .await
            .map_err(|error| ApiError::new("BUILD_CHECK_FAILED", error.to_string()))?
    }
    pub(crate) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.git_stop.send_replace(true);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let mut issues = Vec::new();
        let runner = self.runner.lock().unwrap().take();
        if let Some(runner) = runner {
            if let Err(error) = runner.await {
                issues.push(crate::shutdown::ShutdownIssue::join("builds", error));
            }
        }
        let ids = self
            .registry
            .lock()
            .unwrap()
            .records
            .values()
            .filter(|record| !record.status.is_final())
            .map(|record| record.task_id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            if let Err(error) = self.finish(
                &id,
                Err(ApiError::new(
                    "BUILD_INTERRUPTED",
                    "Platform shut down before this task finished",
                )),
            ) {
                issues.push(crate::shutdown::ShutdownIssue::new(
                    "builds",
                    "SHUTDOWN_TASK_RECORD_FAILED",
                    error.to_string(),
                    true,
                    false,
                ));
            }
        }
        issues
    }
    pub(crate) fn append(&self, id: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let _guard = self.log_lock.lock().unwrap();
        append_log(&self.task_dir(id), bytes)
    }
    fn task_dir(&self, id: &str) -> PathBuf {
        self.paths.workspace_root().join(".platform/tasks").join(id)
    }
    pub(crate) fn phase(&self, id: &str, phase: &str) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry.records.get_mut(id).unwrap();
        record.status = TaskStatus::Running;
        record.phase = phase.into();
        atomic_json(&self.task_dir(id).join("task.json"), record)
    }
    pub(crate) fn finish(&self, id: &str, result: Result<Value, ApiError>) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry.records.get_mut(id).unwrap();
        match result {
            Ok(value) => {
                record.status = TaskStatus::Succeeded;
                record.phase = "completed".into();
                record.result = Some(value);
                record.error = None;
            }
            Err(error) => {
                record.status = if matches!(
                    error.code.as_str(),
                    "BUILD_INTERRUPTED"
                        | "SOURCE_ACQUISITION_INTERRUPTED"
                        | "PROJECT_GIT_INTERRUPTED"
                        | "NODE_INSTALL_INTERRUPTED"
                ) {
                    TaskStatus::Interrupted
                } else {
                    TaskStatus::Failed
                };
                record.error = Some(error);
            }
        }
        let Some(project_id) = record.project_id.clone() else {
            return atomic_json(&self.task_dir(id).join("task.json"), record);
        };
        let event = BuildEvent {
            project_id,
            task_id: id.into(),
            status: record.status,
        };
        let is_build = record.task_type == "build";
        if !matches!(record.task_type.as_str(), "storageScan" | "storageCleanup") {
            if let Err(error) = crate::maintenance::MaintenanceManager::invalidate_storage(
                self.paths.workspace_root(),
                &event.project_id,
            ) {
                eprintln!("Storage invalidation failed: {error}");
            }
        }
        atomic_json(&self.task_dir(id).join("task.json"), record)?;
        if is_build {
            registry.active.remove(&event.project_id);
        }
        registry.leases.remove(id);
        let _ = self.events.send(event);
        Ok(())
    }
    async fn run(&self, id: &str) {
        let project_id = self.task(id).unwrap().project_id.unwrap();
        let result = self.pipeline(id, &project_id).await;
        if let Err(error) = &result {
            let _ = self.append(
                id,
                format!("\n{}: {}\n", error.code, error.message).as_bytes(),
            );
        }
        if result.is_ok() {
            if let Ok(control) = self.store.control(&project_id) {
                let mut state = control.get();
                state.pending_build = false;
                state.pending_build_reason = None;
                if let Err(error) = control.set(state) {
                    let _ = self.append(
                        id,
                        format!("Maintenance build marker not cleared: {error}\n").as_bytes(),
                    );
                }
            }
        }
        if let Err(error) = self.finish(id, result) {
            eprintln!("Cannot persist build outcome: {error}");
        }
    }
    fn checkpoint(&self, start: Instant) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            Err(ApiError::new(
                "BUILD_INTERRUPTED",
                "Build stopped because the platform is exiting",
            ))
        } else if start.elapsed() > BUILD_TIMEOUT {
            Err(ApiError::new(
                "BUILD_TIMEOUT",
                "Build exceeded the internal 10 minute execution budget",
            ))
        } else {
            Ok(())
        }
    }
    async fn pipeline(&self, id: &str, project_id: &str) -> Result<Value, ApiError> {
        let start = Instant::now();
        self.phase(id, "environment")?;
        let project = self.store.project(project_id)?;
        let _operation = project.operation_gate.read().await;
        project.check()?;
        self.store.check_development_available(project_id)?;
        self.phase(id, "git-build-check")?;
        let git_guard =
            BuildGitGuard::capture(&project, &self.git_environment, self.git_stop.subscribe())
                .await?;
        self.phase(id, "environment")?;
        let tools = self.environment.resolve().await?;
        self.append(id,format!("Node {}: {}\nnpm {}: {}\nLimits: internal test parameters; timeout 600 seconds, global concurrency 1.\n",tools.node.version,tools.node.path.display(),tools.npm.version,tools.npm.path.display()).as_bytes())?;
        self.phase(id, "input-digest")?;
        let baseline = snapshot_input_digest(&project)?;
        let task_root = checked_relative(&project.root, &format!(".platform/builds/{id}"), true)?;
        ensure_dir(&task_root)?;
        let candidate = candidate::BuildCandidate::prepare(&project, &task_root, &baseline)?;
        let source = candidate.source();
        let package: Value = serde_json::from_slice(&fs::read(source.join("package.json"))?)?;
        let lock: Value = serde_json::from_slice(&fs::read(source.join("package-lock.json"))?)?;
        validate_lock(&package, &lock)?;
        let script = project.manifest.build["script"].as_str().unwrap();
        if !script
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_:-".contains(&c))
            || script.starts_with('-')
            || package["scripts"][script].as_str().is_none()
        {
            return Err(ApiError::new(
                "BUILD_SCRIPT_INVALID",
                "build.script must name an existing package.json script",
            ));
        }
        let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
        let old_digest = if output.exists() {
            Some(tree_digest(&output, false)?)
        } else {
            None
        };
        let previous = task_root.join("previous");
        if output.exists() {
            copy_tree(&output, &previous)?;
        }
        let result = self
            .build_and_publish(
                id,
                &project,
                &tools,
                &candidate,
                &git_guard,
                &task_root,
                &baseline,
                old_digest.clone(),
                start,
            )
            .await;
        if result.is_err() {
            let committed = read_optional::<BuildProvenance>(&provenance_path(
                self.paths.workspace_root(),
                project_id,
            ))?
            .is_some_and(|record| record.build_id == id);
            if committed {
                // An I/O error after the commit point cannot be described as a rollback.
                return result.map_err(|error| error.outcome("committed"));
            }
            // A custom script may violate --outDir. Preserve the published artifact even then.
            let now = if output.exists() {
                tree_digest(&output, false).ok()
            } else {
                None
            };
            if now != old_digest {
                if old_digest.is_some() {
                    let restore = task_root.join("restore");
                    copy_tree(&previous, &restore)?;
                    replace_directory(&restore, &output)?;
                } else if output.exists() {
                    check_no_symlinks(&output, false)?;
                    fs::remove_dir_all(&output)?;
                }
            }
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    async fn build_and_publish(
        &self,
        id: &str,
        project: &ProjectHandle,
        tools: &Toolchain,
        candidate: &candidate::BuildCandidate,
        git_guard: &BuildGitGuard,
        task_root: &Path,
        baseline: &str,
        old_digest: Option<String>,
        start: Instant,
    ) -> Result<Value, ApiError> {
        let source = candidate.source();
        self.phase(id, "dependencies")?;
        self.append(id, b"Installing exact locked dependencies in a private source candidate with npm ci. Existing project dependencies are not reused.\n")?;
        self.command(id, tools, source, &["ci", "--no-audit", "--no-fund"], start)
            .await?;
        ensure_unchanged(project, baseline)?;
        candidate.ensure_unchanged(&project.manifest, baseline)?;
        let installed_lock = checked_relative(source, "node_modules/.package-lock.json", true)?;
        if !installed_lock.is_file() {
            return Err(ApiError::new(
                "DEPENDENCIES_INVALID",
                "npm ci did not produce an installed dependency lockfile",
            ));
        }
        candidate.validate_installed_links()?;
        self.checkpoint(start)?;
        ensure_unchanged(project, baseline)?;
        self.phase(id, "build")?;
        let staging = task_root.join("dist");
        let script = project.manifest.build["script"].as_str().unwrap();
        self.command(
            id,
            tools,
            source,
            &[
                "run",
                script,
                "--",
                "--outDir",
                staging
                    .to_str()
                    .ok_or_else(|| ApiError::new("INVALID_PATH", "Build path must be UTF-8"))?,
            ],
            start,
        )
        .await?;
        self.checkpoint(start)?;
        self.phase(id, "git-build-check")?;
        git_guard
            .recheck(project, self.git_stop.subscribe())
            .await?;
        // No more awaits before publication: validate source, candidate, live
        // output and staging again after the potentially slow Git inspection.
        self.checkpoint(start)?;
        self.phase(id, "artifact-validation")?;
        let entry_relative = Path::new(&project.manifest.entry)
            .strip_prefix(&project.manifest.output_dir)
            .map_err(|_| {
                ApiError::new(
                    "INVALID_PROJECT_MANIFEST",
                    "Entry is outside output directory",
                )
            })?
            .to_str()
            .ok_or_else(|| ApiError::new("INVALID_PATH", "Entry must be UTF-8"))?
            .to_owned();
        validate_artifact(&staging, &entry_relative)?;
        let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
        let output_now = if output.exists() {
            Some(tree_digest(&output, false)?)
        } else {
            None
        };
        if output_now != old_digest {
            return Err(ApiError::new("BUILD_OUTPUT_CONTRACT_VIOLATED","Build script modified the current output directory instead of only writing to --outDir"));
        }
        ensure_unchanged(project, baseline)?;
        candidate.ensure_unchanged(&project.manifest, baseline)?;
        let artifact_digest = tree_digest(&staging, false)?;
        self.checkpoint(start)?;
        project.check()?;
        let provenance = BuildProvenance {
            schema_version: 1,
            build_id: id.into(),
            task_id: id.into(),
            completed_at: now_ms(),
            digest_rule_version: DIGEST_RULE,
            input_digest: baseline.into(),
            artifact_digest,
            node: tools.node.clone(),
            npm: tools.npm.clone(),
            sdk_version: sdk_version(source)?,
            entry_relative,
        };
        self.checkpoint(start)?;
        self.phase(id, "publish")?;
        // From the journal onward the filesystem transaction must reach a recoverable outcome.
        publish(&self.paths, project, &staging, &provenance, old_digest)?;
        let _ = self.append(
            id,
            format!(
                "Published build {}\nInput SHA-256: {}\nArtifact SHA-256: {}\n",
                id, baseline, provenance.artifact_digest
            )
            .as_bytes(),
        );
        Ok(
            json!({"buildId":id,"logPath":self.task_dir(id),"url":format!("{}/p/{}/",self.base_url,project.manifest.id),"node":tools.node,"npm":tools.npm,"digestRuleVersion":DIGEST_RULE,"inputDigest":baseline,"artifactDigest":provenance.artifact_digest,"sdkVersion":provenance.sdk_version,"completedAt":provenance.completed_at}),
        )
    }
    async fn command(
        &self,
        id: &str,
        tools: &Toolchain,
        cwd: &Path,
        args: &[&str],
        start: Instant,
    ) -> Result<(), ApiError> {
        self.checkpoint(start)?;
        let cache = self.paths.workspace_root().join(".platform/cache/npm");
        ensure_dir(&cache)?;
        // npm prunes logs on startup. Keep that cleanup inside this task;
        // shared cache/_logs and earlier tasks are preservation evidence.
        let npm_logs = self.task_dir(id).join("npm-logs");
        ensure_dir(&npm_logs)?;
        let mut command = Command::new(&tools.node.path);
        command
            .arg(&tools.npm.path)
            // CLI options override inherited npm prefix/global settings, so ci
            // cannot select the original project as its installation target.
            .arg("--prefix")
            .arg(cwd)
            .arg("--global=false")
            // CLI configuration wins over inherited environment/.npmrc values.
            .arg("--logs-dir")
            .arg(&npm_logs)
            .args(args)
            .current_dir(cwd)
            .env("PATH", &tools.path)
            .env("npm_config_cache", cache)
            .env("npm_config_update_notifier", "false")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        candidate::isolate_git_environment(&mut command, cwd)?;
        let mut child = command
            .spawn()
            .map_err(|error| ApiError::new("BUILD_PROCESS_FAILED", error.to_string()))?;
        let pid = child.id().unwrap() as i32;
        let mut group = candidate::BuildProcessGroup(Some(pid));
        let log = self.task_dir(id);
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out = tokio::spawn(read_output(stdout, log.clone(), self.log_lock.clone()));
        let err = tokio::spawn(read_output(stderr, log, self.log_lock.clone()));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => (),
                Err(error) => break Err(ApiError::new("BUILD_PROCESS_FAILED", error.to_string())),
            }
            if let Err(error) = self.checkpoint(start) {
                break Err(error);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // A shell can exit while leaving grandchildren alive. Always close the entire group.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        if status.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = child.wait().await;
        } else {
            tokio::time::sleep(Duration::from_millis(20)).await;
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        group.0 = None;
        let mut reader_error = None;
        for mut reader in [out, err] {
            match tokio::time::timeout(Duration::from_secs(2), &mut reader).await {
                Ok(Ok(Err(error))) => {
                    reader_error.get_or_insert(error);
                }
                Ok(Ok(Ok(()))) => (),
                Ok(Err(error)) => {
                    reader_error.get_or_insert_with(|| {
                        ApiError::new("BUILD_LOG_FAILED", error.to_string())
                    });
                }
                Err(_) => {
                    reader.abort();
                    let _ = reader.await;
                    reader_error.get_or_insert_with(|| {
                        ApiError::new(
                            "BUILD_PROCESS_FAILED",
                            "Build descendants kept their output streams open",
                        )
                    });
                }
            }
        }
        if let Some(error) = reader_error {
            return Err(error);
        }
        let status = status?;
        if !status.success() {
            return Err(ApiError::new(
                "BUILD_FAILED",
                format!("npm {} exited with {status}", args.first().unwrap_or(&"")),
            ));
        }
        Ok(())
    }
}
async fn read_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    log: PathBuf,
    log_lock: Arc<Mutex<()>>,
) -> Result<(), ApiError> {
    let mut bytes = [0u8; 8192];
    loop {
        let n = reader.read(&mut bytes).await?;
        if n == 0 {
            return Ok(());
        }
        let _guard = log_lock.lock().unwrap();
        append_log(&log, &bytes[..n])?;
    }
}

pub(crate) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::new("INVALID_PATH", "Missing parent directory"))?;
    ensure_dir(parent)?;
    let temp = parent.join(format!(".write-{}", Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn ensure_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn read_optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, ApiError> {
    check_no_symlinks(path, true)?;
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn read_optional_bytes(path: &Path) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, true)?;
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error.into()),
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn valid_task_id(id: &str) -> bool {
    id.len() == 37
        && id.starts_with("task_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn append_log(dir: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let mut meta = read_optional::<LogMeta>(&dir.join("log.json"))?.unwrap_or_default();
    let mut head = read_optional_bytes(&dir.join("head.log"))?;
    let mut tail = read_optional_bytes(&dir.join("tail.log"))?;
    let take = (LOG_HEAD - head.len()).min(bytes.len());
    head.extend_from_slice(&bytes[..take]);
    tail.extend_from_slice(&bytes[take..]);
    if tail.len() > LOG_TAIL {
        tail.drain(..tail.len() - LOG_TAIL);
    }
    meta.total += bytes.len() as u64;
    fs::write(dir.join("head.log"), head)?;
    fs::write(dir.join("tail.log"), tail)?;
    atomic_json(&dir.join("log.json"), &meta)
}
fn excluded(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | ".git" | ".hg" | ".svn" | ".vite" | ".cache"
    )
}
fn collect_files(
    root: &Path,
    relative: &Path,
    source: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), ApiError> {
    let dir = root.join(relative);
    check_no_symlinks(&dir, false)?;
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            ApiError::new("BUILD_INPUT_INVALID", "Non UTF-8 filenames are unsupported")
        })?;
        if source && excluded(name) {
            continue;
        }
        let path = relative.join(name);
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(ApiError::new(
                "BUILD_INPUT_UNSAFE",
                format!(
                    "Symbolic links are not allowed in build inputs or artifacts: {}",
                    path.display()
                ),
            ));
        }
        if ty.is_dir() {
            collect_files(root, &path, source, files)?;
        } else if ty.is_file() {
            files.push(path);
        } else {
            return Err(ApiError::new(
                "BUILD_INPUT_UNSAFE",
                "Only ordinary files and directories are supported",
            ));
        }
    }
    Ok(())
}
fn hash_tree(root: &Path, source: bool, hasher: &mut Sha256) -> Result<(), ApiError> {
    let mut files = vec![];
    collect_files(root, Path::new(""), source, &mut files)?;
    for path in &files {
        let label = path.to_str().unwrap();
        hasher.update((label.len() as u64).to_le_bytes());
        hasher.update(label.as_bytes());
        let full = root.join(path);
        let before = fs::symlink_metadata(&full)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&full)?;
        hasher.update(before.len().to_le_bytes());
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let after = fs::symlink_metadata(full)?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
        {
            return Err(ApiError::new(
                "BUILD_INPUT_CHANGED",
                "A file changed while the input was being scanned",
            ));
        }
    }
    let mut after = vec![];
    collect_files(root, Path::new(""), source, &mut after)?;
    if files != after {
        return Err(ApiError::new(
            "BUILD_INPUT_CHANGED",
            "The file set changed while being scanned",
        ));
    }
    Ok(())
}
pub(crate) fn tree_digest(root: &Path, source: bool) -> Result<String, ApiError> {
    let mut hasher = Sha256::new();
    hash_tree(root, source, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}
pub fn input_digest(project: &ProjectHandle) -> Result<String, ApiError> {
    let manifest = read_manifest(project)?;
    digest_manifest(project, &manifest)
}
fn build_config(manifest: &ProjectManifest) -> Value {
    json!({"manifestVersion":manifest.manifest_version,"sdkApiVersion":manifest.sdk_api_version,"sourceDir":manifest.source_dir,"outputDir":manifest.output_dir,"entry":manifest.entry,"build":manifest.build})
}
fn read_manifest(project: &ProjectHandle) -> Result<ProjectManifest, ApiError> {
    Ok(serde_json::from_slice(&fs::read(checked_relative(
        &project.root,
        "project.json",
        false,
    )?)?)?)
}
fn snapshot_input_digest(project: &ProjectHandle) -> Result<String, ApiError> {
    // Environment detection is asynchronous. Never pair a newer manifest digest with
    // the source path and command selected from an earlier ProjectHandle snapshot.
    if build_config(&read_manifest(project)?) != build_config(&project.manifest) {
        return Err(ApiError::new(
            "BUILD_INPUT_CHANGED",
            "Build configuration changed while the environment was being prepared",
        ));
    }
    digest_manifest(project, &project.manifest)
}
fn digest_manifest(
    project: &ProjectHandle,
    manifest: &ProjectManifest,
) -> Result<String, ApiError> {
    let config = build_config(manifest);
    let digest = digest_project_inputs(&project.root, manifest)?;
    if build_config(&read_manifest(project)?) != config {
        return Err(ApiError::new(
            "BUILD_INPUT_CHANGED",
            "Build configuration changed during the input scan",
        ));
    }
    Ok(digest)
}
pub(crate) fn digest_project_inputs(
    root: &Path,
    manifest: &ProjectManifest,
) -> Result<String, ApiError> {
    let mut digest = Sha256::new();
    digest.update(b"local-projects-input-v1\0");
    digest.update(serde_json::to_vec(&build_config(manifest))?);
    let source = checked_relative(root, &manifest.source_dir, false)?;
    hash_tree(&source, true, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}
fn ensure_unchanged(project: &ProjectHandle, baseline: &str) -> Result<(), ApiError> {
    if snapshot_input_digest(project)? != baseline {
        Err(ApiError::new("BUILD_INPUT_CHANGED","Build inputs changed after the initial scan; the previous webpage remains active. Stop editing and submit a new build explicitly."))
    } else {
        Ok(())
    }
}
fn validate_lock(package: &Value, lock: &Value) -> Result<(), ApiError> {
    if !matches!(lock["lockfileVersion"].as_u64(), Some(2 | 3)) {
        return Err(ApiError::new(
            "LOCKFILE_INVALID",
            "Use an npm package-lock.json with lockfileVersion 2 or 3",
        ));
    }
    let root = lock
        .pointer("/packages/")
        .ok_or_else(|| ApiError::new("LOCKFILE_INVALID", "Lockfile is missing its root package"))?;
    for key in ["dependencies", "devDependencies", "optionalDependencies"] {
        let empty = json!({});
        if package.get(key).unwrap_or(&empty) != root.get(key).unwrap_or(&empty) {
            return Err(ApiError::new("LOCKFILE_MISMATCH",format!("package.json {key} differ from package-lock.json; update the lockfile before building")));
        }
    }
    Ok(())
}
fn sdk_version(source: &Path) -> Result<Option<String>, ApiError> {
    let package: Value = serde_json::from_slice(&fs::read(source.join("package.json"))?)?;
    let lock: Value = serde_json::from_slice(&fs::read(source.join("package-lock.json"))?)?;
    for section in ["dependencies", "devDependencies"] {
        if let Some(deps) = package[section].as_object() {
            for name in deps.keys() {
                if name.ends_with("/sdk") || name == "@local-projects/sdk" {
                    if let Some(version) =
                        lock["packages"][format!("node_modules/{name}")]["version"].as_str()
                    {
                        return Ok(Some(version.into()));
                    }
                }
            }
        }
    }
    Ok(None)
}
fn copy_tree(source: &Path, target: &Path) -> Result<(), ApiError> {
    let mut files = vec![];
    collect_files(source, Path::new(""), false, &mut files)?;
    ensure_dir(target)?;
    for relative in files {
        let dest = target.join(&relative);
        ensure_dir(dest.parent().unwrap())?;
        check_no_symlinks(&dest, true)?;
        let mut input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source.join(relative))?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dest)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    sync_tree(target)?;
    Ok(())
}
fn sync_tree(root: &Path) -> Result<(), ApiError> {
    let mut files = vec![];
    collect_files(root, Path::new(""), false, &mut files)?;
    let mut directories = HashSet::new();
    directories.insert(root.to_path_buf());
    for relative in files {
        let path = root.join(relative);
        File::open(&path)?.sync_all()?;
        let mut parent = path.parent();
        while let Some(directory) = parent.filter(|directory| directory.starts_with(root)) {
            directories.insert(directory.to_path_buf());
            parent = directory.parent();
        }
    }
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}
pub(crate) fn validate_artifact(root: &Path, entry: &str) -> Result<(), ApiError> {
    let entry_path = checked_relative(root, entry, false)?;
    let is_html = |path: &Path| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            == "text/html"
    };
    if !entry_path.is_file() || !is_html(&entry_path) {
        return Err(ApiError::new(
            "BUILD_ARTIFACT_INVALID",
            "Build did not produce the configured HTML entry",
        ));
    }
    let mut files = vec![];
    collect_files(root, Path::new(""), false, &mut files)?;
    let html_refs = Regex::new(r#"(?i)(?:src|href)\s*=\s*["']([^"']+)["']"#).unwrap();
    let css_refs = Regex::new(r#"(?i)url\(\s*["']?([^)'"\s]+)"#).unwrap();
    let js_refs = Regex::new(r#"(?:from\s*|import\s*\(\s*)["'](\.[^"']+)["']"#).unwrap();
    let base_tag = Regex::new(r"(?i)<base(?:\s|>)").unwrap();
    for file in &files {
        let ext = file
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let html = is_html(file);
        if !html && !matches!(ext.as_str(), "css" | "js" | "mjs") {
            continue;
        }
        let text = fs::read_to_string(root.join(file)).map_err(|_| {
            ApiError::new(
                "BUILD_ARTIFACT_INVALID",
                "HTML/CSS/JavaScript artifacts must be UTF-8",
            )
        })?;
        if html && base_tag.is_match(&text) {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_INVALID",
                "Do not emit a base tag; the platform supplies the build generation base",
            ));
        }
        let regex = if html {
            &html_refs
        } else if ext == "css" {
            &css_refs
        } else {
            &js_refs
        };
        for capture in regex.captures_iter(&text) {
            let reference = &capture[1];
            if reference.starts_with('#')
                || reference.starts_with("data:")
                || reference.starts_with("blob:")
                || reference.starts_with("http:")
                || reference.starts_with("https:")
                || reference.starts_with("mailto:")
                || reference.starts_with("tel:")
            {
                continue;
            }
            if reference.starts_with('/') || reference.starts_with("//") {
                return Err(ApiError::new(
                    "BUILD_ARTIFACT_INVALID",
                    format!("Use relative resource paths, not {reference}"),
                ));
            }
            let reference = reference.split(['?', '#']).next().unwrap();
            if reference.is_empty() {
                continue;
            }
            let mut target = file.parent().unwrap_or(Path::new("")).to_path_buf();
            for part in Path::new(reference).components() {
                match part {
                    std::path::Component::CurDir => (),
                    std::path::Component::ParentDir => {
                        if !target.pop() {
                            return Err(ApiError::new(
                                "BUILD_ARTIFACT_INVALID",
                                "Resource escapes artifact root",
                            ));
                        }
                    }
                    std::path::Component::Normal(name) => target.push(name),
                    _ => {
                        return Err(ApiError::new(
                            "BUILD_ARTIFACT_INVALID",
                            "Invalid resource path",
                        ))
                    }
                }
            }
            if !root.join(&target).is_file() {
                return Err(ApiError::new(
                    "BUILD_ARTIFACT_INVALID",
                    format!(
                        "Artifact {} refers to missing local resource {}",
                        file.display(),
                        target.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}
fn provenance_path(workspace: &Path, id: &str) -> PathBuf {
    workspace
        .join(".platform/project-state/builds")
        .join(format!("{id}.json"))
}
fn journal_path(workspace: &Path, id: &str) -> PathBuf {
    workspace
        .join(".platform/project-state/builds")
        .join(format!("{id}.publication.json"))
}
fn workspace_for(project: &ProjectHandle) -> &Path {
    project.root.parent().unwrap().parent().unwrap()
}
pub(crate) fn install_starter(
    staging: &Path,
    manifest: &ProjectManifest,
    template: &Path,
) -> Result<(), ApiError> {
    let record_path = checked_relative(template, "starter.json", false)?;
    let distribution: StarterDistribution = serde_json::from_slice(&fs::read(record_path)?)?;
    let metadata: Value = serde_json::from_slice(&fs::read(template.join("template.json"))?)?;
    if distribution.schema_version != 1
        || distribution.origin != "bundledTemplate"
        || metadata["templateVersion"] != distribution.template_version
        || metadata["sdkVersion"] != distribution.sdk_version
        || distribution.entry_relative != "index.html"
        || manifest.entry != format!("{}/{}", manifest.output_dir, distribution.entry_relative)
    {
        return Err(ApiError::new(
            "INVALID_TEMPLATE",
            "Starter metadata does not match the bundled template",
        ));
    }
    let source = checked_relative(staging, &manifest.source_dir, false)?;
    let artifact = checked_relative(template, "starter", false)?;
    if tree_digest(&source, true)? != distribution.source_digest
        || tree_digest(&artifact, false)? != distribution.artifact_digest
    {
        return Err(ApiError::new(
            "INVALID_TEMPLATE",
            "Starter source or artifact does not match its packaging digest",
        ));
    }
    validate_artifact(&artifact, &distribution.entry_relative)?;
    let build_id = format!("starter_{}", Uuid::new_v4().simple());
    copy_tree(
        &artifact,
        &staging.join(format!(".platform/artifacts/{build_id}/dist")),
    )?;
    copy_tree(&artifact, &staging.join(&manifest.output_dir))?;
    let record = StarterProvenance {
        schema_version: 1,
        origin: "bundledTemplate".into(),
        build_id,
        installed_at: now_ms(),
        digest_rule_version: DIGEST_RULE,
        input_digest: digest_project_inputs(staging, manifest)?,
        distribution,
    };
    atomic_json(&staging.join(".platform/starter.json"), &record)?;
    Ok(())
}
#[cfg(test)]
pub(crate) fn write_test_starter(template: &Path) {
    let artifact = template.join("starter");
    fs::create_dir_all(artifact.join("assets")).unwrap();
    fs::write(artifact.join("index.html"), "<!doctype html><html><head><script src=\"./assets/starter.js\"></script></head><body>Initial application</body></html>").unwrap();
    fs::write(
        artifact.join("assets/starter.js"),
        "window.initialApplication = true;",
    )
    .unwrap();
    let metadata = json!({"templateVersion":"test-template-1","sdkVersion":"0.1.0"});
    atomic_json(&template.join("template.json"), &metadata).unwrap();
    let record = StarterDistribution {
        schema_version: 1,
        origin: "bundledTemplate".into(),
        template_version: "test-template-1".into(),
        sdk_version: "0.1.0".into(),
        source_digest: tree_digest(&template.join("app"), true).unwrap(),
        artifact_digest: tree_digest(&artifact, false).unwrap(),
        entry_relative: "index.html".into(),
        built_at: 123,
        node_version: "24.14.1".into(),
        npm_version: "11.11.0".into(),
    };
    atomic_json(&template.join("starter.json"), &record).unwrap();
}
fn starter_provenance(project: &ProjectHandle) -> Result<Option<StarterProvenance>, ApiError> {
    let path = checked_relative(&project.root, ".platform/starter.json", true)?;
    let record = read_optional::<StarterProvenance>(&path)?;
    if record.as_ref().is_some_and(|record| {
        record.schema_version != 1
            || record.origin != "bundledTemplate"
            || record.digest_rule_version != DIGEST_RULE
            || !valid_starter_id(&record.build_id)
    }) {
        return Err(ApiError::new(
            "INVALID_TEMPLATE",
            "Invalid installed starter provenance",
        ));
    }
    Ok(record)
}
fn valid_starter_id(id: &str) -> bool {
    id.len() == 40
        && id.starts_with("starter_")
        && id[8..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub fn artifact_generation(
    project: &ProjectHandle,
) -> Result<Option<ArtifactGeneration>, ApiError> {
    let record = read_optional::<BuildProvenance>(&provenance_path(
        workspace_for(project),
        &project.manifest.id,
    ))?;
    let (build_id, entry_relative) = if let Some(record) = record {
        (record.build_id, record.entry_relative)
    } else if let Some(record) =
        read_optional::<Value>(&project.root.join(".platform/restored-build.json"))?
    {
        (
            record["buildId"]
                .as_str()
                .ok_or_else(|| {
                    ApiError::new("BUILD_PROVENANCE_INVALID", "Missing restored generation")
                })?
                .into(),
            record["provenance"]["entryRelative"]
                .as_str()
                .ok_or_else(|| ApiError::new("BUILD_PROVENANCE_INVALID", "Missing restored entry"))?
                .into(),
        )
    } else if let Some(record) = starter_provenance(project)? {
        (record.build_id, record.distribution.entry_relative)
    } else {
        return Ok(None);
    };
    let root = version_root(project, &build_id)?;
    if !checked_relative(&root, &entry_relative, true)?.is_file() {
        return Err(ApiError::new(
            "BUILD_ARTIFACT_MISSING",
            "The published artifact generation is missing",
        ));
    }
    Ok(Some(ArtifactGeneration {
        build_id,
        root,
        entry_relative,
    }))
}
pub fn version_root(project: &ProjectHandle, build_id: &str) -> Result<PathBuf, ApiError> {
    if !valid_task_id(build_id)
        && !valid_starter_id(build_id)
        && !(build_id.len() == 41
            && build_id.starts_with("restored_")
            && build_id[9..].bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(ApiError::new(
            "INVALID_BUILD_ID",
            "Invalid artifact generation",
        ));
    }
    artifact_directory(
        &project.root,
        &format!(".platform/artifacts/{build_id}/dist"),
    )
}
fn artifact_directory(root: &Path, relative: &str) -> Result<PathBuf, ApiError> {
    // Preserve path/symlink rejection while distinguishing a missing registered
    // artifact from a project which has genuinely never had a published build.
    let path = checked_relative(root, relative, true)?;
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Err(ApiError::new(
            "BUILD_ARTIFACT_INVALID",
            "The published artifact path is no longer a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ApiError::new(
            "BUILD_ARTIFACT_MISSING",
            "A registered published artifact directory is missing",
        )),
        Err(error) => Err(error.into()),
    }
}
pub fn build_info(paths: &RuntimePaths, project: &ProjectHandle) -> Result<Value, ApiError> {
    let checked = now_ms();
    let record = read_optional::<BuildProvenance>(&provenance_path(
        paths.workspace_root(),
        &project.manifest.id,
    ))?;
    let Some(record) = record else {
        if let Some(restored) =
            read_optional::<Value>(&project.root.join(".platform/restored-build.json"))?
        {
            let provenance = &restored["provenance"];
            let id = restored["buildId"].as_str().ok_or_else(|| {
                ApiError::new("BUILD_PROVENANCE_INVALID", "Missing restored generation")
            })?;
            let generation = version_root(project, id)?;
            let output = artifact_directory(&project.root, &project.manifest.output_dir)?;
            for root in [&generation, &output] {
                if tree_digest(root, false)? != provenance["artifactDigest"].as_str().unwrap_or("")
                {
                    return Err(ApiError::new(
                        "BUILD_ARTIFACT_CHANGED",
                        "Restored artifact no longer matches its digest",
                    ));
                }
            }
            let (source_state, stale, reason) = match input_digest(project) {
                Ok(digest) => (
                    if Some(digest.as_str()) == provenance["inputDigest"].as_str() {
                        "matched"
                    } else {
                        "modified"
                    },
                    false,
                    "Restored portable source record".to_string(),
                ),
                Err(error) => ("unknown", true, error.message),
            };
            return Ok(
                json!({"sourceState":source_state,"checkedAt":checked,"stale":stale,"reason":reason,"origin":provenance["origin"],"templateVersion":provenance["templateVersion"],"provenance":provenance,"restored":true}),
            );
        }
        if let Some(starter) = starter_provenance(project)? {
            let generation = version_root(project, &starter.build_id)?;
            let output = artifact_directory(&project.root, &project.manifest.output_dir)?;
            for root in [&generation, &output] {
                if tree_digest(root, false)? != starter.distribution.artifact_digest {
                    return Err(ApiError::new(
                        "BUILD_ARTIFACT_CHANGED",
                        "The starter no longer matches its packaging digest",
                    ));
                }
            }
            let (source_state, stale, reason) = match input_digest(project) {
                Ok(digest) if digest == starter.input_digest => (
                    "matched",
                    false,
                    "Inputs match the bundled initial application".to_owned(),
                ),
                Ok(_) => (
                    "modified",
                    false,
                    "Source changed; the bundled initial application remains active".to_owned(),
                ),
                Err(error) => ("unknown", true, error.message),
            };
            return Ok(
                json!({"sourceState":source_state,"checkedAt":checked,"stale":stale,"reason":reason,"origin":"bundledTemplate","templateVersion":starter.distribution.template_version,"starter":starter}),
            );
        }
        return Ok(
            json!({"sourceState":if project.root.join(&project.manifest.entry).is_file(){"unknown"}else{"unbuilt"},"checkedAt":checked,"stale":false,"reason":"No platform build provenance is available"}),
        );
    };
    let generation = version_root(project, &record.build_id)?;
    let output = artifact_directory(&project.root, &project.manifest.output_dir)?;
    for root in [&generation, &output] {
        if tree_digest(root, false)? != record.artifact_digest {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_CHANGED",
                "The published artifact no longer matches its recorded digest",
            ));
        }
    }
    match input_digest(project) {
        Ok(digest) => Ok(
            json!({"sourceState":if digest==record.input_digest{"matched"}else{"modified"},"checkedAt":checked,"stale":false,"reason":if digest==record.input_digest{"Inputs match the current artifact"}else{"Source changed; the previous successful webpage remains active"},"origin":"projectBuild","provenance":record}),
        ),
        Err(error) => Ok(
            json!({"sourceState":"unknown","checkedAt":checked,"stale":true,"reason":error.message,"provenance":record}),
        ),
    }
}
fn publish(
    paths: &RuntimePaths,
    project: &ProjectHandle,
    staging: &Path,
    provenance: &BuildProvenance,
    previous_digest: Option<String>,
) -> Result<(), ApiError> {
    let generation = checked_relative(
        &project.root,
        &format!(".platform/artifacts/{}/dist", provenance.build_id),
        true,
    )?;
    ensure_dir(generation.parent().unwrap())?;
    sync_tree(staging)?;
    fs::rename(staging, &generation)?;
    File::open(generation.parent().unwrap())?.sync_all()?;
    let publication = staging.parent().unwrap().join("publication");
    copy_tree(&generation, &publication)?;
    let journal = Journal {
        project_id: project.manifest.id.clone(),
        provenance: provenance.clone(),
        previous_digest,
    };
    let journal_path = journal_path(paths.workspace_root(), &project.manifest.id);
    atomic_json(&journal_path, &journal)?;
    let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
    replace_directory(&publication, &output)?;
    atomic_json(
        &provenance_path(paths.workspace_root(), &project.manifest.id),
        provenance,
    )?;
    // Provenance is the commit point. Cleanup failure must not turn a committed build into failure.
    let _ = fs::remove_file(&journal_path);
    let _ = File::open(journal_path.parent().unwrap()).and_then(|file| file.sync_all());
    // Previous files are retained as runtime resources, not exposed as user version history.
    Ok(())
}
fn replace_directory(staged: &Path, current: &Path) -> Result<(), ApiError> {
    check_no_symlinks(staged, false)?;
    check_no_symlinks(current, true)?;
    if !current.exists() {
        ensure_dir(current.parent().unwrap())?;
        fs::rename(staged, current)?;
    } else {
        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let a = CString::new(staged.as_os_str().as_bytes())
                .map_err(|_| ApiError::new("INVALID_PATH", "NUL in artifact path"))?;
            let b = CString::new(current.as_os_str().as_bytes())
                .map_err(|_| ApiError::new("INVALID_PATH", "NUL in artifact path"))?;
            let code = unsafe {
                libc::renameatx_np(
                    libc::AT_FDCWD,
                    a.as_ptr(),
                    libc::AT_FDCWD,
                    b.as_ptr(),
                    libc::RENAME_SWAP,
                )
            };
            if code != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err(ApiError::new(
                "UNSUPPORTED_PLATFORM",
                "Atomic artifact exchange currently targets macOS",
            ));
        }
    }
    File::open(current.parent().unwrap())?.sync_all()?;
    File::open(staged.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn recover_publications(paths: &RuntimePaths, store: &ProjectStore) -> Result<(), ApiError> {
    let state = paths
        .workspace_root()
        .join(".platform/project-state/builds");
    for entry in fs::read_dir(&state)? {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|x| x.to_str())
            .is_some_and(|name| name.ends_with(".publication.json"))
        {
            continue;
        }
        check_no_symlinks(&path, false)?;
        let journal: Journal = serde_json::from_slice(&fs::read(&path)?)?;
        let project = store.project(&journal.project_id)?;
        let generation = version_root(&project, &journal.provenance.build_id)?;
        if tree_digest(&generation, false)? != journal.provenance.artifact_digest {
            return Err(ApiError::new(
                "BUILD_RECOVERY_REQUIRED",
                "Interrupted build generation is damaged; publication was not guessed",
            ));
        }
        let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
        let actual = if output.exists() {
            Some(tree_digest(&output, false)?)
        } else {
            None
        };
        if actual.as_ref() == Some(&journal.provenance.artifact_digest) {
            atomic_json(
                &provenance_path(paths.workspace_root(), &journal.project_id),
                &journal.provenance,
            )?;
        } else if actual != journal.previous_digest {
            return Err(ApiError::new("BUILD_RECOVERY_REQUIRED","Interrupted publication matches neither previous nor new artifact; retained files require inspection"));
        }
        fs::remove_file(&path)?;
        File::open(&state)?.sync_all()?;
    }
    Ok(())
}
fn reconcile_recovered_tasks(
    paths: &RuntimePaths,
    store: &ProjectStore,
    base_url: &str,
    records: &mut HashMap<String, TaskRecord>,
) -> Result<(), ApiError> {
    for record in records
        .values_mut()
        .filter(|record| record.status == TaskStatus::Interrupted && record.task_type == "build")
    {
        let Some(project_id) = record.project_id.as_ref() else {
            continue;
        };
        let Some(provenance) =
            read_optional::<BuildProvenance>(&provenance_path(paths.workspace_root(), project_id))?
        else {
            continue;
        };
        if provenance.task_id != record.task_id {
            continue;
        }
        let project = store.project(project_id)?;
        let generation = version_root(&project, &provenance.build_id)?;
        let output = checked_relative(&project.root, &project.manifest.output_dir, false)?;
        if tree_digest(&generation, false)? != provenance.artifact_digest
            || tree_digest(&output, false)? != provenance.artifact_digest
        {
            record.error = Some(ApiError::new("BUILD_RECOVERY_REQUIRED", "Interrupted task has publication provenance but the retained artifact no longer matches").outcome("unknown"));
        } else {
            record.error = Some(ApiError::new("BUILD_INTERRUPTED", "The platform exited after publication; the new artifact and its provenance were recovered, but the task did not finish reporting").outcome("committed"));
            record.result = Some(
                json!({"recovered":true,"operationOutcome":"committed","buildId":provenance.build_id,"logPath":paths.workspace_root().join(".platform/tasks").join(&record.task_id),"url":format!("{base_url}/p/{project_id}/"),"node":provenance.node,"npm":provenance.npm,"digestRuleVersion":provenance.digest_rule_version,"inputDigest":provenance.input_digest,"artifactDigest":provenance.artifact_digest,"sdkVersion":provenance.sdk_version,"completedAt":provenance.completed_at}),
            );
        }
        atomic_json(
            &paths
                .workspace_root()
                .join(".platform/tasks")
                .join(&record.task_id)
                .join("task.json"),
            record,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CreateProjectInput;
    async fn project(store: &ProjectStore, name: &str) -> ProjectHandle {
        let created = store
            .create(CreateProjectInput {
                name: name.into(),
                description: None,
            })
            .await
            .unwrap();
        store.project(&created.id).unwrap()
    }
    fn fixture(project: &ProjectHandle, script: &str) {
        let app = project.root.join("app");
        ensure_dir(&app.join("vendor/tiny")).unwrap();
        fs::write(
            app.join("vendor/tiny/package.json"),
            r#"{"name":"tiny","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::write(app.join("package.json"),r#"{"name":"build-test","version":"1.0.0","scripts":{"build":"node build.cjs"},"dependencies":{"tiny":"file:vendor/tiny"}}"#).unwrap();
        fs::write(app.join("package-lock.json"),r#"{"name":"build-test","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"build-test","version":"1.0.0","dependencies":{"tiny":"file:vendor/tiny"}},"node_modules/tiny":{"resolved":"vendor/tiny","link":true},"vendor/tiny":{"name":"tiny","version":"1.0.0"}}}"#).unwrap();
        fs::write(app.join("build.cjs"), script).unwrap();
    }
    const BUILD: &str = r#"const fs=require('fs');const p=require('path');const out=process.argv[process.argv.indexOf('--outDir')+1];fs.mkdirSync(out,{recursive:true});fs.writeFileSync(p.join(out,'index.html'),'<html><body>generation-one</body></html>');"#;
    mod candidate_pipeline {
        include!("build_pipeline_candidate_tests.rs");
    }
    mod git_pipeline {
        include!("build_pipeline_git_tests.rs");
    }
    mod result_recovery {
        include!("build_result_tests.rs");
    }

    async fn wait(manager: &BuildManager, id: &str) -> TaskRecord {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let task = manager.task(id).unwrap();
                if task.status.is_final() {
                    return task;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn input_digest_ignores_display_metadata_and_data_but_includes_source_and_build_config() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap();
        let project = project(&store, "Digest").await;
        fixture(&project, BUILD);
        let initial = input_digest(&project).unwrap();
        let path = project.root.join("project.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["name"] = json!("Renamed");
        manifest["description"] = json!("Changed");
        atomic_json(&path, &manifest).unwrap();
        fs::write(project.root.join("README.md"), "new README").unwrap();
        fs::write(
            project.root.join("data/attachments/example.txt"),
            "new attachment",
        )
        .unwrap();
        assert_eq!(initial, input_digest(&project).unwrap());
        fs::write(project.root.join("app/src/new.ts"), "export const n=1").unwrap();
        assert_ne!(initial, input_digest(&project).unwrap());
        fs::remove_file(project.root.join("app/src/new.ts")).unwrap();
        manifest["build"]["script"] = json!("custom");
        atomic_json(&path, &manifest).unwrap();
        assert_ne!(initial, input_digest(&project).unwrap());
    }
    #[tokio::test]
    async fn manifest_change_during_environment_probe_cannot_relabel_old_build_inputs() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap();
        let original = project(&store, "Manifest race").await;
        fixture(&original, BUILD);
        let original_digest = snapshot_input_digest(&original).unwrap();
        let manifest_path = original.root.join("project.json");
        let mut changed = original.manifest.clone();
        changed.name = "Metadata is harmless".into();
        atomic_json(&manifest_path, &changed).unwrap();
        assert_eq!(original_digest, snapshot_input_digest(&original).unwrap());
        // This is the state after the async environment probe returns to a pipeline
        // holding the original ProjectHandle, while an editor changed build config.
        changed.build = json!({"script":"another-script"});
        atomic_json(&manifest_path, &changed).unwrap();
        assert_eq!(
            snapshot_input_digest(&original).unwrap_err().code,
            "BUILD_INPUT_CHANGED"
        );
        changed.build = original.manifest.build.clone();
        changed.source_dir = "another-source".into();
        atomic_json(&manifest_path, &changed).unwrap();
        assert_eq!(
            snapshot_input_digest(&original).unwrap_err().code,
            "BUILD_INPUT_CHANGED"
        );
    }
    #[tokio::test]
    async fn build_success_failure_input_mutation_and_output_violation_preserve_published_page() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let project = project(&store, "Pipeline").await;
        fixture(&project, BUILD);
        let manager =
            BuildManager::new(paths.clone(), store, "http://127.0.0.1:1234".into()).unwrap();
        let first = manager.submit(&project.manifest.id).await.unwrap();
        let first = wait(&manager, &first.task_id).await;
        assert_eq!(first.status, TaskStatus::Succeeded, "{:?}", first.error);
        let previous = fs::read(project.root.join("dist/index.html")).unwrap();
        let provenance = fs::read(provenance_path(
            paths.workspace_root(),
            &project.manifest.id,
        ))
        .unwrap();
        assert_eq!(
            manager.info(&project.manifest.id).await.unwrap()["sourceState"],
            "matched"
        );
        let output_violation = format!(
            "const fs=require('fs');fs.writeFileSync({},'bad');",
            serde_json::to_string(&project.root.join("dist/index.html")).unwrap()
        );
        let cases = [
            ("process.exit(2);", "BUILD_FAILED"),
            (
                "const fs=require('fs');fs.appendFileSync('build.cjs','\\n// changed');",
                "BUILD_INPUT_CHANGED",
            ),
            (output_violation.as_str(), "BUILD_OUTPUT_CONTRACT_VIOLATED"),
        ];
        for (extra, code) in cases {
            fs::write(
                project.root.join("app/build.cjs"),
                format!("{BUILD}\n{{{extra}}}"),
            )
            .unwrap();
            let task = manager.submit(&project.manifest.id).await.unwrap();
            let done = wait(&manager, &task.task_id).await;
            assert_eq!(done.status, TaskStatus::Failed, "{:?}", done);
            assert_eq!(done.error.unwrap().code, code);
            assert_eq!(
                fs::read(project.root.join("dist/index.html")).unwrap(),
                previous
            );
            assert_eq!(
                fs::read(provenance_path(
                    paths.workspace_root(),
                    &project.manifest.id
                ))
                .unwrap(),
                provenance
            );
        }
        assert_eq!(
            manager.info(&project.manifest.id).await.unwrap()["sourceState"],
            "modified"
        );
        manager.shutdown().await;
    }
    #[tokio::test]
    async fn first_build_failure_preserves_starter_and_success_retains_its_generation() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let project = project(&store, "Starter lifecycle").await;
        fixture(&project, BUILD);
        let template = paths.data_root().join("starter-template");
        copy_tree(&project.root.join("app"), &template.join("app")).unwrap();
        write_test_starter(&template);
        install_starter(&project.root, &project.manifest, &template).unwrap();
        let first = artifact_generation(&project).unwrap().unwrap();
        let old_html = fs::read(first.root.join("index.html")).unwrap();
        let old_asset = fs::read(first.root.join("assets/starter.js")).unwrap();
        let connection = rusqlite::Connection::open(project.root.join("data/main.sqlite")).unwrap();
        connection
            .execute_batch("CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('keep me');")
            .unwrap();
        drop(connection);
        let manager =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
                .unwrap();
        assert_eq!(
            manager.info(&project.manifest.id).await.unwrap()["sourceState"],
            "matched"
        );
        assert!(manager.registry.lock().unwrap().records.is_empty());
        fs::write(project.root.join("app/build.cjs"), "process.exit(42)").unwrap();
        let failed = manager.submit(&project.manifest.id).await.unwrap();
        let failed = wait(&manager, &failed.task_id).await;
        assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
        assert_eq!(failed.error.unwrap().code, "BUILD_FAILED");
        assert_eq!(
            fs::read(project.root.join("dist/index.html")).unwrap(),
            old_html
        );
        assert_eq!(
            artifact_generation(&project).unwrap().unwrap().build_id,
            first.build_id
        );
        let info = manager.info(&project.manifest.id).await.unwrap();
        assert_eq!(info["sourceState"], "modified");
        assert_eq!(info["origin"], "bundledTemplate");
        fs::write(project.root.join("app/build.cjs"), BUILD).unwrap();
        let built = manager.submit(&project.manifest.id).await.unwrap();
        let built = wait(&manager, &built.task_id).await;
        assert_eq!(built.status, TaskStatus::Succeeded, "{built:?}");
        assert_eq!(
            artifact_generation(&project).unwrap().unwrap().build_id,
            built.task_id
        );
        assert_eq!(
            manager.info(&project.manifest.id).await.unwrap()["origin"],
            "projectBuild"
        );
        let old_root = version_root(&project, &first.build_id).unwrap();
        assert_eq!(fs::read(old_root.join("index.html")).unwrap(), old_html);
        assert_eq!(
            fs::read(old_root.join("assets/starter.js")).unwrap(),
            old_asset
        );
        let connection = rusqlite::Connection::open(project.root.join("data/main.sqlite")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT text FROM notes", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "keep me"
        );
        manager.shutdown().await;
        store.shutdown().await;
    }
    #[tokio::test]
    async fn missing_registered_starter_directories_are_project_errors() {
        for remove_generation in [false, true] {
            let (_temp, paths) = RuntimePaths::for_test();
            let lock = paths.acquire_lock().unwrap();
            let service = crate::RuntimeService::start(paths.clone(), lock)
                .await
                .unwrap();
            let project = project(&service.store(), "Missing starter").await;
            fixture(&project, BUILD);
            let template = paths.data_root().join("starter-template");
            copy_tree(&project.root.join("app"), &template.join("app")).unwrap();
            write_test_starter(&template);
            install_starter(&project.root, &project.manifest, &template).unwrap();
            let generation = artifact_generation(&project).unwrap().unwrap();
            assert_eq!(
                service
                    .handle()
                    .project(&project.manifest.id)
                    .await
                    .unwrap()["state"],
                "ready"
            );
            let removed = if remove_generation {
                generation.root
            } else {
                project.root.join("dist")
            };
            fs::remove_dir_all(&removed).unwrap();
            assert_eq!(
                build_info(&paths, &project).unwrap_err().code,
                "BUILD_ARTIFACT_MISSING"
            );
            let info = service
                .handle()
                .project(&project.manifest.id)
                .await
                .unwrap();
            assert_eq!(info["state"], "error", "{info}");
            assert_eq!(info["buildInfo"]["sourceState"], "unknown");
            assert!(!info["issues"].as_array().unwrap().is_empty());
            service.shutdown().await.unwrap();
        }
    }
    #[tokio::test]
    async fn missing_registered_build_directories_are_not_unbuilt_and_symlinks_remain_rejected() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let project = project(&store, "Missing build").await;
        fixture(&project, BUILD);
        let manager =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
                .unwrap();
        let task = manager.submit(&project.manifest.id).await.unwrap();
        let task = wait(&manager, &task.task_id).await;
        assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
        let generation = artifact_generation(&project).unwrap().unwrap();
        for directory in [project.root.join("dist"), generation.root] {
            let backup = paths.data_root().join("artifact-backup");
            fs::rename(&directory, &backup).unwrap();
            assert_eq!(
                build_info(&paths, &project).unwrap_err().code,
                "BUILD_ARTIFACT_MISSING"
            );
            std::os::unix::fs::symlink(&backup, &directory).unwrap();
            assert_eq!(
                build_info(&paths, &project).unwrap_err().code,
                "UNSAFE_PATH"
            );
            fs::remove_file(&directory).unwrap();
            fs::rename(&backup, &directory).unwrap();
        }
        assert_eq!(
            manager.info(&project.manifest.id).await.unwrap()["sourceState"],
            "matched"
        );
        manager.shutdown().await;
        store.shutdown().await;
    }
    #[tokio::test]
    async fn duplicate_builds_and_shutdown_are_explicit_and_persisted() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let project = project(&store, "Cancel").await;
        let ticks_path = project.root.join(".platform/child-ticks");
        let child_code = format!(
            "const fs=require('fs');setInterval(()=>fs.appendFileSync({},'x'),20)",
            serde_json::to_string(&ticks_path).unwrap()
        );
        let script = format!(
            "const cp=require('child_process');cp.spawn(process.execPath,['-e',{}],{{stdio:'ignore'}});setInterval(()=>{{}},1000);",
            serde_json::to_string(&child_code).unwrap()
        );
        fixture(&project, &script);
        let manager =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
                .unwrap();
        let task = manager.submit(&project.manifest.id).await.unwrap();
        let error = manager.submit(&project.manifest.id).await.unwrap_err();
        assert_eq!(error.code, "PROJECT_BUSY");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !project.root.join(".platform/child-ticks").is_file() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        let ticks = fs::read(project.root.join(".platform/child-ticks")).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            ticks,
            fs::read(project.root.join(".platform/child-ticks")).unwrap(),
            "A descendant kept running after shutdown"
        );
        assert_eq!(
            manager.task(&task.task_id).unwrap().status,
            TaskStatus::Interrupted
        );
        let restarted = BuildManager::new(paths, store, "http://127.0.0.1:1234".into()).unwrap();
        assert_eq!(
            restarted.task(&task.task_id).unwrap().status,
            TaskStatus::Interrupted
        );
        restarted.shutdown().await;
    }
    #[tokio::test]
    async fn publication_recovery_never_attaches_new_provenance_to_old_files() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let project = project(&store, "Recovery").await;
        fixture(&project, BUILD);
        let manager =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
                .unwrap();
        let first = manager.submit(&project.manifest.id).await.unwrap();
        assert_eq!(
            wait(&manager, &first.task_id).await.status,
            TaskStatus::Succeeded
        );
        manager.shutdown().await;
        let old: BuildProvenance = read_optional(&provenance_path(
            paths.workspace_root(),
            &project.manifest.id,
        ))
        .unwrap()
        .unwrap();
        let next_id = format!("task_{}", Uuid::new_v4().simple());
        let generation = project
            .root
            .join(format!(".platform/artifacts/{next_id}/dist"));
        ensure_dir(&generation).unwrap();
        fs::write(generation.join("index.html"), "<html>second</html>").unwrap();
        let mut next = old.clone();
        next.build_id = next_id.clone();
        next.task_id = next_id;
        next.artifact_digest = tree_digest(&generation, false).unwrap();
        let journal = Journal {
            project_id: project.manifest.id.clone(),
            provenance: next.clone(),
            previous_digest: Some(old.artifact_digest.clone()),
        };
        let path = journal_path(paths.workspace_root(), &project.manifest.id);
        atomic_json(&path, &journal).unwrap();
        recover_publications(&paths, &store).unwrap();
        let unchanged: BuildProvenance = read_optional(&provenance_path(
            paths.workspace_root(),
            &project.manifest.id,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(unchanged.build_id, old.build_id);
        atomic_json(&path, &journal).unwrap();
        let publication = project.root.join(".platform/simulated-publication");
        copy_tree(&generation, &publication).unwrap();
        replace_directory(&publication, &project.root.join("dist")).unwrap();
        let pending = TaskRecord {
            task_id: next.task_id.clone(),
            project_id: Some(project.manifest.id.clone()),
            task_type: "build".into(),
            status: TaskStatus::Running,
            phase: "publish".into(),
            result: None,
            error: None,
        };
        atomic_json(
            &paths
                .workspace_root()
                .join(".platform/tasks")
                .join(&next.task_id)
                .join("task.json"),
            &pending,
        )
        .unwrap();
        let restarted =
            BuildManager::new(paths.clone(), store, "http://127.0.0.1:1234".into()).unwrap();
        let recovered: BuildProvenance = read_optional(&provenance_path(
            paths.workspace_root(),
            &project.manifest.id,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(recovered.build_id, next.build_id);
        assert_eq!(
            tree_digest(&project.root.join("dist"), false).unwrap(),
            recovered.artifact_digest
        );
        let task = restarted.task(&next.task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Interrupted);
        assert_eq!(task.error.unwrap().details["operationOutcome"], "committed");
        assert_eq!(task.result.unwrap()["recovered"], true);
        restarted.shutdown().await;
    }
    #[test]
    fn artifact_validator_rejects_missing_resources_and_paths_outside_build() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(
            root.join("index.html"),
            "<script src='./missing.js'></script>",
        )
        .unwrap();
        assert!(validate_artifact(&root, "index.html").is_err());
        fs::write(root.join("index.html"), "<img src='../outside.png'>").unwrap();
        assert!(validate_artifact(&root, "index.html").is_err());
        fs::write(root.join("index.html"), "<script src='./main.js'></script>").unwrap();
        fs::write(root.join("main.js"), "console.log('okay')").unwrap();
        validate_artifact(&root, "index.html").unwrap();
    }
    #[test]
    fn artifact_validation_covers_every_served_html_extension() {
        for entry in ["index.HTML", "index.htm", "index.HTM"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fs::write(root.join(entry), "<script src='./missing.js'></script>").unwrap();
            assert_eq!(
                validate_artifact(&root, entry).unwrap_err().code,
                "BUILD_ARTIFACT_INVALID"
            );
            fs::write(root.join("missing.js"), "console.log('present')").unwrap();
            validate_artifact(&root, entry).unwrap();
            fs::write(root.join(entry), "<BASE\nhref='./'>").unwrap();
            assert!(validate_artifact(&root, entry).is_err());
        }
    }
    #[test]
    fn logs_keep_head_and_tail_and_expose_cursor_gaps() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        append_log(&path, &vec![b'a'; LOG_HEAD]).unwrap();
        append_log(&path, &vec![b'b'; LOG_TAIL + 128]).unwrap();
        let meta: LogMeta = read_optional(&path.join("log.json")).unwrap().unwrap();
        assert_eq!(meta.total, (LOG_HEAD + LOG_TAIL + 128) as u64);
        assert_eq!(
            fs::metadata(path.join("head.log")).unwrap().len(),
            LOG_HEAD as u64
        );
        assert_eq!(
            fs::metadata(path.join("tail.log")).unwrap().len(),
            LOG_TAIL as u64
        );
    }
    #[test]
    fn lock_mismatch_is_not_hidden_by_dependency_reuse() {
        let package = json!({"dependencies":{"x":"2"}});
        let lock = json!({"lockfileVersion":3,"packages":{"":{"dependencies":{"x":"1"}}}});
        assert_eq!(
            validate_lock(&package, &lock).unwrap_err().code,
            "LOCKFILE_MISMATCH"
        );
    }

    #[tokio::test]
    async fn startup_interrupts_unfinished_records_without_restarting_commands() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let id = format!("task_{}", Uuid::new_v4().simple());
        let record = TaskRecord {
            task_id: id.clone(),
            project_id: Some("prj_stale".into()),
            task_type: "build".into(),
            status: TaskStatus::Running,
            phase: "dependencies".into(),
            result: None,
            error: None,
        };
        atomic_json(
            &paths
                .workspace_root()
                .join(".platform/tasks")
                .join(&id)
                .join("task.json"),
            &record,
        )
        .unwrap();
        let manager = BuildManager::new(paths, store, "http://127.0.0.1:1234".into()).unwrap();
        let recovered = manager.task(&id).unwrap();
        assert_eq!(recovered.status, TaskStatus::Interrupted);
        assert_eq!(recovered.phase, "dependencies");
        assert!(serde_json::to_value(&recovered).unwrap()["final"]
            .as_bool()
            .unwrap());
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn unavailable_node_only_blocks_building_existing_artifacts_stay_inspectable() {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let project = project(&store, "No Node").await;
        ensure_dir(&project.root.join("dist")).unwrap();
        fs::write(
            project.root.join("dist/index.html"),
            "<html>existing</html>",
        )
        .unwrap();
        atomic_json(
            &paths.data_root().join("toolchain.json"),
            &json!({"schemaVersion":1,"nodePath":paths.data_root().join("does-not-exist/node")}),
        )
        .unwrap();
        let manager = BuildManager::new(paths, store, "http://127.0.0.1:1234".into()).unwrap();
        assert!(!manager.environment().doctor().await.ready);
        assert_eq!(
            manager.submit(&project.manifest.id).await.unwrap_err().code,
            "NODE_NOT_FOUND"
        );
        assert_eq!(
            manager.info(&project.manifest.id).await.unwrap()["sourceState"],
            "unknown"
        );
        assert!(project.root.join("dist/index.html").is_file());
        manager.shutdown().await;
    }
}

#[cfg(test)]
#[path = "build_npm_logs_tests.rs"]
mod npm_logs_tests;
