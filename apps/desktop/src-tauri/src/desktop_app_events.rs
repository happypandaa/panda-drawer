//! Native manager-window and application lifecycle behavior.
use super::{desktop_shutdown_native, request_safe_shutdown, AppState, StartupFailure};
use tauri::Manager;

pub(super) fn window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let Some(state) = window.app_handle().try_state::<AppState>() {
        if !matches!(event, tauri::WindowEvent::Destroyed) {
            if let Ok(_io_lease) = state.handle.io_lifetime().admit_cleanup() {
                state.window_sizes.on_event(window, event);
            }
        }
    }
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        if window.label() == "manager" {
            api.prevent_close();
            if window
                .app_handle()
                .try_state::<AppState>()
                .is_some_and(|state| state.local_data_cleanup.active())
            {
                return;
            }
            // macOS keeps the manager and managed services in the Dock until
            // explicit quit; Reopen restores this same window.
            #[cfg(target_os = "macos")]
            if let Err(error) = window.hide() {
                eprintln!("Unable to hide management window: {error}");
            }
            #[cfg(not(target_os = "macos"))]
            window.app_handle().exit(0);
        }
    }
}

pub(super) fn run_event(app: &tauri::AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            let wait_for_running = app
                .try_state::<AppState>()
                .is_some_and(|state| !state.exit_ready.load(std::sync::atomic::Ordering::Acquire));
            let wait_for_partial = app.try_state::<StartupFailure>().is_some_and(|failure| {
                failure.runtime.is_some()
                    && !failure
                        .exit_ready
                        .load(std::sync::atomic::Ordering::Acquire)
            });
            if wait_for_running || wait_for_partial {
                api.prevent_exit();
                request_safe_shutdown(app);
            }
        }
        // Forced termination can bypass the preventable request. Retain the
        // owned cleanup fallback without blocking the event loop.
        tauri::RunEvent::Exit => desktop_shutdown_native::forced_exit(app),
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => super::show_manager(app),
        _ => {}
    }
}
