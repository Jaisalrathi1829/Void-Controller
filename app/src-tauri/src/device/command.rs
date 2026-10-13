//! The typed command layer.
//!
//! This is the *only* way the frontend can cause anything to happen to the
//! hardware. Note what is absent: there is no variant carrying a UUID, a byte
//! array, or a raw payload. The frontend cannot express "write these bytes to
//! that characteristic", because no such command exists in the type system.
//!
//! Every command is validated here before any backend sees it.

use super::state::{check_device_name, is_paired_device_id, EqSettings, NoiseControlMode};
use super::{DeviceError, DeviceResult};
use serde::{Deserialize, Serialize};

/// The complete allowlist of operations the UI may request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DeviceCommand {
    // --- Reads -------------------------------------------------------------
    RefreshSnapshot,
    ReadBattery,
    ReadNoiseControl,
    ReadEqualizer,
    ReadDeviceInfo,

    // --- Connection --------------------------------------------------------
    Connect,
    Disconnect,
    Reconnect,

    // --- Windows audio (system-side, not device-internal) -------------------
    #[serde(rename_all = "camelCase")]
    SetSystemVolume {
        percent: u8,
    },
    #[serde(rename_all = "camelCase")]
    SetSystemMute {
        muted: bool,
    },

    // --- Media transport, via the Windows session model ---------------------
    MediaPlayPause,
    MediaNext,
    MediaPrevious,

    // --- Device-internal features (vendor protocol) -------------------------
    #[serde(rename_all = "camelCase")]
    SetNoiseControl {
        mode: NoiseControlMode,
    },
    #[serde(rename_all = "camelCase")]
    SetNoiseControlLevel {
        level: u8,
    },
    #[serde(rename_all = "camelCase")]
    SetEqualizer {
        settings: EqSettings,
    },
    /// Whether the headphones stay connected to two devices at once.
    #[serde(rename_all = "camelCase")]
    SetMultipoint {
        enabled: bool,
    },
    /// What the headphones are called. Text, checked before anything sees
    /// it: see [`check_device_name`].
    #[serde(rename_all = "camelCase")]
    SetDeviceName {
        name: String,
    },
    /// Connect the headphones to one of the devices they are paired with, or
    /// disconnect them from it. The device is named by the id the snapshot
    /// gave for it — never by an address, which the interface does not have.
    #[serde(rename_all = "camelCase")]
    SetPairedDeviceConnection {
        device: String,
        connected: bool,
    },
}

impl DeviceCommand {
    /// Validates the command's parameters.
    ///
    /// Called before dispatch, unconditionally, for both mock and real
    /// backends — so the mock exercises exactly the same validation the real
    /// hardware path does.
    pub fn validate(&self) -> DeviceResult<()> {
        match self {
            Self::SetSystemVolume { percent } => {
                if *percent > 100 {
                    return Err(DeviceError::InvalidInput(format!(
                        "volume must be 0-100, got {percent}"
                    )));
                }
            }
            Self::SetNoiseControlLevel { level } => {
                if *level > 10 {
                    return Err(DeviceError::InvalidInput(format!(
                        "noise control level must be 0-10, got {level}"
                    )));
                }
            }
            Self::SetEqualizer { settings } => {
                if !settings.is_within_range() {
                    return Err(DeviceError::InvalidInput(format!(
                        "EQ gains must be within {}..={} dB, got bass={} mid={} treble={}",
                        EqSettings::MIN_DB,
                        EqSettings::MAX_DB,
                        settings.bass,
                        settings.mid,
                        settings.treble
                    )));
                }
            }
            Self::SetDeviceName { name } => {
                check_device_name(name).map_err(DeviceError::InvalidInput)?;
            }
            Self::SetPairedDeviceConnection { device, .. } => {
                if !is_paired_device_id(device) {
                    return Err(DeviceError::InvalidInput(
                        "that is not one of the headphones' paired devices".to_string(),
                    ));
                }
            }
            // Reads, connection changes, media keys, mute and the multipoint
            // switch carry no parameters that can be out of range.
            _ => {}
        }
        Ok(())
    }

    /// Whether this command changes device or system state.
    /// Used to decide whether a result needs state verification.
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            Self::SetSystemVolume { .. }
                | Self::SetSystemMute { .. }
                | Self::SetNoiseControl { .. }
                | Self::SetNoiseControlLevel { .. }
                | Self::SetEqualizer { .. }
                | Self::SetMultipoint { .. }
                | Self::SetDeviceName { .. }
                | Self::SetPairedDeviceConnection { .. }
                | Self::MediaPlayPause
                | Self::MediaNext
                | Self::MediaPrevious
                | Self::Connect
                | Self::Disconnect
                | Self::Reconnect
        )
    }

    /// Short stable name for logging. Never includes payload details, so that
    /// ordinary logs stay free of device data.
    pub fn name(&self) -> &'static str {
        match self {
            Self::RefreshSnapshot => "refresh_snapshot",
            Self::ReadBattery => "read_battery",
            Self::ReadNoiseControl => "read_noise_control",
            Self::ReadEqualizer => "read_equalizer",
            Self::ReadDeviceInfo => "read_device_info",
            Self::Connect => "connect",
            Self::Disconnect => "disconnect",
            Self::Reconnect => "reconnect",
            Self::SetSystemVolume { .. } => "set_system_volume",
            Self::SetSystemMute { .. } => "set_system_mute",
            Self::MediaPlayPause => "media_play_pause",
            Self::MediaNext => "media_next",
            Self::MediaPrevious => "media_previous",
            Self::SetNoiseControl { .. } => "set_noise_control",
            Self::SetNoiseControlLevel { .. } => "set_noise_control_level",
            Self::SetEqualizer { .. } => "set_equalizer",
            Self::SetMultipoint { .. } => "set_multipoint",
            Self::SetDeviceName { .. } => "set_device_name",
            Self::SetPairedDeviceConnection { .. } => "set_paired_device_connection",
        }
    }
}

/// The result of a command.
///
/// The distinction between `Applied` and `SentUnverified` is the heart of the
/// state verification rule: we only report success when the device's own
/// reported state confirmed it.
///
/// Variant names are kebab-case on the wire (`sent-unverified`), matching the
/// `CommandOutcome` union in `app/frontend/src/types/device.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CommandOutcome {
    /// The device's reported state was re-read and matches what we asked for.
    #[serde(rename_all = "camelCase")]
    Applied { verified_at: String },

    /// The command was transmitted, but the device gave us no evidence of the
    /// outcome. The UI must say "Command sent. State could not be verified."
    #[serde(rename_all = "camelCase")]
    SentUnverified { reason: String },

    #[serde(rename_all = "camelCase")]
    Rejected { reason: String },

    #[serde(rename_all = "camelCase")]
    Unsupported { reason: String },
}

impl CommandOutcome {
    pub fn applied() -> Self {
        Self::Applied {
            verified_at: super::now_rfc3339(),
        }
    }

    pub fn sent_unverified(reason: impl Into<String>) -> Self {
        Self::SentUnverified {
            reason: reason.into(),
        }
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    /// True only for a confirmed state change. Deliberately narrow.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The interface can name a paired device only by the id it was given.
    /// Anything shaped like an address, or like anything else, stops here.
    #[test]
    fn a_paired_device_is_named_by_its_id_and_nothing_else() {
        let ask = |device: &str| DeviceCommand::SetPairedDeviceConnection {
            device: device.into(),
            connected: false,
        };
        assert!(ask("0123456789abcdef0123").validate().is_ok());
        for bad in [
            "",
            "E4:58:BC:F9:F0:2E",
            "e458bcf9f02e",
            "0123456789ABCDEF0123",
            "0123456789abcdef01234",
            "0123456789abcdef012",
            "0123456789abcdef012g",
            "04 02 05 06 34 11 22",
        ] {
            assert!(ask(bad).validate().is_err(), "{bad:?}");
        }
        let cmd: DeviceCommand = serde_json::from_str(
            r#"{"kind":"setPairedDeviceConnection","device":"0123456789abcdef0123","connected":true}"#,
        )
        .unwrap();
        assert!(cmd.is_mutating());
        assert_eq!(cmd.name(), "set_paired_device_connection");
    }

    #[test]
    fn a_device_name_is_checked_before_anything_sees_it() {
        let name = |name: &str| DeviceCommand::SetDeviceName { name: name.into() };
        assert!(name("Aurora").validate().is_ok());
        assert!(name("  Aurora  ").validate().is_ok(), "trimmed where it is used");
        assert!(name("").validate().is_err());
        assert!(name("   ").validate().is_err());
        assert!(name(&"x".repeat(31)).validate().is_ok());
        assert!(name(&"x".repeat(32)).validate().is_err());
        assert!(name("two\nlines").validate().is_err());
    }

    #[test]
    fn the_new_commands_change_the_device_and_log_no_payload() {
        let rename = DeviceCommand::SetDeviceName { name: "Private name".into() };
        let multipoint = DeviceCommand::SetMultipoint { enabled: false };
        assert!(rename.is_mutating() && multipoint.is_mutating());
        assert_eq!(rename.name(), "set_device_name");
        assert_eq!(multipoint.name(), "set_multipoint");
        assert!(!rename.name().contains("Private"));
    }

    /// What the interface sends, as it sends it.
    #[test]
    fn the_new_commands_are_read_from_the_interfaces_json() {
        let multipoint: DeviceCommand =
            serde_json::from_str(r#"{"kind":"setMultipoint","enabled":false}"#).unwrap();
        assert_eq!(multipoint, DeviceCommand::SetMultipoint { enabled: false });
        let rename: DeviceCommand =
            serde_json::from_str(r#"{"kind":"setDeviceName","name":"Nova"}"#).unwrap();
        assert_eq!(rename, DeviceCommand::SetDeviceName { name: "Nova".into() });
        // A switch is on or off; it cannot carry anything else.
        assert!(serde_json::from_str::<DeviceCommand>(r#"{"kind":"setMultipoint","enabled":2}"#).is_err());
        assert!(serde_json::from_str::<DeviceCommand>(r#"{"kind":"setDeviceName","name":[1,2]}"#).is_err());
    }

    #[test]
    fn volume_above_100_is_rejected() {
        let cmd = DeviceCommand::SetSystemVolume { percent: 101 };
        assert!(cmd.validate().is_err());
    }

    #[test]
    fn volume_bounds_are_inclusive() {
        assert!(DeviceCommand::SetSystemVolume { percent: 0 }
            .validate()
            .is_ok());
        assert!(DeviceCommand::SetSystemVolume { percent: 100 }
            .validate()
            .is_ok());
    }

    #[test]
    fn out_of_range_eq_is_rejected() {
        let cmd = DeviceCommand::SetEqualizer {
            settings: EqSettings {
                bass: 50,
                mid: 0,
                treble: 0,
            },
        };
        let err = cmd.validate().unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput(_)));
    }

    #[test]
    fn in_range_eq_is_accepted() {
        let cmd = DeviceCommand::SetEqualizer {
            settings: EqSettings {
                bass: -10,
                mid: 3,
                treble: 10,
            },
        };
        assert!(cmd.validate().is_ok());
    }

    #[test]
    fn noise_level_bounds() {
        assert!(DeviceCommand::SetNoiseControlLevel { level: 10 }
            .validate()
            .is_ok());
        assert!(DeviceCommand::SetNoiseControlLevel { level: 11 }
            .validate()
            .is_err());
    }

    #[test]
    fn reads_are_not_mutating() {
        assert!(!DeviceCommand::ReadBattery.is_mutating());
        assert!(!DeviceCommand::RefreshSnapshot.is_mutating());
        assert!(DeviceCommand::SetNoiseControl {
            mode: NoiseControlMode::Aware
        }
        .is_mutating());
    }

    #[test]
    fn sent_unverified_is_not_treated_as_success() {
        let outcome = CommandOutcome::sent_unverified("device did not echo the new state");
        assert!(!outcome.is_confirmed());
    }

    /// Guards the security property that motivates this whole module: no
    /// command variant may carry a raw payload the frontend controls. If
    /// someone adds one, this test is where they should be forced to think
    /// about it.
    #[test]
    fn command_json_surface_contains_no_raw_byte_payloads() {
        let samples = vec![
            DeviceCommand::ReadBattery,
            DeviceCommand::SetSystemVolume { percent: 50 },
            DeviceCommand::SetNoiseControl {
                mode: NoiseControlMode::Quiet,
            },
            DeviceCommand::SetEqualizer {
                settings: EqSettings::flat(),
            },
        ];
        for cmd in samples {
            let json = serde_json::to_string(&cmd).unwrap();
            assert!(
                !json.contains("uuid") && !json.contains("bytes") && !json.contains("payload"),
                "command {json} exposes a raw protocol surface to the frontend"
            );
        }
    }
}
