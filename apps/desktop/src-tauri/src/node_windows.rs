//! Exactly one native container per Node application. Only the native manager
//! can create it; closing the container revokes its page and stops its backend.
use super::{ApiError, AppState};
use platform_core::app_windows::{AppWindowConfig, WindowIdentity, MIN_HEIGHT, MIN_WIDTH};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Arc, Mutex},
};
use tauri::{
    webview::{NewWindowResponse, PermissionResponse},
    Manager, Url,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Opening,
    Running,
    Restarting,
    Closing,
    Failed,
}
struct Session {
    origin: Option<Url>,
    phase: Phase,
}
#[derive(Default)]
pub(super) struct NodeWindows {
    pub(super) operations: tokio::sync::Mutex<()>,
    sessions: Mutex<BTreeMap<String, Arc<Mutex<Session>>>>,
}
impl NodeWindows {
    fn owns(&self, id: &str, current: &Arc<Mutex<Session>>) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .is_some_and(|owned| Arc::ptr_eq(owned, current))
    }
    fn restart_allowed(&self, id: &str, current: &Arc<Mutex<Session>>) -> bool {
        self.owns(id, current)
            && current.lock().unwrap_or_else(|p| p.into_inner()).phase == Phase::Restarting
    }
}

fn error(message: &str) -> ApiError {
    ApiError::new("NODE_APP_WINDOW_ERROR", message)
}
fn label(id: &str) -> Result<String, ApiError> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(error("应用窗口标识无效。"));
    }
    Ok(format!("node-app-{id}"))
}
fn session(state: &AppState, id: &str) -> Option<Arc<Mutex<Session>>> {
    state
        .node_windows
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .cloned()
}
fn phase(value: &Arc<Mutex<Session>>, next: Phase, origin: Option<Url>) {
    let mut current = value.lock().unwrap_or_else(|p| p.into_inner());
    current.phase = next;
    current.origin = origin;
}
fn owned(state: &AppState, id: &str, current: &Arc<Mutex<Session>>) -> bool {
    state.node_windows.owns(id, current)
}
fn closing(current: &Arc<Mutex<Session>>) -> bool {
    current.lock().unwrap_or_else(|p| p.into_inner()).phase == Phase::Closing
}
fn alive(state: &AppState) -> Result<(), ApiError> {
    if state.shutting_down.load(Ordering::Acquire) {
        Err(error("平台正在退出，暂时不能打开应用。"))
    } else {
        Ok(())
    }
}
fn navigation_allowed(url: &Url, origin: Option<&Url>, allow_blank: bool) -> bool {
    if allow_blank && url.as_str() == "about:blank" {
        return true;
    }
    url.username().is_empty()
        && url.password().is_none()
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && origin.is_some_and(|allowed| url.origin() == allowed.origin())
}
fn focus(window: &tauri::WebviewWindow) -> Result<(), ApiError> {
    window
        .show()
        .and_then(|_| window.unminimize())
        .and_then(|_| window.set_focus())
        .map_err(|_| error("无法显示应用窗口，请关闭后重新打开。"))
}

pub(super) async fn apply_install(
    app: &tauri::AppHandle,
    state: &AppState,
    candidate_id: &str,
) -> Result<Value, ApiError> {
    let _operation = state.node_windows.operations.lock().await;
    alive(state)?;
    let id = state.handle.node_install_target(candidate_id)?;
    if session(state, &id).is_some() || app.get_webview_window(&label(&id)?).is_some() {
        return Err(ApiError::new(
            "NODE_INSTALL_WINDOW_OPEN",
            "请先处理草稿并关闭此应用窗口，等待服务停止后再应用更新。",
        ));
    }
    state
        .handle
        .node_install_apply(state.node_apps.clone(), candidate_id)
        .await
}

pub(super) async fn delete(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
    input: platform_core::node_apps::DeleteNodeAppInput,
) -> Result<Value, ApiError> {
    let _operation = state.node_windows.operations.lock().await;
    alive(state)?;
    if session(state, id).is_some() || app.get_webview_window(&label(id)?).is_some() {
        return Err(ApiError::new(
            "NODE_DELETE_WINDOW_OPEN",
            "请先保存未完成内容并关闭此应用窗口，等待服务停止后再确认永久删除。",
        ));
    }
    state.node_apps.delete(id, input).await
}

pub(super) async fn open(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let _operation = state.node_windows.operations.lock().await;
    alive(state)?;
    let window_label = label(id)?;
    if session(state, id).is_some_and(|current| {
        current.lock().unwrap_or_else(|p| p.into_inner()).phase == Phase::Closing
    }) {
        return Err(error("应用正在关闭，请稍候再打开。"));
    }
    if let Some(window) = app.get_webview_window(&window_label) {
        let current =
            session(state, id).ok_or_else(|| error("应用窗口身份待核对，请先关闭窗口。"))?;
        if matches!(
            current.lock().unwrap_or_else(|p| p.into_inner()).phase,
            Phase::Closing | Phase::Restarting
        ) {
            return Err(error("应用正在关闭或重启，请稍候。"));
        }
        // Reusing the same native document preserves form state and playback;
        // never exchange another bootstrap URL for a second open click.
        focus(&window)?;
        return state.node_apps.status(id).await;
    }
    // Registry removal precedes delivery of Destroyed. Do not create a new
    // generation while an older session or cleanup still owns this app ID.
    if session(state, id).is_some() {
        return Err(error(
            "上次应用窗口尚未完成关闭。请稍候；若创建窗口失败，请重新启动平台后再试。",
        ));
    }
    let detail = state.node_apps.start(id).await?;
    if let Err(error) = alive(state) {
        let _ = state.node_apps.stop_runtime(id).await;
        return Err(error);
    }
    let opened = build(app, state, id, &detail).await;
    if let Err(open_error) = opened {
        if let Some(current) = session(state, id) {
            phase(&current, Phase::Failed, None);
        }
        let cleanup = state.node_apps.stop_runtime(id).await;
        return Err(if cleanup.is_err() {
            error("窗口未能打开，服务关闭状态需要核对。请在平台查看运行状态，不要重复启动。")
        } else {
            open_error
        });
    }
    Ok(detail)
}

async fn build(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
    detail: &Value,
) -> Result<(), ApiError> {
    let launch =
        Url::parse(&state.node_apps.open_url(id).await?).map_err(|_| error("应用入口无效。"))?;
    let current = Arc::new(Mutex::new(Session {
        origin: Some(launch.clone()),
        phase: Phase::Opening,
    }));
    state
        .node_windows
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(id.into(), current.clone());
    let navigation = current.clone();
    let title = detail["name"].as_str().unwrap_or("应用");
    let config = AppWindowConfig::from_value(detail.get("window").filter(|value| !value.is_null()))
        .unwrap_or_default();
    let identity = WindowIdentity::node(id);
    let size = state.window_sizes.initial(app, &identity, config);
    let window =
        tauri::WebviewWindowBuilder::new(app, label(id)?, tauri::WebviewUrl::External(launch))
            .title(super::project_title(title))
            .inner_size(size.width, size.height)
            .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
            .disable_drag_drop_handler()
            .on_navigation(move |url| {
                let current = navigation.lock().unwrap_or_else(|p| p.into_inner());
                navigation_allowed(
                    url,
                    current.origin.as_ref(),
                    matches!(
                        current.phase,
                        Phase::Restarting | Phase::Closing | Phase::Failed
                    ),
                )
            })
            .on_new_window(|_, _| NewWindowResponse::Deny)
            // This source contract does not yet declare camera/microphone grants.
            .on_permission_request(|_, _| PermissionResponse::Deny)
            .build()
            .map_err(|_| error("无法打开应用窗口，请检查系统窗口状态后再试。"))?;
    state.window_sizes.track(&window, identity, config);
    let mut current = current.lock().unwrap_or_else(|p| p.into_inner());
    if current.phase == Phase::Opening {
        current.phase = Phase::Running;
    }
    Ok(())
}

async fn blank(window: &tauri::WebviewWindow) -> Result<(), ApiError> {
    // Discard the business document before stopping, releasing buffered audio,
    // SSE and any page resources even if backend cleanup later fails.
    window
        .navigate(Url::parse("about:blank").unwrap())
        .map_err(|_| error("无法关闭应用页面，请保留窗口并重试。"))?;
    for _ in 0..80 {
        if window.url().is_ok_and(|url| url.as_str() == "about:blank") {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    Err(error("应用页面尚未完成关闭，请重试。"))
}

pub(super) async fn close(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let _operation = state.node_windows.operations.lock().await;
    close_locked(app, state, id).await
}
async fn close_locked(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let current = session(state, id);
    if let Some(current) = &current {
        phase(current, Phase::Closing, None);
    }
    let window = app.get_webview_window(&label(id)?);
    if let Some(window) = &window {
        let _ = window.set_title("应用 · 正在关闭");
        let _ = blank(window).await;
    }
    let stopped = state.node_apps.stop_runtime(id).await;
    // The visible container must not keep a live document after its backend is
    // stopped. Destroy is also requested when blank navigation could not finish.
    if let Some(window) = window {
        if window.destroy().is_err() {
            if let Some(current) = &current {
                phase(current, Phase::Failed, None);
            }
            return Err(error("应用窗口尚未关闭，请再次关闭窗口并核对服务状态。"));
        }
    }
    stopped?;
    state.node_apps.configuration(id).await
}

pub(super) async fn restart(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let _operation = state.node_windows.operations.lock().await;
    alive(state)?;
    let window = app
        .get_webview_window(&label(id)?)
        .ok_or_else(|| error("请先打开应用，再应用新配置。"))?;
    let current = session(state, id).ok_or_else(|| error("应用窗口身份待核对，请先关闭窗口。"))?;
    if current.lock().unwrap_or_else(|p| p.into_inner()).phase == Phase::Closing {
        return Err(error("应用正在关闭，请稍候。"));
    }
    phase(&current, Phase::Restarting, None);
    let _ = window.set_title("应用 · 正在重启");
    if let Err(error) = blank(&window).await {
        let _ = state.node_apps.stop_runtime(id).await;
        if !closing(&current) {
            phase(&current, Phase::Failed, None);
            let _ = window.destroy();
        }
        return Err(error);
    }
    if !state.node_windows.restart_allowed(id, &current) {
        return Err(error("应用正在关闭，已取消重启。"));
    }
    let result = async {
        state.node_apps.stop_runtime(id).await?;
        if !state.node_windows.restart_allowed(id, &current) {
            return Err(error("应用正在关闭，已取消重启。"));
        }
        let detail = state.node_apps.start(id).await?;
        alive(state)?;
        if !state.node_windows.restart_allowed(id, &current) {
            return Err(error("应用正在关闭，已取消重启。"));
        }
        let launch = Url::parse(&state.node_apps.open_url(id).await?)
            .map_err(|_| error("应用入口无效。"))?;
        {
            let mut current = current.lock().unwrap_or_else(|p| p.into_inner());
            if current.phase == Phase::Closing {
                return Err(error("应用正在关闭，已取消重启。"));
            }
            current.phase = Phase::Running;
            current.origin = Some(launch.clone());
        }
        window
            .navigate(launch)
            .map_err(|_| error("无法重新加载应用窗口。"))?;
        let _ = window.set_title(&super::project_title(
            detail["name"].as_str().unwrap_or("应用"),
        ));
        focus(&window)?;
        Ok(detail)
    }
    .await;
    if result.is_err() {
        let _ = state.node_apps.stop_runtime(id).await;
        if !closing(&current) {
            phase(&current, Phase::Failed, None);
            let _ = window.destroy();
        }
    }
    result
}

/// Called synchronously on CloseRequested, before returning to Cocoa.
pub(super) fn close_requested(window: &tauri::Window) -> bool {
    let Some(id) = window.label().strip_prefix("node-app-") else {
        return false;
    };
    let app = window.app_handle();
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    if state.shutting_down.load(Ordering::Acquire) {
        return false;
    }
    let Some(current) = session(&state, id) else {
        return false;
    };
    let Ok(io_lease) = state.handle.io_lifetime().admit_cleanup() else {
        return true;
    };
    {
        let mut current = current.lock().unwrap_or_else(|p| p.into_inner());
        if current.phase == Phase::Closing {
            return true;
        }
        current.phase = Phase::Closing;
        current.origin = None;
    }
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(io_lease.hold(Box::pin(async move {
        let state = app.state::<AppState>();
        let _operation = state.node_windows.operations.lock().await;
        if !owned(&state, &id, &current) {
            return;
        }
        if close_locked(&app, &state, &id).await.is_err() {
            super::show_manager(&app);
            use tauri_plugin_dialog::DialogExt;
            app.dialog().message("应用关闭未完整完成，请在‘我的项目 → 详情 → 运行状态’中核对并重试停止；平台不会自动重新启动。")
                .title("应用关闭状态").show(|_|{});
        }
    })));
    true
}

pub(super) fn destroyed(window: &tauri::Window) {
    let Some(id) = window.label().strip_prefix("node-app-") else {
        return;
    };
    let app = window.app_handle();
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Some(current) = session(&state, id) else {
        return;
    };
    if state.shutting_down.load(Ordering::Acquire) {
        state
            .node_windows
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        return;
    }
    // Closing may still mean a queued close operation. Keep the tombstone until
    // owned cleanup has run even when destruction precedes that queued task.
    // For ordinary close this is an idempotent no-process operation, never a
    // second shutdown message to the child.
    let Ok(io_lease) = state.handle.io_lifetime().admit_cleanup() else {
        return;
    };
    phase(&current, Phase::Closing, None);
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(io_lease.hold(Box::pin(async move {
        let state = app.state::<AppState>();
        let _operation = state.node_windows.operations.lock().await;
        if !owned(&state, &id, &current) {
            return;
        }
        let _ = state.node_apps.stop_runtime(&id).await;
        let mut sessions = state
            .node_windows
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if sessions
            .get(&id)
            .is_some_and(|owned| Arc::ptr_eq(owned, &current))
        {
            sessions.remove(&id);
        }
    })));
}

pub(super) async fn reconcile(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if alive(&state).is_err() {
        return;
    }
    let _operation = state.node_windows.operations.lock().await;
    let active = state.node_apps.running_ids().await;
    let stale: Vec<_> = state
        .node_windows
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|(id, current)| {
            !active.contains(id)
                && current.lock().unwrap_or_else(|p| p.into_inner()).phase == Phase::Running
        })
        .map(|(id, current)| (id.clone(), current.clone()))
        .collect();
    for (id, current) in stale {
        phase(&current, Phase::Closing, None);
        if let Ok(label) = label(&id) {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = blank(&window).await;
                if window.destroy().is_err() {
                    phase(&current, Phase::Failed, None);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "node_windows_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_tracks_only_the_owned_origin_and_controlled_blank() {
        let origin =
            Url::parse("http://127.0.0.1:49152/_platform/bootstrap?token=artificial").unwrap();
        for path in ["http://127.0.0.1:49152/", "http://127.0.0.1:49152/settings"] {
            assert!(navigation_allowed(
                &Url::parse(path).unwrap(),
                Some(&origin),
                false
            ));
        }
        for path in [
            "http://127.0.0.1:49153/",
            "http://localhost:49152/",
            "http://user:pass@127.0.0.1:49152/",
            "https://example.com/",
            "tauri://localhost/",
            "file:///etc/passwd",
            "about:blank",
        ] {
            assert!(!navigation_allowed(
                &Url::parse(path).unwrap(),
                Some(&origin),
                false
            ));
        }
        assert!(navigation_allowed(
            &Url::parse("about:blank").unwrap(),
            None,
            true
        ));
        assert!(!navigation_allowed(
            &Url::parse("about:blank#other").unwrap(),
            None,
            true
        ));
        assert!(!navigation_allowed(
            &Url::parse("http://127.0.0.1:49152/").unwrap(),
            None,
            true
        ));
    }
}
