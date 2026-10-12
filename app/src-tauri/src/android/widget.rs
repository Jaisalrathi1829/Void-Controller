//! The home-screen widget's native half.
//!
//! The widget itself is Kotlin (`ModeWidget.kt`): three buttons on the home
//! screen. It decides nothing. A tap on it is a request for a mode, by name,
//! and this is where the request is acted on — through the same verified
//! switch the application's own buttons use — and where what the widget shows
//! comes from: the mode the headphones report, never the one asked for.
//!
//! Two directions, both over the bridge this crate already has:
//!
//! * **Requests in.** One thread asks Kotlin for the next request and waits
//!   until there is one. That it is asking is also how the widget knows the
//!   native layer is alive; when the application is not running, nothing
//!   asks, and the widget says so rather than showing a stale mode.
//! * **State out.** Whenever the headphones are seen to connect, go, or
//!   change mode, the widget is told — if what it would show has changed.
//!
//! A request carries a mode's name and nothing else. It is read into the same
//! closed type the interface's requests are, so the widget can ask for
//! Quiet, Aware or the custom mode, and for nothing besides.

use crate::device::state::{DeviceSource, NoiseControlMode, NoiseControlState};
use crate::NoiseModeRequest;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64};

/// What the widget is given to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetState {
    /// `connected`, `disconnected`, or `simulated` when the application is
    /// on its simulated device — whose modes are not the headphones'.
    pub status: &'static str,
    /// What the headphones are called.
    pub name: Option<String>,
    /// The mode the headphones last reported: `quiet`, `aware` or `custom`.
    /// `None` when they have not reported one.
    pub mode: Option<&'static str>,
    /// The headphones' own name for their custom mode, if they have one.
    pub custom_name: Option<String>,
    /// A few words about the last request, when it did not simply succeed.
    pub note: Option<String>,
}

/// Builds what the widget shows from what is known.
///
/// A mode is passed on only for real, connected headphones. The simulated
/// device has modes too; a home-screen widget showing one as the headphones'
/// would be the simulation passing itself off as real.
pub fn state_from(
    source: DeviceSource,
    connected: bool,
    name: Option<String>,
    reported: Option<&NoiseControlState>,
    custom_modes: &[String],
    note: Option<String>,
) -> WidgetState {
    if source != DeviceSource::Real {
        return WidgetState {
            status: "simulated",
            name: None,
            mode: None,
            custom_name: None,
            note: None,
        };
    }
    let live = connected.then_some(());
    let mode = live.and(reported).and_then(|state| match state.mode {
        NoiseControlMode::Quiet => Some("quiet"),
        NoiseControlMode::Aware => Some("aware"),
        NoiseControlMode::Custom => Some("custom"),
        NoiseControlMode::Off => None,
    });
    // The custom mode's name: the one they are in, if they are in one, and
    // otherwise the one they listed.
    let active_custom = reported
        .filter(|state| state.mode == NoiseControlMode::Custom)
        .and_then(|state| state.label.clone());
    let custom_name = live
        .and(active_custom.or_else(|| custom_modes.first().cloned()))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    WidgetState {
        status: if connected { "connected" } else { "disconnected" },
        name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        mode,
        custom_name,
        note: live.and(note),
    }
}

/// A request from the widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Switch to this mode.
    Mode(NoiseModeRequest),
    /// Say where things stand.
    Refresh,
}

/// Reads what Kotlin handed over. Anything that is not one of the three
/// modes, or a plain request for the state, is nothing.
pub fn parse_request(kind: &str, mode: Option<&str>) -> Option<Request> {
    match kind {
        "refresh" => Some(Request::Refresh),
        "mode" => {
            let name = serde_json::Value::String(mode?.to_string());
            serde_json::from_value::<NoiseModeRequest>(name)
                .ok()
                .map(Request::Mode)
        }
        _ => None,
    }
}

/// The few words shown when a request did not simply succeed. `None` for a
/// change the headphones confirmed.
pub fn note_for(outcome: &Result<crate::device::command::CommandOutcome, String>) -> Option<String> {
    use crate::device::command::CommandOutcome;
    match outcome {
        Ok(outcome) if outcome.is_confirmed() => None,
        Ok(CommandOutcome::Rejected { .. }) => Some("Refused by the headphones".to_string()),
        Ok(CommandOutcome::Unsupported { .. }) => Some("Not available".to_string()),
        Ok(_) => Some("Not confirmed".to_string()),
        Err(_) => Some("Could not reach them".to_string()),
    }
}

/// What the widget was last told, so that it is told again only when
/// something it shows has changed.
static SHOWN: Mutex<Option<WidgetState>> = Mutex::new(None);

/// Whether the headphones were connected, and what they were called, the
/// last time the watcher looked. The watcher looks every few seconds; a mode
/// change is reported in between, and uses this.
static SEEN: Mutex<(bool, Option<String>)> = Mutex::new((false, None));

/// Each start of the listener has a number; one that finds a later number
/// has been replaced and stops.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Tells the widget, if what it shows would change. Must not be called from
/// the thread that runs the event loop.
fn publish(state: WidgetState, force: bool) {
    let mut shown = SHOWN.lock();
    if !force && shown.as_ref() == Some(&state) {
        return;
    }
    if super::bridge::run("widgetState", &state).is_ok() {
        *shown = Some(state);
    }
}

#[cfg(target_os = "android")]
mod live {
    use super::*;
    use crate::bose::real::Sighting;
    use crate::AppState;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tauri::{AppHandle, Emitter, Manager};

    /// How long one question to Kotlin waits for a request before it is
    /// asked again. Each asking is also the sign of life the widget goes by.
    const WAIT: Duration = Duration::from_secs(50);

    #[derive(serde::Deserialize)]
    struct Next {
        kind: String,
        #[serde(default)]
        mode: Option<String>,
    }

    fn current(app: &AppHandle, note: Option<String>) -> Option<WidgetState> {
        let state = app.try_state::<AppState>()?;
        let device = state.current();
        let (connected, name) = SEEN.lock().clone();
        let name = name.or_else(|| state.settings().get().preferred_device_name);
        let reported = device.reported_noise_control();
        Some(state_from(
            device.source(),
            // Having a mode in hand from them is being connected to them,
            // whatever the watcher last saw.
            connected || reported.is_some(),
            name,
            reported.as_ref(),
            &device.known_custom_modes(),
            note,
        ))
    }

    /// What the watcher just saw of the headphones.
    pub fn seen(app: &AppHandle, sighting: Option<&Sighting>) {
        *SEEN.lock() = match sighting {
            Some(seen) => (seen.connected, Some(seen.name.clone())),
            None => (false, None),
        };
        if let Some(state) = current(app, None) {
            publish(state, false);
        }
    }

    /// The mode has changed, or may have.
    pub fn mode_changed(app: &AppHandle) {
        if let Some(state) = current(app, None) {
            publish(state, false);
        }
    }

    async fn switch(app: &AppHandle, mode: NoiseModeRequest) {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let device = state.current();
        let note = if device.source() == DeviceSource::Real {
            let outcome = device
                .set_noise_control(mode.into())
                .await
                .map_err(|e| e.to_string());
            // What the headphones now report is what is announced, and what
            // any open window should show.
            crate::observe_reported_mode(&state, device.as_ref());
            let _ = app.emit(crate::watcher::DEVICE_CHANGED_EVENT, ());
            note_for(&outcome)
        } else {
            None
        };
        // Always said, even if nothing changed: the widget is showing
        // "switching" and has to be told the attempt is over.
        if let Some(state) = current(app, note) {
            publish(state, true);
        }
    }

    /// Starts taking the widget's requests. Runs for the life of the
    /// application.
    pub fn spawn(app: AppHandle) {
        STOPPING.store(false, Ordering::SeqCst);
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::Builder::new()
            .name("widget-requests".into())
            .spawn(move || {
                let mut failures = 0u32;
                // Say where things stand as soon as there is anything to say.
                if let Some(state) = current(&app, None) {
                    publish(state, true);
                }
                loop {
                    if STOPPING.load(Ordering::SeqCst) || GENERATION.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    let asked = super::super::bridge::call::<Next>(
                        "widgetNext",
                        serde_json::json!({ "waitMs": WAIT.as_millis() as u64 }),
                    );
                    let next = match asked {
                        Ok(next) => {
                            failures = 0;
                            next
                        }
                        // The Kotlin side is not answering. Not something to
                        // spin on; and if it stays that way, there is nobody
                        // to take requests from.
                        Err(_) => {
                            failures += 1;
                            if failures >= 5 {
                                return;
                            }
                            std::thread::sleep(Duration::from_secs(2));
                            continue;
                        }
                    };
                    match parse_request(&next.kind, next.mode.as_deref()) {
                        Some(Request::Mode(mode)) => {
                            tauri::async_runtime::block_on(switch(&app, mode));
                        }
                        Some(Request::Refresh) => {
                            if let Some(state) = current(&app, None) {
                                publish(state, true);
                            }
                        }
                        None => {}
                    }
                }
            })
            .expect("the widget's request thread can be started");
    }

    /// The application is going: stop asking.
    pub fn stop() {
        STOPPING.store(true, Ordering::SeqCst);
    }
}

#[cfg(target_os = "android")]
pub use live::{mode_changed, seen, spawn, stop};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::command::CommandOutcome;
    use crate::device::state::StateSource;

    fn reported(mode: NoiseControlMode, label: Option<&str>) -> NoiseControlState {
        NoiseControlState {
            mode,
            level: None,
            label: label.map(str::to_string),
            source: StateSource::VendorProtocol,
            read_at: "2026-10-12T19:00:00Z".into(),
        }
    }

    fn real(connected: bool, mode: Option<NoiseControlState>, customs: &[&str]) -> WidgetState {
        let customs: Vec<String> = customs.iter().map(|s| s.to_string()).collect();
        state_from(
            DeviceSource::Real,
            connected,
            Some("Aurora".into()),
            mode.as_ref(),
            &customs,
            None,
        )
    }

    #[test]
    fn connected_headphones_show_the_mode_they_reported() {
        let state = real(true, Some(reported(NoiseControlMode::Quiet, None)), &["Focus"]);
        assert_eq!(state.status, "connected");
        assert_eq!(state.name.as_deref(), Some("Aurora"));
        assert_eq!(state.mode, Some("quiet"));
        assert_eq!(state.custom_name.as_deref(), Some("Focus"));

        let state = real(true, Some(reported(NoiseControlMode::Aware, None)), &[]);
        assert_eq!(state.mode, Some("aware"));
        assert_eq!(state.custom_name, None, "no custom mode has been listed");
    }

    #[test]
    fn the_custom_mode_goes_by_the_headphones_name_for_it() {
        let state = real(true, Some(reported(NoiseControlMode::Custom, Some("Focus"))), &[]);
        assert_eq!(state.mode, Some("custom"));
        assert_eq!(state.custom_name.as_deref(), Some("Focus"));

        // The one they are in wins over the one that was listed first.
        let state = real(
            true,
            Some(reported(NoiseControlMode::Custom, Some("Commute"))),
            &["Focus", "Commute"],
        );
        assert_eq!(state.custom_name.as_deref(), Some("Commute"));
    }

    /// No report is no mode: nothing is highlighted on the strength of what
    /// was last asked for, or of a default.
    #[test]
    fn with_no_report_no_mode_is_shown() {
        let state = real(true, None, &["Focus"]);
        assert_eq!(state.status, "connected");
        assert_eq!(state.mode, None);

        let off = real(true, Some(reported(NoiseControlMode::Off, None)), &[]);
        assert_eq!(off.mode, None, "not a mode the widget has a button for");
    }

    /// Headphones that have gone are in no mode as far as this goes, whatever
    /// they last reported.
    #[test]
    fn disconnected_headphones_show_no_mode() {
        let state = real(false, Some(reported(NoiseControlMode::Quiet, None)), &["Focus"]);
        assert_eq!(state.status, "disconnected");
        assert_eq!(state.mode, None);
        assert_eq!(state.custom_name, None);
        assert_eq!(state.name.as_deref(), Some("Aurora"), "still called what they are called");
    }

    /// The simulated device's modes are not the headphones'.
    #[test]
    fn the_simulated_device_is_never_shown_as_the_headphones() {
        let state = state_from(
            DeviceSource::Mock,
            true,
            Some("Bose QuietComfort (SIMULATED)".into()),
            Some(&reported(NoiseControlMode::Quiet, None)),
            &["Focus".to_string()],
            Some("Not confirmed".into()),
        );
        assert_eq!(state.status, "simulated");
        assert_eq!((state.mode, state.name, state.custom_name, state.note), (None, None, None, None));
    }

    #[test]
    fn a_note_is_only_shown_about_connected_headphones() {
        let note = Some("Not confirmed".to_string());
        let on = state_from(DeviceSource::Real, true, None, None, &[], note.clone());
        assert_eq!(on.note.as_deref(), Some("Not confirmed"));
        let off = state_from(DeviceSource::Real, false, None, None, &[], note);
        assert_eq!(off.note, None);
    }

    #[test]
    fn blank_names_are_no_names() {
        let state = state_from(
            DeviceSource::Real,
            true,
            Some("  ".into()),
            None,
            &["   ".to_string()],
            None,
        );
        assert_eq!(state.name, None);
        assert_eq!(state.custom_name, None);
    }

    // --- requests -----------------------------------------------------------

    #[test]
    fn the_widget_can_ask_for_exactly_three_modes() {
        assert_eq!(
            parse_request("mode", Some("quiet")),
            Some(Request::Mode(NoiseModeRequest::Quiet))
        );
        assert_eq!(
            parse_request("mode", Some("aware")),
            Some(Request::Mode(NoiseModeRequest::Aware))
        );
        assert_eq!(
            parse_request("mode", Some("custom")),
            Some(Request::Mode(NoiseModeRequest::Custom))
        );
        assert_eq!(parse_request("refresh", None), Some(Request::Refresh));
    }

    /// Nothing else gets through: not "off", not an index, not bytes.
    #[test]
    fn anything_else_from_the_widget_is_nothing() {
        for mode in ["off", "Quiet", "2", "", "1F 03 05 02 02 00", "custom:3"] {
            assert_eq!(parse_request("mode", Some(mode)), None, "{mode:?}");
        }
        assert_eq!(parse_request("mode", None), None);
        assert_eq!(parse_request("none", None), None);
        assert_eq!(parse_request("write", Some("quiet")), None);
    }

    // --- what is said about an attempt ----------------------------------------

    #[test]
    fn a_confirmed_change_needs_no_note_and_anything_else_gets_one() {
        assert_eq!(note_for(&Ok(CommandOutcome::applied())), None);
        assert_eq!(
            note_for(&Ok(CommandOutcome::sent_unverified("still Quiet"))).as_deref(),
            Some("Not confirmed")
        );
        assert_eq!(
            note_for(&Ok(CommandOutcome::Rejected { reason: "busy".into() })).as_deref(),
            Some("Refused by the headphones")
        );
        assert_eq!(
            note_for(&Ok(CommandOutcome::unsupported("no custom mode"))).as_deref(),
            Some("Not available")
        );
        assert_eq!(
            note_for(&Err("unreachable".into())).as_deref(),
            Some("Could not reach them")
        );
    }

    /// What Kotlin is sent, as Kotlin reads it.
    #[test]
    fn the_state_is_written_the_way_kotlin_reads_it() {
        let state = real(true, Some(reported(NoiseControlMode::Custom, Some("Focus"))), &[]);
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "connected");
        assert_eq!(json["name"], "Aurora");
        assert_eq!(json["mode"], "custom");
        assert_eq!(json["customName"], "Focus");
        assert!(json["note"].is_null());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn off_android_there_is_no_widget_to_tell() {
        let state = real(true, None, &[]);
        publish(state, true);
        assert_eq!(*SHOWN.lock(), None, "nothing is recorded as shown that was not");
    }
}
