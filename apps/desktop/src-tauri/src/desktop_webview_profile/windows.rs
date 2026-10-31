use super::*;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

#[path = "windows/actual.rs"]
mod actual;

pub(super) fn manager_from_config<'a>(
    app: &'a AppHandle,
    config: &tauri::utils::config::WindowConfig,
) -> Result<ManagerBuilder<'a>, ApiError> {
    actual::builder_from_config(app, config)
}
#[path = "windows/exit.rs"]
mod exit;
#[path = "windows/profile.rs"]
mod profile;
#[path = "windows/purge.rs"]
mod purge;
#[path = "windows/runtime.rs"]
mod runtime;

pub(super) async fn close_and_clear_manager(app: &AppHandle) -> Result<(), ApiError> {
    exit::close(app).await?;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profile = app
            .try_state::<profile::ManagerProfile>()
            .ok_or_else(profile::not_initialized)?;
        purge::clear_contents(profile.directory(), profile.original_identity())
    })
    .await
    .map_err(|_| {
        ApiError::new(
            "MANAGER_PROFILE_CLEANUP_UNCONFIRMED",
            "管理器浏览器目录清理工作未确认。",
        )
    })?
}
pub(super) fn manager_closed_for_cleanup(app: &AppHandle) -> bool {
    // Once native cleanup seals this manager, even an incomplete browser exit
    // is reported by a native dialog. Rebuilding it would reopen the same UDF.
    exit::cleanup_started(app)
        || app
            .try_state::<profile::ManagerProfile>()
            .is_some_and(|profile| profile.is_released())
}

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
    // The original scope can still be pinned while Env5 exit is pending.
    // Scope validity is not permission to reopen a sealed cleanup generation.
    exit::require_open(app)?;
    let profile = app
        .try_state::<profile::ManagerProfile>()
        .ok_or_else(profile::not_initialized)?;
    profile.verify()?;
    Ok(builder.data_directory(profile.directory().to_path_buf()))
}

pub(super) fn build_manager(app: &AppHandle, builder: ManagerBuilder<'_>) -> Result<(), ApiError> {
    actual::build(app, builder)
}

pub(super) fn build_failure_manager(app: &AppHandle) -> Result<(), ApiError> {
    match super::super::build_manager_window(app) {
        Err(error) if error.code == "MANAGER_PROFILE_NOT_INITIALIZED" => Ok(()),
        result => result.and_then(|()| actual::activate(app)),
    }
}

pub(super) fn finish_startup(app: AppHandle, result: Result<(), ApiError>) {
    let handle = app.state::<super::super::AppState>().handle.clone();
    let owner = app.clone();
    let dispatched = app.run_on_main_thread(move || {
        let state = owner.state::<super::super::AppState>();
        // Keep readiness sealed through real engine UDF readback. Even an
        // unrelated startup error can use this verified manager to show status.
        if let Err(error) = super::super::build_manager_window(&owner) {
            state.handle.fail_native_startup(error.clone());
            eprintln!("Manager actual profile: {}", error.code);
            owner
                .dialog()
                .message(error.message)
                .title("熊猫小抽屉 · 浏览器目录无法核实")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
            return;
        }
        match result {
            Ok(()) => state.handle.complete_native_startup(),
            Err(error) => state.handle.fail_native_startup(error),
        }
        if let Err(error) = actual::activate(&owner) {
            state.handle.fail_native_startup(error.clone());
            eprintln!("Manager actual profile: {}", error.code);
            owner
                .dialog()
                .message(error.message)
                .title("熊猫小抽屉 · 浏览器目录无法核实")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    });
    if let Err(error) = dispatched {
        handle.fail_native_startup(ApiError::new(
            "MANAGER_PROFILE_ACTUAL_UNKNOWN",
            "管理窗口目录检查未完成，平台尚未就绪。",
        ));
        eprintln!("Manager UI dispatch: {error}");
    }
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
