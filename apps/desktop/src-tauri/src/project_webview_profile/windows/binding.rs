use super::owner::{self, BrowserOwner};
use platform_core::{
    project_cleanup::ProjectKind,
    webview_storage::{BoundProfile, StoreLedger},
    ApiError,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use tauri::{AppHandle, Manager, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment7;
use webview2_com::{
    BrowserProcessExitedEventHandler, CoTaskMemPWSTR,
    Microsoft::Web::WebView2::Win32::ICoreWebView2Environment5,
};
use windows::core::{Interface, PWSTR};

fn unknown(message: &str) -> ApiError {
    ApiError::new("WEBVIEW_PROFILE_PROCESS_UNKNOWN", message)
}

struct Prepared {
    profile: Arc<BoundProfile>,
    engine_directory: PathBuf,
    engine_scope: Arc<platform_native::windows::files::PinnedDirectory>,
    label: String,
    owner: OnceLock<Arc<BrowserOwner>>,
    recovering: bool,
}
#[derive(Clone)]
pub(crate) struct Binding(Arc<Prepared>);
impl Binding {
    pub(crate) fn prepare(
        app: &AppHandle,
        ledger: &StoreLedger,
        kind: ProjectKind,
        id: &str,
        label: &str,
    ) -> Result<Self, ApiError> {
        Self::prepare_with(app, ledger, kind, id, label, false)
    }
    pub(crate) fn prepare_for_cleanup(
        app: &AppHandle,
        ledger: &StoreLedger,
        kind: ProjectKind,
        id: &str,
        label: &str,
    ) -> Result<Self, ApiError> {
        Self::prepare_with(app, ledger, kind, id, label, true)
    }
    fn prepare_with(
        app: &AppHandle,
        ledger: &StoreLedger,
        kind: ProjectKind,
        id: &str,
        label: &str,
        recovering: bool,
    ) -> Result<Self, ApiError> {
        super::super::require_profile_label(kind, id, label)?;
        owner::require_available(app, label)?;
        let profile = Arc::new(ledger.bind_profile(kind, id)?);
        // The original record's full identity and pins already authorize this
        // root. External engine descendants need the same user-only access;
        // ordinary ancestor or credential permissions are never repaired here.
        let selected = platform_native::windows::files::open_no_reparse(profile.directory())?;
        platform_native::windows::files::enable_private_directory_inheritance(
            profile.directory(),
            &selected.identity,
        )
        .map_err(|cause| {
            ApiError::new(
                "WEBVIEW_PROFILE_PRIVATE_PREPARATION_FAILED",
                format!("无法准备项目浏览器的私有存储权限，项目页面尚未打开：{cause}"),
            )
        })?;
        profile.verify()?;
        let (canonical, engine_scope) =
            platform_native::windows::files::directory_scope(&selected.file)?;
        let engine_directory =
            platform_native::windows::api_paths::conventional_path(&canonical)
                .map_err(|cause| unknown(&format!("无法准备项目浏览器的 Windows 路径：{cause}")))?;
        profile.verify_actual_directory(&engine_directory)?;
        Ok(Self(Arc::new(Prepared {
            profile,
            engine_directory,
            engine_scope: Arc::new(engine_scope),
            label: label.into(),
            owner: OnceLock::new(),
            recovering,
        })))
    }
    pub(crate) fn directory(&self) -> &std::path::Path {
        &self.0.engine_directory
    }
    pub(crate) fn reserve(&self, app: &AppHandle, generation: u64) -> Result<(), ApiError> {
        if self.0.owner.get().is_some() {
            return Err(unknown("本次项目浏览器目录已登记，未替换原凭据。"));
        }
        let owner = if self.0.recovering {
            BrowserOwner::new_for_cleanup(self.0.label.clone(), generation, self.0.profile.clone())?
        } else {
            BrowserOwner::new(self.0.label.clone(), generation, self.0.profile.clone())?
        };
        // Keep the SDK's canonical namespace pinned until the real exit, too;
        // a DOS alias must not shorten the original scope's ancestor lifetime.
        owner.hold_resource(self.0.engine_scope.clone())?;
        owner::register(app, owner.clone())?;
        self.0
            .owner
            .set(owner)
            .map_err(|_| unknown("本次项目浏览器目录已登记，未替换原凭据。"))
    }
    fn owner(&self) -> Result<Arc<BrowserOwner>, ApiError> {
        self.0
            .owner
            .get()
            .cloned()
            .ok_or_else(|| unknown("本次项目浏览器目录尚未登记。"))
    }
    pub(crate) fn hold_resource(
        &self,
        resource: impl Send + Sync + 'static,
    ) -> Result<(), ApiError> {
        self.owner()?.hold_resource(resource)
    }
    pub(crate) fn document_allowed(&self) -> bool {
        self.0
            .owner
            .get()
            .is_some_and(|owner| owner.document_allowed())
    }
    pub(crate) fn creation_failed(&self) {
        if let Some(owner) = self.0.owner.get() {
            owner.fail();
        }
    }
    pub(crate) fn bind_window(&self, window: &WebviewWindow) -> Result<(), ApiError> {
        let owner = self.owner()?;
        let captured = owner.clone();
        let result = Arc::new(Mutex::new(None));
        let reply = result.clone();
        let window_app = window.app_handle().clone();
        window
            .with_webview(move |webview| {
                let checked = (|| {
                    let actual = actual_directory(&webview)?;
                    captured.profile()?.verify_actual_directory(&actual)?;
                    let environment = webview
                        .environment()
                        .cast::<ICoreWebView2Environment5>()
                        .map_err(|_| unknown("当前 WebView2 无法提供项目浏览器退出回执。"))?;
                    install_observer(window_app, environment, captured.clone())?;
                    captured.bind_pid(browser_pid(&webview)?)
                })();
                if checked.is_err() {
                    captured.fail();
                    let _ = unsafe { webview.controller().Close() };
                }
                *reply.lock().unwrap_or_else(|p| p.into_inner()) = Some(checked);
            })
            .map_err(|_| {
                owner.fail();
                unknown("项目 WebView2 实际目录检查派发失败。")
            })?;
        let checked = result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .ok_or_else(|| {
                owner.fail();
                unknown("项目 WebView2 检查未在主线程完成。")
            })?;
        if checked.is_err() {
            owner.fail();
            let _ = window.destroy();
        }
        checked
    }
}

fn actual_directory(webview: &tauri::webview::PlatformWebview) -> Result<PathBuf, ApiError> {
    let environment = webview
        .environment()
        .cast::<ICoreWebView2Environment7>()
        .map_err(|_| unknown("当前 WebView2 无法报告实际项目存储目录。"))?;
    let mut path = PWSTR::null();
    let result = unsafe { environment.UserDataFolder(&mut path) };
    let _allocation = CoTaskMemPWSTR::from(path);
    result.map_err(|_| unknown("WebView2 实际项目存储目录查询失败。"))?;
    if path.is_null() {
        return Err(unknown("WebView2 未报告实际项目存储目录。"));
    }
    let units = unsafe { path.as_wide() };
    if units.is_empty() {
        return Err(unknown("WebView2 实际项目存储目录为空。"));
    }
    Ok(PathBuf::from(OsString::from_wide(units)))
}
fn browser_pid(webview: &tauri::webview::PlatformWebview) -> Result<u32, ApiError> {
    let mut pid = 0;
    unsafe {
        webview
            .controller()
            .CoreWebView2()
            .and_then(|view| view.BrowserProcessId(&mut pid))
    }
    .map_err(|_| unknown("项目浏览器进程标识无法核实。"))?;
    if pid == 0 {
        return Err(unknown("项目浏览器进程标识为空。"));
    }
    Ok(pid)
}

#[cfg(test)]
pub(super) fn fixture_facts(window: &WebviewWindow) -> Result<(PathBuf, u32, bool), ApiError> {
    let result = Arc::new(Mutex::new(None));
    let reply = result.clone();
    window
        .with_webview(move |webview| {
            let checked = (|| {
                let actual = actual_directory(&webview)?;
                let pid = browser_pid(&webview)?;
                let mut private = Default::default();
                unsafe {
                    webview.controller().CoreWebView2()
                        .and_then(|view| view.cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_13>())
                        .and_then(|view| view.Profile())
                        .and_then(|profile| profile.IsInPrivateModeEnabled(&mut private))
                }.map_err(|_| unknown("Fixture actual InPrivate status unavailable"))?;
                Ok((actual,pid,private.as_bool()))
            })();
            *reply.lock().unwrap_or_else(|p| p.into_inner()) = Some(checked);
        })
        .map_err(|_| unknown("Fixture native facts dispatch failed"))?;
    let checked = result
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
        .ok_or_else(|| unknown("Fixture native facts missing"))?;
    checked
}

struct Observer {
    generation: u64,
    environment: ICoreWebView2Environment5,
    token: i64,
}
thread_local! {
    // COM interfaces stay in the Tauri native apartment; no unsafe Send/Sync.
    static OBSERVERS: RefCell<BTreeMap<String, Observer>> = const { RefCell::new(BTreeMap::new()) };
}
fn install_observer(
    app: AppHandle,
    environment: ICoreWebView2Environment5,
    owner: Arc<BrowserOwner>,
) -> Result<(), ApiError> {
    OBSERVERS.with(|observers| {
        if observers.borrow().contains_key(&owner.label) {
            return Err(unknown("旧项目浏览器退出观察仍在，未覆盖其回执。"));
        }
        let callback_owner = owner.clone();
        let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let mut pid = 0;
                unsafe {
                    args.BrowserProcessId(&mut pid)?;
                }
                #[cfg(test)]
                {
                    let mut kind = webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL;
                    let observed = unsafe { args.BrowserProcessExitKind(&mut kind) };
                    eprintln!("Actual project browser exit: label={}, generation={}, pid={}, kind={kind:?}, result={observed:?}", callback_owner.label, callback_owner.generation, pid);
                }
                // This environment event represents the entire browser family
                // releasing the UDF, on normal or failed browser termination.
                callback_owner.browser_exited(pid);
                schedule_retirement(&app, &callback_owner);
            }
            Ok(())
        }));
        let mut token = 0;
        unsafe { environment.add_BrowserProcessExited(&handler, &mut token) }
            .map_err(|_| unknown("项目浏览器退出回执登记失败。"))?;
        observers.borrow_mut().insert(
            owner.label.clone(),
            Observer {
                generation: owner.generation,
                environment,
                token,
            },
        );
        Ok(())
    })
}

pub(crate) fn before_destroy(
    app: &AppHandle,
    window: &WebviewWindow,
    generation: u64,
) -> Result<(), ApiError> {
    let owner = owner::current(app, window.label(), generation)?;
    let reply = Arc::new(Mutex::new(None));
    let result = reply.clone();
    let captured = owner.clone();
    window
        .with_webview(move |webview| {
            let checked = (|| {
                captured
                    .profile()?
                    .verify_actual_directory(&actual_directory(&webview)?)?;
                // A browser can crash/restart within an environment. A past exit
                // for an earlier PID cannot acknowledge the last live generation.
                captured.seal_pid(browser_pid(&webview)?)
            })();
            *result.lock().unwrap_or_else(|p| p.into_inner()) = Some(checked);
        })
        .map_err(|_| {
            owner.fail();
            unknown("项目浏览器关闭前核验派发失败。")
        })?;
    let checked = reply
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
        .ok_or_else(|| {
            owner.fail();
            unknown("项目浏览器关闭前核验未完成。")
        })?;
    if checked.is_err() {
        owner.fail();
    }
    checked
}

pub(crate) fn window_destroyed(app: &AppHandle, label: &str, generation: u64) {
    match owner::current(app, label, generation) {
        Ok(owner) => {
            owner.destroyed();
            schedule_retirement(app, &owner);
        }
        Err(issue) => eprintln!("Project browser Destroyed receipt retained: {}", issue.code),
    }
}

fn schedule_retirement(app: &AppHandle, owner: &Arc<BrowserOwner>) {
    if !owner.ready_to_retire() || owner.closed_and_retired() {
        return;
    }
    let app = app.clone();
    let owner = owner.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(issue) = wait_closed(&app, &owner.label, owner.generation).await {
            eprintln!("Project browser owner retained: {}", issue.code);
        }
    });
}

pub(crate) async fn wait_closed(
    app: &AppHandle,
    label: &str,
    generation: u64,
) -> Result<(), ApiError> {
    let owner = owner::current(app, label, generation)?;
    owner.wait_exit().await?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let dispatch = app.clone();
    let first_app = app.clone();
    let first_label = label.to_owned();
    dispatch
        .run_on_main_thread(move || {
            let result = (|| {
                let current = owner::current(&first_app, &first_label, generation)?;
                if !Arc::ptr_eq(&owner, &current) {
                    return Err(unknown("项目浏览器目录凭据已经更换。"));
                }
                if owner.closed_and_retired() {
                    return Ok(());
                }
                if owner.observer_removed() {
                    return Ok(());
                }
                OBSERVERS.with(|observers| {
                    let mut observers = observers.borrow_mut();
                    let observer = observers
                        .get(&first_label)
                        .filter(|observer| observer.generation == generation)
                        .ok_or_else(|| unknown("原项目浏览器退出观察缺失，未释放目录凭据。"))?;
                    unsafe {
                        observer
                            .environment
                            .remove_BrowserProcessExited(observer.token)
                    }
                    .map_err(|_| unknown("项目浏览器退出观察尚未结束。"))?;
                    observers.remove(&first_label);
                    owner.mark_observer_removed();
                    Ok::<(), ApiError>(())
                })?;
                Ok(())
            })();
            let _ = send.send(result);
        })
        .map_err(|_| unknown("项目浏览器退出收尾派发失败。"))?;
    receive
        .await
        .map_err(|_| unknown("项目浏览器退出收尾回执缺失。"))??;
    let owner = owner::current(app, label, generation)?;
    let finish = owner.clone();
    tauri::async_runtime::spawn_blocking(move || finish.finish_session())
        .await
        .map_err(|_| unknown("项目浏览器会话凭据收尾未确认。"))??;
    let (send, receive) = tokio::sync::oneshot::channel();
    let dispatch = app.clone();
    let check_app = app.clone();
    let label = label.to_owned();
    dispatch
        .run_on_main_thread(move || {
            let result = owner::current(&check_app, &label, generation).and_then(|current| {
                if !Arc::ptr_eq(&owner, &current) {
                    return Err(unknown("项目浏览器目录凭据已经更换。"));
                }
                owner.retire()
            });
            let _ = send.send(result);
        })
        .map_err(|_| unknown("项目浏览器凭据释放派发失败。"))?;
    receive
        .await
        .map_err(|_| unknown("项目浏览器凭据释放回执缺失。"))?
}
