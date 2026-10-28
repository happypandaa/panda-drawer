//! Per-instance admission. No process or filesystem wait holds these short locks.
//! Batch one uses request-owned deletion reservations; durable deletion tasks
//! take over the same gate in batch two.
use super::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
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
    operations: BTreeMap<u64, String>,
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
        self.instance
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .operations
            .remove(&self.token);
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
    fn admit(self: &Arc<Self>, id: &str, label: &str) -> Result<NodeOperationPermit> {
        let instance = self.instance(id)?;
        let mut state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.deletion.is_some() {
            return Err(deleting());
        }
        let token = self.next.fetch_add(1, Ordering::Relaxed);
        state.operations.insert(token, label.into());
        drop(state);
        Ok(NodeOperationPermit(Arc::new(OperationOwner {
            coordinator: self.clone(),
            instance,
            app_id: id.into(),
            token,
        })))
    }
    fn deletion(self: &Arc<Self>, id: &str) -> Result<NodeDeletionPermit> {
        let instance = self.instance(id)?;
        let mut state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.deletion.is_some() {
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
        })))
    }
    fn snapshot(&self, id: &str) -> Result<Value> {
        let instance = self.instance(id)?;
        let state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        let labels: std::collections::BTreeSet<_> = state.operations.values().cloned().collect();
        Ok(
            json!({"active":state.deletion.is_some(),"phase":state.deletion.as_ref().map(|(_,phase)|phase),"waitingFor":labels}),
        )
    }
}
impl NodeApps {
    pub fn admit_operation(&self, id: &str, label: &str) -> Result<NodeOperationPermit> {
        self.operations.admit(id, label)
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
        let instance = self.operations.instance(id)?;
        let state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.deletion.is_some() {
            return Err(deleting());
        }
        action()
    }
    pub fn deletion_operation_status(&self, id: &str) -> Result<Value> {
        self.operations.snapshot(id)
    }
    pub(super) fn require_configuration_access(&self, id: &str) -> Result<()> {
        let instance = self.operations.instance(id)?;
        let state = instance.state.lock().unwrap_or_else(|p| p.into_inner());
        // Existing admitted commits may still read their configuration while
        // deletion waits. Once all owners drain, configuration constructors
        // must not recreate a directory during filesystem removal.
        if state.deletion.is_some() && state.operations.is_empty() {
            return Err(deleting());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
