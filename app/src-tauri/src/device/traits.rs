//! The `BoseDevice` contract.
//!
//! Both the mock and the real backend implement this. The application layer
//! above holds a `Box<dyn BoseDevice>` and cannot tell which it has, other
//! than by reading `source()` — which is exactly what the UI uses to decide
//! whether to stamp everything SIMULATED.
//!
//! Note the shape of the mutating methods: they return `CommandOutcome`, not
//! `()`. A backend that cannot confirm a state change is *required* by the
//! type to say so.

use super::capability::CapabilitySet;
use super::command::CommandOutcome;
use super::state::{
    BatteryReading, DeviceIdentity, DeviceSnapshot, DeviceSource, EqSettings, EqState,
    NoiseControlMode, NoiseControlState,
};
use super::DeviceResult;
use async_trait::async_trait;

#[async_trait]
pub trait BoseDevice: Send + Sync {
    /// Whether this backend is mock or real. Drives the SIMULATED badge.
    fn source(&self) -> DeviceSource;

    /// Current capability set. Never fabricated: a backend that has not
    /// interrogated the device returns all-unknown.
    async fn capabilities(&self) -> CapabilitySet;

    /// A full state snapshot for the UI.
    async fn snapshot(&self) -> DeviceResult<DeviceSnapshot>;

    async fn connect(&self) -> DeviceResult<CommandOutcome>;
    async fn disconnect(&self) -> DeviceResult<CommandOutcome>;

    async fn device_info(&self) -> DeviceResult<Option<DeviceIdentity>>;

    /// `Ok(None)` means "we have no reading", which is different from an error
    /// and very different from zero.
    async fn battery(&self) -> DeviceResult<Option<BatteryReading>>;

    async fn noise_control(&self) -> DeviceResult<Option<NoiseControlState>>;

    /// The battery level the device itself last reported, if the backend
    /// has one in hand. Costs nothing: nothing is asked of the device.
    fn reported_battery(&self) -> Option<u8> {
        None
    }

    /// The mode, asked for as cheaply as the backend can: one question to the
    /// device and nothing else. For asking often.
    ///
    /// `Ok(None)` means there is nothing to ask right now. An error means the
    /// question was asked and not answered; it says nothing about the mode.
    /// The default has nothing to ask.
    async fn peek_noise_control(&self) -> DeviceResult<Option<NoiseControlState>> {
        Ok(None)
    }

    /// Implementations must re-read the device's own state and return
    /// `Applied` only when it matches `mode`.
    async fn set_noise_control(&self, mode: NoiseControlMode)
        -> DeviceResult<CommandOutcome>;

    /// Sets the noise-cancellation strength of the active mode, 0-10 where 10
    /// is the most. Same verification requirement as `set_noise_control`:
    /// `Applied` only when the device then reports that level.
    async fn set_noise_control_level(&self, level: u8) -> DeviceResult<CommandOutcome>;

    async fn equalizer(&self) -> DeviceResult<Option<EqState>>;

    /// Same verification requirement as `set_noise_control`.
    async fn set_equalizer(&self, settings: EqSettings) -> DeviceResult<CommandOutcome>;
}
