//! The home-screen widget's native half.
//!
//! The widget itself is Kotlin (`ModeWidget.kt`): three mode buttons on the
//! home screen and, when it is tall enough, a step down and a step up for
//! each band of the equalizer. It decides nothing. A tap on it is a request
//! by name — a mode, or one band one step — and this is where the request is
//! acted on, through the same verified changes the application's own controls
//! use, and where what the widget shows comes from: what the headphones
//! report, never what was asked for.
//!
//! Two directions, both over the bridge this crate already has:
//!
//! * **Requests in.** One thread asks Kotlin for the next request and waits
//!   until there is one. That it is asking is also how the widget knows the
//!   native layer is alive; when the application is not running, nothing
//!   asks, and the widget says so rather than showing a stale mode.
//! * **State out.** Whenever the headphones are seen to connect, go, or
//!   change mode or equalizer, the widget is told — if what it would show has
//!   changed.
//!
//! A request carries names and nothing else. It is read into closed types,
//! so the widget can ask for Quiet, Aware or the custom mode, or for bass,
//! mid or treble to go up or down by one, and for nothing besides.

use crate::device::state::{
    DeviceSource, EqSettings, EqState, NoiseControlMode, NoiseControlState,
};
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
    /// The equalizer the headphones last reported, in whole decibels. All
    /// three or none: `None` when they have not reported one.
    pub bass: Option<i8>,
    pub mid: Option<i8>,
    pub treble: Option<i8>,
    /// A few words about the last request, when it did not simply succeed.
    pub note: Option<String>,
}

/// Builds what the widget shows from what is known.
///
/// A mode or an equalizer is passed on only for real, connected headphones.
/// The simulated device has both too; a home-screen widget showing one as
/// the headphones' would be the simulation passing itself off as real.
pub fn state_from(
    source: DeviceSource,
    connected: bool,
    name: Option<String>,
    reported: Option<&NoiseControlState>,
    custom_modes: &[String],
    equalizer: Option<&EqState>,
    note: Option<String>,
) -> WidgetState {
    if source != DeviceSource::Real {
        return WidgetState {
            status: "simulated",
            name: None,
            mode: None,
            custom_name: None,
            bass: None,
            mid: None,
            treble: None,
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
    let eq = live.and(equalizer).map(|state| state.settings);
    WidgetState {
        status: if connected { "connected" } else { "disconnected" },
        name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
        mode,
        custom_name,
        bass: eq.map(|eq| eq.bass),
        mid: eq.map(|eq| eq.mid),
        treble: eq.map(|eq| eq.treble),
        note: live.and(note),
    }
}

/// One band of the equalizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Bass,
    Mid,
    Treble,
}

/// A request from the widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Switch to this mode.
    Mode(NoiseModeRequest),
    /// Move one band of the equalizer one decibel.
    Step { band: Band, up: bool },
    /// Say where things stand.
    Refresh,
}

/// Reads what Kotlin handed over. Anything that is not one of the three
/// modes, one band one step either way, or a plain request for the state, is
/// nothing. There is no form in which a level, an index or bytes could be
/// asked for.
pub fn parse_request(
    kind: &str,
    mode: Option<&str>,
    band: Option<&str>,
    step: Option<&str>,
) -> Option<Request> {
    match kind {
        "refresh" => Some(Request::Refresh),
        "mode" => {
            let name = serde_json::Value::String(mode?.to_string());
            serde_json::from_value::<NoiseModeRequest>(name)
                .ok()
                .map(Request::Mode)
        }
        "eq" => {
            let band = match band? {
                "bass" => Band::Bass,
                "mid" => Band::Mid,
                "treble" => Band::Treble,
                _ => return None,
            };
            let up = match step? {
                "up" => true,
                "down" => false,
                _ => return None,
            };
            Some(Request::Step { band, up })
        }
        _ => None,
    }
}

/// The equalizer one step on from `now`: one band moved by one decibel, the
/// other two exactly as they are. `None` if that band is already at the end
/// of its range, or anything in `now` is outside the range to begin with —
/// then there is nothing this can properly send.
pub fn stepped(now: EqSettings, band: Band, up: bool) -> Option<EqSettings> {
    if !now.is_within_range() {
        return None;
    }
    let mut next = now;
    let gain = match band {
        Band::Bass => &mut next.bass,
        Band::Mid => &mut next.mid,
        Band::Treble => &mut next.treble,
    };
    *gain = if up { gain.checked_add(1)? } else { gain.checked_sub(1)? };
    next.is_within_range().then_some(next)
}

/// Said when a step was asked for at the end of the range.
pub const AT_THE_LIMIT: &str = "Already at the limit";

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

/// Whether a widget that is on a home screen has the equalizer on it. Kotlin
/// says, each time it is told the state. Until it says so, nothing is read
/// from the headphones on the widget's account.
static SHOWS_EQ: AtomicBool = AtomicBool::new(false);

/// Whether the headphones were connected, and what they were called, the
/// last time the watcher looked. The watcher looks every few seconds; a mode
/// change is reported in between, and uses this.
static SEEN: Mutex<(bool, Option<String>)> = Mutex::new((false, None));

/// Each start of the listener has a number; one that finds a later number
/// has been replaced and stops.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);

/// What Kotlin answers when it is told the state.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Drawn {
    #[serde(default)]
    shows_eq: bool,
}

/// Tells the widget, if what it shows would change. Must not be called from
/// the thread that runs the event loop.
fn publish(state: WidgetState, force: bool) {
    let mut shown = SHOWN.lock();
    if !force && shown.as_ref() == Some(&state) {
        return;
    }
    if let Ok(drawn) = super::bridge::call::<Drawn>("widgetState", &state) {
        SHOWS_EQ.store(drawn.shows_eq, std::sync::atomic::Ordering::SeqCst);
        *shown = Some(state);
    }
}

#[cfg(target_os = "android")]
mod live {
    use super::*;
    use crate::bose::real::Sighting;
    use crate::AppState;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Emitter, Manager};

    /// How long one question to Kotlin waits for a request before it is
    /// asked again. Each asking is also the sign of life the widget goes by.
    const WAIT: Duration = Duration::from_secs(50);

    /// How old the equalizer on the widget is allowed to grow before it is
    /// read again. Nothing on the headphones changes it; another phone or
    /// computer they are connected to can.
    const EQ_FRESH: Duration = Duration::from_secs(300);
    /// And how soon it is asked for again when there is none to show.
    const EQ_RETRY: Duration = Duration::from_secs(60);

    /// When the equalizer was last asked for on the widget's account.
    static EQ_ASKED: Mutex<Option<Instant>> = Mutex::new(None);

    #[derive(serde::Deserialize)]
    struct Next {
        kind: String,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        band: Option<String>,
        #[serde(default)]
        step: Option<String>,
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
            device.reported_equalizer().as_ref(),
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

    /// The mode or the equalizer has changed, or may have.
    pub fn mode_changed(app: &AppHandle) {
        if let Some(state) = current(app, None) {
            publish(state, false);
        }
    }

    /// Reads the equalizer for the widget, if a widget shows one and what it
    /// shows is missing or has grown old. One question to the headphones,
    /// over the link that is already open; nothing is asked of headphones
    /// that have not already answered for their mode.
    pub async fn freshen(app: &AppHandle) {
        if !SHOWS_EQ.load(Ordering::SeqCst) {
            return;
        }
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let device = state.current();
        if device.source() != DeviceSource::Real || device.reported_noise_control().is_none() {
            return;
        }
        let wait = if device.reported_equalizer().is_some() {
            EQ_FRESH
        } else {
            EQ_RETRY
        };
        {
            let mut asked = EQ_ASKED.lock();
            if asked.is_some_and(|at| at.elapsed() < wait) {
                return;
            }
            *asked = Some(Instant::now());
        }
        let _ = device.equalizer().await;
        let app = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || mode_changed(&app)).await;
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

    async fn step(app: &AppHandle, band: Band, up: bool) {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let device = state.current();
        let note = if device.source() == DeviceSource::Real {
            // Read first. The step is taken from what the headphones have
            // now, not from what the widget was last showing, and the other
            // two bands go back exactly as they were just read.
            match device.equalizer().await {
                Ok(Some(now)) => {
                    *EQ_ASKED.lock() = Some(Instant::now());
                    match stepped(now.settings, band, up) {
                        Some(next) => {
                            let outcome = device
                                .set_equalizer(next)
                                .await
                                .map_err(|e| e.to_string());
                            let _ = app.emit(crate::watcher::DEVICE_CHANGED_EVENT, ());
                            note_for(&outcome)
                        }
                        None => Some(AT_THE_LIMIT.to_string()),
                    }
                }
                // Not connected: the widget will say so.
                Ok(None) => None,
                Err(_) => Some("Could not reach them".to_string()),
            }
        } else {
            None
        };
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
                    let request = parse_request(
                        &next.kind,
                        next.mode.as_deref(),
                        next.band.as_deref(),
                        next.step.as_deref(),
                    );
                    match request {
                        Some(Request::Mode(mode)) => {
                            tauri::async_runtime::block_on(switch(&app, mode));
                        }
                        Some(Request::Step { band, up }) => {
                            tauri::async_runtime::block_on(step(&app, band, up));
                        }
                        Some(Request::Refresh) => {
                            if let Some(state) = current(&app, None) {
                                publish(state, true);
                            }
                            // Kotlin has just said whether a widget shows the
                            // equalizer; if one does and has none, read it.
                            tauri::async_runtime::block_on(freshen(&app));
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
pub use live::{freshen, mode_changed, seen, spawn, stop};

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

    fn eq(bass: i8, mid: i8, treble: i8) -> EqState {
        EqState {
            settings: EqSettings { bass, mid, treble },
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
            None,
        )
    }

    fn real_eq(connected: bool, equalizer: Option<EqState>) -> WidgetState {
        state_from(
            DeviceSource::Real,
            connected,
            Some("Aurora".into()),
            None,
            &[],
            equalizer.as_ref(),
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

    /// The simulated device's modes and equalizer are not the headphones'.
    #[test]
    fn the_simulated_device_is_never_shown_as_the_headphones() {
        let state = state_from(
            DeviceSource::Mock,
            true,
            Some("Bose QuietComfort (SIMULATED)".into()),
            Some(&reported(NoiseControlMode::Quiet, None)),
            &["Focus".to_string()],
            Some(&eq(4, 1, 4)),
            Some("Not confirmed".into()),
        );
        assert_eq!(state.status, "simulated");
        assert_eq!((state.mode, state.name, state.custom_name, state.note), (None, None, None, None));
        assert_eq!((state.bass, state.mid, state.treble), (None, None, None));
    }

    #[test]
    fn a_note_is_only_shown_about_connected_headphones() {
        let note = Some("Not confirmed".to_string());
        let on = state_from(DeviceSource::Real, true, None, None, &[], None, note.clone());
        assert_eq!(on.note.as_deref(), Some("Not confirmed"));
        let off = state_from(DeviceSource::Real, false, None, None, &[], None, note);
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
            None,
        );
        assert_eq!(state.name, None);
        assert_eq!(state.custom_name, None);
    }

    // --- the equalizer -------------------------------------------------------

    #[test]
    fn connected_headphones_show_the_equalizer_they_reported() {
        let state = real_eq(true, Some(eq(4, 1, -3)));
        assert_eq!((state.bass, state.mid, state.treble), (Some(4), Some(1), Some(-3)));
    }

    /// Not a flat one, not the last one asked for: none.
    #[test]
    fn with_no_report_no_equalizer_is_shown() {
        let state = real_eq(true, None);
        assert_eq!((state.bass, state.mid, state.treble), (None, None, None));
    }

    #[test]
    fn disconnected_headphones_show_no_equalizer() {
        let state = real_eq(false, Some(eq(4, 1, 4)));
        assert_eq!((state.bass, state.mid, state.treble), (None, None, None));
    }

    #[test]
    fn a_step_moves_one_band_by_one_and_leaves_the_others() {
        let now = EqSettings { bass: 4, mid: 1, treble: -3 };
        assert_eq!(
            stepped(now, Band::Bass, true),
            Some(EqSettings { bass: 5, mid: 1, treble: -3 })
        );
        assert_eq!(
            stepped(now, Band::Mid, false),
            Some(EqSettings { bass: 4, mid: 0, treble: -3 })
        );
        assert_eq!(
            stepped(now, Band::Treble, false),
            Some(EqSettings { bass: 4, mid: 1, treble: -4 })
        );
    }

    #[test]
    fn there_is_no_step_past_either_end() {
        let top = EqSettings { bass: EqSettings::MAX_DB, mid: 0, treble: EqSettings::MIN_DB };
        assert_eq!(stepped(top, Band::Bass, true), None);
        assert_eq!(stepped(top, Band::Treble, false), None);
        // Back from the end is an ordinary step.
        assert_eq!(stepped(top, Band::Bass, false).map(|eq| eq.bass), Some(9));
        assert_eq!(stepped(top, Band::Treble, true).map(|eq| eq.treble), Some(-9));
    }

    /// Whatever is sent, all three bands are sent. If the headphones report a
    /// band outside the range this application knows, nothing it could send
    /// would leave that band as it is.
    #[test]
    fn nothing_is_stepped_from_a_reading_outside_the_range() {
        let odd = EqSettings { bass: 0, mid: 12, treble: 0 };
        assert_eq!(stepped(odd, Band::Bass, true), None);
        let far = EqSettings { bass: i8::MAX, mid: 0, treble: 0 };
        assert_eq!(stepped(far, Band::Bass, true), None, "and does not overflow");
        let low = EqSettings { bass: i8::MIN, mid: 0, treble: 0 };
        assert_eq!(stepped(low, Band::Bass, false), None);
    }

    /// Every step the widget can ask for, from every in-range value, is one
    /// the command layer would accept.
    #[test]
    fn every_step_stays_in_range() {
        for gain in EqSettings::MIN_DB..=EqSettings::MAX_DB {
            let now = EqSettings { bass: gain, mid: gain, treble: gain };
            for band in [Band::Bass, Band::Mid, Band::Treble] {
                for up in [true, false] {
                    if let Some(next) = stepped(now, band, up) {
                        assert!(next.is_within_range(), "{now:?} {band:?} {up}");
                    }
                }
            }
        }
    }

    // --- requests -----------------------------------------------------------

    #[test]
    fn the_widget_can_ask_for_exactly_three_modes() {
        assert_eq!(
            parse_request("mode", Some("quiet"), None, None),
            Some(Request::Mode(NoiseModeRequest::Quiet))
        );
        assert_eq!(
            parse_request("mode", Some("aware"), None, None),
            Some(Request::Mode(NoiseModeRequest::Aware))
        );
        assert_eq!(
            parse_request("mode", Some("custom"), None, None),
            Some(Request::Mode(NoiseModeRequest::Custom))
        );
        assert_eq!(parse_request("refresh", None, None, None), Some(Request::Refresh));
    }

    #[test]
    fn the_widget_can_ask_for_one_band_one_step_either_way() {
        for (name, band) in [("bass", Band::Bass), ("mid", Band::Mid), ("treble", Band::Treble)] {
            assert_eq!(
                parse_request("eq", None, Some(name), Some("up")),
                Some(Request::Step { band, up: true })
            );
            assert_eq!(
                parse_request("eq", None, Some(name), Some("down")),
                Some(Request::Step { band, up: false })
            );
        }
    }

    /// Nothing else gets through: not "off", not an index, not bytes.
    #[test]
    fn anything_else_from_the_widget_is_nothing() {
        for mode in ["off", "Quiet", "2", "", "1F 03 05 02 02 00", "custom:3"] {
            assert_eq!(parse_request("mode", Some(mode), None, None), None, "{mode:?}");
        }
        assert_eq!(parse_request("mode", None, None, None), None);
        assert_eq!(parse_request("none", None, None, None), None);
        assert_eq!(parse_request("write", Some("quiet"), None, None), None);
    }

    /// A step is a direction, never an amount or a level.
    #[test]
    fn an_equalizer_request_is_a_band_and_a_direction_and_nothing_else() {
        for band in ["Bass", "low", "0", "", "all", "01 07"] {
            assert_eq!(parse_request("eq", None, Some(band), Some("up")), None, "{band:?}");
        }
        for step in ["+1", "2", "10", "-10", "UP", "", "max", "reset"] {
            assert_eq!(parse_request("eq", None, Some("bass"), Some(step)), None, "{step:?}");
        }
        assert_eq!(parse_request("eq", None, None, Some("up")), None);
        assert_eq!(parse_request("eq", None, Some("bass"), None), None);
        // A mode's name in the wrong place asks for nothing.
        assert_eq!(parse_request("eq", Some("quiet"), None, None), None);
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
        let state = state_from(
            DeviceSource::Real,
            true,
            Some("Aurora".into()),
            Some(&reported(NoiseControlMode::Custom, Some("Focus"))),
            &[],
            Some(&eq(4, 1, -3)),
            None,
        );
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "connected");
        assert_eq!(json["name"], "Aurora");
        assert_eq!(json["mode"], "custom");
        assert_eq!(json["customName"], "Focus");
        assert_eq!((json["bass"].as_i64(), json["mid"].as_i64(), json["treble"].as_i64()), (Some(4), Some(1), Some(-3)));
        assert!(json["note"].is_null());

        let none = serde_json::to_value(real(true, None, &[])).unwrap();
        assert!(none["bass"].is_null() && none["mid"].is_null() && none["treble"].is_null());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn off_android_there_is_no_widget_to_tell() {
        let state = real(true, None, &[]);
        publish(state, true);
        assert_eq!(*SHOWN.lock(), None, "nothing is recorded as shown that was not");
    }
}
