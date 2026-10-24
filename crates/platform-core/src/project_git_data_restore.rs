//! Explicit project-v1 data application and same-operation recovery. All live I/O
//! belongs to joinable task runners; no request future owns a destructive worker.
use super::*;
use crate::{
    data_restore_guard as guard,
    file_identity::PersistedIdentity,
    files,
    projects::{
        data_restore_registration::{
            read_proof, DataRestoreDirection, DataRestoreRegistrationPreparation,
        },
        ProjectTaskLease,
    },
};
#[path = "project_git_data_restore_files.rs"]
mod slot_files;
#[path = "project_git_data_restore_transaction.rs"]
mod transaction;
pub(crate) use transaction::inspect_completion;
pub(crate) use transaction::metadata_usage;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataRestoreApplyInput {
    pub target_commit: String,
    pub expected_head: String,
    pub expected_branch: String,
    pub expected_snapshot: String,
    pub expected_document_epoch: String,
    pub expected_live_data_digest: String,
    pub expected_target_snapshot_digest: String,
    pub expected_plan_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataRestoreRecoverInput {
    pub restore_task_id: String,
    pub expected_plan_digest: String,
    pub expected_journal_digest: String,
    pub action: RestoreAction,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreSlotState {
    pub kind: String,
    pub path: String,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreRecoveryOperation {
    pub restore_task_id: String,
    pub target_commit: String,
    pub phase: String,
    pub plan_digest: String,
    pub journal_digest: String,
    pub operation_outcome: String,
    pub data_state: String,
    pub document_epoch: Option<String>,
    pub can_continue: bool,
    pub can_undo: bool,
    pub completion_published: bool,
    pub original_retained: bool,
    pub retained_path: PathBuf,
    pub slots: Vec<DataRestoreSlotState>,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDataRestoreRecoveryStatus {
    pub state: String,
    pub business_blocked: bool,
    pub operations: Vec<DataRestoreRecoveryOperation>,
    pub issues: Vec<ApiError>,
}

enum Request {
    Apply(GitDataRestoreApplyInput),
    Recover(GitDataRestoreRecoverInput),
}
impl Request {
    fn kind(&self) -> &'static str {
        match self {
            Self::Apply(_) => "gitDataRestoreApply",
            Self::Recover(input) if input.action == RestoreAction::Continue => {
                "gitDataRestoreContinue"
            }
            Self::Recover(_) => "gitDataRestoreUndo",
        }
    }
}

impl ProjectGitManager {
    #[cfg(test)]
    pub(crate) fn test_data_restore_fault(&self, point: u32) {
        self.data_restore_fault.store(point, Ordering::Release);
    }
    pub async fn data_restore_apply(
        self: &Arc<Self>,
        id: &str,
        input: GitDataRestoreApplyInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        for oid in [&input.target_commit, &input.expected_head] {
            if oid.len() != 40 || !valid_oid(oid) {
                return Err(input_error());
            }
        }
        for digest in [
            &input.expected_snapshot,
            &input.expected_live_data_digest,
            &input.expected_target_snapshot_digest,
            &input.expected_plan_digest,
        ] {
            validate_digest(digest)?;
        }
        one_ref(format!("{}\n", input.expected_branch).as_bytes()).map_err(|_| input_error())?;
        if uuid::Uuid::parse_str(&input.expected_document_epoch).map_or(true, |v| {
            v.is_nil() || v.to_string() != input.expected_document_epoch
        }) {
            return Err(input_error());
        }
        self.submit_data_restore(id, Request::Apply(input), stop)
    }
    pub async fn data_restore_recover(
        self: &Arc<Self>,
        id: &str,
        input: GitDataRestoreRecoverInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_digest(&input.expected_plan_digest)?;
        validate_digest(&input.expected_journal_digest)?;
        if !crate::projects::valid_id(&input.restore_task_id)
            || !input.restore_task_id.starts_with("task_")
        {
            return Err(input_error());
        }
        self.submit_data_restore(id, Request::Recover(input), stop)
    }
    fn submit_data_restore(
        self: &Arc<Self>,
        id: &str,
        request: Request,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let _submission = self.submission.lock().unwrap();
        self.available()?;
        if !cfg!(target_os = "macos") {
            return Err(error("UNSUPPORTED_PLATFORM", "数据取回目前仅验证 macOS。"));
        }
        if *stop.borrow() || stop.has_changed().is_err() {
            return Err(interrupted());
        }
        let kind = request.kind();
        let (task, lease) = match &request {
            Request::Apply(_) => {
                self.store.check_development_available(id)?;
                self.store.project(id)?.control.check(false, None, false)?;
                self.tasks.create_operation(id, kind)?
            }
            Request::Recover(input) => {
                let permit = guard::validate_recovery(
                    self.paths.workspace_root(),
                    id,
                    &input.restore_task_id,
                    &input.expected_plan_digest,
                    &input.expected_journal_digest,
                )?;
                self.tasks
                    .create_data_restore_recovery_operation(id, kind, &permit)?
            }
        };
        let (sender, cancel) = watch::channel(false);
        self.active
            .lock()
            .unwrap()
            .insert(task.task_id.clone(), sender);
        let manager = self.clone();
        let task_id = task.task_id.clone();
        let id = id.to_string();
        let runner = tokio::spawn(async move {
            let restore_task = match &request {
                Request::Apply(_) => task_id.clone(),
                Request::Recover(input) => input.restore_task_id.clone(),
            };
            let report_stop = stop.clone();
            let result = match request {
                Request::Apply(input) => {
                    manager
                        .run_data_restore_apply(&id, &task_id, input, lease, stop, cancel)
                        .await
                }
                Request::Recover(input) => {
                    manager
                        .run_data_restore_recover(&id, &task_id, input, lease, stop, cancel)
                        .await
                }
            }
            .map_err(|error| manager.data_restore_error(&id, &restore_task, error, &report_stop));
            if let Err(error) = &result {
                let _ = manager.tasks.append(
                    &task_id,
                    format!("{}: {}\n", error.code, error.message).as_bytes(),
                );
            }
            let _ = manager.tasks.finish(&task_id, result);
            manager.active.lock().unwrap().remove(&task_id);
        });
        let mut runners = self.runners.lock().unwrap();
        runners.retain(|runner| !runner.is_finished());
        runners.push(runner);
        Ok(task)
    }

    pub async fn data_restore_status(
        &self,
        id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<GitDataRestoreRecoveryStatus, ApiError> {
        self.available()?;
        let lease = self
            .store
            .reserve_transient_operation(id, "data-restore-status")?;
        if *stop.borrow() || stop.has_changed().is_err() {
            return Err(interrupted());
        }
        let workspace = self.paths.workspace_root().to_path_buf();
        let id = id.to_string();
        let worker_stop = stop.clone();
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        let mut worker = self.preview_worker(move || {
            let _lease = lease;
            let state = guard::inspect(&workspace, &id);
            let mut operations = vec![];
            for pending in &state.pending {
                let root = workspace
                    .join(".platform/project-state/data-restore")
                    .join(&id)
                    .join(&pending.task_id);
                operations.push(transaction::status(
                    &workspace,
                    &root,
                    &id,
                    &pending.task_id,
                    &worker_stop,
                ));
            }
            let business_blocked = !state.pending.is_empty() || !state.issues.is_empty();
            let blocked = !state.issues.is_empty()
                || state.pending.len() > 1
                || operations.iter().any(|op| !op.issues.is_empty());
            Ok(GitDataRestoreRecoveryStatus {
                state: if !business_blocked {
                    "clear"
                } else if blocked {
                    "blocked"
                } else {
                    "pending"
                }
                .into(),
                business_blocked,
                operations,
                issues: state.issues,
            })
        })?;
        data_preview::await_worker(&mut worker, &mut context).await
    }
    fn data_restore_error(
        &self,
        id: &str,
        task: &str,
        error: ApiError,
        stop: &watch::Receiver<bool>,
    ) -> ApiError {
        let operation = self.data_restore_root(id, task);
        if let Ok((_, plan, _)) = transaction::load(&operation, id, task) {
            return transaction::receipt_error(
                self.paths.workspace_root(),
                &operation,
                &plan,
                error,
                stop,
            );
        }
        let state = guard::inspect(self.paths.workspace_root(), id);
        let pending = !state.pending.is_empty() || !state.issues.is_empty();
        let exists = operation.try_exists().unwrap_or(true);
        error
            .outcome(if exists { "unknown" } else { "not_started" })
            .detail("dataState", if exists { "unknown" } else { "before" })
            .detail("restoreTaskId", task)
            .detail("pending", pending)
            .detail("businessBlocked", pending)
            .detail("liveDataChanged", exists)
            .detail("originalRetained", false)
            .detail(
                "retainedPath",
                if exists {
                    operation
                } else {
                    self.paths
                        .workspace_root()
                        .join(".platform")
                        .join(format!("data-restore-preparing-{task}"))
                }
                .display()
                .to_string(),
            )
    }
    fn data_restore_root(&self, id: &str, task: &str) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/project-state/data-restore")
            .join(id)
            .join(task)
    }

    async fn run_data_restore_apply(
        &self,
        id: &str,
        task: &str,
        input: GitDataRestoreApplyInput,
        lease: ProjectTaskLease,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, ApiError> {
        let project = self.store.project(id)?;
        let mut context = Context::new(stop.clone(), cancel.clone())?;
        let gate = data_versions::read_gate_for_data(&project, &mut context).await?;
        let tools = context.resolve(&self.git).await?;
        let initial =
            Box::pin(self.preview_data_git_status(&project, &tools, &mut context)).await?;
        verify_input(&input, &project, &initial)?;
        self.tasks.phase(task, "data-restore-target")?;
        let tree = Box::pin(data_preview::objects::target_tree(
            &project,
            &input.target_commit,
            &tools,
            &mut context,
        ))
        .await?;
        let (target_temporary, target) = Box::pin(data_preview::target_snapshot(
            self,
            &project,
            &tools,
            &mut context,
            tree,
        ))
        .await?;
        if target.manifest_digest != input.expected_target_snapshot_digest {
            return Err(stale_data());
        }
        drop(gate);
        let snapshot_lease =
            version_data::SnapshotLease::acquire(project.clone(), stop.clone(), cancel.clone())
                .await?;
        let fresh = Box::pin(self.preview_data_git_status(&project, &tools, &mut context)).await?;
        verify_input(&input, &project, &fresh)?;
        let computed = data_preview::plan_digest(
            &project,
            &fresh,
            &input.target_commit,
            &input.expected_live_data_digest,
            &target.manifest_digest,
        )?;
        if computed != input.expected_plan_digest {
            return Err(stale_data());
        }
        // Freeze and the exclusive gate remain held across worker closure and all
        // later filesystem work. A task runner always awaits this blocking join.
        self.store.close_database(id).await?;
        context.checkpoint()?;
        let store = self.store.clone();
        let paths = self.paths.clone();
        let tasks = self.tasks.clone();
        let task = task.to_string();
        let deadline = context.deadline;
        #[cfg(test)]
        let fault = self.data_restore_fault.swap(0, Ordering::AcqRel);
        #[cfg(not(test))]
        let fault = 0;
        #[cfg(test)]
        let pause = self.data_restore_pause.lock().unwrap().take();
        let worker = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.recv();
            }
            let _target_temporary = target_temporary;
            let mut check = || {
                checkpoint(&stop, &cancel)?;
                if Instant::now() >= deadline {
                    Err(error("PROJECT_DATA_RESTORE_TIMEOUT","Data restore reached its operation deadline; retained evidence requires explicit recovery."))
                } else {
                    Ok(())
                }
            };
            let mut faults = Faults::new(fault);
            check()?;
            let stage = paths
                .workspace_root()
                .join(".platform")
                .join(format!("data-restore-preparing-{task}"));
            private_dir(&stage)?;
            let temporary = tempfile::Builder::new()
                .prefix("data-restore-current-")
                .tempdir_in(paths.workspace_root().join(".platform"))?;
            let current = version_data::prepare_snapshot_locked(
                &snapshot_lease,
                &temporary.path().join("current"),
                version_data::SnapshotLimits::default(),
                &stop,
                &cancel,
            )?;
            if current.manifest_digest != input.expected_live_data_digest {
                return Err(stale_data());
            }
            slot_files::copy_target(&target, &stage, &mut check)?;
            let binding = transaction::binding(&project, &fresh, &mut check)?;
            let slots = transaction::prepare_slots(
                &project,
                &stage,
                &current.manifest,
                &target.manifest,
                &mut check,
            )?;
            let apply_epoch = uuid::Uuid::new_v4().to_string();
            let undo_epoch = uuid::Uuid::new_v4().to_string();
            let registration_plan = store.prepare_data_restore_registration(
                &project.manifest.id,
                &lease,
                snapshot_lease.gate(),
                DataRestoreRegistrationPreparation {
                    staging: &stage,
                    candidate_database: &stage.join("slots/database/after"),
                    apply_epoch: &apply_epoch,
                    undo_epoch: &undo_epoch,
                },
            )?;
            let stage_fd = files::root(&stage)?;
            let mut prepared_journal = files::create(&stage_fd, "journal.json")?;
            let journal_identity =
                PersistedIdentity::capture(&stage.join("journal.json")).map_err(|_| changed())?;
            let plan = transaction::Plan {
                schema_version: 1,
                policy: transaction::POLICY.into(),
                project_id: project.manifest.id.clone(),
                task_id: task.clone(),
                target_commit: input.target_commit,
                preview_plan_digest: input.expected_plan_digest,
                operation_identity: PersistedIdentity::capture(&stage).map_err(|_| changed())?,
                registration_plan: registration_plan.clone(),
                manifest: project.manifest.clone(),
                parents: slot_files::capture_parents(
                    &project.root,
                    slots.iter().map(|slot| slot.path.clone()),
                )?,
                operation_parents: slot_files::capture_parents(
                    &stage,
                    slots
                        .iter()
                        .flat_map(|slot| {
                            [
                                format!("slots/{}/before", slot.kind),
                                format!("slots/{}/after", slot.kind),
                            ]
                        })
                        .chain([
                            "metadata-attempts/placeholder".into(),
                            "metadata-reserve/placeholder".into(),
                        ]),
                )?,
                metadata_reserve: ["completion", "terminal"]
                    .into_iter()
                    .map(|name| {
                        Ok((
                            name.to_string(),
                            read_proof(
                                &stage.join("metadata-reserve").join(name),
                                16 * 1024,
                                false,
                            )?
                            .0,
                        ))
                    })
                    .collect::<Result<_, ApiError>>()?,
                journal_identity: journal_identity.clone(),
                binding,
                current: current.manifest,
                target: target.manifest,
                live_data_digest: current.manifest_digest,
                target_snapshot_digest: target.manifest_digest,
                slots,
            };
            let plan_bytes = serde_json::to_vec_pretty(&plan)?;
            if plan_bytes.len() as u64 > MAX_METADATA - 128 * 1024 {
                return Err(limit());
            }
            let stage_fd = files::root(&stage)?;
            let mut file = files::create(&stage_fd, "plan.json")?;
            file.write_all(&plan_bytes)?;
            file.sync_all()?;
            let proof = read_proof(&stage.join("plan.json"), MAX_METADATA, false)?.0;
            let mut journal = transaction::Journal {
                schema_version: 1,
                operation: "dataRestore".into(),
                project_id: project.manifest.id.clone(),
                task_id: task.clone(),
                phase: "prepared".into(),
                plan_digest: plan.preview_plan_digest.clone(),
                operation_identity: plan.operation_identity.clone(),
                registration_plan,
                transaction_plan: Some(proof),
                journal_identity,
            };
            prepared_journal.write_all(&serde_json::to_vec_pretty(&journal)?)?;
            prepared_journal.sync_all()?;
            plan.validate(&journal.header())?;
            transaction::verify_binding(&project.root, &plan, &mut check)?;
            transaction::classify(&project.root, &stage, &plan, &mut check)?;
            faults.at("before-publication")?;
            check()?;
            let operation = guard::publish_prepared(paths.workspace_root(), &stage, &journal)?;
            faults.at("after-publication").map_err(|e| {
                e.detail("restoreTaskId", task.clone())
                    .detail("pending", true)
            })?;
            let bytes = read_file(&operation.join("journal.json"), MAX_METADATA)?;
            let permit = guard::validate_recovery(
                paths.workspace_root(),
                &project.manifest.id,
                &task,
                &journal.plan_digest,
                &sha256(&bytes),
            )?;
            let access = store.data_restore_recovery_access(&project.manifest.id, lease, permit)?;
            let (gate, freeze) = snapshot_lease.into_lock_parts();
            let mut access = access.with_lock(gate, freeze)?;
            let result = transaction::execute(
                paths.workspace_root(),
                &mut access,
                &mut journal,
                &plan,
                DataRestoreDirection::Continue,
                &mut check,
                &mut faults,
                &tasks,
                &task,
            );
            result.map_err(|e| {
                transaction::receipt_error(paths.workspace_root(), &operation, &plan, e, &stop)
            })
        });
        worker.await.map_err(|_| {
            error(
                "PROJECT_DATA_RESTORE_WORKER_FAILED",
                "数据取回工作者停止；请读取数据恢复状态核对保留现场。",
            )
            .outcome("unknown")
        })?
    }
    async fn run_data_restore_recover(
        &self,
        id: &str,
        task: &str,
        input: GitDataRestoreRecoverInput,
        lease: ProjectTaskLease,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, ApiError> {
        let deadline = Instant::now() + TIMEOUT;
        let permit = guard::validate_recovery(
            self.paths.workspace_root(),
            id,
            &input.restore_task_id,
            &input.expected_plan_digest,
            &input.expected_journal_digest,
        )?;
        let access = self
            .store
            .data_restore_recovery_access(id, lease, permit)?
            .lock(stop.clone(), cancel.clone())
            .await?;
        self.store.close_database(id).await?;
        let operation = access.operation().to_path_buf();
        let paths = self.paths.clone();
        let tasks = self.tasks.clone();
        let id = id.to_string();
        let task = task.to_string();
        #[cfg(test)]
        let fault = self.data_restore_fault.swap(0, Ordering::AcqRel);
        #[cfg(not(test))]
        let fault = 0;
        let worker = tokio::task::spawn_blocking(move || {
            let mut access = access;
            let mut check = || {
                checkpoint(&stop, &cancel)?;
                if Instant::now() >= deadline {
                    Err(error("PROJECT_DATA_RESTORE_TIMEOUT","Data recovery reached its operation deadline; retained evidence remains protected."))
                } else {
                    Ok(())
                }
            };
            let mut faults = Faults::new(fault);
            let (mut journal, plan, bytes) =
                transaction::load(&operation, &id, &input.restore_task_id)?;
            if sha256(&bytes) != input.expected_journal_digest
                || journal.plan_digest != input.expected_plan_digest
            {
                return Err(stale_data());
            }
            let direction = if input.action == RestoreAction::Continue {
                DataRestoreDirection::Continue
            } else {
                DataRestoreDirection::Undo
            };
            transaction::execute(
                paths.workspace_root(),
                &mut access,
                &mut journal,
                &plan,
                direction,
                &mut check,
                &mut faults,
                &tasks,
                &task,
            )
            .map_err(|e| {
                transaction::receipt_error(paths.workspace_root(), &operation, &plan, e, &stop)
            })
        });
        worker.await.map_err(|_| {
            error(
                "PROJECT_DATA_RESTORE_WORKER_FAILED",
                "数据恢复工作者停止；原现场继续保留。",
            )
            .outcome("unknown")
            .detail("pending", true)
        })?
    }
}

fn verify_input(
    input: &GitDataRestoreApplyInput,
    project: &ProjectHandle,
    snapshot: &Snapshot,
) -> Result<(), ApiError> {
    let head = snapshot
        .view
        .repository
        .head
        .as_ref()
        .ok_or_else(stale_data)?;
    if project.document_epoch != input.expected_document_epoch
        || snapshot.view.snapshot_digest != input.expected_snapshot
        || head.oid.as_deref() != Some(&input.expected_head)
        || head.branch.as_deref() != Some(&input.expected_branch)
    {
        return Err(stale_data());
    }
    Ok(())
}
fn valid_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn checkpoint(
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), ApiError> {
    if *stop.borrow()
        || *cancel.borrow()
        || stop.has_changed().is_err()
        || cancel.has_changed().is_err()
    {
        Err(interrupted())
    } else {
        Ok(())
    }
}
fn invalid() -> ApiError {
    error(
        "PROJECT_DATA_RESTORE_INVALID",
        "数据取回计划或完成凭据无法核验；保留现场并继续停止业务。",
    )
}
fn changed() -> ApiError {
    error(
        "PROJECT_DATA_RESTORE_CHANGED",
        "数据、原件、注册或目录身份已有第三方变化；保留所有现场并停止操作。",
    )
}
fn limit() -> ApiError {
    error(
        "PROJECT_DATA_RESTORE_LIMIT",
        "数据取回超过本次核验预算；未覆盖或删除任何已有材料。",
    )
}
fn stale_data() -> ApiError {
    error(
        "PROJECT_DATA_RESTORE_STALE",
        "当前数据、文档代次或Git状态与预览不同；请重新预览后确认。",
    )
    .outcome("not_started")
    .detail("liveDataChanged", false)
}
struct Faults {
    wanted: u32,
    next: u32,
}
impl Faults {
    fn new(wanted: u32) -> Self {
        Self { wanted, next: 0 }
    }
    fn at(&mut self, label: &str) -> Result<(), ApiError> {
        self.next += 1;
        if self.wanted == self.next
            || (self.wanted == u32::MAX && label == "before-epoch")
            || (self.wanted == u32::MAX - 1 && label == "after-metadata-candidate-create")
            || (self.wanted == u32::MAX - 2 && label == "after-metadata-directory-sync")
        {
            Err(error("PROJECT_DATA_RESTORE_TEST_FAULT", label).detail("faultPoint", self.next))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "project_git_data_restore_tests.rs"]
mod tests;
