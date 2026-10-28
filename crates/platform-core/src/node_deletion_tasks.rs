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

#[derive(Default)]
pub(super) struct NodeDeletionTasks {
    tasks: SyncMutex<BTreeMap<String, Arc<DeletionTask>>>,
    store: SyncMutex<Option<Arc<ProjectStore>>>,
    abandoned_receipts: SyncMutex<BTreeMap<String, Value>>,
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
                let incomplete = journal.snapshot()["renamedAny"] == true || !can_abandon;
                let mut error = error
                    .outcome(if incomplete {
                        "delete_incomplete"
                    } else {
                        "not_deleted"
                    })
                    .detail("taskId", journal.task_id())
                    .detail("remaining", journal_view(journal)["remaining"].clone());
                error = error.detail("platformRoot", journal.metadata()["ownedPaths"][0].clone());
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
        for journal in NodeDeletionJournal::list(&self.data_root)? {
            if journal.phase() == "deleted" {
                let permit = Some(self.begin_delete_operation(journal.app_id())?);
                let task_id = journal.task_id().to_owned();
                let task = Arc::new(DeletionTask::new(journal, permit));
                task.fail(ApiError::new(
                    "NODE_DELETION_FINALIZATION_REQUIRED",
                    "项目文件已删除，请继续完成缓存和原生存储清理。",
                ));
                tasks.insert(task_id, task);
                continue;
            }
            if journal.is_terminal() {
                match journal.remove_terminal() {
                    Ok(()) => continue,
                    Err(error) => {
                        let permit = Some(self.begin_delete_operation(journal.app_id())?);
                        let task_id = journal.task_id().to_owned();
                        let task = Arc::new(DeletionTask::new(journal, permit));
                        task.fail(error);
                        tasks.insert(task_id, task);
                        continue;
                    }
                }
            }
            let permit = Some(self.begin_delete_operation(journal.app_id())?);
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
            value["result"] = json!({"appId":id,"operationOutcome":"deleted","ownership":value["ownership"],"sourcePreserved":value["ownership"] == "external","evidenceRetained":false,"metadataCleanupPending":false,"durabilityConfirmed":true});
        }
        value
    }
    pub fn deletion_task_status(&self, task_id: &str, app_id: &str) -> Result<Value> {
        if let Some(receipt) = self
            .deletion_tasks
            .abandoned_receipts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(task_id)
            .filter(|value| value["appId"] == app_id)
            .cloned()
        {
            return Ok(receipt);
        }
        match self.deletion_task(task_id, app_id) {
            Ok(task) => Ok(self.task_view(&task)),
            Err(error) => {
                if self.deletion_fully_absent(app_id)? {
                    Ok(absent_deletion(task_id, app_id))
                } else {
                    Err(error)
                }
            }
        }
    }
    pub fn deletion_tasks(&self) -> Result<Vec<Value>> {
        Ok(self
            .deletion_tasks
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|task| {
                !matches!(
                    task.cached()["phase"].as_str(),
                    Some("deleted" | "abandoned")
                )
            })
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
        if let Some(runtime) = &self.runtime {
            self.set_deletion_store(runtime.store())?;
        }
        if self.deletion_fully_absent(id)? {
            return Ok(absent_deletion(&Uuid::new_v4().to_string(), id));
        }
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
                let ownership = self.project_ownership();
                let generated_source = ownership.generated_source(id)?;
                let managed = record.managed.is_some() || generated_source.is_some();
                let mut preserved = vec![];
                if !managed {
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
                let mut paths = vec![slot.clone()];
                paths.extend(ownership.owned_paths(id)?);
                let task_ids = ownership.task_ids(id)?;
                let metadata = json!({"appId":id,"name":name,"ownership":if managed{"managed"}else{"external"},"sourcePath":record.source_path,"expectedRegistrationRevision":input.expected_revision,"registrationBefore":before_value,"registrationBeforeText":before_text,"rootIdentity":PersistedIdentity::capture(&slot).map_err(|e|fail(e.to_string()))?,"preservedPaths":preserved,"ownedPaths":paths,"ownedTaskIds":task_ids,"workspaceRoot":self.deletion_workspace(),"generatedSource":generated_source,"externalOwnedPaths":ownership.external_owned_paths(id)?});
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
        if self.deletion_fully_absent(app_id)? {
            return Ok(absent_deletion(task_id, app_id));
        }
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
                if !journal.is_terminal() {
                    journal.set_phase("waiting")?;
                    task.publish(journal);
                }
                *task.lease.lock().unwrap_or_else(|p| p.into_inner()) = lease;
                task.running.store(true, Ordering::Release);
                true
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
                if !journal.can_abandon()? {
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
                if matches!(
                    task.cached()["phase"].as_str(),
                    Some("deleted" | "abandoned")
                ) || task.running.load(Ordering::Acquire)
                {
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
        if self.deletion_fully_absent(app_id)? {
            return Ok(absent_deletion(task_id, app_id)["result"].clone());
        }
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
            let completed =
                match std::panic::AssertUnwindSafe(apps.run_deletion(&task, before_stop))
                    .catch_unwind()
                    .await
                {
                    Ok(Ok(())) => true,
                    Ok(Err(error)) => {
                        apps.record_deletion_failure(&task, error);
                        false
                    }
                    Err(_) => {
                        apps.record_deletion_failure(
                            &task,
                            fail("删除工作意外结束，任务及目录现场已保留。"),
                        );
                        false
                    }
                };
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
                let (pending, deleted, abandoned) = {
                    let slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
                    (
                        slot.as_ref().is_some_and(|journal| {
                            journal.abandon_requested() && !journal.is_terminal()
                        }),
                        slot.as_ref()
                            .is_some_and(|journal| journal.phase() == "deleted"),
                        slot.as_ref()
                            .is_some_and(|journal| journal.phase() == "abandoned"),
                    )
                };
                if pending && !handled_abandon {
                    continue;
                }
                task.lease.lock().unwrap_or_else(|p| p.into_inner()).take();
                if deleted && completed {
                    let slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(journal) = slot.as_ref() {
                        task.publish(journal);
                    }
                }
                if completed && abandoned {
                    let slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(journal) = slot.as_ref() {
                        task.publish(journal);
                    }
                }
                task.running.store(false, Ordering::Release);
                task.changed.notify_waiters();
                if matches!(
                    task.cached()["phase"].as_str(),
                    Some("deleted" | "abandoned")
                ) {
                    let task_id = task.cached()["taskId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    if task.cached()["phase"] == "abandoned" {
                        let receipt = apps.task_view(&task);
                        apps.deletion_tasks
                            .abandoned_receipts
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(task_id.clone(), receipt);
                        let receipts = apps.deletion_tasks.clone();
                        let receipt_id = task_id.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            receipts
                                .abandoned_receipts
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .remove(&receipt_id);
                        });
                    }
                    let mut tasks = apps
                        .deletion_tasks
                        .tasks
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if tasks
                        .get(&task_id)
                        .is_some_and(|current| Arc::ptr_eq(current, &task))
                    {
                        tasks.remove(&task_id);
                    }
                }
                break;
            }
        });
    }
    async fn run_deletion(
        &self,
        task: &Arc<DeletionTask>,
        before_stop: Option<NodeDeletionBeforeStop>,
    ) -> Result<()> {
        {
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(journal) = slot
                .as_mut()
                .filter(|journal| journal.phase() == "abandoned")
            {
                self.confirm_terminal_deletion(journal)?;
                task.release_gate();
                return Ok(());
            }
        }
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
        let terminal_deleted = task
            .journal
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|journal| journal.phase() == "deleted");
        if terminal_deleted {
            let cleanup = self
                .runtime
                .as_ref()
                .map(crate::RuntimeHandle::project_cleanup)
                .unwrap_or_else(|| {
                    crate::project_cleanup::ProjectCleanupCoordinator::new(&self.data_root)
                });
            let notice = cleanup.prepare(self.node_deletion_notice(task)?).await?;
            cleanup.purge(&notice).await?;
            self.forget_deleted_instance(&id, &permit).await?;
            {
                let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
                let journal = slot.as_mut().ok_or_else(|| fail("删除清理归属缺失。"))?;
                journal.remove_terminal()?;
                task.publish(journal);
                task.release_gate();
            }
            cleanup.committed(&notice).await;
            return Ok(());
        }
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
        if self.inner.lock().await.running.contains_key(&id) {
            return Err(fail("应用仍有资源未停止，未移动删除目录。"));
        }
        let owner_path = self.project_ownership().directory(&id)?;
        match fs::symlink_metadata(&owner_path) {
            Ok(_) => {
                ordinary_path(&owner_path, true)?;
                let owner = crate::durable_writer::WriterOwner::node(&self.data_root, &id)?;
                crate::durable_writer::require_owner_clear(&owner)?;
            }
            // A resumed journal may already own the staged ledger. Never
            // recreate its original namespace while completing deletion.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        let cleanup = self
            .runtime
            .as_ref()
            .map(crate::RuntimeHandle::project_cleanup)
            .unwrap_or_else(|| {
                crate::project_cleanup::ProjectCleanupCoordinator::new(&self.data_root)
            });
        let notice = cleanup.prepare(self.node_deletion_notice(task)?).await?;
        {
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            let journal = slot.as_mut().ok_or_else(|| fail("删除清理归属缺失。"))?;
            journal.set_cleanup_notice(&notice)?;
            task.publish(journal);
        }
        let apps = self.clone();
        let files_task = task.clone();
        tokio::task::spawn_blocking(move || apps.finish_deletion_files(&files_task))
            .await
            .map_err(|_| fail("删除文件工作意外结束，请保留任务现场。"))??;
        cleanup.purge(&notice).await?;
        self.forget_deleted_instance(&id, &permit).await?;
        {
            let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
            let journal = slot.as_mut().ok_or_else(|| fail("删除清理归属缺失。"))?;
            journal.mark_completed()?;
            journal.remove_terminal()?;
            task.publish(journal);
            task.release_gate();
        }
        cleanup.committed(&notice).await;
        Ok(())
    }
    fn node_deletion_notice(
        &self,
        task: &DeletionTask,
    ) -> Result<crate::project_cleanup::DeletedProjectNotice> {
        use crate::project_cleanup::{DeletedProjectNotice, ProjectKind};
        let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
        let journal = slot.as_mut().ok_or_else(|| fail("删除清理归属缺失。"))?;
        if let Some(saved) = journal.metadata().get("cleanupNotice") {
            let mut notice: DeletedProjectNotice = serde_json::from_value(saved.clone())?;
            notice.normalize()?;
            if notice.kind != ProjectKind::Node || notice.id != journal.app_id() {
                return Err(fail("删除清理身份不符。"));
            }
            return Ok(notice);
        }
        if journal.snapshot()["renamedAny"] == false {
            let ownership = self.project_ownership();
            let mut paths = vec![self.root.join(journal.app_id())];
            paths.extend(ownership.owned_paths(journal.app_id())?);
            journal.refresh_owned_paths(
                paths,
                ownership.task_ids(journal.app_id())?,
                ownership.generated_source(journal.app_id())?,
                ownership.external_owned_paths(journal.app_id())?,
            )?;
        }
        let metadata = journal.metadata();
        let tasks: Vec<String> = serde_json::from_value(metadata["ownedTaskIds"].clone())?;
        let sources = metadata["ownedPaths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .filter(|path| {
                path.parent()
                    == Some(
                        self.deletion_workspace()
                            .join(".platform/sources")
                            .as_path(),
                    )
            })
            .filter_map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .collect();
        let mut roots = metadata["sourcePath"]
            .as_str()
            .filter(|path| !path.is_empty())
            .map(|path| vec![PathBuf::from(path)])
            .unwrap_or_default();
        if journal.snapshot()["renamedAny"] == false {
            roots.extend(self.project_ownership().source_roots(journal.app_id())?);
            roots.sort();
            roots.dedup();
        }
        Ok(DeletedProjectNotice::new(
            ProjectKind::Node,
            journal.app_id().into(),
            tasks,
            sources,
            roots,
        ))
    }
    fn finalize_abandonment(&self, task: &DeletionTask) -> Result<()> {
        let mut slot = task.journal.lock().unwrap_or_else(|p| p.into_inner());
        let journal = slot.as_mut().ok_or_else(cannot_abandon)?;
        if journal.phase() != "abandoned" {
            if !journal.abandon_requested() {
                return Err(cannot_abandon());
            }
            journal.abandon()?;
        } else {
            self.confirm_terminal_deletion(journal)?;
        }
        journal.remove_terminal()?;
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
            if journal.snapshot()["renamedAny"] == false {
                let ownership = self.project_ownership();
                let mut paths = vec![self.root.join(journal.app_id())];
                paths.extend(ownership.owned_paths(journal.app_id())?);
                journal.refresh_owned_paths(
                    paths,
                    ownership.task_ids(journal.app_id())?,
                    ownership.generated_source(journal.app_id())?,
                    ownership.external_owned_paths(journal.app_id())?,
                )?;
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
            let ids: Vec<String> =
                serde_json::from_value(journal.metadata()["ownedTaskIds"].clone())?;
            if !ids.is_empty() {
                let manager = self
                    .current_node_task_manager()?
                    .ok_or_else(|| fail("项目任务管理器尚未绑定，删除未完成。"))?;
                manager.forget_owned_tasks(&ids)?;
            }
            Ok(())
        })();
        if let Err(error) = &outcome {
            let _ = journal.can_abandon();
            let _ = journal.mark_failed(error);
        }
        *task.journal.lock().unwrap_or_else(|p| p.into_inner()) = Some(journal);
        outcome
    }
    pub(super) fn deletion_workspace(&self) -> PathBuf {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.info().workspace_root)
            .or_else(|| {
                self.source_workspace
                    .read()
                    .ok()
                    .and_then(|root| root.clone())
            })
            .unwrap_or_else(|| self.data_root.join("workspace"))
    }
    pub(super) fn project_ownership(&self) -> crate::node_project_ownership::NodeProjectOwnership {
        crate::node_project_ownership::NodeProjectOwnership::new(
            self.data_root.clone(),
            self.deletion_workspace(),
        )
    }
    fn deletion_fully_absent(&self, id: &str) -> Result<bool> {
        if !Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id) {
            return Err(fail("应用标识无效。"));
        }
        if crate::webview_storage::StoreLedger::new(self.data_root.clone())
            .contains(crate::project_cleanup::ProjectKind::Node, id)?
        {
            return Ok(false);
        }
        for path in [self.root.join(id), self.project_ownership().directory(id)?] {
            match fs::symlink_metadata(path) {
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(!self.pending_delete_for(id))
    }
    fn check_registration_for_deletion(&self, journal: &NodeDeletionJournal) -> Result<()> {
        let slot = self.root.join(journal.app_id());
        if !slot.exists() && journal.snapshot()["renamedAny"] == true {
            return Ok(());
        }
        let identity: PersistedIdentity =
            serde_json::from_value(journal.metadata()["rootIdentity"].clone())?;
        if !identity
            .matches_path(&slot)
            .map_err(|error| fail(error.to_string()))?
        {
            return Err(registration_changed());
        }
        let before = journal.metadata()["registrationBeforeText"]
            .as_str()
            .ok_or_else(registration_changed)?;
        if read_small(&slot.join("registration.json"))? != before.as_bytes()
            || deletion::revision(&slot)? != journal.metadata()["expectedRegistrationRevision"]
        {
            return Err(registration_changed());
        }
        Ok(())
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
        if !journal.is_terminal() {
            return Err(fail("删除任务尚未结束。"));
        }
        // mark_completed already confirmed each captured target before writing
        // this terminal record. Cleanup may have removed the task directory and
        // then failed its parent fsync, so do not require that directory again.
        journal.remove_terminal()
    }
}
fn absent_deletion(task_id: &str, app_id: &str) -> Value {
    let result = json!({"appId":app_id,"operationOutcome":"deleted","ownership":"absent","sourcePreserved":false,"evidenceRetained":false,"alreadyAbsent":true,"metadataCleanupPending":false,"durabilityConfirmed":true});
    json!({"taskId":task_id,"appId":app_id,"name":app_id,"ownership":"absent","sourcePath":"","phase":"deleted","active":false,"journalRevision":1,"targetRevision":"absent","canAbandon":false,"canRetry":false,"renamedAny":false,"abandonRequested":false,"waitingFor":[],"targets":[],"remaining":[],"preservedPaths":[],"error":null,"result":result})
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
    if value["phase"] == "failed" {
        value["canRetry"] = json!(true);
    }
    value
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
