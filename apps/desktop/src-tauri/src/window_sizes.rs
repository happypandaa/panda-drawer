//! Native-only size handling for application windows, never the manager.
use platform_core::app_windows::{
    fit_window_size, AppWindowConfig, LogicalWindowSize, WindowIdentity, WindowSizeStore,
};
use std::{collections::BTreeMap, path::Path, sync::Mutex};
use tauri::{Manager, WindowEvent};

#[derive(Clone)]
struct Binding {
    identity: WindowIdentity,
    config: AppWindowConfig,
}
pub(super) struct WindowSizes {
    store: WindowSizeStore,
    bindings: Mutex<BTreeMap<String, Binding>>,
}

fn screen(monitor: &tauri::Monitor) -> Option<LogicalWindowSize> {
    let area = monitor.work_area();
    LogicalWindowSize::from_physical(area.size.width, area.size.height, monitor.scale_factor())
}
fn normal(window: &tauri::Window) -> bool {
    window.is_minimized().is_ok_and(|value| !value)
        && window.is_maximized().is_ok_and(|value| !value)
        && window.is_fullscreen().is_ok_and(|value| !value)
}

impl WindowSizes {
    pub fn new(data_root: &Path) -> Self {
        let store = WindowSizeStore::new(data_root);
        if let Some(warning) = store.warning() {
            eprintln!("{warning}");
        }
        Self {
            store,
            bindings: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn initial(
        &self,
        app: &tauri::AppHandle,
        identity: &WindowIdentity,
        config: AppWindowConfig,
    ) -> LogicalWindowSize {
        let monitor = app
            .get_webview_window("manager")
            .and_then(|window| window.current_monitor().ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten());
        self.store
            .initial_size(identity, config, monitor.as_ref().and_then(screen))
    }

    pub fn track(
        &self,
        window: &tauri::WebviewWindow,
        identity: WindowIdentity,
        config: AppWindowConfig,
    ) {
        // Confirm the actual placement as OS placement may choose another
        // display than the manager. Never retain a prior screen's oversize.
        if let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) {
            if let Some(size) = LogicalWindowSize::from_physical(size.width, size.height, scale) {
                let monitor = window.current_monitor().ok().flatten();
                let fitted = fit_window_size(size, monitor.as_ref().and_then(screen));
                if fitted != size {
                    let _ = window.set_size(tauri::LogicalSize::new(fitted.width, fitted.height));
                    let _ = window.center();
                }
                self.store.remember(&identity, config, fitted);
            }
        }
        self.bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(window.label().into(), Binding { identity, config });
    }

    pub fn on_event(&self, window: &tauri::Window, event: &WindowEvent) {
        let binding = self
            .bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(window.label())
            .cloned();
        let Some(binding) = binding else {
            return;
        };
        let observed = match event {
            WindowEvent::Resized(size) if normal(window) => window
                .scale_factor()
                .ok()
                .and_then(|scale| LogicalWindowSize::from_physical(size.width, size.height, scale)),
            WindowEvent::ScaleFactorChanged {
                scale_factor,
                new_inner_size,
                ..
            } if normal(window) => LogicalWindowSize::from_physical(
                new_inner_size.width,
                new_inner_size.height,
                *scale_factor,
            ),
            WindowEvent::CloseRequested { .. } if normal(window) => window
                .inner_size()
                .ok()
                .zip(window.scale_factor().ok())
                .and_then(|(size, scale)| {
                    LogicalWindowSize::from_physical(size.width, size.height, scale)
                }),
            _ => None,
        };
        if let Some(size) = observed {
            self.store.remember(&binding.identity, binding.config, size);
        }
        if matches!(
            event,
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
        ) {
            self.flush();
        }
        if matches!(event, WindowEvent::Destroyed) {
            self.bindings
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(window.label());
        }
    }

    pub fn flush(&self) {
        if let Err(error) = self.store.flush() {
            if let Some(path) = error.details["preservedPath"].as_str() {
                eprintln!(
                    "Window size preferences: {} (preserved at {path})",
                    error.code
                );
            } else {
                eprintln!("Window size preferences: {}", error.code);
            }
        }
    }
}
