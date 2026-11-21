use crate::ui_locale;
use tauri::{
    menu::{Menu, MenuEvent, MenuItem},
    tray::TrayIconBuilder,
    AppHandle,
};

#[cfg(target_os = "macos")]
mod macos;

pub(super) fn install_application_menu(app: &AppHandle) -> tauri::Result<()> {
    // macOS has a system menu bar. On Windows, an application menu would
    // instead occupy a row in every window, where WebView2 handles editing.
    #[cfg(target_os = "macos")]
    macos::install_application_menu(app)?;
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

const TRAY_ID: &str = "panda-main";

fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(
        app,
        "show",
        if ui_locale::windows_production() {
            ui_locale::text("打开熊猫小抽屉")
        } else {
            ui_locale::text("打开熊猫小抽屉（开发版）")
        },
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        if ui_locale::windows_production() {
            ui_locale::text("退出")
        } else {
            ui_locale::text("退出开发版")
        },
        true,
        None::<&str>,
    )?;
    Menu::with_items(app, &[&show, &quit])
}

pub(super) fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(ui_locale::application_title())
        .menu(&tray_menu(app)?);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

pub(super) fn refresh_tray(app: &AppHandle) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(tray_menu(app)?))?;
        tray.set_tooltip(Some(ui_locale::application_title()))?;
    }
    Ok(())
}

pub(super) fn handle_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        "show" | "show-manager" => super::show_manager(app),
        "quit" => app.exit(0),
        _ => {}
    }
}
