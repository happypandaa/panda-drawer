//! Backup/maintenance tasks and recoverable directory replacement.
use crate::{
    backups::{self, PendingBuild, SnapshotOptions, SnapshotState},
    builds::{self, BuildManager},
    control::{ControlState, Freeze},
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{sync::broadcast, task::JoinHandle};
use uuid::Uuid;

#[derive(Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperationInput {
    pub maintenance_id: Option<String>,
    pub backup_id: Option<String>,
    pub reason: Option<String>,
    pub note: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreJournal {
    task_id: String,
    project_id: String,
    backup_id: String,
    original_identity: PersistedIdentity,
    candidate_identity: PersistedIdentity,
    committed: bool,
    resolved: bool,
    final_control: Option<ControlState>,
    previous_control: ControlState,
    previous_provenance: Option<Value>,
}
fn identity(path: &Path) -> Result<(u64, u64), ApiError> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected ordinary project directory",
        ));
    }
    Ok((m.dev(), m.ino()))
}
fn persistent_identity(path: &Path) -> Result<PersistedIdentity, ApiError> {
    identity(path)?;
    PersistedIdentity::capture(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))
}
fn matches_identity(expected: &PersistedIdentity, path: &Path) -> bool {
    identity(path).is_ok() && expected.matches_path(path).unwrap_or(false)
}
fn sync_parent(path: &Path) -> Result<(), ApiError> {
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn move_dir(from: &Path, to: &Path) -> Result<(), ApiError> {
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    fs::rename(from, to)?;
    sync_parent(from)?;
    sync_parent(to)
}
fn read_optional(path: &Path) -> Result<Option<Value>, ApiError> {
    check_no_symlinks(path, true)?;
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn provenance_path(workspace: &Path, id: &str) -> PathBuf {
    workspace
        .join(".platform/project-state/builds")
        .join(format!("{id}.json"))
}
fn set_provenance(workspace: &Path, id: &str, value: &Option<Value>) -> Result<(), ApiError> {
    let path = provenance_path(workspace, id);
    if let Some(value) = value {
        builds::atomic_json(&path, value)
    } else {
        if path.exists() {
            fs::remove_file(&path)?;
            sync_parent(&path)?;
        }
        Ok(())
    }
}
pub(crate) struct MaintenanceManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    running: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
    events: broadcast::Sender<String>,
}
impl MaintenanceManager {
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
    ) -> Result<Arc<Self>, ApiError> {
        let (events, _) = broadcast::channel(64);
        let manager = Arc::new(Self {
            paths,
            store,
            tasks,
            running: Mutex::new(vec![]),
            stopping: AtomicBool::new(false),
            events,
        });
        for (task, outcome) in Self::recover(&manager.paths, &manager.store)? {
            let original = manager
                .paths
                .workspace_root()
                .join(".platform/operations")
                .join(&task)
                .join("original");
            let retained = identity(&original).is_ok();
            let mut result =
                json!({"operationOutcome":outcome,"recovered":true,"originalRetained":retained});
            if retained {
                result["originalPath"] = json!(original);
            }
            manager.tasks.reconcile_operation_result(&task, result)?;
        }
        manager.reconcile_snapshot_tasks()?;
        Ok(manager)
    }
    fn reconcile_snapshot_tasks(&self) -> Result<(), ApiError> {
        let directory = self.paths.workspace_root().join(".platform/operations");
        for entry in fs::read_dir(directory)? {
            let path = entry?.path().join("snapshot.json");
            if !path.is_file() {
                continue;
            }
            let Some(record) = read_optional(&path)? else {
                continue;
            };
            let Some(task) = record["taskId"].as_str() else {
                continue;
            };
            let Ok(task_record) = self.tasks.task(task) else {
                continue;
            };
            if task_record.status != crate::TaskStatus::Interrupted || task_record.result.is_some()
            {
                continue;
            }
            let id = record["projectId"].as_str().ok_or_else(|| {
                ApiError::new("RECOVERY_REQUIRED", "Snapshot journal lacks project ID")
            })?;
            let backup = record["backupId"].as_str().ok_or_else(|| {
                ApiError::new("RECOVERY_REQUIRED", "Snapshot journal lacks backup ID")
            })?;
            let verified = self
                .archive(id, backup)
                .and_then(|p| backups::read_info(&p))
                .is_ok();
            let outcome = if task_record.task_type == "backup" {
                if verified {
                    "committed"
                } else {
                    "not_published"
                }
            } else if task_record.task_type == "maintenanceBegin" {
                let state = self.store.control(id)?.get();
                if verified
                    && state.ready_for_changes
                    && state.baseline_backup_id.as_deref() == Some(backup)
                    && state.maintenance_id.as_deref() == record["maintenanceId"].as_str()
                {
                    "baseline_ready"
                } else {
                    "maintenance_not_ready"
                }
            } else {
                "not_switched"
            };
            self.tasks.reconcile_operation(task, outcome)?;
        }
        Ok(())
    }
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }
    pub(crate) fn notify(&self, id: &str) {
        let _ = self.events.send(id.into());
    }
    pub fn status(&self, id: &str) -> Result<Value, ApiError> {
        let control = self.store.control(id)?;
        let current = control.get();
        let available = current
            .baseline_backup_id
            .as_deref()
            .is_some_and(|backup| self.archive(id, backup).is_ok_and(|p| p.is_file()));
        let mut value = json!(current);
        value["baselineAvailable"] = json!(available);
        if current.mode == "maintenance" && current.ready_for_changes && !available {
            value["readyForChanges"] = json!(false);
            value["lastError"] = json!(ApiError::new(
                "BASELINE_MISSING",
                "Protected baseline is missing; retry preparation with this maintenance ID"
            ));
        }
        value["busy"] = json!(control.frozen.load(Ordering::Acquire));
        value["activeTask"] = json!(self.store.active_task(id));
        value["latestBuild"] = json!(self.tasks.latest_build(id));
        value["documentEpoch"] = json!(self.store.project(id).ok().map(|p| p.document_epoch));
        Ok(value)
    }
    pub fn browser_status(&self, id: &str, epoch: &str) -> Value {
        let status = self
            .status(id)
            .unwrap_or(json!({"mode":"maintenance","recoveryRequired":true}));
        json!({"mode":if status["mode"]=="ready"{json!("normal")}else{status["mode"].clone()},"busy":status["busy"],"recoveryRequired":status["recoveryRequired"],"reloadRequired":status["documentEpoch"].as_str()!=Some(epoch),"trashed":self.store.is_retired(id).ok().flatten().is_some(),"workspaceMigrating":self.store.workspace_paused()})
    }
    fn directory(&self, id: &str) -> Result<PathBuf, ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let path = self.paths.workspace_root().join("backups").join(id);
        check_no_symlinks(&path, true)?;
        Ok(path)
    }
    fn archive(&self, id: &str, backup: &str) -> Result<PathBuf, ApiError> {
        if !valid_id(backup) {
            return Err(ApiError::new("INVALID_BACKUP_ID", "Invalid backup ID"));
        }
        let path = self.directory(id)?.join(format!("{backup}.zip"));
        check_no_symlinks(&path, false)?;
        Ok(path)
    }
    fn protected(&self, id: &str) -> Result<HashSet<String>, ApiError> {
        let state = self.store.control(id)?.get();
        let mut protected: HashSet<_> = state.protected_backup_ids.into_iter().collect();
        if state.mode == "maintenance" {
            if let Some(id) = state.baseline_backup_id {
                protected.insert(id);
            }
        }
        Ok(protected)
    }
    pub async fn list(&self, id: &str) -> Result<Value, ApiError> {
        let directory = self.directory(id)?;
        let protected = self.protected(id)?;
        let project_id = id.to_owned();
        tokio::task::spawn_blocking(move||{
            let list=backups::list_backups(&directory)?;let mut items=vec![];
            for info in list.backups {
                let matches=info.manifest.project_id==project_id;
                let mut value=json!(info.manifest);
                value["database"]=json!(info.database);
                value["sizeBytes"]=json!(info.bytes);
                value["verified"]=json!(matches);
                if !matches{value["issues"]=json!(["Archive belongs to a different project"]);}
                value["protectionReasons"]=json!(if protected.contains(value["backupId"].as_str().unwrap_or("")){vec!["maintenance or recovery baseline"]}else{vec![]});items.push(value);
            }
            for issue in list.issues{items.push(json!({"backupId":issue.path.file_stem().and_then(|p|p.to_str()),"createdAt":null,"kind":"invalid","sizeBytes":fs::metadata(&issue.path).ok().map(|m|m.len()),"verified":false,"issues":[issue.error.message],"protectionReasons":["invalid archive retained"]}));}
            Ok(json!(items))
        }).await.map_err(|e|ApiError::new("BACKUP_FAILED",e.to_string()))?
    }
    pub async fn info(&self, id: &str, backup: &str) -> Result<Value, ApiError> {
        let path = self.archive(id, backup)?;
        let protected = self.protected(id)?.contains(backup);
        let project_id = id.to_owned();
        let backup_id = backup.to_owned();
        tokio::task::spawn_blocking(move || {
            let info = backups::read_info(&path)?;
            if info.manifest.project_id != project_id || info.manifest.backup_id != backup_id {
                return Err(ApiError::new(
                    "BACKUP_PROJECT_MISMATCH",
                    "Archive belongs to a different project",
                ));
            }
            let mut value = json!(info.manifest);
            value["database"] = json!(info.database);
            value["sizeBytes"] = json!(info.bytes);
            value["path"] = json!(info.path);
            value["verified"] = json!(true);
            value["protectionReasons"] = json!(if protected {
                vec!["maintenance or recovery baseline"]
            } else {
                vec![]
            });
            Ok(value)
        })
        .await
        .map_err(|e| ApiError::new("BACKUP_FAILED", e.to_string()))?
    }
    pub async fn submit(
        self: &Arc<Self>,
        id: &str,
        kind: &str,
        input: OperationInput,
    ) -> Result<TaskRecord, ApiError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let control = self.store.control(id)?;
        let current = control.get();
        if current.recovery_required {
            return Err(ApiError::new(
                "RECOVERY_REQUIRED",
                "Resolve the retained recovery journal before starting another operation",
            ));
        }
        match kind {
            "backup" => {
                self.store.project(id)?.check()?;
                control.check(false, input.maintenance_id.as_deref(), false)?;
            }
            "maintenanceBegin" => {
                self.store.project(id)?.check()?;
                if current.mode == "maintenance" {
                    if input.maintenance_id != current.maintenance_id {
                        return Err(ApiError::new(
                            "MAINTENANCE_ID_MISMATCH",
                            "Retry requires the current maintenanceId",
                        ));
                    }
                    if current.ready_for_changes
                        && current.baseline_backup_id.as_deref().is_some_and(|backup| {
                            self.archive(id, backup).is_ok_and(|p| p.is_file())
                        })
                    {
                        return Err(ApiError::new(
                            "PROJECT_MAINTENANCE",
                            "Maintenance baseline is already ready",
                        ));
                    }
                } else if input.maintenance_id.is_some() {
                    return Err(ApiError::new(
                        "MAINTENANCE_ID_MISMATCH",
                        "No maintenance session is active",
                    ));
                }
                if input.reason.as_ref().is_none_or(|s| s.trim().is_empty()) {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Maintenance reason is required",
                    ));
                }
            }
            "maintenanceEnd" => {
                if current.mode != "maintenance"
                    || input.maintenance_id.is_none()
                    || input.maintenance_id != current.maintenance_id
                {
                    return Err(ApiError::new(
                        "MAINTENANCE_ID_MISMATCH",
                        "End requires the current maintenanceId",
                    ));
                }
            }
            "restore" => {
                if current.mode == "maintenance" && input.maintenance_id != current.maintenance_id {
                    return Err(ApiError::new(
                        "MAINTENANCE_ID_MISMATCH",
                        "Restore requires the current maintenanceId",
                    ));
                }
                if current.mode == "ready" && input.maintenance_id.is_some() {
                    return Err(ApiError::new(
                        "MAINTENANCE_ID_MISMATCH",
                        "No maintenance session is active",
                    ));
                }
                self.archive(
                    id,
                    input
                        .backup_id
                        .as_deref()
                        .ok_or_else(|| ApiError::new("INVALID_INPUT", "backupId is required"))?,
                )?;
            }
            _ => return Err(ApiError::new("INVALID_INPUT", "Unknown task type")),
        }
        let (record, lease) = self.tasks.create_operation(id, kind)?;
        let now = control.get();
        if now.mode != current.mode
            || now.maintenance_id != current.maintenance_id
            || now.ready_for_changes != current.ready_for_changes
            || now.recovery_required != current.recovery_required
        {
            let error=ApiError::new("PROJECT_BUSY","Maintenance state changed while submitting; read the current state before retrying");
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            drop(lease);
            return Err(error);
        }
        let mut saved = control.get();
        saved.last_task_id = Some(record.task_id.clone());
        if let Err(error) = control.set(saved) {
            self.tasks.finish(&record.task_id, Err(error.clone()))?;
            drop(lease);
            return Err(error);
        }
        let task = record.task_id.clone();
        let id = id.to_owned();
        let kind = kind.to_owned();
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            let result = manager.run(&task, &id, &kind, input).await;
            if let Err(error) = &result {
                let _ = manager.tasks.append(
                    &task,
                    format!("{}: {}\n", error.code, error.message).as_bytes(),
                );
                if let Ok(control) = manager.store.control(&id) {
                    let mut current = control.get();
                    if current.mode == "maintenance" {
                        current.last_error = Some(error.clone());
                        let _ = control.set(current);
                    }
                }
            }
            if let Err(error) = manager.tasks.finish(&task, result) {
                eprintln!("Cannot persist maintenance task: {error}");
            }
            drop(lease);
            manager.notify(&id);
        });
        let mut running = self.running.lock().unwrap();
        running.retain(|h| !h.is_finished());
        running.push(runner);
        Ok(record)
    }
    fn phase(&self, task: &str, phase: &str) -> Result<(), ApiError> {
        self.tasks.phase(task, phase)?;
        self.tasks.append(task, format!("{phase}\n").as_bytes())
    }
    async fn run(
        &self,
        task: &str,
        id: &str,
        kind: &str,
        input: OperationInput,
    ) -> Result<Value, ApiError> {
        match kind {
            "backup" => {
                let backup = self.snapshot(task, id, "manual", input.note).await?;
                Ok(json!({"backupId":backup,"maintenance":self.status(id)?}))
            }
            "maintenanceBegin" => self.begin(task, id, input).await,
            "maintenanceEnd" => self.end(task, id).await,
            "restore" => self.restore(task, id, input.backup_id.unwrap()).await,
            _ => unreachable!(),
        }
    }
    async fn snapshot(
        &self,
        task: &str,
        id: &str,
        kind: &str,
        note: Option<String>,
    ) -> Result<String, ApiError> {
        self.phase(task, "snapshot-freeze")?;
        let control = self.store.control(id)?;
        let freeze = Freeze::new(control.clone());
        self.notify(id);
        let gate = self.store.gate(id);
        let guard = gate.write().await;
        let project = self.store.project(id)?;
        project.check()?;
        let state = control.get();
        let backup_id = format!("backup_{}", Uuid::new_v4().simple());
        let directory = self.directory(id)?;
        let archive = directory.join(format!("{backup_id}.zip"));
        let snapshot = self
            .paths
            .workspace_root()
            .join(".platform/operations")
            .join(task)
            .join(format!("snapshot-{backup_id}"));
        let info = builds::build_info(&self.paths, &project)?;
        let pending = state.pending_build
            || (state.mode == "maintenance"
                && info["sourceState"] != "matched"
                && state.baseline_input_digest.as_ref().is_some_and(|digest| {
                    builds::input_digest(&project).ok().as_ref() != Some(digest)
                }));
        let options = SnapshotOptions {
            backup_id: backup_id.clone(),
            kind: kind.into(),
            note,
            snapshot_state: if state.mode == "maintenance" {
                SnapshotState::Maintenance
            } else {
                SnapshotState::Ready
            },
            pending_build: PendingBuild {
                required: pending,
                reason: if pending {
                    Some("Maintenance includes unfinished build inputs".into())
                } else {
                    None
                },
            },
            build_provenance: backups::capture_provenance(&project)?,
        };
        builds::atomic_json(
            &snapshot.parent().unwrap().join("snapshot.json"),
            &json!({"taskId":task,"projectId":id,"backupId":backup_id,"maintenanceId":state.maintenance_id}),
        )?;
        let prepared = tokio::task::spawn_blocking(move || {
            backups::prepare_snapshot(&project.root, &project.manifest, &snapshot, options)
        })
        .await
        .map_err(|e| ApiError::new("BACKUP_FAILED", e.to_string()))??;
        drop(guard);
        drop(freeze);
        self.notify(id);
        self.phase(task, "archive-publish")?;
        let staging = prepared.root.clone();
        tokio::task::spawn_blocking(move || backups::publish_snapshot(&prepared, &archive))
            .await
            .map_err(|e| ApiError::new("BACKUP_FAILED", e.to_string()))??;
        if let Err(error) = fs::remove_dir_all(staging) {
            let _ = self.tasks.append(
                task,
                format!("Snapshot cleanup deferred: {error}\n").as_bytes(),
            );
        }
        Ok(backup_id)
    }
    async fn begin(&self, task: &str, id: &str, input: OperationInput) -> Result<Value, ApiError> {
        self.phase(task, "maintenance-intent")?;
        let control = self.store.control(id)?;
        if control.get().mode != "maintenance" {
            let project = self.store.project(id)?;
            let info = self.tasks.info(id).await?;
            control.begin(
                input.reason.unwrap(),
                Some(builds::input_digest(&project)?),
                info["sourceState"] != "unbuilt",
            )?;
            self.notify(id);
        }
        let backup = self
            .snapshot(task, id, "maintenanceBaseline", input.note)
            .await?;
        let mut state = control.get();
        state.baseline_backup_id = Some(backup.clone());
        state.protected_backup_ids.push(backup.clone());
        state.ready_for_changes = true;
        state.last_error = None;
        control.set(state)?;
        self.retention(task, id);
        Ok(
            json!({"maintenanceId":control.get().maintenance_id,"baselineBackupId":backup,"readyForChanges":true,"maintenance":self.status(id)?}),
        )
    }
    async fn end(&self, task: &str, id: &str) -> Result<Value, ApiError> {
        self.phase(task, "maintenance-check")?;
        let control = self.store.control(id)?;
        let freeze = Freeze::new(control.clone());
        self.notify(id);
        let gate = self.store.gate(id);
        let _guard = gate.write().await;
        let project = self.store.project(id)?;
        project.check()?;
        let database = project.root.join(&project.manifest.database);
        let diagnostics = tokio::task::spawn_blocking(move || backups::check_database(&database))
            .await
            .map_err(|e| ApiError::new("DATABASE_INVALID", e.to_string()))??;
        if !diagnostics.foreign_key_violations.is_empty() {
            return Err(ApiError::new(
                "DATABASE_FOREIGN_KEY_VIOLATION",
                "Foreign key violations must be repaired before ending maintenance",
            )
            .detail("diagnostics", json!(diagnostics)));
        }
        let info = self.tasks.info(id).await?;
        let digest = builds::input_digest(&project)?;
        let mut state = control.get();
        let changed = state
            .baseline_input_digest
            .as_deref()
            .is_none_or(|d| d != digest);
        let unbuilt = info["sourceState"] == "unbuilt" && !state.had_artifact;
        if state.had_artifact && info["sourceState"] == "unbuilt" {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_MISSING",
                "Previously runnable project has lost its webpage",
            ));
        }
        if !unbuilt {
            let entry = project
                .manifest
                .entry
                .strip_prefix(&format!("{}/", project.manifest.output_dir))
                .unwrap();
            builds::validate_artifact(&project.root.join(&project.manifest.output_dir), entry)?;
        }
        if (state.pending_build || changed) && !unbuilt && info["sourceState"] != "matched" {
            state.pending_build = true;
            state.pending_build_reason =
                Some("Build current inputs before ending maintenance".into());
            control.set(state)?;
            return Err(ApiError::new("BUILD_REQUIRED","Maintenance changed build inputs; build the current source before ending maintenance"));
        }
        if state.pending_build && !unbuilt {
            return Err(ApiError::new("BUILD_REQUIRED","The restored snapshot has an unfinished build requirement; complete a build before ending maintenance"));
        }
        state.pending_build = unbuilt && (state.pending_build || changed);
        state.pending_build_reason = if state.pending_build {
            Some("Project has no published webpage; build before opening".into())
        } else {
            None
        };
        self.phase(task, "maintenance-commit")?;
        let epoch = self.store.rebind(id, true)?;
        state.mode = "ready".into();
        state.maintenance_id = None;
        state.ready_for_changes = false;
        state.last_error = None;
        state.protected_backup_ids.clear();
        state.baseline_backup_id = None;
        control.set(state)?;
        drop(freeze);
        self.notify(id);
        self.retention(task, id);
        Ok(json!({"documentEpoch":epoch,"reloadRequired":true,"maintenance":self.status(id)?}))
    }
    fn retention(&self, task: &str, id: &str) {
        let result = self
            .directory(id)
            .and_then(|dir| backups::apply_retention(&dir, &self.protected(id)?));
        if let Err(error) = result {
            let _ = self
                .tasks
                .append(task, format!("Retention deferred: {error}\n").as_bytes());
        }
    }
    async fn restore(&self, task: &str, id: &str, backup: String) -> Result<Value, ApiError> {
        self.phase(task, "candidate-validation")?;
        let archive = self.archive(id, &backup)?;
        let op = self
            .paths
            .workspace_root()
            .join(".platform/operations")
            .join(task);
        check_no_symlinks(&op, true)?;
        fs::create_dir_all(&op)?;
        let candidate = op.join("candidate");
        let candidate_clone = candidate.clone();
        let id_owned = id.to_owned();
        let validated = tokio::task::spawn_blocking(move || {
            backups::extract_validated(&archive, &candidate_clone, Some(&id_owned))
        })
        .await
        .map_err(|e| ApiError::new("BACKUP_INVALID", e.to_string()))??;
        if validated.manifest.backup_id != backup {
            return Err(ApiError::new(
                "BACKUP_INVALID",
                "Backup file identity differs from the selected backupId",
            ));
        }
        // Build a new local generation without inventing a successful local build task.
        backups::restore_provenance(
            &candidate,
            self.paths.workspace_root(),
            &validated.project_manifest,
            validated.manifest.build_provenance.as_ref(),
        )?;
        // A healthy current project gets an automatic pre-restore archive. A broken
        // database must never prevent recovery; the original directory is retained in all cases.
        let pre_restore = match self
            .snapshot(
                task,
                id,
                "preRestore",
                Some(format!("Before restoring {backup}")),
            )
            .await
        {
            Ok(id) => Some(id),
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "SPACE_INSUFFICIENT" | "INSUFFICIENT_SPACE" | "BACKUP_SOURCE_CHANGED"
                ) =>
            {
                return Err(error)
            }
            Err(error) => {
                self.tasks.append(
                    task,
                    format!(
                        "Pre-restore archive unavailable; preserving complete original: {error}\n"
                    )
                    .as_bytes(),
                )?;
                None
            }
        };
        self.phase(task, "restore-maintenance")?;
        let control = self.store.control(id)?;
        if control.get().mode != "maintenance" {
            control.begin(format!("Restore {backup}"), None, false)?;
        }
        if let Some(backup) = pre_restore {
            let mut state = control.get();
            state.protected_backup_ids.push(backup);
            control.set(state)?;
        }
        let previous_control = control.get();
        let mut blocked = previous_control.clone();
        blocked.ready_for_changes = false;
        blocked.recovery_required = true;
        let freeze = Freeze::new(control.clone());
        self.notify(id);
        let gate = self.store.gate(id);
        let guard = gate.write().await;
        self.store.close_database(id).await;
        let root = self.paths.workspace_root().join("projects").join(id);
        let original = op.join("original");
        let journal_path = op.join("journal.json");
        let mut journal = RestoreJournal {
            task_id: task.into(),
            project_id: id.into(),
            backup_id: backup.clone(),
            original_identity: persistent_identity(&root)?,
            candidate_identity: persistent_identity(&candidate)?,
            committed: false,
            resolved: false,
            final_control: None,
            previous_control,
            previous_provenance: read_optional(&provenance_path(self.paths.workspace_root(), id))?,
        };
        builds::atomic_json(&journal_path, &journal)?;
        control.set(blocked)?;
        let result: Result<Value, ApiError> = (|| {
            self.phase(task, "preserve-original")?;
            move_dir(&root, &original)?;
            self.phase(task, "install-candidate")?;
            move_dir(&candidate, &root)?;
            // Existing workers have been joined. Rebind only after the candidate is in place.
            set_provenance(self.paths.workspace_root(), id, &None)?;
            self.store.rebind(id, true)?;
            let project = self.store.project(id)?;
            project.check()?;
            backups::validate_candidate(&root, &validated.manifest)?;
            let info = builds::build_info(&self.paths, &project)?;
            let digest = builds::input_digest(&project).ok();
            let mut state = control.get();
            state.recovery_required = false;
            state.ready_for_changes = true;
            state.baseline_backup_id = Some(backup.clone());
            if !state.protected_backup_ids.contains(&backup) {
                state.protected_backup_ids.push(backup.clone());
            }
            state.had_artifact = validated.manifest.has_artifact;
            state.baseline_input_digest = digest;
            state.pending_build = validated.manifest.pending_build.required;
            state.pending_build_reason = validated.manifest.pending_build.reason.clone();
            if !matches!(validated.manifest.snapshot_state, SnapshotState::Ready)
                && state.had_artifact
                && info["sourceState"] != "matched"
            {
                state.pending_build = true;
                state.pending_build_reason = Some(
                    "Restored maintenance/unknown snapshot needs a matching successful build"
                        .into(),
                );
            }
            state.last_error = None;
            journal.final_control = Some(state.clone());
            journal.committed = true;
            self.phase(task, "restore-commit")?;
            builds::atomic_json(&journal_path, &journal)?;
            control.set(state)?;
            Self::invalidate_storage(self.paths.workspace_root(), id)?;
            journal.resolved = true;
            builds::atomic_json(&journal_path, &journal)?;
            Ok(
                json!({"backupId":backup,"maintenanceId":control.get().maintenance_id,"readyForChanges":true,"operationOutcome":"restored","originalPath":original,"originalRetained":true,"maintenance":self.status(id)?,"diagnostics":validated.database}),
            )
        })();
        let result = result.map_err(|error| {
            match Self::reconcile_journal(&self.paths, &self.store, &journal_path) {
                Ok(outcome) => {
                    let _ = self
                        .tasks
                        .append(task, format!("Recovery outcome: {outcome}\n").as_bytes());
                    error
                        .outcome(&outcome)
                        .detail("originalPath", json!(original))
                }
                Err(recovery) => error
                    .outcome("needs_attention")
                    .detail("recoveryError", json!(recovery))
                    .detail("originalPath", json!(original)),
            }
        });
        drop(guard);
        drop(freeze);
        self.notify(id);
        result
    }
    pub(crate) fn invalidate_storage(workspace: &Path, id: &str) -> Result<(), ApiError> {
        if !valid_id(id) {
            return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
        }
        let invalidations = workspace.join(".platform/cache/storage/invalidations");
        check_no_symlinks(&invalidations, true)?;
        let token = json!({"generation":Uuid::new_v4().to_string()});
        builds::atomic_json(
            &invalidations.join("projects").join(format!("{id}.json")),
            &token,
        )?;
        builds::atomic_json(&invalidations.join("workspace.json"), &token)?;
        for path in [
            workspace
                .join(".platform/cache/storage/projects")
                .join(format!("{id}.json")),
            workspace.join(".platform/cache/storage/workspace.json"),
        ] {
            check_no_symlinks(&path, true)?;
            if path.is_file() {
                fs::remove_file(&path)?;
                sync_parent(&path)?;
            }
        }
        Ok(())
    }
    pub(crate) fn recover(
        paths: &RuntimePaths,
        store: &ProjectStore,
    ) -> Result<Vec<(String, String)>, ApiError> {
        let directory = paths.workspace_root().join(".platform/operations");
        check_no_symlinks(&directory, true)?;
        fs::create_dir_all(&directory)?;
        let mut outcomes = vec![];
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let journal_path = entry.path().join("journal.json");
            if !journal_path.is_file() {
                continue;
            }
            check_no_symlinks(&journal_path, false)?;
            let journal: RestoreJournal = serde_json::from_slice(&fs::read(&journal_path)?)?;
            // A crash can occur after resolving the journal but before the task
            // response is saved. Keep its outcome available without replaying it.
            let outcome = Self::reconcile_journal(paths, store, &journal_path)?;
            outcomes.push((journal.task_id, outcome));
        }
        Ok(outcomes)
    }
    fn reconcile_journal(
        paths: &RuntimePaths,
        store: &ProjectStore,
        path: &Path,
    ) -> Result<String, ApiError> {
        let mut journal: RestoreJournal = serde_json::from_slice(&fs::read(path)?)?;
        if !valid_id(&journal.project_id)
            || !valid_id(&journal.task_id)
            || path
                .parent()
                .and_then(Path::file_name)
                .and_then(|p| p.to_str())
                != Some(journal.task_id.as_str())
        {
            return Err(ApiError::new(
                "RECOVERY_REQUIRED",
                "Invalid restore journal identity",
            ));
        }
        if journal.resolved {
            // Historical completion must not rebind the current project or apply
            // old control state over later successful edits/maintenance sessions.
            return Ok(if journal.committed {
                "restored"
            } else {
                "rolled_back"
            }
            .into());
        }
        let root = paths
            .workspace_root()
            .join("projects")
            .join(&journal.project_id);
        let op = path.parent().unwrap();
        let candidate = op.join("candidate");
        let original = op.join("original");
        let control = store.control(&journal.project_id)?;
        let result = (|| {
            if journal.committed {
                if !matches_identity(&journal.candidate_identity, &root) {
                    return Err(ApiError::new(
                        "RECOVERY_REQUIRED",
                        "Committed restore root identity cannot be verified",
                    ));
                }
                store.rebind(&journal.project_id, true)?;
                let project = store.project(&journal.project_id)?;
                backups::check_database(&root.join(&project.manifest.database))?;
                builds::build_info(paths, &project)?;
                let state = journal.final_control.clone().ok_or_else(|| {
                    ApiError::new(
                        "RECOVERY_REQUIRED",
                        "Committed journal lacks final maintenance state",
                    )
                })?;
                control.set(state)?;
                Self::invalidate_storage(paths.workspace_root(), &journal.project_id)?;
                Ok("restored")
            } else {
                match (identity(&root).ok(), identity(&original).ok()) {
                    (Some(_), None) if matches_identity(&journal.original_identity, &root) => {}
                    (None, Some(_)) if matches_identity(&journal.original_identity, &original) => {
                        move_dir(&original, &root)?
                    }
                    (Some(_), Some(_))
                        if matches_identity(&journal.candidate_identity, &root)
                            && matches_identity(&journal.original_identity, &original)
                            && !candidate.exists() =>
                    {
                        move_dir(&root, &candidate)?;
                        move_dir(&original, &root)?;
                    }
                    _ => return Err(ApiError::new(
                        "RECOVERY_REQUIRED",
                        "Cannot identify an intact original root; all directories were retained",
                    )),
                }
                set_provenance(
                    paths.workspace_root(),
                    &journal.project_id,
                    &journal.previous_provenance,
                )?;
                store.rebind(&journal.project_id, true)?;
                let mut state = journal.previous_control.clone();
                state.mode = "maintenance".into();
                state.recovery_required = false;
                state.last_error = Some(ApiError::new(
                    "RESTORE_ROLLED_BACK",
                    "Interrupted restore rolled back to its preserved original",
                ));
                control.set(state)?;
                Ok("rolled_back")
            }
        })();
        match result {
            Ok(outcome) => Self::persist_reconciliation(path, &mut journal, &control, outcome),
            Err(error) => {
                control.mark_recovery_required(error.outcome("needs_attention"));
                Ok("needs_attention".into())
            }
        }
    }
    fn persist_reconciliation(
        path: &Path,
        journal: &mut RestoreJournal,
        control: &crate::control::ProjectControl,
        outcome: &str,
    ) -> Result<String, ApiError> {
        journal.resolved = true;
        if let Err(error) = builds::atomic_json(path, journal) {
            return Err(control.mark_recovery_required(
                error
                    .outcome("needs_attention")
                    .detail("recoveryStage", "resolve-journal"),
            ));
        }
        Ok(outcome.into())
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let runners = std::mem::take(&mut *self.running.lock().unwrap());
        for runner in runners {
            let _ = runner.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateProjectInput, SqlCaller, SqlRequest, TaskStatus};
    async fn fixture() -> (
        tempfile::TempDir,
        RuntimePaths,
        Arc<ProjectStore>,
        Arc<BuildManager>,
        Arc<MaintenanceManager>,
        String,
    ) {
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "M3 disposable unit".into(),
                description: None,
            })
            .await
            .unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let manager = MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
        (temp, paths, store, tasks, manager, project.id)
    }
    async fn perform(
        manager: &Arc<MaintenanceManager>,
        id: &str,
        kind: &str,
        input: Value,
    ) -> TaskRecord {
        let task = manager
            .submit(id, kind, serde_json::from_value(input).unwrap())
            .await
            .unwrap();
        for _ in 0..1000 {
            let record = manager.tasks.task(&task.task_id).unwrap();
            if record.status.is_final() {
                tokio::task::yield_now().await;
                return record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("task timeout");
    }
    fn good(task: &TaskRecord) -> &Value {
        assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
        task.result.as_ref().unwrap()
    }
    async fn stop(
        manager: Arc<MaintenanceManager>,
        tasks: Arc<BuildManager>,
        store: Arc<ProjectStore>,
    ) {
        manager.shutdown().await;
        tasks.shutdown().await;
        store.shutdown().await;
    }
    #[tokio::test]
    async fn unbuilt_maintenance_baseline_and_pending_inputs_survive_end() {
        let (_temp, _paths, store, tasks, manager, id) = fixture().await;
        let old = store.project(&id).unwrap().document_epoch;
        let begin = perform(
            &manager,
            &id,
            "maintenanceBegin",
            json!({"reason":"schema change"}),
        )
        .await;
        good(&begin);
        let state = store.control(&id).unwrap().get();
        assert!(state.ready_for_changes);
        let mid = state.maintenance_id.unwrap();
        let request: SqlRequest =
            serde_json::from_value(json!({"sql":"CREATE TABLE probe(value TEXT)"})).unwrap();
        assert_eq!(
            store
                .execute(&id, request.clone(), None, SqlCaller::Management)
                .await
                .unwrap_err()
                .code,
            "PROJECT_MAINTENANCE"
        );
        store
            .execute_managed(&id, request, None, SqlCaller::Management, Some(&mid))
            .await
            .unwrap();
        fs::write(
            store.project(&id).unwrap().root.join("app/src/new.ts"),
            "new source",
        )
        .unwrap();
        let end = perform(
            &manager,
            &id,
            "maintenanceEnd",
            json!({"maintenanceId":mid}),
        )
        .await;
        good(&end);
        let state = store.control(&id).unwrap().get();
        assert_eq!(state.mode, "ready");
        assert!(state.pending_build);
        assert_ne!(store.project(&id).unwrap().document_epoch, old);
        stop(manager, tasks, store).await;
    }
    #[tokio::test]
    async fn failed_baseline_stays_locked_and_retries_same_identity() {
        let (_temp, _paths, store, tasks, manager, id) = fixture().await;
        let root = store.project(&id).unwrap().root;
        std::os::unix::fs::symlink("README.md", root.join("invalid-link")).unwrap();
        let task = perform(
            &manager,
            &id,
            "maintenanceBegin",
            json!({"reason":"test baseline failure"}),
        )
        .await;
        assert_eq!(task.status, TaskStatus::Failed);
        let state = store.control(&id).unwrap().get();
        assert_eq!(state.mode, "maintenance");
        assert!(!state.ready_for_changes);
        assert_eq!(
            store
                .project(&id)
                .unwrap()
                .validate_epoch(Some(&store.project(&id).unwrap().document_epoch))
                .unwrap_err()
                .code,
            "PROJECT_MAINTENANCE"
        );
        fs::remove_file(root.join("invalid-link")).unwrap();
        let task = perform(
            &manager,
            &id,
            "maintenanceBegin",
            json!({"reason":"retry","maintenanceId":state.maintenance_id}),
        )
        .await;
        good(&task);
        assert_eq!(
            store.control(&id).unwrap().get().maintenance_id,
            state.maintenance_id
        );
        stop(manager, tasks, store).await;
    }
    #[tokio::test]
    async fn restore_bad_original_preserves_every_byte_and_unknown_ready_artifact() {
        let (_temp, paths, store, tasks, manager, id) = fixture().await;
        let root = store.project(&id).unwrap().root;
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(
            root.join("dist/index.html"),
            "<html><body>legacy unknown</body></html>",
        )
        .unwrap();
        fs::write(root.join("data/attachments/keep"), "old attachment").unwrap();
        let backup = perform(&manager, &id, "backup", json!({})).await;
        let backup = good(&backup)["backupId"].as_str().unwrap().to_owned();
        store.close_database(&id).await;
        fs::write(
            root.join("data/main.sqlite"),
            "broken original database retained",
        )
        .unwrap();
        fs::write(root.join("data/attachments/keep"), "new attachment").unwrap();
        let restore = perform(&manager, &id, "restore", json!({"backupId":backup})).await;
        let result = good(&restore);
        let original = PathBuf::from(result["originalPath"].as_str().unwrap());
        assert_eq!(
            fs::read(original.join("data/main.sqlite")).unwrap(),
            b"broken original database retained"
        );
        assert_eq!(
            fs::read(root.join("data/attachments/keep")).unwrap(),
            b"old attachment"
        );
        assert_eq!(
            builds::build_info(&paths, &store.project(&id).unwrap()).unwrap()["sourceState"],
            "unknown"
        );
        let mid = store.control(&id).unwrap().get().maintenance_id;
        let end = perform(
            &manager,
            &id,
            "maintenanceEnd",
            json!({"maintenanceId":mid}),
        )
        .await;
        good(&end);
        stop(manager, tasks, store).await;
    }
    #[tokio::test]
    async fn operation_conflicts_and_bad_archive_never_replace_original() {
        let (_temp, paths, store, tasks, manager, id) = fixture().await;
        let lease = store.reserve_task(&id, "task_busy").unwrap();
        let error = manager
            .submit(&id, "backup", OperationInput::default())
            .await
            .unwrap_err();
        assert_eq!(error.code, "PROJECT_BUSY");
        drop(lease);
        let root = store.project(&id).unwrap().root;
        let before = identity(&root).unwrap();
        let dir = paths.workspace_root().join("backups").join(&id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("backup_bad.zip"), b"not a ZIP").unwrap();
        let task = perform(&manager, &id, "restore", json!({"backupId":"backup_bad"})).await;
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(identity(&root).unwrap(), before);
        assert_eq!(store.control(&id).unwrap().get().mode, "ready");
        stop(manager, tasks, store).await;
    }
    #[tokio::test]
    async fn foreign_key_violations_are_archived_but_block_end() {
        let (_temp, _paths, store, tasks, manager, id) = fixture().await;
        let root = store.project(&id).unwrap().root;
        let conn = rusqlite::Connection::open(root.join("data/main.sqlite")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF; CREATE TABLE parent(id PRIMARY KEY);CREATE TABLE child(id REFERENCES parent(id));INSERT INTO child VALUES(5);").unwrap();
        drop(conn);
        let task = perform(
            &manager,
            &id,
            "maintenanceBegin",
            json!({"reason":"repair foreign keys"}),
        )
        .await;
        good(&task);
        let mid = store.control(&id).unwrap().get().maintenance_id;
        let task = perform(
            &manager,
            &id,
            "maintenanceEnd",
            json!({"maintenanceId":mid}),
        )
        .await;
        assert_eq!(task.error.unwrap().code, "DATABASE_FOREIGN_KEY_VIOLATION");
        assert_eq!(store.control(&id).unwrap().get().mode, "maintenance");
        stop(manager, tasks, store).await;
    }
    #[tokio::test]
    async fn restart_reconciles_directory_moves_and_stable_identity_device_renumbering() {
        for phase in 0..4 {
            let (_temp, paths, store, tasks, manager, id) = fixture().await;
            let backup = perform(&manager, &id, "backup", json!({})).await;
            let backup = good(&backup)["backupId"].as_str().unwrap().to_owned();
            let task = format!("task_{}", Uuid::new_v4().simple());
            let op = paths
                .workspace_root()
                .join(".platform/operations")
                .join(&task);
            fs::create_dir_all(&op).unwrap();
            let root = store.project(&id).unwrap().root;
            let candidate = op.join("candidate");
            let original = op.join("original");
            backups::extract_validated(
                &manager.archive(&id, &backup).unwrap(),
                &candidate,
                Some(&id),
            )
            .unwrap();
            let control = store.control(&id).unwrap();
            let state = control
                .begin("interrupted restore".into(), None, false)
                .unwrap();
            let mut journal = RestoreJournal {
                task_id: task,
                project_id: id.clone(),
                backup_id: backup,
                original_identity: persistent_identity(&root).unwrap(),
                candidate_identity: persistent_identity(&candidate).unwrap(),
                committed: false,
                resolved: false,
                previous_control: state.clone(),
                final_control: None,
                previous_provenance: None,
            };
            // Simulate the persisted mount number changing across reboot while
            // preserving the real volume UUID and directory inode.
            #[cfg(target_os = "macos")]
            {
                assert!(!journal.original_identity.is_legacy());
                assert!(!journal.candidate_identity.is_legacy());
                journal.original_identity.device = journal.original_identity.device.wrapping_add(1);
                journal.candidate_identity.device =
                    journal.candidate_identity.device.wrapping_add(1);
            }
            let path = op.join("journal.json");
            builds::atomic_json(&path, &journal).unwrap();
            if phase >= 1 {
                move_dir(&root, &original).unwrap();
            }
            if phase >= 2 {
                move_dir(&candidate, &root).unwrap();
            }
            if phase == 3 {
                journal.committed = true;
                let mut state = state.clone();
                state.ready_for_changes = true;
                state.recovery_required = false;
                journal.final_control = Some(state);
                builds::atomic_json(&path, &journal).unwrap();
            }
            // Recreate the store as a restart would; no cached controls or database workers.
            let restarted = ProjectStore::new(paths.workspace_root().into()).unwrap();
            let outcomes = MaintenanceManager::recover(&paths, &restarted).unwrap();
            assert_eq!(
                outcomes[0].1,
                if phase == 3 {
                    "restored"
                } else {
                    "rolled_back"
                }
            );
            assert!(matches_identity(
                if phase == 3 {
                    &journal.candidate_identity
                } else {
                    &journal.original_identity
                },
                &root,
            ));
            assert_eq!(restarted.control(&id).unwrap().get().mode, "maintenance");
            assert!(!restarted.control(&id).unwrap().get().recovery_required);
            restarted.project(&id).unwrap().check().unwrap();
            stop(manager, tasks, store).await;
        }
    }
    #[tokio::test]
    async fn legacy_restore_journal_tuples_recover_only_on_a_verified_device() {
        for renumbered in [false, true] {
            let (_temp, paths, store, tasks, manager, id) = fixture().await;
            let root = store.project(&id).unwrap().root;
            let before = identity(&root).unwrap();
            let task = format!("task_{}", Uuid::new_v4().simple());
            let operation = paths
                .workspace_root()
                .join(".platform/operations")
                .join(&task);
            fs::create_dir_all(operation.join("candidate")).unwrap();
            let state = store
                .control(&id)
                .unwrap()
                .begin("Legacy restore recovery".into(), None, false)
                .unwrap();
            builds::atomic_json(&operation.join("journal.json"), &json!({
                "taskId":task,"projectId":id,"backupId":"backup_legacy_fixture",
                "originalIdentity":[if renumbered { before.0.wrapping_add(1) } else { before.0 },before.1],
                "candidateIdentity":identity(&operation.join("candidate")).unwrap(),
                "committed":false,"resolved":false,"finalControl":null,
                "previousControl":state,"previousProvenance":null
            })).unwrap();
            let outcomes = MaintenanceManager::recover(&paths, &store).unwrap();
            assert_eq!(
                outcomes[0].1,
                if renumbered {
                    "needs_attention"
                } else {
                    "rolled_back"
                }
            );
            assert_eq!(identity(&root).unwrap(), before);
            assert!(operation.join("candidate").is_dir());
            assert_eq!(
                store.control(&id).unwrap().get().recovery_required,
                renumbered
            );
            stop(manager, tasks, store).await;
        }
    }
    #[tokio::test]
    async fn ambiguous_recovery_preserves_all_directories_and_blocks_changes() {
        let (_temp, paths, store, tasks, manager, id) = fixture().await;
        let root = store.project(&id).unwrap().root;
        let task = format!("task_{}", Uuid::new_v4().simple());
        let op = paths
            .workspace_root()
            .join(".platform/operations")
            .join(&task);
        fs::create_dir_all(op.join("candidate")).unwrap();
        let state = store
            .control(&id)
            .unwrap()
            .begin("ambiguous".into(), None, false)
            .unwrap();
        let journal = RestoreJournal {
            task_id: task,
            project_id: id.clone(),
            backup_id: "backup_test".into(),
            original_identity: serde_json::from_value(json!([0, 0])).unwrap(),
            candidate_identity: persistent_identity(&op.join("candidate")).unwrap(),
            committed: false,
            resolved: false,
            previous_control: state,
            final_control: None,
            previous_provenance: None,
        };
        builds::atomic_json(&op.join("journal.json"), &journal).unwrap();
        let outcomes = MaintenanceManager::recover(&paths, &store).unwrap();
        assert_eq!(outcomes[0].1, "needs_attention");
        assert!(root.exists() && op.join("candidate").exists());
        assert!(store.control(&id).unwrap().get().recovery_required);
        assert_eq!(
            manager
                .submit(&id, "maintenanceEnd", OperationInput::default())
                .await
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        stop(manager, tasks, store).await;
    }
}

#[cfg(test)]
#[path = "maintenance_restart_tests.rs"]
mod restart_tests;
