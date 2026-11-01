//! Extend only the business view's existing delegate. Inherited navigation,
//! permission and file-picker methods remain owned by the browser adapter.
use block2::DynBlock;
use objc2::{
    msg_send,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel, MainThreadOnly,
};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSTextField};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use platform_core::ApiError;

fn panel(message: &NSString, cancel: bool) -> objc2::rc::Retained<NSAlert> {
    let marker = MainThreadMarker::new().expect("WebKit dialogs execute on the main thread");
    let alert = NSAlert::new(marker);
    alert.setMessageText(message);
    alert.addButtonWithTitle(&NSString::from_str("确定"));
    if cancel {
        alert.addButtonWithTitle(&NSString::from_str("取消"));
    }
    alert
}
extern "C" fn alert(
    _this: &AnyObject,
    _sel: Sel,
    _view: &AnyObject,
    message: &NSString,
    _frame: &AnyObject,
    complete: &DynBlock<dyn Fn()>,
) {
    panel(message, false).runModal();
    complete.call(());
}
extern "C" fn confirm(
    _this: &AnyObject,
    _sel: Sel,
    _view: &AnyObject,
    message: &NSString,
    _frame: &AnyObject,
    complete: &DynBlock<dyn Fn(Bool)>,
) {
    complete.call((Bool::new(
        panel(message, true).runModal() == NSAlertFirstButtonReturn,
    ),));
}
extern "C" fn prompt(
    _this: &AnyObject,
    _sel: Sel,
    _view: &AnyObject,
    message: &NSString,
    default: Option<&NSString>,
    _frame: &AnyObject,
    complete: &DynBlock<dyn Fn(*mut NSString)>,
) {
    let marker = MainThreadMarker::new().expect("WebKit dialogs execute on the main thread");
    let alert = panel(message, true);
    let input = NSTextField::initWithFrame(
        NSTextField::alloc(marker),
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(320., 24.)),
    );
    if let Some(default) = default {
        input.setStringValue(default);
    }
    alert.setAccessoryView(Some(&input));
    if alert.runModal() == NSAlertFirstButtonReturn {
        let text = input.stringValue();
        complete.call((objc2::rc::Retained::as_ptr(&text).cast_mut(),));
    } else {
        complete.call((std::ptr::null_mut(),));
    }
}

pub(super) unsafe fn install(view: *mut std::ffi::c_void) -> Result<(), ApiError> {
    let fail = || ApiError::new("WEB_DIALOGS_UNAVAILABLE", "无法启用应用的系统对话框。");
    let view = unsafe { view.cast::<AnyObject>().as_ref() }.ok_or_else(fail)?;
    let delegate: *mut AnyObject = unsafe { msg_send![view, UIDelegate] };
    let delegate = unsafe { delegate.as_ref() }.ok_or_else(fail)?;
    let parent = delegate.class();
    if parent
        .name()
        .to_bytes()
        .starts_with(b"PandaBusinessDialogs_")
    {
        return Ok(());
    }
    let name = std::ffi::CString::new(format!(
        "PandaBusinessDialogs_{:x}",
        parent as *const AnyClass as usize
    ))
    .unwrap();
    let class = if let Some(class) = AnyClass::get(&name) {
        class
    } else {
        let mut builder = ClassBuilder::new(&name, parent).ok_or_else(fail)?;
        // Exact WKUIDelegate ABIs; the subclass adds no ivars and inherits all
        // other behavior, so changing this one main-thread instance is valid.
        unsafe {
            builder.add_method(sel!(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:), alert as extern "C" fn(_, _, _, _, _, _));
            builder.add_method(sel!(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:), confirm as extern "C" fn(_, _, _, _, _, _));
            builder.add_method(sel!(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:), prompt as extern "C" fn(_, _, _, _, _, _, _));
        }
        builder.register()
    };
    unsafe {
        AnyObject::set_class(delegate, class);
    }
    Ok(())
}
