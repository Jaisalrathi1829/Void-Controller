//! Device abstraction: the boundary between the application and hardware.
//!
//! The frontend never touches Bluetooth. It issues typed, validated commands
//! that land here, and here they are dispatched to whichever backend is active
//! (mock or real). Swapping the real backend must not require restructuring
//! anything above this layer.

pub mod capability;
pub mod command;
pub mod state;
pub mod traits;

pub use capability::{
    Capability, CapabilityError, CapabilityKey, CapabilitySet, CapabilityStatus, HardwareProof,
    Mechanism,
};
pub use command::{CommandOutcome, DeviceCommand};
pub use state::{
    BatteryReading, BatterySource, ConnectionState, DeviceIdentity, DeviceSnapshot, DeviceSource,
    EqSettings, EqState, NoiseControlMode, NoiseControlState, Transport, WindowsAudioState,
};
pub use traits::BoseDevice;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Current time as an RFC3339 string. Used for every timestamped record so
/// diagnostics captures can be correlated with user-marked events.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Errors any device backend can produce.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no device is connected")]
    NotConnected,

    #[error("Bluetooth is unavailable or disabled on this system")]
    BluetoothUnavailable,

    #[error("the device is not reachable (powered off, out of range, or connected elsewhere)")]
    Unreachable,

    #[error("operation timed out after {0}ms")]
    Timeout(u64),

    #[error("{feature} is not supported on this device: {reason}")]
    Unsupported { feature: String, reason: String },

    #[error("the device returned a response we could not interpret: {0}")]
    InvalidResponse(String),

    #[error("command rejected: {0}")]
    Rejected(String),

    /// The control connection to the device failed. Says nothing about
    /// whether a request in flight took effect.
    #[error("the control connection to the headphones failed: {0}")]
    ControlLink(String),

    #[error("Windows API error: {0}")]
    Platform(String),

    /// The user has not let this application use Bluetooth. Nothing is
    /// wrong with the device or the radio; there is a permission to grant.
    #[error("{0}")]
    NotPermitted(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type DeviceResult<T> = Result<T, DeviceError>;
