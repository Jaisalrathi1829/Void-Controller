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
use super::command::{CommandOutcome, PlaybackControl};
use super::state::{
    BatteryReading, DeviceIdentity, DeviceSnapshot, DeviceSource, EqSettings, EqState,
    HeadphonesPlaying, NoiseControlMode, NoiseControlState,
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

    /// Whether the device is headphones or earbuds, where the backend knows.
    /// Costs nothing: nothing is asked of the device.
    fn reported_category(&self) -> Option<crate::bose::bmap::models::Category> {
        None
    }

    /// The battery level the device itself last reported, if the backend
    /// has one in hand. Costs nothing: nothing is asked of the device.
    fn reported_battery(&self) -> Option<u8> {
        None
    }

    /// The mode the device itself last reported, if the backend has one in
    /// hand. Costs nothing: nothing is asked of the device.
    ///
    /// After a change this is the device's own read-back, which is what
    /// makes it fit to announce: it is never what was merely asked for.
    fn reported_noise_control(&self) -> Option<NoiseControlState> {
        None
    }

    /// The names of the custom modes the device has listed, if the backend
    /// has the list in hand. Costs nothing, and says nothing about which mode
    /// is active.
    fn known_custom_modes(&self) -> Vec<String> {
        Vec::new()
    }

    /// The equalizer the device itself last reported, if the backend has one
    /// in hand. Costs nothing: nothing is asked of the device. It is as old
    /// as its `read_at` says, and is forgotten when the device goes.
    fn reported_equalizer(&self) -> Option<EqState> {
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

    /// Switches to one of the modes the device lists, by its slot. Same
    /// verification requirement as `set_noise_control`. A slot that is not
    /// one of the device's modes is refused before anything is sent.
    async fn select_noise_mode(&self, index: u8) -> DeviceResult<CommandOutcome>;

    /// Sets the noise-cancellation strength of the active mode, 0-10 where 10
    /// is the most. Same verification requirement as `set_noise_control`:
    /// `Applied` only when the device then reports that level.
    async fn set_noise_control_level(&self, level: u8) -> DeviceResult<CommandOutcome>;

    async fn equalizer(&self) -> DeviceResult<Option<EqState>>;

    /// Same verification requirement as `set_noise_control`.
    async fn set_equalizer(&self, settings: EqSettings) -> DeviceResult<CommandOutcome>;

    /// Switches multipoint: whether the device stays connected to two hosts
    /// at once. `Applied` only when the device then reports that state.
    ///
    /// With it going off the device keeps one connection, and that may not
    /// be this host's. A device that says "off" and then drops this
    /// connection has still applied the change.
    async fn set_multipoint(&self, enabled: bool) -> DeviceResult<CommandOutcome>;

    /// Sets what an earbud's touch shortcut does, to one of the actions the
    /// earbuds list for it. Applied only when they then report it.
    async fn set_shortcut(&self, action: u8) -> DeviceResult<CommandOutcome>;

    /// Whether the earbuds pause when taken out. Applied only when they then
    /// report it.
    async fn set_auto_pause(&self, enabled: bool) -> DeviceResult<CommandOutcome>;

    /// Whether the earbuds answer a call when put in. Applied only when they
    /// then report it.
    async fn set_auto_answer(&self, enabled: bool) -> DeviceResult<CommandOutcome>;

    /// Connects the device to one of the hosts it is paired with, or
    /// disconnects it from one. `device` is the id the snapshot gave for
    /// that host. `Applied` only when the device then reports it so — or,
    /// for the host that is asking being disconnected, when the device takes
    /// the request and drops this connection.
    async fn set_paired_connection(
        &self,
        device: &str,
        connected: bool,
    ) -> DeviceResult<CommandOutcome>;

    /// What the device is playing from — which of the devices it is
    /// connected to — and whether it is playing. `Ok(None)` when there is
    /// nothing to ask, it has no source, or it does not say.
    async fn playing(&self) -> DeviceResult<Option<HeadphonesPlaying>> {
        Ok(None)
    }

    /// Passes play/pause, next or previous on to the device it is playing
    /// from. `Applied` only for play or pause that the device then reports;
    /// a skip cannot be confirmed, the track not being known.
    async fn control_playback(&self, control: PlaybackControl) -> DeviceResult<CommandOutcome> {
        let _ = control;
        Ok(CommandOutcome::unsupported(
            "This backend cannot pass controls on through the headphones.",
        ))
    }

    /// Renames the device. `Applied` only when it then reports that name.
    async fn set_device_name(&self, name: &str) -> DeviceResult<CommandOutcome>;

    /// What the device itself last said it is called, if the backend has
    /// that in hand. Costs nothing. The host system keeps a name for it too,
    /// which can be an older one.
    fn reported_name(&self) -> Option<String> {
        None
    }
}
