//! A narrow engine-write barrier. Only Finished (including failure/cancellation)
//! releases an accepted writer; callback destruction is unknown, never success.
use platform_core::{
    io_lifetime::{DeferredIo, IoLease, IoLifetime, MigrationIoPause, TrackedIo},
    ApiError,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
use tokio::sync::Notify;
#[derive(Default)]
struct State {
    closed: bool,
    migration_paused: bool,
    next: u64,
    active: BTreeMap<u64, Option<String>>,
    projects: BTreeMap<String, ProjectIo>,
    handlers: BTreeMap<String, Vec<Weak<AtomicBool>>>,
}
#[derive(Default)]
struct ProjectIo {
    lifetime: IoLifetime,
    sealed: bool,
    drained: bool,
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
pub(super) struct HandlerPermit(Arc<AtomicBool>);
/// Reversible workspace barrier. Existing native documents stay alive, while
/// their old download handlers lose permission only after a committed switch.
pub(super) struct EngineMigrationPause {
    owner: Arc<EngineDownloads>,
    pauses: Vec<MigrationIoPause>,
    handlers: Vec<Weak<AtomicBool>>,
}
impl EngineMigrationPause {
    pub fn commit(&self) {
        for handler in self.handlers.iter().filter_map(Weak::upgrade) {
            handler.store(false, Ordering::Release);
        }
    }
}
impl Drop for EngineMigrationPause {
    fn drop(&mut self) {
        // Resource/lease destructors can reenter the engine owner. Release the
        // private fences before locking it, then expose ordinary admission.
        drop(std::mem::take(&mut self.pauses));
        self.owner
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .migration_paused = false;
        // closed and project.sealed remain permanent independent fences.
        self.owner.changed.notify_waiters();
    }
}
impl HandlerPermit {
    pub fn check(&self) -> Result<(), ApiError> {
        if self.0.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ApiError::new(
                "PROJECT_DOWNLOADS_STOPPING",
                "原项目窗口的下载准入已失效。",
            ))
        }
    }
}
impl EngineDownloads {
    pub fn bind_handler(&self, id: &str) -> HandlerPermit {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let live = Arc::new(AtomicBool::new(
            !state.closed
                && !state.migration_paused
                && !state.projects.get(id).is_some_and(|project| project.sealed),
        ));
        if live.load(Ordering::Acquire) {
            let handlers = state.handlers.entry(id.into()).or_default();
            handlers.retain(|handler| handler.strong_count() > 0);
            handlers.push(Arc::downgrade(&live));
        }
        HandlerPermit(live)
    }
    pub fn project_open(&self, id: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        !state.closed
            && !state.migration_paused
            && state
                .projects
                .get(id)
                .is_some_and(|project| !project.sealed)
    }
    pub fn close(&self) {
        let lifetimes = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.closed = true;
            for handler in state.handlers.values().flatten().filter_map(Weak::upgrade) {
                handler.store(false, Ordering::Release);
            }
            state
                .projects
                .values_mut()
                .map(|project| {
                    project.sealed = true;
                    project.lifetime.clone()
                })
                .collect::<Vec<_>>()
        };
        for lifetime in lifetimes {
            lifetime.close();
        }
        self.changed.notify_waiters();
    }
    pub async fn pause_workspace(self: &Arc<Self>) -> Result<EngineMigrationPause, ApiError> {
        self.pause_workspace_with_timeout(std::time::Duration::from_secs(30))
            .await
    }
    async fn pause_workspace_with_timeout(
        self: &Arc<Self>,
        timeout: std::time::Duration,
    ) -> Result<EngineMigrationPause, ApiError> {
        let (lifetimes, handlers) = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed {
                return Err(ApiError::new(
                    "DESKTOP_DOWNLOADS_STOPPING",
                    "平台正在退出，不能切换下载工作区。",
                ));
            }
            if state.migration_paused {
                return Err(ApiError::new("WORKSPACE_MIGRATING", "下载工作区正在切换。"));
            }
            state.migration_paused = true;
            (
                state
                    .projects
                    .iter()
                    .map(|(id, project)| (id.clone(), project.lifetime.clone(), project.sealed))
                    .collect::<Vec<_>>(),
                state
                    .handlers
                    .values()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        // Construct the rollback owner before any fallible private pause.
        let mut pause = EngineMigrationPause {
            owner: self.clone(),
            pauses: Vec::new(),
            handlers,
        };
        for (id, lifetime, sealed) in &lifetimes {
            if *sealed {
                continue;
            }
            match lifetime.pause_for_migration() {
                Ok(private) => pause.pauses.push(private),
                Err(issue) => {
                    // An independent close may have permanently fenced this
                    // scope after the snapshot. It still participates in drain.
                    let permanently_fenced = {
                        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                        state.closed || state.projects.get(id).is_none_or(|project| project.sealed)
                    };
                    if !permanently_fenced {
                        return Err(issue);
                    }
                }
            }
        }
        tokio::time::timeout(timeout, async {
            loop {
                let changed = self.changed.notified();
                if self
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .active
                    .is_empty()
                {
                    break;
                }
                changed.await;
            }
            for (_, lifetime, _) in &lifetimes {
                lifetime.drain().await;
            }
        })
        .await
        .map_err(|_| {
            ApiError::new(
                "WORKSPACE_DOWNLOAD_FINISH_UNCONFIRMED",
                "尚不能确认下载或文件保存已经结束，已停止工作区迁移；当前工作区可继续使用。",
            )
        })?;
        if self.state.lock().unwrap_or_else(|p| p.into_inner()).closed {
            return Err(ApiError::new(
                "DESKTOP_DOWNLOADS_STOPPING",
                "平台正在退出，不能切换下载工作区。",
            ));
        }
        Ok(pause)
    }
    #[cfg(test)]
    pub fn admit<T>(
        self: &Arc<Self>,
        admit: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<(T, EngineTicket), ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed || state.migration_paused {
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
        state.active.insert(id, None);
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
    pub fn admit_project<T>(
        self: &Arc<Self>,
        id: &str,
        admit: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<(T, IoLease, EngineTicket), ApiError> {
        let (resource, lease, ticket) = self.admit_in_project(id, admit, true)?;
        Ok((resource, lease, ticket.expect("registered engine ticket")))
    }
    /// Save dialogs and actual copies participate in this same project seal.
    /// Waiting dialogs must be passed to defer_project, so seal can revoke them.
    pub fn admit_project_io<T>(
        self: &Arc<Self>,
        id: &str,
        admit: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<(T, IoLease), ApiError> {
        let (resource, lease, _) = self.admit_in_project(id, admit, false)?;
        Ok((resource, lease))
    }
    fn admit_in_project<T>(
        self: &Arc<Self>,
        project_id: &str,
        admit: impl FnOnce() -> Result<T, ApiError>,
        engine: bool,
    ) -> Result<(T, IoLease, Option<EngineTicket>), ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed
            || state.migration_paused
            || state
                .projects
                .get(project_id)
                .is_some_and(|project| project.sealed)
        {
            return Err(ApiError::new(
                "PROJECT_DOWNLOADS_STOPPING",
                "项目正在关闭，不能开始新的文件保存。",
            ));
        }
        let resource = admit()?;
        let project = state.projects.entry(project_id.into()).or_default();
        let lease = match project.lifetime.admit() {
            Ok(lease) => lease,
            Err(issue) => {
                drop(state);
                drop(resource);
                return Err(issue);
            }
        };
        let ticket = if engine {
            state.next += 1;
            let id = state.next;
            state.active.insert(id, Some(project_id.into()));
            Some(EngineTicket {
                owner: self.clone(),
                id,
                started: false,
                released: false,
            })
        } else {
            None
        };
        Ok((resource, lease, ticket))
    }
    pub fn defer_project<T: Send + 'static>(
        &self,
        id: &str,
        resource: TrackedIo<T>,
    ) -> Result<DeferredIo<T>, ApiError> {
        let lifetime = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed
                || state.migration_paused
                || state.projects.get(id).is_none_or(|project| project.sealed)
            {
                drop(state);
                drop(resource);
                return Err(ApiError::new(
                    "PROJECT_DOWNLOADS_STOPPING",
                    "项目文件保存已失效。",
                ));
            }
            state
                .projects
                .get(id)
                .expect("validated project")
                .lifetime
                .clone()
        };
        lifetime.defer(resource)
    }
    pub async fn seal_project_and_drain(&self, id: &str) -> Result<(), ApiError> {
        self.seal_project_and_drain_with_timeout(id, std::time::Duration::from_secs(30))
            .await
    }
    async fn seal_project_and_drain_with_timeout(
        &self,
        id: &str,
        timeout: std::time::Duration,
    ) -> Result<(), ApiError> {
        let lifetime = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(handlers) = state.handlers.get(id) {
                for handler in handlers.iter().filter_map(Weak::upgrade) {
                    handler.store(false, Ordering::Release);
                }
            }
            let project = state.projects.entry(id.into()).or_default();
            project.sealed = true;
            project.lifetime.clone()
        };
        lifetime.close();
        tokio::time::timeout(timeout, async {
            loop {
                let changed = self.changed.notified();
                let active = self
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .active
                    .values()
                    .any(|owner| owner.as_deref() == Some(id));
                if !active {
                    break;
                }
                changed.await;
            }
            lifetime.drain().await;
        })
        .await
        .map_err(|_| {
            ApiError::new(
                "PROJECT_DOWNLOAD_FINISH_UNCONFIRMED",
                "尚不能确认本项目下载或文件保存已经结束，请稍后重试；未删除其写入证据。",
            )
        })?;
        if let Some(project) = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .projects
            .get_mut(id)
        {
            project.drained = true;
        }
        Ok(())
    }
    pub fn forget_project(&self, id: &str) -> Result<(), ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state
            .projects
            .get(id)
            .is_some_and(|project| !project.sealed || !project.drained)
            || state
                .active
                .values()
                .any(|owner| owner.as_deref() == Some(id))
        {
            return Err(ApiError::new(
                "PROJECT_DOWNLOADS_BUSY",
                "项目文件写入尚未结束。",
            ));
        }
        state.projects.remove(id);
        state.handlers.remove(id);
        Ok(())
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
    #[tokio::test]
    async fn project_seal_waits_for_finished_and_actual_copy_without_blocking_another_project() {
        let engines = Arc::new(EngineDownloads::default());
        let (_, a_io, mut a) = engines.admit_project("prj_A", || Ok(())).unwrap();
        a.started();
        let (_, b_io, mut b) = engines.admit_project("prj_B", || Ok(())).unwrap();
        b.started();
        let copy = a_io.hold("actual copy still owns its project I/O");
        let sealing = engines.clone();
        let mut sealed = tokio::spawn(async move { sealing.seal_project_and_drain("prj_A").await });
        tokio::task::yield_now().await;
        assert!(!engines.project_open("prj_A"));
        assert!(engines.project_open("prj_B"));
        assert!(engines
            .admit_project("prj_A", || -> Result<(), ApiError> {
                panic!("sealed admission must not execute")
            })
            .is_err());
        assert!(engines.admit_project_io("prj_B", || Ok(())).is_ok());
        assert!(engines.forget_project("prj_A").is_err());
        a.finished();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut sealed)
                .await
                .is_err()
        );
        drop(copy);
        sealed.await.unwrap().unwrap();
        engines.forget_project("prj_A").unwrap();
        assert!(engines.project_open("prj_B"));
        b.finished();
        drop(b_io);
        engines.close();
        engines.wait_for_finished().await;
    }
    #[tokio::test]
    async fn project_seal_revokes_an_unselected_dialog_and_late_claim_stays_revoked_after_forget() {
        struct Stage(Arc<AtomicBool>);
        impl Drop for Stage {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let engines = Arc::new(EngineDownloads::default());
        let cleaned = Arc::new(AtomicBool::new(false));
        let (_, lease) = engines.admit_project_io("prj_A", || Ok(())).unwrap();
        let dialog = engines
            .defer_project("prj_A", lease.hold(Stage(cleaned.clone())))
            .unwrap();
        engines.seal_project_and_drain("prj_A").await.unwrap();
        assert!(cleaned.load(Ordering::Acquire));
        engines.forget_project("prj_A").unwrap();
        assert!(dialog.claim().is_err());
    }
    #[tokio::test]
    async fn selected_copy_outlives_its_waiter_and_seal_waits_for_the_actual_blocking_worker() {
        let engines = Arc::new(EngineDownloads::default());
        let (_, lease) = engines.admit_project_io("prj_A", || Ok(())).unwrap();
        let copy = engines
            .defer_project("prj_A", lease.hold(()))
            .unwrap()
            .claim()
            .unwrap();
        let (release, waiting) = std::sync::mpsc::channel();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let _copy = copy;
            let _ = entered.send(());
            waiting
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            worker_done.store(true, Ordering::Release);
        });
        entered_rx.await.unwrap();
        drop(worker);
        let sealing = engines.clone();
        let mut sealed = tokio::spawn(async move { sealing.seal_project_and_drain("prj_A").await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut sealed)
                .await
                .is_err()
        );
        assert!(!done.load(Ordering::Acquire));
        release.send(()).unwrap();
        sealed.await.unwrap().unwrap();
        assert!(done.load(Ordering::Acquire));
        engines.forget_project("prj_A").unwrap();
    }
    #[tokio::test]
    async fn a_missing_finished_project_ticket_cannot_be_drained_or_forgotten() {
        let engines = Arc::new(EngineDownloads::default());
        let (_, lease, mut ticket) = engines.admit_project("prj_A", || Ok(())).unwrap();
        ticket.started();
        drop(ticket);
        drop(lease);
        assert_eq!(
            engines
                .seal_project_and_drain_with_timeout("prj_A", std::time::Duration::from_millis(20))
                .await
                .unwrap_err()
                .code,
            "PROJECT_DOWNLOAD_FINISH_UNCONFIRMED"
        );
        assert!(engines.forget_project("prj_A").is_err());
        assert!(engines.admit_project_io("prj_B", || Ok(())).is_ok());
    }
    #[tokio::test]
    async fn a_project_timeout_retains_evidence_and_a_later_real_finished_allows_retry() {
        let engines = Arc::new(EngineDownloads::default());
        let (_, lease, mut ticket) = engines.admit_project("prj_A", || Ok(())).unwrap();
        ticket.started();
        assert_eq!(
            engines
                .seal_project_and_drain_with_timeout("prj_A", std::time::Duration::from_millis(20))
                .await
                .unwrap_err()
                .code,
            "PROJECT_DOWNLOAD_FINISH_UNCONFIRMED"
        );
        assert!(!engines.project_open("prj_A"));
        assert_eq!(engines.state.lock().unwrap().active.len(), 1);
        assert!(engines.forget_project("prj_A").is_err());
        ticket.finished();
        drop(lease);
        engines.seal_project_and_drain("prj_A").await.unwrap();
        engines.forget_project("prj_A").unwrap();
    }
    #[tokio::test]
    async fn rejected_late_admission_and_defer_never_recreate_a_forgotten_project_bucket() {
        let engines = Arc::new(EngineDownloads::default());
        engines.seal_project_and_drain("prj_A").await.unwrap();
        engines.forget_project("prj_A").unwrap();
        assert!(engines
            .admit_project("prj_A", || -> Result<(), ApiError> {
                Err(ApiError::new("PROJECT_REMOVED", "disposable stale handler"))
            })
            .is_err());
        assert!(engines
            .admit_project_io("prj_A", || -> Result<(), ApiError> {
                Err(ApiError::new("PROJECT_REMOVED", "disposable stale dialog"))
            })
            .is_err());
        let resource = IoLifetime::default().admit().unwrap().hold(());
        assert!(engines.defer_project("prj_A", resource).is_err());
        let state = engines.state.lock().unwrap();
        assert!(state.projects.is_empty());
        assert!(state.active.is_empty());
    }
    #[tokio::test]
    async fn an_old_handler_cannot_reenter_after_same_project_reload_with_the_same_document_epoch()
    {
        let engines = Arc::new(EngineDownloads::default());
        let old = engines.bind_handler("prj_A");
        let other = engines.bind_handler("prj_B");
        engines.seal_project_and_drain("prj_A").await.unwrap();
        engines.forget_project("prj_A").unwrap();
        let new = engines.bind_handler("prj_A");
        assert!(engines.admit_project("prj_A", || old.check()).is_err());
        assert!(engines.state.lock().unwrap().projects.is_empty());
        assert!(other.check().is_ok());
        let (_, lease) = engines.admit_project_io("prj_A", || new.check()).unwrap();
        drop(lease);
        engines.seal_project_and_drain("prj_A").await.unwrap();
        engines.forget_project("prj_A").unwrap();
    }

    #[tokio::test]
    async fn migration_revokes_unclaimed_saves_outside_the_engine_lock_and_abort_restores_admission(
    ) {
        struct Stage {
            owner: Weak<EngineDownloads>,
            released_unlocked: Arc<AtomicBool>,
        }
        impl Drop for Stage {
            fn drop(&mut self) {
                self.released_unlocked.store(
                    self.owner
                        .upgrade()
                        .is_some_and(|owner| owner.state.try_lock().is_ok()),
                    Ordering::Release,
                );
            }
        }
        let engines = Arc::new(EngineDownloads::default());
        let handler = engines.bind_handler("prj_migration");
        let (_, lease) = engines
            .admit_project_io("prj_migration", || handler.check())
            .unwrap();
        let released_unlocked = Arc::new(AtomicBool::new(false));
        let deferred = engines
            .defer_project(
                "prj_migration",
                lease.hold(Stage {
                    owner: Arc::downgrade(&engines),
                    released_unlocked: released_unlocked.clone(),
                }),
            )
            .unwrap();
        let pause = engines.pause_workspace().await.unwrap();
        assert!(released_unlocked.load(Ordering::Acquire));
        assert!(!engines.project_open("prj_migration"));
        assert!(engines
            .admit_project_io("prj_migration", || -> Result<(), ApiError> {
                panic!("migration pause must reject before executing resource admission")
            })
            .is_err());
        let paused_handler = engines.bind_handler("prj_migration");
        assert!(paused_handler.check().is_err());
        assert!(
            handler.check().is_ok(),
            "abort must retain the original handler"
        );
        drop(pause);
        assert!(engines.project_open("prj_migration"));
        assert!(
            deferred.claim().is_err(),
            "revoked dialog can never reacquire old material"
        );
        assert!(handler.check().is_ok());
        assert!(paused_handler.check().is_err());
        let (_, fresh_io) = engines
            .admit_project_io("prj_migration", || handler.check())
            .unwrap();
        drop(fresh_io);
    }

    #[tokio::test]
    async fn migration_waits_for_real_engine_finished_and_a_copy_after_its_waiter_is_lost() {
        let engines = Arc::new(EngineDownloads::default());
        let (_, lease, mut ticket) = engines.admit_project("prj_migration", || Ok(())).unwrap();
        ticket.started();
        let copy = engines
            .defer_project("prj_migration", lease.hold(()))
            .unwrap()
            .claim()
            .unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let _copy = copy;
            let _ = entered.send(());
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        });
        ready.await.unwrap();
        drop(worker);
        let owner = engines.clone();
        let mut paused = tokio::spawn(async move { owner.pause_workspace().await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut paused)
                .await
                .is_err()
        );
        ticket.finished();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut paused)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let pause = paused.await.unwrap().unwrap();
        assert!(engines.state.lock().unwrap().active.is_empty());
        assert!(engines.state.lock().unwrap().migration_paused);
        drop(pause);
        assert!(engines.admit_project_io("prj_migration", || Ok(())).is_ok());
    }

    #[tokio::test]
    async fn migration_timeout_keeps_unconfirmed_writers_and_restores_the_current_workspace() {
        let engines = Arc::new(EngineDownloads::default());
        let handler = engines.bind_handler("prj_migration");
        let (_, lease, mut ticket) = engines
            .admit_project("prj_migration", || handler.check())
            .unwrap();
        ticket.started();
        drop(lease);
        let error = engines
            .pause_workspace_with_timeout(std::time::Duration::from_millis(20))
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "WORKSPACE_DOWNLOAD_FINISH_UNCONFIRMED");
        assert_eq!(engines.state.lock().unwrap().active.len(), 1);
        assert!(!engines.state.lock().unwrap().migration_paused);
        assert!(handler.check().is_ok());
        assert!(engines.admit_project_io("prj_other", || Ok(())).is_ok());
        ticket.finished();
        let pause = engines.pause_workspace().await.unwrap();
        drop(pause);

        let (_, lease, mut missing) = engines.admit_project("prj_missing", || Ok(())).unwrap();
        missing.started();
        drop(missing);
        drop(lease);
        assert!(engines
            .pause_workspace_with_timeout(std::time::Duration::from_millis(20))
            .await
            .is_err());
        assert_eq!(
            engines.state.lock().unwrap().active.len(),
            1,
            "missing callback is not Finished"
        );
        assert!(!engines.state.lock().unwrap().migration_paused);
    }

    #[tokio::test]
    async fn migration_commit_retires_only_captured_handlers_and_never_reopens_permanent_fences() {
        let engines = Arc::new(EngineDownloads::default());
        let old = engines.bind_handler("prj_migration");
        let (_, lease) = engines
            .admit_project_io("prj_migration", || old.check())
            .unwrap();
        drop(lease);
        let pause = engines.pause_workspace().await.unwrap();
        assert!(engines.pause_workspace().await.is_err());
        assert!(engines.state.lock().unwrap().migration_paused);
        let rejected = engines.bind_handler("prj_migration");
        pause.commit();
        assert!(old.check().is_err());
        drop(pause);
        assert!(rejected.check().is_err());
        let fresh = engines.bind_handler("prj_migration");
        let (_, lease) = engines
            .admit_project_io("prj_migration", || fresh.check())
            .unwrap();
        drop(lease);
        let pause = engines.pause_workspace().await.unwrap();
        engines
            .seal_project_and_drain("prj_migration")
            .await
            .unwrap();
        drop(pause);
        assert!(!engines.project_open("prj_migration"));
        assert!(engines.bind_handler("prj_migration").check().is_err());
        assert!(engines.bind_handler("prj_other").check().is_ok());
        let pause = engines.pause_workspace().await.unwrap();
        engines.close();
        drop(pause);
        assert!(engines.bind_handler("prj_after_shutdown").check().is_err());
        assert!(engines
            .admit_project_io("prj_after_shutdown", || Ok(()))
            .is_err());
    }
}
