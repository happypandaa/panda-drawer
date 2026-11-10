//! Preserve the native window and profile while replacing an application document.
//! A completed blank navigation separates old engine callbacks from the new epoch.
use super::{ApiError, AppState};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{Emitter, Manager};

type DownloadHandler =
    dyn for<'a> Fn(tauri::Webview, tauri::webview::DownloadEvent<'a>) -> bool + Send + Sync;
pub(super) struct Document {
    titles: Mutex<[String; 2]>,
    download: Mutex<Option<Box<DownloadHandler>>>,
    lease: Mutex<Option<platform_core::WebWindowLease>>,
    blank: tokio::sync::watch::Sender<bool>,
    pub close_requested: AtomicBool,
    switching: AtomicBool,
}
impl Document {
    pub fn new(lease: Option<platform_core::WebWindowLease>, titles: [String; 2]) -> Arc<Self> {
        Arc::new(Self {
            titles: Mutex::new(titles),
            download: Mutex::new(None),
            lease: Mutex::new(lease),
            blank: tokio::sync::watch::channel(false).0,
            close_requested: AtomicBool::new(false),
            switching: AtomicBool::new(false),
        })
    }
    pub fn localized_name(&self) -> String {
        self.titles.lock().unwrap_or_else(|p| p.into_inner())
            [usize::from(super::ui_locale::current() == "en")]
        .clone()
    }
    pub fn bind_download(
        &self,
        handler: impl for<'a> Fn(tauri::Webview, tauri::webview::DownloadEvent<'a>) -> bool
            + Send
            + Sync
            + 'static,
    ) {
        *self.download.lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(handler));
    }
    pub fn download(&self, view: tauri::Webview, event: tauri::webview::DownloadEvent<'_>) -> bool {
        self.download
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|handler| handler(view, event))
    }
    pub fn page_loaded(&self, payload: tauri::webview::PageLoadPayload<'_>) {
        if payload.url().as_str() == "about:blank"
            && payload.event() == tauri::webview::PageLoadEvent::Finished
        {
            self.blank.send_replace(true);
        }
    }
}
pub(super) fn title_names(manifest: &platform_core::panda_manifest::PandaManifest) -> [String; 2] {
    [
        manifest.localized_app("zh-CN").name,
        manifest.localized_app("en").name,
    ]
}
pub(super) fn localized_name(app: &tauri::AppHandle, id: &str) -> Option<String> {
    document(app, id).map(|current| current.localized_name())
}
fn document(app: &tauri::AppHandle, id: &str) -> Option<Arc<Document>> {
    app.state::<AppState>()
        .documents
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .cloned()
}
pub(super) fn validate_window_action(
    app: &tauri::AppHandle,
    id: &str,
    expected: u64,
) -> Result<(), ApiError> {
    let state = app.state::<AppState>();
    let label = super::project_label(id);
    let receipt = state.window_lifetimes.receipt(&label);
    if !receipt.as_ref().is_some_and(|receipt| {
        receipt.generation == expected
            && !receipt.destroyed()
            && state.window_lifetimes.is_current(receipt)
    }) || app.get_webview_window(&label).is_none()
        || document(app, id).is_none_or(|document| document.close_requested.load(Ordering::Acquire))
    {
        return Err(ApiError::new(
            "APP_WINDOW_ACTION_EXPIRED",
            "原应用窗口已关闭，此标题栏操作已失效。",
        ));
    }
    Ok(())
}
pub(super) fn install(app: &tauri::AppHandle) {
    let owner = app.clone();
    app.state::<AppState>()
        .handle
        .set_app_reload_hook(Arc::new(move |notice| {
            let app = owner.clone();
            Box::pin(async move { dispatch(app, notice).await })
        }));
    let mut requests = app
        .state::<AppState>()
        .handle
        .subscribe_management_requests();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match requests.recv().await {
                Ok(request) => {
                    super::show_manager(&app);
                    let _ = app.emit_to("manager", "panda-management-confirmation", request);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
async fn dispatch(app: tauri::AppHandle, notice: Value) -> Result<(), ApiError> {
    let id = notice["appId"]
        .as_str()
        .ok_or_else(|| ApiError::new("APP_RELOAD_INVALID", "缺少应用身份。"))?
        .to_owned();
    match notice["phase"].as_str() {
        Some("prepare") => prepare(&app, &id).await,
        Some("ready") => {
            ready(&app, &id).await?;
            app.state::<AppState>()
                .reload_documents
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            let payload = notice.get("result").cloned().unwrap_or(notice);
            app.emit_to("manager", "panda-app-reload", payload)
                .map_err(|e| ApiError::new("APP_RELOAD_NOTIFY_FAILED", e.to_string()))
        }
        Some("failed") => {
            let previous = app
                .state::<AppState>()
                .reload_documents
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            let user_closed = previous
                .flatten()
                .is_some_and(|document| document.close_requested.load(Ordering::Acquire));
            if !user_closed {
                show_failure(&app, &id, &notice).await?;
            }
            super::show_manager(&app);
            let payload = notice.get("result").cloned().unwrap_or(notice);
            app.emit_to("manager", "panda-app-reload", payload)
                .map_err(|e| ApiError::new("APP_RELOAD_NOTIFY_FAILED", e.to_string()))
        }
        Some("needsConfiguration" | "invalidManifest") => {
            super::show_manager(&app);
            let payload = notice.get("result").cloned().unwrap_or(notice);
            app.emit_to("manager", "panda-app-reload", payload)
                .map_err(|e| ApiError::new("APP_RELOAD_NOTIFY_FAILED", e.to_string()))
        }
        _ => Err(ApiError::new(
            "APP_RELOAD_INVALID",
            "无法识别重新加载阶段。",
        )),
    }
}
async fn prepare(app: &tauri::AppHandle, id: &str) -> Result<(), ApiError> {
    let state = app.state::<AppState>();
    super::window_creation_allowed(&state)?;
    let Some(current) = document(app, id) else {
        state
            .reload_documents
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.into(), None);
        return Ok(());
    };
    state
        .reload_documents
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(id.into(), Some(current.clone()));
    if current.close_requested.load(Ordering::Acquire) {
        return Err(ApiError::new(
            "APP_WINDOW_CLOSING",
            "应用窗口正在关闭，请稍后重试。",
        ));
    }
    state.engine_downloads.seal_project_and_drain(id).await?;
    *current.download.lock().unwrap_or_else(|p| p.into_inner()) = None;
    current.switching.store(true, Ordering::Release);
    let mut blank = current.blank.subscribe();
    current.blank.send_replace(false);
    let label = super::project_label(id);
    super::native_ui(app, move |app, state| {
        super::window_creation_allowed(state)?;
        let window = app
            .get_webview_window(&label)
            .ok_or_else(|| ApiError::new("APP_WINDOW_CLOSED", "应用窗口已关闭。"))?;
        window
            .eval("window.stop()")
            .and_then(|_| window.navigate(tauri::Url::parse("about:blank").unwrap()))
            .map_err(|e| ApiError::new("APP_DOCUMENT_STOP_FAILED", e.to_string()))
    })
    .await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if *blank.borrow_and_update() {
                return Ok::<(), ApiError>(());
            }
            blank
                .changed()
                .await
                .map_err(|_| ApiError::new("APP_DOCUMENT_REPLY_LOST", "应用页面关闭回执丢失。"))?;
        }
    })
    .await
    .map_err(|_| {
        ApiError::new(
            "APP_DOCUMENT_STOP_TIMEOUT",
            "尚未确认旧页面停止，请稍后重试。",
        )
    })??;
    current
        .lease
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    state.engine_downloads.forget_project(id)?;
    Ok(())
}
async fn ready(app: &tauri::AppHandle, id: &str) -> Result<(), ApiError> {
    let expected = app
        .state::<AppState>()
        .reload_documents
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .cloned()
        .flatten();
    let Some(current) = expected.or_else(|| document(app, id)) else {
        super::open_app_window(app.clone(), id.into()).await?;
        return Ok(());
    };
    let state = app.state::<AppState>();
    if current.close_requested.load(Ordering::Acquire)
        || app.get_webview_window(&super::project_label(id)).is_none()
    {
        return Err(ApiError::new(
            "APP_WINDOW_CLOSED",
            "应用窗口已关闭，未重新打开。",
        ));
    }
    let (launch, lease) = state.handle.launch_window(id).await?;
    let resolved = state.handle.store().project(id)?;
    resolved.validate_epoch(Some(&launch.document_epoch))?;
    let url = tauri::Url::parse(&launch.launch_url)
        .map_err(|_| ApiError::new("INVALID_LAUNCH_URL", "应用入口无效。"))?;
    current.bind_download(super::browser_download::handler(
        app.clone(),
        state.handle.clone(),
        id.into(),
        launch.document_epoch,
        url.clone(),
        state.engine_downloads.clone(),
    ));
    *current.lease.lock().unwrap_or_else(|p| p.into_inner()) = lease;
    let label = super::project_label(id);
    *current.titles.lock().unwrap_or_else(|p| p.into_inner()) = title_names(&resolved.manifest);
    let title = super::project_title(&current.localized_name());
    let activated = current.clone();
    super::native_ui(app, move |app, state| {
        super::window_creation_allowed(state)?;
        if activated.close_requested.load(Ordering::Acquire) {
            return Err(ApiError::new("APP_WINDOW_CLOSING", "应用窗口正在关闭。"));
        }
        let receipt = state
            .window_lifetimes
            .receipt(&label)
            .ok_or_else(|| ApiError::new("APP_WINDOW_RECEIPT_MISSING", "应用窗口记录已失效。"))?;
        state
            .window_lifetimes
            .acknowledge_restart(&receipt)
            .map_err(|issue| ApiError::new(issue.code, "应用窗口代次已变化。"))?;
        let window = app
            .get_webview_window(&label)
            .ok_or_else(|| ApiError::new("APP_WINDOW_CLOSED", "应用窗口已关闭。"))?;
        window
            .set_title(&title)
            .and_then(|_| window.navigate(url))
            .and_then(|_| window.show())
            .map_err(|e| ApiError::new("APP_WINDOW_RELOAD_FAILED", e.to_string()))
    })
    .await?;
    current.switching.store(false, Ordering::Release);
    Ok(())
}
async fn show_failure(app: &tauri::AppHandle, id: &str, notice: &Value) -> Result<(), ApiError> {
    if document(app, id).is_none() {
        let state = app.state::<AppState>();
        super::window_creation_allowed(&state)?;
        let admission = state
            .window_lifetimes
            .admit_project("app", id)
            .map_err(|issue| ApiError::new(issue.code, "应用窗口正在关闭。"))?;
        let _operation = admission
            .operation()
            .await
            .map_err(|issue| ApiError::new(issue.code, "应用窗口正在关闭。"))?;
        let info = state.handle.store().info(id).await?;
        let resolved = state.handle.store().project(id)?;
        let id = id.to_owned();
        super::native_ui(app, move |app, state| {
            admission
                .check()
                .map_err(|issue| ApiError::new(issue.code, "应用窗口正在关闭。"))?;
            super::build_project_window(
                app,
                state,
                super::ProjectWindowInput {
                    project_id: id,
                    title_names: title_names(&resolved.manifest),
                    launch_url: tauri::Url::parse("about:blank").unwrap(),
                    window_config: info
                        .manifest
                        .and_then(|manifest| manifest.window)
                        .unwrap_or_default(),
                    document_epoch: resolved.document_epoch,
                    native_admission: admission,
                    open_lease: None,
                },
            )
        })
        .await?;
        if let Some(current) = document(app, &notice["appId"].as_str().unwrap_or_default()) {
            let mut blank = current.blank.subscribe();
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if *blank.borrow_and_update() {
                        break;
                    }
                    if blank.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await
            .map_err(|_| {
                ApiError::new(
                    "APP_FAILURE_DISPLAY_TIMEOUT",
                    "尚未收到失败页面的加载回执。",
                )
            })?;
        }
    }
    if let Some(current) = document(app, id) {
        current.switching.store(false, Ordering::Release);
        let text = format!(
            "{}\n\n{}\n\n{}",
            super::ui_locale::text("启动失败"),
            super::ui_locale::app_failure_message(
                notice["error"]["message"]
                    .as_str()
                    .or_else(|| notice["message"].as_str())
                    .unwrap_or("请查看应用日志，修好后重新加载。")
            ),
            notice["error"]["details"]["logPath"]
                .as_str()
                .or_else(|| notice["logPath"].as_str())
                .unwrap_or("")
        );
        let literal = serde_json::to_string(&text)
            .map_err(|e| ApiError::new("APP_FAILURE_DISPLAY_FAILED", e.to_string()))?;
        let label = super::project_label(id);
        super::native_ui(app, move |app, _| {
            if let Some(window) = app.get_webview_window(&label) {
                window.eval(&format!("document.body.textContent={literal};document.body.style.cssText='white-space:pre-wrap;padding:32px;font:16px system-ui;color:#333'"))
                    .map_err(|e|ApiError::new("APP_FAILURE_DISPLAY_FAILED",e.to_string()))?;
            }
            Ok(())
        }).await?;
    }
    Ok(())
}
