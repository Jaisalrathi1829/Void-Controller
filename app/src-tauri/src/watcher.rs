//! Watches the headphones come and go, in the background.
//!
//! The window cannot do this job. It stops refreshing once the headphones are
//! gone, so it would never notice them return, and it may be hidden in the
//! tray when a notification or a spoken announcement is due. So a small
//! native task looks at what Windows knows about the headphones every few
//! seconds and reports what changed: connected, disconnected, battery low.
//! What is done about a change — refresh the window, let go of the
//! headphones, raise a notification, speak — is decided by whoever is told.
//!
//! It only ever reads Windows' device properties. Nothing is sent to the
//! headphones from here.

use crate::bose::real::Sighting;
use crate::device::state::DeviceSource;
use crate::settings::StoredSettings;
use crate::AppState;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// How often Windows is asked.
pub const INTERVAL: Duration = Duration::from_secs(4);

/// Battery level at or below which the battery counts as low.
pub const LOW_BATTERY_PERCENT: u8 = 20;

/// The level it has to climb back above before it can be reported low again,
/// so a reading hovering at the threshold does not report itself repeatedly.
pub const LOW_BATTERY_REARM_PERCENT: u8 = 25;

/// How many looks in a row the headphones must be missing from Windows
/// altogether before that counts as a disconnection. Paired headphones that
/// disconnect do not vanish — Windows still lists them, as not connected — so
/// a single look that finds nothing at all is far more likely to be a failed
/// look than a real change.
const MISSING_LOOKS_BEFORE_GONE: u8 = 2;

/// Emitted to the window when the headphones' connection changed.
pub const DEVICE_CHANGED_EVENT: &str = "device-changed";

/// Something worth telling the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Connected { name: String },
    Disconnected { name: String },
    LowBattery { name: String, percent: u8 },
}

impl Notice {
    pub fn title(&self) -> String {
        match self {
            Self::Connected { name } => format!("{name} connected"),
            Self::Disconnected { name } => format!("{name} disconnected"),
            Self::LowBattery { name, .. } => format!("{name} battery low"),
        }
    }

    pub fn body(&self) -> String {
        match self {
            Self::Connected { .. } => "Your headphones are connected to this PC.".to_string(),
            Self::Disconnected { .. } => {
                "Your headphones are no longer connected to this PC.".to_string()
            }
            Self::LowBattery { percent, .. } => format!("{percent}% remaining."),
        }
    }
}

/// What one look at the headphones found to have changed.
///
/// Facts only. Whether any of it is announced is up to the settings of
/// whoever is announcing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    /// The connection state changed, so the window should refresh.
    pub changed: bool,
    /// The headphones went from connected to not.
    pub lost: bool,
    /// The headphones went from not connected to connected.
    pub returned: bool,
    /// The battery was found to be low: a valid reading from connected
    /// headphones at or below the threshold. Reported once per dip.
    pub low_battery: Option<u8>,
    /// What the headphones are called.
    pub name: String,
}

impl Step {
    /// The notifications this step calls for under the given settings.
    pub fn notices(&self, settings: &StoredSettings) -> Vec<Notice> {
        let mut notices = Vec::new();
        let name = || self.name.clone();
        if self.returned && settings.notify_connected {
            notices.push(Notice::Connected { name: name() });
        }
        if self.lost && settings.notify_disconnected {
            notices.push(Notice::Disconnected { name: name() });
        }
        if let (Some(percent), true) = (self.low_battery, settings.notify_low_battery) {
            notices.push(Notice::LowBattery {
                name: name(),
                percent,
            });
        }
        notices
    }
}

/// What has been seen so far.
#[derive(Debug, Default)]
pub struct WatchState {
    /// Whether the headphones were connected at the last look. `None` before
    /// the first one.
    was_connected: Option<bool>,
    last_name: Option<String>,
    low_battery_reported: bool,
    missing_looks: u8,
}

impl WatchState {
    /// Takes one look and reports what changed.
    ///
    /// `now` is `None` when Windows does not list the headphones at all. The
    /// very first look reports no connection change: the application starting
    /// up is not the headphones connecting.
    pub fn observe(&mut self, now: Option<&Sighting>) -> Step {
        let name = now
            .map(|s| s.name.clone())
            .or_else(|| self.last_name.clone())
            .unwrap_or_else(|| "Headphones".to_string());
        let mut step = Step {
            name: name.clone(),
            ..Step::default()
        };

        // Headphones that were connected and are suddenly not listed at all:
        // wait for a second look before believing it.
        match now {
            Some(_) => self.missing_looks = 0,
            None if self.was_connected == Some(true) => {
                self.missing_looks += 1;
                if self.missing_looks < MISSING_LOOKS_BEFORE_GONE {
                    return step;
                }
            }
            None => {}
        }

        let connected = now.is_some_and(|s| s.connected);
        match self.was_connected {
            Some(false) if connected => {
                step.changed = true;
                step.returned = true;
            }
            Some(true) if !connected => {
                step.changed = true;
                step.lost = true;
            }
            _ => {}
        }

        // Low battery: once per dip, from a reading that is actually there
        // and belongs to headphones that are actually connected.
        match now.and_then(|s| s.battery).filter(|_| connected) {
            Some(percent) if percent <= LOW_BATTERY_PERCENT => {
                if !self.low_battery_reported {
                    self.low_battery_reported = true;
                    step.low_battery = Some(percent);
                }
            }
            Some(percent) if percent > LOW_BATTERY_REARM_PERCENT => {
                self.low_battery_reported = false;
            }
            // Between the two thresholds: no change either way.
            Some(_) => {}
            // No reading. If that is because the headphones are gone, the
            // next session starts afresh; a reading that is merely missing
            // changes nothing.
            None => {
                if !connected {
                    self.low_battery_reported = false;
                }
            }
        }

        self.was_connected = Some(connected);
        if now.is_some() {
            self.last_name = Some(name);
        }
        step
    }
}

fn notify(app: &AppHandle, notice: &Notice) {
    // A notification that cannot be shown is not worth failing anything over.
    let _ = app
        .notification()
        .builder()
        .title(notice.title())
        .body(notice.body())
        .show();
}

/// Starts the background task. It runs for the life of the application.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut watch = WatchState::default();
        loop {
            tokio::time::sleep(INTERVAL).await;

            let Some(state) = app.try_state::<AppState>() else {
                continue;
            };
            // Only real headphones are watched. A simulated device raising
            // Windows notifications, or being announced aloud, would be
            // passing itself off as real.
            if state.current().source() != DeviceSource::Real {
                watch = WatchState::default();
                continue;
            }

            let settings = state.settings().get();
            let stored = settings.preferred_device_id.clone();
            let sighting = tauri::async_runtime::spawn_blocking(move || {
                crate::bose::real::sight_preferred(stored.as_deref())
            })
            .await
            .ok()
            .flatten();

            if let Some(seen) = &sighting {
                state.remember_device(seen);
            }

            let step = watch.observe(sighting.as_ref());

            // With auto reconnect off, headphones that leave are let go of:
            // when they return, the application waits to be asked.
            if step.lost && !settings.auto_reconnect {
                state.detach();
            }
            for notice in step.notices(&settings) {
                notify(&app, &notice);
            }
            state.voice().observe_presence(&step);

            // A mode changed on the headphones themselves, or from a phone,
            // is only found by asking. The window asks while it is on screen
            // and stops when it is hidden; this asks either way, so that a
            // change is spoken with the application in the tray — and only
            // when the answer would be spoken at all. It is the same read the
            // window makes, with the same backoff after a failed one.
            let connected = sighting.as_ref().is_some_and(|seen| seen.connected);
            if connected && state.voice().wants_mode_readings() {
                if let Ok(snapshot) = state.current().snapshot().await {
                    state.voice().observe_snapshot(&snapshot);
                }
            }

            if step.changed {
                let _ = app.emit(DEVICE_CHANGED_EVENT, ());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(connected: bool, battery: Option<u8>) -> Sighting {
        Sighting {
            instance_id: "BTHENUM\\DEV_TEST".into(),
            name: "Aurora".into(),
            connected,
            battery,
        }
    }

    fn all_on() -> StoredSettings {
        StoredSettings {
            notify_connected: true,
            notify_disconnected: true,
            notify_low_battery: true,
            ..StoredSettings::default()
        }
    }

    fn look(state: &mut WatchState, connected: bool, battery: Option<u8>) -> Step {
        state.observe(Some(&seen(connected, battery)))
    }

    fn quiet(step: &Step) -> bool {
        !step.changed && !step.lost && !step.returned && step.low_battery.is_none()
    }

    // --- connection ---------------------------------------------------------

    #[test]
    fn starting_up_with_the_headphones_connected_reports_no_change() {
        let mut w = WatchState::default();
        assert!(quiet(&look(&mut w, true, Some(80))));
    }

    #[test]
    fn starting_up_with_them_disconnected_reports_no_change_either() {
        let mut w = WatchState::default();
        assert!(quiet(&look(&mut w, false, None)));
        assert!(quiet(&w.observe(None)));
    }

    #[test]
    fn a_connection_and_a_disconnection_are_each_reported_once() {
        let mut w = WatchState::default();
        look(&mut w, false, None);

        let step = look(&mut w, true, Some(80));
        assert!(step.changed && step.returned && !step.lost);
        assert_eq!(step.name, "Aurora");
        assert!(quiet(&look(&mut w, true, Some(80))), "still connected");

        let step = look(&mut w, false, None);
        assert!(step.changed && step.lost && !step.returned);
        assert!(quiet(&look(&mut w, false, None)), "still disconnected");
    }

    #[test]
    fn reconnecting_after_a_real_disconnection_is_reported_once() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(80));
        look(&mut w, false, None);
        let returns: usize = (0..5)
            .filter(|_| look(&mut w, true, Some(80)).returned)
            .count();
        assert_eq!(returns, 1);
    }

    /// One look that finds nothing at all is not believed. Paired headphones
    /// that really disconnect are still listed, as not connected.
    #[test]
    fn a_single_failed_look_is_not_a_disconnection() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(80));

        assert!(quiet(&w.observe(None)), "one empty look");
        // Found again, still connected: nothing happened.
        assert!(quiet(&look(&mut w, true, Some(80))));
    }

    #[test]
    fn headphones_missing_for_two_looks_running_are_gone() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(80));
        assert!(quiet(&w.observe(None)));
        let step = w.observe(None);
        assert!(step.lost && step.changed);
        assert_eq!(step.name, "Aurora", "still called what they were called");
        assert!(quiet(&w.observe(None)), "reported once");
    }

    /// A machine waking from sleep, or the Bluetooth radio restarting: a
    /// burst of failed looks, then the truth. One report at most each way.
    #[test]
    fn a_burst_of_failed_looks_then_recovery_does_not_flood() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(80));

        let mut lost = 0;
        let mut returned = 0;
        for sighting in [None, None, None, None] {
            let step = w.observe(sighting);
            lost += step.lost as u32;
            returned += step.returned as u32;
        }
        for _ in 0..4 {
            let step = look(&mut w, true, Some(80));
            lost += step.lost as u32;
            returned += step.returned as u32;
        }
        assert_eq!((lost, returned), (1, 1));
    }

    // --- battery ------------------------------------------------------------

    #[test]
    fn crossing_the_threshold_downward_is_reported_once() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(40));
        assert_eq!(look(&mut w, true, Some(21)).low_battery, None, "not low yet");
        assert_eq!(look(&mut w, true, Some(20)).low_battery, Some(20));
        // Lower still, and hovering around the line: no repeats.
        for level in [19, 15, 22, 19, 24, 20] {
            assert_eq!(look(&mut w, true, Some(level)).low_battery, None, "at {level}%");
        }
    }

    #[test]
    fn the_warning_rearms_only_above_the_reset_threshold() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(18));
        // Back up to exactly the reset level: not above it, so still armed off.
        look(&mut w, true, Some(LOW_BATTERY_REARM_PERCENT));
        assert_eq!(look(&mut w, true, Some(18)).low_battery, None);
        // Above it: rearmed.
        look(&mut w, true, Some(LOW_BATTERY_REARM_PERCENT + 1));
        assert_eq!(look(&mut w, true, Some(18)).low_battery, Some(18));
    }

    /// A battery already low when first seen is worth saying once.
    #[test]
    fn a_battery_already_low_when_first_seen_is_reported() {
        let mut w = WatchState::default();
        let step = look(&mut w, true, Some(12));
        assert_eq!(step.low_battery, Some(12));
        assert!(!step.changed, "the connection itself did not change");
    }

    #[test]
    fn missing_readings_report_nothing_and_do_not_rearm() {
        let mut w = WatchState::default();
        assert_eq!(look(&mut w, true, None).low_battery, None);
        look(&mut w, true, Some(15));
        // The reading disappears for a while, then comes back still low.
        assert_eq!(look(&mut w, true, None).low_battery, None);
        assert_eq!(look(&mut w, true, Some(15)).low_battery, None, "same dip");
    }

    /// Windows can keep the last level it saw after the headphones have gone.
    /// That is not a current reading.
    #[test]
    fn a_level_from_disconnected_headphones_is_ignored() {
        let mut w = WatchState::default();
        assert_eq!(look(&mut w, false, Some(5)).low_battery, None);
    }

    #[test]
    fn a_low_battery_is_reported_again_after_reconnecting() {
        let mut w = WatchState::default();
        look(&mut w, true, Some(15));
        look(&mut w, false, None);
        assert_eq!(look(&mut w, true, Some(15)).low_battery, Some(15));
    }

    // --- notifications ------------------------------------------------------

    #[test]
    fn each_notification_obeys_its_own_switch() {
        let step = Step {
            changed: true,
            lost: true,
            returned: false,
            low_battery: Some(10),
            name: "Aurora".into(),
        };
        assert!(step.notices(&StoredSettings::default()).is_empty());
        assert_eq!(
            step.notices(&all_on()),
            vec![
                Notice::Disconnected { name: "Aurora".into() },
                Notice::LowBattery { name: "Aurora".into(), percent: 10 },
            ]
        );
        let only_battery = StoredSettings {
            notify_low_battery: true,
            ..StoredSettings::default()
        };
        assert_eq!(
            step.notices(&only_battery),
            vec![Notice::LowBattery { name: "Aurora".into(), percent: 10 }]
        );
    }

    #[test]
    fn a_step_with_nothing_in_it_notifies_nothing() {
        let step = Step {
            name: "Aurora".into(),
            ..Step::default()
        };
        assert!(step.notices(&all_on()).is_empty());
    }

    #[test]
    fn notices_read_like_sentences() {
        let n = Notice::LowBattery {
            name: "Aurora".into(),
            percent: 18,
        };
        assert_eq!(n.title(), "Aurora battery low");
        assert_eq!(n.body(), "18% remaining.");
        assert_eq!(
            Notice::Connected { name: "Aurora".into() }.title(),
            "Aurora connected"
        );
    }
}
