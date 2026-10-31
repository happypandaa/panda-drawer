//! Bind the engine's actual user-data directory before allowing manager page
//! navigation. WebView2 environment/registry overrides can replace UDF input.
use super::{profile, ApiError, AppHandle, ManagerBuilder};
use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Manager, Url, WebviewWindow};
use webview2_com::{CoTaskMemPWSTR, Microsoft::Web::WebView2::Win32::ICoreWebView2Environment7};
use windows::core::{Interface, PWSTR};

fn unknown(message: &str) -> ApiError {
    ApiError::new("MANAGER_PROFILE_ACTUAL_UNKNOWN", message).outcome("not_started")
}

#[derive(Default)]
struct Navigation {
    active: AtomicBool,
}
impl Navigation {
    fn allows(&self, url: &Url) -> bool {
        if !super::super::is_manager_url(url, cfg!(debug_assertions)) {
            return false;
        }
        self.active.load(Ordering::Acquire)
    }
    fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }
    fn seal(&self) {
        self.active.store(false, Ordering::Release);
    }
}

struct ManagerTarget(Url);
pub(super) fn builder_from_config<'a>(
    app: &'a AppHandle,
    config: &tauri::utils::config::WindowConfig,
) -> Result<ManagerBuilder<'a>, ApiError> {
    use tauri::utils::config::{FrontendDist, WebviewUrl};
    // Mirror locked Tauri's desktop URL resolution, retaining the original
    // config target while constructing an inert about:blank WebView. Runtime
    // creation can pump native messages before on_navigation is installed.
    let target = match &config.url {
        WebviewUrl::App(path) => {
            let configured = if tauri::is_dev() {
                app.config().build.dev_url.as_ref()
            } else {
                match app.config().build.frontend_dist.as_ref() {
                    Some(FrontendDist::Url(url)) => Some(url),
                    _ => None,
                }
            };
            let base = configured.cloned().unwrap_or_else(|| {
                Url::parse(if config.use_https_scheme {
                    "https://tauri.localhost"
                } else {
                    "http://tauri.localhost"
                })
                .unwrap()
            });
            if path.to_str() == Some("index.html") {
                base
            } else {
                base.join(&path.to_string_lossy())
                    .map_err(|_| unknown("管理页面原始地址无法解析。"))?
            }
        }
        WebviewUrl::External(url) | WebviewUrl::CustomProtocol(url) => url.clone(),
        _ => return Err(unknown("管理页面原始地址类型无法核实。")),
    };
    if !super::super::is_manager_url(&target, cfg!(debug_assertions)) {
        return Err(unknown("管理页面原始地址不属于管理器。"));
    }
    if !app.manage(ManagerTarget(target)) {
        return Err(unknown("管理页面原始地址已绑定，未覆盖原窗口。"));
    }
    let mut blank = config.clone();
    blank.url = WebviewUrl::External(Url::parse("about:blank").unwrap());
    blank.visible = false;
    tauri::WebviewWindowBuilder::from_config(app, &blank)
        .map_err(|error| ApiError::new("MANAGER_WINDOW_OPEN_FAILED", error.to_string()))
}

struct VerifiedManager {
    window: WebviewWindow,
    navigation: Arc<Navigation>,
    target: Url,
}

fn actual_directory(webview: &tauri::webview::PlatformWebview) -> Result<PathBuf, ApiError> {
    let environment = webview
        .environment()
        .cast::<ICoreWebView2Environment7>()
        .map_err(|_| unknown("当前 WebView2 无法报告实际浏览器目录，管理页面尚未加载。"))?;
    let mut path = PWSTR::null();
    let result = unsafe { environment.UserDataFolder(&mut path) };
    // Own even a pointer returned alongside a failed HRESULT. Keep UTF-16
    // unchanged rather than routing a filesystem name through lossy UTF-8.
    let _allocation = CoTaskMemPWSTR::from(path);
    result.map_err(|_| unknown("WebView2 实际浏览器目录查询失败，管理页面尚未加载。"))?;
    if path.is_null() {
        return Err(unknown("WebView2 未报告实际浏览器目录，管理页面尚未加载。"));
    }
    let units = unsafe { path.as_wide() };
    if units.is_empty() {
        return Err(unknown("WebView2 实际浏览器目录为空，管理页面尚未加载。"));
    }
    Ok(PathBuf::from(OsString::from_wide(units)))
}

fn verify_on_main(app: &AppHandle, window: &WebviewWindow) -> Result<(), ApiError> {
    let result = Arc::new(Mutex::new(None));
    let reply = result.clone();
    let owner = app.clone();
    window
        .with_webview(move |webview| {
            let checked = actual_directory(&webview).and_then(|actual| {
                owner
                    .try_state::<profile::ManagerProfile>()
                    .ok_or_else(profile::not_initialized)?
                    .verify_actual_directory(&actual)?;
                super::exit::install(&owner, &webview)
            });
            if checked.is_err() {
                // Close only this exact controller. An override's actual directory
                // is never adopted, inspected for contents, repaired or deleted.
                let _ = unsafe { webview.controller().Close() };
            }
            *reply.lock().unwrap_or_else(|p| p.into_inner()) = Some(checked);
        })
        .map_err(|_| unknown("无法核实实际 WebView2，管理页面尚未加载。"))?;
    // Locked Tauri-runtime-wry dispatches WithWebview inline on its main
    // thread. Unexpected deferred execution is unknown, never a success.
    let checked = result.lock().unwrap_or_else(|p| p.into_inner()).take();
    checked.ok_or_else(|| unknown("WebView2 实际目录检查未在主线程完成，管理页面尚未加载。"))??;
    let exit_app = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            super::exit::destroyed(&exit_app);
        }
    });
    Ok(())
}

pub(super) fn build(app: &AppHandle, builder: ManagerBuilder<'_>) -> Result<(), ApiError> {
    let navigation = Arc::new(Navigation::default());
    let guard = navigation.clone();
    let window = builder
        .visible(false)
        .on_navigation(move |url| guard.allows(url))
        .build()
        .map_err(|error| ApiError::new("MANAGER_WINDOW_OPEN_FAILED", error.to_string()))?;
    if let Err(error) = verify_on_main(app, &window) {
        let _ = window.destroy();
        return Err(error);
    }
    if !app.manage(VerifiedManager {
        window: window.clone(),
        navigation,
        target: app
            .try_state::<ManagerTarget>()
            .ok_or_else(|| unknown("管理页面原始地址缺失。"))?
            .0
            .clone(),
    }) {
        let _ = window.destroy();
        return Err(unknown("管理窗口目录凭据已经存在，未替换原凭据。"));
    }
    Ok(())
}

pub(super) fn destroy_for_cleanup(app: &AppHandle) -> Result<(), ApiError> {
    let binding = app
        .try_state::<VerifiedManager>()
        .ok_or_else(|| unknown("原管理器窗口凭据缺失。"))?;
    binding.navigation.seal();
    super::exit::seal(app, &binding.window)?;
    binding
        .window
        .destroy()
        .map_err(|_| unknown("管理器原生窗口销毁未确认。"))
}

pub(super) fn activate(app: &AppHandle) -> Result<(), ApiError> {
    let binding = app
        .try_state::<VerifiedManager>()
        .ok_or_else(|| unknown("管理窗口实际目录尚未核实，管理页面尚未加载。"))?;
    binding.navigation.activate();
    binding
        .window
        .navigate(binding.target.clone())
        .map_err(|_| unknown("管理页面首次导航未完成，请重新启动平台。"))?;
    binding
        .window
        .show()
        .map_err(|_| unknown("无法显示已核实目录的管理窗口。"))
}

#[cfg(test)]
#[path = "actual_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "actual_fixture.rs"]
mod fixture;
