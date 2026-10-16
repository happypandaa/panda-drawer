//! Single-project lifecycle transactions. Candidate directories, trash and
//! durable intent live in the workspace; external ZIPs are never modified by import.
use crate::{
    backups::{
        self, BackupManifest, PendingBuild, PreparedSnapshot, SnapshotOptions, SnapshotState,
    },
    builds::{self, BuildManager},
    control::{ControlState, Freeze},
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
    time::{SystemTime, UNIX_EPOCH},
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
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    task_id: String,
    kind: String,
    project_id: String,
    source_project_id: Option<String>,
    trash_id: Option<String>,
    target_epoch: String,
    phase: String,
    directory_identity: Option<(u64, u64)>,
    control: Option<ControlState>,
    destination: Option<PathBuf>,
    published_sha256: Option<String>,
    committed: bool,
    resolved: bool,
    result: Value,
    error: Option<ApiError>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrashRecord {
    schema_version: u32,
    trash_id: String,
    project_id: String,
    name: String,
    deleted_at: u64,
    state: String,
    control: ControlState,
    root_identity: (u64, u64),
    delete_task_id: String,
    last_task_id: String,
    issues: Vec<ApiError>,
}

pub(crate) struct LifecycleManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    running: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
    events: broadcast::Sender<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
/// Rebind only verified trash directories in an independently verified workspace
/// copy. The caller persists this evidence before publishing the new workspace.
pub(crate) fn rebind_migrated_trash(source: &Path, target: &Path) -> Result<Value, ApiError> {
    if identity(source)? == identity(target)? {
        return Err(ApiError::new(
            "WORKSPACE_MIGRATION_FAILED",
            "Trash identities can only be rebound in an independent workspace copy",
        ));
    }
    let source_trash = source.join(".platform/trash");
    check_no_symlinks(&source_trash, true)?;
    if !source_trash.exists() {
        return Ok(json!({"schemaVersion":1,"entries":[]}));
    }
    let mut entries = fs::read_dir(&source_trash)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut evidence = Vec::new();
    for entry in entries {
        let trash_id = entry.file_name().to_string_lossy().into_owned();
        let relative = PathBuf::from(".platform/trash").join(&trash_id);
        let from = source.join(&relative).join("record.json");
        let to = target.join(&relative).join("record.json");
        if !valid_trash_id(&trash_id) || !from.is_file() {
            evidence.push(json!({"trashId":trash_id,"status":"unchanged","reason":"missing_or_invalid_record"}));
            continue;
        }
        let source_digest = match sha256(&from) {
            Ok(digest) => digest,
            Err(error) => {
                evidence.push(json!({"trashId":trash_id,"status":"unchanged","reason":"unreadable_record","error":error}));
                continue;
            }
        };
        let record: TrashRecord = match read_json(&from) {
            Ok(record) => record,
            Err(error) => {
                evidence.push(json!({"trashId":trash_id,"status":"unchanged","reason":"unreadable_record","error":error}));
                continue;
            }
        };
        if record.schema_version != 1
            || record.trash_id != trash_id
            || !valid_id(&record.project_id)
            || !active_trash(&record.state)
        {
            evidence.push(json!({"trashId":trash_id,"status":"unchanged","reason":"inactive_or_invalid_record"}));
            continue;
        }
        let old_actual = identity(&source.join(&relative).join("project")).ok();
        let new_actual = identity(&target.join(&relative).join("project")).ok();
        if old_actual != Some(record.root_identity) {
            // Do not turn a previously replaced or missing directory into an
            // authorized restore/purge target merely because it was copied.
            if new_actual == Some(record.root_identity) {
                return Err(ApiError::new(
                    "WORKSPACE_MIGRATION_FAILED",
                    "Copied trash would accidentally match an unverified historical identity",
                )
                .detail("trashId", trash_id));
            }
            evidence.push(json!({"trashId":trash_id,"status":"unchanged","reason":"source_identity_unverified","recordIdentity":record.root_identity,"sourceIdentity":old_actual,"targetIdentity":new_actual}));
            continue;
        }
        let new_actual = new_actual.ok_or_else(|| {
            ApiError::new(
                "WORKSPACE_MIGRATION_FAILED",
                "Verified source trash is missing from the copied workspace",
            )
            .detail("trashId", trash_id.clone())
        })?;
        let original: Value = read_json(&from)?;
        let copied: Value = read_json(&to)?;
        let mut rebound = original.clone();
        rebound["rootIdentity"] = json!(new_actual);
        if copied != original && copied != rebound {
            return Err(ApiError::new(
                "WORKSPACE_MIGRATION_FAILED",
                "Copied trash metadata differs beyond its authorized directory identity",
            )
            .detail("trashId", trash_id));
        }
        if copied != rebound {
            builds::atomic_json(&to, &rebound)?;
        }
        if sha256(&from)? != source_digest {
            return Err(ApiError::new(
                "WORKSPACE_MIGRATION_FAILED",
                "Source trash metadata changed during identity rebinding",
            )
            .detail("trashId", trash_id));
        }
        evidence.push(json!({"trashId":trash_id,"projectId":record.project_id,"status":"rebound","recordPath":relative.join("record.json"),"sourceIdentity":record.root_identity,"targetIdentity":new_actual,"sourceRecordSha256":source_digest,"targetRecordSha256":sha256(&to)?}));
    }
    Ok(json!({"schemaVersion":1,"entries":evidence}))
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
    fs::rename(from, to)?;
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
fn valid_trash_id(id: &str) -> bool {
    id.starts_with("trash_") && valid_id(id)
}
fn active_trash(state: &str) -> bool {
    matches!(
        state,
        "trashed" | "purging" | "purgeIncomplete" | "needsAttention"
    )
}

impl LifecycleManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        maintenance: Arc<MaintenanceManager>,
    ) -> Result<Arc<Self>, ApiError> {
        for relative in [".platform/operations", ".platform/trash"] {
            let path = paths.workspace_root().join(relative);
            check_no_symlinks(&path, true)?;
            fs::create_dir_all(path)?;
        }
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            paths,
            store,
            tasks,
            maintenance,
            running: Mutex::new(vec![]),
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
    fn trash_root(&self, id: &str) -> Result<PathBuf, ApiError> {
        if !valid_trash_id(id) {
            return Err(ApiError::new("INVALID_TRASH_ID", "Invalid trash ID"));
        }
        let path = self.paths.workspace_root().join(".platform/trash").join(id);
        check_no_symlinks(&path, true)?;
        Ok(path)
    }
    fn read_trash(&self, id: &str) -> Result<TrashRecord, ApiError> {
        let path = self.trash_root(id)?.join("record.json");
        if !path.is_file() {
            return Err(ApiError::new(
                "TRASH_NOT_FOUND",
                "Trash entry was not found",
            ));
        }
        let record: TrashRecord = read_json(&path)?;
        if record.schema_version != 1 || record.trash_id != id || !valid_id(&record.project_id) {
            return Err(ApiError::new(
                "TRASH_RECORD_INVALID",
                "Trash record identity is invalid",
            ));
        }
        Ok(record)
    }
    fn save_trash(&self, record: &TrashRecord) -> Result<(), ApiError> {
        builds::atomic_json(
            &self.trash_root(&record.trash_id)?.join("record.json"),
            record,
        )
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
            target_epoch: Uuid::new_v4().to_string(),
            phase: "intent".into(),
            directory_identity: None,
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
        self.check_running()?;
        let input = Self::input(value)?;
        if !matches!(kind, "export" | "copy" | "delete") {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "Unknown project lifecycle operation",
            ));
        }
        let (root, manifest) = self.store.metadata_root(id)?;
        self.authorize(id, input.maintenance_id.as_deref())?;
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
        let (mut record, source_lease) = self.tasks.create_operation(id, kind)?;
        let target = if kind == "copy" {
            format!("prj_{}", Uuid::new_v4().simple())
        } else {
            id.into()
        };
        let mut journal = Self::journal(&record, kind, &target);
        journal.source_project_id = Some(id.into());
        journal.destination = input.destination_path.clone();
        if kind == "delete" {
            journal.trash_id = Some(format!("trash_{}", Uuid::new_v4().simple()));
        }
        let mut leases = vec![source_lease];
        let prepare = (|| {
            self.authorize(id, input.maintenance_id.as_deref())?;
            self.save(&journal)?;
            if kind == "copy" {
                leases.push(self.store.reserve_task(&target, &record.task_id)?);
                self.store
                    .reserve_identity(&target, &record.task_id, None)?;
                record.result = Some(json!({"targetProjectId":target}));
            } else if let Some(trash) = &journal.trash_id {
                record.result = Some(json!({"trashId":trash}));
            }
            Ok::<(), ApiError>(())
        })();
        if let Err(error) = prepare {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        self.launch(journal, input, leases);
        Ok(record)
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
        let mode = input.mode.as_deref().unwrap_or("original");
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
        let metadata = tokio::task::spawn_blocking(move || backups::inspect_manifest(&archive))
            .await
            .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        let target = if mode == "copy" {
            format!("prj_{}", Uuid::new_v4().simple())
        } else {
            metadata.project_id.clone()
        };
        self.store.check_identity_available(&target, None, None)?;
        let (mut record, lease) = self.tasks.create_operation(&target, "import")?;
        let mut journal = Self::journal(&record, "import", &target);
        journal.source_project_id = Some(metadata.project_id);
        let prepared = self
            .save(&journal)
            .and_then(|()| self.store.reserve_identity(&target, &record.task_id, None));
        if let Err(error) = prepared {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        record.result = Some(json!({"targetProjectId":target}));
        self.launch(journal, input, vec![lease]);
        Ok(record)
    }
    pub async fn submit_trash(
        self: &Arc<Self>,
        trash_id: &str,
        kind: &str,
    ) -> Result<TaskRecord, ApiError> {
        self.check_running()?;
        let mut trash = self.read_trash(trash_id)?;
        if !matches!(kind, "restore" | "purge") {
            return Err(ApiError::new("INVALID_INPUT", "Unknown trash operation"));
        }
        if !active_trash(&trash.state) {
            return Err(ApiError::new(
                "TRASH_NOT_AVAILABLE",
                "Trash entry has already been restored or purged",
            ));
        }
        if kind == "restore" && trash.state != "trashed" {
            return Err(ApiError::new(
                "TRASH_INCOMPLETE",
                "Incomplete trash contents cannot be restored; inspect the retained entry",
            ));
        }
        self.store
            .check_identity_available(&trash.project_id, Some(trash_id), None)?;
        self.check_protection(&trash.project_id, trash_id, None)?;
        let task_kind = if kind == "restore" {
            "trashRestore"
        } else {
            "trashPurge"
        };
        let (mut record, lease) = self.tasks.create_operation(&trash.project_id, task_kind)?;
        let mut journal = Self::journal(&record, task_kind, &trash.project_id);
        journal.trash_id = Some(trash_id.into());
        journal.directory_identity = Some(trash.root_identity);
        journal.control = Some(trash.control.clone());
        let prepared = (|| {
            self.save(&journal)?;
            if kind == "restore" {
                self.store
                    .reserve_identity(&trash.project_id, &record.task_id, Some(trash_id))?;
            }
            trash.last_task_id = record.task_id.clone();
            self.save_trash(&trash)
        })();
        if let Err(error) = prepared {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            return Err(error);
        }
        record.result = Some(json!({"targetProjectId":trash.project_id,"trashId":trash_id}));
        self.launch(journal, Input::default(), vec![lease]);
        Ok(record)
    }
    fn launch(self: &Arc<Self>, mut journal: Journal, input: Input, leases: Vec<ProjectTaskLease>) {
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
        self.running.lock().unwrap().push(runner);
    }
    async fn run(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        match journal.kind.as_str() {
            "export" => self.export(journal, input).await,
            "copy" => self.copy(journal, input).await,
            "import" => self.import(journal, input).await,
            "delete" => self.delete(journal).await,
            "trashRestore" => self.restore_trash(journal).await,
            "trashPurge" => self.purge(journal).await,
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
        self.prepare_install(journal, candidate, manifest, snapshot.manifest, None)
            .await
    }
    async fn import(&self, journal: &mut Journal, input: Input) -> Result<Value, ApiError> {
        self.phase(journal, "archive-copy")?;
        let archived = self.operation(&journal.task_id).join("source.zip");
        let from = input.archive_path.unwrap();
        let to = archived.clone();
        tokio::task::spawn_blocking(move || copy_verified(&from, &to))
            .await
            .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        self.phase(journal, "archive-validate")?;
        let candidate = self.operation(&journal.task_id).join("candidate");
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
        self.prepare_install(
            journal,
            candidate,
            manifest,
            validated.manifest,
            original.then_some(archived),
        )
        .await
    }
    async fn prepare_install(
        &self,
        journal: &mut Journal,
        candidate: PathBuf,
        manifest: ProjectManifest,
        metadata: BackupManifest,
        source_archive: Option<PathBuf>,
    ) -> Result<Value, ApiError> {
        self.phase(journal, "candidate-initialize")?;
        let workspace = self.paths.workspace_root().to_owned();
        let root = candidate.clone();
        let project = manifest.clone();
        let task = journal.task_id.clone();
        let (state, baseline_error, diagnostics) = tokio::task::spawn_blocking(move || {
            initialize_candidate(
                &workspace,
                &root,
                &project,
                &metadata,
                source_archive.as_deref(),
                &task,
            )
        })
        .await
        .map_err(|e| ApiError::new("IMPORT_FAILED", e.to_string()))??;
        journal.directory_identity = Some(identity(&candidate)?);
        journal.control = Some(state.clone());
        journal.result = json!({"projectId":journal.project_id,"targetProjectId":journal.project_id,"sourceProjectId":journal.source_project_id,"documentEpoch":journal.target_epoch,"mode":state.mode,"maintenanceId":state.maintenance_id,"readyForChanges":state.ready_for_changes,"baselineBackupId":state.baseline_backup_id,"pendingBuild":{"required":state.pending_build,"reason":state.pending_build_reason},"diagnostics":diagnostics,"operationOutcome":if baseline_error.is_some(){"registered_maintenance_not_ready"}else if journal.kind=="copy"{"copied"}else{"imported"}});
        self.phase(journal, "install-intent")?;
        self.store.control(&journal.project_id)?.set(state)?;
        clear_project_provenance(self.paths.workspace_root(), &journal.project_id)?;
        let target = self
            .paths
            .workspace_root()
            .join("projects")
            .join(&journal.project_id);
        move_directory(&candidate, &target)?;
        self.store.register_lifecycle(
            &journal.project_id,
            &journal.target_epoch,
            &journal.task_id,
            false,
        )?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &journal.project_id)?;
        journal.committed = true;
        journal.error = baseline_error.clone();
        self.finish_journal(journal)?;
        self.notify(&journal.project_id);
        if let Some(error) = baseline_error {
            return Err(error
                .outcome("registered_maintenance_not_ready")
                .detail("projectId", journal.project_id.clone())
                .detail(
                    "maintenanceId",
                    json!(journal.control.as_ref().unwrap().maintenance_id),
                ));
        }
        Ok(journal.result.clone())
    }
    async fn delete(&self, journal: &mut Journal) -> Result<Value, ApiError> {
        self.phase(journal, "trash-freeze")?;
        let id = journal.project_id.clone();
        let control = self.store.control(&id)?;
        let freeze = Freeze::new(control.clone());
        self.notify(&id);
        let gate = self.store.gate(&id);
        let guard = gate.write().await;
        self.store.close_database(&id).await;
        let (root, manifest) = self.store.metadata_root(&id)?;
        let trash_id = journal.trash_id.clone().unwrap();
        let mut state = control.get();
        state.had_artifact |= manifest
            .as_ref()
            .is_some_and(|manifest| root.join(&manifest.entry).is_file());
        state.had_artifact |= root.join(".platform/starter.json").is_file()
            || root.join(".platform/restored-build.json").is_file()
            || self
                .paths
                .workspace_root()
                .join(".platform/project-state/builds")
                .join(format!("{id}.json"))
                .is_file();
        let record = TrashRecord {
            schema_version: 1,
            trash_id: trash_id.clone(),
            project_id: id.clone(),
            name: manifest.map(|m| m.name).unwrap_or_else(|| id.clone()),
            deleted_at: now_ms(),
            state: "trashed".into(),
            control: state.clone(),
            root_identity: identity(&root)?,
            delete_task_id: journal.task_id.clone(),
            last_task_id: journal.task_id.clone(),
            issues: vec![],
        };
        journal.directory_identity = Some(record.root_identity);
        journal.control = Some(state);
        journal.result = json!({"projectId":id,"trashId":trash_id,"operationOutcome":"trashed"});
        self.phase(journal, "trash-move-intent")?;
        self.save_trash(&record)?;
        let destination = self.trash_root(&trash_id)?.join("project");
        move_directory(&root, &destination)?;
        self.store
            .unregister_lifecycle(&id, &journal.target_epoch, Some(&trash_id))?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &id)?;
        journal.committed = true;
        self.finish_journal(journal)?;
        drop(guard);
        drop(freeze);
        self.notify(&id);
        Ok(journal.result.clone())
    }
    async fn restore_trash(&self, journal: &mut Journal) -> Result<Value, ApiError> {
        self.phase(journal, "trash-restore-check")?;
        let trash_id = journal.trash_id.clone().unwrap();
        let mut record = self.read_trash(&trash_id)?;
        let source = self.trash_root(&trash_id)?.join("project");
        if identity(&source)? != record.root_identity {
            return Err(ApiError::new(
                "TRASH_IDENTITY_CHANGED",
                "Trash project directory was replaced; original identity cannot be verified",
            ));
        }
        let mut state = record.control.clone();
        let root = source.clone();
        let id = record.project_id.clone();
        let diagnostic = self.operation(&journal.task_id).join("database-inspection");
        let had_artifact = state.had_artifact;
        let inspection = tokio::task::spawn_blocking(move || {
            inspect_restored_project(&root, &id, &diagnostic, had_artifact)
        })
        .await
        .map_err(|e| ApiError::new("TRASH_RESTORE_FAILED", e.to_string()))?;
        if let Err(error) = &inspection {
            if matches!(error.code.as_str(), "UNSAFE_PATH" | "UNSAFE_DATABASE_PATH") {
                return Err(error.clone().detail("trashId", trash_id));
            }
        }
        if let Err(error) = &inspection {
            state.mode = "maintenance".into();
            state
                .maintenance_id
                .get_or_insert_with(|| format!("maintenance_{}", Uuid::new_v4().simple()));
            state.ready_for_changes = false;
            state.last_error = Some(error.clone());
            state.reason = Some("Restored files need inspection or repair before use".into());
        }
        state.last_task_id = Some(journal.task_id.clone());
        journal.control = Some(state.clone());
        journal.result = json!({"projectId":record.project_id,"trashId":trash_id,"documentEpoch":journal.target_epoch,"mode":state.mode,"maintenanceId":state.maintenance_id,"readyForChanges":state.ready_for_changes,"issues":inspection.as_ref().err().into_iter().collect::<Vec<_>>(),"operationOutcome":"restored"});
        self.phase(journal, "trash-restore-intent")?;
        self.store.control(&record.project_id)?.set(state)?;
        let destination = self
            .paths
            .workspace_root()
            .join("projects")
            .join(&record.project_id);
        move_directory(&source, &destination)?;
        self.store.register_lifecycle(
            &record.project_id,
            &journal.target_epoch,
            &journal.task_id,
            true,
        )?;
        record.state = "restored".into();
        self.save_trash(&record)?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &record.project_id)?;
        journal.committed = true;
        self.finish_journal(journal)?;
        Ok(journal.result.clone())
    }
    fn check_protection(
        &self,
        project: &str,
        trash: &str,
        except_task: Option<&str>,
    ) -> Result<(), ApiError> {
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
                    && (value["projectId"] == project
                        || value["sourceProjectId"] == project
                        || value["trashId"] == trash)
                {
                    return Err(ApiError::new(
                        "TRASH_PROTECTED",
                        "Another unresolved operation still references this project or trash entry",
                    )
                    .detail("taskId", value["taskId"].clone()));
                }
            }
        }
        Ok(())
    }
    async fn purge(&self, journal: &mut Journal) -> Result<Value, ApiError> {
        let trash_id = journal.trash_id.clone().unwrap();
        self.check_protection(&journal.project_id, &trash_id, Some(&journal.task_id))?;
        self.store.check_identity_available(
            &journal.project_id,
            Some(&trash_id),
            Some(&journal.task_id),
        )?;
        let mut record = self.read_trash(&trash_id)?;
        let root = self.trash_root(&trash_id)?.join("project");
        if root.exists() && identity(&root)? != record.root_identity {
            return Err(ApiError::new(
                "TRASH_IDENTITY_CHANGED",
                "Trash contents were replaced; refusing to purge an unrecognized directory",
            ));
        }
        self.phase(journal, "purge-intent")?;
        record.state = "purging".into();
        record.issues.clear();
        self.save_trash(&record)?;
        let deleting = root.clone();
        let removal = tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
            check_no_symlinks(&deleting, true)?;
            if deleting.exists() {
                fs::remove_dir_all(&deleting)?;
            }
            sync_parent(&deleting)
        })
        .await
        .map_err(|e| ApiError::new("TRASH_PURGE_FAILED", e.to_string()))?;
        if let Err(error) = removal {
            record.state = "purgeIncomplete".into();
            record.issues.push(error.clone());
            self.save_trash(&record)?;
            journal.result = json!({"projectId":journal.project_id,"trashId":trash_id,"operationOutcome":"purge_incomplete","restoreAvailable":false,"remainingPath":root});
            journal.resolved = true;
            self.save(journal)?;
            return Err(error
                .outcome("purge_incomplete")
                .detail("trashId", trash_id));
        }
        self.finish_purge(journal, &mut record)?;
        Ok(journal.result.clone())
    }
    fn finish_purge(
        &self,
        journal: &mut Journal,
        record: &mut TrashRecord,
    ) -> Result<(), ApiError> {
        // The active-ID conflict was checked before deletion. Preserve all
        // backups and operation directories; only this trash owner's references end.
        self.store
            .control(&journal.project_id)?
            .set(ControlState::default())?;
        clear_project_provenance(self.paths.workspace_root(), &journal.project_id)?;
        self.store
            .unregister_lifecycle(&journal.project_id, &journal.target_epoch, None)?;
        record.state = "purged".into();
        record.control = ControlState::default();
        record.issues.clear();
        self.save_trash(record)?;
        MaintenanceManager::invalidate_storage(self.paths.workspace_root(), &journal.project_id)?;
        journal.result = json!({"projectId":journal.project_id,"trashId":record.trash_id,"operationOutcome":"purged","historicalBackupsRetained":true,"restoreAvailable":false});
        journal.control = Some(ControlState::default());
        journal.committed = true;
        self.finish_journal(journal)
    }
    /// A history cleanup owns the workspace gate. It supplies the exact,
    /// preview-bound file deletion; lifecycle intent and retirement stay here.
    pub(crate) fn cleanup_trash_with<F>(
        &self,
        trash_id: &str,
        parent_task_id: &str,
        remove: F,
    ) -> Result<Value, ApiError>
    where
        F: FnOnce(&Path) -> Result<Value, ApiError>,
    {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        if self.store.check_workspace_available().is_ok()
            || self.tasks.task(parent_task_id)?.task_type != "workspaceCleanup"
        {
            return Err(ApiError::new(
                "CLEANUP_NOT_EXCLUSIVE",
                "History cleanup must hold the workspace operation gate",
            ));
        }
        let mut trash = self.read_trash(trash_id)?;
        if !active_trash(&trash.state) {
            return Err(ApiError::new(
                "TRASH_NOT_AVAILABLE",
                "Trash entry is no longer available",
            ));
        }
        self.check_protection(&trash.project_id, trash_id, None)?;
        self.store
            .check_identity_available(&trash.project_id, Some(trash_id), None)?;
        let root = self.trash_root(trash_id)?.join("project");
        if identity(&root)? != trash.root_identity {
            return Err(ApiError::new(
                "TRASH_IDENTITY_CHANGED",
                "Trash directory was replaced after preview",
            ));
        }
        let task = self.tasks.create_workspace_operation("trashPurge")?;
        let mut journal = Self::journal(&task, "trashPurge", &trash.project_id);
        journal.trash_id = Some(trash_id.into());
        journal.directory_identity = Some(trash.root_identity);
        journal.control = Some(trash.control.clone());
        let result = (|| {
            self.save(&journal)?;
            trash.last_task_id = task.task_id.clone();
            trash.state = "purging".into();
            trash.issues.clear();
            self.save_trash(&trash)?;
            self.phase(&mut journal, "purge-intent")?;
            let removed = remove(&root)?;
            match fs::symlink_metadata(&root) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    return Err(ApiError::new(
                        "CLEANUP_INCOMPLETE",
                        "Trash payload still exists after deletion",
                    ))
                }
            }
            self.finish_purge(&mut journal, &mut trash)?;
            let mut result = removed.as_object().cloned().ok_or_else(|| {
                ApiError::new(
                    "CLEANUP_RESULT_INVALID",
                    "Deletion summary must be an object",
                )
            })?;
            result.insert("childTaskId".into(), json!(task.task_id));
            result.insert("lifecycle".into(), journal.result.clone());
            Ok(Value::Object(result))
        })();
        match &result {
            Ok(value) => self.tasks.finish(&task.task_id, Ok(value.clone()))?,
            Err(error) => {
                if trash.state != "purged" {
                    trash.state = "purgeIncomplete".into();
                    trash.issues.push(error.clone());
                    self.save_trash(&trash)?;
                    journal.error = Some(error.clone());
                    journal.resolved = true;
                    journal.result = json!({"projectId":trash.project_id,"trashId":trash_id,"operationOutcome":"purge_incomplete","remainingPath":root,"parentTaskId":parent_task_id});
                    self.save(&journal)?;
                }
                self.tasks.finish(&task.task_id, Err(error.clone()))?;
            }
        }
        result.map_err(|error| error.detail("childTaskId", task.task_id))
    }
    pub async fn trash_list(&self) -> Result<Value, ApiError> {
        let mut result = vec![];
        for entry in fs::read_dir(self.paths.workspace_root().join(".platform/trash"))? {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_trash_id(&id) {
                continue;
            }
            match self.read_trash(&id) {
                Ok(record) if active_trash(&record.state) => result.push(self.trash_info(&id).await?),
                Ok(_) => {},
                Err(error) => result.push(json!({"trashId":id,"state":"needsAttention","sizeBytes":null,"issues":[error]})),
            }
        }
        result.sort_by_key(|value| std::cmp::Reverse(value["deletedAt"].as_u64().unwrap_or(0)));
        Ok(json!(result))
    }
    pub async fn trash_info(&self, id: &str) -> Result<Value, ApiError> {
        let record = self.read_trash(id)?;
        let project = self.trash_root(id)?.join("project");
        let root = project.clone();
        let backups = self
            .paths
            .workspace_root()
            .join("backups")
            .join(&record.project_id);
        let (size, history) =
            tokio::task::spawn_blocking(move || (tree_size(&root), tree_size(&backups)))
                .await
                .map_err(|e| ApiError::new("TRASH_SCAN_FAILED", e.to_string()))?;
        let mut issues = record.issues;
        let size = match size {
            Ok(value) => Some(value),
            Err(error) => {
                issues.push(error);
                None
            }
        };
        let history = match history {
            Ok(value) => Some(value),
            Err(error) => {
                issues.push(error);
                None
            }
        };
        Ok(
            json!({"trashId":record.trash_id,"projectId":record.project_id,"name":record.name,"deletedAt":record.deleted_at,"sizeBytes":size,"historicalBackupBytes":history,"mode":record.control.mode,"maintenanceId":record.control.maintenance_id,"readyForChanges":record.control.ready_for_changes,"state":if record.state=="purging"{"purgeIncomplete"}else{&record.state},"issues":issues,"path":project,"lastTaskId":record.last_task_id,"restoreAvailable":record.state=="trashed"}),
        )
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
        if journal.resolved {
            return Ok(journal.result.clone());
        }
        let root = self
            .paths
            .workspace_root()
            .join("projects")
            .join(&journal.project_id);
        let expected = journal.directory_identity;
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
                if expected.is_some() && identity(&root).ok() == expected {
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
            "delete" => {
                let trash_id = journal.trash_id.as_deref().ok_or_else(|| {
                    ApiError::new("RECOVERY_REQUIRED", "Delete journal lacks trashId")
                })?;
                let retained = self.trash_root(trash_id)?.join("project");
                if expected.is_some() && identity(&retained).ok() == expected && !root.exists() {
                    self.store.unregister_lifecycle(
                        &journal.project_id,
                        &journal.target_epoch,
                        Some(trash_id),
                    )?;
                    let mut record = self.read_trash(trash_id)?;
                    record.state = "trashed".into();
                    self.save_trash(&record)?;
                    journal.committed = true;
                    MaintenanceManager::invalidate_storage(
                        self.paths.workspace_root(),
                        &journal.project_id,
                    )?;
                } else if !retained.exists()
                    && (expected.is_none() || identity(&root).ok() == expected)
                {
                    if let Ok(mut record) = self.read_trash(trash_id) {
                        record.state = "cancelled".into();
                        self.save_trash(&record)?;
                    }
                    outcome = "not_deleted";
                } else {
                    return self.ambiguous(journal,"Cannot determine whether the original project entered trash; both locations were retained");
                }
            }
            "trashRestore" => {
                let trash_id = journal.trash_id.as_deref().ok_or_else(|| {
                    ApiError::new("RECOVERY_REQUIRED", "Trash restore journal lacks trashId")
                })?;
                let retained = self.trash_root(trash_id)?.join("project");
                if expected.is_some() && identity(&root).ok() == expected && !retained.exists() {
                    if self.store.registered_epoch(&journal.project_id)?.as_deref()
                        != Some(&journal.target_epoch)
                    {
                        self.store.control(&journal.project_id)?.set(
                            journal.control.clone().ok_or_else(|| {
                                ApiError::new(
                                    "RECOVERY_REQUIRED",
                                    "Trash restore lacks saved control state",
                                )
                            })?,
                        )?;
                        self.store.register_lifecycle(
                            &journal.project_id,
                            &journal.target_epoch,
                            &journal.task_id,
                            true,
                        )?;
                    }
                    let mut record = self.read_trash(trash_id)?;
                    record.state = "restored".into();
                    self.save_trash(&record)?;
                    journal.committed = true;
                    MaintenanceManager::invalidate_storage(
                        self.paths.workspace_root(),
                        &journal.project_id,
                    )?;
                } else if !root.exists() && identity(&retained).ok() == expected {
                    self.store
                        .release_identity(&journal.project_id, &journal.task_id)?;
                    outcome = "not_restored";
                } else {
                    return self.ambiguous(journal,"Trash restore directory identity cannot be determined; all files were retained");
                }
            }
            "trashPurge" => {
                let trash_id = journal.trash_id.as_deref().ok_or_else(|| {
                    ApiError::new("RECOVERY_REQUIRED", "Purge journal lacks trashId")
                })?;
                let retained = self.trash_root(trash_id)?.join("project");
                let mut record = self.read_trash(trash_id)?;
                if record.state == "purged" {
                    // A later same-ID import may own the control/registration
                    // files now. This durable completion is only historical.
                    journal.committed = true;
                    journal.control = Some(ControlState::default());
                    journal.result = json!({"projectId":journal.project_id,"trashId":trash_id,"operationOutcome":"purged","historicalBackupsRetained":true,"restoreAvailable":false});
                } else if !retained.exists() && journal.phase == "purge-intent" {
                    self.finish_purge(journal, &mut record)?;
                } else if journal.phase == "purge-intent" {
                    record.state = "purgeIncomplete".into();
                    if let Some(error) = &journal.error {
                        record.issues.push(error.clone());
                    }
                    self.save_trash(&record)?;
                    outcome = "purge_incomplete";
                } else {
                    outcome = "not_purged";
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
            journal.result = json!({"projectId":journal.project_id,"trashId":journal.trash_id,"operationOutcome":outcome});
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
            json!({"projectId":journal.project_id,"trashId":journal.trash_id,"operationOutcome":"needs_attention","error":error}),
        )
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let runners = std::mem::take(&mut *self.running.lock().unwrap());
        for runner in runners {
            let _ = runner.await;
        }
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
fn tree_size(root: &Path) -> Result<u64, ApiError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(metadata.len());
    }
    let mut size = 0u64;
    for entry in fs::read_dir(root)? {
        size = size.saturating_add(tree_size(&entry?.path())?);
    }
    Ok(size)
}
fn inspect_restored_project(
    root: &Path,
    id: &str,
    diagnostic: &Path,
    had_artifact: bool,
) -> Result<backups::DatabaseDiagnostics, ApiError> {
    let manifest: ProjectManifest = read_json(&root.join("project.json"))?;
    validate_manifest(&manifest, id, root)?;
    if !root.join(&manifest.source_dir).is_dir() || !root.join(&manifest.attachments_dir).is_dir() {
        return Err(ApiError::new(
            "PROJECT_FILES_MISSING",
            "Project source or attachments directory is missing",
        ));
    }
    for key in ["readme", "database", "platform"] {
        let relative = manifest.docs[key].as_str().ok_or_else(|| {
            ApiError::new(
                "PROJECT_FILES_MISSING",
                "Declared documentation path is missing",
            )
        })?;
        if !crate::projects::checked_relative(root, relative, false)?.is_file() {
            return Err(ApiError::new(
                "PROJECT_FILES_MISSING",
                "Declared project documentation is missing",
            ));
        }
    }
    if had_artifact {
        let entry = Path::new(&manifest.entry)
            .strip_prefix(&manifest.output_dir)
            .map_err(|e| ApiError::new("INVALID_PROJECT_MANIFEST", e.to_string()))?;
        builds::validate_artifact(&root.join(&manifest.output_dir), entry.to_str().unwrap())?;
    }
    // SQLite may update its shared-memory companion even for read-only database
    // connections. Inspect an independent copy so every retained byte survives.
    fs::create_dir_all(diagnostic)?;
    let original = root.join(&manifest.database);
    if fs::metadata(&original)?.nlink() != 1 {
        return Err(ApiError::new(
            "UNSAFE_DATABASE_PATH",
            "A restored database must be an independent ordinary file",
        ));
    }
    let copied = diagnostic.join("database.sqlite");
    copy_verified(&original, &copied)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let source = PathBuf::from(format!("{}{suffix}", original.display()));
        if source.exists() {
            copy_verified(
                &source,
                &PathBuf::from(format!("{}{suffix}", copied.display())),
            )?;
        }
    }
    let diagnostics = backups::check_database(&copied)?;
    if !diagnostics.foreign_key_violations.is_empty() {
        return Err(ApiError::new(
            "DATABASE_FOREIGN_KEY_VIOLATION",
            "Restored database has foreign key issues",
        )
        .detail("diagnostics", json!(diagnostics)));
    }
    Ok(diagnostics)
}
fn initialize_candidate(
    workspace: &Path,
    root: &Path,
    project: &ProjectManifest,
    metadata: &BackupManifest,
    source_archive: Option<&Path>,
    task: &str,
) -> Result<(ControlState, Option<ApiError>, backups::DatabaseDiagnostics), ApiError> {
    validate_manifest(project, &project.id, root)?;
    let diagnostics = backups::check_database(&root.join(&project.database))?;
    backups::restore_provenance(root, workspace, project, metadata.build_provenance.as_ref())?;
    let input_digest = builds::digest_project_inputs(root, project)?;
    let matched = metadata
        .build_provenance
        .as_ref()
        .is_some_and(|provenance| provenance.input_digest == input_digest);
    let maintenance = !matches!(metadata.snapshot_state, SnapshotState::Ready)
        || !diagnostics.foreign_key_violations.is_empty();
    let unknown_build = maintenance && metadata.has_artifact && !matched;
    let mut state=ControlState {
        mode:if maintenance{"maintenance"}else{"ready"}.into(),
        maintenance_id:maintenance.then(||format!("maintenance_{}",Uuid::new_v4().simple())),
        reason:maintenance.then(||"Imported snapshot requires explicit maintenance checks".into()),
        started_at:maintenance.then(now_ms),baseline_input_digest:Some(input_digest),had_artifact:metadata.has_artifact,
        pending_build:metadata.pending_build.required || unknown_build,
        pending_build_reason:metadata.pending_build.reason.clone().or_else(||unknown_build.then(||"Existing artifact cannot be confirmed against current inputs; build before ending maintenance".into())),
        last_task_id:Some(task.into()),..ControlState::default()
    };
    fs::create_dir_all(root.join(".platform"))?;
    let handoff = root.join(".platform/handoff.md");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&handoff)?;
    write!(file,"# Platform-managed project handoff\n\nProject ID: `{}`\nProject root: `{}`\nRead the unchanged `README.md`, project skill and bundled documentation before editing.\nSource project: `{}`\n",project.id,workspace.join("projects").join(&project.id).display(),metadata.project_id)?;
    file.sync_all()?;
    let baseline = (|| -> Result<Option<String>, ApiError> {
        if !maintenance {
            return Ok(None);
        }
        let directory = workspace.join("backups").join(&project.id);
        check_no_symlinks(&directory, true)?;
        fs::create_dir_all(&directory)?;
        if let Some(source) = source_archive {
            let destination = directory.join(format!("{}.zip", metadata.backup_id));
            check_no_symlinks(&destination, true)?;
            if destination.exists() {
                if sha256(&destination)? != sha256(source)? {
                    return Err(ApiError::new(
                        "BACKUP_CONFLICT",
                        "A different historical archive already uses this backupId",
                    ));
                }
            } else {
                let staged = directory.join(format!(".import-{task}.partial"));
                copy_verified(source, &staged)?;
                fs::hard_link(&staged, &destination)?;
                fs::remove_file(staged)?;
                sync_parent(&destination)?;
            }
            return Ok(Some(metadata.backup_id.clone()));
        }
        let backup_id = format!("backup_{}", Uuid::new_v4().simple());
        let prepared = backups::prepare_snapshot(
            root,
            project,
            &workspace
                .join(".platform/operations")
                .join(task)
                .join("new-identity-baseline"),
            SnapshotOptions {
                backup_id: backup_id.clone(),
                kind: "maintenanceBaseline".into(),
                note: Some(format!(
                    "Independent identity copied from {}",
                    metadata.project_id
                )),
                snapshot_state: SnapshotState::Maintenance,
                pending_build: PendingBuild {
                    required: state.pending_build,
                    reason: state.pending_build_reason.clone(),
                },
                build_provenance: metadata.build_provenance.clone(),
            },
        )?;
        backups::publish_snapshot(&prepared, &directory.join(format!("{backup_id}.zip")))?;
        Ok(Some(backup_id))
    })();
    let baseline_error = match baseline {
        Ok(Some(id)) => {
            state.baseline_backup_id = Some(id.clone());
            state.protected_backup_ids.push(id);
            state.ready_for_changes = true;
            None
        }
        Ok(None) => None,
        Err(error) if error.code == "BACKUP_CONFLICT" => return Err(error),
        Err(error) => {
            state.last_error = Some(error.clone());
            Some(error)
        }
    };
    Ok((state, baseline_error, diagnostics))
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
