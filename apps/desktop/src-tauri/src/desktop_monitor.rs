//! A retained idle/tick task, outside the global I/O lease so drain never waits
//! on itself. Individual accepted ticks keep their own leases until completion.
use super::desktop_shutdown::Issue;
use std::future::Future;
use tokio::{
    runtime::Handle,
    sync::{watch, Mutex},
    task::JoinHandle,
};
struct State {
    task: Option<JoinHandle<()>>,
    result: Option<Result<(), Issue>>,
}
pub(super) struct OwnedMonitor {
    stop: watch::Sender<bool>,
    state: Mutex<State>,
}
impl OwnedMonitor {
    pub fn start<F, W>(runtime: Handle, work: W) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
        W: FnOnce(watch::Receiver<bool>) -> F,
    {
        let (stop, receive) = watch::channel(false);
        Self {
            stop,
            state: Mutex::new(State {
                task: Some(runtime.spawn(work(receive))),
                result: None,
            }),
        }
    }
    pub fn request_stop(&self) {
        self.stop.send_replace(true);
    }
    pub async fn join(&self) -> Result<(), Issue> {
        let mut state = self.state.lock().await;
        if let Some(result) = &state.result {
            return result.clone();
        }
        // Await in the owner. A canceled waiting future never takes/drops the
        // only real handle, and absence alone is never completion evidence.
        let result = match state.task.as_mut() {
            Some(task) => task
                .await
                .map_err(|_| Issue::new("DESKTOP_MONITOR_JOIN_FAILED", false, false)),
            None => Err(Issue::new("DESKTOP_MONITOR_HANDLE_MISSING", false, false)),
        };
        state.result = Some(result.clone());
        state.task = None;
        result
    }
}
pub(super) async fn next_tick(
    stop: &mut watch::Receiver<bool>,
    delay: std::time::Duration,
) -> bool {
    if *stop.borrow() {
        return false;
    }
    tokio::select! {
        _ = stop.changed() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    #[tokio::test]
    async fn idle_stop_wakes_without_waiting_for_the_three_second_timer() {
        let monitor = OwnedMonitor::start(Handle::current(), |mut stop| async move {
            assert!(!next_tick(&mut stop, std::time::Duration::from_secs(3)).await);
        });
        monitor.request_stop();
        tokio::time::timeout(std::time::Duration::from_millis(250), monitor.join())
            .await
            .unwrap()
            .unwrap();
        monitor.join().await.unwrap();
    }
    #[tokio::test]
    async fn accepted_tick_and_lost_join_waiter_keep_the_real_file_worker_until_release() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("artificial-monitor-file");
        std::fs::write(&path, b"artificial preserved bytes").unwrap();
        let io = platform_core::io_lifetime::IoLifetime::default();
        let lease = io.admit().unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        let (started, observed) = tokio::sync::oneshot::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let worker_closed = closed.clone();
        let monitor = Arc::new(OwnedMonitor::start(
            Handle::current(),
            move |_| async move {
                let _lease = lease;
                let file = std::fs::File::open(path).unwrap();
                started.send(()).unwrap();
                wait.await.unwrap();
                drop(file);
                worker_closed.store(true, Ordering::Release);
            },
        ));
        observed.await.unwrap();
        monitor.request_stop();
        io.close();
        io.seal();
        let owner = monitor.clone();
        let waiter = tokio::spawn(async move { owner.join().await });
        tokio::task::yield_now().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), io.drain())
                .await
                .is_err()
        );
        assert!(!closed.load(Ordering::Acquire));
        release.send(()).unwrap();
        monitor.join().await.unwrap();
        io.drain().await;
        assert!(closed.load(Ordering::Acquire));
        assert_eq!(
            std::fs::read(root.path().join("artificial-monitor-file")).unwrap(),
            b"artificial preserved bytes"
        );
    }
    #[tokio::test]
    async fn actual_task_join_failure_is_sticky_across_repeated_stop_and_join() {
        let monitor = OwnedMonitor::start(Handle::current(), |_| async move {
            panic!("controlled artificial monitor panic");
        });
        monitor.request_stop();
        let first = monitor.join().await.unwrap_err();
        assert_eq!(first.code, "DESKTOP_MONITOR_JOIN_FAILED");
        assert!(!first.resource_closed && !first.retryable);
        monitor.request_stop();
        assert_eq!(monitor.join().await.unwrap_err(), first);
    }
}
