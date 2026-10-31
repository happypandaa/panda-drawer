//! Instance-local tasks and plans only; persistent recovery is not inventoried.
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tokio::task::JoinHandle;

pub(super) struct Coordinator {
    pub(super) stopping: AtomicBool,
    pub(super) running: Mutex<Vec<JoinHandle<()>>>,
    pub(super) latest: Mutex<Option<String>>,
    pub(super) deleting: Mutex<BTreeSet<String>>,
}

impl Coordinator {
    pub(super) fn new() -> Self {
        Self {
            stopping: AtomicBool::new(false),
            running: Mutex::new(Vec::new()),
            latest: Mutex::new(None),
            deleting: Mutex::new(BTreeSet::new()),
        }
    }
    pub(super) fn request_stop(&self) {
        let _submission = self.running.lock().unwrap();
        self.stopping.store(true, Ordering::Release);
    }
    pub(super) async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.request_stop();
        let running = std::mem::take(&mut *self.running.lock().unwrap());
        crate::shutdown::join_all("historyCleanup", running).await
    }
    pub(super) fn finish_deleted_project(&self, id: &str) {
        self.deleting.lock().unwrap().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initial_state_is_instance_local_and_shutdown_joins_its_real_runner() {
        let coordinator = Coordinator::new();
        assert!(coordinator.latest.lock().unwrap().is_none());
        assert!(!coordinator.stopping.load(Ordering::Acquire));
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        coordinator
            .running
            .lock()
            .unwrap()
            .push(tokio::spawn(async {
                entered.send(()).unwrap();
                release_rx.await.unwrap();
            }));
        entered_rx.await.unwrap();
        let shutdown = coordinator.shutdown();
        tokio::pin!(shutdown);
        tokio::select! {
            biased;
            _ = &mut shutdown => panic!("the actual registered runner is still alive"),
            _ = tokio::task::yield_now() => {}
        }
        assert!(coordinator.stopping.load(Ordering::Acquire));
        release.send(()).unwrap();
        let issues = tokio::time::timeout(std::time::Duration::from_secs(2), shutdown)
            .await
            .unwrap();
        assert!(issues.is_empty());
        assert!(coordinator.running.lock().unwrap().is_empty());
        assert!(coordinator.latest.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn registered_runner_failure_is_reported_without_a_closure_receipt() {
        let coordinator = Coordinator::new();
        coordinator
            .running
            .lock()
            .unwrap()
            .push(tokio::spawn(async {
                panic!("disposable history cleanup coordinator runner failure");
            }));
        let issues = coordinator.shutdown().await;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].component, "historyCleanup");
        assert_eq!(issues[0].code, "SHUTDOWN_JOIN_FAILED");
        assert!(!issues[0].resource_closed);
    }
}
