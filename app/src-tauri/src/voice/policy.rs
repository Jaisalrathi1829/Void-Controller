//! What gets said, and when a confirmed change is worth saying.
//!
//! Everything here is a pure function of observations that have already been
//! confirmed elsewhere — a mode the headphones themselves reported, a
//! connection state Windows reported. Nothing in this module can produce an
//! announcement from a request, a click, or an acknowledgement, because none
//! of those ever reach it.

use crate::device::state::{
    ConnectionState, DeviceSnapshot, DeviceSource, NoiseControlMode, NoiseControlState,
};

/// A mode as it will be spoken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpokenMode {
    Quiet,
    Aware,
    /// A custom mode, by the headphones' own name for it if they gave one.
    Custom(Option<String>),
}

impl SpokenMode {
    /// The spoken mode for a state the headphones reported, if it is one this
    /// application announces.
    pub fn from_state(state: &NoiseControlState) -> Option<Self> {
        match state.mode {
            NoiseControlMode::Quiet => Some(Self::Quiet),
            NoiseControlMode::Aware => Some(Self::Aware),
            NoiseControlMode::Custom => Some(Self::Custom(state.label.clone())),
            NoiseControlMode::Off => None,
        }
    }
}

/// The setting that switches a kind of announcement on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Mode,
    Connection,
    Battery,
    /// The Test Voice button. Not an automatic announcement and not governed
    /// by the announcement switches.
    Test,
}

/// Something to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Announcement {
    Disconnected { name: String },
    Connected { name: String },
    Mode(SpokenMode),
    LowBattery { percent: u8 },
    Test,
}

/// The sentence spoken by the Test Voice button.
pub const TEST_PHRASE: &str = "Voice feedback is ready.";

impl Announcement {
    pub fn text(&self) -> String {
        match self {
            Self::Disconnected { name } => format!("{name} disconnected."),
            Self::Connected { name } => format!("{name} connected."),
            Self::Mode(SpokenMode::Quiet) => "Quiet mode activated.".to_string(),
            Self::Mode(SpokenMode::Aware) => "Aware mode activated.".to_string(),
            Self::Mode(SpokenMode::Custom(Some(label))) => format!("{label} mode activated."),
            Self::Mode(SpokenMode::Custom(None)) => "Custom mode activated.".to_string(),
            Self::LowBattery { percent } => format!("Battery low. {percent} percent."),
            Self::Test => TEST_PHRASE.to_string(),
        }
    }

    pub fn category(&self) -> Category {
        match self {
            Self::Disconnected { .. } | Self::Connected { .. } => Category::Connection,
            Self::Mode(_) => Category::Mode,
            Self::LowBattery { .. } => Category::Battery,
            Self::Test => Category::Test,
        }
    }

    /// Lower speaks first.
    ///
    /// 0 — the headphones connecting or disconnecting
    /// 1 — a confirmed mode change
    /// 2 — a low-battery warning
    /// 3 — reserved for routine battery reports, which are not implemented
    /// 4 — the Test Voice button
    pub fn priority(&self) -> u8 {
        match self {
            Self::Disconnected { .. } | Self::Connected { .. } => 0,
            Self::Mode(_) => 1,
            Self::LowBattery { .. } => 2,
            Self::Test => 4,
        }
    }

    /// Whether the application decided to say this on its own, as opposed to
    /// the user pressing Test Voice.
    pub fn is_automatic(&self) -> bool {
        !matches!(self, Self::Test)
    }
}

/// Follows the mode the headphones report and notices when it changes.
#[derive(Debug, Default)]
pub struct ModeTracker {
    current: Option<SpokenMode>,
}

impl ModeTracker {
    /// The mode the headphones most recently confirmed, if any.
    pub fn current(&self) -> Option<&SpokenMode> {
        self.current.as_ref()
    }

    /// Takes one confirmed reading — `None` when there is none, because the
    /// headphones are not connected or have not reported a mode.
    ///
    /// Returns an announcement only for a change between two confirmed
    /// readings. The first reading after there was none is where things
    /// stand, not a change: starting the application, or the headphones
    /// reconnecting, does not announce the mode they happen to be in.
    pub fn observe(&mut self, confirmed: Option<SpokenMode>) -> Option<Announcement> {
        let changed = match (&self.current, &confirmed) {
            (Some(before), Some(now)) => before != now,
            _ => false,
        };
        self.current = confirmed;
        if changed {
            self.current.clone().map(Announcement::Mode)
        } else {
            None
        }
    }

    /// Forgets the mode: the headphones went away.
    pub fn reset(&mut self) {
        self.current = None;
    }

    /// Whether a queued announcement still describes reality. A mode
    /// announcement goes stale the moment the headphones report a different
    /// mode — or stop reporting one.
    pub fn still_true(&self, announcement: &Announcement) -> bool {
        match announcement {
            Announcement::Mode(mode) => self.current.as_ref() == Some(mode),
            _ => true,
        }
    }
}

/// The confirmed mode in a snapshot, if the snapshot is evidence of one.
///
/// Only a snapshot of real, connected headphones counts. A simulated device's
/// mode is not a hardware state, and is never announced as one.
pub fn confirmed_mode(snapshot: &DeviceSnapshot) -> Option<SpokenMode> {
    if snapshot.source != DeviceSource::Real || snapshot.connection != ConnectionState::Connected {
        return None;
    }
    snapshot
        .noise_control
        .as_ref()
        .and_then(SpokenMode::from_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::state::StateSource;

    fn state(mode: NoiseControlMode, label: Option<&str>) -> NoiseControlState {
        NoiseControlState {
            mode,
            level: None,
            label: label.map(str::to_string),
            source: StateSource::VendorProtocol,
            read_at: "2026-10-10T09:00:00Z".into(),
        }
    }

    fn snapshot(
        source: DeviceSource,
        connection: ConnectionState,
        mode: Option<NoiseControlMode>,
    ) -> DeviceSnapshot {
        DeviceSnapshot {
            connection,
            noise_control: mode.map(|m| state(m, None)),
            ..DeviceSnapshot::empty(source)
        }
    }

    // --- wording ------------------------------------------------------------

    #[test]
    fn the_required_sentences_are_exactly_these() {
        assert_eq!(Announcement::Mode(SpokenMode::Quiet).text(), "Quiet mode activated.");
        assert_eq!(Announcement::Mode(SpokenMode::Aware).text(), "Aware mode activated.");
        assert_eq!(
            Announcement::Connected { name: "Aurora".into() }.text(),
            "Aurora connected."
        );
        assert_eq!(
            Announcement::Disconnected { name: "Aurora".into() }.text(),
            "Aurora disconnected."
        );
        assert_eq!(Announcement::Test.text(), "Voice feedback is ready.");
    }

    #[test]
    fn a_custom_mode_is_called_by_its_own_name() {
        let named = SpokenMode::Custom(Some("Focus".into()));
        assert_eq!(Announcement::Mode(named).text(), "Focus mode activated.");
        assert_eq!(
            Announcement::Mode(SpokenMode::Custom(None)).text(),
            "Custom mode activated."
        );
    }

    #[test]
    fn a_battery_warning_says_the_level_it_was_given() {
        assert_eq!(
            Announcement::LowBattery { percent: 18 }.text(),
            "Battery low. 18 percent."
        );
    }

    // --- priorities ---------------------------------------------------------

    #[test]
    fn priorities_run_connection_then_mode_then_battery_then_test() {
        let connection = Announcement::Disconnected { name: "A".into() }.priority();
        let mode = Announcement::Mode(SpokenMode::Quiet).priority();
        let battery = Announcement::LowBattery { percent: 10 }.priority();
        let test = Announcement::Test.priority();
        assert!(connection < mode && mode < battery && battery < test);
        assert_eq!(
            Announcement::Connected { name: "A".into() }.priority(),
            connection
        );
    }

    #[test]
    fn only_the_test_button_is_not_automatic() {
        assert!(!Announcement::Test.is_automatic());
        assert!(Announcement::Mode(SpokenMode::Quiet).is_automatic());
        assert!(Announcement::LowBattery { percent: 5 }.is_automatic());
        assert!(Announcement::Connected { name: "A".into() }.is_automatic());
        assert_eq!(Announcement::Test.category(), Category::Test);
    }

    // --- mode changes -------------------------------------------------------

    #[test]
    fn a_confirmed_change_to_quiet_announces_quiet() {
        let mut t = ModeTracker::default();
        t.observe(Some(SpokenMode::Aware));
        assert_eq!(
            t.observe(Some(SpokenMode::Quiet)),
            Some(Announcement::Mode(SpokenMode::Quiet))
        );
    }

    #[test]
    fn a_confirmed_change_to_aware_announces_aware() {
        let mut t = ModeTracker::default();
        t.observe(Some(SpokenMode::Quiet));
        assert_eq!(
            t.observe(Some(SpokenMode::Aware)),
            Some(Announcement::Mode(SpokenMode::Aware))
        );
    }

    /// The application starting, or the headphones reconnecting, finds them in
    /// some mode. That is not a change and is not announced.
    #[test]
    fn the_first_reading_is_not_a_change() {
        let mut t = ModeTracker::default();
        assert_eq!(t.observe(Some(SpokenMode::Quiet)), None);

        t.reset();
        assert_eq!(t.observe(Some(SpokenMode::Aware)), None, "after reconnecting");
        t.observe(None);
        assert_eq!(t.observe(Some(SpokenMode::Quiet)), None, "after a gap in readings");
    }

    #[test]
    fn the_same_mode_read_again_says_nothing() {
        let mut t = ModeTracker::default();
        t.observe(Some(SpokenMode::Quiet));
        for _ in 0..5 {
            assert_eq!(t.observe(Some(SpokenMode::Quiet)), None);
        }
    }

    /// A change is announced once, however many times it is then re-read.
    #[test]
    fn a_change_is_announced_exactly_once() {
        let mut t = ModeTracker::default();
        t.observe(Some(SpokenMode::Quiet));
        let said: Vec<_> = (0..4)
            .filter_map(|_| t.observe(Some(SpokenMode::Aware)))
            .collect();
        assert_eq!(said, vec![Announcement::Mode(SpokenMode::Aware)]);
    }

    #[test]
    fn switching_between_two_custom_modes_is_a_change_but_re_reading_one_is_not() {
        let focus = || SpokenMode::Custom(Some("Focus".into()));
        let commute = || SpokenMode::Custom(Some("Commute".into()));
        let mut t = ModeTracker::default();
        t.observe(Some(focus()));
        assert_eq!(t.observe(Some(focus())), None);
        assert_eq!(t.observe(Some(commute())), Some(Announcement::Mode(commute())));
    }

    /// A queued "Quiet mode activated" must not be spoken once the headphones
    /// have moved on to something else, or gone away.
    #[test]
    fn a_mode_announcement_goes_stale_when_the_mode_moves_on() {
        let mut t = ModeTracker::default();
        t.observe(Some(SpokenMode::Aware));
        let queued = t.observe(Some(SpokenMode::Quiet)).unwrap();
        assert!(t.still_true(&queued));

        t.observe(Some(SpokenMode::Aware));
        assert!(!t.still_true(&queued), "the headphones are in Aware now");

        t.observe(Some(SpokenMode::Quiet));
        t.reset();
        assert!(!t.still_true(&queued), "the headphones are gone");
    }

    #[test]
    fn other_announcements_do_not_depend_on_the_mode() {
        let t = ModeTracker::default();
        assert!(t.still_true(&Announcement::Test));
        assert!(t.still_true(&Announcement::LowBattery { percent: 9 }));
        assert!(t.still_true(&Announcement::Connected { name: "A".into() }));
    }

    // --- what counts as a confirmed mode ------------------------------------

    #[test]
    fn a_connected_real_snapshot_carries_a_confirmed_mode() {
        let snap = snapshot(
            DeviceSource::Real,
            ConnectionState::Connected,
            Some(NoiseControlMode::Aware),
        );
        assert_eq!(confirmed_mode(&snap), Some(SpokenMode::Aware));
    }

    /// The simulated device has modes too. They are not hardware states.
    #[test]
    fn a_simulated_mode_is_never_a_confirmed_mode() {
        let snap = snapshot(
            DeviceSource::Mock,
            ConnectionState::Connected,
            Some(NoiseControlMode::Quiet),
        );
        assert_eq!(confirmed_mode(&snap), None);
    }

    #[test]
    fn nothing_is_confirmed_while_disconnected_or_in_between() {
        for connection in [
            ConnectionState::Disconnected,
            ConnectionState::Connecting,
            ConnectionState::Reconnecting,
            ConnectionState::Discovering,
            ConnectionState::Error,
        ] {
            let snap = snapshot(DeviceSource::Real, connection, Some(NoiseControlMode::Quiet));
            assert_eq!(confirmed_mode(&snap), None, "{connection:?}");
        }
        let no_reading = snapshot(DeviceSource::Real, ConnectionState::Connected, None);
        assert_eq!(confirmed_mode(&no_reading), None);
    }

    #[test]
    fn a_custom_mode_keeps_the_headphones_name_for_it() {
        let named = state(NoiseControlMode::Custom, Some("Focus"));
        assert_eq!(
            SpokenMode::from_state(&named),
            Some(SpokenMode::Custom(Some("Focus".into())))
        );
        assert_eq!(SpokenMode::from_state(&state(NoiseControlMode::Off, None)), None);
    }
}
