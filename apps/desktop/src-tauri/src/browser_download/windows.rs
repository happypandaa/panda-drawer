//! WebView2 downloads retain real engine tickets until its actual Finished event.
use platform_core::{io_lifetime::TrackedIo, ApiError, RuntimeHandle};
use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc, Mutex},
};
use tauri::{webview::DownloadEvent, AppHandle, Url, Webview};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
#[path = "windows/stage.rs"]
mod stage;
use stage::PendingDownload;
fn invalid(message: &str) -> ApiError {
    ApiError::new("DOWNLOAD_FAILED", message)
}
// Until Finished, WebView2 owns the destination and may still write it. Dropping
// the callback alone is not an engine-cancellation acknowledgement. Retain both
// evidence and the busy lease in that exceptional case, rather than deleting
// the engine's file and falsely reporting a drained service.
struct EngineDownload {
    resource: Option<ProjectDownload>,
    ticket: Option<super::super::engine_downloads::EngineTicket>,
}
type ProjectDownload = TrackedIo<TrackedIo<PendingDownload>>;
fn pending_stage(value: &ProjectDownload) -> &PendingDownload {
    value.resource().resource()
}
impl EngineDownload {
    fn finished(mut self) -> ProjectDownload {
        pending_stage(self.resource.as_ref().expect("owned download"))
            .engine_finished
            .store(true, Ordering::Release);
        self.ticket.take().expect("owned engine ticket").finished();
        self.resource.take().expect("owned download")
    }
}
impl Drop for EngineDownload {
    fn drop(&mut self) {
        if let Some(resource) = self.resource.take() {
            eprintln!("Native download callback ended without Finished; preserving its stage and blocking drain");
            std::mem::forget(resource);
        }
    }
}

fn take_finished(
    pending: &Mutex<HashMap<String, EngineDownload>>,
    url: &Url,
) -> Option<ProjectDownload> {
    pending
        .lock()
        .ok()?
        .remove(url.as_str())
        .map(EngineDownload::finished)
}

fn permitted_url(base: &Url, current: &Url, requested: &Url) -> bool {
    let same_origin = |url: &Url| {
        url.origin() == base.origin() && url.username().is_empty() && url.password().is_none()
    };
    if base.scheme() != "http" || base.host_str() != Some("127.0.0.1") || !same_origin(current) {
        return false;
    }
    if requested.scheme() == "blob" {
        return requested
            .as_str()
            .strip_prefix("blob:")
            .and_then(|inner| Url::parse(inner).ok())
            .is_some_and(|inner| {
                same_origin(&inner)
                    && !inner.path().is_empty()
                    && inner.path() != "/"
                    && inner.query().is_none()
                    && inner.fragment().is_none()
            });
    }
    same_origin(requested)
        && !requested.path().contains("%2f")
        && !requested.path().contains("%2F")
        && !requested.path().contains("%5c")
        && !requested.path().contains("%5C")
}

fn notify_error(app: &AppHandle, error: &ApiError) {
    app.dialog()
        .message(format!("下载未保存：{}", error.message))
        .title("保存下载")
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

pub fn handler(
    app: AppHandle,
    handle: RuntimeHandle,
    id: String,
    epoch: String,
    bound_origin: Url,
    engines: Arc<super::super::engine_downloads::EngineDownloads>,
) -> impl Fn(Webview, DownloadEvent<'_>) -> bool + Send + Sync + 'static {
    let pending = Arc::new(Mutex::new(HashMap::<String, EngineDownload>::new()));
    let window_admission = engines.bind_handler(&id);
    move |webview, event| match event {
        DownloadEvent::Requested { url, destination } => {
            let Ok(((io_lease, writer), project_lease, mut ticket)) =
                engines.admit_project(&id, || {
                    window_admission.check()?;
                    let lease = handle.io_lifetime().admit()?;
                    let writer =
                        handle.native_project_io(&id, &epoch, "native-webview-download")?;
                    Ok((lease, writer))
                })
            else {
                return false;
            };
            let permitted = webview
                .url()
                .ok()
                .is_some_and(|current| permitted_url(&bound_origin, &current, &url));
            if !permitted {
                let _ = writer.finish();
                return false;
            }
            let Ok(mut downloads) = pending.lock() else {
                let _ = writer.finish();
                return false;
            };
            let key = url.to_string();
            // Finished.path is unavailable on macOS; URL is our completion key.
            if downloads.contains_key(&key) {
                let _ = writer.finish();
                return false;
            }
            let suggested = destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("download.bin");
            match PendingDownload::create_owned(writer, suggested) {
                Ok(stage) => {
                    *destination = stage.file.clone();
                    ticket.started();
                    stage.engine_accepted.store(true, Ordering::Release);
                    downloads.insert(
                        key,
                        EngineDownload {
                            resource: Some(project_lease.hold(io_lease.hold(stage))),
                            ticket: Some(ticket),
                        },
                    );
                    true
                }
                Err(error) => {
                    notify_error(&app, &error);
                    false
                }
            }
        }
        DownloadEvent::Finished { url, success, .. } => {
            let Some(stage) = take_finished(&pending, &url) else {
                return false;
            };
            if !engines.project_open(&id) {
                return false;
            }
            if !success {
                notify_error(&app, &invalid("Browser download did not complete"));
                return false;
            }
            if let Err(error) = pending_stage(&stage).open_file() {
                notify_error(&app, &error);
                return false;
            }
            let suggested_name = pending_stage(&stage).suggested_name.clone();
            let Ok(deferred) = engines.defer_project(&id, stage) else {
                return false;
            };
            let app = app.clone();
            let handle = handle.clone();
            app.dialog()
                .file()
                .set_parent(&webview.window())
                .set_title("保存项目下载")
                .set_file_name(&suggested_name)
                .save_file(move |selected| {
                    let Some(selected) = selected else {
                        return;
                    };
                    // Claim and stop share admission's lock. An unselected stage
                    // is revoked on stop; a successful claim keeps the original
                    // lease through copying and private-stage destruction.
                    let Ok(stage) = deferred.claim() else {
                        return;
                    };
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = selected
                            .into_path()
                            .map_err(|error| invalid(&error.to_string()))
                            .and_then(|destination| {
                                let pending = pending_stage(&stage);
                                let writer = pending.writer.as_ref().ok_or_else(|| {
                                    invalid("Project writer admission is missing")
                                })?;
                                handle.publish_native_file(
                                    writer,
                                    pending.open_file()?,
                                    &destination,
                                )
                            });
                        if let Err(error) = result {
                            notify_error(&app, &error);
                        }
                        drop(stage);
                    });
                });
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "windows/fixture.rs"]
mod fixture;
#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
