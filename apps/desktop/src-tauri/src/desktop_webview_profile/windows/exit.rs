//! Manager closure during explicit full cleanup. The normal diagnostic manager
//! still stays alive until native exit; this path has its own Env5 receipt.
use super::{actual, profile};
use platform_core::ApiError;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager, WebviewWindow};
use tokio::sync::Notify;
use webview2_com::{
    BrowserProcessExitedEventHandler, Microsoft::Web::WebView2::Win32::ICoreWebView2Environment5,
};
use windows::core::Interface;

fn unknown(message: &str) -> ApiError {
    ApiError::new("MANAGER_PROFILE_EXIT_UNCONFIRMED", message).outcome("unknown")
}
#[derive(Default)]
struct State {
    pid: u32,
    exited: u32,
    sealed: bool,
    destroyed: bool,
    unknown: bool,
    retired: bool,
    observer_removed: bool,
}
impl State {
    fn closed(&self) -> bool {
        !self.unknown && self.sealed && self.destroyed && self.pid != 0 && self.exited == self.pid
    }
}
#[derive(Default)]
struct Owner {
    state: Mutex<State>,
    changed: Notify,
}
struct Managed(Arc<Owner>);
struct Observer {
    environment: ICoreWebView2Environment5,
    token: i64,
}
thread_local! { static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) }; }

fn owner(app: &AppHandle) -> Result<Arc<Owner>, ApiError> {
    app.try_state::<Managed>()
        .map(|owner| owner.0.clone())
        .ok_or_else(|| unknown("管理器浏览器退出观察缺失。"))
}
pub(super) fn cleanup_started(app: &AppHandle) -> bool {
    owner(app).is_ok_and(|owner| {
        let state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
        state.sealed || state.destroyed
    })
}
pub(super) fn require_open(app: &AppHandle) -> Result<(), ApiError> {
    if cleanup_started(app) {
        return Err(ApiError::new(
            "MANAGER_PROFILE_RETIRED",
            "管理器已为本次完整清理封门，未重新创建浏览器存储。",
        ));
    }
    Ok(())
}
fn pid(webview: &tauri::webview::PlatformWebview) -> Result<u32, ApiError> {
    let mut pid = 0;
    unsafe {
        webview
            .controller()
            .CoreWebView2()
            .and_then(|view| view.BrowserProcessId(&mut pid))
    }
    .map_err(|_| unknown("无法核实管理器浏览器最后进程。"))?;
    if pid == 0 {
        return Err(unknown("管理器浏览器进程身份为空。"));
    }
    Ok(pid)
}
pub(super) fn install(
    app: &AppHandle,
    webview: &tauri::webview::PlatformWebview,
) -> Result<(), ApiError> {
    let pid = pid(webview)?;
    if app.try_state::<Managed>().is_some() {
        return Err(unknown("管理器退出凭据已经存在，未覆盖原凭据。"));
    }
    let environment = webview
        .environment()
        .cast::<ICoreWebView2Environment5>()
        .map_err(|_| unknown("管理器 WebView2 无法提供浏览器退出回执。"))?;
    let owner = Arc::new(Owner::default());
    owner.state.lock().unwrap_or_else(|p| p.into_inner()).pid = pid;
    let captured = owner.clone();
    let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = args {
            let mut pid = 0;
            unsafe {
                args.BrowserProcessId(&mut pid)?;
            }
            let mut state = captured.state.lock().unwrap_or_else(|p| p.into_inner());
            if pid == state.pid {
                state.exited = pid;
            }
            captured.changed.notify_waiters();
        }
        Ok(())
    }));
    let mut token = 0;
    unsafe { environment.add_BrowserProcessExited(&handler, &mut token) }
        .map_err(|_| unknown("管理器浏览器退出观察登记失败。"))?;
    OBSERVER.with(|observer| *observer.borrow_mut() = Some(Observer { environment, token }));
    if !app.manage(Managed(owner)) {
        return Err(unknown("管理器浏览器退出观察重复。"));
    }
    Ok(())
}
pub(super) fn destroyed(app: &AppHandle) {
    if let Ok(owner) = owner(app) {
        let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
        state.destroyed = true;
        if !state.sealed {
            state.unknown = true;
        }
        owner.changed.notify_waiters();
    }
}
pub(super) fn seal(app: &AppHandle, window: &WebviewWindow) -> Result<(), ApiError> {
    let owner = owner(app)?;
    let reply = Arc::new(Mutex::new(None));
    let send = reply.clone();
    let app = app.clone();
    window
        .with_webview(move |webview| {
            let result = (|| {
                // Actual UDF was verified before manager navigation. Verify the
                // original pinned root again before sealing its final browser PID.
                app.try_state::<profile::ManagerProfile>()
                    .ok_or_else(profile::not_initialized)?
                    .verify()?;
                let pid = pid(&webview)?;
                let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
                if state.unknown || (state.sealed && state.pid != pid) {
                    state.unknown = true;
                    return Err(unknown("管理器浏览器关闭进程已变化。"));
                }
                state.pid = pid;
                state.sealed = true;
                Ok(())
            })();
            *send.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
        })
        .map_err(|_| unknown("管理器浏览器关闭检查派发失败。"))?;
    let result = reply.lock().unwrap_or_else(|p| p.into_inner()).take();
    result.ok_or_else(|| unknown("管理器关闭检查未在主线程完成。"))?
}
pub(super) async fn close(app: &AppHandle) -> Result<(), ApiError> {
    let owner = owner(app)?;
    let retired = owner
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retired;
    if retired {
        return Ok(());
    }
    let (send, receive) = tokio::sync::oneshot::channel();
    let dispatch = app.clone();
    let app_owner = app.clone();
    let captured = owner.clone();
    dispatch
        .run_on_main_thread(move || {
            let already_destroyed = captured
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .destroyed;
            let result = if already_destroyed {
                Ok(())
            } else {
                actual::destroy_for_cleanup(&app_owner)
            };
            let _ = send.send(result);
        })
        .map_err(|_| unknown("管理器关闭派发失败。"))?;
    receive
        .await
        .map_err(|_| unknown("管理器关闭回执丢失。"))??;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let notified = owner.changed.notified();
            {
                let state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
                if state.closed() {
                    return Ok(());
                }
                if state.unknown {
                    return Err(unknown("管理器浏览器关闭状态未知，已保留原目录。"));
                }
            }
            notified.await;
        }
    })
    .await
    .map_err(|_| unknown("管理器浏览器家族尚未确认退出，已保留原目录。"))??;
    let (send, receive) = tokio::sync::oneshot::channel();
    let dispatch = app.clone();
    let app_owner = app.clone();
    dispatch
        .run_on_main_thread(move || {
            let result = (|| {
                if owner
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retired
                {
                    return Ok(());
                }
                if !owner
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .observer_removed
                {
                    OBSERVER.with(|observer| {
                        let mut observer = observer.borrow_mut();
                        let original = observer
                            .as_ref()
                            .ok_or_else(|| unknown("管理器退出观察缺失。"))?;
                        unsafe {
                            original
                                .environment
                                .remove_BrowserProcessExited(original.token)
                        }
                        .map_err(|_| unknown("管理器退出观察尚未结束。"))?;
                        observer.take();
                        Ok::<(), ApiError>(())
                    })?;
                    owner
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .observer_removed = true;
                }
                app_owner
                    .try_state::<profile::ManagerProfile>()
                    .ok_or_else(profile::not_initialized)?
                    .release_after_native_close()?;
                owner
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retired = true;
                Ok(())
            })();
            let _ = send.send(result);
        })
        .map_err(|_| unknown("管理器浏览器凭据释放派发失败。"))?;
    receive
        .await
        .map_err(|_| unknown("管理器浏览器凭据释放回执丢失。"))?
}
