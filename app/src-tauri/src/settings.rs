//! Application settings, stored locally as a small JSON file.
//!
//! These have to live natively rather than in the webview: "start minimized"
//! and "minimize to tray" are acted on before any page has loaded, and the
//! background watcher reads the notification switches while the window may be
//! hidden.
//!
//! One setting is deliberately *not* stored here. Whether the application
//! starts with Windows is a fact about Windows' startup list, so it is read
//! from there each time (see [`crate::autostart`]) instead of being remembered
//! separately and drifting out of step with it.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The settings kept in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredSettings {
    /// Open hidden (in the tray) or minimized instead of on screen.
    pub start_minimized: bool,
    /// Closing or minimizing the window hides it to a tray icon.
    pub minimize_to_tray: bool,
    /// Attach to the preferred headphones at launch without being asked.
    pub auto_connect: bool,
    /// Re-attach when the headphones come back after going away.
    pub auto_reconnect: bool,
    pub notify_connected: bool,
    pub notify_disconnected: bool,
    pub notify_low_battery: bool,
    /// The headphones this application attaches to: their Windows instance
    /// id, which contains the Bluetooth address. It stays in this file and in
    /// the native layer; the UI is only ever given the name.
    pub preferred_device_id: Option<String>,
    pub preferred_device_name: Option<String>,
    /// What those headphones last said they are called, read from them over
    /// the control link. Where there is one it is the name this application
    /// goes by: the system keeps its own, which stays the old one for some
    /// time after the headphones are renamed.
    pub reported_device_name: Option<String>,

    /// Speak announcements at all.
    pub voice_enabled: bool,
    /// The Kokoro voice, by the model's own name for it.
    pub voice_id: String,
    /// Speaking rate, 100 = the model's natural pace.
    pub voice_speed_percent: u16,
    /// Loudness of speech, as a share of the level the model produces. Applied
    /// to the speech only; Windows' volume is never touched.
    pub voice_volume_percent: u8,
    pub announce_mode_changes: bool,
    pub announce_connection: bool,
    pub announce_battery: bool,
    /// When the headphones' own output is not available, speak through
    /// Windows' default output instead of staying silent.
    pub voice_use_default_output: bool,

    /// Show the desktop widget: a small window of its own with the modes and
    /// the equalizer, so that the main window need not be opened for them.
    pub desktop_widget: bool,
    /// Keep the widget above other windows.
    pub widget_on_top: bool,
    /// Where the widget was last put, in screen pixels. Checked against the
    /// screens that are actually there before it is used.
    pub widget_position: Option<(i32, i32)>,
}

/// Speaking rates offered, as percentages of the model's natural pace.
pub const VOICE_SPEED_RANGE: std::ops::RangeInclusive<u16> = 50..=200;

impl Default for StoredSettings {
    /// Attaching to the headphones is the point of the application, so both
    /// connection switches start on. Everything that changes how the window
    /// behaves, or interrupts with a notification, starts off.
    fn default() -> Self {
        Self {
            start_minimized: false,
            minimize_to_tray: false,
            auto_connect: true,
            auto_reconnect: true,
            notify_connected: false,
            notify_disconnected: false,
            notify_low_battery: false,
            preferred_device_id: None,
            preferred_device_name: None,
            reported_device_name: None,
            // Speaking aloud is not something to start doing uninvited. Once
            // it is switched on, every kind of announcement is on with it.
            voice_enabled: false,
            voice_id: crate::voice::assets::DEFAULT_VOICE.to_string(),
            voice_speed_percent: 100,
            voice_volume_percent: 80,
            announce_mode_changes: true,
            announce_connection: true,
            announce_battery: true,
            // Speech meant for the headphones does not come out of the
            // laptop's speakers unless that was asked for.
            voice_use_default_output: false,
            // A window that appears on the desktop uninvited would be an
            // imposition. Once asked for, it floats, which is what a widget
            // is for; that can be switched off on the widget itself.
            desktop_widget: false,
            widget_on_top: true,
            widget_position: None,
        }
    }
}

impl StoredSettings {
    /// Brings values read from disk back into range. A settings file edited
    /// by hand, or written by a build with a different voice list, must not
    /// be able to ask for a voice that does not exist or a speed of zero.
    pub fn sanitized(mut self) -> Self {
        if crate::voice::assets::voice(&self.voice_id).is_none() {
            self.voice_id = crate::voice::assets::DEFAULT_VOICE.to_string();
        }
        self.voice_speed_percent = self
            .voice_speed_percent
            .clamp(*VOICE_SPEED_RANGE.start(), *VOICE_SPEED_RANGE.end());
        self.voice_volume_percent = self.voice_volume_percent.min(100);
        self
    }
}

/// The switches the UI can flip. A closed set: a request naming anything else
/// does not deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingKey {
    StartWithWindows,
    StartMinimized,
    MinimizeToTray,
    AutoConnect,
    AutoReconnect,
    NotifyConnected,
    NotifyDisconnected,
    NotifyLowBattery,
    VoiceEnabled,
    AnnounceModeChanges,
    AnnounceConnection,
    AnnounceBattery,
    VoiceUseDefaultOutput,
    DesktopWidget,
    WidgetOnTop,
}

impl StoredSettings {
    /// Sets one of the stored switches. `StartWithWindows` is not stored and
    /// is handled by the caller; asking for it here changes nothing.
    pub fn set(&mut self, key: SettingKey, value: bool) {
        match key {
            SettingKey::StartWithWindows => {}
            SettingKey::StartMinimized => self.start_minimized = value,
            SettingKey::MinimizeToTray => self.minimize_to_tray = value,
            SettingKey::AutoConnect => self.auto_connect = value,
            SettingKey::AutoReconnect => self.auto_reconnect = value,
            SettingKey::NotifyConnected => self.notify_connected = value,
            SettingKey::NotifyDisconnected => self.notify_disconnected = value,
            SettingKey::NotifyLowBattery => self.notify_low_battery = value,
            SettingKey::VoiceEnabled => self.voice_enabled = value,
            SettingKey::AnnounceModeChanges => self.announce_mode_changes = value,
            SettingKey::AnnounceConnection => self.announce_connection = value,
            SettingKey::AnnounceBattery => self.announce_battery = value,
            SettingKey::VoiceUseDefaultOutput => self.voice_use_default_output = value,
            SettingKey::DesktopWidget => self.desktop_widget = value,
            SettingKey::WidgetOnTop => self.widget_on_top = value,
        }
    }
}

/// Settings in memory, written through to disk on every change.
pub struct SettingsStore {
    path: Option<PathBuf>,
    current: RwLock<StoredSettings>,
}

impl SettingsStore {
    /// Loads settings from `path`. A missing file means first run; a file
    /// that cannot be read as settings is left where it is and defaults are
    /// used, so a damaged file is never silently overwritten at startup.
    pub fn load(path: PathBuf) -> Self {
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<StoredSettings>(&text).ok())
            .unwrap_or_default()
            .sanitized();
        Self {
            path: Some(path),
            current: RwLock::new(current),
        }
    }

    /// Settings that are never written anywhere. Used when no settings
    /// directory is available, and in tests.
    pub fn in_memory(settings: StoredSettings) -> Self {
        Self {
            path: None,
            current: RwLock::new(settings),
        }
    }

    pub fn get(&self) -> StoredSettings {
        self.current.read().clone()
    }

    /// Applies a change and saves it. The in-memory value is updated even if
    /// the save fails, and the failure is returned so it can be reported: the
    /// setting works for this run but will not survive a restart.
    pub fn update(&self, change: impl FnOnce(&mut StoredSettings)) -> Result<StoredSettings, String> {
        let snapshot = {
            let mut current = self.current.write();
            change(&mut current);
            *current = current.clone().sanitized();
            current.clone()
        };
        match &self.path {
            Some(path) => save(path, &snapshot).map(|()| snapshot),
            None => Ok(snapshot),
        }
    }
}

/// Writes to a temporary file and renames it over the real one, so a crash
/// mid-write cannot leave half a settings file behind.
fn save(path: &Path, settings: &StoredSettings) -> Result<(), String> {
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("could not encode settings: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create the settings folder: {e}"))?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|e| format!("could not write settings: {e}"))?;
    std::fs::rename(&temp, path).map_err(|e| format!("could not save settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_run_connects_by_default_and_stays_quiet() {
        let s = StoredSettings::default();
        assert!(s.auto_connect && s.auto_reconnect);
        assert!(!s.start_minimized && !s.minimize_to_tray);
        assert!(!s.notify_connected && !s.notify_disconnected && !s.notify_low_battery);
        assert!(s.preferred_device_id.is_none());
    }

    #[test]
    fn settings_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");

        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get(), StoredSettings::default(), "no file yet");
        store
            .update(|s| {
                s.set(SettingKey::MinimizeToTray, true);
                s.set(SettingKey::AutoConnect, false);
                s.preferred_device_name = Some("Aurora".into());
            })
            .unwrap();

        let reopened = SettingsStore::load(path).get();
        assert!(reopened.minimize_to_tray);
        assert!(!reopened.auto_connect);
        assert_eq!(reopened.preferred_device_name.as_deref(), Some("Aurora"));
        assert!(reopened.auto_reconnect, "untouched settings keep their value");
    }

    /// A file from an older build, missing newer fields, still loads: the
    /// missing ones take their defaults rather than failing the whole file.
    #[test]
    fn a_partial_file_fills_in_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"minimizeToTray": true}"#).unwrap();

        let s = SettingsStore::load(path).get();
        assert!(s.minimize_to_tray);
        assert!(s.auto_connect, "default");
    }

    #[test]
    fn a_damaged_file_gives_defaults_and_is_not_overwritten_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();

        let store = SettingsStore::load(path.clone());
        assert_eq!(store.get(), StoredSettings::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    /// Every switch off, so that turning one on is visible.
    fn all_off() -> StoredSettings {
        StoredSettings {
            auto_connect: false,
            auto_reconnect: false,
            announce_mode_changes: false,
            announce_connection: false,
            announce_battery: false,
            widget_on_top: false,
            ..StoredSettings::default()
        }
    }

    fn flags(s: &StoredSettings) -> [bool; 14] {
        [
            s.start_minimized,
            s.minimize_to_tray,
            s.auto_connect,
            s.auto_reconnect,
            s.notify_connected,
            s.notify_disconnected,
            s.notify_low_battery,
            s.voice_enabled,
            s.announce_mode_changes,
            s.announce_connection,
            s.announce_battery,
            s.voice_use_default_output,
            s.desktop_widget,
            s.widget_on_top,
        ]
    }

    #[test]
    fn every_stored_key_sets_its_own_field() {
        let keys = [
            SettingKey::StartMinimized,
            SettingKey::MinimizeToTray,
            SettingKey::AutoConnect,
            SettingKey::AutoReconnect,
            SettingKey::NotifyConnected,
            SettingKey::NotifyDisconnected,
            SettingKey::NotifyLowBattery,
            SettingKey::VoiceEnabled,
            SettingKey::AnnounceModeChanges,
            SettingKey::AnnounceConnection,
            SettingKey::AnnounceBattery,
            SettingKey::VoiceUseDefaultOutput,
            SettingKey::DesktopWidget,
            SettingKey::WidgetOnTop,
        ];
        let mut seen = std::collections::HashSet::new();
        for key in keys {
            let mut on = all_off();
            on.set(key, true);
            let set: Vec<usize> = flags(&on)
                .iter()
                .enumerate()
                .filter(|(_, f)| **f)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(set.len(), 1, "{key:?} set {set:?}");
            assert!(seen.insert(set[0]), "{key:?} shares a field with another key");
        }
    }

    #[test]
    fn voice_feedback_starts_off_with_every_kind_of_announcement_ready() {
        let s = StoredSettings::default();
        assert!(!s.voice_enabled, "nothing speaks until it is switched on");
        assert!(s.announce_mode_changes && s.announce_connection && s.announce_battery);
        assert!(!s.voice_use_default_output);
        assert_eq!(s.voice_speed_percent, 100);
        assert!(crate::voice::assets::voice(&s.voice_id).is_some());
    }

    /// A settings file is a text file; someone may edit it.
    #[test]
    fn out_of_range_voice_settings_are_brought_back_into_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"voiceId": "no_such_voice", "voiceSpeedPercent": 0, "voiceVolumePercent": 250}"#,
        )
        .unwrap();

        let s = SettingsStore::load(path).get();
        assert_eq!(s.voice_id, crate::voice::assets::DEFAULT_VOICE);
        assert_eq!(s.voice_speed_percent, 50);
        assert_eq!(s.voice_volume_percent, 100);

        // And the same through an update.
        let store = SettingsStore::in_memory(StoredSettings::default());
        let after = store
            .update(|s| {
                s.voice_speed_percent = 9000;
                s.voice_id = "another_models_voice".into();
            })
            .unwrap();
        assert_eq!(after.voice_speed_percent, 200);
        assert_eq!(after.voice_id, crate::voice::assets::DEFAULT_VOICE);
    }

    /// The widget is off until asked for, floats once it is, and comes
    /// back where it was left.
    #[test]
    fn the_widget_is_off_by_default_and_remembers_where_it_was_put() {
        let s = StoredSettings::default();
        assert!(!s.desktop_widget);
        assert!(s.widget_on_top);
        assert_eq!(s.widget_position, None);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(path.clone());
        store
            .update(|s| {
                s.set(SettingKey::DesktopWidget, true);
                s.widget_position = Some((-1200, 340));
            })
            .unwrap();
        let reopened = SettingsStore::load(path).get();
        assert!(reopened.desktop_widget);
        assert_eq!(reopened.widget_position, Some((-1200, 340)), "a second screen to the left");
    }

    /// The startup-list switch is not a stored setting.
    #[test]
    fn start_with_windows_is_not_kept_in_the_file() {
        let mut s = StoredSettings::default();
        s.set(SettingKey::StartWithWindows, true);
        assert_eq!(s, StoredSettings::default());
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("startWithWindows"), "{json}");
    }

    #[test]
    fn setting_names_are_a_closed_set() {
        let parse = |json: &str| serde_json::from_str::<SettingKey>(json);
        assert_eq!(parse("\"minimizeToTray\"").unwrap(), SettingKey::MinimizeToTray);
        assert!(parse("\"preferredDeviceId\"").is_err());
        assert!(parse("\"somethingElse\"").is_err());
    }
}
