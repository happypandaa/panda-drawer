//! Public recommendations are fetched natively, outside the manager webview CSP.
use super::{require_manager, ApiError};
use serde_json::Value;
use std::time::Duration;

// The owner supplies the HTTPS URL of the published version-1 JSON catalog.
// An empty URL intentionally leaves recommendations hidden, without sample data.
const APP_CATALOG_URL: &str = "";
const MAX_CATALOG_BYTES: usize = 1024 * 1024;

#[tauri::command]
pub(super) async fn app_catalog(window: tauri::WebviewWindow) -> Result<Value, ApiError> {
    require_manager(&window)?;
    if APP_CATALOG_URL.is_empty() {
        return Ok(Value::Null);
    }
    let failed = || ApiError::new("APP_CATALOG_UNAVAILABLE", "暂时无法读取推荐应用。");
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|_| failed())?;
    let mut response = client
        .get(APP_CATALOG_URL)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| failed())?;
    if response.content_length().is_some_and(|size| size > MAX_CATALOG_BYTES as u64) {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
        if bytes.len() + chunk.len() > MAX_CATALOG_BYTES {
            return Err(failed());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| failed())
}
