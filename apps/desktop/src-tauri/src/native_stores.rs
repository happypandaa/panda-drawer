//! WebKit deletion is complete only after its native callback and a second
//! read. All Objective-C work is dispatched without blocking Cocoa's run loop.
use platform_core::ApiError;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
const TIMEOUT: Duration = Duration::from_secs(30);

fn error(code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(code, message)
}

pub(crate) async fn identifiers(app: &tauri::AppHandle) -> Result<Vec<[u8; 16]>, ApiError> {
    #[cfg(target_os = "macos")]
    {
        let mut ids = tokio::time::timeout(TIMEOUT, app.fetch_data_store_identifiers())
            .await
            .map_err(|_| error("WEBVIEW_STORE_TIMEOUT", "WebKit 存储清单尚未确认。"))?
            .map_err(|cause| error("WEBVIEW_STORE_QUERY_FAILED", cause.to_string()))?;
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(error(
            "WEBVIEW_STORE_UNAVAILABLE",
            "当前平台不支持 WebKit 存储清理。",
        ))
    }
}

pub(crate) async fn purge_identifier(app: &tauri::AppHandle, id: [u8; 16]) -> Result<(), ApiError> {
    if id == [0; 16] {
        return Err(error("WEBVIEW_STORE_ID_INVALID", "WebKit 存储标识无效。"));
    }
    #[cfg(target_os = "macos")]
    {
        if !identifiers(app).await?.contains(&id) {
            return Ok(());
        }
        // WebKit can retain session cookies in the in-process store even after
        // remove_data_store reports success. Clear its live data first, then
        // remove the persistent store itself. Never instantiate an absent ID.
        clear_identifier_and_verify(app, id).await?;
        tokio::time::timeout(TIMEOUT, app.remove_data_store(id))
            .await
            .map_err(|_| {
                error(
                    "WEBVIEW_STORE_TIMEOUT",
                    "WebKit 尚未确认存储已删除，请保留删除记录。",
                )
            })?
            .map_err(|cause| error("WEBVIEW_STORE_DELETE_FAILED", cause.to_string()))?;
        if identifiers(app).await?.contains(&id) {
            return Err(error(
                "WEBVIEW_STORE_REMAINS",
                "WebKit 存储仍在，删除尚未完成。",
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(error(
            "WEBVIEW_STORE_UNAVAILABLE",
            "当前平台不支持 WebKit 存储清理。",
        ))
    }
}

/// Full cleanup can clear global manager preferences while keeping its status
/// window alive. This never removes the store or creates an absent identifier.
pub(crate) async fn clear_identifier_and_verify(
    app: &tauri::AppHandle,
    id: [u8; 16],
) -> Result<(), ApiError> {
    if id == [0; 16] {
        return Err(error("WEBVIEW_STORE_ID_INVALID", "WebKit 存储标识无效。"));
    }
    #[cfg(target_os = "macos")]
    {
        if !identifiers(app).await?.contains(&id) {
            return Ok(());
        }
        macos::clear_store(app, Some(id)).await?;
        if macos::record_count(app, Some(id)).await? != 0 {
            return Err(error(
                "WEBVIEW_STORE_REMAINS",
                "网页存储仍有内容，清理尚未完成。",
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(error(
            "WEBVIEW_STORE_UNAVAILABLE",
            "当前平台不支持 WebKit 存储清理。",
        ))
    }
}

/// Clears the old shared store itself, without constructing a persistent
/// WebView that could write new content while verification is in progress.
pub(crate) async fn clear_default_and_verify(app: &tauri::AppHandle) -> Result<(), ApiError> {
    #[cfg(target_os = "macos")]
    {
        macos::clear_store(app, None).await?;
        if macos::record_count(app, None).await? != 0 {
            return Err(error(
                "WEBVIEW_DEFAULT_STORE_REMAINS",
                "共享网页存储仍有内容，暂时不能打开项目。",
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(error(
            "WEBVIEW_STORE_UNAVAILABLE",
            "当前平台不支持 WebKit 存储清理。",
        ))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSArray, NSDate, NSUUID};
    use objc2_web_kit::{WKWebsiteDataRecord, WKWebsiteDataStore};
    use std::{ptr::NonNull, sync::Mutex};
    use tokio::sync::oneshot;

    async fn receipt<T>(receive: oneshot::Receiver<Result<T, ApiError>>) -> Result<T, ApiError> {
        tokio::time::timeout(TIMEOUT, receive)
            .await
            .map_err(|_| error("WEBVIEW_STORE_TIMEOUT", "WebKit 存储操作尚未收到系统回执。"))?
            .map_err(|_| error("WEBVIEW_STORE_REPLY_LOST", "WebKit 存储操作回执丢失。"))?
    }

    pub(super) async fn clear_store(
        app: &tauri::AppHandle,
        id: Option<[u8; 16]>,
    ) -> Result<(), ApiError> {
        let (send, receive) = oneshot::channel();
        app.run_on_main_thread(move || {
            let Some(main) = MainThreadMarker::new() else {
                let _ = send.send(Err(error(
                    "WEBVIEW_STORE_WRONG_THREAD",
                    "WebKit 存储操作未进入主线程。",
                )));
                return;
            };
            let send = Mutex::new(Some(send));
            let callback = RcBlock::new(move || {
                if let Some(send) = send.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    let _ = send.send(Ok(()));
                }
            });
            // WKWebsiteDataStore and its Foundation objects never cross the
            // Send future boundary; WebKit retains the asynchronous block.
            unsafe {
                let store = match id {
                    Some(id) => {
                        WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(id), main)
                    }
                    None => WKWebsiteDataStore::defaultDataStore(main),
                };
                let types = WKWebsiteDataStore::allWebsiteDataTypes(main);
                store.removeDataOfTypes_modifiedSince_completionHandler(
                    &types,
                    &NSDate::distantPast(),
                    &callback,
                );
            }
        })
        .map_err(|cause| error("WEBVIEW_STORE_DISPATCH_FAILED", cause.to_string()))?;
        receipt(receive).await
    }

    pub(super) async fn record_count(
        app: &tauri::AppHandle,
        id: Option<[u8; 16]>,
    ) -> Result<usize, ApiError> {
        let (send, receive) = oneshot::channel();
        app.run_on_main_thread(move || {
            let Some(main) = MainThreadMarker::new() else {
                let _ = send.send(Err(error(
                    "WEBVIEW_STORE_WRONG_THREAD",
                    "WebKit 存储核验未进入主线程。",
                )));
                return;
            };
            let send = Mutex::new(Some(send));
            let callback = RcBlock::new(move |records: NonNull<NSArray<WKWebsiteDataRecord>>| {
                let count = unsafe { records.as_ref() }.count();
                if let Some(send) = send.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    let _ = send.send(Ok(count));
                }
            });
            unsafe {
                let store = match id {
                    Some(id) => {
                        WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(id), main)
                    }
                    None => WKWebsiteDataStore::defaultDataStore(main),
                };
                let types = WKWebsiteDataStore::allWebsiteDataTypes(main);
                store.fetchDataRecordsOfTypes_completionHandler(&types, &callback);
            }
        })
        .map_err(|cause| error("WEBVIEW_STORE_DISPATCH_FAILED", cause.to_string()))?;
        receipt(receive).await
    }
}
