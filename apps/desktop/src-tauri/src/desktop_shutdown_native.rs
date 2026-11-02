use super::{
    business_window_lifetime::Receipt,
    desktop_shutdown::{Backend, CoreState, Issue, Report, Status},
    ApiError, AppState,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::Ordering, Arc, Mutex},
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
type Work<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
struct Binding {
    receipt: Arc<Receipt>,
    window: Option<tauri::WebviewWindow>,
}
struct Prepared {
    windows: Vec<Binding>,
    sessions: super::node_windows::ShutdownSessions,
}
struct NativeBackend {
    app: tauri::AppHandle,
    prepared: Arc<Mutex<Option<Prepared>>>,
}
impl Backend for NativeBackend {
    fn start_core(&self) {
        self.app.state::<AppState>().runtime.begin_quiesce();
    }
    fn core_state(&self) -> CoreState {
        super::desktop_partial_startup::core_state(
            self.app.state::<AppState>().runtime.shutdown_status(),
        )
    }
    fn confirm_core(&self) -> Work<Result<(), Issue>> {
        super::desktop_partial_startup::confirm_core(self.app.state::<AppState>().runtime.clone())
    }
    fn windows(&self) -> Work<Result<(), Issue>> {
        let app = self.app.clone();
        let prepared = self.prepared.clone();
        Box::pin(async move {
            let state = app.state::<AppState>();
            if prepared.lock().unwrap_or_else(|p| p.into_inner()).is_none() {
                // Pending create/restart calls hold their original native lease.
                // After these locks, their shutdown rechecks prohibit late UI.
                let _projects = state.project_windows.lock().await;
                let _nodes = state.node_windows.operations.lock().await;
                let sessions = state.node_windows.freeze_for_shutdown();
                let actual = app.webview_windows();
                for label in actual.keys().filter(|label| label.as_str() != "manager") {
                    if !label.starts_with("project-") && !label.starts_with("node-app-") {
                        return Err(Issue::new("DESKTOP_UNOWNED_WINDOW", false, false));
                    }
                    if state.window_lifetimes.receipt(label).is_none() {
                        return Err(Issue::new("DESKTOP_WINDOW_RECEIPT_MISSING", false, false));
                    }
                }
                let windows = state
                    .window_lifetimes
                    .active()
                    .into_iter()
                    .map(|receipt| Binding {
                        window: actual.get(&receipt.label).cloned(),
                        receipt,
                    })
                    .collect();
                *prepared.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(Prepared { windows, sessions });
            }
            let bindings = {
                let prepared = prepared.lock().unwrap_or_else(|p| p.into_inner());
                prepared
                    .as_ref()
                    .unwrap()
                    .windows
                    .iter()
                    .map(|b| (b.receipt.clone(), b.window.clone()))
                    .collect::<Vec<_>>()
            };
            // Keep the real callbacks alive until every accepted engine write
            // delivers Finished. Waiting on all I/O here would wait on our own
            // aggregate scopes; completed saves keep their separate IoLeases.
            state.engine_downloads.wait_for_finished().await;
            let mut issue = None;
            // Request every real native destroy before awaiting any receipt.
            for (receipt, window) in &bindings {
                if receipt.destroyed() {
                    continue;
                }
                if receipt.creation_failed() {
                    issue = Some(Issue::new(
                        "DESKTOP_WINDOW_CREATION_UNCONFIRMED",
                        false,
                        false,
                    ));
                }
                if let Some(window) = window {
                    if receipt.label.starts_with("node-app-") {
                        let _ = window.navigate(tauri::Url::parse("about:blank").unwrap());
                    }
                    if super::project_webview_profile::destroy_for_shutdown(
                        &app,
                        window.clone(),
                        receipt.generation,
                    )
                    .await
                    .is_err()
                        && !receipt.destroyed()
                    {
                        eprintln!(
                            "Native business window destroy rejected: generation={}",
                            receipt.generation
                        );
                        issue = Some(Issue::new("DESKTOP_WINDOW_DESTROY_REJECTED", false, true));
                    }
                }
            }
            if let Some(issue) = issue {
                return Err(issue);
            }
            for (receipt, _) in bindings {
                receipt.wait().await;
            }
            super::project_webview_profile::drain_shutdown(&app)
                .await
                .map_err(|issue| Issue::new(&issue.code, false, true))?;
            Ok(())
        })
    }
    fn nodes(&self) -> Work<Result<(), Issue>> {
        let app = self.app.clone();
        Box::pin(async move {
            let state = app.state::<AppState>();
            state.node_monitor.request_stop();
            // Broadcast/poll all Node/proxy stops alongside the actual monitor
            // join. A monitor waiting on Node locks cannot prevent stop polling.
            let (result, monitor) =
                tokio::join!(state.node_apps.stop_all(), state.node_monitor.join());
            monitor?;
            let remaining = state.node_apps.owned_process_ids().await;
            if remaining.is_empty() {
                // An unclean business shutdown may retain active.json after
                // all process/proxy resources are positively closed.
                result.map_err(|e| Issue::new(&e.code, true, false))
            } else {
                let retryable = result.as_ref().err().is_none_or(node_stop_retryable);
                Err(Issue::new(
                    "DESKTOP_NODE_RESOURCES_UNCONFIRMED",
                    false,
                    retryable,
                ))
            }
        })
    }
    fn sessions(&self) -> Work<Result<(), Issue>> {
        let app = self.app.clone();
        let prepared = self.prepared.clone();
        Box::pin(async move {
            let state = app.state::<AppState>();
            let _nodes = state.node_windows.operations.lock().await;
            let prepared = prepared.lock().unwrap_or_else(|p| p.into_inner());
            let prepared = prepared
                .as_ref()
                .ok_or_else(|| Issue::new("DESKTOP_WINDOW_SNAPSHOT_MISSING", false, false))?;
            state
                .node_windows
                .finish_shutdown(&prepared.sessions, &state.window_lifetimes)
        })
    }
    fn changed(&self, report: &Report) {
        if self
            .app
            .state::<AppState>()
            .local_data_cleanup
            .shutdown_changed(report)
        {
            return;
        }
        show_blocked(&self.app, report);
    }
    fn ready(&self) {
        if self
            .app
            .state::<AppState>()
            .local_data_cleanup
            .drained(&self.app)
        {
            return;
        }
        // Normal Quit only. The diagnostic manager remains alive until Tauri's
        // final native exit; no installer permit or all-WebKit Complete exists.
        eprintln!("Desktop ReadyForNativeExit: business windows/core closed; manager closes in final native exit; no installer permit");
        self.app
            .state::<AppState>()
            .exit_ready
            .store(true, Ordering::Release);
        self.app.exit(0);
    }
}
pub(super) fn request(app: &tauri::AppHandle) {
    if app
        .try_state::<AppState>()
        .is_some_and(|state| state.local_data_cleanup.intercept_quit(app))
    {
        return;
    }
    request_for_cleanup(app);
}
pub(super) fn request_for_cleanup(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        request_partial_startup(app);
        return;
    };
    state.shutting_down.store(true, Ordering::Release);
    state.engine_downloads.close();
    state.window_lifetimes.seal();
    let lifetime = state.handle.io_lifetime();
    lifetime.close();
    let backend = Arc::new(NativeBackend {
        app: app.clone(),
        prepared: Arc::new(Mutex::new(None)),
    });
    state.desktop_shutdown.request(
        backend,
        Some(tauri::async_runtime::handle().inner().clone()),
        || {
            let windows = lifetime
                .admit_cleanup()
                .map_err(|_| Issue::new("DESKTOP_WINDOW_SCOPE_UNAVAILABLE", false, false))?;
            let nodes = lifetime
                .admit_cleanup()
                .map_err(|_| Issue::new("DESKTOP_NODE_SCOPE_UNAVAILABLE", false, false))?;
            // Synchronous preferences I/O is covered by the window scope before
            // core seal. Later Destroyed callbacks only forget in-memory bindings.
            state.window_sizes.flush();
            Ok((Box::new(windows), Box::new(nodes)))
        },
    );
}
/// Forced OS exit has no prevent API; do not block the main thread waiting for
/// Destroyed events that require this same loop. Start/reuse owned cleanup only.
pub(super) fn forced_exit(app: &tauri::AppHandle) {
    if app.try_state::<AppState>().is_some()
        || app
            .try_state::<super::StartupFailure>()
            .is_some_and(|failure| failure.runtime.is_some())
    {
        request(app);
    }
}

/// Trial reader/ownership failures are retained by the actual resource owner.
/// A missing proxy issue array cannot turn those known sticky errors retryable.
fn node_stop_retryable(error: &ApiError) -> bool {
    if matches!(
        error.code.as_str(),
        "TRIAL_READER_JOIN_FAILED"
            | "TRIAL_PROCESS_OWNERSHIP_LOST"
            | "TRIAL_OWNERSHIP_RELEASED"
            | "TRIAL_PLATFORM_UNSUPPORTED"
    ) {
        return false;
    }
    error.details["shutdownIssues"]
        .as_array()
        .is_none_or(|issues| issues.iter().all(|i| i["retryable"] == true))
}

fn show_blocked(app: &tauri::AppHandle, report: &Report) {
    if report.status == Status::Blocked {
        eprintln!(
            "Desktop normal quit blocked: attempt={}, phase={}, issueCodes={:?}",
            report.attempt,
            report.phase,
            report.issues.iter().map(|i| &i.code).collect::<Vec<_>>()
        );
        super::show_manager(app);
        let dialog = app.dialog().message(crate::ui_locale::text("退出尚未完成，部分窗口、后台服务或文件工作仍待确认。请等待相关操作结束后再次通过菜单退出。平台没有强制释放现场。"))
                .title(crate::ui_locale::text("正在保护现有工作"));
        if let Some(manager) = app.get_webview_window("manager") {
            // On macOS rfd uses an NSAlert sheet only with an explicit parent;
            // an unparented message takes the separate CFUserNotification path.
            dialog.parent(&manager).buttons(tauri_plugin_dialog::MessageDialogButtons::OkCustom(crate::ui_locale::text("确定").into())).show(|_| {});
        } else {
            eprintln!("Desktop quit-blocked dialog has no manager parent; fallback presentation is unconfirmed");
            dialog.buttons(tauri_plugin_dialog::MessageDialogButtons::OkCustom(crate::ui_locale::text("确定").into())).show(|_| {});
        }
    }
}
fn request_partial_startup(app: &tauri::AppHandle) {
    let Some(failure) = app.try_state::<super::StartupFailure>() else {
        app.exit(0);
        return;
    };
    let Some(runtime) = failure.runtime.clone() else {
        app.exit(0);
        return;
    };
    let changed_app = app.clone();
    let ready_app = app.clone();
    let backend = super::desktop_partial_startup::CoreOnlyBackend::new(
        runtime,
        move |report| show_blocked(&changed_app, report),
        move || {
            eprintln!("Desktop ReadyForNativeExit: partial-startup core closed; manager closes in final native exit; no installer permit");
            ready_app
                .state::<super::StartupFailure>()
                .exit_ready
                .store(true, Ordering::Release);
            ready_app.exit(0);
        },
    );
    failure.shutdown.request(
        Arc::new(backend),
        Some(tauri::async_runtime::handle().inner().clone()),
        || Ok((Box::new(()), Box::new(()))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_reader_and_ownership_failures_stay_sticky_without_proxy_details() {
        for code in [
            "TRIAL_READER_JOIN_FAILED",
            "TRIAL_PROCESS_OWNERSHIP_LOST",
            "TRIAL_OWNERSHIP_RELEASED",
            "TRIAL_PLATFORM_UNSUPPORTED",
        ] {
            assert!(!node_stop_retryable(&ApiError::new(
                code,
                "artificial failure"
            )));
        }
        assert!(node_stop_retryable(&ApiError::new(
            "TRIAL_READER_CLEANUP_PENDING",
            "artificial pending"
        )));
        assert!(!node_stop_retryable(
            &ApiError::new(
                "NODE_APP_PROXY_SHUTDOWN_BLOCKED",
                "artificial proxy failure"
            )
            .detail("shutdownIssues", serde_json::json!([{"retryable":false}]))
        ));
        assert!(node_stop_retryable(
            &ApiError::new(
                "NODE_APP_PROXY_SHUTDOWN_PENDING",
                "artificial proxy pending"
            )
            .detail("shutdownIssues", serde_json::json!([{"retryable":true}]))
        ));
    }
}
