//! Public recommendations are fetched natively, outside the manager webview CSP.
use super::{require_manager, ApiError};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

const MAX_CATALOG_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct DownloadSources {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    catalog: CatalogSources,
    routes: BTreeMap<String, RouteSources>,
}

#[derive(Deserialize)]
struct CatalogSources {
    sources: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RouteSources {
    catalog: String,
}

fn catalog_url(route: &str) -> Option<String> {
    // The same build-time registry is used by the release tooling. A future
    // Gitee or OSS catalog is registered here, never supplied by a webview.
    let configuration: DownloadSources = serde_json::from_str(include_str!(
        "../../../../distribution/download-sources.json"
    ))
    .ok()?;
    if configuration.schema_version != 1 {
        return None;
    }
    let source = &configuration.routes.get(route)?.catalog;
    let url = configuration.catalog.sources.get(source)?;
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(url.clone())
}

#[tauri::command]
pub(super) async fn app_catalog(
    window: tauri::WebviewWindow,
    route: Option<String>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let failed = || ApiError::new("APP_CATALOG_UNAVAILABLE", "暂时无法读取推荐应用。");
    let url = catalog_url(route.as_deref().unwrap_or("global")).ok_or_else(failed)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|_| failed())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| failed())?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_CATALOG_BYTES as u64)
    {
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

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_uses_only_registered_route_sources() {
        assert_eq!(
            super::catalog_url("global").as_deref(),
            Some(
                "https://raw.githubusercontent.com/happypandaa/panda-drawer/develop/catalog/apps.json"
            )
        );
        assert_eq!(
            super::catalog_url("cn").as_deref(),
            Some("https://gitee.com/mycolorfuldays/panda-drawer/raw/develop/catalog/apps.json")
        );
        assert!(super::catalog_url("https://example.invalid").is_none());
    }
}
