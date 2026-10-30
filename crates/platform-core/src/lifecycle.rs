//! Single-project import/export/delete operations. Process-local ownership is
//! retained only while this service is alive; no lifecycle history is written.
//! External ZIPs are never modified by import.
use crate::{
    backups::{
        self, BackupManifest, PendingBuild, PreparedSnapshot, SnapshotOptions, SnapshotState,
    },
    builds::{self, BuildManager},
    control::{ControlState, Freeze},
    file_identity::PersistedIdentity,
    maintenance::MaintenanceManager,
    projects::{check_no_symlinks, valid_id, validate_manifest, ProjectTaskLease},
    ApiError, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord, TaskStatus,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
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
    destination_path: Option<PathBuf>,
    archive_path: Option<PathBuf>,
    #[serde(default)]
    overwrite: bool,
    mode: Option<String>,
    name: Option<String>,
    maintenance_id: Option<String>,
    expected_document_epoch: Option<String>,
    #[serde(default)]
    cleanup_temporary_files: bool,
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
    cleanup_temporary_files: bool,
    import_temporary_identity: Option<PersistedIdentity>,
    control: Option<ControlState>,
    destination: Option<PathBuf>,
    delivery_parent_identity: Option<PersistedIdentity>,
    published_sha256: Option<String>,
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

pub(crate) struct LifecycleManager {
    project_cleanup: Mutex<Arc<crate::project_cleanup::ProjectCleanupCoordinator>>,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    running: Mutex<Vec<JoinHandle<()>>>,
    operations: Mutex<HashMap<String, Operation>>,
    #[cfg(test)]
    import_inspection_pause: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    import_cleanup_pause: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    delete_cleanup_checkpoint: Mutex<Option<DeleteCleanupCheckpoint>>,
    stopping: AtomicBool,
    events: broadcast::Sender<String>,
}

use crate::lifecycle_file_io::require_directory as identity;

fn persistent_identity(path: &Path) -> Result<PersistedIdentity, ApiError> {
    identity(path)?;
    PersistedIdentity::capture(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}
fn matches_identity(expected: &PersistedIdentity, path: &Path) -> Result<bool, ApiError> {
    identity(path)?;
    expected
        .matches_path(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}
/// Operations bind the target before publication. Older operations retain their
/// original ID directory even if a later project has a different registration.
fn operation_project_root(
    workspace: &Path,
    id: &str,
    directory: Option<&str>,
) -> Result<PathBuf, ApiError> {
    let directory = directory.unwrap_or(id);
    if !valid_id(id)
        || directory.is_empty()
        || directory.len() > 128
        || !directory.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
        || Path::new(directory).components().count() != 1
        || !matches!(
            Path::new(directory).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(ApiError::new(
            "RECOVERY_REQUIRED",
            "Invalid operation project directory binding",
        ));
    }
    let root = workspace.join("projects").join(directory);
    check_no_symlinks(&root, true)?;
    Ok(root)
}
fn root_directory_name(root: &Path) -> Result<String, ApiError> {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::new("INVALID_PROJECT_PATH", "Project directory name is invalid"))
}
fn sync_parent(path: &Path) -> Result<(), ApiError> {
    File::open(
        path.parent()
            .ok_or_else(|| ApiError::new("UNSAFE_PATH", "Path has no parent"))?,
    )?
    .sync_all()?;
    Ok(())
}
fn move_directory(from: &Path, to: &Path) -> Result<(), ApiError> {
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    if to.exists() {
        return Err(
            ApiError::new("PROJECT_ID_CONFLICT", "Destination already exists")
                .detail("path", json!(to)),
        );
    }
    crate::projects::publish_directory_noreplace(from, to)?;
    sync_parent(from)?;
    sync_parent(to)
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    check_no_symlinks(path, false)?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn sha256(path: &Path) -> Result<String, ApiError> {
    check_no_symlinks(path, false)?;
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ApiError::new("UNSAFE_PATH", "Expected an ordinary file"));
    }
    let mut digest = Sha256::new();
    let mut bytes = [0; 128 * 1024];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        digest.update(&bytes[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn copy_verified(from: &Path, to: &Path) -> Result<(), ApiError> {
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    let expected = sha256(from)?;
    let bytes = fs::metadata(from)?.len();
    let parent = to
        .parent()
        .ok_or_else(|| ApiError::new("UNSAFE_PATH", "Copy has no parent"))?;
    fs::create_dir_all(parent)?;
    if fs2::available_space(parent)? < bytes.saturating_add(16 * 1024 * 1024) {
        return Err(ApiError::new(
            "SPACE_INSUFFICIENT",
            "Insufficient space for an independent archive copy",
        ));
    }
    let mut output = platform_native::filesystem::create_new_private_file(to, false)?;
    std::io::copy(&mut File::open(from)?, &mut output)?;
    output.sync_all()?;
    if sha256(to)? != expected || sha256(from)? != expected {
        return Err(ApiError::new(
            "ARCHIVE_SOURCE_CHANGED",
            "Archive changed while it was copied",
        ));
    }
    sync_parent(to)
}

impl LifecycleManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        maintenance: Arc<MaintenanceManager>,
    ) -> Result<Arc<Self>, ApiError> {
        let operations = paths.workspace_root().join(".platform/operations");
        check_no_symlinks(&operations, true)?;
        fs::create_dir_all(operations)?;
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
            import_inspection_pause: Mutex::new(None),
            #[cfg(test)]
            import_cleanup_pause: Mutex::new(None),
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
    fn operation(&self, task: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/operations")
            .join(task)
    }
    fn remember(&self, operation: &Operation) -> Result<(), ApiError> {
        let mut operations = self.operations.lock().map_err(|_| {
            ApiError::new(
                "LIFECYCLE_STATE_FAILED",
                "Current operation ownership is unavailable",
            )
        })?;
        if operation.resolved && !operation.cleanup_temporary_files {
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
    fn authorize(&self, id: &str, maintenance: Option<&str>) -> Result<ControlState, ApiError> {
        let state = self.store.control(id)?.get();
        if state.recovery_required {
            return Err(ApiError::new(
                "RECOVERY_REQUIRED",
                "Resolve the retained recovery operation first",
            ));
        }
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
    fn input(value: Value) -> Result<Input, ApiError> {
        let input: Input = serde_json::from_value(value)?;
        if input
            .name
            .as_ref()
            .is_some_and(|name| name.trim().is_empty() || name.chars().count() > 200)
        {
            return Err(ApiError::new(
                "INVALID_PROJECT_NAME",
                "Name must contain 1 to 200 characters",
            ));
        }
        Ok(input)
    }
    fn operation_state(record: &TaskRecord, kind: &str, project: &str) -> Operation {
        Operation {
            cleanup_notice: None,
            task_id: record.task_id.clone(),
            kind: kind.into(),
            project_id: project.into(),
            source_project_id: None,
            target_epoch: Uuid::new_v4().to_string(),
            phase: "intent".into(),
            directory_name: None,
            directory_identity: None,
            cleanup_temporary_files: false,
            import_temporary_identity: None,
            control: None,
            destination: None,
            delivery_parent_identity: None,
            published_sha256: None,
            committed: false,
            resolved: false,
            result: json!({}),
        }
    }
    pub async fn submit_project(
        self: &Arc<Self>,
        id: &str,
        kind: &str,
        value: Value,
    ) -> Result<TaskRecord, ApiError> {
        let mut running = self.running.lock().unwrap();
        self.check_running()?;
        let input = Self::input(value)?;
        if input.cleanup_temporary_files {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Temporary import cleanup is only available for import",
            ));
        }
        if !matches!(kind, "export" | "copy" | "delete") {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Unknown project lifecycle operation",
            ));
        }
        if kind == "delete" && self.store.deleted_project_identity_absent(id)? {
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
                    let task_file = path.join("task.json");
                    check_no_symlinks(&task_file, false)?;
                    let saved: TaskRecord = read_json(&task_file)?;
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
            let mut operation = Self::operation_state(&record, "delete", id);
            operation.committed = true;
            operation.result =
                json!({"projectId":id,"operationOutcome":"deleted","restoreAvailable":false});
            self.remember(&operation)?;
            self.launch(&mut running, operation, input, vec![lease]);
            return Ok(record);
        }
        let deletion = (kind == "delete")
            .then(|| self.store.deletion_root(id))
            .transpose()?;
        let (root, manifest) = if let Some((root, _)) = &deletion {
            (root.clone(), None)
        } else {
            self.store.metadata_root(id)?
        };
        if kind == "delete" {
            self.authorize_delete(id, input.maintenance_id.as_deref())?;
            self.store
                .validate_deletion_epoch(id, input.expected_document_epoch.as_deref())?;
        } else {
            self.authorize(id, input.maintenance_id.as_deref())?;
        }
        if kind == "export" {
            self.export_destination(
                input
                    .destination_path
                    .as_deref()
                    .ok_or_else(|| ApiError::new("INVALID_INPUT", "destinationPath is required"))?,
                &root,
                input.overwrite,
            )?;
        }
        if kind != "delete" && manifest.is_none() {
            return Err(ApiError::new(
                "INVALID_PROJECT_MANIFEST",
                "Export and copy require a valid project manifest",
            ));
        }
        let target = if kind == "copy" {
            format!("prj_{}", Uuid::new_v4().simple())
        } else {
            id.into()
        };
        if kind == "copy" {
            self.store.initialize_project_ownership(&target)?;
        }
        let admitted = if kind == "delete" {
            self.tasks.create_delete_operation(id)
        } else {
            self.tasks.create_operation(&target, kind)
        };
        let (mut record, source_lease) = admitted.inspect_err(|_| {
            if kind == "copy" {
                let _ = self.store.discard_unpublished_ownership(&target);
            }
        })?;
        let mut operation = Self::operation_state(&record, kind, &target);
        operation.source_project_id = Some(id.into());
        operation.destination = input.destination_path.clone();
        if kind != "copy" {
            operation.directory_name = Some(root_directory_name(&root)?);
        }
        operation.directory_identity = deletion.map(|(_, expected)| expected);
        let mut leases = vec![source_lease];
        let prepare = (|| {
            if kind == "delete" {
                self.authorize_delete(id, input.maintenance_id.as_deref())?;
                self.store
                    .validate_deletion_epoch(id, input.expected_document_epoch.as_deref())?;
            } else {
                self.authorize(id, input.maintenance_id.as_deref())?;
            }
            if kind == "export" {
                let destination = self.export_destination(
                    input.destination_path.as_deref().unwrap(),
                    &root,
                    false,
                )?;
                operation.delivery_parent_identity =
                    Some(persistent_identity(destination.parent().unwrap())?);
                operation.destination = Some(destination);
            }
            self.remember(&operation)?;
            if kind == "copy" {
                leases.push(self.store.reserve_task(id, &record.task_id)?);
                self.store.reserve_identity(&target, &record.task_id)?;
                record.result = Some(json!({"targetProjectId":target}));
            }
            Ok::<(), ApiError>(())
        })();
        if let Err(error) = prepare {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        self.launch(&mut running, operation, input, leases);
        Ok(record)
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
    async fn owned_operation<T, F, W>(self: &Arc<Self>, work: W) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
        W: FnOnce(Arc<Self>) -> F + Send + 'static,
    {
        let receive = {
            let mut running = self.running.lock().unwrap();
            self.check_running()?;
            let manager = self.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            running.push(tokio::spawn(async move {
                let result = work(manager).await;
                let _ = send.send(result);
            }));
            receive
        };
        receive.await.map_err(|_| {
            ApiError::new(
                "LIFECYCLE_WORKER_FAILED",
                "Lifecycle worker did not return a result; shutdown retains its JoinError",
            )
        })?
    }
    #[cfg(test)]
    pub(crate) fn fixture_import_inspection_pause(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self.import_inspection_pause.lock().unwrap() = Some((entered, release));
    }
    pub async fn submit_import(self: &Arc<Self>, value: Value) -> Result<TaskRecord, ApiError> {
        self.check_running()?;
        let input = Self::input(value)?;
        if input.overwrite || input.destination_path.is_some() || input.maintenance_id.is_some() {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Import does not overwrite existing projects",
            ));
        }
        let archive = input
            .archive_path
            .clone()
            .ok_or_else(|| ApiError::new("INVALID_INPUT", "archivePath is required"))?;
        if input.mode.as_deref().is_some_and(|mode| mode != "copy") {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Every import creates a new project identity; mode can only be copy",
            ));
        }
        if !archive.is_absolute() {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "archivePath must be absolute",
            ));
        }
        self.owned_operation(move |manager| async move { manager.prepare_import(input).await })
            .await
    }
    async fn prepare_import(self: &Arc<Self>, input: Input) -> Result<TaskRecord, ApiError> {
        let archive = input.archive_path.clone().expect("validated archive path");
        #[cfg(test)]
        let pause = self.import_inspection_pause.lock().unwrap().take();
        let metadata = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.recv();
            }
            backups::inspect_manifest(&archive)
        })
        .await
        .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        let mut running = self.running.lock().unwrap();
        self.check_running()?;
        let target = format!("prj_{}", Uuid::new_v4().simple());
        self.store.initialize_project_ownership(&target)?;
        self.store.check_identity_available(&target, None)?;
        let (mut record, lease) = self.tasks.create_operation(&target, "import")?;
        let mut operation = Self::operation_state(&record, "import", &target);
        operation.source_project_id = Some(metadata.project_id);
        operation.cleanup_temporary_files = input.cleanup_temporary_files;
        let prepared = self
            .remember(&operation)
            .and_then(|()| self.store.reserve_identity(&target, &record.task_id));
        if let Err(error) = prepared {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        record.result = Some(json!({"targetProjectId":target}));
        if operation.cleanup_temporary_files {
            let temporary = self.import_temporary(&operation);
            let prepared = (|| {
                check_no_symlinks(&temporary, true)?;
                fs::create_dir_all(self.operation(&record.task_id))?;
                fs::create_dir(&temporary)?;
                operation.import_temporary_identity = Some(persistent_identity(&temporary)?);
                self.remember(&operation)
            })();
            if let Err(error) = prepared {
                self.tasks.finish(&record.task_id, Err(error.clone()))?;
                return Err(error.detail("taskId", record.task_id.clone()));
            }
        }
        self.launch(&mut running, operation, input, vec![lease]);
        Ok(record)
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
            let mut result = std::panic::AssertUnwindSafe(manager.run(&mut operation, input))
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
                match manager.reconcile(&mut operation) {
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
    async fn run(&self, operation: &mut Operation, input: Input) -> Result<Value, ApiError> {
        match operation.kind.as_str() {
            "export" => self.export(operation, input).await,
            "copy" => self.copy(operation, input).await,
            "import" => self.import(operation, input).await,
            "delete" => self.delete(operation, input).await,
            _ => Err(ApiError::new(
                "INVALID_INPUT",
                "Unknown lifecycle operation kind",
            )),
        }
    }
    fn export_destination(
        &self,
        path: &Path,
        source: &Path,
        _overwrite: bool,
    ) -> Result<PathBuf, ApiError> {
        validated_export_destination(&self.paths, &self.store, path, source)
    }
    async fn snapshot(
        &self,
        operation: &mut Operation,
        source: &str,
    ) -> Result<PreparedSnapshot, ApiError> {
        self.phase(operation, "snapshot-freeze")?;
        let control = self.store.control(source)?;
        let freeze = Freeze::new(control.clone());
        self.notify(source);
        let gate = self.store.gate(source);
        let guard = gate.write().await;
        let project = self.store.project(source)?;
        project.check()?;
        let state = control.get();
        let info = builds::build_info(&self.paths, &project)?;
        let changed = state.mode == "maintenance"
            && info["sourceState"] != "matched"
            && state
                .baseline_input_digest
                .as_ref()
                .is_some_and(|old| builds::input_digest(&project).ok().as_ref() != Some(old));
        let options = SnapshotOptions {
            backup_id: format!("backup_{}", Uuid::new_v4().simple()),
            kind: "export".into(),
            note: None,
            snapshot_state: if state.mode == "maintenance" {
                SnapshotState::Maintenance
            } else {
                SnapshotState::Ready
            },
            pending_build: PendingBuild {
                required: state.pending_build || changed,
                reason: state.pending_build_reason.or_else(|| {
                    changed.then(|| "Maintenance contains unfinished source changes".into())
                }),
            },
            build_provenance: backups::capture_provenance(&project)?,
        };
        let destination = self.operation(&operation.task_id).join("snapshot");
        let prepared = tokio::task::spawn_blocking(move || {
            backups::prepare_snapshot(&project.root, &project.manifest, &destination, options)
        })
        .await
        .map_err(|e| ApiError::new("SNAPSHOT_FAILED", e.to_string()))??;
        drop(guard);
        drop(freeze);
        self.notify(source);
        Ok(prepared)
    }
    async fn export(&self, operation: &mut Operation, input: Input) -> Result<Value, ApiError> {
        let source = operation.project_id.clone();
        let root = self.store.metadata_root(&source)?.0;
        let destination = self.export_destination(
            input.destination_path.as_deref().unwrap(),
            &root,
            input.overwrite,
        )?;
        operation.destination = Some(destination.clone());
        let delivery_parent = operation
            .delivery_parent_identity
            .clone()
            .ok_or_else(|| ApiError::new("EXPORT_FAILED", "导出缺少本次选择的父目录身份。"))?;
        if !delivery_parent
            .matches_path(destination.parent().unwrap())
            .unwrap_or(false)
        {
            return Err(ApiError::new(
                "EXPORT_DESTINATION_CHANGED",
                "导出文件夹已变化，请重新选择。",
            ));
        }
        let snapshot = self.snapshot(operation, &source).await?;
        let stage = destination
            .parent()
            .unwrap()
            .join(format!(".export-{}.partial", operation.task_id));
        let archive = self.operation(&operation.task_id).join("export.zip");
        let staged = archive.clone();
        let snapshot_root = snapshot.root.clone();
        let snapshot_identity = persistent_identity(&snapshot_root)?;
        self.phase(operation, "archive-verify")?;
        let info =
            tokio::task::spawn_blocking(move || backups::publish_snapshot(&snapshot, &staged))
                .await
                .map_err(|e| ApiError::new("EXPORT_FAILED", e.to_string()))??;
        let ownership = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let copy_owner = ownership.clone();
        let copy_project = source.clone();
        let copy_task = operation.task_id.clone();
        let copy_archive = archive.clone();
        let copy_stage = stage.clone();
        let copy_paths = self.paths.clone();
        let copy_store = self.store.clone();
        let copy_destination = destination.clone();
        let copy_root = root.clone();
        let copy_parent = delivery_parent.clone();
        tokio::task::spawn_blocking(move || {
            write_owned_export_stage(
                &copy_owner,
                &copy_project,
                &copy_task,
                &copy_archive,
                &copy_stage,
                || {
                    if !copy_parent
                        .matches_path(copy_destination.parent().unwrap())
                        .unwrap_or(false)
                    {
                        return Err(ApiError::new(
                            "EXPORT_DESTINATION_CHANGED",
                            "导出文件夹已变化，请重新选择。",
                        ));
                    }
                    validated_export_destination(
                        &copy_paths,
                        &copy_store,
                        &copy_destination,
                        &copy_root,
                    )
                    .map(|_| ())
                },
            )
        })
        .await
        .map_err(|error| ApiError::new("EXPORT_FAILED", error.to_string()))??;
        operation.published_sha256 = Some(sha256(&stage)?);
        operation.result = json!({"projectId":source,"path":destination,"backupId":info.manifest.backup_id,"createdAt":info.manifest.created_at,"snapshotState":info.manifest.snapshot_state,"pendingBuild":info.manifest.pending_build,"verified":true,"diagnostics":info.database,"operationOutcome":"exported"});
        self.phase(operation, "export-publish")?;
        self.export_destination(&destination, &root, input.overwrite)?;
        if !delivery_parent
            .matches_path(destination.parent().unwrap())
            .unwrap_or(false)
        {
            return Err(ApiError::new(
                "EXPORT_DESTINATION_CHANGED",
                "导出文件夹已变化，已保留临时文件。",
            ));
        }
        let stage_identity = ownership
            .material_identity(&source, &stage)?
            .ok_or_else(|| ApiError::new("EXPORT_FAILED", "导出临时文件缺少已登记身份。"))?;
        ownership.capture_export_stage(&source, &operation.task_id, &stage, &stage_identity)?;
        publish_export_file(&stage, &destination, &stage_identity)?;
        sync_parent(&destination)?;
        operation.committed = true;
        operation.resolved = true;
        self.remember(operation)?;
        if let Err(error) = ownership.forget_export_stage(&source, &operation.task_id) {
            operation.result["cleanupWarning"] =
                json!(format!("导出已完成，临时材料登记清理失败：{error}"));
        }
        let _ = fs::remove_file(&archive);
        let cleaned = tokio::task::spawn_blocking(move || {
            crate::history_cleanup::remove_owned_directory(&snapshot_root, &snapshot_identity)
        })
        .await;
        if let Err(error) = cleaned
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()))
        {
            operation.result["cleanupWarning"] = json!(format!(
                "Export completed but temporary snapshot cleanup failed: {error}"
            ));
        }
        let _ = fs::remove_dir(self.operation(&operation.task_id));
        Ok(operation.result.clone())
    }
    async fn copy(&self, operation: &mut Operation, input: Input) -> Result<Value, ApiError> {
        let source = operation.source_project_id.clone().unwrap();
        let snapshot = self.snapshot(operation, &source).await?;
        let snapshot_root = snapshot.root.clone();
        let snapshot_identity = persistent_identity(&snapshot_root)?;
        let candidate = self.operation(&operation.task_id).join("candidate");
        move_directory(&snapshot.root.join("project"), &candidate)?;
        let mut manifest: ProjectManifest = read_json(&candidate.join("project.json"))?;
        manifest.id = operation.project_id.clone();
        manifest.name = input
            .name
            .unwrap_or_else(|| format!("{} · 副本", manifest.name));
        builds::atomic_json(&candidate.join("project.json"), &manifest)?;
        let mut result = self
            .prepare_install(operation, candidate, manifest, snapshot.manifest)
            .await?;
        let cleaned = tokio::task::spawn_blocking(move || {
            crate::history_cleanup::remove_owned_directory(&snapshot_root, &snapshot_identity)
        })
        .await;
        if let Err(error) = cleaned
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()))
        {
            result["cleanupWarning"] = json!(format!(
                "Project copied but temporary snapshot cleanup failed: {error}"
            ));
        }
        let _ = fs::remove_dir(self.operation(&operation.task_id));
        Ok(result)
    }
    async fn import(&self, operation: &mut Operation, input: Input) -> Result<Value, ApiError> {
        self.phase(operation, "archive-copy")?;
        let payload = if operation.cleanup_temporary_files {
            self.import_temporary(operation)
        } else {
            self.operation(&operation.task_id)
        };
        let archived = payload.join("source.zip");
        let from = input.archive_path.unwrap();
        let to = archived.clone();
        tokio::task::spawn_blocking(move || copy_verified(&from, &to))
            .await
            .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        self.phase(operation, "archive-validate")?;
        let candidate = payload.join("candidate");
        let path = archived.clone();
        let destination = candidate.clone();
        let original_id = operation.source_project_id.clone().unwrap();
        let validated = tokio::task::spawn_blocking(move || {
            backups::extract_validated(&path, &destination, Some(&original_id))
        })
        .await
        .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        let mut manifest = validated.project_manifest;
        manifest.id = operation.project_id.clone();
        if let Some(name) = input.name {
            manifest.name = name;
        }
        builds::atomic_json(&candidate.join("project.json"), &manifest)?;
        self.prepare_install(operation, candidate, manifest, validated.manifest)
            .await?;
        if operation.cleanup_temporary_files {
            self.phase(operation, "cleanup")?;
            #[cfg(test)]
            {
                let pause = self.import_cleanup_pause.lock().unwrap().take();
                if let Some((entered, release)) = pause {
                    let _ = entered.send(());
                    let _ = release.await;
                }
            }
            let cleanup = self.cleanup_import_payload(operation)?;
            operation.result["cleanupPending"] = json!(cleanup["completed"] != true);
            operation.result["cleanup"] = cleanup;
            self.remember(operation)?;
        }
        Ok(operation.result.clone())
    }
    async fn prepare_install(
        &self,
        operation: &mut Operation,
        candidate: PathBuf,
        manifest: ProjectManifest,
        metadata: BackupManifest,
    ) -> Result<Value, ApiError> {
        validate_manifest(&manifest, &operation.project_id, &candidate)?;
        // Validate before creating control state or committing a new instance.
        // Old maintenance/unknown archives need an explicit compatibility flow;
        // they cannot silently become ready when ZIP baselines are retired.
        let root = candidate.clone();
        let project = manifest.clone();
        let legacy = metadata.clone();
        tokio::task::spawn_blocking(move || validate_install_snapshot(&root, &project, &legacy))
            .await
            .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        let target = self.store.reserve_named_identity(
            &operation.project_id,
            &operation.task_id,
            &manifest.name,
        )?;
        operation.directory_name = Some(root_directory_name(&target)?);
        self.remember(operation)?;
        self.phase(operation, "candidate-initialize")?;
        let workspace = self.paths.workspace_root().to_owned();
        let root = candidate.clone();
        let project = manifest.clone();
        let task = operation.task_id.clone();
        let final_root = target.clone();
        let (state, diagnostics) = tokio::task::spawn_blocking(move || {
            initialize_candidate(&workspace, &root, &final_root, &project, &metadata, &task)
        })
        .await
        .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        operation.directory_identity = Some(persistent_identity(&candidate)?);
        operation.control = Some(state.clone());
        operation.result = json!({"projectId":operation.project_id,"targetProjectId":operation.project_id,"sourceProjectId":operation.source_project_id,"documentEpoch":operation.target_epoch,"mode":state.mode,"maintenanceId":state.maintenance_id,"readyForChanges":state.ready_for_changes,"baselineBackupId":state.baseline_backup_id,"pendingBuild":{"required":state.pending_build,"reason":state.pending_build_reason},"diagnostics":diagnostics,"operationOutcome":if operation.kind=="copy"{"copied"}else{"imported"}});
        self.phase(operation, "install-intent")?;
        self.store.control(&operation.project_id)?.set(state)?;
        clear_project_provenance(self.paths.workspace_root(), &operation.project_id)?;
        move_directory(&candidate, &target)?;
        self.store.register_lifecycle(
            &operation.project_id,
            &operation.target_epoch,
            &operation.task_id,
            false,
        )?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &operation.project_id)?;
        operation.committed = true;
        self.finish_operation(operation)?;
        self.notify(&operation.project_id);
        Ok(operation.result.clone())
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
        operation.control = Some(control.get());
        self.phase(operation, "delete-prepared")?;
        self.store
            .begin_deletion(&id, &operation.task_id, &operation.target_epoch, &expected)?;
        self.phase(operation, "delete-intent")?;
        if root.try_exists()? {
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
        if root.try_exists()? {
            return Err(ApiError::new(
                "PROJECT_DELETE_INCOMPLETE",
                "Project files remain; deletion is incomplete.",
            ));
        }
        File::open(root.parent().unwrap())?.sync_all()?;
        operation.committed = true;
        operation.result = json!({"projectId":operation.project_id,"operationOutcome":"deleted","restoreAvailable":false});
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
                Ok((root, _)) if !root.try_exists()? => {
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

    fn reconcile(&self, operation: &mut Operation) -> Result<Value, ApiError> {
        if operation.kind == "delete" {
            return self.reconcile_direct_delete(operation);
        }
        if operation.resolved {
            return Ok(operation.result.clone());
        }
        let root = operation_project_root(
            self.paths.workspace_root(),
            &operation.project_id,
            operation.directory_name.as_deref(),
        )?;
        let expected = operation.directory_identity.clone();
        let matches = |path: &Path| {
            expected
                .as_ref()
                .is_some_and(|expected| matches_identity(expected, path).unwrap_or(false))
        };
        let mut outcome = "not_started";
        match operation.kind.as_str() {
            "export" => {
                let published = operation
                    .destination
                    .as_ref()
                    .zip(operation.published_sha256.as_ref())
                    .is_some_and(|(path, digest)| {
                        sha256(path).is_ok_and(|actual| &actual == digest)
                    });
                if published {
                    operation.committed = true;
                } else {
                    outcome = "not_published";
                }
            }
            "import" | "copy" => {
                if matches(&root) {
                    if !self
                        .store
                        .project_root(&operation.project_id)
                        .is_ok_and(|registered| registered == root)
                    {
                        return self.ambiguous(operation, "Registered project directory differs from the install operation; all copies were preserved");
                    }
                    if self
                        .store
                        .registered_epoch(&operation.project_id)?
                        .as_deref()
                        != Some(&operation.target_epoch)
                    {
                        self.store.control(&operation.project_id)?.set(
                            operation.control.clone().ok_or_else(|| {
                                ApiError::new(
                                    "RECOVERY_REQUIRED",
                                    "Install operation lacks its prepared control state",
                                )
                            })?,
                        )?;
                        self.store.register_lifecycle(
                            &operation.project_id,
                            &operation.target_epoch,
                            &operation.task_id,
                            false,
                        )?;
                    }
                    operation.committed = true;
                    MaintenanceManager::invalidate_storage(
                        self.paths.workspace_root(),
                        &operation.project_id,
                    )?;
                } else if !root.exists() {
                    self.store
                        .release_identity(&operation.project_id, &operation.task_id)?;
                    outcome = "not_registered";
                } else {
                    return self.ambiguous(
                        operation,
                        "Target project identity differs from the validated candidate",
                    );
                }
            }
            _ => {
                return Err(ApiError::new(
                    "RECOVERY_REQUIRED",
                    "Unknown lifecycle operation kind",
                ))
            }
        }
        if !operation.committed {
            operation.result = json!({"projectId":operation.project_id,"operationOutcome":outcome});
        }
        self.finish_operation(operation)?;
        Ok(operation.result.clone())
    }
    fn ambiguous(&self, operation: &mut Operation, message: &str) -> Result<Value, ApiError> {
        let error = self
            .store
            .control(&operation.project_id)?
            .mark_recovery_required(
                ApiError::new("RECOVERY_REQUIRED", message).outcome("needs_attention"),
            );
        self.remember(operation)?;
        Ok(
            json!({"projectId":operation.project_id,"operationOutcome":"needs_attention","error":error}),
        )
    }
    fn import_temporary(&self, operation: &Operation) -> PathBuf {
        self.operation(&operation.task_id).join("import-temporary")
    }
    fn cleanup_import_payload(&self, operation: &Operation) -> Result<Value, ApiError> {
        if operation.kind != "import"
            || !operation.cleanup_temporary_files
            || operation.import_temporary_identity.is_none()
            || !operation.resolved
            || !matches!(
                operation.result["operationOutcome"].as_str(),
                Some("imported" | "not_registered" | "not_started")
            )
        {
            return Err(ApiError::new(
                "IMPORT_CLEANUP_BLOCKED",
                "导入结果尚未确认，未清理临时材料。",
            ));
        }
        if operation.result["operationOutcome"] == "imported" {
            let root = operation_project_root(
                self.paths.workspace_root(),
                &operation.project_id,
                operation.directory_name.as_deref(),
            )?;
            if !operation.committed
                || !operation
                    .directory_identity
                    .as_ref()
                    .is_some_and(|expected| matches_identity(expected, &root).unwrap_or(false))
                || !self
                    .store
                    .project_root(&operation.project_id)
                    .is_ok_and(|registered| registered == root)
                || self
                    .store
                    .registered_epoch(&operation.project_id)?
                    .as_deref()
                    != Some(operation.target_epoch.as_str())
                || self
                    .store
                    .control(&operation.project_id)?
                    .get()
                    .recovery_required
            {
                return Err(ApiError::new(
                    "IMPORT_CLEANUP_BLOCKED",
                    "已导入项目的登记仍需核对，未清理恢复依据。",
                ));
            }
            self.check_protection(&operation.project_id, Some(&operation.task_id))?;
        } else if operation.committed {
            return Err(ApiError::new(
                "IMPORT_CLEANUP_BLOCKED",
                "安装提交和结果记录不一致，未删除文件。",
            ));
        }
        let temporary = self.import_temporary(operation);
        check_no_symlinks(&temporary, true)?;
        let mut removed = vec![];
        let mut failures = vec![];
        if temporary.try_exists()? {
            let expected = operation
                .import_temporary_identity
                .as_ref()
                .ok_or_else(|| {
                    ApiError::new(
                        "IMPORT_CLEANUP_BLOCKED",
                        "缺少本次临时目录的归属记录，未删除文件。",
                    )
                })?;
            match crate::history_cleanup::remove_owned_directory(&temporary, expected) {
                Ok(_) => removed.push(temporary.clone()),
                Err(error) => failures.push(json!({"path":temporary,"error":error})),
            }
        }
        let result = json!({"completed":failures.is_empty() && !temporary.exists(),"projectId":operation.project_id,"operationOutcome":operation.result["operationOutcome"],"removedPaths":removed,"failures":failures});
        if result["completed"] == true {
            let _ = fs::remove_dir(self.operation(&operation.task_id));
        }
        Ok(result)
    }
    /// Only this service's exact opt-in import files are removable. A restart
    /// discards ownership, so old materials are never adopted as a cleanup target.
    pub async fn cleanup_import(self: &Arc<Self>, task_id: String) -> Result<Value, ApiError> {
        if !valid_id(&task_id) {
            return Err(ApiError::new(
                "IMPORT_CLEANUP_BLOCKED",
                "导入任务编号无效。",
            ));
        }
        self.owned_operation(move |manager| async move {
            let task = manager.tasks.task(&task_id)?;
            if task.task_type != "import" || !task.status.is_final() {
                return Err(ApiError::new(
                    "IMPORT_CLEANUP_BLOCKED",
                    "导入任务尚未结束，请等待后再清理。",
                ));
            }
            let mut operation = manager.operations.lock().map_err(|_| {
                ApiError::new("LIFECYCLE_STATE_FAILED", "Current operation ownership is unavailable")
            })?.get(&task_id).cloned().ok_or_else(|| {
                ApiError::new("IMPORT_CLEANUP_BLOCKED", "本次运行已无该导入的临时目录归属。请重新核对遗留目录；不会根据旧任务自动删除文件。")
            })?;
            if operation.task_id != task_id
                || Some(operation.project_id.as_str()) != task.project_id.as_deref()
            {
                return Err(ApiError::new(
                    "IMPORT_CLEANUP_BLOCKED",
                    "导入记录身份不符，未删除文件。",
                ));
            }
            let _lease = manager
                .store
                .reserve_transient_operation(&operation.project_id, "import temporary cleanup")?;
            let worker = manager.clone();
            tokio::task::spawn_blocking(move || {
                let cleanup = worker.cleanup_import_payload(&operation)?;
                operation.result["cleanupPending"] = json!(cleanup["completed"] != true);
                operation.result["cleanup"] = cleanup.clone();
                worker.remember(&operation)?;
                Ok(cleanup)
            })
            .await
            .map_err(|error| ApiError::new("IMPORT_CLEANUP_FAILED", error.to_string()))?
        })
        .await
    }
    pub(crate) fn request_stop(&self) {
        let _submission = self.running.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.running.lock().unwrap());
        crate::shutdown::join_all("lifecycle", runners).await
    }
}

fn validated_export_destination(
    paths: &RuntimePaths,
    store: &ProjectStore,
    path: &Path,
    source: &Path,
) -> Result<PathBuf, ApiError> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ApiError::new(
            "INVALID_EXPORT_PATH",
            "Export destination must be an absolute file path without parent traversal",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::new("INVALID_EXPORT_PATH", "Export destination has no parent"))?
        .canonicalize()?;
    let target = parent.join(path.file_name().ok_or_else(|| {
        ApiError::new("INVALID_EXPORT_PATH", "Export destination needs a filename")
    })?);
    check_no_symlinks(&target, true)?;
    if target.starts_with(source)
        || target.starts_with(paths.data_root())
        || target.starts_with(paths.workspace_root())
    {
        return Err(ApiError::new(
            "INVALID_EXPORT_PATH",
            "Export destination cannot be inside a project or platform-managed data",
        ));
    }
    for protected in [
        paths.data_root().to_path_buf(),
        paths.workspace_root().to_path_buf(),
    ] {
        if protected.try_exists()? {
            let identity = persistent_identity(&protected)?;
            if parent
                .ancestors()
                .any(|ancestor| identity.matches_path(ancestor).unwrap_or(false))
            {
                return Err(ApiError::new(
                    "INVALID_EXPORT_PATH",
                    "Export destination cannot be inside platform-managed data",
                ));
            }
        }
    }
    let workspace = crate::workspace::info(paths, store)?;
    for root in workspace["retainedRoots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["path"].as_str())
        .map(Path::new)
    {
        if target.starts_with(root)
            || (root.try_exists()?
                && parent.ancestors().any(|ancestor| {
                    persistent_identity(root)
                        .is_ok_and(|identity| identity.matches_path(ancestor).unwrap_or(false))
                }))
        {
            return Err(ApiError::new(
                "INVALID_EXPORT_PATH",
                "Export destination cannot be inside retained platform workspaces",
            ));
        }
    }
    crate::node_apps::development::require_export_location_unclaimed(paths.data_root(), &parent)?;
    crate::web_project_ownership::WebProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    )
    .require_global_tool_unowned(&target)?;
    if target.exists() {
        return Err(ApiError::new(
            "EXPORT_DESTINATION_EXISTS",
            "请选择新的导出文件名，现有文件不能覆盖。",
        )
        .detail("path", json!(target)));
    }
    Ok(target)
}
fn write_owned_export_stage(
    ownership: &crate::web_project_ownership::WebProjectOwnership,
    project: &str,
    task: &str,
    archive: &Path,
    stage: &Path,
    validate: impl Fn() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let mut output = crate::node_apps::development::with_source_ownership_admission(|| {
        validate()?;
        ownership.reserve_export_stage(project, task, stage)?;
        let output = match platform_native::filesystem::create_new_private_file(stage, true) {
            Ok(file) => file,
            Err(error) => {
                ownership.forget_export_stage(project, task)?;
                return Err(error.into());
            }
        };
        let identity = PersistedIdentity::from_file(&output)
            .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))?;
        ownership.capture_export_stage(project, task, stage, &identity)?;
        // The consent was captured before snapshot preparation. A replacement
        // parent must be rejected again before the first project payload byte.
        validate()?;
        Ok(output)
    })?;
    let mut input = File::open(archive)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    sync_parent(stage)
}
fn publish_export_file(
    stage: &Path,
    destination: &Path,
    expected: &PersistedIdentity,
) -> Result<(), ApiError> {
    publish_export_file_with(stage, destination, expected, |_| Ok(()))
}

use crate::lifecycle_file_io::publish_export_file_with;
#[cfg(all(test, unix))]
use crate::lifecycle_file_io::ExportPublicationPoint;

#[cfg(all(test, unix))]
#[path = "lifecycle_export_publication_tests.rs"]
mod export_publication_tests;

fn clear_project_provenance(workspace: &Path, id: &str) -> Result<(), ApiError> {
    let path = workspace
        .join(".platform/project-state/builds")
        .join(format!("{id}.json"));
    check_no_symlinks(&path, true)?;
    if path.exists() {
        fs::remove_file(&path)?;
        sync_parent(&path)?;
    }
    Ok(())
}
fn validate_install_snapshot(
    root: &Path,
    project: &ProjectManifest,
    metadata: &BackupManifest,
) -> Result<backups::DatabaseDiagnostics, ApiError> {
    let diagnostics = backups::check_database(&root.join(&project.database))?;
    if !matches!(metadata.snapshot_state, SnapshotState::Ready)
        || metadata.pending_build.required
        || !diagnostics.foreign_key_violations.is_empty()
    {
        return Err(ApiError::new(
            "LEGACY_ARCHIVE_REQUIRES_REVIEW",
            "旧归档处于维护、来源状态未知、仍需构建或存在数据库关系问题，当前不能直接安装为新实例。原归档和候选已保留；请先检查来源实例，未创建 ZIP 基线或清除待构建要求。",
        )
        .outcome("not_started")
        .detail("candidatePath", json!(root))
        .detail("snapshotState", json!(metadata.snapshot_state))
        .detail("pendingBuild", json!(metadata.pending_build))
        .detail("diagnostics", json!(diagnostics)));
    }
    Ok(diagnostics)
}
fn initialize_candidate(
    workspace: &Path,
    root: &Path,
    final_root: &Path,
    project: &ProjectManifest,
    metadata: &BackupManifest,
    task: &str,
) -> Result<(ControlState, backups::DatabaseDiagnostics), ApiError> {
    validate_manifest(project, &project.id, root)?;
    let diagnostics = validate_install_snapshot(root, project, metadata)?;
    backups::restore_provenance(root, workspace, project, metadata.build_provenance.as_ref())?;
    let input_digest = builds::digest_project_inputs(root, project)?;
    let state = ControlState {
        baseline_input_digest: Some(input_digest),
        had_artifact: metadata.has_artifact,
        pending_build: metadata.pending_build.required,
        pending_build_reason: metadata.pending_build.reason.clone(),
        last_task_id: Some(task.into()),
        ..ControlState::default()
    };
    fs::create_dir_all(root.join(".platform"))?;
    let handoff = root.join(".platform/handoff.md");
    let mut file = platform_native::filesystem::create_new_private_file(&handoff, false)?;
    write!(file,"# Platform-managed project handoff\n\nProject ID: `{}`\nProject root: `{}`\nRead the unchanged `README.md`, project skill and bundled documentation before editing.\nSource project: `{}`\n",project.id,final_root.display(),metadata.project_id)?;
    file.sync_all()?;
    Ok((state, diagnostics))
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

fn deletion_task_receipt(task_id: &str, project_id: &str, completed: bool) -> TaskRecord {
    TaskRecord { task_id:task_id.into(),project_id:Some(project_id.into()),task_type:"delete".into(),status:if completed {TaskStatus::Succeeded}else{TaskStatus::Running},phase:if completed {"completed"}else{"deleting-project-records"}.into(),result:completed.then(||json!({"projectId":project_id,"operationOutcome":"deleted","alreadyAbsent":true,"restoreAvailable":false})),error:None }
}

/// Losing an IPC waiter never owns retirement. An unwinding worker reports its
/// current task as failed before removing the temporary running projection.
struct DeleteOperationRetirement {
    manager: Arc<LifecycleManager>,
    task_id: String,
    accepted: Option<TaskRecord>,
    armed: bool,
}
impl DeleteOperationRetirement {
    fn new(
        manager: Arc<LifecycleManager>,
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
