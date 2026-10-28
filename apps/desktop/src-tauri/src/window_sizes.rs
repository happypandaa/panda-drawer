//! Native-only size handling for application windows, never the manager.
use platform_core::app_windows::{
    fit_window_size, AppWindowConfig, LogicalWindowSize, WindowIdentity, WindowSizePermit,
    WindowSizeStore,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tauri::{Manager, WindowEvent};

#[derive(Clone)]
struct Binding {
    identity: WindowIdentity,
    config: AppWindowConfig,
    permit: WindowSizePermit,
}
impl Binding {
    fn remember(&self, store: &WindowSizeStore, size: LogicalWindowSize) {
        store.remember(&self.permit, self.config, size);
    }
}
pub(super) struct WindowSizes {
    store: Arc<WindowSizeStore>,
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
    pub fn new(store: Arc<WindowSizeStore>) -> Self {
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
        if !config.remember_size {
            return;
        }
        let permit = match self.store.permit(&identity) {
            Ok(permit) => permit,
            Err(error) => {
                eprintln!("Window size preferences: {}", error.code);
                return;
            }
        };
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
                self.store.remember(&permit, config, fitted);
            }
        }
        let mut bindings = self.bindings.lock().unwrap_or_else(|p| p.into_inner());
        // Deletion can revoke a permit while OS placement is being inspected.
        // A late track must not recreate even an in-memory binding.
        if !permit.is_revoked() {
            bindings.insert(
                window.label().into(),
                Binding {
                    identity,
                    config,
                    permit,
                },
            );
        }
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
            binding.remember(&self.store, size);
        }
        if matches!(
            event,
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
        ) {
            self.flush();
        }
        if matches!(event, WindowEvent::Destroyed) {
            self.untrack(window.label());
        }
    }

    pub fn untrack(&self, label: &str) {
        if let Some(binding) = self
            .bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(label)
        {
            binding.permit.revoke();
        }
    }
    pub fn forget_project(&self, kind: &str, id: &str) {
        self.bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, binding| {
                if binding.identity.matches_project(kind, id) {
                    binding.permit.revoke();
                    false
                } else {
                    true
                }
            });
    }

    pub fn flush(&self) {
        if let Err(error) = self.store.flush() {
            if let Some(path) = error.details["conflictPath"].as_str() {
                eprintln!(
                    "Window size preferences: {} (project conflict at {path})",
                    error.code
                );
            } else {
                eprintln!("Window size preferences: {}", error.code);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};
    fn owner(root: &Path, id: &str) -> std::path::PathBuf {
        let parent = root.join("web-project-ownership");
        let owner = parent.join(id);
        fs::create_dir_all(&owner).unwrap();
        for path in [&parent, &owner] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(owner.join("ownership.json"), b"{}").unwrap();
        fs::set_permissions(
            owner.join("ownership.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        owner
    }
    fn binding(store: &WindowSizeStore, id: &str) -> Binding {
        let identity = WindowIdentity::project(Path::new("/disposable/window-workspace"), id);
        let permit = store.permit(&identity).unwrap();
        Binding {
            identity,
            config: AppWindowConfig::default(),
            permit,
        }
    }
    #[test]
    fn forgotten_project_drops_native_binding_and_stale_clones_cannot_save_after_close_flush() {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-native-window-sizes-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let a = owner(&root, "prj_window_a");
        let b = owner(&root, "prj_window_b");
        let store = Arc::new(WindowSizeStore::new(&root));
        let sizes = WindowSizes::new(store.clone());
        let a_binding = binding(&store, "prj_window_a");
        let late = a_binding.clone();
        let b_binding = binding(&store, "prj_window_b");
        a_binding.remember(
            &store,
            LogicalWindowSize {
                width: 900.0,
                height: 650.0,
            },
        );
        b_binding.remember(
            &store,
            LogicalWindowSize {
                width: 1000.0,
                height: 750.0,
            },
        );
        sizes.bindings.lock().unwrap().insert("a".into(), a_binding);
        sizes.bindings.lock().unwrap().insert("b".into(), b_binding);
        sizes.flush();
        store.forget_project("web", "prj_window_a").unwrap();
        sizes.forget_project("web", "prj_window_a");
        assert!(!sizes.bindings.lock().unwrap().contains_key("a"));
        assert!(sizes.bindings.lock().unwrap().contains_key("b"));
        assert!(late.permit.is_revoked());
        late.remember(
            &store,
            LogicalWindowSize {
                width: 1200.0,
                height: 800.0,
            },
        );
        sizes.flush();
        assert!(!a.join("window-size.json").exists());
        assert!(b.join("window-size.json").exists());
        fs::remove_dir_all(&a).unwrap();
        store.finish_forget("web", "prj_window_a").unwrap();
        late.remember(
            &store,
            LogicalWindowSize {
                width: 1100.0,
                height: 800.0,
            },
        );
        sizes.flush();
        assert!(!a.exists());
    }
    #[test]
    fn window_generation_change_revokes_old_binding_without_touching_another_project() {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-native-window-generation-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        owner(&root, "prj_window_a");
        owner(&root, "prj_window_b");
        let store = Arc::new(WindowSizeStore::new(&root));
        let old = binding(&store, "prj_window_a");
        let b = binding(&store, "prj_window_b");
        let current = binding(&store, "prj_window_a");
        assert!(old.permit.is_revoked());
        assert!(!b.permit.is_revoked());
        current.remember(
            &store,
            LogicalWindowSize {
                width: 950.0,
                height: 700.0,
            },
        );
        old.remember(
            &store,
            LogicalWindowSize {
                width: 800.0,
                height: 600.0,
            },
        );
        store.flush().unwrap();
        assert_eq!(
            store.initial_size(&current.identity, AppWindowConfig::default(), None),
            LogicalWindowSize {
                width: 950.0,
                height: 700.0
            }
        );
    }
}
