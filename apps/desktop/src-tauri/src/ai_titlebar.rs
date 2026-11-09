//! Native titlebar actions; business pages never receive management access.
//! Both buttons only ask the manager window to act on the registered app.
use serde::Serialize;

const BUTTON_TITLE: &str = "按你的想法改";
const RELOAD_TITLE: &str = "重新加载";

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
    if let Err(error) = macos::install(window, request) {
        // The application's existing window must remain usable if the OS
        // cannot attach this optional management action.
        eprintln!("AI titlebar action unavailable: {error}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, request, BUTTON_TITLE, RELOAD_TITLE);
}

pub(super) fn refresh_language(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let target = window.clone();
        let _ = window.run_on_main_thread(move || macos::refresh_language(&target));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
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
        buttons: std::cell::OnceCell<(Retained<NSButton>, Retained<NSButton>)>,
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
                if let Some((reload, handoff)) = self.ivars().buttons.get() {
                    reload.setTitle(&NSString::from_str(crate::ui_locale::text(RELOAD_TITLE)));
                    handoff.setTitle(&NSString::from_str(crate::ui_locale::text(BUTTON_TITLE)));
                }
            }

            #[unsafe(method(requestHandoff:))]
            fn request_handoff(&self, _sender: Option<&NSButton>) {
                let action = self.ivars();
                let _ = action.app.emit_to("manager", "ai-handoff-request", action.request.clone());
            }

            #[unsafe(method(requestReload:))]
            fn request_reload(&self, _sender: Option<&NSButton>) {
                let action = self.ivars();
                // The manager reloads a web page or restarts a Node app; it
                // re-resolves the registered app, so a stale window does nothing.
                let _ = action.app.emit_to("manager", "app-reload-request", action.request.clone());
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
        let view = NSView::initWithFrame(
            NSView::alloc(marker),
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(238., 28.)),
        );
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(crate::ui_locale::text(BUTTON_TITLE)),
                Some(&accessory),
                Some(sel!(requestHandoff:)),
                marker,
            )
        };
        button.setBezelStyle(NSBezelStyle::Push);
        button.setControlSize(NSControlSize::Small);
        button.setFrame(NSRect::new(NSPoint::new(100., 2.), NSSize::new(132., 24.)));
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
        let _ = accessory.ivars().buttons.set((reload, button));
        accessory.setView(&view);
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
