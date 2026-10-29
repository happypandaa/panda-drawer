//! Admission and lifetime tracking for native and response-body I/O.
//!
//! A lease proves only that an admitted resource has been dropped. This registry
//! does not issue a safe-exit or updater-install permission.
use crate::ApiError;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
};
use tokio::sync::Notify;
tokio::task_local! { static TRACKED_OPERATION: (); }
pub(crate) fn in_tracked_operation() -> bool {
    TRACKED_OPERATION.try_with(|_| ()).is_ok()
}

#[derive(Clone, Default)]
pub struct IoLifetime(Arc<Inner>);
#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    drained: Notify,
}
#[derive(Default)]
struct State {
    closing: bool,
    sealed: bool,
    active: usize,
    panicked: bool,
    deferred: Vec<Weak<dyn Revoke>>,
    migration_paused: bool,
}
trait Revoke: Send + Sync {
    fn revoke(&self);
}

pub struct IoLease {
    inner: Arc<Inner>,
}

/// A temporary admission fence owned by the migration runner. It never seals
/// the application lifetime; dropping it restores admission unless a real
/// shutdown independently closed or sealed the registry.
pub struct MigrationIoPause(Arc<Inner>);
impl Drop for MigrationIoPause {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().migration_paused = false;
        self.0.drained.notify_waiters();
    }
}
/// Resource drops before the lease, including unwinding and future cancellation.
pub struct TrackedIo<T> {
    resource: T,
    _lease: IoLease,
}
impl<T> TrackedIo<T> {
    pub fn resource(&self) -> &T {
        &self.resource
    }
}
impl<T: FnOnce() -> R, R> TrackedIo<T> {
    pub fn run_blocking(self) -> R {
        let Self { resource, _lease } = self;
        let result = resource();
        drop(_lease);
        result
    }
}
impl<T: Future + Unpin> Future for TrackedIo<T> {
    type Output = T::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        TRACKED_OPERATION.sync_scope((), || Pin::new(&mut self.get_mut().resource).poll(cx))
    }
}
impl IoLease {
    pub fn hold<T>(self, resource: T) -> TrackedIo<T> {
        TrackedIo {
            resource,
            _lease: self,
        }
    }
    /// The spawned task owns both work and lease. Losing the reply future cannot
    /// cancel the work or make drain finish before its resources are destroyed.
    pub async fn run<T: Send + 'static>(
        self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> Result<T, ApiError> {
        tokio::spawn(self.hold(Box::pin(work)))
            .await
            .map_err(|_| ApiError::new("NATIVE_IO_FAILED", "原生操作未正常结束"))
    }
}
impl Drop for IoLease {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        // The resource may have spawned nested work before unwinding. Its
        // lease disappearing is not proof that all such resources are closed.
        state.panicked |= std::thread::panicking();
        state.active -= 1;
        if state.active == 0 {
            self.inner.drained.notify_waiters();
        }
    }
}
fn stopping() -> ApiError {
    ApiError::new("SERVICE_STOPPING", "平台正在停止，无法开始新的操作")
}
fn migrating() -> ApiError {
    ApiError::new(
        "WORKSPACE_MIGRATING",
        "工作区正在切换，操作将在迁移完成后恢复。",
    )
}
impl IoLifetime {
    pub fn check_open(&self) -> Result<(), ApiError> {
        let state = self.0.state.lock().unwrap();
        if state.closing {
            Err(stopping())
        } else if state.migration_paused {
            Err(migrating())
        } else {
            Ok(())
        }
    }
    pub fn admit(&self) -> Result<IoLease, ApiError> {
        self.admit_inner(false)
    }
    /// Only explicit stop/status commands may use this admission while ordinary
    /// native work is closed. Service shutdown seals this admission as well.
    pub fn admit_cleanup(&self) -> Result<IoLease, ApiError> {
        self.admit_inner(true)
    }
    fn admit_inner(&self, cleanup: bool) -> Result<IoLease, ApiError> {
        let mut state = self.0.state.lock().unwrap();
        if state.migration_paused {
            return Err(migrating());
        }
        if state.sealed || (state.closing && !cleanup) {
            return Err(stopping());
        }
        state.active += 1;
        Ok(IoLease {
            inner: self.0.clone(),
        })
    }
    pub fn close(&self) {
        self.close_inner(false);
    }
    pub fn pause_for_migration(&self) -> Result<MigrationIoPause, ApiError> {
        let deferred = {
            let mut state = self.0.state.lock().unwrap();
            if state.closing || state.sealed {
                return Err(stopping());
            }
            if state.migration_paused {
                return Err(migrating());
            }
            state.migration_paused = true;
            std::mem::take(&mut state.deferred)
        };
        // Unselected save resources belong to the old scope and cannot be
        // claimed after this switch. Their actual destructors return leases.
        for resource in deferred.into_iter().filter_map(|value| value.upgrade()) {
            resource.revoke();
        }
        Ok(MigrationIoPause(self.0.clone()))
    }
    pub fn seal(&self) {
        self.close_inner(true);
    }
    fn close_inner(&self, seal: bool) {
        let deferred = {
            let mut state = self.0.state.lock().unwrap();
            state.closing = true;
            state.sealed |= seal;
            std::mem::take(&mut state.deferred)
        };
        // Resource destruction can return leases; never hold admission while
        // destroying resources or waiting for their owners.
        for resource in deferred.into_iter().filter_map(|value| value.upgrade()) {
            resource.revoke();
        }
    }
    pub(crate) fn shutdown_issues(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        if self.0.state.lock().unwrap().panicked {
            vec![crate::shutdown::ShutdownIssue::new(
                "nativeIo", "SHUTDOWN_IO_PANICKED",
                "An owned native or response I/O task panicked; nested resource shutdown remains unconfirmed",
                false, false,
            )]
        } else {
            Vec::new()
        }
    }
    pub async fn drain(&self) {
        loop {
            let notified = self.0.drained.notified();
            if self.0.state.lock().unwrap().active == 0 {
                return;
            }
            notified.await;
        }
    }
    /// Completed private download waiting for save consent. Close revokes it;
    /// claim and closing linearize on the same admission mutex.
    pub fn defer<T: Send + 'static>(
        &self,
        resource: TrackedIo<T>,
    ) -> Result<DeferredIo<T>, ApiError> {
        let slot = Arc::new(DeferredSlot(Mutex::new(Some(resource))));
        let mut state = self.0.state.lock().unwrap();
        if state.closing || state.migration_paused {
            drop(state);
            drop(slot);
            return Err(stopping());
        }
        state.deferred.retain(|item| item.strong_count() > 0);
        let erased: Arc<dyn Revoke> = slot.clone();
        state.deferred.push(Arc::downgrade(&erased));
        Ok(DeferredIo {
            lifetime: self.clone(),
            slot,
        })
    }
}
struct DeferredSlot<T>(Mutex<Option<TrackedIo<T>>>);
impl<T: Send> Revoke for DeferredSlot<T> {
    fn revoke(&self) {
        let resource = self.0.lock().unwrap().take();
        drop(resource);
    }
}
pub struct DeferredIo<T> {
    lifetime: IoLifetime,
    slot: Arc<DeferredSlot<T>>,
}
impl<T> DeferredIo<T> {
    pub fn claim(self) -> Result<TrackedIo<T>, ApiError> {
        let state = self.lifetime.0.state.lock().unwrap();
        if state.closing || state.migration_paused {
            return Err(stopping());
        }
        self.slot.0.lock().unwrap().take().ok_or_else(stopping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct DropEvidence(Arc<AtomicBool>);
    impl Drop for DropEvidence {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn losing_receiver_does_not_release_real_work() {
        let registry = IoLifetime::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let evidence = DropEvidence(dropped.clone());
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let lease = registry.admit().unwrap();
        let caller = tokio::spawn(lease.run(async move {
            let _evidence = evidence;
            let _ = started.send(());
            let _ = wait.await;
        }));
        ready.await.unwrap();
        caller.abort();
        let _ = caller.await;
        registry.seal();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), registry.drain())
                .await
                .is_err()
        );
        assert!(!dropped.load(Ordering::SeqCst));
        release.send(()).unwrap();
        registry.drain().await;
        assert!(dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn close_revokes_unselected_save_and_late_selection_cannot_claim() {
        let registry = IoLifetime::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let deferred = registry
            .defer(
                registry
                    .admit()
                    .unwrap()
                    .hold(DropEvidence(dropped.clone())),
            )
            .unwrap();
        registry.close();
        registry.drain().await;
        assert!(dropped.load(Ordering::SeqCst));
        assert!(deferred.claim().is_err());
        assert!(registry.admit().is_err());
        drop(registry.admit_cleanup().unwrap());
        registry.seal();
        assert!(registry.admit_cleanup().is_err());
    }
    #[tokio::test]
    async fn accepted_selection_remains_owned_until_resource_cleanup() {
        let registry = IoLifetime::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let deferred = registry
            .defer(
                registry
                    .admit()
                    .unwrap()
                    .hold(DropEvidence(dropped.clone())),
            )
            .unwrap();
        let resource = deferred.claim().unwrap();
        registry.seal();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), registry.drain())
                .await
                .is_err()
        );
        drop(resource);
        registry.drain().await;
        assert!(dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn dropping_native_reply_still_drains_the_actual_blocking_file_worker() {
        let temporary = tempfile::TempDir::new().unwrap();
        let file_path = temporary.path().join("artificial-input");
        std::fs::write(&file_path, b"artificial local content").unwrap();
        let file = std::fs::File::open(file_path).unwrap();
        let registry = IoLifetime::default();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let dropped = Arc::new(AtomicBool::new(false));
        let evidence = DropEvidence(dropped.clone());
        let caller = tokio::spawn(registry.admit().unwrap().run(async move {
            tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let _evidence = evidence;
                let mut file = file;
                let _ = started.send(());
                wait.recv().unwrap();
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, b"artificial local content");
                drop(file);
            })
            .await
            .unwrap();
        }));
        ready.await.unwrap();
        caller.abort();
        let _ = caller.await;
        registry.seal();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), registry.drain())
                .await
                .is_err()
        );
        assert!(!dropped.load(Ordering::SeqCst));
        release.send(()).unwrap();
        registry.drain().await;
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn panic_destroys_resources_before_drain_completes() {
        let registry = IoLifetime::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let evidence = DropEvidence(dropped.clone());
        let result: Result<(), ApiError> = registry
            .admit()
            .unwrap()
            .run(async move {
                let _evidence = evidence;
                panic!("controlled artificial worker panic");
            })
            .await;
        assert!(result.is_err());
        registry.seal();
        registry.drain().await;
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn simultaneous_selection_and_close_have_one_resource_owner() {
        for _ in 0..64 {
            let registry = IoLifetime::default();
            let dropped = Arc::new(AtomicBool::new(false));
            let deferred = registry
                .defer(
                    registry
                        .admit()
                        .unwrap()
                        .hold(DropEvidence(dropped.clone())),
                )
                .unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let other = barrier.clone();
            let selection = std::thread::spawn(move || {
                other.wait();
                deferred.claim()
            });
            barrier.wait();
            registry.close();
            let resource = selection.join().unwrap();
            if let Ok(resource) = resource {
                assert!(!dropped.load(Ordering::SeqCst));
                drop(resource);
            }
            registry.drain().await;
            assert!(dropped.load(Ordering::SeqCst));
        }
    }
    #[tokio::test]
    async fn migration_pause_revokes_deferred_resources_and_restores_only_temporary_admission() {
        let registry = IoLifetime::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let deferred = registry
            .defer(
                registry
                    .admit()
                    .unwrap()
                    .hold(DropEvidence(dropped.clone())),
            )
            .unwrap();
        let pause = registry.pause_for_migration().unwrap();
        assert_eq!(registry.admit().err().unwrap().code, "WORKSPACE_MIGRATING");
        assert_eq!(
            registry.admit_cleanup().err().unwrap().code,
            "WORKSPACE_MIGRATING"
        );
        assert!(registry.pause_for_migration().is_err());
        assert!(deferred.claim().is_err());
        registry.drain().await;
        assert!(dropped.load(Ordering::SeqCst));
        drop(pause);
        assert!(registry.admit().is_ok());
        let pause = registry.pause_for_migration().unwrap();
        registry.seal();
        drop(pause);
        assert_eq!(
            registry.admit_cleanup().err().unwrap().code,
            "SERVICE_STOPPING"
        );
    }
}
