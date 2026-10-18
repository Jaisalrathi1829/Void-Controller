//! The desktop widget: a small window of its own with the modes and the
//! equalizer, so that the main window need not be opened to change them.
//!
//! It is a second window of this application, not a second application. That
//! matters: the headphones accept one control connection from a computer, and
//! the widget uses the one the application already has. It talks to the
//! native layer through exactly the same typed commands as the main window,
//! and can do nothing the main window cannot.
//!
//! The window exists only while the setting is on. It is frameless, kept off
//! the taskbar, optionally kept above other windows, and put back where it
//! was last left — provided that place is still on a screen.
//!
//! One rule is kept throughout: there is always a way back to the
//! application. Closing the main window while the widget is up hides the main
//! window and leaves the widget; closing the widget while the main window is
//! hidden and there is no tray icon brings the main window back.

use crate::settings::SettingKey;
use crate::{tray, AppState, SettingsView, SETTINGS_CHANGED_EVENT};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

pub const WIDGET_WINDOW: &str = "widget";

/// The window's size, in the units the interface is laid out in. The card
/// inside it leaves a margin for its own shadow.
const WIDTH: f64 = 316.0;
const HEIGHT: f64 = 446.0;

/// How far from the corner of the screen the widget is first put.
const MARGIN: i32 = 24;

/// How long the widget has to stay still before its position is written
/// down. A drag reports a position for every pixel it passes through.
const SETTLE: Duration = Duration::from_millis(600);

/// A screen's usable area — without the taskbar — in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Area {
    fn contains(&self, x: i32, y: i32) -> bool {
        let right = self.x.saturating_add_unsigned(self.width);
        let bottom = self.y.saturating_add_unsigned(self.height);
        x >= self.x && x < right && y >= self.y && y < bottom
    }
}

/// Where to put a widget of `size`, given where it was last left and the
/// screens there are now. The first screen is the primary one.
///
/// The remembered place is used if the widget could still be got hold of
/// there: the middle of its top edge, which is what it is dragged by, has to
/// be on a screen. A laptop taken off its second monitor would otherwise open
/// the widget somewhere that no longer exists. Failing that it goes to the
/// bottom right of the primary screen, clear of the taskbar. With no screens
/// reported at all there is nothing to go by, and the system decides.
pub fn place(saved: Option<(i32, i32)>, size: (u32, u32), screens: &[Area]) -> Option<(i32, i32)> {
    let (width, height) = (size.0 as i32, size.1 as i32);
    if let Some((x, y)) = saved {
        let handle = (x.saturating_add(width / 2), y.saturating_add(20));
        if screens.iter().any(|s| s.contains(handle.0, handle.1)) {
            return Some((x, y));
        }
    }
    let primary = screens.first()?;
    let right = primary.x.saturating_add_unsigned(primary.width);
    let bottom = primary.y.saturating_add_unsigned(primary.height);
    // Never above or left of the screen's own corner, however small it is.
    Some((
        (right - width - MARGIN).max(primary.x),
        (bottom - height - MARGIN).max(primary.y),
    ))
}

fn screens(app: &AppHandle) -> Vec<Area> {
    let area = |m: &tauri::Monitor| {
        let work = m.work_area();
        Area {
            x: work.position.x,
            y: work.position.y,
            width: work.size.width,
            height: work.size.height,
        }
    };
    let mut all = Vec::new();
    if let Ok(Some(primary)) = app.primary_monitor() {
        all.push(area(&primary));
    }
    for monitor in app.available_monitors().unwrap_or_default() {
        let candidate = area(&monitor);
        if !all.contains(&candidate) {
            all.push(candidate);
        }
    }
    all
}

pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(WIDGET_WINDOW).is_some()
}

/// Opens the widget, or brings it forward if it is already there.
///
/// The window is made on a thread of its own. On Windows, making one from
/// inside a command or an event handler — which is where every caller of
/// this is — waits forever on the very thread that would have to make it.
pub fn open(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WIDGET_WINDOW) {
        let _ = window.show();
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(why) = build(&app) {
            // The switch must not go on saying there is a widget.
            eprintln!("the widget could not be opened: {why}");
            record(&app, false);
            keep_reachable(&app);
        }
    });
}

fn build(app: &AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "the application has not finished starting".to_string())?;
    let settings = state.settings().get();

    let window = WebviewWindowBuilder::new(app, WIDGET_WINDOW, WebviewUrl::App("index.html".into()))
        .title("Void — widget")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        // The card draws its own shadow; the system's would outline the
        // transparent rectangle around it.
        .shadow(false)
        .skip_taskbar(true)
        .always_on_top(settings.widget_on_top)
        // Shown once it is where it belongs, not before.
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;

    let size = window
        .outer_size()
        .map(|s| (s.width, s.height))
        .unwrap_or((WIDTH as u32, HEIGHT as u32));
    if let Some((x, y)) = place(settings.widget_position, size, &screens(app)) {
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    window.show().map_err(|e| e.to_string())
}

/// Closes the widget, and makes sure that does not leave the application
/// running with nothing to reach it by.
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WIDGET_WINDOW) {
        // From a thread of its own, so that it happens after whatever asked
        // for it has returned. The usual asker is the widget's own close
        // button, and a window is not something to tear down from inside a
        // request it is still waiting on the answer to.
        //
        // And `destroy`, not `close`: `close` asks the window, and the
        // answer to that question is handled as the user closing it.
        std::thread::spawn(move || {
            let _ = window.destroy();
        });
    }
    keep_reachable(app);
}

/// Makes the window match the setting.
pub fn sync(app: &AppHandle, enabled: bool) {
    if enabled {
        open(app);
    } else {
        close(app);
    }
}

pub fn set_on_top(app: &AppHandle, on_top: bool) {
    if let Some(window) = app.get_webview_window(WIDGET_WINDOW) {
        let _ = window.set_always_on_top(on_top);
    }
}

/// Flips the widget on or off: the tray menu's entry.
pub fn toggle(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let enabled = !state.settings().get().desktop_widget;
    record(app, enabled);
    sync(app, enabled);
}

/// The widget was closed by something other than the setting — Alt+F4, say.
/// The setting follows, and the application stays reachable. The window is
/// left to finish closing by itself.
pub fn closed_by_user(app: &AppHandle) {
    record(app, false);
    keep_reachable(app);
}

/// Writes the switch down and tells every window.
fn record(app: &AppHandle, enabled: bool) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if let Ok(after) = state
        .settings()
        .update(|s| s.set(SettingKey::DesktopWidget, enabled))
    {
        let _ = app.emit(SETTINGS_CHANGED_EVENT, SettingsView::build(&after));
    }
}

/// If the main window is hidden and there is no tray icon, the widget was
/// the only way back to it. Show it.
fn keep_reachable(app: &AppHandle) {
    if tray::is_present(app) {
        return;
    }
    let hidden = app
        .get_webview_window(tray::MAIN_WINDOW)
        .is_some_and(|window| !window.is_visible().unwrap_or(true));
    if hidden {
        tray::show_main(app);
    }
}

static LATEST: Mutex<Option<(i32, i32)>> = Mutex::new(None);
static SAVING: AtomicBool = AtomicBool::new(false);

/// The widget is at a new position. Remembered once it has stopped moving.
pub fn moved(app: &AppHandle, x: i32, y: i32) {
    // Windows parks a minimized window far off screen. Not a place.
    if x <= -30_000 || y <= -30_000 {
        return;
    }
    *LATEST.lock() = Some((x, y));
    if SAVING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut seen = None;
        loop {
            std::thread::sleep(SETTLE);
            let now = *LATEST.lock();
            if now == seen {
                break;
            }
            seen = now;
        }
        SAVING.store(false, Ordering::SeqCst);
        if let (Some(position), Some(state)) = (seen, app.try_state::<AppState>()) {
            if state.settings().get().widget_position != Some(position) {
                let _ = state.settings().update(|s| s.widget_position = Some(position));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920×1080 screen with a 48-pixel taskbar along the bottom.
    const LAPTOP: Area = Area { x: 0, y: 0, width: 1920, height: 1032 };
    /// A second screen to its left.
    const SECOND: Area = Area { x: -1920, y: 0, width: 1920, height: 1080 };
    const SIZE: (u32, u32) = (316, 352);

    #[test]
    fn the_first_time_it_goes_to_the_bottom_right_clear_of_the_taskbar() {
        let (x, y) = place(None, SIZE, &[LAPTOP]).unwrap();
        assert_eq!(x, 1920 - 316 - MARGIN);
        assert_eq!(y, 1032 - 352 - MARGIN);
    }

    #[test]
    fn it_comes_back_where_it_was_left() {
        assert_eq!(place(Some((100, 200)), SIZE, &[LAPTOP]), Some((100, 200)));
        // On the second screen, whose coordinates are negative.
        assert_eq!(
            place(Some((-1500, 300)), SIZE, &[LAPTOP, SECOND]),
            Some((-1500, 300))
        );
    }

    /// Left on a monitor that has since been unplugged: it must not open
    /// somewhere it cannot be seen or dragged back from.
    #[test]
    fn a_place_that_is_no_longer_on_any_screen_is_not_used() {
        let fallback = place(None, SIZE, &[LAPTOP]);
        assert_eq!(place(Some((-1500, 300)), SIZE, &[LAPTOP]), fallback);
        assert_eq!(place(Some((5000, 5000)), SIZE, &[LAPTOP]), fallback);
        // Dragged almost entirely off the top: the part it is dragged by
        // is out of reach.
        assert_eq!(place(Some((400, -200)), SIZE, &[LAPTOP]), fallback);
    }

    /// Partly off an edge is fine, as long as its top edge can be reached.
    #[test]
    fn hanging_off_an_edge_is_allowed_while_it_can_still_be_dragged() {
        assert_eq!(place(Some((1700, 900)), SIZE, &[LAPTOP]), Some((1700, 900)));
        assert_eq!(place(Some((-100, 10)), SIZE, &[LAPTOP]), Some((-100, 10)));
    }

    #[test]
    fn with_no_screens_reported_the_system_decides() {
        assert_eq!(place(Some((100, 100)), SIZE, &[]), None);
        assert_eq!(place(None, SIZE, &[]), None);
    }

    #[test]
    fn on_a_screen_smaller_than_the_widget_it_starts_at_the_corner() {
        let tiny = Area { x: 0, y: 0, width: 200, height: 200 };
        assert_eq!(place(None, SIZE, &[tiny]), Some((0, 0)));
    }

    #[test]
    fn extreme_coordinates_do_not_overflow() {
        let _ = place(Some((i32::MAX, i32::MAX)), SIZE, &[LAPTOP]);
        let _ = place(Some((i32::MIN, i32::MIN)), SIZE, &[LAPTOP]);
        let huge = Area { x: i32::MAX - 10, y: 0, width: u32::MAX, height: 100 };
        let _ = place(None, SIZE, &[huge]);
    }
}
