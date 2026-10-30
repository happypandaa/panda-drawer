use super::*;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

#[path = "windows/profile.rs"]
mod profile;
#[path = "windows/runtime.rs"]
mod runtime;

pub(super) fn runtime_available(app: &AppHandle) -> bool {
    if let Err(error) = runtime::checked_version(tauri::webview_version()) {
        eprintln!("{error}");
        let owner = app.clone();
        // This schedules a native dialog and returns to the event loop. No
        // WebView, workspace or service has been created; ordinary exit cannot
        // be intercepted by a running/partial-startup shutdown owner.
        app.dialog()
            .message(error.message)
            .title("熊猫小抽屉 · WebView2 运行时不可用")
            .kind(MessageDialogKind::Error)
            .show(move |_| owner.exit(1));
        return false;
    }
    true
}

pub(super) fn initialize(app: &AppHandle, data_root: &std::path::Path) -> Result<(), ApiError> {
    if app.try_state::<profile::ManagerProfile>().is_some() {
        return Err(ApiError::new(
            "MANAGER_PROFILE_ALREADY_INITIALIZED",
            "管理器浏览器目录已初始化。",
        ));
    }
    let profile = profile::ManagerProfile::initialize(data_root)?;
    if !app.manage(profile) {
        return Err(ApiError::new(
            "MANAGER_PROFILE_ALREADY_INITIALIZED",
            "管理器浏览器目录已初始化。",
        ));
    }
    Ok(())
}

pub(super) fn verify_initialized(app: &AppHandle) -> Result<(), ApiError> {
    app.try_state::<profile::ManagerProfile>()
        .ok_or_else(profile::not_initialized)?
        .verify()
}

pub(super) fn manager_builder<'a>(
    app: &AppHandle,
    builder: ManagerBuilder<'a>,
) -> Result<ManagerBuilder<'a>, ApiError> {
    let profile = app
        .try_state::<profile::ManagerProfile>()
        .ok_or_else(profile::not_initialized)?;
    profile.verify()?;
    Ok(builder.data_directory(profile.directory().to_path_buf()))
}

pub(super) fn is_bundled_url(url: &Url) -> bool {
    // Tauri's default Windows custom-protocol mapping. No arbitrary HTTP
    // listener or non-default port is a privileged manager origin.
    url.scheme() == "http" && url.host_str() == Some("tauri.localhost") && url.port().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_origin_matches_the_windows_bundle_and_fixed_development_origin_only() {
        for (value, development) in [
            ("http://tauri.localhost/index.html", false),
            ("http://tauri.localhost:80/index.html", false),
            ("http://127.0.0.1:1420/", true),
        ] {
            assert!(super::super::is_manager_url(
                &Url::parse(value).unwrap(),
                development
            ));
        }
        for value in [
            "tauri://localhost/index.html",
            "https://tauri.localhost/",
            "http://tauri.localhost:1420/",
            "http://tauri.localhost.evil.test/",
            "http://user@tauri.localhost/",
            "http://tauri.localhost@evil.test/",
            "http://127.0.0.1:49152/p/prj_a/",
            "http://localhost:1420/",
            "http://127.0.0.1.evil.test:1420/",
            "https://example.com/",
        ] {
            assert!(
                !super::super::is_manager_url(&Url::parse(value).unwrap(), true),
                "{value}"
            );
        }
        assert!(!super::super::is_manager_url(
            &Url::parse("http://127.0.0.1:1420/").unwrap(),
            false
        ));
    }

    #[test]
    fn real_runtime_query_uses_the_installed_webview2_loader_without_a_webview() {
        let version = runtime::checked_version(tauri::webview_version()).unwrap();
        eprintln!("Installed WebView2 runtime: {version}");
    }
}
