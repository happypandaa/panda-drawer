//! Durable platform tasks and queued, in-place web builds.
use crate::{
    environment::EnvironmentManager,
    projects::check_no_symlinks,
    tasks::{TaskRecord, TaskStatus},
    web_build::{WebBuildContext, WebBuildRunner},
    ApiError, ProjectStore, RuntimePaths,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc, watch, OwnedRwLockReadGuard},
    task::JoinHandle,
};
use uuid::Uuid;
#[path = "build_result.rs"]
mod build_result;
#[path = "builds/startup.rs"]
mod startup;
pub use build_result::BuildResultRecovery;
const BUILD_QUEUE: usize = 8;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildEvent {
    pub project_id: String,
    pub task_id: String,
    pub status: TaskStatus,
}
struct Registry {
    records: HashMap<String, TaskRecord>,
    active: HashSet<String>,
    leases: HashMap<String, Arc<crate::projects::ProjectTaskLease>>,
}
struct BuildProtection {
    _lease: Arc<crate::projects::ProjectTaskLease>,
    _operation: OwnedRwLockReadGuard<()>,
}
pub struct BuildManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    environment: EnvironmentManager,
    stop: watch::Sender<bool>,
    registry: Mutex<Registry>,
    sender: mpsc::Sender<String>,
    runner: Mutex<Option<JoinHandle<()>>>,
    stopping: AtomicBool,
    events: broadcast::Sender<BuildEvent>,
}
impl BuildManager {
    pub fn new(paths: RuntimePaths, store: Arc<ProjectStore>) -> Result<Arc<Self>, ApiError> {
        Self::with_managed_node(paths, store, None)
    }
    pub fn with_managed_node(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        managed_node: Option<PathBuf>,
    ) -> Result<Arc<Self>, ApiError> {
        let tasks = paths.workspace_root().join(".platform/tasks");
        ensure_dir(&tasks)?;
        let records = startup::recover_startup(&paths)?;
        let (sender, mut receiver) = mpsc::channel::<String>(BUILD_QUEUE);
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            environment: EnvironmentManager::with_processes(
                paths.data_root().into(),
                managed_node,
                store.processes(),
            )
            .with_project_boundary(paths.workspace_root().into()),
            paths,
            store,
            stop: watch::channel(false).0,
            registry: Mutex::new(Registry {
                records,
                active: HashSet::new(),
                leases: HashMap::new(),
            }),
            sender,
            runner: Mutex::new(None),
            stopping: AtomicBool::new(false),
            events,
        });
        let weak = Arc::downgrade(&manager);
        let runner = tokio::spawn(async move {
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
        *manager.runner.lock().unwrap() = Some(runner);
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
        let lease = Arc::new(self.store.reserve_web_build(project_id, &task_id)?);
        self.store
            .control(project_id)?
            .check(false, maintenance_id, false)?;
        self.store.project(project_id)?.build_target()?;
        if self.registry.lock().unwrap().active.contains(project_id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "This project already has an unfinished build",
            ));
        }
        // Reject missing build environments before accepting a task; opening an existing entry is independent.
        self.environment
            .with_process_registry(self.store.project_processes(project_id)?)
            .resolve_with_keepalive(lease.clone())
            .await?;
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
        self.record_web_task(&record)?;
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
        let admission = build_result::remember(&self.paths, &record).and_then(|()| {
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
        self.record_web_task(&record)?;
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
        self.record_web_task(&record)?;
        ensure_dir(&self.task_dir(&record.task_id))?;
        atomic_json(&self.task_dir(&record.task_id).join("task.json"), &record)?;
        self.registry
            .lock()
            .unwrap()
            .records
            .insert(record.task_id.clone(), record.clone());
        Ok(record)
    }
    fn record_web_task(&self, record: &TaskRecord) -> Result<(), ApiError> {
        if let Some(id) = record
            .project_id
            .as_deref()
            .filter(|id| id.starts_with("prj_"))
        {
            let ownership = crate::web_project_ownership::WebProjectOwnership::new(
                self.paths.data_root().into(),
                self.paths.workspace_root().into(),
            );
            ownership.record_task(id, &record.task_id)?;
            fs::create_dir(self.task_dir(&record.task_id))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    self.task_dir(&record.task_id),
                    fs::Permissions::from_mode(0o700),
                )?;
            }
            let identity =
                crate::file_identity::PersistedIdentity::capture(&self.task_dir(&record.task_id))
                    .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))?;
            ownership.capture_task_directory(id, &record.task_id, &identity)?;
        }
        Ok(())
    }
    pub(crate) fn project_task_ids(&self, id: &str) -> Vec<String> {
        let mut ids = self
            .registry
            .lock()
            .unwrap()
            .records
            .values()
            .filter(|task| task.project_id.as_deref() == Some(id))
            .map(|task| task.task_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }
    pub(crate) fn storage_cache_task_ids(&self, project: &str) -> Vec<String> {
        let mut ids = self
            .registry
            .lock()
            .unwrap()
            .records
            .values()
            .filter(|task| {
                matches!(
                    task.task_type.as_str(),
                    "storageScan" | "storageWorkspaceScan" | "storageWorkspace"
                ) && (task.project_id.as_deref() == Some(project) || task.project_id.is_none())
            })
            .map(|task| task.task_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }
    pub(crate) fn forget_storage_cache_tasks(
        &self,
        project: &str,
        ids: &[String],
    ) -> Result<(), ApiError> {
        let selected = self
            .paths
            .selected_for_cleanup()
            .map_err(|error| ApiError::new("STORAGE_CACHE_UNAVAILABLE", error.to_string()))?;
        if selected.workspace_root() == self.paths.workspace_root() {
            return self.forget_owned_tasks(ids);
        }
        let ownership = crate::web_project_ownership::WebProjectOwnership::new(
            selected.data_root().into(),
            selected.workspace_root().into(),
        );
        let mut registry = self.registry.lock().unwrap();
        for id in ids {
            let Some(record) = registry.records.get(id) else {
                continue;
            };
            if !record.status.is_final()
                || !matches!(
                    record.task_type.as_str(),
                    "storageScan" | "storageWorkspaceScan" | "storageWorkspace"
                )
                || (record.project_id.is_some() && record.project_id.as_deref() != Some(project))
            {
                return Err(ApiError::new(
                    "STORAGE_SCAN_STOP_FAILED",
                    "空间统计任务尚未结束或归属已变化。",
                ));
            }
            let path = self.task_dir(id);
            check_no_symlinks(&path, true)?;
            if path.exists() {
                let expected = if project.starts_with("prj_") && record.project_id.is_some() {
                    ownership.material_identity(project, &path)?
                } else {
                    None
                }
                .ok_or_else(|| {
                    ApiError::new(
                        "STORAGE_CACHE_UNAVAILABLE",
                        "旧工作区统计任务缺少已登记的目录身份。",
                    )
                })?;
                crate::history_cleanup::remove_owned_directory(&path, &expected)?;
            }
            registry.records.remove(id);
        }
        Ok(())
    }
    pub(crate) fn restore_failed_project_task(
        &self,
        snapshot: &TaskRecord,
        error: ApiError,
    ) -> Result<(), ApiError> {
        let project = snapshot
            .project_id
            .as_deref()
            .filter(|id| id.starts_with("prj_"))
            .ok_or_else(|| ApiError::new("INVALID_PROJECT_ID", "删除回执缺少网页项目归属。"))?;
        let ownership = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let path = self.task_dir(&snapshot.task_id);
        check_no_symlinks(&path, true)?;
        let recorded = ownership.path(project)?.exists();
        if path.exists() {
            if !recorded {
                return Err(ApiError::new(
                    "WEB_PROJECT_OWNERSHIP_INVALID",
                    "已有任务目录缺少当期归属。",
                ));
            }
            ownership.verify_task_directory(project, &snapshot.task_id)?;
        } else {
            let previous = if recorded {
                ownership.task_directory_identity(project, &snapshot.task_id)?
            } else {
                None
            };
            fs::create_dir(&path)?;
            let identity = crate::file_identity::PersistedIdentity::capture(&path)
                .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))?;
            if recorded {
                ownership.rebind_recreated_task_directory(
                    project,
                    &snapshot.task_id,
                    previous.as_ref(),
                    &identity,
                )?;
            }
        }
        let mut failed = snapshot.clone();
        failed.status = TaskStatus::Failed;
        failed.phase = "failed".into();
        failed.error = Some(error);
        atomic_json(&path.join("task.json"), &failed)?;
        self.registry
            .lock()
            .unwrap()
            .records
            .insert(failed.task_id.clone(), failed);
        Ok(())
    }
    pub(crate) fn create_node_operation(
        &self,
        app_id: &str,
        kind: &str,
    ) -> Result<TaskRecord, ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let record = TaskRecord {
            task_id: format!("task_{}", Uuid::new_v4().simple()),
            project_id: None,
            task_type: kind.into(),
            status: TaskStatus::Queued,
            phase: "queued".into(),
            result: None,
            error: None,
        };
        ownership.record_task(app_id, &record.task_id)?;
        ensure_dir(&self.task_dir(&record.task_id))?;
        ownership.record_task(app_id, &record.task_id)?;
        atomic_json(&self.task_dir(&record.task_id).join("task.json"), &record)?;
        self.registry
            .lock()
            .unwrap()
            .records
            .insert(record.task_id.clone(), record.clone());
        Ok(record)
    }
    pub(crate) fn record_node_source(
        &self,
        task: &str,
        owner: &str,
        source: &str,
    ) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry
            .records
            .get_mut(task)
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))?;
        if !matches!(
            record.task_type.as_str(),
            "sourceAcquire" | "sourceCapture" | "nodeGitImport" | "gitImport"
        ) || record.status.is_final()
        {
            return Err(ApiError::new("SOURCE_IMPORT_INVALID", "来源任务身份无效。"));
        }
        record.result = Some(json!({"sourceId":source,"ownerAppId":owner}));
        atomic_json(&self.task_dir(task).join("task.json"), record)
    }
    pub(crate) fn forget_owned_tasks(&self, ids: &[String]) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        for id in ids {
            if !valid_task_id(id) {
                return Err(ApiError::new("INVALID_TASK_ID", "Invalid task ID"));
            }
            if registry
                .records
                .get(id)
                .is_some_and(|task| !task.status.is_final())
            {
                return Err(ApiError::new(
                    "NODE_PROJECT_TASK_ACTIVE",
                    "项目任务尚未结束，不能清除。",
                ));
            }
            let path = self.task_dir(id);
            crate::projects::check_no_symlinks(&path, true)?;
            if path.exists() {
                let web_project = registry
                    .records
                    .get(id)
                    .and_then(|record| record.project_id.as_deref())
                    .filter(|project| project.starts_with("prj_"));
                if let Some(project) = web_project {
                    let ownership = crate::web_project_ownership::WebProjectOwnership::new(
                        self.paths.data_root().into(),
                        self.paths.workspace_root().into(),
                    );
                    ownership.verify_task_directory(project, id)?;
                    let identity =
                        ownership
                            .task_directory_identity(project, id)?
                            .ok_or_else(|| {
                                ApiError::new(
                                    "WEB_PROJECT_OWNERSHIP_INVALID",
                                    "任务目录身份未捕获。",
                                )
                            })?;
                    crate::history_cleanup::remove_owned_directory(&path, &identity)?;
                } else {
                    let saved: TaskRecord =
                        serde_json::from_slice(&fs::read(path.join("task.json"))?)?;
                    if saved.task_id != *id || !saved.status.is_final() {
                        return Err(ApiError::new(
                            "NODE_PROJECT_TASK_CHANGED",
                            "项目任务身份或状态已变化。",
                        ));
                    }
                    fs::remove_dir_all(&path)?;
                }
                platform_native::filesystem::sync_directory(path.parent().unwrap())?;
            }
            registry.records.remove(id);
        }
        Ok(())
    }
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
    pub fn task(&self, id: &str) -> Result<TaskRecord, ApiError> {
        self.registry
            .lock()
            .unwrap()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))
    }
    pub(crate) fn operation_tasks(&self, kind: &str) -> Vec<TaskRecord> {
        let mut records: Vec<_> = self
            .registry
            .lock()
            .unwrap()
            .records
            .values()
            .filter(|record| record.task_type == kind)
            .cloned()
            .collect();
        records.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        records
    }
    pub(crate) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.stop.send_replace(true);
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
    pub(crate) fn task_dir(&self, id: &str) -> PathBuf {
        self.paths.workspace_root().join(".platform/tasks").join(id)
    }
    pub(crate) fn phase(&self, id: &str, phase: &str) -> Result<(), ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry
            .records
            .get_mut(id)
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))?;
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
                        | "NODE_INSTALL_INTERRUPTED"
                ) {
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
        if !matches!(
            record.task_type.as_str(),
            "delete" | "storageScan" | "storageCleanup"
        ) && self.store.registered_epoch(&event.project_id)?.is_some()
        {
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

    pub fn latest_build(&self, id: &str) -> BuildResultRecovery {
        let registry = self.registry.lock().unwrap();
        build_result::recover(&self.paths, id, &registry.records)
    }
    pub async fn info(&self, id: &str) -> Result<Value, ApiError> {
        let project = self.store.project(id)?;
        let manifest = &project.manifest;
        let available = manifest.resolve_web_entry(&project.root).is_ok();
        Ok(
            json!({"declared":manifest.development.is_some(),"script":manifest.development.as_ref().map(|development|{let crate::panda_manifest::BuildDecl::Npm{script}=&development.build;script}),"entryExists":available,"latest":self.latest_build(id)}),
        )
    }
    pub(crate) fn set_waiting_confirmation(&self, id: &str) -> Result<TaskRecord, ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry
            .records
            .get_mut(id)
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))?;
        if record.status.is_final() {
            return Err(ApiError::new("TASK_FINAL", "任务已经结束。"));
        }
        record.status = TaskStatus::WaitingConfirmation;
        record.phase = "awaiting-build-confirmation".into();
        atomic_json(&self.task_dir(id).join("task.json"), record)?;
        Ok(record.clone())
    }
    pub(crate) fn bind_web_operation(
        &self,
        task: &str,
        project: &str,
    ) -> Result<TaskRecord, ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry
            .records
            .get_mut(task)
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))?;
        if !project.starts_with("prj_")
            || !crate::projects::valid_id(project)
            || record.status.is_final()
            || record.project_id.as_deref().is_some_and(|id| id != project)
        {
            return Err(ApiError::new(
                "TASK_OWNER_INVALID",
                "任务不能绑定到此网页实例。",
            ));
        }
        let path = self.task_dir(task);
        check_no_symlinks(&path, false)?;
        let saved: TaskRecord = serde_json::from_slice(&fs::read(path.join("task.json"))?)?;
        if saved.task_id != task || saved.project_id != record.project_id {
            return Err(ApiError::new("TASK_OWNER_INVALID", "任务登记已变化。"));
        }
        let ownership = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        ownership.record_task(project, task)?;
        let identity = crate::file_identity::PersistedIdentity::capture(&path)
            .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))?;
        ownership.capture_task_directory(project, task, &identity)?;
        record.project_id = Some(project.into());
        atomic_json(&path.join("task.json"), record)?;
        Ok(record.clone())
    }
    pub(crate) fn release_web_operation(
        &self,
        task: &str,
        project: &str,
    ) -> Result<TaskRecord, ApiError> {
        let mut registry = self.registry.lock().unwrap();
        let record = registry
            .records
            .get_mut(task)
            .ok_or_else(|| ApiError::new("TASK_NOT_FOUND", "Task was not found"))?;
        if record.project_id.as_deref() != Some(project) {
            return Err(ApiError::new(
                "TASK_OWNER_INVALID",
                "任务的网页实例归属已变化。",
            ));
        }
        let path = self.task_dir(task);
        check_no_symlinks(&path, false)?;
        let saved: TaskRecord = serde_json::from_slice(&fs::read(path.join("task.json"))?)?;
        if saved.task_id != task || saved.project_id.as_deref() != Some(project) {
            return Err(ApiError::new("TASK_OWNER_INVALID", "任务登记已变化。"));
        }
        record.project_id = None;
        atomic_json(&path.join("task.json"), record)?;
        Ok(record.clone())
    }
    async fn run(self: &Arc<Self>, id: &str) {
        let result = self.pipeline(id).await;
        if let Err(error) = self.finish(id, result) {
            eprintln!("Cannot persist build outcome: {error}");
        }
    }
    async fn pipeline(self: &Arc<Self>, id: &str) -> Result<Value, ApiError> {
        let project_id = self
            .task(id)?
            .project_id
            .ok_or_else(|| ApiError::new("BUILD_OWNER_INVALID", "构建任务缺少项目归属。"))?;
        let project = self.store.project(&project_id)?;
        let target = project.build_target()?;
        let lease = self
            .registry
            .lock()
            .unwrap()
            .leases
            .get(id)
            .cloned()
            .ok_or_else(|| {
                ApiError::new(
                    "BUILD_TASK_LEASE_MISSING",
                    "Build task has no project lease",
                )
            })?;
        let protection = Arc::new(BuildProtection {
            _lease: lease,
            _operation: project.operation_gate.clone().read_owned().await,
        });
        project.check()?;
        let manager = self.clone();
        let task = id.to_owned();
        let (_cancel_guard, cancel) = watch::channel(false);
        let context = WebBuildContext {
            environment: self.environment.clone(),
            operation_root: project.private_dir.join("builds").join(id),
            cache_root: project.private_dir.join("npm-cache"),
            processes: self.store.project_processes(&project_id)?,
            owner: protection,
            stop: self.stop.subscribe(),
            cancel,
            progress: Arc::new(move |phase| manager.phase(&task, phase)),
        };
        let mut result = json!(WebBuildRunner::run(target, context).await?);
        result["url"] = json!(format!("{}/", project.address.origin()));
        Ok(result)
    }
}
pub(crate) use crate::atomic_control_file::atomic_json;
pub(crate) use crate::project_file_io::ensure_directory as ensure_dir;

pub(crate) fn valid_task_id(id: &str) -> bool {
    id.len() == 37
        && id.starts_with("task_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
#[path = "builds/tests.rs"]
mod tests;
