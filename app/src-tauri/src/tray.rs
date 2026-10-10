//! The system-tray icon, and showing and hiding the main window.
//!
//! The icon exists exactly while "minimize to tray" is on. That is the only
//! time the window can be hidden, and a hidden window with no icon to bring it
//! back would be an application the user cannot reach.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

pub const TRAY_ID: &str = "main";
pub const MAIN_WINDOW: &str = "main";

const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";

/// Brings the main window back: un-minimized, visible, in front.
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Whether the tray icon is there to bring a hidden window back.
pub fn is_present(app: &AppHandle) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

/// Creates the tray icon or removes it, to match the setting.
pub fn sync(app: &AppHandle, enabled: bool) -> Result<(), String> {
    if !enabled {
        let _ = app.remove_tray_by_id(TRAY_ID);
        return Ok(());
    }
    if is_present(app) {
        return Ok(());
    }

    let failed = |e: tauri::Error| format!("could not create the tray icon: {e}");
    let open = MenuItem::with_id(app, MENU_OPEN, "Open Bose QC Control Center", true, None::<&str>)
        .map_err(failed)?;
    let separator = PredefinedMenuItem::separator(app).map_err(failed)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>).map_err(failed)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit]).map_err(failed)?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Bose QC Control Center")
        .menu(&menu)
        // Left click opens the window; the menu is on right click.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            MENU_OPEN => show_main(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map(|_| ()).map_err(failed)
}
