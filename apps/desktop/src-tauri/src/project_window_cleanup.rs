//! Project-local native cleanup. A destroy request, a missing registry entry or
//! a dropped callback is never a native-release receipt. These hooks also run
//! for core/CLI deletion, before the project write gate and disk purge.
use super::{ApiError, AppState};
use platform_core::project_cleanup::{DeletedProjectNotice, ProjectKind};
use std::sync::{atomic::Ordering, Arc};
use tauri::Manager;

fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn lifetime_error(issue: super::desktop_shutdown::Issue) -> ApiError {
    error(
        &issue.code,
        "项目窗口尚未完成清理，已保留原状态，请稍后重试。",
    )
}
fn label(notice: &DeletedProjectNotice) -> String {
    super::project_label(&notice.id)
}

/// This dispatcher deliberately permits trusted cleanup after ordinary window
/// admission is sealed. Its reply also acts as a main-thread queue barrier.
async fn on_main<T: Send + 'static>(
    app: &tauri::AppHandle,
    work: impl FnOnce(&tauri::AppHandle, &AppState) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let owner = app.clone();
    app.run_on_main_thread(move || {
        let state = owner.state::<AppState>();
        let _ = send.send(work(&owner, &state));
    })
    .map_err(|_| {
        error(
            "PROJECT_WINDOW_UI_DISPATCH_FAILED",
            "无法派发项目窗口清理，已保留原状态。",
        )
    })?;
    receive.await.map_err(|_| {
        error(
            "PROJECT_WINDOW_UI_REPLY_LOST",
            "项目窗口清理回执丢失，已保留原状态。",
        )
    })?
}

async fn wait_destroyed(
    receipt: Option<Arc<super::business_window_lifetime::Receipt>>,
) -> Result<(), ApiError> {
    if let Some(receipt) = receipt {
        tokio::time::timeout(std::time::Duration::from_secs(5), receipt.wait())
            .await
            .map_err(|_| {
                error(
                    "PROJECT_WINDOW_DESTROY_UNCONFIRMED",
                    "项目窗口尚未确认关闭，已停止删除，请稍后重试。",
                )
            })?;
    }
    Ok(())
}

pub(super) async fn prepare(
    app: &tauri::AppHandle,
    notice: &DeletedProjectNotice,
) -> Result<(), ApiError> {
    let state = app.state::<AppState>();
    // Match native creation/shutdown lock order. A queued closure owns its
    // creation guard until it executes; after this section no old generation
    // can register. Never retain either global guard while waiting for A.
    let project_operation = {
        let _projects = state.project_windows.lock().await;
        let operation = state
            .window_lifetimes
            .seal_project(notice.kind.as_str(), &notice.id)
            .map_err(lifetime_error)?;
        operation
    };
    // Drain queued reload/open work for this project only. Any old permit now
    // fails its post-wait check; no global creation guard spans this wait.
    let _operation = project_operation.lock_owned().await;
    state
        .window_sizes
        .forget_project(notice.kind.as_str(), &notice.id);
    if notice.kind == ProjectKind::App {
        // Only actual engine Finished and claimed-copy completion release this
        // barrier. An unanswered save dialog is revoked by the download owner.
        state
            .engine_downloads
            .seal_project_and_drain(&notice.id)
            .await?;
    }
    let receipt = state
        .window_lifetimes
        .project_receipt(notice.kind.as_str(), &notice.id)
        .map_err(lifetime_error)?;
    destroy_and_wait(app, label(notice), receipt).await
}

fn report_web_close_failure(app: &tauri::AppHandle, issue: &ApiError) {
    eprintln!("Project window close incomplete: {}", issue.code);
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .message(crate::ui_locale::text("应用关闭尚未完成。请等待文件操作结束，并在应用详情中核对或重试关闭；数据库释放前不会显示已关闭。"))
        .title(crate::ui_locale::text("项目窗口关闭状态"))
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCustom(crate::ui_locale::text("确定").into())).show(|_| {});
}

/// CloseRequested must be prevented synchronously. Work owns its cleanup I/O
/// lease independently of the native callback, and waits only for this project.
pub(super) fn request_web_close(
    app: &tauri::AppHandle,
    id: &str,
    receipt: Arc<super::business_window_lifetime::Receipt>,
    admission: super::business_window_lifetime::ProjectAdmission,
) {
    let state = app.state::<AppState>();
    if state.shutting_down.load(Ordering::Acquire)
        || admission.check_closing().is_err()
        || !state.window_lifetimes.is_current(&receipt)
        || !receipt.begin_close()
    {
        return;
    }
    let lease = match state.handle.io_lifetime().admit_cleanup() {
        Ok(lease) => lease,
        Err(issue) => {
            receipt.reset_close();
            if !state.shutting_down.load(Ordering::Acquire) && admission.check_closing().is_ok() {
                report_web_close_failure(app, &issue);
            }
            return;
        }
    };
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(lease.hold(Box::pin(async move {
        let state = app.state::<AppState>();
        let result = async {
            let _operation = admission
                .closing_operation()
                .await
                .map_err(lifetime_error)?;
            if !state.window_lifetimes.is_current(&receipt) || receipt.destroyed() {
                return Ok(());
            }
            // The engine owner imposes the 30-second bound and preserves an
            // unknown writer on timeout. Keep the view alive until it confirms
            // real Finished and any already-claimed copy has completed.
            state.engine_downloads.seal_project_and_drain(&id).await?;
            destroy_and_wait(&app, receipt.label.clone(), Some(receipt.clone())).await?;
            state.handle.close_web(&id).await?;
            if admission.check_closing().is_ok() {
                state.engine_downloads.forget_project(&id)?;
            }
            Ok::<(), ApiError>(())
        }
        .await;
        if let Err(issue) = result {
            receipt.reset_close();
            // Deletion or global shutdown may have taken ownership while we
            // waited. Its cleanup reports the result; stale callbacks stay quiet.
            if admission.check_closing().is_ok()
                && (state.window_lifetimes.is_current(&receipt) || receipt.destroyed())
                && !state.shutting_down.load(Ordering::Acquire)
            {
                report_web_close_failure(&app, &issue);
            }
        }
    })));
}

async fn destroy_and_wait(
    app: &tauri::AppHandle,
    window_label: String,
    receipt: Option<Arc<super::business_window_lifetime::Receipt>>,
) -> Result<(), ApiError> {
    let expected = receipt.clone();
    let generation = receipt.as_ref().map(|receipt| receipt.generation);
    let profile_label = window_label.clone();
    let destruction_label = window_label.clone();
    let window = on_main(app, move |app, state| {
        if expected
            .as_ref()
            .is_some_and(|receipt| !state.window_lifetimes.is_current(receipt))
        {
            return Err(error(
                "PROJECT_WINDOW_GENERATION_CHANGED",
                "原项目窗口已经更换，未关闭新的窗口。",
            ));
        }
        let window = app.get_webview_window(&destruction_label);
        if let Some(window) = &window {
            if expected.is_none() {
                return Err(error(
                    "PROJECT_WINDOW_RECEIPT_MISSING",
                    "项目窗口关闭记录缺失，已保留状态供核对。",
                ));
            }
            super::project_webview_profile::before_destroy(
                app,
                window,
                expected.as_ref().unwrap().generation,
            )?;
            window.destroy().map_err(|_| {
                error(
                    "PROJECT_WINDOW_DESTROY_FAILED",
                    "无法关闭项目窗口，已保留原状态。",
                )
            })?;
        }
        Ok(window)
    })
    .await?;
    wait_destroyed(receipt).await?;
    // Destroyed is signalled from inside its event callback. This later queue
    // job runs after that callback returns, drops our last captured handle, and
    // checks that no unaccounted native generation occupies this project label.
    on_main(app, move |app, _| {
        drop(window);
        if app.get_webview_window(&window_label).is_some() {
            return Err(error(
                "PROJECT_WINDOW_STILL_REGISTERED",
                "项目窗口仍被原生容器持有，已停止删除。",
            ));
        }
        Ok(())
    })
    .await?;
    super::project_webview_profile::wait_closed(app, &profile_label, generation).await
}

pub(super) async fn purge(
    app: &tauri::AppHandle,
    notice: &DeletedProjectNotice,
) -> Result<(), ApiError> {
    let state = app.state::<AppState>();
    state
        .window_lifetimes
        .forget_project(notice.kind.as_str(), &notice.id)
        .map_err(lifetime_error)
}

pub(super) async fn committed(
    app: &tauri::AppHandle,
    notice: &DeletedProjectNotice,
) -> Result<(), ApiError> {
    let state = app.state::<AppState>();
    if notice.kind == ProjectKind::App {
        state.engine_downloads.forget_project(&notice.id)?;
    }
    state
        .window_lifetimes
        .finish_project(notice.kind.as_str(), &notice.id)
        .map_err(lifetime_error)
}
