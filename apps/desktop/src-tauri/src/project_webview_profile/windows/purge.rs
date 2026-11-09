//! Trusted cleanup engine. A lost in-memory owner is not an exit receipt; a
//! blank controller on the original UDF obtains a real family-exit observation.
use super::{binding, owner, Binding};
use platform_core::{project_cleanup::ProjectKind, webview_storage::StoreLedger, ApiError};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

static NEXT_CLEANUP_GENERATION: AtomicU64 = AtomicU64::new(1 << 63);

fn unknown(message: &str) -> ApiError {
    ApiError::new("WEBVIEW_PROFILE_CLEANUP_UNCONFIRMED", message).outcome("unknown")
}
fn label(_kind: ProjectKind, id: &str) -> String {
    format!("app-{id}")
}

pub(super) async fn confirm_closed(
    app: &AppHandle,
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
) -> Result<(), ApiError> {
    // This receipt is published only by a caller that already observed native
    // closure. Its root identity remains checked by the ledger on every retry.
    if ledger.confirmed_cleanup_pending(kind, id)? {
        return Ok(());
    }
    let label = label(kind, id);
    super::super::require_profile_label(kind, id, &label)?;
    if app.get_webview_window(&label).is_some() {
        return Err(unknown("项目原生窗口仍在，未清理浏览器存储。"));
    }
    if let Some(current) = owner::lookup(app, &label) {
        binding::wait_closed(app, &label, current.generation).await?;
        let profile = std::sync::Arc::new(ledger.bind_profile(kind, id)?);
        let actual = platform_native::windows::files::open_no_reparse(profile.directory())?;
        if !current.identity.same_object(&actual.identity) {
            return Err(unknown("已关闭窗口的原浏览器目录身份已经变化。"));
        }
        if profile.recover_browser_session()?.is_none() {
            return Ok(());
        }
        // A later failed/abandoned reservation may have published a session
        // after this retired generation. Obtain a fresh native exit receipt.
    }

    // No current owner can acknowledge a previous host's browser. Open only
    // an invisible about:blank controller on the already bound original UDF.
    // It attaches to that UDF's existing family, or creates a new empty family;
    // either way its real Env5 event proves the original UDF is now released.
    // No project URL, page script, backend or arbitrary profile is selected.
    let (send, receive) = tokio::sync::oneshot::channel();
    let dispatch = app.clone();
    let native_app = app.clone();
    let ledger = ledger.clone();
    let id = id.to_owned();
    let native_label = label.clone();
    let generation = NEXT_CLEANUP_GENERATION.fetch_add(1, Ordering::Relaxed);
    dispatch
        .run_on_main_thread(move || {
            let result = (|| {
                if native_app.get_webview_window(&native_label).is_some() {
                    return Err(unknown("原浏览器窗口重新出现，未启动清理窗口。"));
                }
                let binding =
                    Binding::prepare_for_cleanup(&native_app, &ledger, kind, &id, &native_label)?;
                binding.reserve(&native_app, generation)?;
                let window = match WebviewWindowBuilder::new(
                    &native_app,
                    &native_label,
                    WebviewUrl::External(tauri::Url::parse("about:blank").unwrap()),
                )
                .data_directory(binding.directory().to_path_buf())
                .visible(false)
                .focused(false)
                .skip_taskbar(true)
                .on_navigation(|url| url.as_str() == "about:blank")
                .build()
                {
                    Ok(window) => window,
                    Err(cause) => {
                        binding.creation_failed();
                        return Err(unknown(&format!("原生浏览器清理窗口创建未确认：{cause}")));
                    }
                };
                let event_app = native_app.clone();
                let event_label = native_label.clone();
                window.on_window_event(move |event| {
                    if matches!(event, tauri::WindowEvent::Destroyed) {
                        binding::window_destroyed(&event_app, &event_label, generation);
                    }
                });
                if let Err(issue) = binding.bind_window(&window) {
                    let _ = window.destroy();
                    return Err(issue);
                }
                binding::before_destroy(&native_app, &window, generation)?;
                window
                    .destroy()
                    .map_err(|cause| unknown(&format!("原生浏览器清理窗口销毁未确认：{cause}")))?;
                // Binding and the local window HANDLE drop here. Registry,
                // observer, profile pins and session remain until real exit.
                Ok(())
            })();
            let _ = send.send(result);
        })
        .map_err(|_| unknown("原生浏览器清理创建派发失败。"))?;
    receive
        .await
        .map_err(|_| unknown("原生浏览器清理创建回执丢失。"))??;
    binding::wait_closed(app, &label, generation).await
}
