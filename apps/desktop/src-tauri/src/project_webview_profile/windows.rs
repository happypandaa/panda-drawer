mod binding;
#[cfg(test)]
mod fixture;
mod owner;
mod purge;
pub(super) use binding::{before_destroy, wait_closed, window_destroyed, Binding};
use platform_core::ApiError;
use tauri::AppHandle;

pub(super) async fn confirm_purge_closed(
    app: &AppHandle,
    ledger: &platform_core::webview_storage::StoreLedger,
    kind: platform_core::project_cleanup::ProjectKind,
    id: &str,
) -> Result<(), ApiError> {
    purge::confirm_closed(app, ledger, kind, id).await
}

pub(super) async fn drain_shutdown(app: &AppHandle) -> Result<(), ApiError> {
    let mut workers = Vec::new();
    for owner in owner::all(app) {
        if owner.closed_and_retired() {
            continue;
        }
        let app = app.clone();
        workers.push(tauri::async_runtime::spawn(async move {
            binding::wait_closed(&app, &owner.label, owner.generation).await
        }));
    }
    let mut first = None;
    for worker in workers {
        match worker.await {
            Ok(Ok(())) => (),
            Ok(Err(error)) => {
                if first.is_none() {
                    first = Some(error);
                }
            }
            Err(_) => {
                if first.is_none() {
                    first = Some(ApiError::new(
                        "WEBVIEW_PROFILE_EXIT_UNCONFIRMED",
                        "项目浏览器退出工作尚未确认。",
                    ));
                }
            }
        }
    }
    first.map_or(Ok(()), Err)
}
