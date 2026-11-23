use crate::ui_locale;
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

pub(super) fn install_application_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    // macOS's predefined Quit calls NSApplication.terminate: directly, which
    // bypasses ExitRequested. Our ordinary menu item uses app.exit instead.
    let quit = MenuItem::with_id(app, "quit", super::quit_label(), true, Some("CmdOrCtrl+Q"))?;
    let application = Submenu::with_items(
        app,
        ui_locale::text("熊猫小抽屉"),
        true,
        &[
            &PredefinedMenuItem::about(
                app,
                Some(ui_locale::text("关于熊猫小抽屉")),
                Some(AboutMetadata {
                    name: Some(ui_locale::text("熊猫小抽屉").into()),
                    version: Some(app.package_info().version.to_string()),
                    icon: app.default_window_icon().cloned(),
                    ..Default::default()
                }),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(ui_locale::text("隐藏熊猫小抽屉")))?,
            &PredefinedMenuItem::hide_others(app, Some(ui_locale::text("隐藏其他")))?,
            &PredefinedMenuItem::show_all(app, Some(ui_locale::text("显示全部")))?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        ui_locale::text("编辑"),
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(ui_locale::text("撤销")))?,
            &PredefinedMenuItem::redo(app, Some(ui_locale::text("重做")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some(ui_locale::text("剪切")))?,
            &PredefinedMenuItem::copy(app, Some(ui_locale::text("复制")))?,
            &PredefinedMenuItem::paste(app, Some(ui_locale::text("粘贴")))?,
            &PredefinedMenuItem::select_all(app, Some(ui_locale::text("全选")))?,
        ],
    )?;
    let project_manager = MenuItem::with_id(
        app,
        "show-manager",
        ui_locale::text("应用管理"),
        true,
        Some("CmdOrCtrl+Shift+H"),
    )?;
    let window = Submenu::with_items(
        app,
        ui_locale::text("窗口"),
        true,
        &[
            &project_manager,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::minimize(app, Some(ui_locale::text("最小化")))?,
            &PredefinedMenuItem::maximize(app, Some(ui_locale::text("缩放")))?,
            &PredefinedMenuItem::close_window(app, Some(ui_locale::text("关闭窗口")))?,
        ],
    )?;
    app.set_menu(Menu::with_items(app, &[&application, &edit, &window])?)?;
    Ok(())
}
