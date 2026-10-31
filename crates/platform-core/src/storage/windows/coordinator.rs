//! Instance-local ownership only. No persistent scan/cache is adopted here.
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{sync::RwLock, task::JoinHandle};

pub(super) struct Coordinator {
    pub(super) stopping: AtomicBool,
    pub(super) runners: Mutex<Vec<StorageRunner>>,
    pub(super) readers: Arc<RwLock<()>>,
    pub(super) forgotten: Mutex<HashSet<String>>,
}
pub(super) struct StorageRunner {
    pub(super) project_id: String,
    pub(super) handle: JoinHandle<()>,
}

impl Coordinator {
    pub(super) fn new() -> Self {
        Self {
            stopping: AtomicBool::new(false),
            runners: Mutex::new(Vec::new()),
            readers: Arc::new(RwLock::new(())),
            forgotten: Mutex::new(HashSet::new()),
        }
    }
    pub(super) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub(super) async fn quiesce(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let runners = std::mem::take(&mut *self.runners.lock().unwrap());
        let issues = crate::shutdown::join_all(
            "storage",
            runners.into_iter().map(|runner| runner.handle).collect(),
        )
        .await;
        let _drained = self.readers.write().await;
        issues
    }
    pub(super) fn resume(&self) {
        self.stopping.store(false, Ordering::Release);
    }
    pub(super) fn finish_deleted_project(&self, id: &str) {
        self.forgotten.lock().unwrap().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quiesce_waits_for_actual_registered_runner_and_reader() {
        let coordinator = Coordinator::new();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let (finished, finished_rx) = tokio::sync::oneshot::channel();
        coordinator.runners.lock().unwrap().push(StorageRunner {
            project_id: "disposable-project".into(),
            handle: tokio::spawn(async {
                entered.send(()).unwrap();
                release_rx.await.unwrap();
                finished.send(()).unwrap();
            }),
        });
        entered_rx.await.unwrap();
        let reader = coordinator.readers.read().await;
        let quiesce = coordinator.quiesce();
        tokio::pin!(quiesce);
        tokio::select! {
            biased;
            _ = &mut quiesce => panic!("the real registered runner has not ended"),
            _ = tokio::task::yield_now() => {}
        }
        assert!(coordinator.stopping.load(Ordering::Acquire));
        release.send(()).unwrap();
        finished_rx.await.unwrap();
        tokio::select! {
            biased;
            _ = &mut quiesce => panic!("the actual reader lease is still held"),
            _ = tokio::task::yield_now() => {}
        }
        drop(reader);
        let issues = tokio::time::timeout(std::time::Duration::from_secs(2), quiesce)
            .await
            .unwrap();
        assert!(issues.is_empty());
        assert!(coordinator.runners.lock().unwrap().is_empty());
        coordinator.resume();
        assert!(!coordinator.stopping.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn registered_runner_failure_is_reported_without_a_closure_receipt() {
        let coordinator = Coordinator::new();
        coordinator.runners.lock().unwrap().push(StorageRunner {
            project_id: "disposable-project".into(),
            handle: tokio::spawn(async {
                panic!("disposable storage coordinator runner failure");
            }),
        });
        let issues = coordinator.quiesce().await;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].component, "storage");
        assert_eq!(issues[0].code, "SHUTDOWN_JOIN_FAILED");
        assert!(!issues[0].resource_closed);
    }
}
