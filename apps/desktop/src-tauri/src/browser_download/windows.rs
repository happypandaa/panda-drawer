//! Browser downloads stay unadmitted until Windows writer ownership and external
//! publication are implemented. A Finished event cannot supply that ownership.
use platform_core::{ApiError, RuntimeHandle};
use std::sync::Arc;
use tauri::{webview::DownloadEvent, AppHandle, Webview};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

#[path = "windows/rejection.rs"]
mod rejection;

fn notify_error(app: &AppHandle, error: &ApiError) {
    app.dialog()
        .message(format!("下载未保存：{}", error.message))
        .title("保存下载")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

pub fn handler(
    app: AppHandle,
    _: RuntimeHandle,
    _: String,
    _: String,
    _: Arc<super::super::engine_downloads::EngineDownloads>,
) -> impl Fn(Webview, DownloadEvent<'_>) -> bool + Send + Sync + 'static {
    // Do not bind an engine handler, admit a writer, or prepare any destination.
    move |_, event| rejection::reject(event, |error| notify_error(&app, error))
}
