//! Exactly one native container per Node application. Only the native manager
//! can create it; closing the container revokes its page and stops its backend.
use super::{ApiError, AppState};
use platform_core::app_windows::{AppWindowConfig, WindowIdentity, MIN_HEIGHT, MIN_WIDTH};
use platform_core::node_runtime_trial::RuntimeStopReason;
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
    // Native creation shares its short reservation/registration section with
    // platform exit and shared-source recovery. Process/receipt waits use only
    // instance locks.
    pub(super) operations: Arc<tokio::sync::Mutex<()>>,
    instance_operations: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    sessions: Mutex<BTreeMap<String, Arc<Mutex<Session>>>>,
}
pub(super) struct ShutdownSessions(Vec<(String, Arc<Mutex<Session>>)>);
impl NodeWindows {
    fn instance_mutex(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.instance_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.into())
            .or_default()
            .clone()
    }

    pub(super) async fn instance_operation(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.instance_mutex(id).lock_owned().await
    }

    fn existing_instance_mutex(&self, id: &str) -> Option<Arc<tokio::sync::Mutex<()>>> {
        self.instance_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    async fn existing_instance_operation(
        &self,
        id: &str,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        Some(self.existing_instance_mutex(id)?.lock_owned().await)
    }

    fn try_instance_operation(&self, id: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        self.existing_instance_mutex(id)?.try_lock_owned().ok()
    }

    pub(super) fn seal_project(&self, id: &str) {
        if let Some(current) = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
        {
            phase(current, Phase::Closing, None);
        }
    }

    pub(super) fn forget_project(&self, id: &str) -> Result<(), ApiError> {
        if self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(id)
        {
            return Err(error("应用窗口清理尚未完成，已保留会话记录。"));
        }
        // Late callbacks use lookup-only operations and cannot reinsert this
        // key. Already queued work retains its old Arc and rechecks ownership.
        self.instance_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        Ok(())
    }

    pub fn freeze_for_shutdown(&self) -> ShutdownSessions {
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        ShutdownSessions(
            sessions
                .iter()
                .map(|(id, current)| {
                    phase(current, Phase::Closing, None);
                    (id.clone(), current.clone())
                })
                .collect(),
        )
    }
    pub fn finish_shutdown(
        &self,
        captured: &ShutdownSessions,
        receipts: &super::business_window_lifetime::WindowLifetimes,
    ) -> Result<(), super::desktop_shutdown::Issue> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        for (id, current) in sessions.iter() {
            if !captured
                .0
                .iter()
                .any(|(old, owned)| old == id && Arc::ptr_eq(owned, current))
            {
                return Err(super::desktop_shutdown::Issue::new(
                    "NODE_SESSION_GENERATION_CHANGED",
                    false,
                    false,
                ));
            }
        }
        for (id, _) in &captured.0 {
            if !receipts
                .receipt(&label(id).map_err(|_| {
                    super::desktop_shutdown::Issue::new("NODE_WINDOW_ID_INVALID", false, false)
                })?)
                .is_some_and(|r| r.destroyed())
            {
                return Err(super::desktop_shutdown::Issue::new(
                    "NODE_WINDOW_DESTROY_UNCONFIRMED",
                    false,
                    false,
                ));
            }
        }
        for (id, _) in &captured.0 {
            sessions.remove(id);
        }
        Ok(())
    }
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
    alive(state)?;
    let id = state.handle.node_install_target(candidate_id)?;
    let _admission = state.node_apps.admit_operation(&id, "应用更新")?;
    let native = state
        .window_lifetimes
        .admit_project("node", &id)
        .map_err(|_| error("应用正在删除，暂不能应用修改。"))?;
    let _operation = state.node_windows.instance_operation(&id).await;
    native
        .check()
        .map_err(|_| error("应用正在删除，暂不能应用修改。"))?;
    alive(state)?;
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
    let mut task = submit_delete(app, state, id, input).await?;
    if task["phase"] == "failed" && task["canRetry"] == true {
        task = retry_delete(
            app,
            state,
            task["taskId"].as_str().unwrap_or_default(),
            id,
            task["journalRevision"].as_u64().unwrap_or(0),
        )
        .await?;
    }
    let task_id = task["taskId"]
        .as_str()
        .ok_or_else(|| error("未取得删除任务身份，请查看删除任务后核对。"))?;
    state.node_apps.wait_deletion_result(task_id, id).await
}

pub(super) async fn submit_delete(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
    input: platform_core::node_apps::DeleteNodeAppInput,
) -> Result<Value, ApiError> {
    alive(state)?;
    state.node_apps.set_deletion_store(state.handle.store())?;
    state
        .node_apps
        .submit_deletion_with(id, input, before_delete_stop(app, id))
        .await
}

pub(super) async fn retry_delete(
    app: &tauri::AppHandle,
    state: &AppState,
    task_id: &str,
    id: &str,
    observed_revision: u64,
) -> Result<Value, ApiError> {
    alive(state)?;
    state.node_apps.set_deletion_store(state.handle.store())?;
    state
        .node_apps
        .retry_deletion_with(task_id, id, observed_revision, before_delete_stop(app, id))
        .await
}

fn before_delete_stop(
    app: &tauri::AppHandle,
    id: &str,
) -> platform_core::node_apps::NodeDeletionBeforeStop {
    let app = app.clone();
    let id = id.to_owned();
    Box::new(move |permit| {
        Box::pin(async move {
            let state = app.state::<AppState>();
            let _operation = deletion_window_operation(&state.node_windows, &id, &permit).await;
            teardown_locked(
                &app,
                &state,
                &id,
                Some(&permit),
                RuntimeStopReason::Deletion,
            )
            .await
        })
    })
}

async fn deletion_window_operation(
    windows: &NodeWindows,
    id: &str,
    permit: &platform_core::node_apps::NodeDeletionPermit,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    let operation = windows.existing_instance_operation(id);
    tokio::pin!(operation);
    tokio::select! {
        operation = &mut operation => operation,
        _ = permit.wait_abandon_requested() => {
            // Old native/start requests must finish, including queued main
            // closures. A long unrelated source/Git operation may keep its key
            // and continue; no new window admission can pass the deletion gate.
            permit.wait_window_operations().await;
            None
        }
    }
}

pub(super) async fn open(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let _admission = state.node_apps.admit_window_operation(id, "打开应用")?;
    let native = state
        .window_lifetimes
        .admit_project("node", id)
        .map_err(|_| error("应用正在删除，暂不能打开窗口。"))?;
    let _operation = state.node_windows.instance_operation(id).await;
    native
        .check()
        .map_err(|_| error("应用正在删除，暂不能打开窗口。"))?;
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
        let _ = state
            .node_apps
            .stop_runtime_with_reason(id, RuntimeStopReason::StartupRollback)
            .await;
        return Err(error);
    }
    if let Err(error) = state.node_apps.admit_operation(id, "创建应用窗口") {
        let _ = state
            .node_apps
            .stop_runtime_with_reason(id, RuntimeStopReason::StartupRollback)
            .await;
        return Err(error);
    }
    let opened = build(app, state, id, &detail, _admission.clone(), native).await;
    if let Err(open_error) = opened {
        if let Some(current) = session(state, id) {
            phase(&current, Phase::Failed, None);
        }
        let cleanup = state
            .node_apps
            .stop_runtime_with_reason(id, RuntimeStopReason::StartupRollback)
            .await;
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
    admission: platform_core::node_apps::NodeOperationPermit,
    native: super::business_window_lifetime::ProjectAdmission,
) -> Result<(), ApiError> {
    let launch =
        Url::parse(&state.node_apps.open_url(id).await?).map_err(|_| error("应用入口无效。"))?;
    // A shutdown snapshot must see either the completed native window or no
    // reservation at all. Keep only this short native dispatch/creation section
    // shared; runtime start and open_url have already completed above.
    let registration = state.node_windows.operations.clone().lock_owned().await;
    alive(state)?;
    let id = id.to_owned();
    let detail = detail.clone();
    super::native_ui(app, move |app, state| {
        // Queued native work owns admission independently of its HTTP/Tauri
        // waiter. Deletion cannot finish and reopen its gate before this runs.
        let _admission = admission;
        let _registration = registration;
        native
            .check()
            .map_err(|_| error("应用正在删除，已取消排队的开窗操作。"))?;
        build_on_main(app, state, &id, &detail, launch, &native)
    })
    .await
}
fn build_on_main(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
    detail: &Value,
    launch: Url,
    native: &super::business_window_lifetime::ProjectAdmission,
) -> Result<(), ApiError> {
    state
        .node_apps
        .with_operation_admission(id, "创建应用窗口", || {
            build_on_main_admitted(app, state, id, detail, launch, native)
        })
}
fn build_on_main_admitted(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
    detail: &Value,
    launch: Url,
    native: &super::business_window_lifetime::ProjectAdmission,
) -> Result<(), ApiError> {
    alive(state)?;
    let store_identifier = state
        .handle
        .webview_stores()
        .record(platform_core::project_cleanup::ProjectKind::Node, id)?;
    let receipt = state
        .window_lifetimes
        .reserve_project(&label(id)?, native)
        .map_err(|_| error("上次应用窗口尚未确认关闭，请保留当前状态。"))?;
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
            .data_store_identifier(store_identifier)
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
            // Node engine downloads are not yet a supported source-contract
            // feature. Do not allow WebKit's untracked default file writes.
            .on_download(|_, _| false)
            .on_new_window(|_, _| NewWindowResponse::Deny)
            // This source contract does not yet declare camera/microphone grants.
            .on_permission_request(|_, _| PermissionResponse::Deny)
            .build()
            .map_err(|_| {
                receipt.failed();
                error("无法打开应用窗口，关闭状态须保留核对。")
            })?;
    let event_app = app.clone();
    let event_id = id.to_owned();
    let event_session = current.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            if close_requested_generation(&event_app, &event_id, &event_session) {
                api.prevent_close();
            }
        }
        tauri::WindowEvent::Destroyed => {
            let state = event_app.state::<AppState>();
            state.window_lifetimes.destroyed(&receipt, || {
                if !state.shutting_down.load(Ordering::Acquire) {
                    if let Ok(_lease) = state.handle.io_lifetime().admit_cleanup() {
                        state.window_sizes.flush();
                    }
                }
                state.window_sizes.untrack(&receipt.label);
            });
            destroyed_generation(&event_app, &event_id, &event_session);
        }
        _ => {}
    });
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

async fn wait_destroyed(
    receipt: Arc<super::business_window_lifetime::Receipt>,
) -> Result<(), ApiError> {
    tokio::time::timeout(std::time::Duration::from_secs(5), receipt.wait())
        .await
        .map_err(|_| error("应用窗口尚未确认关闭，请重试停止。"))
}

pub(super) async fn close(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let _admission = state.node_apps.admit_window_operation(id, "关闭应用")?;
    let native = state
        .window_lifetimes
        .admit_project("node", id)
        .map_err(|_| error("应用正在删除，关闭操作由删除任务接管。"))?;
    let _operation = state.node_windows.instance_operation(id).await;
    native
        .check()
        .map_err(|_| error("应用正在删除，关闭操作由删除任务接管。"))?;
    close_locked(app, state, id).await
}
async fn close_locked(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    teardown_locked(app, state, id, None, RuntimeStopReason::WindowClosed).await?;
    state.node_apps.configuration(id).await
}

// The caller already owns this instance's desktop operation. Deletion reuses
// this teardown without calling public close or reading configuration afterward.
async fn teardown_locked(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
    deletion: Option<&platform_core::node_apps::NodeDeletionPermit>,
    reason: RuntimeStopReason,
) -> Result<(), ApiError> {
    if let Some(deletion) = deletion {
        deletion.set_phase("stopping");
    }
    let current = session(state, id);
    if let Some(current) = &current {
        phase(current, Phase::Closing, None);
    }
    let window = app.get_webview_window(&label(id)?);
    let receipt = state.window_lifetimes.receipt(&label(id)?);
    let receipt_required =
        window.is_some() || current.is_some() || receipt.as_ref().is_some_and(|r| !r.destroyed());
    if let Some(window) = &window {
        let _ = window.set_title("应用 · 正在关闭");
        let _ = blank(window).await;
    }
    // The visible container must not keep a live document after its backend is
    // stopped. Request destroy first, including when blanking did not finish.
    // A window failure must not skip stopping the known owned backend.
    let mut window_issue = None;
    if let Some(window) = window {
        if window.destroy().is_err() {
            if let Some(current) = &current {
                phase(current, Phase::Failed, None);
            }
            window_issue = Some(error("应用窗口尚未关闭，请再次关闭窗口并核对服务状态。"));
        }
    }
    if let Some(deletion) = deletion {
        deletion.set_phase("stopping");
    }
    let stopped = state.node_apps.stop_runtime_with_reason(id, reason).await;
    if let Some(issue) = window_issue {
        return Err(issue);
    }
    if receipt_required {
        let receipt = receipt
            .ok_or_else(|| error("应用窗口关闭记录缺失，未继续删除，请重新启动平台后核对。"))?;
        wait_destroyed(receipt).await?;
    }
    stopped?;
    if let Some(current) = current {
        if state
            .window_lifetimes
            .receipt(&label(id)?)
            .is_some_and(|r| r.destroyed())
        {
            let mut sessions = state
                .node_windows
                .sessions
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if sessions
                .get(id)
                .is_some_and(|owned| Arc::ptr_eq(owned, &current))
            {
                sessions.remove(id);
            }
        }
    }
    Ok(())
}

pub(super) async fn prepare_project_deletion(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<(), ApiError> {
    let _operation = state.node_windows.existing_instance_operation(id).await;
    if session(state, id).is_none()
        && app.get_webview_window(&label(id)?).is_none()
        && !state
            .node_apps
            .owned_process_ids()
            .await
            .iter()
            .any(|owned| owned == id)
    {
        return Ok(());
    }
    teardown_locked(app, state, id, None, RuntimeStopReason::Deletion).await
}

pub(super) async fn restart(
    app: &tauri::AppHandle,
    state: &AppState,
    id: &str,
) -> Result<Value, ApiError> {
    let _admission = state.node_apps.admit_window_operation(id, "重启应用")?;
    let native = state
        .window_lifetimes
        .admit_project("node", id)
        .map_err(|_| error("应用正在删除，暂不能重启窗口。"))?;
    let _operation = state.node_windows.instance_operation(id).await;
    native
        .check()
        .map_err(|_| error("应用正在删除，暂不能重启窗口。"))?;
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
        let _ = state
            .node_apps
            .stop_runtime_with_reason(id, RuntimeStopReason::Restart)
            .await;
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
        state
            .node_apps
            .stop_runtime_with_reason(id, RuntimeStopReason::Restart)
            .await?;
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
        state
            .node_apps
            .with_operation_admission(id, "重启应用窗口", || {
                alive(state)?;
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
                focus(&window)
            })?;
        Ok(detail)
    }
    .await;
    if result.is_err() {
        let _ = state
            .node_apps
            .stop_runtime_with_reason(id, RuntimeStopReason::Restart)
            .await;
        if !closing(&current) {
            phase(&current, Phase::Failed, None);
            let _ = window.destroy();
        }
    }
    result
}

/// Called synchronously on CloseRequested, before returning to Cocoa.
fn close_requested_generation(
    app: &tauri::AppHandle,
    id: &str,
    current: &Arc<Mutex<Session>>,
) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    if state.shutting_down.load(Ordering::Acquire) {
        return false;
    }
    if !owned(&state, id, current) {
        return true;
    }
    let current = current.clone();
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
        let Some(_operation) = state.node_windows.existing_instance_operation(&id).await else { return; };
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

fn destroyed_generation(app: &tauri::AppHandle, id: &str, current: &Arc<Mutex<Session>>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if !owned(&state, id, current) || state.shutting_down.load(Ordering::Acquire) {
        return;
    }
    let current = current.clone();
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
        let Some(_operation) = state.node_windows.existing_instance_operation(&id).await else {
            return;
        };
        if !owned(&state, &id, &current) {
            return;
        }
        if state
            .node_apps
            .stop_runtime_with_reason(&id, RuntimeStopReason::WindowClosed)
            .await
            .is_err()
        {
            return;
        }
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
        // Reconcile runs on the same monitor loop as health checks. A busy
        // instance must not delay the next pass for every other application.
        let Some(_operation) = state.node_windows.try_instance_operation(&id) else {
            continue;
        };
        if alive(&state).is_err() || !owned(&state, &id, &current) {
            continue;
        }
        if state.node_apps.running_ids().await.contains(&id) {
            continue;
        }
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
