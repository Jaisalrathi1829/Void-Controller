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
    /// The level Android's Bluetooth stack holds for the device, as the
    /// headphones reported it over the hands-free profile.
    AndroidBluetooth,
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
    /// Each part's own level, on a device that has more than one battery —
    /// earbuds and their case — as the device reported them. Empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<BatteryPart>,
}

/// One battery of a device that has several: an earbud, or the case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryPart {
    /// What the model's description calls it: "Left", "Right", "Case".
    pub name: String,
    /// 0-100.
    pub percent: u8,
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

/// Which model the device is, as far as Void knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelState {
    /// Bose's name for the model, from the catalogue.
    pub name: String,
    pub category: crate::bose::bmap::models::Category,
    pub support: crate::bose::bmap::models::Support,
}

/// One of the noise modes the device offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoiseModeOption {
    /// The slot the device lists it under. What switching to it asks for.
    pub index: u8,
    /// The device's own name for it.
    pub name: String,
    /// A fixed mode, rather than one the user set up.
    pub preset: bool,
    /// Its noise-cancellation level can be changed from here.
    pub adjustable: bool,
    /// The mode the device last said it is in.
    pub active: bool,
}

/// Whether the headphones will stay connected to two devices at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultipointState {
    pub enabled: bool,
    pub source: StateSource,
    pub read_at: String,
}

/// A device the headphones are paired with: a phone, a computer.
///
/// By name only. The headphones know each by its Bluetooth address, and that
/// is used in the native layer to ask for the name and goes no further.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDeviceState {
    /// Stands for the device when it is to be connected or disconnected. Not
    /// its address, and not derivable from it without this installation's
    /// salt; the native layer matches it against the headphones' own list.
    pub id: String,
    /// What the headphones call it. Empty if they have no name for it.
    pub name: String,
    pub connected: bool,
    /// The phone or computer this application is running on.
    pub this_device: bool,
}

/// Whether `id` has the shape of a paired device's id: twenty lowercase
/// hexadecimal digits. Says nothing about whether such a device exists —
/// that is settled against the headphones' list when it is used.
pub fn is_paired_device_id(id: &str) -> bool {
    id.len() == 20 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The longest name the headphones are given, in bytes of UTF-8.
pub const MAX_DEVICE_NAME_BYTES: usize = 31;

/// A name as it may be given to the headphones: what is left after trimming,
/// or why not. Not empty, no longer than [`MAX_DEVICE_NAME_BYTES`], and
/// nothing in it that is not a printing character.
pub fn check_device_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name cannot be empty.".into());
    }
    if name.chars().any(char::is_control) {
        return Err("A name cannot contain line breaks or control characters.".into());
    }
    if name.len() > MAX_DEVICE_NAME_BYTES {
        return Err(format!(
            "That name is too long: {} bytes, and the most is {MAX_DEVICE_NAME_BYTES}. A plain \
             letter is one byte; accented letters and emoji are more.",
            name.len()
        ));
    }
    Ok(name.to_string())
}

/// The host system's own volume: a Windows audio endpoint, or Android's media
/// volume. Deliberately separate from any Bose-internal volume: they are
/// different mechanisms and the UI never conflates them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemAudioState {
    /// What the volume belongs to: the Windows endpoint's name, or on Android
    /// the output media is playing through at the moment.
    pub endpoint_name: String,
    pub endpoint_id: String,
    /// 0-100. The Windows volume for this endpoint, or Android's media
    /// volume as a share of its steps.
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
    /// Which model the device is. `None` when it cannot be told.
    #[serde(default)]
    pub model: Option<ModelState>,
    /// The noise modes the device offers, in its own order. `None` when it
    /// has not said.
    #[serde(default)]
    pub noise_modes: Option<Vec<NoiseModeOption>>,
    /// Whether multipoint is on, as the headphones last said. `None` when
    /// they have not said, or have no such thing.
    #[serde(default)]
    pub multipoint: Option<MultipointState>,
    /// The devices the headphones are paired with and which are connected,
    /// as they last listed them. `None` when they have not.
    #[serde(default)]
    pub paired_devices: Option<Vec<PairedDeviceState>>,
    pub system_audio: Option<SystemAudioState>,
    /// What the phone or computer this application runs on is called — the
    /// thing the headphones are connected to when they are connected here.
    /// `None` when the system would not say.
    #[serde(default)]
    pub host_name: Option<String>,
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
            model: None,
            noise_modes: None,
            multipoint: None,
            paired_devices: None,
            system_audio: None,
            host_name: None,
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
