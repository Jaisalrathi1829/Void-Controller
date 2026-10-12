//! The hardware-backed device.
//!
//! Two sources of truth, kept apart:
//!
//!   * **Windows** says that the device exists, whether it is connected, and
//!     what its battery level is (PnP enumeration and properties).
//!   * **The headphones themselves** say which noise-control mode they are in,
//!     over Bose's BMAP protocol on an RFCOMM channel. That link lives in
//!     [`super::bmap`]; the protocol implementation is vendored from upstream.
//!
//! Noise control is the one vendor-protocol feature driven here: switching
//! between Quiet, Aware and a custom mode the user has set up, and adjusting
//! that custom mode's noise-cancellation level. Each is reported as
//! EXPERIMENTAL until the headphones have confirmed it, and a change is only
//! ever reported as applied when the device's own read-back says so.
//! The three-band hardware equalizer is read and written the same way.
//! Everything else behind the vendor protocol — multipoint, creating or
//! renaming modes — is left alone.

use super::bmap as control;
use crate::device::capability::{
    Capability, CapabilityKey, CapabilitySet, HardwareProof, Mechanism,
};
use crate::device::command::CommandOutcome;
use crate::device::state::{
    BatteryReading, BatterySource, ConnectionState, DeviceIdentity, DeviceSnapshot, DeviceSource,
    EqSettings, EqState, NoiseControlMode, NoiseControlState, StateSource, Transport,
};
use crate::device::traits::BoseDevice;
use crate::device::{now_rfc3339, DeviceError, DeviceResult};
use async_trait::async_trait;
use control::custom::{flip_level, usable_slots, MAX_RAW_LEVEL};
use control::link::{Confirmed, EqReading, SlotTable, TransitionProof};
use control::noise::{describe_index, preset_for_index};
use control::{
    BmapLink, EqChange, EqLevels, LevelChange, LinkError, LinkErrorKind, LinkStatus, LinkTiming,
    ModeChange, PlatformConnector, Preset, Slot, Target, Verdict,
};
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Where the host's own battery reading comes from on this platform.
#[cfg(not(target_os = "android"))]
const HOST_BATTERY: BatterySource = BatterySource::WindowsPnp;
#[cfg(target_os = "android")]
const HOST_BATTERY: BatterySource = BatterySource::AndroidBluetooth;

/// The battery level to show: the host system's if it has one, and
/// otherwise the one the headphones gave over the control link — each
/// labelled as what it is.
fn battery_reading(
    host: Option<u8>,
    reported: Option<&control::link::BatteryReport>,
    now: &str,
) -> Option<BatteryReading> {
    match (host, reported) {
        (Some(percent), _) => Some(BatteryReading {
            percent,
            source: HOST_BATTERY,
            charging: None,
            read_at: now.to_string(),
        }),
        (None, Some(report)) => Some(BatteryReading {
            percent: report.percent,
            source: BatterySource::VendorProtocol,
            charging: None,
            read_at: report.at.clone(),
        }),
        (None, None) => None,
    }
}

/// Why "Custom" is unavailable on headphones with no custom mode.
const NO_CUSTOM_MODE: &str = "No custom mode is set up on these headphones. Create one in the \
     Bose app first: this application adjusts an existing custom mode and does not create one.";

/// What the user asked the noise control to be.
#[derive(Debug, Clone, Copy)]
enum Wanted {
    Preset(Preset),
    Custom,
}

#[derive(Debug, Clone, Default)]
struct Observed {
    instance_id: Option<String>,
    name: Option<String>,
    connected: bool,
    battery: Option<u8>,
    /// Set once we have actually read a battery value from the physical
    /// device, which is genuine hardware evidence for the battery capability.
    battery_ever_read: bool,
    /// 48-bit Bluetooth address. Used to open the control link and nowhere
    /// else; it never leaves the native layer.
    address: Option<u64>,
    /// Bose product id, the key into upstream's model catalog.
    product_id: Option<u16>,
    /// Whether Windows lists Bose's BMAP service among the device's services.
    advertises_bmap: bool,
}

/// What is needed to open a control link to a device.
#[derive(Clone)]
struct ControlTarget {
    address: u64,
    product_id: u16,
    config: ::bmap::DeviceConfig,
}

/// Decides whether a control link may be opened to the observed device, and
/// says why not when it may not.
///
/// Three independent things must hold: the model is one this application has
/// been tested against, Windows' own service discovery shows the device
/// advertising BMAP, and the address is known. Being a Bose device is not
/// enough.
fn control_target(obs: &Observed) -> Result<ControlTarget, String> {
    let Some(product_id) = obs.product_id else {
        // Android has no product id to give; the headphones are asked, and
        // this is what it looks like when they have not answered.
        #[cfg(target_os = "android")]
        return Err(
            "The headphones have not said which model they are, so no control connection is \
             opened. Another app may be holding their control channel — close the Bose app if \
             it is running."
                .to_string(),
        );
        #[cfg(not(target_os = "android"))]
        return Err(
            "Windows did not report a product id for this device, so its model cannot be \
             identified."
                .to_string(),
        );
    };
    let Some(config) = control::config_for_product(product_id) else {
        return Err(format!(
            "Product 0x{product_id:04X} is not a model this application has been tested against, \
             so no control connection is opened to it."
        ));
    };
    if !obs.advertises_bmap {
        return Err(
            "Bose's control service (BMAP) is not listed among this device's services."
                .to_string(),
        );
    }
    let Some(address) = obs.address else {
        return Err("The device's Bluetooth address could not be determined.".to_string());
    };
    Ok(ControlTarget {
        address,
        product_id,
        config,
    })
}

/// The control link and the device it was built for.
struct Control {
    address: u64,
    product_id: u16,
    link: BmapLink<PlatformConnector>,
}

/// What Windows currently says about the headphones, without touching them.
///
/// Used by the background watcher: it is built from PnP properties alone, so
/// taking one sends nothing over Bluetooth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// Windows instance id. Contains the Bluetooth address; stays native.
    pub instance_id: String,
    pub name: String,
    pub connected: bool,
    pub battery: Option<u8>,
}

/// Picks the device this application is about: the one with the given
/// instance id, or — when none is given — the first Bose device.
#[cfg(windows)]
fn find_device(
    devices: Vec<crate::bluetooth::pnp::PnpDevice>,
    preferred: Option<&str>,
) -> Option<crate::bluetooth::pnp::PnpDevice> {
    devices
        .into_iter()
        .filter(|d| d.is_top_level())
        .find(|d| match preferred {
            Some(want) => d.instance_id == want,
            // Vendor id first: the friendly name is user-editable and the
            // test device is renamed, so name matching alone finds nothing.
            None => super::is_bose_device(
                d.vendor_id,
                d.friendly_name.as_deref().unwrap_or_default(),
            ),
        })
}

/// Looks the headphones up in what Windows knows. `None` if they are not
/// paired with this PC.
pub fn sight_device(preferred: Option<&str>) -> Option<Sighting> {
    #[cfg(windows)]
    {
        let dev = find_device(crate::bluetooth::pnp::enumerate_bluetooth_devices(), preferred)?;
        Some(Sighting {
            name: dev
                .friendly_name
                .clone()
                .unwrap_or_else(|| "Bose device".to_string()),
            connected: dev.is_connected.unwrap_or(false),
            battery: dev.battery_percent,
            instance_id: dev.instance_id,
        })
    }
    #[cfg(target_os = "android")]
    {
        use crate::android::directory;
        let dev = directory::find(directory::report().ok()?.devices, preferred)?;
        Some(Sighting {
            instance_id: dev.instance_id,
            name: dev.name,
            connected: dev.connected,
            battery: dev.battery,
        })
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = preferred;
        None
    }
}

/// The device this application should be about: the remembered one if it is
/// still paired, otherwise the first Bose device Windows knows.
pub fn sight_preferred(stored: Option<&str>) -> Option<Sighting> {
    stored
        .and_then(|id| sight_device(Some(id)))
        .or_else(|| sight_device(None))
}

pub struct RealBoseDevice {
    /// Optional explicit target. When `None`, the first device whose Windows
    /// friendly name matches a Bose hint is used.
    preferred_instance_id: Option<String>,
    /// Whether this application is attached to the headphones.
    ///
    /// Separate from whether Windows has them connected. Detached, the
    /// application reports itself disconnected and opens no control link even
    /// if the headphones are right there — which is what "auto-connect off"
    /// and "auto reconnect off" mean. Shared with the application state, which
    /// is what flips it.
    attached: Arc<AtomicBool>,
    observed: RwLock<Observed>,
    /// The control link. Behind a mutex so there is only ever one conversation
    /// with the headphones at a time, and used from blocking tasks because
    /// RFCOMM I/O blocks.
    control: Arc<Mutex<Option<Control>>>,
    /// What the link last reported about itself, cached so capability evidence
    /// can be built without waiting on the link.
    link_status: RwLock<LinkStatus>,
}

impl Default for RealBoseDevice {
    fn default() -> Self {
        Self::new(None)
    }
}

impl RealBoseDevice {
    /// A backend that is attached from the start.
    pub fn new(preferred_instance_id: Option<String>) -> Self {
        Self::with_attachment(preferred_instance_id, Arc::new(AtomicBool::new(true)))
    }

    /// A backend whose attachment is controlled from outside.
    pub fn with_attachment(preferred_instance_id: Option<String>, attached: Arc<AtomicBool>) -> Self {
        Self {
            preferred_instance_id,
            attached,
            observed: RwLock::new(Observed::default()),
            control: Arc::new(Mutex::new(None)),
            link_status: RwLock::new(LinkStatus::default()),
        }
    }

    /// Locates the target device and refreshes the cached observation.
    ///
    /// Returns `Ok(false)` when no matching device is present — that is a
    /// normal state (headphones off or unpaired), not an error.
    #[cfg(windows)]
    fn refresh(&self) -> DeviceResult<bool> {
        use crate::bluetooth::pnp;

        let radio = crate::bluetooth::radio::availability();
        if !radio.radio_present {
            return Err(DeviceError::BluetoothUnavailable);
        }

        let matched = find_device(
            pnp::enumerate_bluetooth_devices(),
            self.preferred_instance_id.as_deref(),
        );

        let Some(dev) = matched else {
            *self.observed.write() = Observed::default();
            return Ok(false);
        };

        let mut obs = self.observed.write();
        obs.instance_id = Some(dev.instance_id.clone());
        obs.name = dev.friendly_name.clone();
        obs.connected = dev.is_connected.unwrap_or(false);
        obs.battery = dev.battery_percent;
        if dev.battery_percent.is_some() {
            obs.battery_ever_read = true;
        }
        obs.address = pnp::device_address(&dev.instance_id)
            .and_then(|hex| u64::from_str_radix(&hex, 16).ok());
        obs.product_id = dev.product_id;
        obs.advertises_bmap = dev.advertises_service(control::endpoint::BMAP_SERVICE_UUID);
        Ok(true)
    }

    /// The Android counterpart: the same observation, built from what
    /// Android knows about its paired devices.
    #[cfg(target_os = "android")]
    fn refresh(&self) -> DeviceResult<bool> {
        use crate::android::directory;

        let report = directory::report().map_err(DeviceError::Platform)?;
        if !report.radio_present {
            return Err(DeviceError::BluetoothUnavailable);
        }
        if !report.permission {
            return Err(DeviceError::NotPermitted(
                "This application has not been allowed to use Bluetooth. Allow \"Nearby \
                 devices\" for it, and it will find the headphones."
                    .to_string(),
            ));
        }

        // The remembered headphones if they are still paired; otherwise
        // whichever paired device is a Bose one.
        let matched = directory::find(report.devices.clone(), self.preferred_instance_id.as_deref())
            .or_else(|| directory::find(report.devices, None));
        let Some(dev) = matched else {
            *self.observed.write() = Observed::default();
            return Ok(false);
        };

        // Android gives no product id, so the headphones are asked for it —
        // once, and only while they are connected.
        let product_id = match dev.address {
            Some(address) if dev.connected => directory::product_id(address),
            _ => None,
        };

        let mut obs = self.observed.write();
        let same_device = obs.instance_id.as_deref() == Some(dev.instance_id.as_str());
        obs.product_id = product_id.or(if same_device { obs.product_id } else { None });
        obs.instance_id = Some(dev.instance_id.clone());
        obs.name = Some(dev.name.clone());
        obs.connected = dev.connected;
        obs.battery = dev.battery;
        if dev.battery.is_some() {
            obs.battery_ever_read = true;
        }
        obs.address = dev.address;
        // Answering a product-id request over the control protocol shows the
        // service is there at least as well as a listing of it does.
        obs.advertises_bmap = dev.advertises_bmap || obs.product_id.is_some();
        Ok(true)
    }

    #[cfg(not(any(windows, target_os = "android")))]
    fn refresh(&self) -> DeviceResult<bool> {
        Err(DeviceError::Platform(
            "the real device backend is only implemented for Windows and Android".to_string(),
        ))
    }

    fn is_attached(&self) -> bool {
        self.attached.load(Ordering::SeqCst)
    }

    /// The snapshot of a backend that is not attached: disconnected, with
    /// nothing read from anywhere and nothing claimed.
    fn detached_snapshot() -> DeviceSnapshot {
        DeviceSnapshot {
            capabilities: CapabilitySet::all_unknown(
                "Not connected from this application. Press Connect to attach to the headphones.",
            ),
            ..DeviceSnapshot::empty(DeviceSource::Real)
        }
    }

    /// Runs one operation on the control link, creating the link if needed.
    ///
    /// The mutex makes operations strictly sequential: a status refresh that
    /// arrives during a mode change waits for it rather than interleaving
    /// frames with it on the same channel.
    async fn with_link<R, F>(&self, target: ControlTarget, op: F) -> DeviceResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut BmapLink<PlatformConnector>) -> R + Send + 'static,
    {
        let control = Arc::clone(&self.control);
        let (result, status) = tokio::task::spawn_blocking(move || {
            let mut slot = control.lock();

            // A link built for a different device or model is not reused.
            let stale = slot
                .as_ref()
                .is_some_and(|c| c.address != target.address || c.product_id != target.product_id);
            if stale {
                *slot = None;
            }

            let ctl = slot.get_or_insert_with(|| Control {
                address: target.address,
                product_id: target.product_id,
                link: BmapLink::new(
                    PlatformConnector::new(target.address),
                    target.config,
                    LinkTiming::standard(),
                    now_rfc3339,
                ),
            });

            let result = op(&mut ctl.link);
            (result, ctl.link.status())
        })
        .await
        .map_err(|e| DeviceError::Platform(format!("the control task did not finish: {e}")))?;

        *self.link_status.write() = status;
        Ok(result)
    }

    /// Tells the link the headphones are no longer there. Closes the
    /// connection and forgets the last mode they reported.
    async fn link_device_gone(&self) {
        let control = Arc::clone(&self.control);
        let status = tokio::task::spawn_blocking(move || {
            control.lock().as_mut().map(|c| {
                c.link.device_gone();
                c.link.status()
            })
        })
        .await
        .ok()
        .flatten();

        if let Some(status) = status {
            *self.link_status.write() = status;
        }
    }

    fn build_capabilities(&self, obs: &Observed, link: &LinkStatus) -> CapabilitySet {
        let mut caps = CapabilitySet::all_unknown(
            "Not yet investigated on this device. Run Diagnostics to interrogate it.",
        );

        if obs.instance_id.is_none() {
            return caps;
        }

        // Battery. If we have actually read a value off the physical device,
        // that is hardware evidence that the read path works end to end.
        // If the property is simply absent, we say so rather than guessing.
        if obs.battery_ever_read {
            let proof = HardwareProof::observed_passively(
                CapabilityKey::Battery,
                format!(
                    "Windows PnP battery property returned a level for this device{}",
                    obs.battery
                        .map(|b| format!(" (currently {b}%)"))
                        .unwrap_or_default()
                ),
            );
            let _ = caps
                .get_mut(CapabilityKey::Battery)
                .verify_with_hardware(&proof, Mechanism::WindowsPnp);
        } else if obs.connected {
            let _ = caps.get_mut(CapabilityKey::Battery).mark_supported(
                Mechanism::WindowsPnp,
                "The device is connected but has not populated the Windows battery property. \
                 It may report battery only while audio is streaming, or not at all.",
            );
        }

        // Device identity is readable straight from Windows for any paired
        // device, and we have just read it.
        let proof = HardwareProof::observed_passively(
            CapabilityKey::DeviceSettings,
            "device identity read from Windows PnP",
        );
        let _ = caps
            .get_mut(CapabilityKey::DeviceSettings)
            .verify_with_hardware(&proof, Mechanism::WindowsPnp);

        // Vendor-protocol features this build does not drive stay UNKNOWN.
        // Upstream documents several of them for this model; "documented" is
        // not "shown to work here", so nothing is claimed.
        for key in [
            CapabilityKey::Multipoint,
            CapabilityKey::AutoOff,
            CapabilityKey::VoicePrompts,
            CapabilityKey::Sidetone,
            CapabilityKey::DeviceRename,
        ] {
            *caps.get_mut(key) = Capability::unknown(
                key,
                "Not driven by this build. The control protocol is used for noise control \
                 and the equalizer only.",
            );
        }

        apply_control_capabilities(&mut caps, obs, link);
        caps
    }

    fn identity_from(obs: &Observed, link: &LinkStatus) -> Option<DeviceIdentity> {
        let instance_id = obs.instance_id.as_ref()?;
        let target = control_target(obs).ok();
        Some(DeviceIdentity {
            name: obs
                .name
                .clone()
                .unwrap_or_else(|| "Bose device".to_string()),
            id: crate::util::stable_id(instance_id),
            // The raw instance id is kept out of the snapshot handed to the UI
            // so it cannot leak into an exported report or a screenshot.
            instance_id: None,
            manufacturer: Some("Bose".to_string()),
            // Upstream's name for the product id Windows reported.
            model_number: target.as_ref().map(|t| t.config.info.name.to_string()),
            // Only ever the string the headphones themselves returned.
            firmware_version: target.and(link.firmware.clone()),
            serial_number: None,
        })
    }
}

/// Fills in the capabilities that depend on the control link.
///
/// Pure: everything it says is derived from what Windows reported and what the
/// link has actually observed, so it can be tested without hardware.
fn apply_control_capabilities(caps: &mut CapabilitySet, obs: &Observed, link: &LinkStatus) {
    // Everything here needs a working control link, so the reasons for not
    // having one apply to all three alike.
    const NOISE_KEYS: [CapabilityKey; 4] = [
        CapabilityKey::NoiseControl,
        CapabilityKey::AwareMode,
        CapabilityKey::CustomNoiseControl,
        CapabilityKey::Equalizer,
    ];
    const PRESET_KEYS: [CapabilityKey; 2] =
        [CapabilityKey::NoiseControl, CapabilityKey::AwareMode];

    let unknown = |caps: &mut CapabilitySet, key: CapabilityKey, why: String| {
        *caps.get_mut(key) = Capability::unknown(key, why);
    };

    let target = match control_target(obs) {
        Ok(target) => target,
        Err(why) => {
            for key in NOISE_KEYS {
                unknown(caps, key, why.clone());
            }
            unknown(caps, CapabilityKey::FirmwareVersion, why);
            return;
        }
    };

    if !obs.connected {
        for key in NOISE_KEYS {
            unknown(caps, key, "The headphones are not connected.".to_string());
        }
        unknown(
            caps,
            CapabilityKey::FirmwareVersion,
            "The headphones are not connected.".to_string(),
        );
        return;
    }

    // Firmware: verified once the headphones have answered the firmware
    // request on a connection that is still open.
    match (&link.endpoint, &link.firmware) {
        (Some(_), Some(firmware)) => {
            let proof = HardwareProof::observed_passively(
                CapabilityKey::FirmwareVersion,
                format!("the headphones answered a firmware request with {firmware:?}"),
            );
            let _ = caps
                .get_mut(CapabilityKey::FirmwareVersion)
                .verify_with_hardware(&proof, Mechanism::RfcommVendor);
        }
        _ => unknown(
            caps,
            CapabilityKey::FirmwareVersion,
            "The headphones have not answered a firmware request on the control channel yet."
                .to_string(),
        ),
    }

    if let Some(problem) = &link.framing_problem {
        for key in NOISE_KEYS {
            unknown(
                caps,
                key,
                format!(
                    "The headphones answered, but their mode reply does not match the documented \
                     format, so no changes will be sent. {problem}"
                ),
            );
        }
        return;
    }

    if link.confirmed.is_none() {
        let why = match &link.last_failure {
            Some(failure) => format!("The control channel could not be validated: {failure}"),
            None => "The control channel has not been opened yet.".to_string(),
        };
        for key in NOISE_KEYS {
            unknown(caps, key, why.clone());
        }
        return;
    }

    let config = &target.config;
    let where_from = link
        .endpoint
        .as_ref()
        .map(|e| e.describe())
        .unwrap_or_else(|| "the control channel".to_string());

    match (&link.to_quiet, &link.to_aware) {
        // Both directions confirmed by the headphones: this, and only this,
        // is what VERIFIED means for noise control.
        (Some(to_quiet), Some(to_aware)) => {
            for (key, primary, primary_preset, other) in [
                (CapabilityKey::NoiseControl, to_quiet, Preset::Quiet, to_aware),
                (CapabilityKey::AwareMode, to_aware, Preset::Aware, to_quiet),
            ] {
                let Some(proof) = transition_proof(key, config, primary, primary_preset) else {
                    continue;
                };
                let cap = caps.get_mut(key);
                if cap.verify_with_hardware(&proof, Mechanism::RfcommVendor).is_ok() {
                    cap.evidence = format!(
                        "{}. The opposite switch was confirmed the same way at {}: {:?} -> {:?}.",
                        cap.evidence,
                        other.at,
                        describe_index(config, other.before),
                        describe_index(config, other.after),
                    );
                }
            }
        }
        (to_quiet, to_aware) => {
            let seen = |proof: &Option<TransitionProof>| match proof {
                Some(p) => format!("confirmed at {}", p.at),
                None => "not yet".to_string(),
            };
            let evidence = format!(
                "The current mode is read from the headphones over Bose's BMAP protocol \
                 ({where_from}), and every change is checked by reading the mode back. Switches \
                 the headphones have confirmed this session: to Aware — {}; to Quiet — {}. Both \
                 are needed before this is marked VERIFIED.",
                seen(to_aware),
                seen(to_quiet),
            );
            for key in PRESET_KEYS {
                let _ = caps
                    .get_mut(key)
                    .mark_experimental(Mechanism::RfcommVendor, evidence.clone());
            }
        }
    }

    apply_custom_capability(caps, config, link, &where_from);
    apply_eq_capability(caps, link, &where_from);
}

/// The equalizer capability. EXPERIMENTAL once the headphones have reported
/// their equalizer in the documented shape; VERIFIED once they have confirmed
/// a change to it.
fn apply_eq_capability(caps: &mut CapabilitySet, link: &LinkStatus, where_from: &str) {
    let key = CapabilityKey::Equalizer;

    if let Some(problem) = &link.eq_problem {
        *caps.get_mut(key) = Capability::unknown(
            key,
            format!(
                "The headphones' equalizer could not be read as documented, so it is left \
                 alone. {problem}"
            ),
        );
        return;
    }
    let Some(reading) = &link.eq else {
        *caps.get_mut(key) = Capability::unknown(
            key,
            "The headphones' equalizer has not been read yet.",
        );
        return;
    };

    match &link.eq_change {
        Some(change) => {
            let after = change.after.describe();
            if let Some(proof) = HardwareProof::observed_at(
                key,
                &change.before.describe(),
                &after,
                &after,
                change.at.clone(),
            ) {
                let _ = caps
                    .get_mut(key)
                    .verify_with_hardware(&proof, Mechanism::RfcommVendor);
            }
        }
        None => {
            let _ = caps.get_mut(key).mark_experimental(
                Mechanism::RfcommVendor,
                format!(
                    "The headphones report their equalizer as {} ({where_from}). A change is \
                     written to the headphones and checked by reading it back; none has been \
                     confirmed this session yet.",
                    reading.levels.describe()
                ),
            );
        }
    }
}

/// Turns an equalizer reading from the headphones into UI state.
fn eq_state(reading: &EqReading) -> EqState {
    EqState {
        settings: EqSettings {
            bass: reading.levels.bass,
            mid: reading.levels.mid,
            treble: reading.levels.treble,
        },
        source: StateSource::VendorProtocol,
        read_at: reading.at.clone(),
    }
}

/// What to tell the user about an equalizer change. Only a read-back showing
/// all three requested gains is success.
fn eq_outcome(change: &EqChange) -> CommandOutcome {
    let last_confirmed = || {
        format!(
            "The last equalizer the headphones confirmed was {}.",
            change.after.unwrap_or(change.before).describe()
        )
    };
    match &change.verdict {
        Verdict::Verified | Verdict::AlreadyActive => CommandOutcome::applied(),
        Verdict::Unverified { reason } => {
            if change.after.is_some() {
                CommandOutcome::sent_unverified(reason.clone())
            } else {
                CommandOutcome::sent_unverified(format!("{reason} {}", last_confirmed()))
            }
        }
        Verdict::Rejected { reason } => CommandOutcome::Rejected {
            reason: format!("The headphones refused the change ({reason}). {}", last_confirmed()),
        },
    }
}

/// Which custom mode "Custom" means right now.
///
/// The one the headphones are in, if they are in one. Otherwise the one they
/// were last seen in this session, and failing that the first one set up.
fn pick_custom_slot<'a>(
    config: &::bmap::DeviceConfig,
    slots: &'a [Slot],
    current: Option<u8>,
    last: Option<u8>,
) -> Option<&'a Slot> {
    let usable = usable_slots(config, slots);
    let by_index = |index: Option<u8>| {
        index.and_then(|i| usable.iter().copied().find(|s| s.index == i))
    };
    by_index(current)
        .or_else(|| by_index(last))
        .or_else(|| usable.first().copied())
}

/// The custom-mode capability, from the mode list and what has been confirmed.
///
/// Two separate things have to be shown to work: getting the headphones into
/// the custom mode, and changing its level. VERIFIED needs both.
fn apply_custom_capability(
    caps: &mut CapabilitySet,
    config: &::bmap::DeviceConfig,
    link: &LinkStatus,
    where_from: &str,
) {
    let key = CapabilityKey::CustomNoiseControl;
    let unknown = |caps: &mut CapabilitySet, why: String| {
        *caps.get_mut(key) = Capability::unknown(key, why);
    };

    if let Some(problem) = &link.slots_problem {
        unknown(
            caps,
            format!(
                "The headphones' mode list does not match the documented format, so custom \
                 modes are left alone. {problem}"
            ),
        );
        return;
    }
    let Some(table) = &link.slots else {
        unknown(
            caps,
            "The headphones' mode list has not been read yet.".to_string(),
        );
        return;
    };
    let current = link.confirmed.as_ref().map(|c| c.index);
    let Some(slot) = pick_custom_slot(config, &table.slots, current, link.last_custom_slot) else {
        unknown(caps, NO_CUSTOM_MODE.to_string());
        return;
    };

    match (&link.to_custom, &link.level) {
        (Some(switch), Some(level)) => {
            let describe = |raw: u8| format!("{} at level {}", level.name, flip_level(raw));
            let after = describe(level.after_raw);
            let Some(proof) = HardwareProof::observed_at(
                key,
                &describe(level.before_raw),
                &after,
                &after,
                level.at.clone(),
            ) else {
                return;
            };
            let cap = caps.get_mut(key);
            if cap.verify_with_hardware(&proof, Mechanism::RfcommVendor).is_ok() {
                cap.evidence = format!(
                    "{}. A switch into the custom mode was confirmed the same way at {}: \
                     {:?} -> {:?}.",
                    cap.evidence,
                    switch.at,
                    describe_index(config, switch.before),
                    describe_index(config, switch.after),
                );
            }
        }
        (switch, level) => {
            let evidence = format!(
                "The headphones list a custom mode, {:?}, at noise-cancellation level {} of 10 \
                 ({where_from}). Selecting it and changing its level are each checked by reading \
                 the headphones back, and a level change is written to that saved mode. \
                 Confirmed this session: switching to it — {}; changing its level — {}. Both \
                 are needed before this is marked VERIFIED.",
                slot.name,
                slot.strength(),
                match switch {
                    Some(p) => format!("confirmed at {}", p.at),
                    None => "not yet".to_string(),
                },
                match level {
                    Some(p) => format!("confirmed at {}", p.at),
                    None => "not yet".to_string(),
                },
            );
            let _ = caps
                .get_mut(key)
                .mark_experimental(Mechanism::RfcommVendor, evidence);
        }
    }
}

/// Builds hardware proof from a transition the headphones confirmed.
fn transition_proof(
    key: CapabilityKey,
    config: &::bmap::DeviceConfig,
    proof: &TransitionProof,
    preset: Preset,
) -> Option<HardwareProof> {
    HardwareProof::observed_at(
        key,
        &describe_index(config, proof.before),
        &describe_index(config, proof.after),
        preset.label(),
        proof.at.clone(),
    )
}

/// Turns a mode the headphones reported into UI state.
///
/// A custom mode's name and level come from the mode list, and only if the
/// list has an entry for the slot the headphones say they are in. Without
/// one, the mode is still "Custom" — with no level, rather than a guessed one.
fn noise_state(
    config: &::bmap::DeviceConfig,
    confirmed: &Confirmed,
    slots: Option<&SlotTable>,
) -> NoiseControlState {
    let preset = preset_for_index(config, confirmed.index);
    let slot = match preset {
        Some(_) => None,
        None => slots.and_then(|t| t.slots.iter().find(|s| s.index == confirmed.index)),
    };
    NoiseControlState {
        mode: match preset {
            Some(Preset::Quiet) => NoiseControlMode::Quiet,
            Some(Preset::Aware) => NoiseControlMode::Aware,
            // Any other index is one of the user-configurable slots.
            None => NoiseControlMode::Custom,
        },
        level: slot.map(Slot::strength),
        label: slot
            .map(|s| s.name.trim().to_string())
            .filter(|name| !name.is_empty()),
        source: StateSource::VendorProtocol,
        // When the headphones said it, not when it was last displayed.
        read_at: confirmed.at.clone(),
    }
}

/// What to tell the user about a mode-change attempt.
///
/// `Applied` is returned only for a read-back that shows the requested mode.
/// An acknowledgement on its own never is.
fn outcome_for(change: &ModeChange, config: &::bmap::DeviceConfig) -> CommandOutcome {
    // With no read-back at all, the baseline is still the last thing the
    // headphones said about themselves.
    let last_confirmed = || {
        format!(
            "The last mode the headphones confirmed was {}.",
            describe_index(config, change.after.unwrap_or(change.before))
        )
    };

    match &change.verdict {
        Verdict::Verified | Verdict::AlreadyActive => CommandOutcome::applied(),
        Verdict::Unverified { reason } => {
            if change.after.is_some() {
                CommandOutcome::sent_unverified(reason.clone())
            } else {
                CommandOutcome::sent_unverified(format!("{reason} {}", last_confirmed()))
            }
        }
        Verdict::Rejected { reason } => CommandOutcome::Rejected {
            reason: format!("The headphones refused the change ({reason}). {}", last_confirmed()),
        },
    }
}

/// What to tell the user about an attempt to change a custom mode's level.
/// Same rule as for a mode change: only a matching read-back is success.
fn level_outcome(change: &LevelChange) -> CommandOutcome {
    let last_confirmed = || {
        let raw = change
            .after
            .as_ref()
            .map(|slot| slot.raw_level)
            .unwrap_or(change.before_raw);
        format!(
            "The last level the headphones confirmed for {:?} was {}.",
            change.name,
            flip_level(raw)
        )
    };

    match &change.verdict {
        Verdict::Verified | Verdict::AlreadyActive => CommandOutcome::applied(),
        Verdict::Unverified { reason } => {
            if change.after.is_some() {
                CommandOutcome::sent_unverified(reason.clone())
            } else {
                CommandOutcome::sent_unverified(format!("{reason} {}", last_confirmed()))
            }
        }
        Verdict::Rejected { reason } => CommandOutcome::Rejected {
            reason: format!("The headphones refused the change ({reason}). {}", last_confirmed()),
        },
    }
}

fn device_error(e: LinkError) -> DeviceError {
    match e.kind {
        LinkErrorKind::Unreachable => DeviceError::Unreachable,
        LinkErrorKind::Device => DeviceError::Rejected(e.message),
        LinkErrorKind::Unsupported => DeviceError::Unsupported {
            feature: "Noise control".to_string(),
            reason: e.message,
        },
        LinkErrorKind::UnexpectedFraming | LinkErrorKind::NoEndpoint => {
            DeviceError::InvalidResponse(e.message)
        }
        LinkErrorKind::ChannelFailed | LinkErrorKind::BackingOff => {
            DeviceError::ControlLink(e.message)
        }
    }
}

#[async_trait]
impl BoseDevice for RealBoseDevice {
    fn source(&self) -> DeviceSource {
        DeviceSource::Real
    }

    async fn capabilities(&self) -> CapabilitySet {
        let _ = self.refresh();
        let obs = self.observed.read().clone();
        let link = self.link_status.read().clone();
        self.build_capabilities(&obs, &link)
    }

    async fn snapshot(&self) -> DeviceResult<DeviceSnapshot> {
        if !self.is_attached() {
            // Make sure nothing is left open, then say nothing about a device
            // this application is not looking at.
            self.link_device_gone().await;
            return Ok(Self::detached_snapshot());
        }

        let found = self.refresh()?;
        let obs = self.observed.read().clone();
        let now = now_rfc3339();

        let connection = if !found {
            ConnectionState::Disconnected
        } else if obs.connected {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        };

        // Ask the headphones which mode they are in. This is a background
        // refresh, so it honours the link's backoff after a failed open.
        let target = control_target(&obs).ok();
        let mut last_error = None;
        let host_battery_missing = obs.battery.is_none();
        match &target {
            Some(target) if obs.connected => {
                let read = self
                    .with_link(target.clone(), move |link| {
                        let index = link.read_mode(false)?;

                        // The mode list says whether a custom mode exists and,
                        // while the headphones are in one, what it is called
                        // and how strong it is. It is read once, and again on
                        // every refresh spent in a custom mode, because that
                        // is when its level is on screen and may be changed
                        // from elsewhere. A failure is remembered by the link
                        // and surfaces as the capability's reason.
                        let in_custom = preset_for_index(link.config(), index).is_none();
                        let status = link.status();
                        let never_read = status.slots.is_none() && status.slots_problem.is_none();
                        if in_custom || never_read {
                            let _ = link.read_slots(false);
                        }

                        // The equalizer is on screen all the time, so it is
                        // read on every refresh — unless the headphones have
                        // already shown they will not answer it as documented.
                        if link.status().eq_problem.is_none() {
                            let _ = link.read_eq(false);
                        }

                        // The battery level normally comes from the host
                        // system. Where it has none to give — Android does
                        // not always pass one on — the headphones are asked.
                        if host_battery_missing {
                            let _ = link.read_battery(false);
                        }
                        Ok::<u8, LinkError>(index)
                    })
                    .await?;
                if let Err(e) = read {
                    last_error = Some(format!(
                        "The noise-control mode could not be read from the headphones: {e}"
                    ));
                }
            }
            _ => self.link_device_gone().await,
        }

        let link = self.link_status.read().clone();

        // The mode shown is always one the headphones reported, stamped with
        // when they reported it. If the read just now failed, that is the last
        // confirmed mode and `last_error` says the read failed.
        let noise_control = match (&target, &link.confirmed) {
            (Some(target), Some(confirmed)) if obs.connected => {
                Some(noise_state(&target.config, confirmed, link.slots.as_ref()))
            }
            _ => None,
        };

        Ok(DeviceSnapshot {
            source: DeviceSource::Real,
            connection,
            transport: if obs.connected {
                Transport::BluetoothClassic
            } else {
                Transport::None
            },
            identity: Self::identity_from(&obs, &link),
            capabilities: self.build_capabilities(&obs, &link),
            battery: battery_reading(
                obs.battery,
                if obs.connected { link.battery.as_ref() } else { None },
                &now,
            ),
            noise_control,
            // Only ever what the headphones reported, with when.
            equalizer: match (&target, &link.eq) {
                (Some(_), Some(reading)) if obs.connected => Some(eq_state(reading)),
                _ => None,
            },
            windows_audio: None,
            last_error,
            updated_at: now,
        })
    }

    async fn connect(&self) -> DeviceResult<CommandOutcome> {
        // Windows owns Bluetooth audio connection policy. We can observe the
        // connection, but initiating one for an already-paired A2DP device is
        // not something this application should force. Report honestly.
        //
        // What "Connect" does do is attach this application: from here on it
        // reads the headphones whenever Windows has them connected.
        let found = self.refresh()?;
        if !found {
            return Err(DeviceError::Unreachable);
        }
        self.attached.store(true, Ordering::SeqCst);
        let connected = self.observed.read().connected;
        if connected {
            Ok(CommandOutcome::applied())
        } else {
            Ok(CommandOutcome::sent_unverified(
                "Windows manages the audio connection for paired headphones. \
                 Power the headphones on and Windows will connect them automatically.",
            ))
        }
    }

    async fn disconnect(&self) -> DeviceResult<CommandOutcome> {
        Ok(CommandOutcome::unsupported(
            "Disconnecting a paired audio device is managed by Windows. \
             Use Windows Settings > Bluetooth & devices.",
        ))
    }

    async fn device_info(&self) -> DeviceResult<Option<DeviceIdentity>> {
        self.refresh()?;
        let obs = self.observed.read().clone();
        let link = self.link_status.read().clone();
        Ok(Self::identity_from(&obs, &link))
    }

    async fn battery(&self) -> DeviceResult<Option<BatteryReading>> {
        self.refresh()?;
        let obs = self.observed.read().clone();
        Ok(obs.battery.map(|percent| BatteryReading {
            percent,
            source: HOST_BATTERY,
            charging: None,
            read_at: now_rfc3339(),
        }))
    }

    /// Reads the mode from the headphones now.
    ///
    /// `Ok(None)` means there is nothing to ask — no device, not connected, or
    /// not a model this build talks to. A device that should have answered and
    /// did not is an error, not a `None` and certainly not a default.
    async fn noise_control(&self) -> DeviceResult<Option<NoiseControlState>> {
        if !self.is_attached() {
            return Ok(None);
        }
        let found = self.refresh()?;
        let obs = self.observed.read().clone();
        if !found || !obs.connected {
            self.link_device_gone().await;
            return Ok(None);
        }
        let Ok(target) = control_target(&obs) else {
            return Ok(None);
        };

        let config = target.config.clone();
        // An explicit read: the backoff does not apply.
        self.with_link(target, |link| {
            let index = link.read_mode(true)?;
            if preset_for_index(link.config(), index).is_none() {
                // In a custom mode, its name and level are part of the answer.
                let _ = link.read_slots(true);
            }
            Ok::<u8, LinkError>(index)
        })
        .await?
        .map_err(device_error)?;

        let link = self.link_status.read();
        Ok(link
            .confirmed
            .as_ref()
            .map(|c| noise_state(&config, c, link.slots.as_ref())))
    }

    fn reported_battery(&self) -> Option<u8> {
        if !self.is_attached() || !self.observed.read().connected {
            return None;
        }
        self.link_status.read().battery.as_ref().map(|b| b.percent)
    }

    /// One `GET` of the current mode over the link that is already open.
    ///
    /// Unlike [`noise_control`](Self::noise_control) this does not look the
    /// device up in Windows first: it goes by what the last full refresh
    /// found, so it costs one small request and can be made several times a
    /// second. It honours the link's backoff, and reads a custom mode's name
    /// only if that has never been read.
    async fn peek_noise_control(&self) -> DeviceResult<Option<NoiseControlState>> {
        if !self.is_attached() {
            return Ok(None);
        }
        let obs = self.observed.read().clone();
        if !obs.connected {
            return Ok(None);
        }
        let Ok(target) = control_target(&obs) else {
            return Ok(None);
        };

        let config = target.config.clone();
        self.with_link(target, |link| {
            let index = link.read_mode(false)?;
            let status = link.status();
            let unnamed = status.slots.is_none() && status.slots_problem.is_none();
            if unnamed && preset_for_index(link.config(), index).is_none() {
                let _ = link.read_slots(false);
            }
            Ok::<u8, LinkError>(index)
        })
        .await?
        .map_err(device_error)?;

        let link = self.link_status.read();
        Ok(link
            .confirmed
            .as_ref()
            .map(|c| noise_state(&config, c, link.slots.as_ref())))
    }

    async fn set_noise_control(&self, mode: NoiseControlMode) -> DeviceResult<CommandOutcome> {
        let wanted = match mode {
            NoiseControlMode::Quiet => Wanted::Preset(Preset::Quiet),
            NoiseControlMode::Aware => Wanted::Preset(Preset::Aware),
            NoiseControlMode::Custom => Wanted::Custom,
            // Decided before anything touches the hardware.
            NoiseControlMode::Off => {
                return Ok(CommandOutcome::unsupported(
                    "These headphones have no verified way to turn noise control off, so \
                     nothing was sent.",
                ));
            }
        };

        if !self.is_attached() {
            return Err(DeviceError::NotConnected);
        }
        if !self.refresh()? {
            return Err(DeviceError::Unreachable);
        }
        let obs = self.observed.read().clone();
        if !obs.connected {
            return Err(DeviceError::NotConnected);
        }
        let target = match control_target(&obs) {
            Ok(target) => target,
            Err(why) => return Ok(CommandOutcome::unsupported(why)),
        };

        let config = target.config.clone();
        let change = self
            .with_link(target, move |link| {
                let target = match wanted {
                    Wanted::Preset(preset) => Target::Preset(preset),
                    // "Custom" is whichever custom mode the headphones have.
                    // The list is read fresh, so the choice is made from what
                    // they hold now.
                    Wanted::Custom => {
                        let current = link.read_mode(true)?;
                        let slots = link.read_slots(true)?;
                        let last = link.status().last_custom_slot;
                        match pick_custom_slot(link.config(), &slots, Some(current), last) {
                            Some(slot) => Target::Slot {
                                index: slot.index,
                                name: slot.name.clone(),
                            },
                            None => return Ok(None),
                        }
                    }
                };
                link.change_to(target).map(Some)
            })
            .await?
            .map_err(device_error)?;

        Ok(match change {
            Some(change) => outcome_for(&change, &config),
            None => CommandOutcome::unsupported(NO_CUSTOM_MODE),
        })
    }

    async fn set_noise_control_level(&self, level: u8) -> DeviceResult<CommandOutcome> {
        if level > MAX_RAW_LEVEL {
            return Err(DeviceError::InvalidInput(format!(
                "noise control level must be 0-{MAX_RAW_LEVEL}, got {level}"
            )));
        }

        if !self.is_attached() {
            return Err(DeviceError::NotConnected);
        }
        if !self.refresh()? {
            return Err(DeviceError::Unreachable);
        }
        let obs = self.observed.read().clone();
        if !obs.connected {
            return Err(DeviceError::NotConnected);
        }
        let target = match control_target(&obs) {
            Ok(target) => target,
            Err(why) => return Ok(CommandOutcome::unsupported(why)),
        };

        // The slider counts cancellation up; the device counts it down.
        let raw = flip_level(level);
        let change = self
            .with_link(target, move |link| {
                // The level belongs to a custom mode, and only the one the
                // headphones are in is ever edited.
                let current = link.read_mode(true)?;
                if let Some(preset) = preset_for_index(link.config(), current) {
                    return Ok(Err(preset));
                }
                link.set_custom_level(current, raw).map(Ok)
            })
            .await?
            .map_err(device_error)?;

        Ok(match change {
            Ok(change) => level_outcome(&change),
            Err(preset) => CommandOutcome::unsupported(format!(
                "The headphones are in {}, which has a fixed level, so nothing was sent. Select \
                 Custom first.",
                preset.label()
            )),
        })
    }

    /// Reads the equalizer from the headphones now. `Ok(None)` means there is
    /// nothing to ask, as for `noise_control`.
    async fn equalizer(&self) -> DeviceResult<Option<EqState>> {
        if !self.is_attached() {
            return Ok(None);
        }
        let found = self.refresh()?;
        let obs = self.observed.read().clone();
        if !found || !obs.connected {
            self.link_device_gone().await;
            return Ok(None);
        }
        let Ok(target) = control_target(&obs) else {
            return Ok(None);
        };
        self.with_link(target, |link| link.read_eq(true))
            .await?
            .map_err(device_error)?;
        Ok(self.link_status.read().eq.as_ref().map(eq_state))
    }

    async fn set_equalizer(&self, settings: EqSettings) -> DeviceResult<CommandOutcome> {
        if !settings.is_within_range() {
            return Err(DeviceError::InvalidInput(
                "EQ gains out of range".to_string(),
            ));
        }
        if !self.is_attached() {
            return Err(DeviceError::NotConnected);
        }
        if !self.refresh()? {
            return Err(DeviceError::Unreachable);
        }
        let obs = self.observed.read().clone();
        if !obs.connected {
            return Err(DeviceError::NotConnected);
        }
        let target = match control_target(&obs) {
            Ok(target) => target,
            Err(why) => return Ok(CommandOutcome::unsupported(why)),
        };

        let requested = EqLevels {
            bass: settings.bass,
            mid: settings.mid,
            treble: settings.treble,
        };
        let change = self
            .with_link(target, move |link| link.set_eq(requested))
            .await?
            .map_err(device_error)?;
        Ok(eq_outcome(&change))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::CapabilityStatus;
    use control::custom::testing::slot;
    use control::endpoint::{Endpoint, Route};
    use control::link::LevelProof;
    use control::noise::Ack;

    fn prince() -> ::bmap::DeviceConfig {
        control::config_for_product(0x4075).unwrap()
    }

    /// The test headphones as Windows describes them: connected, product
    /// 0x4075, advertising BMAP.
    fn aurora() -> Observed {
        Observed {
            instance_id: Some("BTHENUM\\DEV_TEST".into()),
            name: Some("Aurora".into()),
            connected: true,
            battery: Some(72),
            battery_ever_read: true,
            address: Some(0x0000_1122_3344_5566),
            product_id: Some(0x4075),
            advertises_bmap: true,
        }
    }

    fn confirmed(index: u8) -> Option<Confirmed> {
        Some(Confirmed {
            index,
            at: "2026-10-10T09:00:00Z".into(),
        })
    }

    fn transition(before: u8, after: u8, at: &str) -> Option<TransitionProof> {
        Some(TransitionProof {
            before,
            after,
            at: at.into(),
        })
    }

    /// A link that has validated its endpoint and read the mode once.
    fn open_link(index: u8) -> LinkStatus {
        LinkStatus {
            endpoint: Some(Endpoint {
                route: Route::SerialPortService,
                channel: None,
                firmware: "1.0.6-80+f5f219b".into(),
            }),
            firmware: Some("1.0.6-80+f5f219b".into()),
            confirmed: confirmed(index),
            ..LinkStatus::default()
        }
    }

    fn caps(obs: &Observed, link: &LinkStatus) -> CapabilitySet {
        RealBoseDevice::new(None).build_capabilities(obs, link)
    }

    /// The mode list of the test headphones: two presets, one custom mode
    /// called "Focus" at the given raw level, and one empty slot.
    fn slots(focus_raw: u8) -> Option<SlotTable> {
        Some(SlotTable {
            slots: vec![
                slot(0, "Quiet", 0, false),
                slot(1, "Aware", 10, false),
                slot(2, "Focus", focus_raw, true),
                slot(3, "", 0, true),
            ],
            at: "2026-10-10T09:00:00Z".into(),
        })
    }

    fn level_proof(before_raw: u8, after_raw: u8, at: &str) -> Option<LevelProof> {
        Some(LevelProof {
            slot: 2,
            name: "Focus".into(),
            before_raw,
            after_raw,
            at: at.into(),
        })
    }

    fn level_change(before_raw: u8, after_raw: Option<u8>, verdict: Verdict) -> LevelChange {
        LevelChange {
            slot: 2,
            name: "Focus".into(),
            requested_raw: 3,
            before_raw,
            sent: true,
            ack: Some(Ack::Acknowledged),
            after: after_raw.map(|raw| slot(2, "Focus", raw, true)),
            verdict,
        }
    }

    fn change(before: u8, after: Option<u8>, sent: bool, verdict: Verdict) -> ModeChange {
        ModeChange {
            requested: Target::Preset(Preset::Aware),
            requested_index: 1,
            before,
            sent,
            ack: sent.then_some(Ack::Acknowledged),
            after,
            verdict,
        }
    }

    // --- which devices get a control link -----------------------------------

    #[test]
    fn the_test_headphones_qualify_for_a_control_link() {
        let target = control_target(&aurora()).unwrap();
        assert_eq!(target.product_id, 0x4075);
        assert_eq!(target.config.info.codename, "prince");
    }

    /// A Bose model upstream knows but this application has never been run
    /// against must not be sent anything.
    #[test]
    fn an_untested_bose_model_gets_no_control_link() {
        let obs = Observed {
            product_id: Some(0x4082),
            ..aurora()
        };
        let why = control_target(&obs).err().unwrap();
        assert!(why.contains("0x4082"), "{why}");
    }

    #[test]
    fn a_device_windows_cannot_identify_gets_no_control_link() {
        let obs = Observed {
            product_id: None,
            ..aurora()
        };
        assert!(control_target(&obs).is_err());
    }

    /// The right product id is not sufficient on its own: Windows' service
    /// discovery must also show the device advertising BMAP.
    #[test]
    fn a_device_not_advertising_bmap_gets_no_control_link() {
        let obs = Observed {
            advertises_bmap: false,
            ..aurora()
        };
        let why = control_target(&obs).err().unwrap();
        assert!(why.contains("BMAP"), "{why}");
    }

    // --- capability evidence ------------------------------------------------

    #[test]
    fn capabilities_without_a_device_are_all_unknown() {
        let caps = caps(&Observed::default(), &LinkStatus::default());
        assert_eq!(caps.verified_count(), 0);
        for cap in caps.iter() {
            assert_eq!(cap.status, CapabilityStatus::Unknown);
        }
    }

    /// A device that never reported battery must not be described as having
    /// battery support.
    #[test]
    fn battery_capability_requires_an_actual_reading() {
        let obs = Observed {
            battery: None,
            battery_ever_read: false,
            ..aurora()
        };
        let caps = caps(&obs, &LinkStatus::default());
        let battery = caps.get(CapabilityKey::Battery);
        assert!(!battery.hardware_verified);
        assert_ne!(battery.status, CapabilityStatus::Verified);
    }

    #[test]
    fn battery_capability_is_verified_once_actually_read() {
        let caps = caps(&aurora(), &LinkStatus::default());
        let battery = caps.get(CapabilityKey::Battery);
        assert!(battery.hardware_verified);
        assert_eq!(battery.status, CapabilityStatus::Verified);
    }

    /// Presence of the right model is not evidence about noise control. Until
    /// the headphones have actually answered on the control channel, it stays
    /// UNKNOWN.
    #[test]
    fn noise_control_is_unknown_until_the_headphones_have_answered() {
        let caps = caps(&aurora(), &LinkStatus::default());
        for key in [CapabilityKey::NoiseControl, CapabilityKey::AwareMode] {
            assert_eq!(caps.get(key).status, CapabilityStatus::Unknown, "{key:?}");
        }
    }

    /// Reading the mode successfully makes the control usable — with a caveat.
    #[test]
    fn a_validated_link_makes_noise_control_experimental_not_verified() {
        let caps = caps(&aurora(), &open_link(0));
        for key in [CapabilityKey::NoiseControl, CapabilityKey::AwareMode] {
            let cap = caps.get(key);
            assert_eq!(cap.status, CapabilityStatus::Experimental, "{key:?}");
            assert!(!cap.hardware_verified, "{key:?}");
            assert_eq!(cap.mechanism, Mechanism::RfcommVendor);
            assert!(cap.evidence.contains("not yet"), "{}", cap.evidence);
        }
    }

    /// One confirmed direction is half the evidence, and half is EXPERIMENTAL.
    #[test]
    fn one_confirmed_direction_is_still_experimental() {
        for link in [
            LinkStatus {
                to_aware: transition(0, 1, "2026-10-10T09:01:00Z"),
                ..open_link(1)
            },
            LinkStatus {
                to_quiet: transition(1, 0, "2026-10-10T09:01:00Z"),
                ..open_link(0)
            },
        ] {
            let caps = caps(&aurora(), &link);
            for key in [CapabilityKey::NoiseControl, CapabilityKey::AwareMode] {
                let cap = caps.get(key);
                assert_eq!(cap.status, CapabilityStatus::Experimental, "{key:?}");
                assert!(!cap.hardware_verified);
                assert!(cap.evidence.contains("confirmed at 2026-10-10T09:01:00Z"));
            }
        }
    }

    /// Both directions confirmed by the headphones is the only route to
    /// VERIFIED, and the evidence names both and when each was seen.
    #[test]
    fn both_confirmed_directions_verify_noise_control() {
        let link = LinkStatus {
            to_aware: transition(0, 1, "2026-10-10T09:01:00Z"),
            to_quiet: transition(1, 0, "2026-10-10T09:02:00Z"),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        for key in [CapabilityKey::NoiseControl, CapabilityKey::AwareMode] {
            let cap = caps.get(key);
            assert_eq!(cap.status, CapabilityStatus::Verified, "{key:?}");
            assert!(cap.hardware_verified);
            assert!(cap.evidence.contains("2026-10-10T09:01:00Z"), "{}", cap.evidence);
            assert!(cap.evidence.contains("2026-10-10T09:02:00Z"), "{}", cap.evidence);
        }
    }

    /// A reply that contradicts the documented framing takes the control away
    /// rather than leaving it usable.
    #[test]
    fn unexpected_framing_withdraws_the_control() {
        let link = LinkStatus {
            framing_problem: Some("expected 1 payload byte, got 2".into()),
            confirmed: None,
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::NoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("no changes will be sent"));
    }

    #[test]
    fn a_failed_validation_is_reported_as_the_reason() {
        let link = LinkStatus {
            last_failure: Some("no route answered like a BMAP endpoint".into()),
            ..LinkStatus::default()
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::NoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("no route answered"));
    }

    #[test]
    fn disconnected_headphones_have_no_usable_noise_control() {
        let obs = Observed {
            connected: false,
            ..aurora()
        };
        // Even with everything proven earlier in the session.
        let link = LinkStatus {
            to_aware: transition(0, 1, "2026-10-10T09:01:00Z"),
            to_quiet: transition(1, 0, "2026-10-10T09:02:00Z"),
            ..LinkStatus::default()
        };
        let caps = caps(&obs, &link);
        assert_eq!(
            caps.get(CapabilityKey::NoiseControl).status,
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn firmware_is_verified_only_by_the_headphones_own_answer() {
        let before = caps(&aurora(), &LinkStatus::default());
        assert_eq!(
            before.get(CapabilityKey::FirmwareVersion).status,
            CapabilityStatus::Unknown
        );

        let after = caps(&aurora(), &open_link(0));
        let fw = after.get(CapabilityKey::FirmwareVersion);
        assert_eq!(fw.status, CapabilityStatus::Verified);
        assert!(fw.evidence.contains("1.0.6-80+f5f219b"));
    }

    /// Features this build does not drive must not ride along on noise
    /// control's evidence.
    #[test]
    fn undriven_vendor_features_stay_unknown_on_a_fully_verified_link() {
        let link = LinkStatus {
            to_aware: transition(0, 1, "2026-10-10T09:01:00Z"),
            to_quiet: transition(1, 0, "2026-10-10T09:02:00Z"),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        for key in [
            CapabilityKey::Multipoint,
            CapabilityKey::Sidetone,
            CapabilityKey::DeviceRename,
        ] {
            assert_eq!(caps.get(key).status, CapabilityStatus::Unknown, "{key:?}");
        }
    }

    // --- the equalizer ------------------------------------------------------

    fn eq_levels(bass: i8, mid: i8, treble: i8) -> EqLevels {
        EqLevels { bass, mid, treble }
    }
    fn eq_reading(bass: i8, mid: i8, treble: i8) -> Option<EqReading> {
        Some(EqReading {
            levels: eq_levels(bass, mid, treble),
            at: "2026-10-10T09:00:00Z".into(),
        })
    }
    fn eq_change(before: EqLevels, after: Option<EqLevels>, verdict: Verdict) -> EqChange {
        EqChange {
            requested: eq_levels(4, -2, 3),
            before,
            sent: true,
            ack: Some(Ack::Acknowledged),
            after,
            verdict,
        }
    }

    #[test]
    fn the_equalizer_is_unknown_until_the_headphones_have_reported_it() {
        let caps = caps(&aurora(), &open_link(0));
        let cap = caps.get(CapabilityKey::Equalizer);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("has not been read"), "{}", cap.evidence);
    }

    #[test]
    fn a_reported_equalizer_is_experimental_and_quoted() {
        let link = LinkStatus {
            eq: eq_reading(6, 5, -1),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::Equalizer);
        assert_eq!(cap.status, CapabilityStatus::Experimental);
        assert!(!cap.hardware_verified);
        assert!(cap.evidence.contains("bass +6, mid +5, treble -1"), "{}", cap.evidence);
    }

    #[test]
    fn a_confirmed_equalizer_change_verifies_it() {
        let link = LinkStatus {
            eq: eq_reading(4, -2, 3),
            eq_change: Some(control::link::EqProof {
                before: eq_levels(0, 0, 0),
                after: eq_levels(4, -2, 3),
                at: "2026-10-10T09:05:00Z".into(),
            }),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::Equalizer);
        assert_eq!(cap.status, CapabilityStatus::Verified);
        assert!(cap.hardware_verified);
        assert!(cap.evidence.contains("2026-10-10T09:05:00Z"), "{}", cap.evidence);
        assert!(cap.evidence.contains("bass +4, mid -2, treble +3"), "{}", cap.evidence);
    }

    #[test]
    fn an_equalizer_the_headphones_will_not_report_is_left_alone() {
        let link = LinkStatus {
            eq_problem: Some("Device error 3: FuncNotSupp".into()),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::Equalizer);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("FuncNotSupp"), "{}", cap.evidence);
        // Noise control does not depend on it.
        assert_eq!(
            caps.get(CapabilityKey::NoiseControl).status,
            CapabilityStatus::Experimental
        );
    }

    #[test]
    fn equalizer_state_is_the_headphones_report_with_its_time() {
        let state = eq_state(&eq_reading(6, 5, -1).unwrap());
        assert_eq!(
            (state.settings.bass, state.settings.mid, state.settings.treble),
            (6, 5, -1)
        );
        assert_eq!(state.source, StateSource::VendorProtocol);
        assert_eq!(state.read_at, "2026-10-10T09:00:00Z");
    }

    #[test]
    fn only_a_matching_equalizer_readback_is_reported_as_applied() {
        let zero = eq_levels(0, 0, 0);
        let want = eq_levels(4, -2, 3);
        assert!(eq_outcome(&eq_change(zero, Some(want), Verdict::Verified)).is_confirmed());

        let partly = Verdict::Unverified {
            reason: "the headphones report bass +4, mid +0, treble +0".into(),
        };
        let outcome = eq_outcome(&eq_change(zero, Some(eq_levels(4, 0, 0)), partly));
        assert!(matches!(outcome, CommandOutcome::SentUnverified { .. }));

        let lost = Verdict::Unverified {
            reason: "The equalizer could not be read back.".into(),
        };
        let CommandOutcome::SentUnverified { reason } = eq_outcome(&eq_change(zero, None, lost))
        else {
            panic!("expected SentUnverified");
        };
        assert!(reason.contains("confirmed was bass +0, mid +0, treble +0"), "{reason}");
    }

    // --- the custom mode ----------------------------------------------------

    /// Verified presets say nothing about the custom mode. Until the mode
    /// list has been read there is no evidence one even exists.
    #[test]
    fn the_custom_mode_is_unknown_until_the_mode_list_has_been_read() {
        let link = LinkStatus {
            to_aware: transition(0, 1, "2026-10-10T09:01:00Z"),
            to_quiet: transition(1, 0, "2026-10-10T09:02:00Z"),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::CustomNoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("has not been read"), "{}", cap.evidence);
    }

    #[test]
    fn a_listed_custom_mode_is_experimental_and_named() {
        let link = LinkStatus {
            slots: slots(6),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::CustomNoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Experimental);
        assert!(!cap.hardware_verified);
        assert!(cap.evidence.contains("\"Focus\""), "{}", cap.evidence);
        // Raw 6 on the device's scale is strength 4 on the slider's.
        assert!(cap.evidence.contains("level 4 of 10"), "{}", cap.evidence);
    }

    /// Headphones with no custom mode set up: "Custom" is not offered, and the
    /// reason says what to do about it. Nothing is created on their behalf.
    #[test]
    fn headphones_without_a_custom_mode_offer_none() {
        let empty = SlotTable {
            slots: vec![
                slot(0, "Quiet", 0, false),
                slot(1, "Aware", 10, false),
                slot(2, "None", 0, true),
                slot(3, "", 0, true),
            ],
            at: "2026-10-10T09:00:00Z".into(),
        };
        let link = LinkStatus {
            slots: Some(empty),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::CustomNoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("does not create one"), "{}", cap.evidence);
    }

    /// Each half on its own is still EXPERIMENTAL: a confirmed switch says
    /// nothing about the level, and a confirmed level change says nothing
    /// about getting into the mode.
    #[test]
    fn one_confirmed_half_of_the_custom_mode_is_still_experimental() {
        for link in [
            LinkStatus {
                slots: slots(6),
                to_custom: transition(0, 2, "2026-10-10T09:03:00Z"),
                ..open_link(2)
            },
            LinkStatus {
                slots: slots(3),
                level: level_proof(6, 3, "2026-10-10T09:03:00Z"),
                ..open_link(2)
            },
        ] {
            let caps = caps(&aurora(), &link);
            let cap = caps.get(CapabilityKey::CustomNoiseControl);
            assert_eq!(cap.status, CapabilityStatus::Experimental);
            assert!(!cap.hardware_verified);
            assert!(cap.evidence.contains("confirmed at 2026-10-10T09:03:00Z"));
            assert!(cap.evidence.contains("not yet"));
        }
    }

    #[test]
    fn a_confirmed_switch_and_a_confirmed_level_verify_the_custom_mode() {
        let link = LinkStatus {
            slots: slots(3),
            to_custom: transition(0, 2, "2026-10-10T09:03:00Z"),
            level: level_proof(6, 3, "2026-10-10T09:04:00Z"),
            ..open_link(2)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::CustomNoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Verified);
        assert!(cap.hardware_verified);
        // In the slider's terms: raw 6 -> 3 is strength 4 -> 7.
        assert!(cap.evidence.contains("Focus at level 4"), "{}", cap.evidence);
        assert!(cap.evidence.contains("Focus at level 7"), "{}", cap.evidence);
        assert!(cap.evidence.contains("2026-10-10T09:03:00Z"), "{}", cap.evidence);
        assert!(cap.evidence.contains("2026-10-10T09:04:00Z"), "{}", cap.evidence);

        // And it does not rub off on the presets, which have their own rule.
        assert_eq!(
            caps.get(CapabilityKey::NoiseControl).status,
            CapabilityStatus::Experimental
        );
    }

    #[test]
    fn an_undocumented_mode_list_withdraws_the_custom_mode_only() {
        let link = LinkStatus {
            slots_problem: Some("a mode configuration carried 48 bytes where 47 are documented".into()),
            ..open_link(0)
        };
        let caps = caps(&aurora(), &link);
        let cap = caps.get(CapabilityKey::CustomNoiseControl);
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(cap.evidence.contains("48 bytes"), "{}", cap.evidence);
        // Quiet/Aware do not depend on the mode list.
        assert_eq!(
            caps.get(CapabilityKey::NoiseControl).status,
            CapabilityStatus::Experimental
        );
    }

    #[test]
    fn custom_means_the_mode_the_headphones_are_in_then_the_last_then_the_first() {
        let config = prince();
        let two = vec![
            slot(0, "Quiet", 0, false),
            slot(1, "Aware", 10, false),
            slot(2, "Focus", 6, true),
            slot(3, "Commute", 2, true),
        ];
        let pick = |current, last| {
            pick_custom_slot(&config, &two, current, last).map(|s| s.name.as_str())
        };
        assert_eq!(pick(Some(3), Some(2)), Some("Commute"), "the one they are in");
        assert_eq!(pick(Some(0), Some(3)), Some("Commute"), "the one last seen");
        assert_eq!(pick(Some(0), None), Some("Focus"), "otherwise the first");
        // A preset index, or a slot that no longer exists, is never picked.
        assert_eq!(pick(Some(1), Some(1)), Some("Focus"));
        assert_eq!(pick(None, Some(9)), Some("Focus"));

        let none = vec![slot(0, "Quiet", 0, false), slot(3, "", 0, true)];
        assert!(pick_custom_slot(&config, &none, Some(3), Some(3)).is_none());
    }

    // --- state and outcomes -------------------------------------------------

    #[test]
    fn mode_indices_map_to_modes_through_the_model_config() {
        let config = prince();
        let mode = |index| noise_state(&config, &confirmed(index).unwrap(), None).mode;
        assert_eq!(mode(0), NoiseControlMode::Quiet);
        assert_eq!(mode(1), NoiseControlMode::Aware);
        // Slots 2 and 3 are user-configurable on this model.
        assert_eq!(mode(2), NoiseControlMode::Custom);
        assert_eq!(mode(3), NoiseControlMode::Custom);
    }

    #[test]
    fn reported_state_carries_the_time_the_headphones_were_read() {
        let state = noise_state(&prince(), &confirmed(1).unwrap(), None);
        assert_eq!(state.read_at, "2026-10-10T09:00:00Z");
        assert_eq!(state.source, StateSource::VendorProtocol);
    }

    /// In a custom mode the state carries the mode's own name and its level
    /// on the slider's scale — both from the headphones' mode list.
    #[test]
    fn a_custom_mode_reports_its_name_and_its_strength() {
        let table = slots(6);
        let state = noise_state(&prince(), &confirmed(2).unwrap(), table.as_ref());
        assert_eq!(state.mode, NoiseControlMode::Custom);
        assert_eq!(state.label.as_deref(), Some("Focus"));
        assert_eq!(state.level, Some(4), "raw 6 is strength 4");
    }

    /// No mode list, no level. It is not filled in with a plausible number.
    #[test]
    fn a_custom_mode_without_a_mode_list_has_no_level() {
        let state = noise_state(&prince(), &confirmed(2).unwrap(), None);
        assert_eq!(state.mode, NoiseControlMode::Custom);
        assert_eq!(state.level, None);
        assert_eq!(state.label, None);
    }

    /// The presets carry no level or name of their own, even though the mode
    /// list has entries for them.
    #[test]
    fn presets_report_neither_a_level_nor_a_label() {
        let table = slots(6);
        for index in [0, 1] {
            let state = noise_state(&prince(), &confirmed(index).unwrap(), table.as_ref());
            assert_eq!(state.level, None);
            assert_eq!(state.label, None);
        }
    }

    #[test]
    fn only_a_matching_level_readback_is_reported_as_applied() {
        assert!(level_outcome(&level_change(6, Some(3), Verdict::Verified)).is_confirmed());
        assert!(level_outcome(&level_change(3, Some(3), Verdict::AlreadyActive)).is_confirmed());

        let unchanged = Verdict::Unverified {
            reason: "The device acknowledged the change, but \"Focus\" still reports level 4.".into(),
        };
        let outcome = level_outcome(&level_change(6, Some(6), unchanged));
        assert!(matches!(outcome, CommandOutcome::SentUnverified { .. }));
    }

    /// With no read-back, the user is told the last level actually confirmed —
    /// on the slider's scale, not the device's.
    #[test]
    fn an_unverified_level_with_no_readback_names_the_last_confirmed_level() {
        let verdict = Verdict::Unverified {
            reason: "The level could not be read back.".into(),
        };
        let CommandOutcome::SentUnverified { reason } =
            level_outcome(&level_change(6, None, verdict))
        else {
            panic!("expected SentUnverified");
        };
        assert!(reason.contains("confirmed for \"Focus\" was 4"), "{reason}");
    }

    #[test]
    fn a_refused_level_change_is_rejected() {
        let verdict = Verdict::Rejected {
            reason: "Device error 1: Length".into(),
        };
        let CommandOutcome::Rejected { reason } =
            level_outcome(&level_change(6, Some(6), verdict))
        else {
            panic!("expected Rejected");
        };
        assert!(reason.contains("Length"));
        assert!(reason.contains("was 4"), "{reason}");
    }

    #[test]
    fn only_a_matching_readback_is_reported_as_applied() {
        let config = prince();
        assert!(outcome_for(&change(0, Some(1), true, Verdict::Verified), &config).is_confirmed());
        assert!(
            outcome_for(&change(1, Some(1), false, Verdict::AlreadyActive), &config).is_confirmed()
        );
    }

    /// Sent and acknowledged, but the headphones still report the old mode:
    /// that is "sent, unverified", never success.
    #[test]
    fn an_acknowledged_but_unchanged_mode_is_sent_unverified() {
        let verdict = Verdict::Unverified {
            reason: "The device acknowledged the request, but it still reports Quiet.".into(),
        };
        let outcome = outcome_for(&change(0, Some(0), true, verdict), &prince());
        assert!(!outcome.is_confirmed());
        assert!(matches!(outcome, CommandOutcome::SentUnverified { .. }));
    }

    /// With no read-back at all, the user is told which mode was last
    /// actually confirmed.
    #[test]
    fn an_unverified_change_with_no_readback_names_the_last_confirmed_mode() {
        let verdict = Verdict::Unverified {
            reason: "The mode could not be read back.".into(),
        };
        let outcome = outcome_for(&change(0, None, true, verdict), &prince());
        let CommandOutcome::SentUnverified { reason } = outcome else {
            panic!("expected SentUnverified");
        };
        assert!(reason.contains("last mode the headphones confirmed was Quiet"), "{reason}");
    }

    #[test]
    fn a_refused_change_is_rejected_with_the_devices_reason() {
        let verdict = Verdict::Rejected {
            reason: "Device error 15: InvalidTransition".into(),
        };
        let outcome = outcome_for(&change(0, Some(0), true, verdict), &prince());
        let CommandOutcome::Rejected { reason } = outcome else {
            panic!("expected Rejected");
        };
        assert!(reason.contains("InvalidTransition"));
        assert!(reason.contains("Quiet"));
    }

    #[test]
    fn link_errors_keep_their_meaning_as_device_errors() {
        let err = |kind| {
            device_error(LinkError {
                kind,
                message: "detail".into(),
            })
        };
        assert!(matches!(err(LinkErrorKind::Unreachable), DeviceError::Unreachable));
        assert!(matches!(err(LinkErrorKind::ChannelFailed), DeviceError::ControlLink(_)));
        assert!(matches!(err(LinkErrorKind::NoEndpoint), DeviceError::InvalidResponse(_)));
        assert!(matches!(err(LinkErrorKind::UnexpectedFraming), DeviceError::InvalidResponse(_)));
        assert!(matches!(err(LinkErrorKind::Device), DeviceError::Rejected(_)));
    }

    // --- the backend as a whole, without hardware ---------------------------

    /// What this build does not drive is refused before any hardware is
    /// looked for, let alone written to.
    #[tokio::test]
    async fn undriven_requests_are_unsupported_and_touch_nothing() {
        let dev = RealBoseDevice::new(Some("NON_EXISTENT_DEVICE".to_string()));
        let outcome = dev.set_noise_control(NoiseControlMode::Off).await.unwrap();
        assert!(matches!(outcome, CommandOutcome::Unsupported { .. }));

        assert!(dev.control.lock().is_none(), "no control link was created");
    }

    /// Asking a device that is not there for anything must fail, not "succeed".
    #[tokio::test]
    async fn a_missing_device_never_reports_a_change() {
        let dev = RealBoseDevice::new(Some("NON_EXISTENT_DEVICE".to_string()));
        for mode in [
            NoiseControlMode::Quiet,
            NoiseControlMode::Aware,
            NoiseControlMode::Custom,
        ] {
            let result = dev.set_noise_control(mode).await;
            assert!(
                !matches!(&result, Ok(outcome) if outcome.is_confirmed()),
                "{mode:?}: {result:?}"
            );
        }
        let result = dev.set_noise_control_level(5).await;
        assert!(
            !matches!(&result, Ok(outcome) if outcome.is_confirmed()),
            "{result:?}"
        );
        let result = dev.set_equalizer(EqSettings::flat()).await;
        assert!(
            !matches!(&result, Ok(outcome) if outcome.is_confirmed()),
            "{result:?}"
        );
        assert!(dev.control.lock().is_none(), "no control link was created");
    }

    /// Detached, the backend says "disconnected" and nothing else — and no
    /// request reaches for the hardware, whatever Windows has connected.
    #[tokio::test]
    async fn a_detached_backend_reports_nothing_and_sends_nothing() {
        let attached = Arc::new(AtomicBool::new(false));
        // No preferred id: on the development machine this would find the
        // real headphones if it looked. It must not look.
        let dev = RealBoseDevice::with_attachment(None, attached);

        let snap = dev.snapshot().await.unwrap();
        assert_eq!(snap.connection, ConnectionState::Disconnected);
        assert!(snap.identity.is_none() && snap.battery.is_none());
        assert!(snap.noise_control.is_none() && snap.equalizer.is_none());
        assert_eq!(snap.capabilities.verified_count(), 0);
        let cap = snap.capabilities.get(CapabilityKey::NoiseControl);
        assert!(cap.evidence.contains("Press Connect"), "{}", cap.evidence);

        assert!(matches!(dev.noise_control().await, Ok(None)));
        assert!(matches!(dev.equalizer().await, Ok(None)));
        for result in [
            dev.set_noise_control(NoiseControlMode::Quiet).await,
            dev.set_noise_control(NoiseControlMode::Custom).await,
            dev.set_noise_control_level(5).await,
            dev.set_equalizer(EqSettings::flat()).await,
        ] {
            assert!(matches!(result, Err(DeviceError::NotConnected)), "{result:?}");
        }
        assert!(dev.control.lock().is_none(), "no control link was created");
        assert!(dev.observed.read().instance_id.is_none(), "Windows was not even asked");
    }

    /// An out-of-range level is refused before the device is even looked for.
    #[tokio::test]
    async fn an_out_of_range_level_is_invalid_input() {
        let dev = RealBoseDevice::new(Some("NON_EXISTENT_DEVICE".to_string()));
        let err = dev.set_noise_control_level(11).await.unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput(_)), "{err:?}");
    }

    #[tokio::test]
    async fn unknown_state_is_none_not_a_plausible_default() {
        let dev = RealBoseDevice::new(Some("NON_EXISTENT_DEVICE".to_string()));
        // `Err` on a machine with no Bluetooth radio; never a made-up value.
        assert!(!matches!(dev.noise_control().await, Ok(Some(_))));
        assert!(!matches!(dev.equalizer().await, Ok(Some(_))));
    }
}

/// Stage B: the physical headphones, driven through the same backend the
/// application uses.
///
/// Every test is `#[ignore]`d. The `hw_b` tests need the headphones paired,
/// powered on and connected to this PC. Run them one at a time, in order:
///
/// ```text
/// cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_b
/// ```
///
/// `hw_b1` changes the noise-control mode (and changes it back). `hw_b5`
/// switches to the custom mode, changes its level, and puts both back. The
/// other `hw_b` tests only read. `hw_c1` is the odd one out: it needs the
/// headphones switched off, and checks that nothing is sent to a device that
/// is not there.
#[cfg(all(test, windows))]
mod hardware_tests {
    use super::*;
    use crate::device::CapabilityStatus;

    impl RealBoseDevice {
        fn take_wire_log(&self) -> Vec<String> {
            self.control
                .lock()
                .as_mut()
                .map(|c| c.link.take_wire_log())
                .unwrap_or_default()
                .iter()
                .map(|e| e.render())
                .collect()
        }

        /// Closes the RFCOMM connection underneath the backend, as an
        /// unexpected drop would, without telling it the device has gone.
        fn drop_connection(&self) {
            if let Some(c) = self.control.lock().as_mut() {
                c.link.close();
            }
        }
    }

    fn print_wire(dev: &RealBoseDevice) -> Vec<String> {
        let log = dev.take_wire_log();
        for line in &log {
            println!("      {line}");
        }
        log
    }

    fn status_of(snap: &DeviceSnapshot, key: CapabilityKey) -> CapabilityStatus {
        snap.capabilities.get(key).status
    }

    async fn connected_snapshot(dev: &RealBoseDevice) -> DeviceSnapshot {
        let snap = dev.snapshot().await.expect("snapshot");
        assert_eq!(
            snap.connection,
            ConnectionState::Connected,
            "the headphones must be powered on and connected to this PC"
        );
        snap
    }

    fn mode_of(snap: &DeviceSnapshot) -> NoiseControlMode {
        snap.noise_control
            .as_ref()
            .unwrap_or_else(|| panic!("no mode reported; last error: {:?}", snap.last_error))
            .mode
    }

    /// The operator is the low nibble of a frame's third byte. A rendered
    /// wire event is a direction tag followed by the bytes in hex.
    fn operator_of(rendered: &str) -> Option<u8> {
        let byte = rendered.split_whitespace().nth(3)?;
        Some(u8::from_str_radix(byte, 16).ok()? & 0x0F)
    }
    fn is_tx(rendered: &str) -> bool {
        rendered.starts_with("TX")
    }
    fn is_get(rendered: &str) -> bool {
        operator_of(rendered) == Some(1)
    }
    fn is_start(rendered: &str) -> bool {
        operator_of(rendered) == Some(5)
    }
    /// `START` on the mode-list function: asks for the mode configurations
    /// and changes nothing.
    fn is_mode_list_request(rendered: &str) -> bool {
        rendered.split_whitespace().skip(1).take(4).collect::<Vec<_>>() == ["1F", "01", "05", "00"]
    }
    /// A request that only reads.
    fn is_read(rendered: &str) -> bool {
        is_get(rendered) || is_mode_list_request(rendered)
    }
    /// A write to an equalizer band: SETGET on [1.7].
    fn is_eq_write(rendered: &str) -> bool {
        let bytes: Vec<_> = rendered.split_whitespace().skip(1).take(2).collect();
        bytes == ["01", "07"] && operator_of(rendered) == Some(2)
    }
    fn eq_of(snap: &DeviceSnapshot) -> EqSettings {
        snap.equalizer
            .as_ref()
            .unwrap_or_else(|| panic!("no equalizer reported; last error: {:?}", snap.last_error))
            .settings
    }
    /// A write to a mode configuration: SETGET on [31.6].
    fn is_slot_write(rendered: &str) -> bool {
        let bytes: Vec<_> = rendered.split_whitespace().skip(1).take(2).collect();
        bytes == ["1F", "06"] && operator_of(rendered) == Some(2)
    }

    fn level_of(snap: &DeviceSnapshot) -> Option<u8> {
        snap.noise_control.as_ref().and_then(|n| n.level)
    }

    /// The custom mode's configuration exactly as the headphones last sent it.
    fn custom_slot_bytes(dev: &RealBoseDevice, name: &str) -> Vec<u8> {
        dev.link_status
            .read()
            .slots
            .as_ref()
            .and_then(|t| t.slots.iter().find(|s| s.name == name))
            .map(|s| s.raw.clone())
            .expect("the custom mode is in the remembered mode list")
    }

    /// Read-only. Identifies the device, validates the endpoint and reads the
    /// mode, exactly as opening the app on real hardware does.
    #[tokio::test]
    #[ignore = "requires the physical headphones"]
    async fn hw_b0_snapshot_reads_the_mode() {
        let dev = RealBoseDevice::default();
        let snap = connected_snapshot(&dev).await;
        let identity = snap.identity.as_ref().expect("identity");

        println!("\n  device    : {}", identity.name);
        println!("  model     : {:?}", identity.model_number);
        println!("  firmware  : {:?}", identity.firmware_version);
        println!("  mode      : {:?}", mode_of(&snap));
        println!("  last error: {:?}", snap.last_error);
        let cap = snap.capabilities.get(CapabilityKey::NoiseControl);
        println!("  capability: {} - {}", cap.status.label(), cap.evidence);
        println!("  wire:");
        let wire = print_wire(&dev);

        assert!(identity.firmware_version.is_some(), "firmware read from the device");
        // Reading is not proof that switching works.
        assert_eq!(cap.status, CapabilityStatus::Experimental);
        assert!(!cap.hardware_verified);
        assert!(
            wire.iter().filter(|l| is_tx(l)).all(|l| is_read(l)),
            "a read-only snapshot transmitted something other than a read"
        );
    }

    /// Read-only. The snapshot lists the headphones' modes and reports what
    /// it found about a custom mode, without writing anything.
    #[tokio::test]
    #[ignore = "requires the physical headphones"]
    async fn hw_b4_snapshot_reports_the_custom_mode() {
        let dev = RealBoseDevice::default();
        let snap = connected_snapshot(&dev).await;
        let cap = snap.capabilities.get(CapabilityKey::CustomNoiseControl);
        println!("\n  mode      : {:?}", snap.noise_control);
        println!("  capability: {} - {}", cap.status.label(), cap.evidence);
        if let Some(table) = &dev.link_status.read().slots {
            for slot in &table.slots {
                println!(
                    "    slot {}  {:<10} strength {:>2}  editable {}",
                    slot.index,
                    format!("{:?}", slot.name),
                    slot.strength(),
                    slot.editable
                );
            }
        }
        println!("  wire:");
        let wire = print_wire(&dev);

        assert!(
            wire.iter().filter(|l| is_tx(l)).all(|l| is_read(l)),
            "a read-only snapshot transmitted something other than a read"
        );
        assert!(wire.iter().any(|l| is_mode_list_request(l)), "the mode list was read");
        assert!(!cap.hardware_verified, "listing a mode proves nothing about changing it");
        assert_ne!(cap.status, CapabilityStatus::Verified);
    }

    /// Switches to the custom mode, changes its level, and puts both back —
    /// each step confirmed against the headphones' own report.
    ///
    /// This edits a mode the user saved. It restores the original level
    /// whether or not the change in between was confirmed, and finally checks
    /// that the mode's configuration is byte-for-byte what it was at the
    /// start.
    #[tokio::test]
    #[ignore = "requires the physical headphones; CHANGES the mode and a custom mode's level"]
    async fn hw_b5_custom_mode_round_trip() {
        let dev = RealBoseDevice::default();
        let start = mode_of(&connected_snapshot(&dev).await);
        println!("\n  starting mode: {start:?}");

        // Start from a preset, so that switching *into* the custom mode is
        // something that actually happens and can be confirmed.
        if start == NoiseControlMode::Custom {
            let outcome = dev.set_noise_control(NoiseControlMode::Quiet).await;
            println!("  moved to Quiet first: {outcome:?}");
            assert!(outcome.expect("request").is_confirmed());
        }
        dev.take_wire_log();

        // --- into the custom mode -------------------------------------------
        println!("\n  step 1: request Custom");
        let into = dev.set_noise_control(NoiseControlMode::Custom).await;
        println!("    outcome: {into:?}");
        print_wire(&dev);
        let into = into.expect("the request failed outright");
        assert!(into.is_confirmed(), "the headphones did not confirm Custom: {into:?}");

        let snap = connected_snapshot(&dev).await;
        let state = snap.noise_control.clone().expect("a mode");
        println!(
            "    headphones now report: {:?} {:?} at level {:?}",
            state.mode, state.label, state.level
        );
        assert_eq!(state.mode, NoiseControlMode::Custom);
        let name = state.label.clone().expect("the custom mode has a name");
        let original = state.level.expect("the custom mode reports a level");
        let original_bytes = custom_slot_bytes(&dev, &name);
        dev.take_wire_log();

        // --- change the level, then put it back -----------------------------
        let changed = if original >= 5 { original - 2 } else { original + 2 };
        println!("\n  step 2: level {original} -> {changed}");
        let up = dev.set_noise_control_level(changed).await;
        println!("    outcome: {up:?}");
        let wire_up = print_wire(&dev);
        let after_up = level_of(&connected_snapshot(&dev).await);
        println!("    headphones now report level {after_up:?}");
        dev.take_wire_log();

        // Restored unconditionally, before anything is asserted.
        println!("\n  step 3: level back to {original}");
        let down = dev.set_noise_control_level(original).await;
        println!("    outcome: {down:?}");
        print_wire(&dev);
        let snap = connected_snapshot(&dev).await;
        let after_down = level_of(&snap);
        println!("    headphones now report level {after_down:?}");
        let restored_bytes = custom_slot_bytes(&dev, &name);
        let custom_cap = snap.capabilities.get(CapabilityKey::CustomNoiseControl).clone();
        dev.take_wire_log();

        // --- and back to where the headphones started -----------------------
        println!("\n  step 4: back to {start:?}");
        let back = dev.set_noise_control(start).await;
        println!("    outcome: {back:?}");
        print_wire(&dev);
        let end = mode_of(&connected_snapshot(&dev).await);
        println!("    headphones now report: {end:?}");

        // --- everything is checked only now, after the restore attempts -----
        assert!(up.expect("level change failed outright").is_confirmed());
        assert_eq!(after_up, Some(changed));
        assert_eq!(
            wire_up.iter().filter(|l| is_slot_write(l)).count(),
            1,
            "exactly one write for one level change"
        );

        assert!(down.expect("restoring the level failed outright").is_confirmed());
        assert_eq!(after_down, Some(original), "the original level is back");
        assert_eq!(
            restored_bytes, original_bytes,
            "the custom mode's configuration is byte-for-byte what it was"
        );

        assert!(back.expect("switching back failed outright").is_confirmed());
        assert_eq!(end, start);

        println!("\n  CustomNoiseControl: {} - {}", custom_cap.status.label(), custom_cap.evidence);
        assert_eq!(custom_cap.status, CapabilityStatus::Verified);
        assert!(custom_cap.hardware_verified);
    }

    /// Changes the mode, then changes it back, confirming each against the
    /// headphones' own report. Leaves the device in the mode it started in
    /// when that mode was Quiet or Aware.
    #[tokio::test]
    #[ignore = "requires the physical headphones; CHANGES the noise-control mode"]
    async fn hw_b1_switches_both_ways() {
        let dev = RealBoseDevice::default();
        let snap = connected_snapshot(&dev).await;
        let initial = mode_of(&snap);
        println!("\n  initial mode: {initial:?}");
        print_wire(&dev);

        // Away from the starting mode first, then back to it, so both
        // transitions are exercised wherever the device starts.
        let (first, second) = match initial {
            NoiseControlMode::Aware => (NoiseControlMode::Quiet, NoiseControlMode::Aware),
            _ => (NoiseControlMode::Aware, NoiseControlMode::Quiet),
        };

        for (step, target) in [first, second].into_iter().enumerate() {
            println!("\n  step {}: request {target:?}", step + 1);
            let outcome = dev.set_noise_control(target).await;
            println!("    outcome: {outcome:?}");
            print_wire(&dev);

            let outcome = outcome.expect("the request failed outright");
            assert!(
                outcome.is_confirmed(),
                "the headphones did not confirm {target:?}: {outcome:?}"
            );

            // Independently of the outcome: ask the headphones again.
            let snap = connected_snapshot(&dev).await;
            println!("    headphones now report: {:?}", mode_of(&snap));
            assert_eq!(mode_of(&snap), target);

            let expected = if step == 0 {
                CapabilityStatus::Experimental
            } else {
                CapabilityStatus::Verified
            };
            assert_eq!(status_of(&snap, CapabilityKey::NoiseControl), expected);
            println!(
                "    capability: {}",
                snap.capabilities.get(CapabilityKey::NoiseControl).status.label()
            );
        }

        let snap = connected_snapshot(&dev).await;
        for key in [CapabilityKey::NoiseControl, CapabilityKey::AwareMode] {
            let cap = snap.capabilities.get(key);
            println!("\n  {key:?}: {} - {}", cap.status.label(), cap.evidence);
            assert_eq!(cap.status, CapabilityStatus::Verified);
            assert!(cap.hardware_verified);
        }
    }

    /// Asking for the mode the headphones are already in must not transmit a
    /// mode change at all.
    #[tokio::test]
    #[ignore = "requires the physical headphones"]
    async fn hw_b2_asking_for_the_current_mode_sends_nothing() {
        let dev = RealBoseDevice::default();
        let current = mode_of(&connected_snapshot(&dev).await);
        assert!(
            matches!(current, NoiseControlMode::Quiet | NoiseControlMode::Aware),
            "put the headphones in Quiet or Aware first (currently {current:?})"
        );
        dev.take_wire_log();

        let outcome = dev.set_noise_control(current).await.expect("request");
        println!("\n  requested the current mode ({current:?}): {outcome:?}");
        let wire = print_wire(&dev);

        assert!(outcome.is_confirmed());
        assert!(!wire.iter().any(|l| is_start(l)), "a START was transmitted");
        // And it is not evidence that switching works.
        let snap = connected_snapshot(&dev).await;
        assert_eq!(
            status_of(&snap, CapabilityKey::NoiseControl),
            CapabilityStatus::Experimental
        );
    }

    /// The connection is dropped underneath the backend. The next refresh
    /// reconnects and reads again, with no pause to help it.
    #[tokio::test]
    #[ignore = "requires the physical headphones"]
    async fn hw_b3_recovers_when_the_connection_is_dropped() {
        let dev = RealBoseDevice::default();
        let before = mode_of(&connected_snapshot(&dev).await);
        dev.take_wire_log();

        for round in 1..=3 {
            dev.drop_connection();
            let started = std::time::Instant::now();
            let snap = connected_snapshot(&dev).await;
            println!(
                "\n  round {round}: reconnected and read {:?} in {:?} (last error: {:?})",
                snap.noise_control.as_ref().map(|n| n.mode),
                started.elapsed(),
                snap.last_error
            );
            print_wire(&dev);
            assert_eq!(mode_of(&snap), before);
            assert!(snap.last_error.is_none());
        }
    }

    /// Read-only. The snapshot carries the equalizer the headphones report.
    #[tokio::test]
    #[ignore = "requires the physical headphones"]
    async fn hw_b6_snapshot_reports_the_equalizer() {
        let dev = RealBoseDevice::default();
        let snap = connected_snapshot(&dev).await;
        let cap = snap.capabilities.get(CapabilityKey::Equalizer);
        println!("\n  equalizer : {:?}", snap.equalizer);
        println!("  capability: {} - {}", cap.status.label(), cap.evidence);
        println!("  wire:");
        let wire = print_wire(&dev);

        let eq = eq_of(&snap);
        assert!(eq.is_within_range());
        assert!(
            wire.iter().filter(|l| is_tx(l)).all(|l| is_read(l)),
            "a read-only snapshot transmitted something other than a read"
        );
        assert_eq!(cap.status, CapabilityStatus::Experimental);
        assert!(!cap.hardware_verified);
    }

    /// Changes one band by one step and puts it back, each confirmed against
    /// the headphones' own report. The restore is attempted whether or not
    /// the change in between was confirmed.
    #[tokio::test]
    #[ignore = "requires the physical headphones; CHANGES the equalizer"]
    async fn hw_b7_equalizer_round_trip() {
        let dev = RealBoseDevice::default();
        let original = eq_of(&connected_snapshot(&dev).await);
        println!("\n  original equalizer: {original:?}");
        dev.take_wire_log();

        let changed = EqSettings {
            bass: if original.bass >= 10 { original.bass - 1 } else { original.bass + 1 },
            ..original
        };
        println!("\n  step 1: -> {changed:?}");
        let there = dev.set_equalizer(changed).await;
        println!("    outcome: {there:?}");
        let wire_there = print_wire(&dev);
        let snap = connected_snapshot(&dev).await;
        let after_there = eq_of(&snap);
        let cap = snap.capabilities.get(CapabilityKey::Equalizer).clone();
        println!("    headphones now report: {after_there:?}");
        dev.take_wire_log();

        println!("\n  step 2: back to {original:?}");
        let back = dev.set_equalizer(original).await;
        println!("    outcome: {back:?}");
        print_wire(&dev);
        let after_back = eq_of(&connected_snapshot(&dev).await);
        println!("    headphones now report: {after_back:?}");

        assert!(there.expect("the change failed outright").is_confirmed());
        assert_eq!(after_there, changed);
        assert_eq!(wire_there.iter().filter(|l| is_eq_write(l)).count(), 3);
        assert!(back.expect("the restore failed outright").is_confirmed());
        assert_eq!(after_back, original, "the original equalizer is back");

        println!("\n  Equalizer: {} - {}", cap.status.label(), cap.evidence);
        assert_eq!(cap.status, CapabilityStatus::Verified);
        assert!(cap.hardware_verified);
    }

    /// The opposite precondition from every test above: run this one with the
    /// headphones paired but switched off, or connected to something else.
    ///
    /// A device that is not there must be reported as not there. No mode is
    /// shown for it, a request to change mode fails without a connection
    /// being attempted, and nothing is transmitted.
    #[tokio::test]
    #[ignore = "requires the headphones to be paired but NOT connected"]
    async fn hw_c1_disconnected_headphones_are_reported_as_such() {
        let dev = RealBoseDevice::default();
        let snap = dev.snapshot().await.expect("snapshot");
        assert_eq!(
            snap.connection,
            ConnectionState::Disconnected,
            "switch the headphones off before running this test"
        );

        let cap = snap.capabilities.get(CapabilityKey::NoiseControl);
        println!("
  connection : {:?}", snap.connection);
        println!("  identity   : {:?}", snap.identity.as_ref().map(|i| &i.name));
        println!("  mode       : {:?}", snap.noise_control);
        println!("  capability : {} - {}", cap.status.label(), cap.evidence);

        assert!(snap.identity.is_some(), "still a known, paired device");
        assert!(snap.noise_control.is_none(), "no mode is shown for an absent device");
        assert_eq!(cap.status, CapabilityStatus::Unknown);
        assert!(!cap.hardware_verified);

        let read = dev.noise_control().await;
        println!("  explicit read while disconnected   : {read:?}");
        assert!(matches!(read, Ok(None)));

        let change = dev.set_noise_control(NoiseControlMode::Quiet).await;
        println!("  change request while disconnected  : {change:?}");
        assert!(matches!(change, Err(DeviceError::NotConnected)));

        assert!(dev.control.lock().is_none(), "no control connection was attempted");
        assert!(dev.take_wire_log().is_empty(), "nothing was transmitted");
    }
}

#[cfg(test)]
mod battery_tests {
    use super::*;
    use control::link::BatteryReport;

    fn reported(percent: u8) -> BatteryReport {
        BatteryReport {
            percent,
            at: "2026-10-11T16:40:00Z".to_string(),
        }
    }

    /// Where the host system has a level, that is the one shown.
    #[test]
    fn the_host_level_comes_first_and_is_labelled_as_the_hosts() {
        let reading = battery_reading(Some(70), Some(&reported(65)), "now").unwrap();
        assert_eq!(reading.percent, 70);
        assert_eq!(reading.source, HOST_BATTERY);
        assert_eq!(reading.read_at, "now");
    }

    /// Where it has none, the headphones' own answer is shown — as theirs,
    /// and with the time they gave it, not the time it is being shown.
    #[test]
    fn the_headphones_level_stands_in_and_says_where_it_came_from() {
        let reading = battery_reading(None, Some(&reported(10)), "now").unwrap();
        assert_eq!(reading.percent, 10);
        assert_eq!(reading.source, BatterySource::VendorProtocol);
        assert_eq!(reading.read_at, "2026-10-11T16:40:00Z");
    }

    #[test]
    fn with_neither_there_is_no_level_rather_than_a_zero() {
        assert_eq!(battery_reading(None, None, "now"), None);
    }
}
