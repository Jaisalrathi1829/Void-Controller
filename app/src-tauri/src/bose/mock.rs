//! The simulated device.
//!
//! This exists so the entire application can be developed and regression-tested
//! without physical headphones. It is deliberately honest about what it is:
//! `source()` returns `Mock`, every reading carries a `Simulated` provenance
//! tag, and the UI stamps SIMULATED wherever these values appear.
//!
//! Two design decisions worth noting:
//!
//! 1. The mock reports its capabilities as `Experimental`, never `Verified`.
//!    A simulated device has not verified anything about real hardware, and
//!    letting the mock mint VERIFIED statuses would defeat the entire point of
//!    the capability system.
//!
//! 2. It simulates imperfection: `set_noise_control` occasionally returns
//!    `SentUnverified`, and reads can fail. Otherwise the UI's error and
//!    unverified paths would never be exercised until they met real hardware.

use crate::device::capability::{CapabilityKey, CapabilitySet, Mechanism};
use crate::device::command::CommandOutcome;
use crate::device::state::{
    BatteryReading, BatterySource, ConnectionState, DeviceIdentity, DeviceSnapshot, DeviceSource,
    EqSettings, EqState, NoiseControlMode, NoiseControlState, StateSource, Transport,
};
use crate::device::traits::BoseDevice;
use crate::device::{now_rfc3339, DeviceError, DeviceResult};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Tunables for simulating a less-than-perfect device.
#[derive(Debug, Clone, Copy)]
pub struct MockBehaviour {
    /// Every Nth mutating command returns `SentUnverified` instead of
    /// `Applied`, exercising the "could not verify" UI path. 0 disables.
    pub unverified_every: u64,
    /// Every Nth read fails with `Unreachable`, exercising error handling.
    /// 0 disables.
    pub read_failure_every: u64,
    /// Battery drains one point every N snapshot reads. 0 disables.
    pub drain_every: u64,
}

impl Default for MockBehaviour {
    fn default() -> Self {
        Self {
            unverified_every: 7,
            read_failure_every: 0,
            drain_every: 25,
        }
    }
}

impl MockBehaviour {
    /// Fully deterministic and always-succeeding. Used by tests that are
    /// asserting something other than failure handling.
    pub fn perfect() -> Self {
        Self {
            unverified_every: 0,
            read_failure_every: 0,
            drain_every: 0,
        }
    }
}

#[derive(Debug)]
struct MockState {
    connection: ConnectionState,
    battery_percent: u8,
    charging: bool,
    noise_mode: NoiseControlMode,
    noise_level: u8,
    eq: EqSettings,
}

pub struct MockBoseDevice {
    state: Mutex<MockState>,
    behaviour: MockBehaviour,
    mutation_count: AtomicU64,
    read_count: AtomicU64,
}

impl Default for MockBoseDevice {
    fn default() -> Self {
        Self::new(MockBehaviour::default())
    }
}

impl MockBoseDevice {
    pub fn new(behaviour: MockBehaviour) -> Self {
        Self {
            state: Mutex::new(MockState {
                connection: ConnectionState::Disconnected,
                battery_percent: 78,
                charging: false,
                noise_mode: NoiseControlMode::Quiet,
                noise_level: 10,
                eq: EqSettings::flat(),
            }),
            behaviour,
            mutation_count: AtomicU64::new(0),
            read_count: AtomicU64::new(0),
        }
    }

    fn identity() -> DeviceIdentity {
        DeviceIdentity {
            // The name carries the label too, so that even a screenshot taken
            // out of context cannot be mistaken for real hardware output.
            name: "Bose QuietComfort (SIMULATED)".to_string(),
            id: "mock-0000-0000-0000".to_string(),
            instance_id: None,
            manufacturer: Some("Simulated".to_string()),
            model_number: Some("MOCK-QC".to_string()),
            firmware_version: Some("0.0.0-simulated".to_string()),
            serial_number: None,
        }
    }

    /// The mock's capability set.
    ///
    /// Everything it can do is `Experimental`, and nothing is
    /// `hardware_verified`. A simulated device is evidence about the software,
    /// not about the headphones.
    fn simulated_capabilities() -> CapabilitySet {
        let mut caps = CapabilitySet::all_unknown("Simulated device: no real hardware interrogated.");
        const NOTE: &str = "Simulated by the mock backend. This is evidence about the \
                            application, not about any physical device.";

        for key in [
            CapabilityKey::Battery,
            CapabilityKey::NoiseControl,
            CapabilityKey::AwareMode,
            CapabilityKey::Equalizer,
            CapabilityKey::CustomNoiseControl,
            CapabilityKey::FirmwareVersion,
        ] {
            caps.get_mut(key)
                .mark_experimental(Mechanism::None, NOTE)
                .expect("fresh capability set permits transition");
        }
        caps
    }

    fn should_fail_read(&self) -> bool {
        if self.behaviour.read_failure_every == 0 {
            return false;
        }
        let n = self.read_count.fetch_add(1, Ordering::Relaxed) + 1;
        n % self.behaviour.read_failure_every == 0
    }

    fn should_be_unverified(&self) -> bool {
        if self.behaviour.unverified_every == 0 {
            return false;
        }
        let n = self.mutation_count.fetch_add(1, Ordering::Relaxed) + 1;
        n % self.behaviour.unverified_every == 0
    }

    fn require_connected(&self) -> DeviceResult<()> {
        let s = self.state.lock();
        match s.connection {
            ConnectionState::Connected => Ok(()),
            _ => Err(DeviceError::NotConnected),
        }
    }
}

#[async_trait]
impl BoseDevice for MockBoseDevice {
    fn source(&self) -> DeviceSource {
        DeviceSource::Mock
    }

    async fn capabilities(&self) -> CapabilitySet {
        let connected = matches!(self.state.lock().connection, ConnectionState::Connected);
        if connected {
            Self::simulated_capabilities()
        } else {
            CapabilitySet::all_unknown("Simulated device is disconnected.")
        }
    }

    async fn snapshot(&self) -> DeviceResult<DeviceSnapshot> {
        let (connection, battery, charging, noise_mode, noise_level, eq) = {
            let mut s = self.state.lock();
            if self.behaviour.drain_every > 0
                && matches!(s.connection, ConnectionState::Connected)
                && s.battery_percent > 1
            {
                let n = self.read_count.load(Ordering::Relaxed);
                if n > 0 && n % self.behaviour.drain_every == 0 {
                    s.battery_percent -= 1;
                }
            }
            (
                s.connection,
                s.battery_percent,
                s.charging,
                s.noise_mode,
                s.noise_level,
                s.eq,
            )
        };

        let connected = matches!(connection, ConnectionState::Connected);
        let now = now_rfc3339();

        Ok(DeviceSnapshot {
            source: DeviceSource::Mock,
            connection,
            transport: if connected {
                Transport::Simulated
            } else {
                Transport::None
            },
            identity: connected.then(Self::identity),
            capabilities: self.capabilities().await,
            battery: connected.then(|| BatteryReading {
                percent: battery,
                source: BatterySource::Simulated,
                charging: Some(charging),
                read_at: now.clone(),
            }),
            noise_control: connected.then(|| NoiseControlState {
                mode: noise_mode,
                level: Some(noise_level),
                label: None,
                source: StateSource::Simulated,
                read_at: now.clone(),
            }),
            equalizer: connected.then(|| EqState {
                settings: eq,
                source: StateSource::Simulated,
                read_at: now.clone(),
            }),
            // Windows audio is a real subsystem; the mock device does not
            // fabricate it. The audio module supplies it independently.
            windows_audio: None,
            last_error: None,
            updated_at: now,
        })
    }

    async fn connect(&self) -> DeviceResult<CommandOutcome> {
        // A small delay so the UI's connecting state is actually observable.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        self.state.lock().connection = ConnectionState::Connected;
        Ok(CommandOutcome::applied())
    }

    async fn disconnect(&self) -> DeviceResult<CommandOutcome> {
        self.state.lock().connection = ConnectionState::Disconnected;
        Ok(CommandOutcome::applied())
    }

    async fn device_info(&self) -> DeviceResult<Option<DeviceIdentity>> {
        self.require_connected()?;
        Ok(Some(Self::identity()))
    }

    async fn battery(&self) -> DeviceResult<Option<BatteryReading>> {
        self.require_connected()?;
        if self.should_fail_read() {
            return Err(DeviceError::Unreachable);
        }
        let s = self.state.lock();
        Ok(Some(BatteryReading {
            percent: s.battery_percent,
            source: BatterySource::Simulated,
            charging: Some(s.charging),
            read_at: now_rfc3339(),
        }))
    }

    async fn noise_control(&self) -> DeviceResult<Option<NoiseControlState>> {
        self.require_connected()?;
        let s = self.state.lock();
        Ok(Some(NoiseControlState {
            mode: s.noise_mode,
            level: Some(s.noise_level),
            label: None,
            source: StateSource::Simulated,
            read_at: now_rfc3339(),
        }))
    }

    async fn set_noise_control(&self, mode: NoiseControlMode) -> DeviceResult<CommandOutcome> {
        self.require_connected()?;

        // Simulate a device that sometimes applies a change without echoing
        // it back. The state still changes, but we cannot prove it did, so we
        // must not claim success.
        if self.should_be_unverified() {
            self.state.lock().noise_mode = mode;
            return Ok(CommandOutcome::sent_unverified(
                "Simulated device did not echo the new mode.",
            ));
        }

        {
            let mut s = self.state.lock();
            s.noise_mode = mode;
        }

        // Read back, exactly as the real backend must.
        let readback = self.state.lock().noise_mode;
        if readback == mode {
            Ok(CommandOutcome::applied())
        } else {
            Ok(CommandOutcome::sent_unverified(
                "Readback did not match the requested mode.",
            ))
        }
    }

    async fn set_noise_control_level(&self, level: u8) -> DeviceResult<CommandOutcome> {
        self.require_connected()?;
        if level > 10 {
            return Err(DeviceError::InvalidInput(
                "noise control level out of range".to_string(),
            ));
        }
        // Like the real backend: the level belongs to the custom mode.
        if self.state.lock().noise_mode != NoiseControlMode::Custom {
            return Ok(CommandOutcome::unsupported(
                "The simulated device only has an adjustable level in Custom mode.",
            ));
        }
        if self.should_be_unverified() {
            self.state.lock().noise_level = level;
            return Ok(CommandOutcome::sent_unverified(
                "Simulated device did not echo the new level.",
            ));
        }
        self.state.lock().noise_level = level;
        let readback = self.state.lock().noise_level;
        if readback == level {
            Ok(CommandOutcome::applied())
        } else {
            Ok(CommandOutcome::sent_unverified(
                "Readback did not match the requested level.",
            ))
        }
    }

    async fn equalizer(&self) -> DeviceResult<Option<EqState>> {
        self.require_connected()?;
        let s = self.state.lock();
        Ok(Some(EqState {
            settings: s.eq,
            source: StateSource::Simulated,
            read_at: now_rfc3339(),
        }))
    }

    async fn set_equalizer(&self, settings: EqSettings) -> DeviceResult<CommandOutcome> {
        self.require_connected()?;
        if !settings.is_within_range() {
            return Err(DeviceError::InvalidInput(
                "EQ gains out of range".to_string(),
            ));
        }
        self.state.lock().eq = settings;
        let readback = self.state.lock().eq;
        if readback == settings {
            Ok(CommandOutcome::applied())
        } else {
            Ok(CommandOutcome::sent_unverified("Readback mismatch."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perfect() -> MockBoseDevice {
        MockBoseDevice::new(MockBehaviour::perfect())
    }

    #[tokio::test]
    async fn starts_disconnected_and_reveals_nothing() {
        let dev = perfect();
        let snap = dev.snapshot().await.unwrap();
        assert_eq!(snap.connection, ConnectionState::Disconnected);
        assert!(snap.battery.is_none());
        assert!(snap.identity.is_none());
        assert!(snap.noise_control.is_none());
    }

    #[tokio::test]
    async fn reads_fail_when_disconnected() {
        let dev = perfect();
        assert!(matches!(
            dev.battery().await,
            Err(DeviceError::NotConnected)
        ));
    }

    #[tokio::test]
    async fn connecting_exposes_simulated_readings() {
        let dev = perfect();
        dev.connect().await.unwrap();
        let snap = dev.snapshot().await.unwrap();
        assert_eq!(snap.connection, ConnectionState::Connected);
        assert_eq!(snap.source, DeviceSource::Mock);
        assert_eq!(snap.transport, Transport::Simulated);

        let battery = snap.battery.expect("connected mock reports battery");
        assert_eq!(battery.source, BatterySource::Simulated);
    }

    /// The most important property of the mock: it can never make the
    /// application believe a real capability was verified.
    #[tokio::test]
    async fn mock_never_reports_hardware_verified_capabilities() {
        let dev = perfect();
        dev.connect().await.unwrap();
        let caps = dev.capabilities().await;
        assert_eq!(caps.verified_count(), 0);
        for cap in caps.iter() {
            assert!(!cap.hardware_verified, "{:?} claimed hardware verification", cap.key);
            assert_ne!(
                cap.status,
                crate::device::CapabilityStatus::Verified,
                "{:?} reached VERIFIED from simulation",
                cap.key
            );
        }
    }

    #[tokio::test]
    async fn identity_is_labelled_simulated() {
        let dev = perfect();
        dev.connect().await.unwrap();
        let id = dev.device_info().await.unwrap().unwrap();
        assert!(
            id.name.to_uppercase().contains("SIMULATED"),
            "mock identity must be self-labelling, got {:?}",
            id.name
        );
    }

    #[tokio::test]
    async fn noise_level_round_trips_in_custom_mode_only() {
        let dev = perfect();
        dev.connect().await.unwrap();

        // Not in Custom: nothing to adjust, and it says so.
        let outcome = dev.set_noise_control_level(4).await.unwrap();
        assert!(matches!(outcome, CommandOutcome::Unsupported { .. }));

        dev.set_noise_control(NoiseControlMode::Custom).await.unwrap();
        let outcome = dev.set_noise_control_level(4).await.unwrap();
        assert!(outcome.is_confirmed());
        assert_eq!(dev.noise_control().await.unwrap().unwrap().level, Some(4));

        assert!(dev.set_noise_control_level(11).await.is_err());
    }

    #[tokio::test]
    async fn noise_control_round_trips() {
        let dev = perfect();
        dev.connect().await.unwrap();
        let outcome = dev.set_noise_control(NoiseControlMode::Aware).await.unwrap();
        assert!(outcome.is_confirmed());
        let state = dev.noise_control().await.unwrap().unwrap();
        assert_eq!(state.mode, NoiseControlMode::Aware);
    }

    #[tokio::test]
    async fn unverified_path_is_reachable() {
        // With unverified_every = 1, every mutation must report unverified.
        let dev = MockBoseDevice::new(MockBehaviour {
            unverified_every: 1,
            read_failure_every: 0,
            drain_every: 0,
        });
        dev.connect().await.unwrap();
        let outcome = dev.set_noise_control(NoiseControlMode::Quiet).await.unwrap();
        assert!(!outcome.is_confirmed());
        assert!(matches!(outcome, CommandOutcome::SentUnverified { .. }));
    }

    #[tokio::test]
    async fn out_of_range_eq_is_refused() {
        let dev = perfect();
        dev.connect().await.unwrap();
        let bad = EqSettings {
            bass: 99,
            mid: 0,
            treble: 0,
        };
        assert!(dev.set_equalizer(bad).await.is_err());
    }

    #[tokio::test]
    async fn disconnect_clears_exposed_state() {
        let dev = perfect();
        dev.connect().await.unwrap();
        dev.disconnect().await.unwrap();
        let snap = dev.snapshot().await.unwrap();
        assert!(snap.battery.is_none());
        assert!(snap.identity.is_none());
    }
}
