//! Native app-window actions; business pages never receive management access.
//! macOS shows titlebar buttons; Windows shows two items in the app window's
//! own menu bar. Both only ask the manager window to act on the registered app.
use serde::Serialize;

/// `{tool}` is the short name of the AI tool chosen in the manager ("AI" until one is chosen).
const BUTTON_TITLE: &str = "告诉 {tool} 怎么改";
const RELOAD_TITLE: &str = "重新加载";

static TOOL_NAME: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The handoff action's title in the current language, naming the chosen tool.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn handoff_title() -> String {
    let tool = TOOL_NAME.read().ok().and_then(|name| name.clone());
    crate::ui_locale::text(BUTTON_TITLE).replace("{tool}", tool.as_deref().unwrap_or("AI"))
}

/// The manager reports the chosen tool's short name; open app windows follow it.
#[tauri::command]
pub(super) fn set_ai_titlebar_tool(
    window: tauri::WebviewWindow,
    name: Option<String>,
) -> Result<(), platform_core::ApiError> {
    use tauri::Manager;
    super::require_manager(&window)?;
    let name = name.map(|name| name.trim().to_owned()).filter(|name| {
        !name.is_empty() && name.chars().count() <= 24 && !name.chars().any(char::is_control)
    });
    match TOOL_NAME.write() {
        Ok(mut current) if *current != name => *current = name,
        _ => return Ok(()),
    }
    for (label, window) in window.app_handle().webview_windows() {
        if label != "manager" {
            refresh_language(&window);
        }
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HandoffRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    pub app_name: String,
    pub window_generation: u64,
}

pub(super) fn install(window: &tauri::WebviewWindow, request: HandoffRequest) {
    #[cfg(target_os = "macos")]
    let installed = macos::install(window, request);
    #[cfg(windows)]
    let installed = windows::install(window, request);
    #[cfg(any(target_os = "macos", windows))]
    if let Err(error) = installed {
        // The application's existing window must remain usable if the OS
        // cannot attach this optional management action.
        eprintln!("AI titlebar action unavailable: {error}");
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (window, request, BUTTON_TITLE, RELOAD_TITLE);
}

pub(super) fn refresh_language(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let target = window.clone();
        let _ = window.run_on_main_thread(move || macos::refresh_language(&target));
    }
    #[cfg(windows)]
    windows::refresh_language(window);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = window;
}

/// Menu item identifiers; the application-wide menu handler ignores them.
const HANDOFF_ITEM: &str = "app-window-ai-handoff";
const RELOAD_ITEM: &str = "app-window-reload";

/// The event a clicked action sends to the manager window.
fn action_event(item: &str) -> Option<&'static str> {
    match item {
        HANDOFF_ITEM => Some("ai-handoff-request"),
        RELOAD_ITEM => Some("app-reload-request"),
        _ => None,
    }
}

#[cfg(any(windows, test))]
fn menu_action_id(menu: &str, item: &str) -> String {
    format!("{menu}:{item}")
}

#[cfg(any(windows, test))]
fn menu_action_event(menu: &str, item: &str) -> Option<&'static str> {
    action_event(item.strip_prefix(menu)?.strip_prefix(':')?)
}

/// Windows cannot place buttons in the system title bar, so the app window
/// gets its own menu bar with the same two actions, replacing the
/// application-wide menu for this window only.
#[cfg(windows)]
mod windows {
    use super::*;
    use tauri::{
        menu::{Menu, MenuItem},
        Emitter, Manager,
    };

    pub(super) fn install(
        window: &tauri::WebviewWindow,
        request: HandoffRequest,
    ) -> Result<(), platform_core::ApiError> {
        let fail =
            || platform_core::ApiError::new("AI_TITLEBAR_UNAVAILABLE", "无法创建应用窗口菜单。");
        let app = window.app_handle();
        let menu_id = format!(
            "app-window-actions:{}:{}",
            window.label(),
            request.window_generation
        );
        let reload = MenuItem::with_id(
            app,
            menu_action_id(&menu_id, RELOAD_ITEM),
            crate::ui_locale::text(RELOAD_TITLE),
            true,
            None::<&str>,
        )
        .map_err(|_| fail())?;
        let handoff = MenuItem::with_id(
            app,
            menu_action_id(&menu_id, HANDOFF_ITEM),
            handoff_title(),
            true,
            None::<&str>,
        )
        .map_err(|_| fail())?;
        let menu = Menu::with_id_and_items(app, menu_id.clone(), &[&reload, &handoff])
            .map_err(|_| fail())?;
        window.set_menu(menu).map_err(|_| fail())?;
        let emitter = app.clone();
        window.on_menu_event(move |_, event| {
            // Tauri sends every menu event to every open window listener.
            if let Some(name) = menu_action_event(&menu_id, event.id().as_ref()) {
                let _ = emitter.emit_to("manager", name, request.clone());
            }
        });
        Ok(())
    }

    pub(super) fn refresh_language(window: &tauri::WebviewWindow) {
        let Some(menu) = window.menu() else {
            return;
        };
        for (id, title) in [
            (RELOAD_ITEM, crate::ui_locale::text(RELOAD_TITLE).to_owned()),
            (HANDOFF_ITEM, handoff_title()),
        ] {
            let id = menu_action_id(menu.id().as_ref(), id);
            if let Some(item) = menu.get(id.as_str()) {
                if let Some(item) = item.as_menuitem() {
                    let _ = item.set_text(&title);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_platforms_send_the_same_manager_events() {
        assert_eq!(action_event(HANDOFF_ITEM), Some("ai-handoff-request"));
        assert_eq!(action_event(RELOAD_ITEM), Some("app-reload-request"));
        // Application-wide menu identifiers never reach the manager as actions.
        for other in ["show", "show-manager", "quit", ""] {
            assert_eq!(action_event(other), None);
        }
    }

    #[test]
    fn menu_events_match_only_their_app_window_generation() {
        let menu = "app-window-actions:project-a:1";
        let reload = menu_action_id(menu, RELOAD_ITEM);
        let handoff = menu_action_id(menu, HANDOFF_ITEM);
        assert_eq!(menu_action_event(menu, &reload), Some("app-reload-request"));
        assert_eq!(
            menu_action_event(menu, &handoff),
            Some("ai-handoff-request")
        );
        for other_menu in [
            "app-window-actions:project-b:1",
            "app-window-actions:project-a:10",
        ] {
            assert_eq!(menu_action_event(other_menu, &reload), None);
            assert_eq!(
                menu_action_event(menu, &menu_action_id(other_menu, RELOAD_ITEM)),
                None
            );
        }
        assert_eq!(menu_action_event(menu, RELOAD_ITEM), None);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::{
        define_class, msg_send, rc::Retained, sel, ClassType, DefinedClass, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSBezelStyle, NSButton, NSControlSize, NSLayoutAttribute,
        NSTitlebarAccessoryViewController, NSView, NSWindow,
    };
    use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use tauri::{Emitter, Manager};

    struct Action {
        app: tauri::AppHandle,
        request: HandoffRequest,
        buttons: std::cell::OnceCell<(Retained<NSButton>, Retained<NSButton>, Retained<NSView>)>,
    }

    /// The title changes with the chosen tool and the language, so the button
    /// and its accessory view are sized from the text.
    fn handoff_width(title: &str) -> f64 {
        let text: f64 = title
            .chars()
            .map(|c| if c.is_ascii() { 7. } else { 12. })
            .sum();
        (text + 28.).max(92.)
    }

    fn layout(handoff: &NSButton, view: &NSView, title: &str) {
        let width = handoff_width(title);
        handoff.setFrame(NSRect::new(NSPoint::new(100., 2.), NSSize::new(width, 24.)));
        view.setFrame(NSRect::new(
            NSPoint::new(0., 0.),
            NSSize::new(106. + width, 28.),
        ));
    }

    // NSWindow retains the accessory controller. The button's weak target is
    // this same controller, so no separate target or global registry survives
    // a closed window; its Rust ivars are released with the native controller.
    define_class!(
        #[unsafe(super(NSTitlebarAccessoryViewController))]
        #[thread_kind = MainThreadOnly]
        #[ivars = Action]
        struct HandoffAccessory;

        unsafe impl NSObjectProtocol for HandoffAccessory {}

        impl HandoffAccessory {
            #[unsafe(method(refreshLanguage))]
            fn refresh_language(&self) {
                if let Some((reload, handoff, view)) = self.ivars().buttons.get() {
                    reload.setTitle(&NSString::from_str(crate::ui_locale::text(RELOAD_TITLE)));
                    let title = handoff_title();
                    handoff.setTitle(&NSString::from_str(&title));
                    layout(handoff, view, &title);
                }
            }

            #[unsafe(method(requestHandoff:))]
            fn request_handoff(&self, _sender: Option<&NSButton>) {
                let action = self.ivars();
                if let Some(name) = action_event(HANDOFF_ITEM) {
                    let _ = action.app.emit_to("manager", name, action.request.clone());
                }
            }

            #[unsafe(method(requestReload:))]
            fn request_reload(&self, _sender: Option<&NSButton>) {
                let action = self.ivars();
                // The manager reloads a web page or restarts a Node app; it
                // re-resolves the registered app, so a stale window does nothing.
                if let Some(name) = action_event(RELOAD_ITEM) {
                    let _ = action.app.emit_to("manager", name, action.request.clone());
                }
            }
        }
    );

    pub(super) fn install(
        window: &tauri::WebviewWindow,
        request: HandoffRequest,
    ) -> Result<(), platform_core::ApiError> {
        let fail =
            || platform_core::ApiError::new("AI_TITLEBAR_UNAVAILABLE", "无法创建应用标题栏按钮。");
        let marker = MainThreadMarker::new().ok_or_else(fail)?;
        let native = window.ns_window().map_err(|_| fail())?;
        let native = unsafe { native.cast::<NSWindow>().as_ref() }.ok_or_else(fail)?;
        let allocated = HandoffAccessory::alloc(marker).set_ivars(Action {
            app: window.app_handle().clone(),
            request,
            buttons: std::cell::OnceCell::new(),
        });
        let accessory: Retained<HandoffAccessory> = unsafe { msg_send![super(allocated), init] };
        let title = handoff_title();
        let view = NSView::initWithFrame(
            NSView::alloc(marker),
            NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(106. + handoff_width(&title), 28.),
            ),
        );
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&title),
                Some(&accessory),
                Some(sel!(requestHandoff:)),
                marker,
            )
        };
        button.setBezelStyle(NSBezelStyle::Push);
        button.setControlSize(NSControlSize::Small);
        button.setFrame(NSRect::new(
            NSPoint::new(100., 2.),
            NSSize::new(handoff_width(&title), 24.),
        ));
        let reload = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(crate::ui_locale::text(RELOAD_TITLE)),
                Some(&accessory),
                Some(sel!(requestReload:)),
                marker,
            )
        };
        reload.setBezelStyle(NSBezelStyle::Push);
        reload.setControlSize(NSControlSize::Small);
        reload.setFrame(NSRect::new(NSPoint::new(4., 2.), NSSize::new(92., 24.)));
        view.addSubview(&reload);
        view.addSubview(&button);
        accessory.setView(&view);
        let _ = accessory.ivars().buttons.set((reload, button, view));
        accessory.setLayoutAttribute(NSLayoutAttribute::Right);
        native.addTitlebarAccessoryViewController(&accessory);
        Ok(())
    }

    pub(super) fn refresh_language(window: &tauri::WebviewWindow) {
        let Ok(native) = window.ns_window() else {
            return;
        };
        let Some(native) = (unsafe { native.cast::<NSWindow>().as_ref() }) else {
            return;
        };
        for accessory in native.titlebarAccessoryViewControllers().iter() {
            if accessory.isKindOfClass(HandoffAccessory::class()) {
                let _: () = unsafe { msg_send![&*accessory, refreshLanguage] };
            }
        }
    }
}
