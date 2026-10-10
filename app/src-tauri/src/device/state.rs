//! Device state types.
//!
//! Every reading carries its provenance. A battery percentage read from a
//! Windows PnP property is a materially different claim from one parsed out of
//! a vendor protocol frame, and a different claim again from a simulated value.
//! The UI displays the distinction rather than flattening it.

use super::capability::CapabilitySet;
use serde::{Deserialize, Serialize};

/// Which implementation is backing the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceSource {
    /// Every value is fabricated. The UI must label this SIMULATED.
    Mock,
    /// Values come from real hardware.
    Real,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Disconnected,
    Discovering,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    None,
    BluetoothClassic,
    BluetoothLe,
    WindowsAudioEndpoint,
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub name: String,
    /// Opaque, stable identifier derived by hashing the Bluetooth address with
    /// a per-installation salt. The raw address never leaves the native layer,
    /// so exported diagnostics cannot be used to track the hardware.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BatterySource {
    /// Windows PnP device property. Verified to work as a mechanism on this
    /// machine; whether a given device populates it is per-device.
    WindowsPnp,
    /// Standard BLE Battery Service (0x180F) characteristic 0x2A19.
    BleBatteryService,
    /// Parsed from a vendor protocol frame.
    VendorProtocol,
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryReading {
    /// 0-100.
    pub percent: u8,
    pub source: BatterySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charging: Option<bool>,
    pub read_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseControlMode {
    Quiet,
    Aware,
    Custom,
    Off,
}

impl NoiseControlMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Aware => "aware",
            Self::Custom => "custom",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateSource {
    VendorProtocol,
    SoftwareDsp,
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoiseControlState {
    pub mode: NoiseControlMode,
    /// Noise-cancellation strength of the active mode, 0-10 where 10 is the
    /// most. Present only when the device reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    /// The device's own name for the active mode, when it has one — a custom
    /// mode's name. The presets are named by `mode`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub source: StateSource,
    pub read_at: String,
}

/// EQ band gains, in whole dB. Range is validated at the command layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqSettings {
    pub bass: i8,
    pub mid: i8,
    pub treble: i8,
}

impl EqSettings {
    pub const MIN_DB: i8 = -10;
    pub const MAX_DB: i8 = 10;

    pub fn flat() -> Self {
        Self {
            bass: 0,
            mid: 0,
            treble: 0,
        }
    }

    pub fn is_within_range(&self) -> bool {
        [self.bass, self.mid, self.treble]
            .iter()
            .all(|v| (Self::MIN_DB..=Self::MAX_DB).contains(v))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqState {
    #[serde(flatten)]
    pub settings: EqSettings,
    pub source: StateSource,
    pub read_at: String,
}

/// Windows audio endpoint state. Deliberately separate from any Bose-internal
/// volume: they are different mechanisms and the UI never conflates them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsAudioState {
    pub endpoint_name: String,
    pub endpoint_id: String,
    /// 0-100, Windows system volume for this endpoint.
    pub volume_percent: u8,
    pub muted: bool,
    pub is_default_render: bool,
    pub is_default_communications: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
}

/// The complete picture handed to the frontend.
///
/// Optional fields are `None` unless a real reading was obtained. They are
/// never populated with plausible-looking defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub source: DeviceSource,
    pub connection: ConnectionState,
    pub transport: Transport,
    pub identity: Option<DeviceIdentity>,
    pub capabilities: CapabilitySet,
    pub battery: Option<BatteryReading>,
    pub noise_control: Option<NoiseControlState>,
    pub equalizer: Option<EqState>,
    pub windows_audio: Option<WindowsAudioState>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl DeviceSnapshot {
    /// A snapshot representing "we have nothing yet".
    pub fn empty(source: DeviceSource) -> Self {
        Self {
            source,
            connection: ConnectionState::Disconnected,
            transport: Transport::None,
            identity: None,
            capabilities: CapabilitySet::all_unknown("No device has been interrogated yet."),
            battery: None,
            noise_control: None,
            equalizer: None,
            windows_audio: None,
            last_error: None,
            updated_at: super::now_rfc3339(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_asserts_nothing_about_the_device() {
        let snap = DeviceSnapshot::empty(DeviceSource::Real);
        assert!(snap.battery.is_none());
        assert!(snap.noise_control.is_none());
        assert!(snap.equalizer.is_none());
        assert!(snap.identity.is_none());
        assert_eq!(snap.connection, ConnectionState::Disconnected);
        assert_eq!(snap.capabilities.verified_count(), 0);
    }

    #[test]
    fn eq_range_validation() {
        assert!(EqSettings::flat().is_within_range());
        assert!(EqSettings {
            bass: 10,
            mid: -10,
            treble: 0
        }
        .is_within_range());
        assert!(!EqSettings {
            bass: 11,
            mid: 0,
            treble: 0
        }
        .is_within_range());
        assert!(!EqSettings {
            bass: 0,
            mid: 0,
            treble: -11
        }
        .is_within_range());
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let snap = DeviceSnapshot::empty(DeviceSource::Mock);
        let json = serde_json::to_string(&snap).unwrap();
        let back: DeviceSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }
}
