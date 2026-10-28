//! Per-instance admission. No process or filesystem wait holds these short locks.
//! Batch one uses request-owned deletion reservations; durable deletion tasks
//! take over the same gate in batch two.
use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex as SyncMutex,
};
use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct NodeOperationCoordinator {
    instances: SyncMutex<BTreeMap<String, Arc<InstanceAdmission>>>,
    next: AtomicU64,
}
#[derive(Default)]
struct InstanceAdmission {
    state: SyncMutex<Admission>,
    changed: Notify,
}
#[derive(Default)]
struct Admission {
    closed: bool,
    operations: BTreeMap<u64, String>,
    window_operations: std::collections::BTreeSet<u64>,
    deletion: Option<(u64, String)>,
}

#[derive(Clone)]
pub struct NodeOperationPermit(Arc<OperationOwner>);
struct OperationOwner {
    coordinator: Arc<NodeOperationCoordinator>,
    instance: Arc<InstanceAdmission>,
    app_id: String,
    token: u64,
}
impl Drop for OperationOwner {
    fn drop(&mut self) {
        let mut state = self
            .instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        state.operations.remove(&self.token);
        state.window_operations.remove(&self.token);
        drop(state);
        self.instance.changed.notify_waiters();
    }
}
impl NodeOperationPermit {
    pub fn keep_alive(&self) -> Arc<dyn Send + Sync> {
        self.0.clone()
    }
    pub(super) fn validate(&self, apps: &NodeApps, id: &str) -> Result<()> {
        if self.0.app_id != id || !Arc::ptr_eq(&self.0.coordinator, &apps.operations) {
            return Err(fail("操作归属不属于此应用。"));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct NodeDeletionPermit(Arc<DeletionOwner>);
struct DeletionOwner {
    coordinator: Arc<NodeOperationCoordinator>,
    instance: Arc<InstanceAdmission>,
    app_id: String,
    token: u64,
    abandon_requested: AtomicBool,
}
impl Drop for DeletionOwner {
    fn drop(&mut self) {
        let mut state = self
            .instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state
            .deletion
            .as_ref()
            .is_some_and(|(token, _)| *token == self.token)
        {
            state.deletion = None;
        }
        drop(state);
        self.instance.changed.notify_waiters();
    }
}
impl NodeDeletionPermit {
    pub(super) fn request_abandon(&self) {
        self.0.abandon_requested.store(true, Ordering::Release);
        self.0.instance.changed.notify_waiters();
    }
    pub fn is_abandon_requested(&self) -> bool {
        self.0.abandon_requested.load(Ordering::Acquire)
    }
    pub async fn wait_abandon_requested(&self) {
        loop {
            let changed = self.0.instance.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_abandon_requested() {
                return;
            }
            changed.await;
        }
    }
    pub async fn wait_window_operations(&self) {
        loop {
            let changed = self.0.instance.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .0
                .instance
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .window_operations
                .is_empty()
            {
                return;
            }
            changed.await;
        }
    }
    pub(super) fn release_gate(&self) {
        let mut state = self
            .0
            .instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state
            .deletion
            .as_ref()
            .is_some_and(|(token, _)| *token == self.0.token)
        {
            state.deletion = None;
        }
        drop(state);
        self.0.instance.changed.notify_waiters();
    }
    pub async fn wait_ready(&self) {
        loop {
            let changed = self.0.instance.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .0
                .instance
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .operations
                .is_empty()
            {
                return;
            }
            changed.await;
        }
    }
    pub fn set_phase(&self, phase: &str) {
        let mut state = self
            .0
            .instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some((token, current)) = &mut state.deletion {
            if *token == self.0.token {
                *current = phase.into();
            }
        }
    }
    pub(super) fn validate(&self, apps: &NodeApps, id: &str) -> Result<()> {
        if self.0.app_id != id || !Arc::ptr_eq(&self.0.coordinator, &apps.operations) {
            return Err(fail("删除归属不属于此应用。"));
        }
        let state = self
            .0
            .instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if !state
            .deletion
            .as_ref()
            .is_some_and(|(token, _)| *token == self.0.token)
            || !state.operations.is_empty()
        {
            return Err(fail("删除尚未取得此应用的独占操作归属。"));
        }
        Ok(())
    }
}

fn deleting() -> ApiError {
    ApiError::new(
        "NODE_APP_DELETING",
        "此应用正在停止并删除，不能开始新的操作。请查看删除进度。",
    )
}
impl NodeOperationCoordinator {
    fn with_checked<T>(
        &self,
        id: &str,
        check: impl FnOnce() -> Result<()>,
        action: impl FnOnce(&Arc<InstanceAdmission>, &mut Admission) -> Result<T>,
    ) -> Result<T> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用标识无效。"));
        }
        let mut instances = self.instances.lock().unwrap_or_else(|p| p.into_inner());
        let created = !instances.contains_key(id);
        let instance = instances.entry(id.into()).or_default().clone();
        let mut state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        let result = if state.closed {
            Err(deleting())
        } else {
            check().and_then(|()| action(&instance, &mut state))
        };
        if result.is_err() && created && state.operations.is_empty() && state.deletion.is_none() {
            instances.remove(id);
        }
        result
    }
    pub(super) fn forget_deleting_instance(
        &self,
        id: &str,
        permit: &NodeDeletionPermit,
    ) -> Result<()> {
        let mut instances = self.instances.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = permit
            .0
            .instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if permit.0.app_id != id
            || !state.operations.is_empty()
            || !state
                .deletion
                .as_ref()
                .is_some_and(|(token, _)| *token == permit.0.token)
        {
            return Err(fail("应用删除归属尚未结束，不能清理内存状态。"));
        }
        if instances
            .get(id)
            .is_some_and(|current| !Arc::ptr_eq(current, &permit.0.instance))
        {
            return Err(fail("应用操作归属已变化。"));
        }
        state.closed = true;
        instances.remove(id);
        Ok(())
    }
    fn instance(&self, id: &str) -> Result<Arc<InstanceAdmission>> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用标识无效。"));
        }
        Ok(self
            .instances
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.into())
            .or_default()
            .clone())
    }
    #[cfg(test)]
    fn admit(self: &Arc<Self>, id: &str, label: &str) -> Result<NodeOperationPermit> {
        self.admit_kind(id, label, false)
    }
    #[cfg(test)]
    fn admit_kind(
        self: &Arc<Self>,
        id: &str,
        label: &str,
        window: bool,
    ) -> Result<NodeOperationPermit> {
        self.admit_checked(id, label, window, || Ok(()))
    }
    fn admit_checked(
        self: &Arc<Self>,
        id: &str,
        label: &str,
        window: bool,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<NodeOperationPermit> {
        self.with_checked(id, check, |instance, state| {
            if state.deletion.is_some() {
                return Err(deleting());
            }
            let token = self.next.fetch_add(1, Ordering::Relaxed);
            state.operations.insert(token, label.into());
            if window {
                state.window_operations.insert(token);
            }
            Ok(NodeOperationPermit(Arc::new(OperationOwner {
                coordinator: self.clone(),
                instance: instance.clone(),
                app_id: id.into(),
                token,
            })))
        })
    }
    fn deletion(self: &Arc<Self>, id: &str) -> Result<NodeDeletionPermit> {
        let instance = self.instance(id)?;
        let mut state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed || state.deletion.is_some() {
            return Err(deleting());
        }
        let token = self.next.fetch_add(1, Ordering::Relaxed);
        state.deletion = Some((token, "waiting".into()));
        drop(state);
        Ok(NodeDeletionPermit(Arc::new(DeletionOwner {
            coordinator: self.clone(),
            instance,
            app_id: id.into(),
            token,
            abandon_requested: AtomicBool::new(false),
        })))
    }
    fn snapshot(&self, id: &str) -> Result<Value> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用标识无效。"));
        }
        let Some(instance) = self
            .instances
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
        else {
            return Ok(json!({"active":false,"phase":null,"waitingFor":[]}));
        };
        let state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        let labels: std::collections::BTreeSet<_> = state.operations.values().cloned().collect();
        Ok(
            json!({"active":state.deletion.is_some(),"phase":state.deletion.as_ref().map(|(_,phase)|phase),"waitingFor":labels}),
        )
    }
}
impl NodeApps {
    fn require_current_owner(&self, id: &str) -> Result<()> {
        self.project_ownership().task_ids(id).map(|_| ())
    }
    pub fn admit_operation(&self, id: &str, label: &str) -> Result<NodeOperationPermit> {
        self.operations
            .admit_checked(id, label, false, || self.require_current_owner(id))
    }
    pub fn admit_window_operation(&self, id: &str, label: &str) -> Result<NodeOperationPermit> {
        self.operations
            .admit_checked(id, label, true, || self.require_current_owner(id))
    }
    pub fn begin_delete_operation(&self, id: &str) -> Result<NodeDeletionPermit> {
        self.operations.deletion(id)
    }
    /// A synchronous window reservation must share the deletion admission
    /// critical section. The callback must not await or re-enter admission.
    pub fn with_operation_admission<T>(
        &self,
        id: &str,
        _label: &str,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.operations.with_checked(
            id,
            || self.require_current_owner(id),
            |_, state| {
                if state.deletion.is_some() {
                    return Err(deleting());
                }
                action()
            },
        )
    }
    pub fn deletion_operation_status(&self, id: &str) -> Result<Value> {
        self.operations.snapshot(id)
    }
    pub(super) fn require_configuration_access(&self, id: &str) -> Result<()> {
        // Existing admitted commits may still read their configuration while
        // deletion waits. Once all owners drain, configuration constructors
        // must not recreate a directory during filesystem removal.
        self.operations.with_checked(
            id,
            || self.require_current_owner(id),
            |_, state| {
                if state.deletion.is_some() && state.operations.is_empty() {
                    return Err(deleting());
                }
                Ok(())
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn completed_deletion_forgets_admission_before_release_and_late_requests_leave_no_cache()
    {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().canonicalize().unwrap();
        let apps = NodeApps::new(data, "/disposable/nonexistent-node".into()).unwrap();
        let id = Uuid::new_v4().to_string();
        let owner = apps.project_ownership();
        owner.initialize(&id).unwrap();
        let admitted = apps
            .admit_operation(&id, "disposable admitted source task")
            .unwrap();
        drop(admitted);
        let deletion = apps.begin_delete_operation(&id).unwrap();
        deletion.wait_ready().await;
        let directory = owner.directory(&id).unwrap();
        crate::history_cleanup::remove_owned_directory(
            &directory,
            &crate::file_identity::PersistedIdentity::capture(&directory).unwrap(),
        )
        .unwrap();
        apps.forget_deleted_instance(&id, &deletion).await.unwrap();
        assert!(apps.operations.instances.lock().unwrap().is_empty());
        assert!(deletion.0.instance.state.lock().unwrap().closed);
        deletion.release_gate();
        assert!(apps.admit_operation(&id, "late start").is_err());
        assert!(apps
            .admit_window_operation(&id, "late native window")
            .is_err());
        assert!(apps
            .with_operation_admission(&id, "late reservation", || Ok(()))
            .is_err());
        assert!(apps.require_configuration_access(&id).is_err());
        assert_eq!(
            apps.deletion_operation_status(&id).unwrap()["active"],
            false
        );
        assert!(apps.operations.instances.lock().unwrap().is_empty());
        assert!(apps.runtime_operations.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn abandonment_waits_for_actual_window_owner_but_not_a_long_git_owner() {
        let ops = Arc::new(NodeOperationCoordinator::default());
        let id = Uuid::new_v4().to_string();
        let _git = ops.admit(&id, "Git 操作").unwrap();
        let window = ops.admit_kind(&id, "创建应用窗口", true).unwrap();
        let native_closure = window.keep_alive();
        let deletion = ops.deletion(&id).unwrap();
        deletion.request_abandon();
        deletion.wait_abandon_requested().await;
        drop(window);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(30),
            deletion.wait_window_operations()
        )
        .await
        .is_err());
        drop(native_closure);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            deletion.wait_window_operations(),
        )
        .await
        .unwrap();
        assert_eq!(
            ops.snapshot(&id).unwrap()["waitingFor"],
            json!(["Git 操作"])
        );
        assert!(ops.admit(&id, "新打开").is_err());
        deletion.release_gate();
        assert!(ops.admit(&id, "用户后来打开").is_ok());
    }

    #[tokio::test]
    async fn deletion_seals_new_admission_and_waits_for_actual_shared_owner() {
        let ops = Arc::new(NodeOperationCoordinator::default());
        let id = Uuid::new_v4().to_string();
        let old = ops.admit(&id, "Git 操作").unwrap();
        let writer = old.keep_alive();
        let deletion = ops.deletion(&id).unwrap();
        drop(old);
        assert_eq!(
            ops.admit(&id, "启动").err().unwrap().code,
            "NODE_APP_DELETING"
        );
        assert!(ops.deletion(&id).is_err());
        assert_eq!(
            ops.snapshot(&id).unwrap()["waitingFor"],
            json!(["Git 操作"])
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), deletion.wait_ready())
                .await
                .is_err()
        );
        let other = Uuid::new_v4().to_string();
        let _independent = ops.admit(&other, "启动").unwrap();
        drop(writer);
        tokio::time::timeout(std::time::Duration::from_secs(1), deletion.wait_ready())
            .await
            .unwrap();
        let retained = deletion.clone();
        drop(deletion);
        assert!(ops.admit(&id, "启动").is_err());
        drop(retained);
        assert!(ops.admit(&id, "启动").is_ok());
    }
}
