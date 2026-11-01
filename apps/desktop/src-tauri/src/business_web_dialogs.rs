//! Business-page JavaScript dialogs, adapted to the native browser engine.
use platform_core::ApiError;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
mod macos;

pub(super) fn install(window: &tauri::WebviewWindow) -> Result<(), ApiError> {
    let result = Arc::new(Mutex::new(None));
    let reply = result.clone();
    window
        .with_webview(move |webview| {
            #[cfg(target_os = "macos")]
            let configured = unsafe { macos::install(webview.inner()) };
            #[cfg(windows)]
            let configured = unsafe {
                webview
                    .controller()
                    .CoreWebView2()
                    .and_then(|core| core.Settings())
                    .and_then(|settings| settings.SetAreDefaultScriptDialogsEnabled(true))
                    .map_err(|_| {
                        ApiError::new("WEB_DIALOGS_UNAVAILABLE", "无法启用应用的系统对话框。")
                    })
            };
            #[cfg(not(any(target_os = "macos", windows)))]
            let configured = {
                let _ = webview;
                Ok(())
            };
            *reply.lock().unwrap_or_else(|p| p.into_inner()) = Some(configured);
        })
        .map_err(|_| ApiError::new("WEB_DIALOGS_UNAVAILABLE", "无法设置应用的系统对话框。"))?;
    let configured = result
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
        .ok_or_else(|| {
            ApiError::new("WEB_DIALOGS_UNAVAILABLE", "没有收到应用对话框的设置回执。")
        })?;
    configured
}
