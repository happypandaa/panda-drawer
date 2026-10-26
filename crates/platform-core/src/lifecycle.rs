//! Single-project lifecycle transactions. Candidate directories and durable
//! intent live in the workspace; external ZIPs are never modified by import.
use crate::{
    backups::{
        self, BackupManifest, PendingBuild, PreparedSnapshot, SnapshotOptions, SnapshotState,
    },
    builds::{self, BuildManager},
    control::{ControlState, Freeze},
    file_identity::PersistedIdentity,
    maintenance::MaintenanceManager,
    projects::{check_no_symlinks, valid_id, validate_manifest, ProjectTaskLease},
    ApiError, ProjectManifest, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    task_id: String,
    kind: String,
    project_id: String,
    source_project_id: Option<String>,
    // Retired recycle-bin and operating-system trash journals are recognized
    // only so a foreign record is reported instead of misread. This manager
    // never creates, restores, purges or adopts them.
    trash_id: Option<String>,
    #[serde(default)]
    system_trash: bool,
    #[serde(default)]
    system_trash_path: Option<PathBuf>,
    target_epoch: String,
    phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory_name: Option<String>,
    directory_identity: Option<PersistedIdentity>,
    #[serde(default)]
    cleanup_temporary_files: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import_temporary_identity: Option<PersistedIdentity>,
    control: Option<ControlState>,
    destination: Option<PathBuf>,
    published_sha256: Option<String>,
    committed: bool,
    resolved: bool,
    result: Value,
    error: Option<ApiError>,
}

pub(crate) struct LifecycleManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    running: Mutex<Vec<JoinHandle<()>>>,
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
    stopping: AtomicBool,
    events: broadcast::Sender<String>,
}

fn identity(path: &Path) -> Result<(u64, u64), ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected an ordinary project directory",
        ));
    }
    Ok((metadata.dev(), metadata.ino()))
}
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
/// Journals bind the target before publication. Older journals retain their
/// original ID directory even if a later project has a different registration.
fn journal_project_root(
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
            "Invalid journal project directory binding",
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
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
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
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            paths,
            store,
            tasks,
            maintenance,
            running: Mutex::new(vec![]),
            #[cfg(test)]
            import_inspection_pause: Mutex::new(None),
            #[cfg(test)]
            import_cleanup_pause: Mutex::new(None),
            stopping: AtomicBool::new(false),
            events,
        });
        manager.recover()?;
        Ok(manager)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
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
    fn journal_path(&self, task: &str) -> PathBuf {
        self.operation(task).join("lifecycle.json")
    }
    fn save(&self, journal: &Journal) -> Result<(), ApiError> {
        builds::atomic_json(&self.journal_path(&journal.task_id), journal)
    }
    fn finish_journal(&self, journal: &mut Journal) -> Result<(), ApiError> {
        journal.resolved = true;
        if let Err(error) = self.save(journal) {
            journal.resolved = false;
            return Err(self
                .store
                .control(&journal.project_id)?
                .mark_recovery_required(
                    error
                        .outcome("needs_attention")
                        .detail("recoveryStage", "lifecycle-journal")
                        .detail("recoveryTaskId", journal.task_id.clone()),
                ));
        }
        Ok(())
    }
    fn clear_completion_gate(&self, journal: &Journal) -> Result<(), ApiError> {
        let control = self.store.control(&journal.project_id)?;
        let mut current = control.get();
        let owned = current.last_error.as_ref().is_some_and(|error| {
            error.details["recoveryStage"] == "lifecycle-journal"
                && error.details["recoveryTaskId"] == journal.task_id
        });
        if current.recovery_required && owned {
            // Only undo this exact failed checkpoint's gate. Preserve newer
            // pending-build metadata and other current state.
            if let Some(saved) = &journal.control {
                current.mode = saved.mode.clone();
                current.ready_for_changes = saved.ready_for_changes;
            }
            current.recovery_required = false;
            current.last_error = journal
                .control
                .as_ref()
                .and_then(|saved| saved.last_error.clone());
            control.set(current)?;
        }
        Ok(())
    }
    fn phase(&self, journal: &mut Journal, phase: &str) -> Result<(), ApiError> {
        journal.phase = phase.into();
        self.save(journal)?;
        self.tasks.phase(&journal.task_id, phase)
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
    fn journal(record: &TaskRecord, kind: &str, project: &str) -> Journal {
        Journal {
            schema_version: 1,
            task_id: record.task_id.clone(),
            kind: kind.into(),
            project_id: project.into(),
            source_project_id: None,
            trash_id: None,
            system_trash: false,
            system_trash_path: None,
            target_epoch: Uuid::new_v4().to_string(),
            phase: "intent".into(),
            directory_name: None,
            directory_identity: None,
            cleanup_temporary_files: false,
            import_temporary_identity: None,
            control: None,
            destination: None,
            published_sha256: None,
            committed: false,
            resolved: false,
            result: json!({}),
            error: None,
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
        let (mut record, source_lease) = if kind == "delete" {
            self.tasks.create_delete_operation(id)?
        } else {
            self.tasks.create_operation(id, kind)?
        };
        let target = if kind == "copy" {
            format!("prj_{}", Uuid::new_v4().simple())
        } else {
            id.into()
        };
        let mut journal = Self::journal(&record, kind, &target);
        journal.source_project_id = Some(id.into());
        journal.destination = input.destination_path.clone();
        if kind != "copy" {
            journal.directory_name = Some(root_directory_name(&root)?);
        }
        journal.directory_identity = deletion.map(|(_, expected)| expected);
        let mut leases = vec![source_lease];
        let prepare = (|| {
            if kind == "delete" {
                self.authorize_delete(id, input.maintenance_id.as_deref())?;
                self.store
                    .validate_deletion_epoch(id, input.expected_document_epoch.as_deref())?;
            } else {
                self.authorize(id, input.maintenance_id.as_deref())?;
            }
            self.save(&journal)?;
            if kind == "copy" {
                leases.push(self.store.reserve_task(&target, &record.task_id)?);
                self.store.reserve_identity(&target, &record.task_id)?;
                record.result = Some(json!({"targetProjectId":target}));
            }
            Ok::<(), ApiError>(())
        })();
        if let Err(error) = prepare {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        self.launch(&mut running, journal, input, leases);
        Ok(record)
    }
    fn authorize_delete(
        &self,
        id: &str,
        maintenance: Option<&str>,
    ) -> Result<ControlState, ApiError> {
        crate::source_restore_guard::check_development_available(self.paths.workspace_root(), id)?;
        crate::data_restore_guard::check_development_available(self.paths.workspace_root(), id)?;
        let state = self.store.control(id)?.get();
        if state.recovery_required {
            let owned = self.store.pending_deletion(id)?.is_some_and(|marker| {
                state.last_error.as_ref().is_some_and(|error| {
                    error.details["recoveryStage"] == "lifecycle-journal"
                        && error.details["recoveryTaskId"] == marker.task_id
                })
            });
            if owned {
                return Ok(state);
            }
        }
        self.authorize(id, maintenance)
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
        // Ordinary imports create an independent local instance. Preserving an
        // archive's identity is an explicit recovery choice, never an update.
        let mode = input.mode.as_deref().unwrap_or("copy");
        if !matches!(mode, "original" | "copy") {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "mode must be original or copy",
            ));
        }
        if mode == "original" && input.name.is_some() {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Original import preserves the archive name",
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
        let mode = input.mode.as_deref().unwrap_or("copy");
        let target = if mode == "copy" {
            format!("prj_{}", Uuid::new_v4().simple())
        } else {
            metadata.project_id.clone()
        };
        self.store.check_identity_available(&target, None)?;
        let (mut record, lease) = self.tasks.create_operation(&target, "import")?;
        let mut journal = Self::journal(&record, "import", &target);
        journal.source_project_id = Some(metadata.project_id);
        journal.cleanup_temporary_files = input.cleanup_temporary_files;
        let prepared = self
            .save(&journal)
            .and_then(|()| self.store.reserve_identity(&target, &record.task_id));
        if let Err(error) = prepared {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        record.result = Some(json!({"targetProjectId":target}));
        if journal.cleanup_temporary_files {
            let temporary = self.import_temporary(&journal);
            let prepared = (|| {
                check_no_symlinks(&temporary, true)?;
                fs::create_dir(&temporary)?;
                journal.import_temporary_identity = Some(persistent_identity(&temporary)?);
                self.save(&journal)
            })();
            if let Err(error) = prepared {
                self.tasks.finish(&record.task_id, Err(error.clone()))?;
                return Err(error.detail("taskId", record.task_id.clone()));
            }
        }
        self.launch(&mut running, journal, input, vec![lease]);
        Ok(record)
    }
    fn launch(
        self: &Arc<Self>,
        running: &mut Vec<JoinHandle<()>>,
        mut journal: Journal,
        input: Input,
        leases: Vec<ProjectTaskLease>,
    ) {
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            let mut result = manager.run(&mut journal, input).await;
            if let Err(error) = &result {
                let _ = manager.tasks.append(
                    &journal.task_id,
                    format!("{}: {}\n", error.code, error.message).as_bytes(),
                );
                journal.error = Some(error.clone());
                let _ = manager.save(&journal);
                match manager.reconcile(&mut journal) {
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
                        let _ = manager.tasks.append(
                            &journal.task_id,
                            format!("Lifecycle recovery needs attention: {recovery}\n").as_bytes(),
                        );
                        result = Err(error
                            .clone()
                            .outcome("needs_attention")
                            .detail("recoveryError", json!(recovery)));
                    }
                }
            }
            let _ = manager.tasks.finish(&journal.task_id, result);
            manager.notify(&journal.project_id);
            if let Some(source) = &journal.source_project_id {
                manager.notify(source);
            }
            drop(leases);
        });
        running.push(runner);
    }
    async fn run(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        match journal.kind.as_str() {
            "export" => self.export(journal, input).await,
            "copy" => self.copy(journal, input).await,
            "import" => self.import(journal, input).await,
            "delete" if journal.trash_id.is_none() => self.delete(journal, input).await,
            _ => Err(ApiError::new(
                "INVALID_INPUT",
                "Unknown lifecycle journal kind",
            )),
        }
    }
    fn export_destination(
        &self,
        path: &Path,
        source: &Path,
        overwrite: bool,
    ) -> Result<PathBuf, ApiError> {
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(ApiError::new(
                "INVALID_EXPORT_PATH",
                "Export destination must be an absolute file path without parent traversal",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| {
                ApiError::new("INVALID_EXPORT_PATH", "Export destination has no parent")
            })?
            .canonicalize()?;
        let target = parent.join(path.file_name().ok_or_else(|| {
            ApiError::new("INVALID_EXPORT_PATH", "Export destination needs a filename")
        })?);
        check_no_symlinks(&target, true)?;
        if target.starts_with(source)
            || ["projects", "backups", ".platform"]
                .iter()
                .any(|p| target.starts_with(self.paths.workspace_root().join(p)))
        {
            return Err(ApiError::new(
                "INVALID_EXPORT_PATH",
                "Export destination cannot be inside a project or platform-managed data",
            ));
        }
        if target.exists() && (!target.is_file() || !overwrite) {
            return Err(ApiError::new(
                "EXPORT_DESTINATION_EXISTS",
                "Choose a new filename or explicitly request overwrite",
            )
            .detail("path", json!(target)));
        }
        Ok(target)
    }
    async fn snapshot(
        &self,
        journal: &mut Journal,
        source: &str,
    ) -> Result<PreparedSnapshot, ApiError> {
        self.phase(journal, "snapshot-freeze")?;
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
        let destination = self.operation(&journal.task_id).join("snapshot");
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
    async fn export(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        let source = journal.project_id.clone();
        let root = self.store.metadata_root(&source)?.0;
        let destination = self.export_destination(
            input.destination_path.as_deref().unwrap(),
            &root,
            input.overwrite,
        )?;
        journal.destination = Some(destination.clone());
        let snapshot = self.snapshot(journal, &source).await?;
        let stage = destination
            .parent()
            .unwrap()
            .join(format!(".export-{}.partial", journal.task_id));
        let staged = stage.clone();
        let snapshot_root = snapshot.root.clone();
        self.phase(journal, "archive-verify")?;
        let info =
            tokio::task::spawn_blocking(move || backups::publish_snapshot(&snapshot, &staged))
                .await
                .map_err(|e| ApiError::new("EXPORT_FAILED", e.to_string()))??;
        journal.published_sha256 = Some(sha256(&stage)?);
        journal.result = json!({"projectId":source,"path":destination,"backupId":info.manifest.backup_id,"createdAt":info.manifest.created_at,"snapshotState":info.manifest.snapshot_state,"pendingBuild":info.manifest.pending_build,"verified":true,"diagnostics":info.database,"operationOutcome":"exported"});
        self.phase(journal, "export-publish")?;
        self.export_destination(&destination, &root, input.overwrite)?;
        if input.overwrite {
            fs::rename(&stage, &destination)?;
        } else {
            fs::hard_link(&stage, &destination)?;
            fs::remove_file(&stage)?;
        }
        sync_parent(&destination)?;
        journal.committed = true;
        journal.resolved = true;
        self.save(journal)?;
        if let Err(error) = fs::remove_dir_all(snapshot_root) {
            let _ = self.tasks.append(
                &journal.task_id,
                format!("Temporary snapshot cleanup deferred: {error}\n").as_bytes(),
            );
        }
        Ok(journal.result.clone())
    }
    async fn copy(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        let source = journal.source_project_id.clone().unwrap();
        let snapshot = self.snapshot(journal, &source).await?;
        let candidate = self.operation(&journal.task_id).join("candidate");
        move_directory(&snapshot.root.join("project"), &candidate)?;
        let mut manifest: ProjectManifest = read_json(&candidate.join("project.json"))?;
        manifest.id = journal.project_id.clone();
        manifest.name = input
            .name
            .unwrap_or_else(|| format!("{} · 副本", manifest.name));
        builds::atomic_json(&candidate.join("project.json"), &manifest)?;
        self.prepare_install(journal, candidate, manifest, snapshot.manifest)
            .await
    }
    async fn import(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        self.phase(journal, "archive-copy")?;
        let payload = if journal.cleanup_temporary_files {
            self.import_temporary(journal)
        } else {
            self.operation(&journal.task_id)
        };
        let archived = payload.join("source.zip");
        let from = input.archive_path.unwrap();
        let to = archived.clone();
        tokio::task::spawn_blocking(move || copy_verified(&from, &to))
            .await
            .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        self.phase(journal, "archive-validate")?;
        let candidate = payload.join("candidate");
        let path = archived.clone();
        let destination = candidate.clone();
        let original_id = journal.source_project_id.clone().unwrap();
        let validated = tokio::task::spawn_blocking(move || {
            backups::extract_validated(&path, &destination, Some(&original_id))
        })
        .await
        .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        let mut manifest = validated.project_manifest;
        let original = manifest.id == journal.project_id;
        if !original {
            manifest.id = journal.project_id.clone();
            manifest.name = input
                .name
                .unwrap_or_else(|| format!("{} · 副本", manifest.name));
            builds::atomic_json(&candidate.join("project.json"), &manifest)?;
        }
        self.prepare_install(journal, candidate, manifest, validated.manifest)
            .await?;
        if journal.cleanup_temporary_files {
            self.phase(journal, "cleanup")?;
            #[cfg(test)]
            {
                let pause = self.import_cleanup_pause.lock().unwrap().take();
                if let Some((entered, release)) = pause {
                    let _ = entered.send(());
                    let _ = release.await;
                }
            }
            let cleanup = self.cleanup_import_payload(journal)?;
            journal.result["cleanupPending"] = json!(cleanup["completed"] != true);
            journal.result["cleanup"] = cleanup;
            self.save(journal)?;
        }
        Ok(journal.result.clone())
    }
    async fn prepare_install(
        &self,
        journal: &mut Journal,
        candidate: PathBuf,
        manifest: ProjectManifest,
        metadata: BackupManifest,
    ) -> Result<Value, ApiError> {
        validate_manifest(&manifest, &journal.project_id, &candidate)?;
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
            &journal.project_id,
            &journal.task_id,
            &manifest.name,
        )?;
        journal.directory_name = Some(root_directory_name(&target)?);
        self.save(journal)?;
        self.phase(journal, "candidate-initialize")?;
        let workspace = self.paths.workspace_root().to_owned();
        let root = candidate.clone();
        let project = manifest.clone();
        let task = journal.task_id.clone();
        let final_root = target.clone();
        let (state, diagnostics) = tokio::task::spawn_blocking(move || {
            initialize_candidate(&workspace, &root, &final_root, &project, &metadata, &task)
        })
        .await
        .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        journal.directory_identity = Some(persistent_identity(&candidate)?);
        journal.control = Some(state.clone());
        journal.result = json!({"projectId":journal.project_id,"targetProjectId":journal.project_id,"sourceProjectId":journal.source_project_id,"documentEpoch":journal.target_epoch,"mode":state.mode,"maintenanceId":state.maintenance_id,"readyForChanges":state.ready_for_changes,"baselineBackupId":state.baseline_backup_id,"pendingBuild":{"required":state.pending_build,"reason":state.pending_build_reason},"diagnostics":diagnostics,"operationOutcome":if journal.kind=="copy"{"copied"}else{"imported"}});
        self.phase(journal, "install-intent")?;
        self.store.control(&journal.project_id)?.set(state)?;
        clear_project_provenance(self.paths.workspace_root(), &journal.project_id)?;
        move_directory(&candidate, &target)?;
        self.store.register_lifecycle(
            &journal.project_id,
            &journal.target_epoch,
            &journal.task_id,
            false,
        )?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &journal.project_id)?;
        journal.committed = true;
        self.finish_journal(journal)?;
        self.notify(&journal.project_id);
        Ok(journal.result.clone())
    }
    async fn delete(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        self.phase(journal, "delete-freeze")?;
        let id = journal.project_id.clone();
        let control = self.store.control(&id)?;
        let _freeze = Freeze::new(control.clone());
        self.notify(&id);
        let gate = self.store.gate(&id);
        let _guard = gate.write().await;
        self.store.close_database(&id).await?;
        crate::source_restore_guard::check_development_available(self.paths.workspace_root(), &id)?;
        crate::data_restore_guard::check_development_available(self.paths.workspace_root(), &id)?;
        self.check_protection(&id, Some(&journal.task_id))?;
        let (root, expected) = self.store.deletion_root(&id)?;
        if root
            != journal_project_root(
                self.paths.workspace_root(),
                &id,
                journal.directory_name.as_deref(),
            )?
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory binding changed after deletion was requested",
            ));
        }
        self.store
            .validate_deletion_epoch(&id, input.expected_document_epoch.as_deref())?;
        if !journal
            .directory_identity
            .as_ref()
            .is_some_and(|identity| identity.matches_captured(&expected))
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Project directory changed after deletion was requested; no files were removed",
            ));
        }
        journal.control = Some(control.get());
        self.phase(journal, "delete-prepared")?;
        self.store
            .begin_deletion(&id, &journal.task_id, &journal.target_epoch, &expected)?;
        self.phase(journal, "delete-intent")?;
        if root.try_exists()? {
            let progress = self
                .operation(&journal.task_id)
                .join("delete-progress.ndjson");
            tokio::task::spawn_blocking(move || {
                crate::history_cleanup::remove_owned_directory(&root, &expected, &progress)
                    .map(|_| ())
            })
            .await
            .map_err(|error| ApiError::new("PROJECT_DELETE_FAILED", error.to_string()))??;
        }
        self.finish_direct_delete(journal)
    }
    fn finish_direct_delete(&self, journal: &mut Journal) -> Result<Value, ApiError> {
        let marker = self
            .store
            .pending_deletion(&journal.project_id)?
            .ok_or_else(|| {
                ApiError::new(
                    "PROJECT_IDENTITY_CHANGED",
                    "Deletion ownership marker is missing",
                )
            })?;
        if marker.task_id != journal.task_id
            || marker.target_epoch != journal.target_epoch
            || !journal
                .directory_identity
                .as_ref()
                .is_some_and(|identity| identity.matches_captured(&marker.root_identity))
        {
            return Err(ApiError::new(
                "PROJECT_IDENTITY_CHANGED",
                "Another operation owns the deletion target; its files and state were retained",
            ));
        }
        self.store.retire_deleted_project(&marker)?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &journal.project_id)?;
        journal.committed = true;
        journal.result = json!({"projectId":journal.project_id,"operationOutcome":"deleted","restoreAvailable":false});
        self.finish_journal(journal)?;
        self.clear_completion_gate(journal)?;
        self.store.finish_deletion_marker(
            &journal.project_id,
            &journal.task_id,
            &journal.target_epoch,
        )?;
        Ok(journal.result.clone())
    }
    fn reconcile_direct_delete(&self, journal: &mut Journal) -> Result<Value, ApiError> {
        let marker = self.store.pending_deletion(&journal.project_id)?;
        let owned = marker.as_ref().is_some_and(|marker| {
            marker.task_id == journal.task_id
                && marker.target_epoch == journal.target_epoch
                && journal
                    .directory_identity
                    .as_ref()
                    .is_some_and(|identity| identity.matches_captured(&marker.root_identity))
        });
        if journal.resolved {
            // Removing a completed marker is bookkeeping only. A later instance
            // or retry's marker must never be touched by historical recovery.
            if journal.committed && owned {
                self.store.finish_deletion_marker(
                    &journal.project_id,
                    &journal.task_id,
                    &journal.target_epoch,
                )?;
            }
            return Ok(journal.result.clone());
        }
        if journal.committed && marker.is_none() && journal.result["operationOutcome"] == "deleted"
        {
            // The durable completion may outlive a later same-ID import. Its
            // result is historical; it must not reset the new owner's state.
            journal.resolved = true;
            self.save(journal)?;
            return Ok(journal.result.clone());
        }
        let mut outcome = "not_deleted";
        if owned {
            match self.store.deletion_root(&journal.project_id) {
                Ok((root, _)) if !root.try_exists()? => return self.finish_direct_delete(journal),
                Ok(_) => outcome = "delete_incomplete",
                Err(error) => {
                    journal.error = Some(error);
                    outcome = "needs_attention";
                }
            }
        } else if marker.is_some() || journal.phase == "delete-intent" {
            // An old journal is not authority over a later same-ID instance.
            // In particular, do not call ambiguous(), which changes its control.
            outcome = "needs_attention";
        }
        journal.result = json!({"projectId":journal.project_id,"operationOutcome":outcome,"restoreAvailable":false,
                    "remainingPath":journal_project_root(self.paths.workspace_root(), &journal.project_id, journal.directory_name.as_deref())?,
        });
        journal.resolved = true;
        self.save(journal)?;
        Ok(journal.result.clone())
    }
    fn check_protection(&self, project: &str, except_task: Option<&str>) -> Result<(), ApiError> {
        let directory = self.paths.workspace_root().join(".platform/operations");
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_name().to_str() == except_task {
                continue;
            }
            for name in ["journal.json", "lifecycle.json"] {
                let path = entry.path().join(name);
                if !path.exists() {
                    continue;
                }
                let value: Value = read_json(&path)?;
                if value["resolved"] != true
                    && (value["projectId"] == project || value["sourceProjectId"] == project)
                {
                    return Err(ApiError::new(
                        "PROJECT_OPERATION_PROTECTED",
                        "Another unresolved operation still references this project",
                    )
                    .detail("taskId", value["taskId"].clone()));
                }
            }
        }
        Ok(())
    }
    fn recover(&self) -> Result<(), ApiError> {
        for entry in fs::read_dir(self.paths.workspace_root().join(".platform/operations"))? {
            let path = entry?.path().join("lifecycle.json");
            if !path.is_file() {
                continue;
            }
            let mut journal: Journal = read_json(&path)?;
            if journal.schema_version != 1
                || !valid_id(&journal.project_id)
                || !valid_id(&journal.task_id)
                || path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|v| v.to_str())
                    != Some(journal.task_id.as_str())
                || Uuid::parse_str(&journal.target_epoch).is_err()
            {
                return Err(ApiError::new(
                    "RECOVERY_REQUIRED",
                    "Invalid lifecycle journal identity",
                ));
            }
            let mut result = self.reconcile(&mut journal)?;
            result["recovered"] = true.into();
            self.tasks
                .reconcile_operation_result(&journal.task_id, result)?;
        }
        Ok(())
    }
    fn reconcile(&self, journal: &mut Journal) -> Result<Value, ApiError> {
        // These records have different deletion semantics. In particular, even
        // resolved records must not inspect or retire a direct-delete marker.
        if journal.system_trash
            || journal.system_trash_path.is_some()
            || journal.phase.starts_with("system-trash")
            || journal.trash_id.is_some()
            || matches!(journal.kind.as_str(), "trashRestore" | "trashPurge")
        {
            return if journal.resolved {
                Ok(journal.result.clone())
            } else {
                Err(ApiError::new(
                    "RECOVERY_REQUIRED",
                    "A retained recycle-bin operation from a retired version is not supported; all files and records were preserved",
                )
                .detail("taskId", journal.task_id.clone()))
            };
        }
        if journal.kind == "delete" {
            return self.reconcile_direct_delete(journal);
        }
        if journal.resolved {
            return Ok(journal.result.clone());
        }
        let root = journal_project_root(
            self.paths.workspace_root(),
            &journal.project_id,
            journal.directory_name.as_deref(),
        )?;
        let expected = journal.directory_identity.clone();
        let matches = |path: &Path| {
            expected
                .as_ref()
                .is_some_and(|expected| matches_identity(expected, path).unwrap_or(false))
        };
        let mut outcome = "not_started";
        match journal.kind.as_str() {
            "export" => {
                let published = journal
                    .destination
                    .as_ref()
                    .zip(journal.published_sha256.as_ref())
                    .is_some_and(|(path, digest)| {
                        sha256(path).is_ok_and(|actual| &actual == digest)
                    });
                if published {
                    journal.committed = true;
                } else {
                    outcome = "not_published";
                }
            }
            "import" | "copy" => {
                if matches(&root) {
                    if !self
                        .store
                        .project_root(&journal.project_id)
                        .is_ok_and(|registered| registered == root)
                    {
                        return self.ambiguous(journal, "Registered project directory differs from the install journal; all copies were preserved");
                    }
                    if self.store.registered_epoch(&journal.project_id)?.as_deref()
                        != Some(&journal.target_epoch)
                    {
                        self.store.control(&journal.project_id)?.set(
                            journal.control.clone().ok_or_else(|| {
                                ApiError::new(
                                    "RECOVERY_REQUIRED",
                                    "Install journal lacks its prepared control state",
                                )
                            })?,
                        )?;
                        self.store.register_lifecycle(
                            &journal.project_id,
                            &journal.target_epoch,
                            &journal.task_id,
                            false,
                        )?;
                    }
                    journal.committed = true;
                    MaintenanceManager::invalidate_storage(
                        self.paths.workspace_root(),
                        &journal.project_id,
                    )?;
                } else if !root.exists() {
                    self.store
                        .release_identity(&journal.project_id, &journal.task_id)?;
                    outcome = "not_registered";
                } else {
                    return self.ambiguous(
                        journal,
                        "Target project identity differs from the validated candidate",
                    );
                }
            }
            _ => {
                return Err(ApiError::new(
                    "RECOVERY_REQUIRED",
                    "Unknown lifecycle journal kind",
                ))
            }
        }
        if !journal.committed {
            journal.result = json!({"projectId":journal.project_id,"operationOutcome":outcome});
        } else {
            self.clear_completion_gate(journal)?;
        }
        self.finish_journal(journal)?;
        Ok(journal.result.clone())
    }
    fn ambiguous(&self, journal: &mut Journal, message: &str) -> Result<Value, ApiError> {
        let error = self
            .store
            .control(&journal.project_id)?
            .mark_recovery_required(
                ApiError::new("RECOVERY_REQUIRED", message).outcome("needs_attention"),
            );
        journal.error = Some(error.clone());
        self.save(journal)?;
        Ok(
            json!({"projectId":journal.project_id,"operationOutcome":"needs_attention","error":error}),
        )
    }
    fn import_temporary(&self, journal: &Journal) -> PathBuf {
        self.operation(&journal.task_id).join("import-temporary")
    }
    fn cleanup_import_payload(&self, journal: &Journal) -> Result<Value, ApiError> {
        if journal.kind != "import"
            || !journal.cleanup_temporary_files
            || journal.import_temporary_identity.is_none()
            || !journal.resolved
            || !matches!(
                journal.result["operationOutcome"].as_str(),
                Some("imported" | "not_registered" | "not_started")
            )
        {
            return Err(ApiError::new(
                "IMPORT_CLEANUP_BLOCKED",
                "导入结果尚未确认，未清理临时材料。",
            ));
        }
        if journal.result["operationOutcome"] == "imported" {
            let root = journal_project_root(
                self.paths.workspace_root(),
                &journal.project_id,
                journal.directory_name.as_deref(),
            )?;
            if !journal.committed
                || !journal
                    .directory_identity
                    .as_ref()
                    .is_some_and(|expected| matches_identity(expected, &root).unwrap_or(false))
                || !self
                    .store
                    .project_root(&journal.project_id)
                    .is_ok_and(|registered| registered == root)
                || self.store.registered_epoch(&journal.project_id)?.as_deref()
                    != Some(journal.target_epoch.as_str())
                || self
                    .store
                    .control(&journal.project_id)?
                    .get()
                    .recovery_required
            {
                return Err(ApiError::new(
                    "IMPORT_CLEANUP_BLOCKED",
                    "已导入项目的登记仍需核对，未清理恢复依据。",
                ));
            }
            self.check_protection(&journal.project_id, Some(&journal.task_id))?;
        } else if journal.committed {
            return Err(ApiError::new(
                "IMPORT_CLEANUP_BLOCKED",
                "安装提交和结果记录不一致，未删除文件。",
            ));
        }
        let temporary = self.import_temporary(journal);
        check_no_symlinks(&temporary, true)?;
        let mut removed = vec![];
        let mut failures = vec![];
        if temporary.try_exists()? {
            let expected = journal.import_temporary_identity.as_ref().ok_or_else(|| {
                ApiError::new(
                    "IMPORT_CLEANUP_BLOCKED",
                    "缺少本次临时目录的归属记录，未删除文件。",
                )
            })?;
            let progress = self
                .operation(&journal.task_id)
                .join(format!("import-cleanup-{}.jsonl", Uuid::new_v4().simple()));
            match crate::history_cleanup::remove_owned_directory(&temporary, expected, &progress) {
                Ok(_) => removed.push(temporary.clone()),
                Err(error) => failures.push(json!({"path":temporary,"error":error})),
            }
        }
        let result = json!({"completed":failures.is_empty() && !temporary.exists(),"projectId":journal.project_id,"operationOutcome":journal.result["operationOutcome"],"removedPaths":removed,"failures":failures});
        builds::atomic_json(
            &self.operation(&journal.task_id).join("import-cleanup.json"),
            &result,
        )?;
        Ok(result)
    }
    /// Only the exact opt-in import's process files are removable. Lifecycle
    /// journals and the caller's archive remain outside this bound directory.
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
            let mut journal: Journal = read_json(&manager.journal_path(&task_id))?;
            if journal.schema_version != 1
                || journal.task_id != task_id
                || Some(journal.project_id.as_str()) != task.project_id.as_deref()
            {
                return Err(ApiError::new(
                    "IMPORT_CLEANUP_BLOCKED",
                    "导入记录身份不符，未删除文件。",
                ));
            }
            let _lease = manager
                .store
                .reserve_transient_operation(&journal.project_id, "import temporary cleanup")?;
            let worker = manager.clone();
            tokio::task::spawn_blocking(move || {
                let cleanup = worker.cleanup_import_payload(&journal)?;
                journal.result["cleanupPending"] = json!(cleanup["completed"] != true);
                journal.result["cleanup"] = cleanup.clone();
                worker.save(&journal)?;
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
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&handoff)?;
    write!(file,"# Platform-managed project handoff\n\nProject ID: `{}`\nProject root: `{}`\nRead the unchanged `README.md`, project skill and bundled documentation before editing.\nSource project: `{}`\n",project.id,final_root.display(),metadata.project_id)?;
    file.sync_all()?;
    Ok((state, diagnostics))
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
