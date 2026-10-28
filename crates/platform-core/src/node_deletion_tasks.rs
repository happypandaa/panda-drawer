//! Persistent Node deletion ownership and recovery. Window teardown is supplied
//! by the native host; process, journal and filesystem ownership stay here.
use super::*;
use crate::{
    file_identity::PersistedIdentity, node_deletion_journal::NodeDeletionJournal, ProjectStore,
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex as SyncMutex, RwLock,
};
use tokio::sync::Notify;

pub type NodeDeletionBeforeStop =
    Box<dyn FnOnce(NodeDeletionPermit) -> BoxFuture<'static, Result<()>> + Send>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistrationRemoval {
    directory_identity: PersistedIdentity,
    original_identity: PersistedIdentity,
    sha256: String,
    #[serde(default)]
    restore_identity: Option<PersistedIdentity>,
}

#[derive(Default)]
pub(super) struct NodeDeletionTasks {
    tasks: SyncMutex<BTreeMap<String, Arc<DeletionTask>>>,
    store: SyncMutex<Option<Arc<ProjectStore>>>,
}
struct DeletionTask {
    journal: SyncMutex<Option<NodeDeletionJournal>>,
    snapshot: RwLock<Value>,
    permit: SyncMutex<Option<NodeDeletionPermit>>,
    lease: SyncMutex<Option<crate::projects::ProjectTaskLease>>,
    barrier: SyncMutex<Option<crate::projects::NodeDeletionBarrier>>,
    running: AtomicBool,
    changed: Notify,
}
impl DeletionTask {
    fn new(journal: NodeDeletionJournal, permit: Option<NodeDeletionPermit>) -> Self {
        let snapshot = journal_view(&journal);
        Self {
            journal: SyncMutex::new(Some(journal)),
            snapshot: RwLock::new(snapshot),
            permit: SyncMutex::new(permit),
            lease: SyncMutex::new(None),
            barrier: SyncMutex::new(None),
            running: AtomicBool::new(false),
            changed: Notify::new(),
        }
    }
    fn cached(&self) -> Value {
        self.snapshot
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn publish(&self, journal: &NodeDeletionJournal) {
        *self.snapshot.write().unwrap_or_else(|p| p.into_inner()) = journal_view(journal);
        self.changed.notify_waiters();
    }
    fn abandon_requested(&self) -> bool {
        self.cached()["abandonRequested"] == true
    }
    fn release_gate(&self) {
        if let Some(permit) = self.permit.lock().unwrap_or_else(|p| p.into_inner()).take() {
            permit.release_gate();
        }
        self.barrier
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    }
    fn change_phase(&self, phase: &str) -> Result<()> {
        let mut slot = self.journal.lock().unwrap_or_else(|p| p.into_inner());
        let journal = slot
            .as_mut()
            .ok_or_else(|| fail("删除文件工作仍持有任务。"))?;
        if journal.is_terminal() || journal.abandon_requested() {
            return Ok(());
        }
        journal.set_phase(phase)?;
        self.publish(journal);
        Ok(())
    }
    fn fail(&self, error: ApiError) {
        let mut slot = self.journal.lock().unwrap_or_else(|p| p.into_inner());
        let mut snapshot = match slot.as_mut() {
            Some(journal) => {
                let can_abandon = journal.can_abandon().unwrap_or(false);
                let terminal = journal.is_terminal();
                let incomplete = journal.snapshot()["renamedAny"] == true
                    || journal.metadata()["legacyIrreversible"] == true
                    || !can_abandon
                    || !journal.metadata()["registrationRemoval"].is_null();
                let mut error = error
                    .outcome(if incomplete {
                        "delete_incomplete"
                    } else {
                        "not_deleted"
                    })
                    .detail("taskId", journal.task_id())
                    .detail("remaining", journal_view(journal)["remaining"].clone());
                if let Some(slot) = journal_instance(journal) {
                    error = error.detail("platformRoot", json!(slot));
                }
                if incomplete && !error.message.starts_with("项目删除未完成") {
                    error.message = format!(
                        "项目删除未完成，部分文件可能已永久删除。原因：{}",
                        error.message
                    );
                }
                let reported = if !terminal {
                    match journal.mark_failed(&error) {
                        Ok(()) => error,
                        Err(persist) => persist.detail("deletionCause", json!(error)),
                    }
                } else {
                    error
                };
                let mut value = journal_view(journal);
                value["phase"] = json!("failed");
                value["canAbandon"] =
                    json!(!terminal && can_abandon && value["canAbandon"] == true);
                value["canRetry"] = json!(true);
                value["error"] = json!(reported);
                value
            }
            None => {
                let mut value = self.cached();
                value["phase"] = json!("failed");
                value["canAbandon"] = json!(false);
                value["canRetry"] = json!(false);
                value["error"] = json!(error);
                value
            }
        };
        snapshot["active"] = json!(self.running.load(Ordering::Acquire));
        *self.snapshot.write().unwrap_or_else(|p| p.into_inner()) = snapshot;
        self.changed.notify_waiters();
    }
}

impl NodeApps {
    pub(super) fn load_delete_tasks(&self) -> Result<()> {
        let mut tasks = self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for mut journal in NodeDeletionJournal::list(&self.data_root)? {
            if journal.abandon_requested() && !journal.is_terminal() {
                if journal.metadata()["legacyIrreversible"] == true {
                    return Err(cannot_abandon());
                }
                let slot = self.root.join(journal.app_id());
                let has_marker = match fs::symlink_metadata(slot.join("active.json")) {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => return Err(error.into()),
                };
                if !has_marker
                    && !has_start_intent_cleanup(&slot)?
                    && journal.metadata()["registrationRemoval"].is_null()
                {
                    journal.abandon()?;
                }
            }
            let permit = if journal.is_terminal() {
                None
            } else {
                Some(self.begin_delete_operation(journal.app_id())?)
            };
            if journal.abandon_requested() {
                if let Some(permit) = &permit {
                    permit.request_abandon();
                }
            }
            tasks.insert(
                journal.task_id().into(),
                Arc::new(DeletionTask::new(journal, permit)),
            );
        }
        Ok(())
    }
    pub fn set_deletion_store(&self, store: Arc<ProjectStore>) -> Result<()> {
        let tasks = self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut current = self
            .deletion_tasks
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if current.as_ref().is_some_and(|old| Arc::ptr_eq(old, &store)) {
            return Ok(());
        }
        if current.is_some()
            && tasks.values().any(|task| {
                !matches!(
                    task.cached()["phase"].as_str(),
                    Some("deleted" | "abandoned")
                )
            })
        {
            return Err(ApiError::new(
                "NODE_DELETION_PENDING",
                "仍有未完成的删除任务，不能切换工作区协调。",
            ));
        }
        let mut assigned: Vec<Arc<DeletionTask>> = Vec::new();
        for task in tasks.values() {
            let _journal = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            if matches!(
                task.cached()["phase"].as_str(),
                Some("deleted" | "abandoned")
            ) {
                continue;
            }
            let id = task.cached()["appId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            match store.reserve_node_deletion_barrier(&id) {
                Ok(barrier) => {
                    *task.barrier.lock().unwrap_or_else(|p| p.into_inner()) = Some(barrier);
                    assigned.push(task.clone());
                }
                Err(error) => {
                    for previous in assigned {
                        previous
                            .barrier
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .take();
                    }
                    return Err(error);
                }
            }
        }
        *current = Some(store);
        Ok(())
    }
    fn deletion_barrier(&self, id: &str) -> Result<Option<crate::projects::NodeDeletionBarrier>> {
        self.deletion_tasks
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .map(|store| store.reserve_node_deletion_barrier(id))
            .transpose()
    }
    pub(super) fn pending_delete_for(&self, id: &str) -> bool {
        self.deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|task| {
                let value = task.cached();
                value["appId"] == id
                    && !matches!(value["phase"].as_str(), Some("deleted" | "abandoned"))
            })
    }
    fn deletion_lease(&self, id: &str) -> Result<Option<crate::projects::ProjectTaskLease>> {
        self.deletion_tasks
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .map(|store| {
                store.reserve_transient_operation(&format!("node-delete:{id}"), "node-delete")
            })
            .transpose()
    }
    fn deletion_task(&self, task_id: &str, app_id: &str) -> Result<Arc<DeletionTask>> {
        let task = self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(task_id)
            .cloned()
            .ok_or_else(|| fail("找不到此删除任务。"))?;
        if task.cached()["appId"] != app_id {
            return Err(fail("删除任务与应用身份不一致。"));
        }
        Ok(task)
    }
    fn task_view(&self, task: &DeletionTask) -> Value {
        let mut value = task.cached();
        let id = value["appId"].as_str().unwrap_or_default().to_owned();
        let operation = self.deletion_operation_status(&id).unwrap_or_default();
        value["waitingFor"] = if value["phase"] == "waiting" {
            operation["waitingFor"].clone()
        } else {
            json!([])
        };
        value["active"] = json!(task.running.load(Ordering::Acquire));
        if value["active"] == true {
            value["canRetry"] = json!(false);
        }
        if task
            .journal
            .try_lock()
            .map_or(true, |journal| journal.is_none())
        {
            value["canAbandon"] = json!(false);
        }
        if value["phase"] == "deleted" {
            value["result"] = json!({"appId":id,"operationOutcome":"deleted","ownership":value["ownership"],"sourcePreserved":true,"evidenceRetained":true,"metadataCleanupPending":false,"durabilityConfirmed":true});
        }
        value
    }
    pub fn deletion_task_status(&self, task_id: &str, app_id: &str) -> Result<Value> {
        let task = self.deletion_task(task_id, app_id)?;
        Ok(self.task_view(&task))
    }
    pub fn deletion_tasks(&self) -> Result<Vec<Value>> {
        Ok(self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|task| self.task_view(task))
            .collect())
    }
    pub async fn submit_deletion(&self, id: &str, input: DeleteNodeAppInput) -> Result<Value> {
        self.submit_deletion_optional(id, input, None).await
    }
    pub async fn submit_deletion_with(
        &self,
        id: &str,
        input: DeleteNodeAppInput,
        before_stop: NodeDeletionBeforeStop,
    ) -> Result<Value> {
        self.submit_deletion_optional(id, input, Some(before_stop))
            .await
    }
    async fn submit_deletion_optional(
        &self,
        id: &str,
        input: DeleteNodeAppInput,
        before_stop: Option<NodeDeletionBeforeStop>,
    ) -> Result<Value> {
        // Check and publish under one task-map admission critical section.
        let (task, start) = {
            let mut tasks = self
                .deletion_tasks
                .tasks
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(task) = tasks.values().find(|task| {
                let value = task.cached();
                value["appId"] == id
                    && !matches!(value["phase"].as_str(), Some("deleted" | "abandoned"))
            }) {
                (task.clone(), false)
            } else {
                if self.directory_stopping.load(Ordering::Acquire) {
                    return Err(fail("平台正在退出，不能受理删除。"));
                }
                let barrier = self.deletion_barrier(id)?;
                let lease = self.deletion_lease(id)?;
                let permit = self.begin_delete_operation(id)?;
                let record = self.registration(id)?;
                let slot = self.root.join(id);
                if deletion::revision(&slot)? != input.expected_revision {
                    return Err(ApiError::new(
                        "NODE_APP_DELETE_CHANGED",
                        "项目登记已变化，请重新核对删除范围。",
                    ));
                }
                let mut preserved = vec![slot.join("runs"), self.root.join(".installations")];
                if record.managed.is_none() {
                    preserved.push(record.source_path.clone());
                }
                let name = read_small(&record.source_path.join("local-app.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|v| v["app"]["name"].as_str().map(str::to_owned))
                    .filter(|name| !name.trim().is_empty() && name.len() <= 1024)
                    .unwrap_or_else(|| "项目".into());
                let before = read_small(&slot.join("registration.json"))?;
                let before_text = std::str::from_utf8(&before)
                    .map_err(|_| fail("项目登记编码无效。"))?
                    .to_owned();
                let before_value: Value = serde_json::from_slice(&before)?;
                let metadata = json!({"appId":id,"name":name,"ownership":if record.managed.is_some(){"managed"}else{"external"},"sourcePath":record.source_path,"expectedRegistrationRevision":input.expected_revision,"registrationBefore":before_value,"registrationBeforeText":before_text,"legacyIrreversible":record.removal_pending,"rootIdentity":PersistedIdentity::capture(&slot).map_err(|e|fail(e.to_string()))?,"preservedPaths":preserved});
                let (journal, acceptance_issue) = match NodeDeletionJournal::create(
                    &self.data_root,
                    metadata.clone(),
                ) {
                    Ok(journal) => (journal, None),
                    Err(error) => {
                        let Some(task_id) = error.details["taskId"].as_str() else {
                            return Err(error);
                        };
                        match NodeDeletionJournal::load(&self.data_root, task_id) {
                            Ok(journal) if journal.metadata() == &metadata => {
                                (journal, Some(error))
                            }
                            _ => {
                                // Publication is known to have happened. An
                                // unreadable record must still own this ID and
                                // prevent a second independent deletion.
                                let snapshot = json!({"taskId":task_id,"appId":id,"name":metadata["name"],
                                    "ownership":metadata["ownership"],"sourcePath":metadata["sourcePath"],
                                    "phase":"failed","journalRevision":1,"targetRevision":input.expected_revision,
                                    "canAbandon":false,"canRetry":false,"renamedAny":false,"abandonRequested":false,
                                    "remaining":[],"targets":[],"preservedPaths":metadata["preservedPaths"],"error":error});
                                let task = Arc::new(DeletionTask {
                                    journal: SyncMutex::new(None),
                                    snapshot: RwLock::new(snapshot),
                                    permit: SyncMutex::new(Some(permit)),
                                    lease: SyncMutex::new(None),
                                    barrier: SyncMutex::new(barrier),
                                    running: AtomicBool::new(false),
                                    changed: Notify::new(),
                                });
                                tasks.insert(task_id.into(), task.clone());
                                return Ok(self.task_view(&task));
                            }
                        }
                    }
                };
                let task_id = journal.task_id().to_owned();
                let task = Arc::new(DeletionTask::new(journal, Some(permit)));
                *task.barrier.lock().unwrap_or_else(|p| p.into_inner()) = barrier;
                let start = acceptance_issue.is_none();
                if let Some(error) = acceptance_issue {
                    task.fail(error);
                } else {
                    *task.lease.lock().unwrap_or_else(|p| p.into_inner()) = lease;
                    task.running.store(true, Ordering::Release);
                }
                tasks.insert(task_id, task.clone());
                (task, start)
            }
        };
        if start {
            self.launch_deletion(task.clone(), before_stop);
        }
        Ok(self.task_view(&task))
    }
    pub async fn retry_deletion_with(
        &self,
        task_id: &str,
        app_id: &str,
        observed_revision: u64,
        before_stop: NodeDeletionBeforeStop,
    ) -> Result<Value> {
        self.retry_deletion_optional(task_id, app_id, observed_revision, Some(before_stop))
            .await
    }
    pub async fn retry_deletion(
        &self,
        task_id: &str,
        app_id: &str,
        observed_revision: u64,
    ) -> Result<Value> {
        self.retry_deletion_optional(task_id, app_id, observed_revision, None)
            .await
    }
    async fn retry_deletion_optional(
        &self,
        task_id: &str,
        app_id: &str,
        observed_revision: u64,
        before_stop: Option<NodeDeletionBeforeStop>,
    ) -> Result<Value> {
        let task = self.deletion_task(task_id, app_id)?;
        let start = {
            // The journal lock serializes retry, abandon, and worker retirement.
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            check_observed(&task.cached(), observed_revision)?;
            if task.running.load(Ordering::Acquire) || task.cached()["phase"] != "failed" {
                false
            } else {
                if self.directory_stopping.load(Ordering::Acquire) {
                    return Err(fail("平台正在退出，不能重试删除。"));
                }
                let journal = slot
                    .as_mut()
                    .ok_or_else(|| fail("删除文件工作仍持有任务。"))?;
                let lease = self.deletion_lease(app_id)?;
                if journal.is_terminal() {
                    self.confirm_terminal_deletion(journal)?;
                    task.publish(journal);
                    task.release_gate();
                    false
                } else {
                    journal.set_phase("waiting")?;
                    task.publish(journal);
                    *task.lease.lock().unwrap_or_else(|p| p.into_inner()) = lease;
                    task.running.store(true, Ordering::Release);
                    true
                }
            }
        };
        if start {
            self.launch_deletion(task.clone(), before_stop);
        }
        Ok(self.task_view(&task))
    }
    pub async fn abandon_deletion(
        &self,
        task_id: &str,
        app_id: &str,
        observed_revision: u64,
    ) -> Result<Value> {
        let task = self.deletion_task(task_id, app_id)?;
        let start = {
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            check_observed(&task.cached(), observed_revision)?;
            let journal = slot.as_mut().ok_or_else(cannot_abandon)?;
            if journal.phase() == "abandoned" {
                false
            } else {
                if journal.metadata()["legacyIrreversible"] == true || !journal.can_abandon()? {
                    return Err(cannot_abandon());
                }
                let start = !task.running.load(Ordering::Acquire);
                let lease = if start {
                    if self.directory_stopping.load(Ordering::Acquire) {
                        return Err(fail("平台正在退出，请重新打开后放弃此任务。"));
                    }
                    self.deletion_lease(app_id)?
                } else {
                    None
                };
                journal.request_abandon()?;
                task.publish(journal);
                if let Some(permit) = task
                    .permit
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                {
                    permit.request_abandon();
                }
                if start {
                    *task.lease.lock().unwrap_or_else(|p| p.into_inner()) = lease;
                    task.running.store(true, Ordering::Release);
                }
                start
            }
        };
        task.changed.notify_waiters();
        if start {
            self.launch_deletion(task.clone(), None);
        }
        Ok(self.task_view(&task))
    }
    pub async fn resume_deletions(&self) -> Result<()> {
        let tasks = self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for task in tasks {
            let start = {
                let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
                let journal = slot
                    .as_mut()
                    .ok_or_else(|| fail("删除文件工作仍持有任务。"))?;
                if journal.is_terminal() || task.running.load(Ordering::Acquire) {
                    false
                } else {
                    if self.directory_stopping.load(Ordering::Acquire) {
                        return Err(fail("平台正在退出，不能恢复删除工作。"));
                    }
                    let lease = self.deletion_lease(journal.app_id())?;
                    *task.lease.lock().unwrap_or_else(|p| p.into_inner()) = lease;
                    task.running.store(true, Ordering::Release);
                    true
                }
            };
            if start {
                self.launch_deletion(task, None);
            }
        }
        Ok(())
    }
    pub async fn wait_deletion_result(&self, task_id: &str, app_id: &str) -> Result<Value> {
        let task = self.deletion_task(task_id, app_id)?;
        loop {
            let changed = task.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let value = self.task_view(&task);
            match value["phase"].as_str() {
                Some("deleted") => return Ok(value["result"].clone()),
                Some("abandoned") => {
                    return Err(ApiError::new(
                        "NODE_DELETION_ABANDONED",
                        "已放弃删除；应用不会自动启动。",
                    ))
                }
                Some("failed") => {
                    return Err(serde_json::from_value::<ApiError>(value["error"].clone())
                        .unwrap_or_else(|_| fail("删除未完成，请查看任务中的剩余范围。")))
                }
                _ => changed.await,
            }
        }
    }
    pub(super) async fn wait_deletion_workers(&self) -> Result<()> {
        let tasks = self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for task in tasks {
            loop {
                let changed = task.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if !task.running.load(Ordering::Acquire) {
                    break;
                }
                changed.await;
            }
        }
        Ok(())
    }
    fn launch_deletion(
        &self,
        task: Arc<DeletionTask>,
        before_stop: Option<NodeDeletionBeforeStop>,
    ) {
        // Every caller preclaims running and the actual migration lease under
        // the same journal lock used by retry and abandonment.
        let apps = self.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            match std::panic::AssertUnwindSafe(apps.run_deletion(&task, before_stop))
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => (),
                Ok(Err(error)) => apps.record_deletion_failure(&task, error),
                Err(_) => apps.record_deletion_failure(
                    &task,
                    fail("删除工作意外结束，任务及目录现场已保留。"),
                ),
            }
            let mut handled_abandon = false;
            loop {
                // Do not expose an inactive worker before a concurrent abandon
                // request has safely stopped its original runtime generation.
                if task.abandon_requested()
                    && !handled_abandon
                    && !matches!(
                        task.cached()["phase"].as_str(),
                        Some("deleted" | "abandoned")
                    )
                {
                    let id = task.cached()["appId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    if let Err(error) = apps
                        .stop_runtime_with_reason(
                            &id,
                            crate::node_runtime_trial::RuntimeStopReason::Deletion,
                        )
                        .await
                    {
                        eprintln!(
                            "Abandoned Node deletion retained stop issue: {}",
                            error.code
                        );
                    }
                    if let Err(error) = apps.finalize_abandonment(&task) {
                        apps.record_deletion_failure(&task, error);
                    }
                    handled_abandon = true;
                }
                let slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
                let pending = slot
                    .as_ref()
                    .is_some_and(|journal| journal.abandon_requested() && !journal.is_terminal());
                if pending && !handled_abandon {
                    drop(slot);
                    continue;
                }
                task.lease.lock().unwrap_or_else(|p| p.into_inner()).take();
                task.running.store(false, Ordering::Release);
                drop(slot);
                task.changed.notify_waiters();
                break;
            }
        });
    }
    async fn run_deletion(
        &self,
        task: &Arc<DeletionTask>,
        before_stop: Option<NodeDeletionBeforeStop>,
    ) -> Result<()> {
        let value = task.cached();
        let id = value["appId"]
            .as_str()
            .ok_or_else(|| fail("删除应用身份缺失。"))?
            .to_owned();
        let permit = task
            .permit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| fail("删除操作归属缺失。"))?;
        loop {
            let changed = task.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if task.abandon_requested() {
                permit.request_abandon();
                break;
            }
            tokio::select! { _ = permit.wait_ready() => break, _ = changed => () }
        }
        task.change_phase("stopping")?;
        let native = match before_stop {
            Some(before_stop) => before_stop(permit.clone()).await,
            None => Ok(()),
        };
        let stopped = self
            .stop_runtime_with_reason(&id, crate::node_runtime_trial::RuntimeStopReason::Deletion)
            .await;
        if task.abandon_requested() {
            if let Err(error) = &native {
                eprintln!("Abandoned Node deletion window issue: {}", error.code);
            }
            if let Err(error) = &stopped {
                eprintln!(
                    "Abandoned Node deletion retained stop issue: {}",
                    error.code
                );
            }
            return self.finalize_abandonment(task);
        }
        native?;
        stopped?;
        permit.validate(self, &id)?;
        self.require_no_development_operation(&id)?;
        self.require_no_source_restore(&id)?;
        if self.inner.lock().await.running.contains_key(&id) {
            return Err(fail("应用仍有资源未停止，未移动删除目录。"));
        }
        let apps = self.clone();
        let task = task.clone();
        tokio::task::spawn_blocking(move || apps.finish_deletion_files(&task))
            .await
            .map_err(|_| fail("删除文件工作意外结束，请保留任务现场。"))?
    }
    fn finalize_abandonment(&self, task: &DeletionTask) -> Result<()> {
        let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
        let journal = slot.as_mut().ok_or_else(cannot_abandon)?;
        if journal.phase() != "abandoned" {
            if journal.metadata()["legacyIrreversible"] == true || !journal.abandon_requested() {
                return Err(cannot_abandon());
            }
            self.restore_registration_capture(journal)?;
            journal.abandon()?;
        } else {
            self.confirm_terminal_deletion(journal)?;
        }
        task.publish(journal);
        task.release_gate();
        Ok(())
    }
    fn finish_deletion_files(&self, task: &Arc<DeletionTask>) -> Result<()> {
        let mut journal = {
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            let journal = slot
                .as_mut()
                .ok_or_else(|| fail("删除文件工作已在执行。"))?;
            if journal.abandon_requested() || journal.phase() == "abandoned" {
                return Ok(());
            }
            journal.reconcile()?;
            self.check_registration_for_deletion(journal)?;
            // The external journal is the gate; new tasks never rewrite the
            // registration or restore stale registration metadata on abandon.
            journal.set_phase("staging")?;
            if let Err(error) = journal.stage_all() {
                let _ = journal.can_abandon();
                task.publish(journal);
                return Err(error);
            }
            journal.set_phase("removing")?;
            task.publish(journal);
            slot.take().unwrap()
        };
        let outcome = (|| -> Result<()> {
            while journal.remove_next_staged()? {
                task.publish(&journal);
            }
            self.remove_confirmed_registration(&mut journal)?;
            journal.mark_completed()?;
            Ok(())
        })();
        if let Err(error) = &outcome {
            let _ = journal.can_abandon();
            let _ = journal.mark_failed(error);
        }
        if outcome.is_ok() {
            task.publish(&journal);
        }
        *task.journal.lock().unwrap_or_else(|p| p.into_inner()) = Some(journal);
        if outcome.is_ok() {
            task.release_gate();
        }
        outcome
    }
    fn registration_owner(
        &self,
        journal: &NodeDeletionJournal,
    ) -> Result<Option<StartIntentOwnership>> {
        let slot = self.root.join(journal.app_id());
        let identity: PersistedIdentity =
            serde_json::from_value(journal.metadata()["rootIdentity"].clone())?;
        if !identity
            .matches_path(&slot)
            .map_err(|e| fail(e.to_string()))?
        {
            return Err(registration_changed());
        }
        let path = slot.join("registration.json");
        match fs::symlink_metadata(&path) {
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && (journal.snapshot()["renamedAny"] == true
                        || !journal.metadata()["registrationRemoval"].is_null()) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let owner = StartIntentOwnership::open_existing(&path)?;
        let original = journal.metadata()["registrationBeforeText"]
            .as_str()
            .ok_or_else(|| fail("删除任务缺少精确项目登记，未继续删除。"))?;
        if owner.contents != original.as_bytes()
            || serde_json::from_slice::<Value>(&owner.contents)?
                != journal.metadata()["registrationBefore"]
        {
            return Err(registration_changed());
        }
        if deletion::revision(&slot)? != journal.metadata()["expectedRegistrationRevision"] {
            return Err(registration_changed());
        }
        Ok(Some(owner))
    }
    fn check_registration_for_deletion(&self, journal: &NodeDeletionJournal) -> Result<()> {
        self.registration_owner(journal).map(|_| ())
    }
    fn registration_transaction(&self, journal: &NodeDeletionJournal) -> PathBuf {
        self.data_root
            .join("node-deletion-tasks")
            .join(journal.task_id())
            .join("registration-removal")
    }
    fn removal_binding(journal: &NodeDeletionJournal) -> Result<Option<RegistrationRemoval>> {
        let value = &journal.metadata()["registrationRemoval"];
        if value.is_null() {
            return Ok(None);
        }
        let binding: RegistrationRemoval = serde_json::from_value(value.clone())?;
        let original = journal.metadata()["registrationBeforeText"]
            .as_str()
            .ok_or_else(registration_changed)?;
        if binding.sha256 != format!("{:x}", Sha256::digest(original.as_bytes()))
            || binding.directory_identity.is_legacy()
            || binding.original_identity.is_legacy()
            || binding
                .restore_identity
                .as_ref()
                .is_some_and(PersistedIdentity::is_legacy)
        {
            return Err(registration_changed());
        }
        Ok(Some(binding))
    }
    fn registration_directory(
        &self,
        path: &Path,
        binding: Option<&RegistrationRemoval>,
    ) -> Result<fs::File> {
        ordinary_path(path, true)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let directory = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = directory.metadata()?;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err(fail("登记移除事务目录权限或归属已变化。"));
            }
        }
        if let Some(binding) = binding {
            let actual =
                PersistedIdentity::from_file(&directory).map_err(|e| fail(e.to_string()))?;
            if !binding.directory_identity.matches_captured(&actual)
                || !binding
                    .directory_identity
                    .matches_path(path)
                    .map_err(|e| fail(e.to_string()))?
            {
                return Err(fail("登记移除事务目录身份已变化，已保留现场。"));
            }
        }
        Ok(directory)
    }
    fn create_registration_directory(&self, path: &Path) -> Result<fs::File> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ordinary_path(path.parent().unwrap(), true)?;
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(path)?;
                fs::File::open(path.parent().unwrap())?.sync_all()?;
            }
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let directory = self.registration_directory(path, None)?;
        if fs::read_dir(path)?.next().is_some() {
            return Err(fail(format!(
                "未绑定的登记移除事务已有内容，已保留：{}",
                path.display()
            )));
        }
        Ok(directory)
    }
    fn check_registration_capture(
        owner: &StartIntentOwnership,
        identity: &PersistedIdentity,
        original: &str,
    ) -> Result<()> {
        let actual = PersistedIdentity::from_file(&owner.file).map_err(|e| fail(e.to_string()))?;
        if !identity.matches_captured(&actual) || owner.contents != original.as_bytes() {
            return Err(registration_changed());
        }
        Ok(())
    }
    fn optional_registration_file(path: &Path) -> Result<Option<StartIntentOwnership>> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
            Ok(_) => StartIntentOwnership::open_existing(path).map(Some),
        }
    }
    fn unlink_registration_transaction_file(
        &self,
        directory: &Path,
        binding: &RegistrationRemoval,
        name: &str,
        owner: &StartIntentOwnership,
    ) -> Result<()> {
        let parent = self.registration_directory(directory, Some(binding))?;
        let current = StartIntentOwnership::open_existing(&directory.join(name))?;
        let expected =
            PersistedIdentity::from_file(&owner.file).map_err(|e| fail(e.to_string()))?;
        let actual =
            PersistedIdentity::from_file(&current.file).map_err(|e| fail(e.to_string()))?;
        if !expected.same_runtime_object(&actual) || owner.contents != current.contents {
            return Err(registration_changed());
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = std::ffi::CString::new(name).map_err(|_| registration_changed())?;
            // The one task worker owns this private transaction; unlink only
            // the identity-checked entry under its retained directory descriptor.
            if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(unix))]
        return Err(fail("当前平台尚未验证登记移除事务。"));
        parent.sync_all()?;
        Ok(())
    }
    fn cleanup_registration_directory(
        &self,
        path: &Path,
        binding: &RegistrationRemoval,
    ) -> Result<()> {
        self.registration_directory(path, Some(binding))?;
        if fs::read_dir(path)?.next().is_some() {
            return Err(fail(format!(
                "登记移除事务还有未确认内容，已保留：{}",
                path.display()
            )));
        }
        fs::remove_dir(path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
    fn remove_confirmed_registration(&self, journal: &mut NodeDeletionJournal) -> Result<()> {
        self.remove_confirmed_registration_with(journal, || Ok(()))
    }
    fn remove_confirmed_registration_with(
        &self,
        journal: &mut NodeDeletionJournal,
        mut captured: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        journal.reconcile()?;
        let path = self.root.join(journal.app_id()).join("registration.json");
        let directory = self.registration_transaction(journal);
        let capture = directory.join("captured-original");
        let original_text = journal.metadata()["registrationBeforeText"]
            .as_str()
            .ok_or_else(registration_changed)?
            .to_owned();
        let original = self.registration_owner(journal)?;
        let mut binding = match Self::removal_binding(journal)? {
            Some(binding) => binding,
            None => {
                let Some(owner) = &original else {
                    if fs::symlink_metadata(&directory).is_ok() {
                        return Err(fail("未能核实登记捕获事务，已保留现场。"));
                    }
                    return Ok(());
                };
                let transaction = self.create_registration_directory(&directory)?;
                let binding = RegistrationRemoval {
                    directory_identity: PersistedIdentity::from_file(&transaction)
                        .map_err(|e| fail(e.to_string()))?,
                    original_identity: PersistedIdentity::from_file(&owner.file)
                        .map_err(|e| fail(e.to_string()))?,
                    sha256: format!("{:x}", Sha256::digest(original_text.as_bytes())),
                    restore_identity: None,
                };
                journal.bind_registration_removal(serde_json::to_value(&binding)?)?;
                binding
            }
        };
        if fs::symlink_metadata(&directory).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            let Some(owner) = &original else {
                return Ok(());
            };
            Self::check_registration_capture(owner, &binding.original_identity, &original_text)?;
            let transaction = self.create_registration_directory(&directory)?;
            binding.directory_identity =
                PersistedIdentity::from_file(&transaction).map_err(|e| fail(e.to_string()))?;
            journal.bind_registration_removal(serde_json::to_value(&binding)?)?;
        }
        self.registration_directory(&directory, Some(&binding))?;
        if let Some(owner) = &original {
            Self::check_registration_capture(owner, &binding.original_identity, &original_text)?;
            if fs::symlink_metadata(&capture).is_ok() {
                return Err(registration_changed());
            }
            move_start_intent_exclusive(&path, &capture)?;
            captured()?;
        }
        if let Some(owner) = Self::optional_registration_file(&capture)? {
            if let Err(error) =
                Self::check_registration_capture(&owner, &binding.original_identity, &original_text)
            {
                // A checked public name can still be replaced before capture.
                // Return a displaced foreign object only without overwriting.
                let _ = move_start_intent_exclusive(&capture, &path);
                return Err(error.detail("capturedRegistration", json!(capture)));
            }
            if fs::symlink_metadata(&path).is_ok() {
                return Err(registration_changed());
            }
            self.unlink_registration_transaction_file(
                &directory,
                &binding,
                "captured-original",
                &owner,
            )?;
        }
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(registration_changed()),
        }
        self.cleanup_registration_directory(&directory, &binding)
    }
    fn restore_registration_capture(&self, journal: &mut NodeDeletionJournal) -> Result<()> {
        let Some(mut binding) = Self::removal_binding(journal)? else {
            return Ok(());
        };
        journal.reconcile()?;
        let directory = self.registration_transaction(journal);
        let path = self.root.join(journal.app_id()).join("registration.json");
        let capture = directory.join("captured-original");
        let restored = directory.join("restored-original");
        let original_text = journal.metadata()["registrationBeforeText"]
            .as_str()
            .ok_or_else(registration_changed)?
            .to_owned();
        if let Some(owner) = Self::optional_registration_file(&path)? {
            let identity =
                PersistedIdentity::from_file(&owner.file).map_err(|e| fail(e.to_string()))?;
            if owner.contents != original_text.as_bytes()
                || !(binding.original_identity.matches_captured(&identity)
                    || binding
                        .restore_identity
                        .as_ref()
                        .is_some_and(|expected| expected.matches_captured(&identity)))
            {
                return Err(registration_changed());
            }
            if fs::symlink_metadata(&directory)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                return Ok(());
            }
            return self.cleanup_registration_directory(&directory, &binding);
        }
        if fs::symlink_metadata(&directory).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            let transaction = self.create_registration_directory(&directory)?;
            binding.directory_identity =
                PersistedIdentity::from_file(&transaction).map_err(|e| fail(e.to_string()))?;
            journal.bind_registration_removal(serde_json::to_value(&binding)?)?;
        }
        self.registration_directory(&directory, Some(&binding))?;
        if let Some(owner) = Self::optional_registration_file(&capture)? {
            Self::check_registration_capture(&owner, &binding.original_identity, &original_text)?;
            move_start_intent_exclusive(&capture, &path)?;
        } else {
            // With no directory ever moved, abandonment also restores the
            // exact platform registration if its unlink preceded a crash.
            use std::io::{Seek, SeekFrom};
            let existing = Self::optional_registration_file(&restored)?;
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            if existing.is_none() {
                options.create_new(true);
            }
            let mut file = options.open(&restored)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let metadata = file.metadata()?;
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.uid() != unsafe { libc::geteuid() }
                {
                    return Err(registration_changed());
                }
            }
            let identity = PersistedIdentity::from_file(&file).map_err(|e| fail(e.to_string()))?;
            if let Some(existing) = &existing {
                let opened = PersistedIdentity::from_file(&existing.file)
                    .map_err(|e| fail(e.to_string()))?;
                if !opened.same_runtime_object(&identity) {
                    return Err(registration_changed());
                }
            }
            if let Some(expected) = &binding.restore_identity {
                if !expected.matches_captured(&identity) {
                    return Err(registration_changed());
                }
            } else {
                if existing.as_ref().is_some_and(|owner| {
                    !owner.contents.is_empty() && owner.contents != original_text.as_bytes()
                }) {
                    return Err(registration_changed());
                }
                binding.restore_identity = Some(identity.clone());
                journal.bind_registration_removal(serde_json::to_value(&binding)?)?;
            }
            file.seek(SeekFrom::Start(0))?;
            file.set_len(0)?;
            file.write_all(original_text.as_bytes())?;
            file.sync_all()?;
            let owner = StartIntentOwnership::open_existing(&restored)?;
            Self::check_registration_capture(&owner, &identity, &original_text)?;
            move_start_intent_exclusive(&restored, &path)?;
        }
        let owner = StartIntentOwnership::open_existing(&path)?;
        let identity =
            PersistedIdentity::from_file(&owner.file).map_err(|e| fail(e.to_string()))?;
        if owner.contents != original_text.as_bytes()
            || !(binding.original_identity.matches_captured(&identity)
                || binding
                    .restore_identity
                    .as_ref()
                    .is_some_and(|expected| expected.matches_captured(&identity)))
        {
            return Err(registration_changed());
        }
        self.cleanup_registration_directory(&directory, &binding)
    }
    fn record_deletion_failure(&self, task: &DeletionTask, error: ApiError) {
        // A failed blocking owner may have taken its journal before panicking.
        // Reload the durable intent; never turn an absent in-memory journal into
        // a permanently running UI with no recovery action.
        {
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_none() {
                let id = task.cached()["taskId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if let Ok(journal) = NodeDeletionJournal::load(&self.data_root, &id) {
                    *slot = Some(journal);
                }
            }
        }
        task.fail(error);
    }
    fn confirm_terminal_deletion(&self, journal: &mut NodeDeletionJournal) -> Result<()> {
        let mut loaded = NodeDeletionJournal::load(&self.data_root, journal.task_id())?;
        if !loaded.is_terminal() {
            *journal = loaded;
            return Err(fail("删除任务尚未确认结束，请再次重试。"));
        }
        if loaded.phase() == "deleted" {
            loaded.reconcile()?;
            match fs::symlink_metadata(self.root.join(loaded.app_id()).join("registration.json")) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
                Ok(_) => return Err(registration_changed()),
            }
        }
        let directory = self
            .data_root
            .join("node-deletion-tasks")
            .join(loaded.task_id());
        let owner = StartIntentOwnership::open_existing(&directory.join("task.json"))?;
        owner.file.sync_all()?;
        fs::File::open(&directory)?.sync_all()?;
        fs::File::open(directory.parent().unwrap())?.sync_all()?;
        *journal = loaded;
        Ok(())
    }
}
fn registration_changed() -> ApiError {
    ApiError::new(
        "NODE_APP_DELETE_CHANGED",
        "删除期间项目登记或实例身份已变化，未继续处理替换对象。",
    )
}
fn cannot_abandon() -> ApiError {
    ApiError::new(
        "NODE_DELETION_CANNOT_ABANDON",
        "已有目录进入待清理位置，或移动结果尚未核实。不能放弃删除，请重试或继续。",
    )
}
fn journal_view(journal: &NodeDeletionJournal) -> Value {
    let mut value = journal.snapshot();
    value["targetRevision"] = journal.metadata()["expectedRegistrationRevision"].clone();
    if journal.metadata()["legacyIrreversible"] == true {
        value["canAbandon"] = json!(false);
    }
    if value["phase"] == "failed" {
        value["canRetry"] = json!(true);
    }
    if !journal.is_terminal() && !journal.metadata()["registrationRemoval"].is_null() {
        if let Some(slot) = journal_instance(journal) {
            let staged = slot
                .parent()
                .and_then(Path::parent)
                .unwrap()
                .join("node-deletion-tasks")
                .join(journal.task_id())
                .join("registration-removal/captured-original");
            value["remaining"].as_array_mut().unwrap().push(json!({
                "path":slot.join("registration.json"),"stagedPath":staged,
                "state":"registration","reason":"登记文件与移除事务仍需收尾"
            }));
        }
    }
    value
}
fn journal_instance(journal: &NodeDeletionJournal) -> Option<PathBuf> {
    let snapshot = journal.snapshot();
    snapshot["targets"].as_array()?.first()?["path"]
        .as_str()
        .and_then(|path| Path::new(path).parent())
        .map(Path::to_path_buf)
}
fn check_observed(value: &Value, observed: u64) -> Result<()> {
    if observed == 0 || observed > value["journalRevision"].as_u64().unwrap_or(0) {
        return Err(fail("删除任务修订不匹配，请刷新任务。"));
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
#[path = "node_deletion_tasks_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "node_deletion_registration_tests.rs"]
mod registration_tests;
