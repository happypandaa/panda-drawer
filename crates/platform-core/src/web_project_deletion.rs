//! Web-project deletion coordination. Source/data versioning and ZIP operations are retired.
use crate::{
    builds::BuildManager,
    control::{ControlState, Freeze},
    file_identity::PersistedIdentity,
    maintenance::MaintenanceManager,
    projects::{check_no_symlinks, valid_id, ProjectTaskLease},
    ApiError, ProjectStore, RuntimePaths, TaskRecord, TaskStatus,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{sync::broadcast, task::JoinHandle};
use uuid::Uuid;

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    maintenance_id: Option<String>,
    expected_document_epoch: Option<String>,
}
#[derive(Clone)]
struct Operation {
    cleanup_notice: Option<crate::project_cleanup::DeletedProjectNotice>,
    task_id: String,
    kind: String,
    project_id: String,
    source_project_id: Option<String>,
    target_epoch: String,
    phase: String,
    directory_name: Option<String>,
    directory_identity: Option<PersistedIdentity>,
    committed: bool,
    resolved: bool,
    result: Value,
}
#[cfg(test)]
type DeleteCleanupCheckpoint = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
    bool,
);
pub(crate) struct WebProjectDeletionManager {
    project_cleanup: Mutex<Arc<crate::project_cleanup::ProjectCleanupCoordinator>>,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    running: Mutex<Vec<JoinHandle<()>>>,
    operations: Mutex<HashMap<String, Operation>>,
    #[cfg(test)]
    pub(crate) delete_cleanup_checkpoint: Mutex<Option<DeleteCleanupCheckpoint>>,
    stopping: AtomicBool,
    events: broadcast::Sender<String>,
}
fn identity(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected an ordinary project directory",
        ));
    }
    Ok(())
}

fn persistent_identity(path: &Path) -> Result<PersistedIdentity, ApiError> {
    identity(path)?;
    PersistedIdentity::capture(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}

fn operation_project_root(
    _workspace: &Path,
    _id: &str,
    bound: Option<&str>,
) -> Result<PathBuf, ApiError> {
    let root = bound
        .map(PathBuf::from)
        .ok_or_else(|| ApiError::new("RECOVERY_REQUIRED", "Missing project root binding"))?;
    if !root.is_absolute()
        || root
            .components()
            .any(|p| matches!(p, Component::CurDir | Component::ParentDir))
    {
        return Err(ApiError::new(
            "RECOVERY_REQUIRED",
            "Invalid project root binding",
        ));
    }
    Ok(root)
}
fn root_directory_name(root: &Path) -> Result<String, ApiError> {
    root.to_str()
        .map(str::to_owned)
        .ok_or_else(|| ApiError::new("INVALID_PROJECT_PATH", "Project path is not valid Unicode"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    check_no_symlinks(path, false)?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

impl WebProjectDeletionManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        maintenance: Arc<MaintenanceManager>,
    ) -> Result<Arc<Self>, ApiError> {
        store.set_node_source_data_root(paths.data_root().to_path_buf());
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            project_cleanup: Mutex::new(crate::project_cleanup::ProjectCleanupCoordinator::new(
                paths.data_root(),
            )),
            paths,
            store,
            tasks,
            maintenance,
            running: Mutex::new(vec![]),
            operations: Mutex::new(HashMap::new()),
            #[cfg(test)]
            delete_cleanup_checkpoint: Mutex::new(None),
            stopping: AtomicBool::new(false),
            events,
        });
        Ok(manager)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    pub(crate) fn set_project_cleanup(
        &self,
        cleanup: Arc<crate::project_cleanup::ProjectCleanupCoordinator>,
    ) {
        *self
            .project_cleanup
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = cleanup;
    }

    fn cleanup(&self) -> Arc<crate::project_cleanup::ProjectCleanupCoordinator> {
        self.project_cleanup
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn deletion_notice(
        &self,
        operation: &Operation,
    ) -> Result<crate::project_cleanup::DeletedProjectNotice, ApiError> {
        if let Some(notice) = &operation.cleanup_notice {
            return Ok(notice.clone());
        }
        let owner = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let mut tasks = if owner.path(&operation.project_id)?.exists() {
            owner.task_ids(&operation.project_id)?
        } else {
            self.tasks.project_task_ids(&operation.project_id)
        };
        tasks.push(operation.task_id.clone());
        let mut roots = if let Some(directory) = &operation.directory_name {
            vec![operation_project_root(
                self.paths.workspace_root(),
                &operation.project_id,
                Some(directory),
            )?]
        } else {
            self.store
                .deletion_root(&operation.project_id)
                .ok()
                .map(|(root, _)| vec![root])
                .unwrap_or_default()
        };
        if owner.path(&operation.project_id)?.exists() {
            roots.extend(owner.source_roots(&operation.project_id)?);
            roots.sort();
            roots.dedup();
        }
        Ok(crate::project_cleanup::DeletedProjectNotice::new(
            crate::project_cleanup::ProjectKind::Web,
            operation.project_id.clone(),
            tasks,
            vec![],
            roots,
        ))
    }

    fn notify(&self, id: &str) {
        let _ = self.events.send(id.into());
        self.maintenance.notify(id);
    }

    fn remember(&self, operation: &Operation) -> Result<(), ApiError> {
        let mut operations = self.operations.lock().map_err(|_| {
            ApiError::new(
                "LIFECYCLE_STATE_FAILED",
                "Current operation ownership is unavailable",
            )
        })?;
        if operation.resolved {
            operations.remove(&operation.task_id);
        } else {
            operations.insert(operation.task_id.clone(), operation.clone());
        }
        Ok(())
    }

    fn finish_operation(&self, operation: &mut Operation) -> Result<(), ApiError> {
        operation.resolved = true;
        self.remember(operation)
    }

    fn phase(&self, operation: &mut Operation, phase: &str) -> Result<(), ApiError> {
        operation.phase = phase.into();
        self.remember(operation)?;
        self.tasks.phase(&operation.task_id, phase)
    }

    pub(crate) fn task_for_project(
        &self,
        task_id: &str,
        project_id: &str,
        kind: &str,
    ) -> Result<TaskRecord, ApiError> {
        if !valid_id(project_id)
            || !task_id.strip_prefix("task_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            || kind.is_empty()
            || kind.len() > 128
        {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Explicit task, project and type are required.",
            ));
        }
        let operations = self.operations.lock().map_err(|_| {
            ApiError::new(
                "LIFECYCLE_STATE_FAILED",
                "Current operation ownership is unavailable",
            )
        })?;
        let active = operations
            .get(task_id)
            .filter(|operation| !operation.resolved);
        if active
            .is_some_and(|operation| operation.project_id != project_id || operation.kind != kind)
        {
            return Err(ApiError::new(
                "TASK_IDENTITY_CHANGED",
                "Task does not belong to the requested project and type.",
            ));
        }
        let mut record = match self.tasks.task(task_id) {
            Ok(record) => record,
            Err(error) if error.code == "TASK_NOT_FOUND" => {
                if active.is_some_and(|operation| {
                    operation.project_id == project_id && operation.kind == kind && kind == "delete"
                }) {
                    return Ok(deletion_task_receipt(task_id, project_id, false));
                }
                if kind == "delete"
                    && !self.store.has_active_project_work(project_id)
                    && self.store.deleted_project_identity_absent(project_id)?
                    && crate::web_project_cleanup::project_materials_absent(
                        &self.paths,
                        &self.tasks,
                        project_id,
                    )?
                {
                    return Ok(deletion_task_receipt(task_id, project_id, true));
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if record.project_id.as_deref() != Some(project_id) || record.task_type != kind {
            return Err(ApiError::new(
                "TASK_IDENTITY_CHANGED",
                "Task does not belong to the requested project and type.",
            ));
        }
        if kind == "delete" {
            if active.is_some() {
                record.status = TaskStatus::Running;
                record.phase = "deleting-project-records".into();
                record.result = None;
                record.error = None;
            } else if record.status == TaskStatus::Succeeded {
                // A successful delete has no saved task. A saved success can
                // only be the interruption window before final cleanup.
                record.status = TaskStatus::Failed;
                record.phase = "delete-finalization-required".into();
                record.result = None;
                record.error = Some(
                    ApiError::new(
                        "PROJECT_DELETE_INCOMPLETE",
                        "Project deletion cleanup must be retried.",
                    )
                    .outcome("delete_incomplete")
                    .detail("finalCleanupPending", true),
                );
            }
        }
        Ok(record)
    }

    fn check_running(&self) -> Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"))
        } else {
            Ok(())
        }
    }

    fn authorize_delete(
        &self,
        id: &str,
        maintenance: Option<&str>,
    ) -> Result<ControlState, ApiError> {
        let state = self.store.control_for_deletion(id)?.get();
        if (state.mode == "maintenance" && maintenance != state.maintenance_id.as_deref())
            || (state.mode == "ready" && maintenance.is_some())
        {
            return Err(ApiError::new(
                "MAINTENANCE_ID_MISMATCH",
                "Provide the current maintenanceId for this operation",
            ));
        }
        Ok(state)
    }

    fn launch(
        self: &Arc<Self>,
        running: &mut Vec<JoinHandle<()>>,
        mut operation: Operation,
        input: Input,
        leases: Vec<ProjectTaskLease>,
    ) {
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            use futures_util::FutureExt;
            let accepted_task = manager.tasks.task(&operation.task_id).ok();
            let mut retirement =
                DeleteOperationRetirement::new(manager.clone(), &operation, accepted_task.clone());
            let mut result = std::panic::AssertUnwindSafe(manager.delete(&mut operation, input))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(ApiError::new(
                        "LIFECYCLE_WORKER_FAILED",
                        "Project operation stopped unexpectedly.",
                    ))
                });
            if let Err(error) = &result {
                let _ = manager.remember(&operation);
                match manager.reconcile_direct_delete(&mut operation) {
                    Ok(recovered) => {
                        result = Err(error
                            .clone()
                            .outcome(
                                recovered["operationOutcome"]
                                    .as_str()
                                    .unwrap_or("needs_attention"),
                            )
                            .detail("lifecycleResult", recovered));
                    }
                    Err(recovery) => {
                        result = Err(error
                            .clone()
                            .outcome("needs_attention")
                            .detail("recoveryError", json!(recovery)));
                    }
                }
            }
            let deleted = operation.kind == "delete"
                && operation.committed
                && operation.result["operationOutcome"] == "deleted";
            if deleted {
                operation.phase = "deleting-project-records".into();
                let _ = manager.remember(&operation);
                result = Ok(operation.result.clone());
            }
            let finished = manager.tasks.finish(&operation.task_id, result);
            // Keep workspace migration/cleanup behind this accepted worker even
            // after the project lease ends for identity finalization.
            let _finalization_lease = if deleted {
                manager
                    .store
                    .reserve_transient_operation(
                        &format!("project-delete-finalization:{}", operation.project_id),
                        "project-delete",
                    )
                    .ok()
            } else {
                None
            };
            drop(leases);
            if deleted {
                let cleanup_manager = manager.clone();
                let cleanup_operation = operation.clone();
                let cleanup = async {
                    #[cfg(test)]
                    {
                        let checkpoint =
                            { manager.delete_cleanup_checkpoint.lock().unwrap().take() };
                        if let Some((entered, release, fail)) = checkpoint {
                            let _ = entered.send(());
                            let _ = release.await;
                            if fail {
                                return Err(ApiError::new(
                                    "DISPOSABLE_DELETE_CLEANUP_FAILURE",
                                    "Artificial cleanup failure after task finish.",
                                ));
                            }
                        }
                    }
                    cleanup_manager
                        .finish_deleted_project(&cleanup_operation)
                        .await
                }
                .await;
                if let Err(error) = cleanup {
                    let remaining = crate::web_project_cleanup::project_material_paths(
                        &manager.paths,
                        &operation.project_id,
                    )
                    .unwrap_or_default();
                    manager.persist_delete_failure(
                        accepted_task.as_ref(),
                        error
                            .outcome("delete_incomplete")
                            .detail("finalCleanupPending", true)
                            .detail("remainingPaths", json!(remaining)),
                    );
                } else {
                    retirement.disarm();
                    manager
                        .operations
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .retain(|_, existing| existing.project_id != operation.project_id);
                }
                let _ = manager.finish_operation(&mut operation);
            } else if let Err(error) = finished {
                eprintln!("Lifecycle result persistence failed: {}", error.code);
            }
            manager.notify(&operation.project_id);
            if let Some(source) = &operation.source_project_id {
                manager.notify(source);
            }
            retirement.disarm();
        });
        running.push(runner);
    }

    async fn delete(&self, operation: &mut Operation, input: Input) -> Result<Value, ApiError> {
        self.phase(operation, "delete-freeze")?;
        let id = operation.project_id.clone();
        if operation.committed && self.store.deleted_project_identity_absent(&id)? {
            operation.cleanup_notice = Some(
                self.cleanup()
                    .prepare(self.deletion_notice(operation)?)
                    .await?,
            );
            return Ok(operation.result.clone());
        }
        let control = self.store.control_for_deletion(&id)?;
        let _freeze = Freeze::new(control.clone());
        self.notify(&id);
        operation.cleanup_notice = Some(
            self.cleanup()
                .prepare(self.deletion_notice(operation)?)
                .await?,
        );
        let gate = self.store.gate(&id);
        let _guard = gate.write().await;
        self.store.close_database(&id).await?;
        crate::files::recover_attachment_stages(&self.store.private_dir(&id)?.join("uploads"))?;
        self.check_protection(&id, Some(&operation.task_id))?;
        let owner_directory = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        )
        .directory(&id)?;
        match fs::symlink_metadata(&owner_directory) {
            Ok(_) => {
                check_no_symlinks(&owner_directory, false)?;
                let owner = crate::durable_writer::WriterOwner::web(self.paths.data_root(), &id)?;
                crate::durable_writer::require_owner_clear(&owner)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let (root, expected) = self.store.deletion_root(&id)?;
        if root
            != operation_project_root(
                self.paths.workspace_root(),
                &id,
                operation.directory_name.as_deref(),
            )?
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory binding changed after deletion was requested",
            ));
        }
        self.store
            .validate_deletion_epoch(&id, input.expected_document_epoch.as_deref())?;
        if !operation
            .directory_identity
            .as_ref()
            .is_some_and(|identity| identity.matches_captured(&expected))
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory changed after deletion was requested; no files were removed",
            ));
        }
        self.phase(operation, "delete-prepared")?;
        self.store
            .begin_deletion(&id, &operation.task_id, &operation.target_epoch, &expected)?;
        self.phase(operation, "delete-intent")?;
        if self.store.project_ownership(&id)? == crate::projects::ProjectOwnership::Platform
            && root.try_exists()?
        {
            tokio::task::spawn_blocking(move || {
                crate::history_cleanup::remove_owned_directory(&root, &expected).map(|_| ())
            })
            .await
            .map_err(|error| ApiError::new("PROJECT_DELETE_FAILED", error.to_string()))??;
        }
        self.finish_direct_delete(operation)
    }

    fn finish_direct_delete(&self, operation: &mut Operation) -> Result<Value, ApiError> {
        let marker = self
            .store
            .pending_deletion(&operation.project_id)?
            .ok_or_else(|| {
                ApiError::new(
                    "PROJECT_IDENTITY_CHANGED",
                    "Deletion ownership marker is missing",
                )
            })?;
        if marker.task_id != operation.task_id
            || marker.target_epoch != operation.target_epoch
            || !operation
                .directory_identity
                .as_ref()
                .is_some_and(|identity| identity.matches_captured(&marker.root_identity))
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Another operation owns the deletion target; its files and state were retained",
            ));
        }
        let root = operation_project_root(
            self.paths.workspace_root(),
            &operation.project_id,
            operation.directory_name.as_deref(),
        )?;
        if marker.ownership == crate::projects::ProjectOwnership::Platform && root.try_exists()? {
            return Err(ApiError::new(
                "PROJECT_DELETE_INCOMPLETE",
                "Project files remain; deletion is incomplete.",
            ));
        }
        if marker.ownership == crate::projects::ProjectOwnership::Platform {
            platform_native::filesystem::sync_directory(root.parent().unwrap())?;
        }
        operation.committed = true;
        operation.result = json!({"projectId":operation.project_id,"operationOutcome":"deleted","restoreAvailable":false,"sourcePreserved":marker.ownership==crate::projects::ProjectOwnership::User});
        self.remember(operation)?;
        Ok(operation.result.clone())
    }

    fn persist_delete_failure(&self, accepted: Option<&TaskRecord>, error: ApiError) {
        if let Some(accepted) = accepted {
            let result = self.tasks.restore_failed_project_task(accepted, error);
            if let Err(error) = result {
                eprintln!("Deletion failure persistence failed: {}", error.code);
            }
        }
    }

    async fn finish_deleted_project(
        self: &Arc<Self>,
        operation: &Operation,
    ) -> Result<(), ApiError> {
        let cleanup = self.cleanup();
        let notice = if let Some(notice) = &operation.cleanup_notice {
            notice.clone()
        } else {
            cleanup.prepare(self.deletion_notice(operation)?).await?
        };
        let manager = self.clone();
        let removing = operation.clone();
        tokio::task::spawn_blocking(move || {
            crate::web_project_cleanup::remove_project_materials(
                &manager.paths,
                &manager.store,
                &manager.tasks,
                &removing.project_id,
                &removing.task_id,
            )
        })
        .await
        .map_err(|error| ApiError::new("PROJECT_DELETE_FAILED", error.to_string()))??;
        cleanup.purge(&notice).await?;
        let manager = self.clone();
        let finishing = operation.clone();
        tokio::task::spawn_blocking(move || manager.finish_deleted_project_records(&finishing))
            .await
            .map_err(|error| ApiError::new("PROJECT_DELETE_FAILED", error.to_string()))??;
        cleanup.committed(&notice).await;
        Ok(())
    }

    fn finish_deleted_project_records(&self, operation: &Operation) -> Result<(), ApiError> {
        if let Some(marker) = self.store.pending_deletion(&operation.project_id)? {
            if marker.task_id != operation.task_id || marker.target_epoch != operation.target_epoch
            {
                return Err(ApiError::new(
                    "PROJECT_IDENTITY_CHANGED",
                    "Another deletion owns this project.",
                ));
            }
            self.store.complete_deleted_project_identity(&marker)?;
        } else if !self
            .store
            .deleted_project_identity_absent(&operation.project_id)?
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Deletion identity is incomplete.",
            ));
        }
        self.tasks
            .forget_owned_tasks(std::slice::from_ref(&operation.task_id))?;
        crate::web_project_cleanup::finish_project_ownership(&self.paths, &operation.project_id)?;
        Ok(())
    }

    fn reconcile_direct_delete(&self, operation: &mut Operation) -> Result<Value, ApiError> {
        let marker = self.store.pending_deletion(&operation.project_id)?;
        let owned = marker.as_ref().is_some_and(|marker| {
            marker.task_id == operation.task_id
                && marker.target_epoch == operation.target_epoch
                && operation
                    .directory_identity
                    .as_ref()
                    .is_some_and(|identity| identity.matches_captured(&marker.root_identity))
        });
        if operation.resolved {
            return Ok(operation.result.clone());
        }
        let mut outcome = "not_deleted";
        if owned {
            match self.store.deletion_root(&operation.project_id) {
                Ok((root, _))
                    if self.store.project_ownership(&operation.project_id)?
                        == crate::projects::ProjectOwnership::User
                        || !root.try_exists()? =>
                {
                    return self.finish_direct_delete(operation)
                }
                Ok(_) => outcome = "delete_incomplete",
                Err(_) => {
                    outcome = "needs_attention";
                }
            }
        } else if marker.is_some() || operation.phase == "delete-intent" {
            // An old operation is not authority over a later same-ID instance.
            // In particular, do not call ambiguous(), which changes its control.
            outcome = "needs_attention";
        }
        operation.result = json!({"projectId":operation.project_id,"operationOutcome":outcome,"restoreAvailable":false,
                    "remainingPath":operation_project_root(self.paths.workspace_root(), &operation.project_id, operation.directory_name.as_deref())?,
        });
        operation.resolved = true;
        self.remember(operation)?;
        Ok(operation.result.clone())
    }

    fn check_protection(&self, project: &str, except_task: Option<&str>) -> Result<(), ApiError> {
        let operations = self.operations.lock().map_err(|_| {
            ApiError::new(
                "LIFECYCLE_STATE_FAILED",
                "Current operation ownership is unavailable",
            )
        })?;
        if let Some(operation) = operations.values().find(|operation| {
            Some(operation.task_id.as_str()) != except_task
                && !operation.resolved
                && (operation.project_id == project
                    || operation.source_project_id.as_deref() == Some(project))
        }) {
            return Err(ApiError::new(
                "PROJECT_OPERATION_PROTECTED",
                "Another active operation still references this project",
            )
            .detail("taskId", operation.task_id.clone()));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fixture_shutdown_runner(&self, runner: JoinHandle<()>) {
        self.running.lock().unwrap().push(runner);
    }
    #[cfg(test)]
    pub(crate) fn fixture_shutdown_cancel(self: &Arc<Self>, release: std::sync::mpsc::Sender<()>) {
        let manager = self.clone();
        self.fixture_shutdown_runner(tokio::spawn(async move {
            while !manager.stopping.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
            let _ = release.send(());
        }));
    }
    pub(crate) fn request_stop(&self) {
        let _submission = self.running.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
    }

    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.running.lock().unwrap());
        crate::shutdown::join_all("webProjectDeletion", runners).await
    }

    pub async fn submit_project(
        self: &Arc<Self>,
        id: &str,
        kind: &str,
        value: Value,
    ) -> Result<TaskRecord, ApiError> {
        if kind != "delete" {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Only project deletion is supported",
            ));
        }
        let mut running = self.running.lock().unwrap();
        self.check_running()?;
        let input: Input = serde_json::from_value(value)?;
        if self.store.deleted_project_identity_absent(id)? {
            if self.store.has_active_project_work(id) {
                return Err(ApiError::new(
                    "PROJECT_BUSY",
                    "Project work has not finished.",
                ));
            }
            if crate::web_project_cleanup::project_materials_absent(&self.paths, &self.tasks, id)? {
                return Ok(deletion_task_receipt(
                    &format!("task_{}", Uuid::new_v4().simple()),
                    id,
                    true,
                ));
            }
            let lease = self
                .store
                .reserve_transient_operation(id, "delete-finalization")?;
            let ownership = crate::web_project_ownership::WebProjectOwnership::new(
                self.paths.data_root().into(),
                self.paths.workspace_root().into(),
            );
            ownership.initialize(id)?;
            for task in self.tasks.project_task_ids(id) {
                let path = self
                    .paths
                    .workspace_root()
                    .join(".platform/tasks")
                    .join(&task);
                if path.try_exists()? {
                    if ownership
                        .task_directory_identity(id, &task)
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        ownership.verify_task_directory(id, &task)?;
                        continue;
                    }
                    let saved: TaskRecord = read_json(&path.join("task.json"))?;
                    if saved.task_id != task
                        || saved.project_id.as_deref() != Some(id)
                        || !saved.status.is_final()
                    {
                        return Err(ApiError::new(
                            "TASK_IDENTITY_CHANGED",
                            "Remaining task ownership is not confirmed.",
                        ));
                    }
                    ownership.record_task(id, &task)?;
                    ownership.capture_task_directory(id, &task, &persistent_identity(&path)?)?;
                } else {
                    ownership.record_task(id, &task)?;
                }
            }
            let record = self.tasks.create_readonly_operation(id, "delete")?;
            let mut operation = Self::operation_state(&record, id);
            operation.committed = true;
            operation.result =
                json!({"projectId":id,"operationOutcome":"deleted","restoreAvailable":false});
            self.remember(&operation)?;
            self.launch(&mut running, operation, input, vec![lease]);
            return Ok(record);
        }
        let (root, expected) = self.store.deletion_root(id)?;
        self.authorize_delete(id, input.maintenance_id.as_deref())?;
        self.store
            .validate_deletion_epoch(id, input.expected_document_epoch.as_deref())?;
        let (record, lease) = self.tasks.create_delete_operation(id)?;
        let mut operation = Self::operation_state(&record, id);
        operation.directory_name = Some(root_directory_name(&root)?);
        operation.directory_identity = Some(expected);
        let prepared = (|| {
            self.authorize_delete(id, input.maintenance_id.as_deref())?;
            self.store
                .validate_deletion_epoch(id, input.expected_document_epoch.as_deref())?;
            self.remember(&operation)
        })();
        if let Err(error) = prepared {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        self.launch(&mut running, operation, input, vec![lease]);
        Ok(record)
    }
    fn operation_state(record: &TaskRecord, project: &str) -> Operation {
        Operation {
            cleanup_notice: None,
            task_id: record.task_id.clone(),
            kind: "delete".into(),
            project_id: project.into(),
            source_project_id: None,
            target_epoch: Uuid::new_v4().to_string(),
            phase: "intent".into(),
            directory_name: None,
            directory_identity: None,
            committed: false,
            resolved: false,
            result: json!({}),
        }
    }
}

fn deletion_task_receipt(task_id: &str, project_id: &str, completed: bool) -> TaskRecord {
    TaskRecord { task_id:task_id.into(),project_id:Some(project_id.into()),task_type:"delete".into(),status:if completed {TaskStatus::Succeeded}else{TaskStatus::Running},phase:if completed {"completed"}else{"deleting-project-records"}.into(),result:completed.then(||json!({"projectId":project_id,"operationOutcome":"deleted","alreadyAbsent":true,"restoreAvailable":false})),error:None }
}

struct DeleteOperationRetirement {
    manager: Arc<WebProjectDeletionManager>,
    task_id: String,
    accepted: Option<TaskRecord>,
    armed: bool,
}
impl DeleteOperationRetirement {
    fn new(
        manager: Arc<WebProjectDeletionManager>,
        operation: &Operation,
        accepted: Option<TaskRecord>,
    ) -> Self {
        Self {
            manager,
            task_id: operation.task_id.clone(),
            accepted,
            armed: operation.kind == "delete",
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for DeleteOperationRetirement {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.manager.persist_delete_failure(
                self.accepted.as_ref(),
                ApiError::new(
                    "PROJECT_DELETE_INCOMPLETE",
                    "Deletion worker stopped before cleanup completed.",
                )
                .outcome("delete_incomplete")
                .detail("finalCleanupPending", true),
            );
        }));
        self.manager
            .operations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.task_id);
    }
}
