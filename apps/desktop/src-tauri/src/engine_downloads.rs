//! A narrow engine-write barrier. Only Finished (including failure/cancellation)
//! releases an accepted writer; callback destruction is unknown, never success.
use platform_core::ApiError;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
#[derive(Default)]
struct State {
    closed: bool,
    next: u64,
    active: BTreeSet<u64>,
}
#[derive(Default)]
pub(super) struct EngineDownloads {
    state: Mutex<State>,
    changed: Notify,
}
pub(super) struct EngineTicket {
    owner: Arc<EngineDownloads>,
    id: u64,
    started: bool,
    released: bool,
}
impl EngineDownloads {
    pub fn close(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        self.changed.notify_waiters();
    }
    pub fn admit<T>(
        self: &Arc<Self>,
        admit: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<(T, EngineTicket), ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err(ApiError::new(
                "DESKTOP_DOWNLOADS_STOPPING",
                "平台正在退出，暂不开始新的下载",
            ));
        }
        // Close and IoLifetime admission cannot pass between admission and the
        // actual engine-ticket registration. No WebKit writer exists yet.
        let resource = admit()?;
        state.next += 1;
        let id = state.next;
        state.active.insert(id);
        Ok((
            resource,
            EngineTicket {
                owner: self.clone(),
                id,
                started: false,
                released: false,
            },
        ))
    }
    fn release(&self, id: u64) {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .remove(&id);
        self.changed.notify_waiters();
    }
    pub async fn wait_for_finished(&self) {
        loop {
            let changed = self.changed.notified();
            let complete = {
                let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.closed && state.active.is_empty()
            };
            if complete {
                return;
            }
            changed.await;
        }
    }
}
impl EngineTicket {
    pub fn started(&mut self) {
        self.started = true;
    }
    pub fn finished(mut self) {
        self.owner.release(self.id);
        self.released = true;
    }
}
impl Drop for EngineTicket {
    fn drop(&mut self) {
        if !self.started && !self.released {
            self.owner.release(self.id);
        }
        // Once the callback accepted WebKit's write, a missing Finished leaves
        // this ID in the owned registry, even if the callback map disappears.
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[test]
    fn close_and_admission_use_the_same_lock_and_register_preaccepted_work() {
        let engines = Arc::new(EngineDownloads::default());
        let io = platform_core::io_lifetime::IoLifetime::default();
        let (entered, observed) = std::sync::mpsc::channel();
        let release = Arc::new(std::sync::Barrier::new(2));
        let worker_release = release.clone();
        let worker = engines.clone();
        let admit = std::thread::spawn(move || {
            worker
                .admit(|| {
                    entered.send(()).unwrap();
                    worker_release.wait();
                    io.admit()
                })
                .unwrap()
        });
        observed.recv().unwrap();
        let closer = engines.clone();
        let close = std::thread::spawn(move || closer.close());
        release.wait();
        let (lease, ticket) = admit.join().unwrap();
        close.join().unwrap();
        assert_eq!(engines.state.lock().unwrap().active.len(), 1);
        assert!(engines
            .admit::<()>(|| panic!("sealed admission must not execute"))
            .is_err());
        drop(ticket);
        drop(lease);
        assert!(engines.state.lock().unwrap().active.is_empty());
    }
    #[tokio::test]
    async fn native_destroy_waits_for_finished_but_not_its_own_aggregate_io_scope() {
        let engines = Arc::new(EngineDownloads::default());
        let io = platform_core::io_lifetime::IoLifetime::default();
        let aggregate = io.admit_cleanup().unwrap();
        let (lease, mut ticket) = engines.admit(|| io.admit()).unwrap();
        ticket.started();
        engines.close();
        io.close();
        io.seal();
        let destroyed = Arc::new(AtomicBool::new(false));
        let signal = destroyed.clone();
        let worker = engines.clone();
        let windows = tokio::spawn(async move {
            worker.wait_for_finished().await;
            signal.store(true, Ordering::Release);
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(!destroyed.load(Ordering::Acquire));
        ticket.finished();
        windows.await.unwrap();
        assert!(destroyed.load(Ordering::Acquire));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), io.drain())
                .await
                .is_err()
        );
        drop(lease);
        drop(aggregate);
        io.drain().await;
    }
    #[tokio::test]
    async fn losing_a_started_callback_does_not_synthesize_finished() {
        let engines = Arc::new(EngineDownloads::default());
        let (_, mut ticket) = engines.admit(|| Ok(())).unwrap();
        ticket.started();
        engines.close();
        drop(ticket);
        assert_eq!(engines.state.lock().unwrap().active.len(), 1);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(30),
            engines.wait_for_finished()
        )
        .await
        .is_err());
    }
    #[tokio::test]
    async fn rejected_prestart_and_failed_admission_have_no_engine_writer() {
        let engines = Arc::new(EngineDownloads::default());
        assert!(engines
            .admit::<()>(|| Err(ApiError::new("ARTIFICIAL_REJECTION", "test")))
            .is_err());
        let (_, ticket) = engines.admit(|| Ok(())).unwrap();
        drop(ticket);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            engines.wait_for_finished()
        )
        .await
        .is_err());
        engines.close();
        engines.wait_for_finished().await;
    }
}
