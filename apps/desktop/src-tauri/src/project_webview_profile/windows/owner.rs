//! A project profile remains pinned beyond WindowEvent::Destroyed. Only its
//! own environment's last browser-process exit can release the native owner.
use platform_core::{webview_storage::BoundProfile, ApiError};
use platform_native::windows::files as native;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

fn failure(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserState {
    Preparing,
    Bound(u32),
    Exited(u32),
    Unknown,
}
struct Lifecycle {
    browser: BrowserState,
    sealed: bool,
    destroyed: bool,
    retired: bool,
}
impl Lifecycle {
    fn released(&self) -> bool {
        self.sealed && self.destroyed && matches!(self.browser, BrowserState::Exited(_))
    }
}
pub(super) struct BrowserOwner {
    pub(super) label: String,
    pub(super) generation: u64,
    pub(super) identity: native::FileIdentity,
    profile: Mutex<Option<Arc<BoundProfile>>>,
    state: Mutex<Lifecycle>,
    resources: Mutex<Vec<Box<dyn Send + Sync>>>,
    changed: Notify,
}
impl BrowserOwner {
    pub(super) fn new(
        label: String,
        generation: u64,
        profile: Arc<BoundProfile>,
    ) -> Result<Arc<Self>, ApiError> {
        // This capture is inside the scope already bound to the persisted ID;
        // it cannot turn an unregistered same-name directory into authority.
        profile.verify()?;
        let identity = native::open_no_reparse(profile.directory())?.identity;
        Ok(Arc::new(Self {
            label,
            generation,
            identity,
            profile: Mutex::new(Some(profile)),
            state: Mutex::new(Lifecycle {
                browser: BrowserState::Preparing,
                sealed: false,
                destroyed: false,
                retired: false,
            }),
            resources: Mutex::new(vec![]),
            changed: Notify::new(),
        }))
    }
    pub(super) fn profile(&self) -> Result<Arc<BoundProfile>, ApiError> {
        let profile = self
            .profile
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| {
                failure(
                    "WEBVIEW_PROFILE_RETIRED",
                    "本次浏览器目录凭据已经结束，未重用旧窗口。",
                )
            })?;
        let observed = native::open_no_reparse(profile.directory())?;
        if !self.identity.same_object(&observed.identity) {
            return Err(failure(
                "WEBVIEW_PROFILE_GENERATION_CHANGED",
                "项目浏览器目录与原进程凭据不一致。",
            ));
        }
        Ok(profile)
    }
    pub(super) fn bind_pid(&self, pid: u32) -> Result<(), ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if pid == 0 || state.retired || state.sealed || state.browser == BrowserState::Unknown {
            return Err(failure(
                "WEBVIEW_PROFILE_PROCESS_UNKNOWN",
                "项目浏览器进程无法核实。",
            ));
        }
        if state.browser != BrowserState::Exited(pid) {
            state.browser = BrowserState::Bound(pid);
        }
        Ok(())
    }
    pub(super) fn seal_pid(&self, pid: u32) -> Result<(), ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if pid == 0 || state.retired || state.browser == BrowserState::Unknown {
            return Err(failure(
                "WEBVIEW_PROFILE_PROCESS_UNKNOWN",
                "项目浏览器最后进程无法核实，未释放目录凭据。",
            ));
        }
        if state.sealed {
            if matches!(state.browser, BrowserState::Bound(last) | BrowserState::Exited(last) if last == pid)
            {
                return Ok(());
            }
            state.browser = BrowserState::Unknown;
            return Err(failure(
                "WEBVIEW_PROFILE_PROCESS_UNKNOWN",
                "关闭中的项目浏览器进程已经变化，未释放目录凭据。",
            ));
        }
        if state.browser != BrowserState::Exited(pid) {
            state.browser = BrowserState::Bound(pid);
        }
        state.sealed = true;
        self.changed.notify_waiters();
        Ok(())
    }
    pub(super) fn browser_exited(&self, pid: u32) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.browser == BrowserState::Bound(pid) || state.browser == BrowserState::Preparing {
            state.browser = BrowserState::Exited(pid);
            self.changed.notify_waiters();
        }
    }
    pub(super) fn destroyed(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.destroyed = true;
        if !state.sealed {
            state.browser = BrowserState::Unknown;
        }
        self.changed.notify_waiters();
    }
    pub(super) fn fail(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).browser = BrowserState::Unknown;
        self.changed.notify_waiters();
    }
    pub(super) fn document_allowed(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        !state.sealed && !state.destroyed && matches!(state.browser, BrowserState::Bound(_))
    }
    pub(super) fn ready_to_retire(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .released()
    }
    pub(super) fn hold_resource(
        &self,
        resource: impl Send + Sync + 'static,
    ) -> Result<(), ApiError> {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.retired
            || state.sealed
            || state.destroyed
            || state.browser == BrowserState::Unknown
        {
            return Err(failure(
                "WEBVIEW_PROFILE_RETIRED",
                "项目浏览器目录凭据已经结束。",
            ));
        }
        drop(state);
        resources.push(Box::new(resource));
        Ok(())
    }
    pub(super) fn closed_and_retired(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.released() && state.retired
    }
    pub(super) async fn wait_exit(&self) -> Result<(), ApiError> {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let changed = self.changed.notified();
                {
                    let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    if state.released() {
                        return Ok(());
                    }
                    if state.browser == BrowserState::Unknown {
                        return Err(failure(
                            "WEBVIEW_PROFILE_PROCESS_UNKNOWN",
                            "项目浏览器关闭状态无法核实，已保留目录凭据。",
                        ));
                    }
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| {
            failure(
                "WEBVIEW_PROFILE_EXIT_UNCONFIRMED",
                "项目浏览器进程尚未确认退出，已保留项目存储目录。请稍后重试。",
            )
        })?
    }
    /// Call only on the native thread, after the real exit receipt and a later
    /// main-queue turn removed this generation's environment event handler.
    pub(super) fn retire(&self) -> Result<(), ApiError> {
        {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !state.released() {
                return Err(failure(
                    "WEBVIEW_PROFILE_EXIT_UNCONFIRMED",
                    "浏览器进程退出及本代窗口销毁尚未确认，未释放目录凭据。",
                ));
            }
        }
        let resources =
            std::mem::take(&mut *self.resources.lock().unwrap_or_else(|p| p.into_inner()));
        let profile = self
            .profile
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        // Dropping build pins can schedule core work; hold no owner mutex.
        drop(resources);
        drop(profile);
        self.state.lock().unwrap_or_else(|p| p.into_inner()).retired = true;
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Profiles(Mutex<BTreeMap<String, Arc<BrowserOwner>>>);
fn registry(app: &AppHandle) -> tauri::State<'_, Profiles> {
    if app.try_state::<Profiles>().is_none() {
        app.manage(Profiles::default());
    }
    app.state::<Profiles>()
}
pub(super) fn require_available(app: &AppHandle, label: &str) -> Result<(), ApiError> {
    let registry = registry(app);
    let profiles = registry.0.lock().unwrap_or_else(|p| p.into_inner());
    if profiles
        .get(label)
        .is_some_and(|owner| !owner.closed_and_retired())
    {
        return Err(failure(
            "WEBVIEW_PROFILE_GENERATION_RETAINED",
            "上次项目浏览器进程尚未确认退出，不能重用其存储目录。",
        ));
    }
    Ok(())
}
pub(super) fn register(app: &AppHandle, owner: Arc<BrowserOwner>) -> Result<(), ApiError> {
    let registry = registry(app);
    let mut profiles = registry.0.lock().unwrap_or_else(|p| p.into_inner());
    if profiles
        .get(&owner.label)
        .is_some_and(|previous| !previous.closed_and_retired())
    {
        return Err(failure(
            "WEBVIEW_PROFILE_GENERATION_RETAINED",
            "旧项目浏览器目录凭据仍需核对。",
        ));
    }
    profiles.insert(owner.label.clone(), owner);
    Ok(())
}
pub(super) fn current(
    app: &AppHandle,
    label: &str,
    generation: u64,
) -> Result<Arc<BrowserOwner>, ApiError> {
    registry(app)
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(label)
        .filter(|owner| owner.generation == generation)
        .cloned()
        .ok_or_else(|| {
            failure(
                "WEBVIEW_PROFILE_GENERATION_CHANGED",
                "项目浏览器目录凭据缺失或已经更换，未将窗口消失当作退出确认。",
            )
        })
}
pub(super) fn all(app: &AppHandle) -> Vec<Arc<BrowserOwner>> {
    registry(app)
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .values()
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "owner_tests.rs"]
mod tests;
