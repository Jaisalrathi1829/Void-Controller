//! The control link: one validated BMAP connection at a time, and what has
//! been learned over it.
//!
//! This owns the things that must outlive a single request:
//!
//!   * the open connection, reused between requests and reopened once if it
//!     turns out to have died;
//!   * the last mode the device itself reported, so that when a change cannot
//!     be verified the application still shows the last *confirmed* state;
//!   * which transitions have actually been confirmed by the hardware in this
//!     run — the evidence the capability engine needs before it will call
//!     noise control verified rather than merely ready.

use super::custom::{self, LevelChange, Slot};
use super::eq::{self, EqChange, EqLevels};
use super::endpoint::{open_validated, probe_current_mode, Attempt, Connector, Endpoint, OpenError};
use super::noise::{
    self, is_link_failure, preset_for_index, ModeChange, Preset, Target, VerifyPolicy,
};
use super::stream::{StreamTransport, Timing, WireEvent};
use super::unit::{self, ConnectionChange, MultipointChange, NameChange, PairedDevice};
use bmap::device::DeviceConfig;
use bmap::error::BmapError;
use bmap::BmapConnection;
use std::time::{Duration, Instant};

/// How long a failed open suppresses further automatic attempts. A connect to
/// an unresponsive channel can block for many seconds, and a status refresh
/// must not pay that price every time. An explicit user action ignores it.
pub const REOPEN_BACKOFF: Duration = Duration::from_secs(20);

/// A mode the device itself reported, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub index: u8,
    /// RFC3339 timestamp of the read.
    pub at: String,
}

/// A mode change the device confirmed by reporting the new mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionProof {
    pub before: u8,
    pub after: u8,
    pub at: String,
}

/// The mode slots the device listed, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotTable {
    pub slots: Vec<Slot>,
    pub at: String,
}

/// A custom mode's level change the device confirmed by reporting the new
/// level. Levels are on the device's own scale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelProof {
    pub slot: u8,
    pub name: String,
    pub before_raw: u8,
    pub after_raw: u8,
    pub at: String,
}

/// The equalizer as the device last reported it, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqReading {
    pub levels: EqLevels,
    pub at: String,
}

/// The battery level as the headphones themselves last reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryReport {
    pub percent: u8,
    /// Each part's level, named as the model's description names it, on a
    /// device that reports several: earbuds and their case. A part that
    /// reported no level (`FF`) is left out.
    pub parts: Vec<(&'static str, u8)>,
    pub at: String,
}

/// What the headphones called themselves, when they last said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameReading {
    pub name: String,
    pub at: String,
}

/// A rename the headphones confirmed by reporting the new name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameProof {
    pub before: String,
    pub after: String,
    pub at: String,
}

/// Whether multipoint was on, when the headphones last said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipointReading {
    pub enabled: bool,
    pub at: String,
}

/// A switch of multipoint the headphones confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipointProof {
    /// What it was switched to.
    pub enabled: bool,
    pub at: String,
}

/// A paired device the headphones confirmed connecting to or dropping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedProof {
    /// What it was changed to: connected, or not.
    pub connected: bool,
    pub at: String,
}

/// The devices the headphones were paired with, when they last said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedReading {
    pub devices: Vec<PairedDevice>,
    pub at: String,
}

/// How long a name, multipoint setting or list of paired devices that could
/// not be read is left before it is asked for again.
const UNIT_PROBLEM_RETRY: Duration = Duration::from_secs(60);

/// An equalizer change the device confirmed by reporting the new gains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqProof {
    pub before: EqLevels,
    pub after: EqLevels,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkErrorKind {
    /// The headphones could not be reached at all.
    Unreachable,
    /// Reachable, but no route answered like a BMAP endpoint.
    NoEndpoint,
    /// A recent open failed and automatic retries are being held off.
    BackingOff,
    /// The channel failed mid-conversation.
    ChannelFailed,
    /// The device answered with an error.
    Device,
    /// The device's replies contradict the documented framing, so writes are
    /// refused.
    UnexpectedFraming,
    /// The model or the operation is not supported.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkError {
    pub kind: LinkErrorKind,
    pub message: String,
}

impl LinkError {
    fn new(kind: LinkErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn from_bmap(e: BmapError) -> Self {
        let kind = match &e {
            BmapError::Unsupported(_) | BmapError::InvalidArg(_) => LinkErrorKind::Unsupported,
            BmapError::Device { .. } | BmapError::Auth(_) => LinkErrorKind::Device,
            _ => LinkErrorKind::ChannelFailed,
        };
        Self::new(kind, e.to_string())
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A read-only summary of the link, for building capability evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkStatus {
    /// The validated endpoint, while a session is open.
    pub endpoint: Option<Endpoint>,
    /// Firmware string from the most recent successful validation. Kept after
    /// the session closes: it is a fact about the device, not the socket.
    pub firmware: Option<String>,
    /// Why the last open failed, if it did and nothing has succeeded since.
    pub last_failure: Option<String>,
    /// Set when the device's mode reply did not match the documented framing.
    pub framing_problem: Option<String>,
    pub confirmed: Option<Confirmed>,
    pub to_quiet: Option<TransitionProof>,
    pub to_aware: Option<TransitionProof>,
    /// The mode slots as last listed by the device.
    pub slots: Option<SlotTable>,
    /// Set when the mode list did not match the documented layout. While it
    /// is set, nothing is written to a slot.
    pub slots_problem: Option<String>,
    /// The custom slot the device was most recently seen in.
    pub last_custom_slot: Option<u8>,
    /// A confirmed switch into a custom slot.
    pub to_custom: Option<TransitionProof>,
    /// The latest switch to any mode that the headphones confirmed. What
    /// noise control is verified by on a model without Quiet and Aware
    /// presets in its description.
    pub to_mode: Option<TransitionProof>,
    /// A confirmed change to a custom mode's level.
    pub level: Option<LevelProof>,
    /// The equalizer as last reported by the device.
    pub eq: Option<EqReading>,
    /// Set when the equalizer reply did not match the documented layout, or
    /// the device refused the read. While it is set, the equalizer is not
    /// written.
    pub eq_problem: Option<String>,
    /// A confirmed equalizer change.
    pub eq_change: Option<EqProof>,
    /// The battery level as last reported by the device over this link.
    /// Asked for only when the host system has no level of its own to give.
    pub battery: Option<BatteryReport>,
    /// What the headphones call themselves, as they last said.
    pub name: Option<NameReading>,
    /// Set when the name could not be read as observed. While it is set, the
    /// headphones are not renamed.
    pub name_problem: Option<String>,
    /// A confirmed rename.
    pub name_change: Option<NameProof>,
    /// Whether multipoint is on, as the headphones last said.
    pub multipoint: Option<MultipointReading>,
    /// Set when multipoint could not be read as observed, or the headphones
    /// have none. While it is set, it is not switched.
    pub multipoint_problem: Option<String>,
    /// A confirmed switch of multipoint.
    pub multipoint_change: Option<MultipointProof>,
    /// The paired devices, as the headphones last listed them.
    pub paired: Option<PairedReading>,
    /// Set when the list could not be read as observed.
    pub paired_problem: Option<String>,
    /// A confirmed connection to, or disconnection from, a paired device.
    pub paired_change: Option<PairedProof>,
}

struct Session<S: super::stream::ByteStream> {
    conn: BmapConnection<StreamTransport<S>>,
    /// A second handle to the same stream, kept for its wire log.
    transport: StreamTransport<S>,
}

/// Clock and sleep, injectable so tests need neither.
pub struct LinkTiming {
    pub transport: Timing,
    pub verify: VerifyPolicy,
    /// Read-back policy for a custom mode's level.
    pub level_verify: VerifyPolicy,
    /// How long a paired device is given to connect, and to disconnect.
    pub connect_verify: VerifyPolicy,
    pub disconnect_verify: VerifyPolicy,
    pub backoff: Duration,
    pub pause: fn(Duration),
}

impl LinkTiming {
    pub fn standard() -> Self {
        Self {
            transport: Timing::standard(),
            verify: VerifyPolicy::standard(),
            level_verify: VerifyPolicy::level(),
            connect_verify: unit::connect_policy(),
            disconnect_verify: unit::disconnect_policy(),
            backoff: REOPEN_BACKOFF,
            pause: std::thread::sleep,
        }
    }
}

pub struct BmapLink<C: Connector> {
    connector: C,
    config: DeviceConfig,
    timing: LinkTiming,
    now_text: fn() -> String,
    session: Option<Session<C::Stream>>,
    last_open_failure: Option<(Instant, String)>,
    /// When the mode list was last asked for, whatever came of it.
    slots_asked: Option<Instant>,
    /// When the name, multipoint and the paired devices were last asked for.
    unit_asked: Option<Instant>,
    /// When one of those that had a problem was last asked about again.
    unit_problems_asked: Option<Instant>,
    status: LinkStatus,
    attempts: Vec<Attempt>,
    wire_log: Vec<WireEvent>,
}

const WIRE_LOG_CAP: usize = 512;

impl<C: Connector> BmapLink<C> {
    pub fn new(connector: C, config: DeviceConfig, timing: LinkTiming, now_text: fn() -> String) -> Self {
        Self {
            connector,
            config,
            timing,
            now_text,
            session: None,
            last_open_failure: None,
            slots_asked: None,
            unit_asked: None,
            unit_problems_asked: None,
            status: LinkStatus::default(),
            attempts: Vec::new(),
            wire_log: Vec::new(),
        }
    }

    pub fn config(&self) -> &DeviceConfig {
        &self.config
    }

    pub fn status(&self) -> LinkStatus {
        self.status.clone()
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// How long ago the mode list was last asked for, if it has been since
    /// the headphones were last seen to go.
    ///
    /// Listing the modes is slow in a way nothing else here is: the reply is
    /// several frames with nothing to mark the last, so it is over only when
    /// the headphones have been silent for a while. Callers use this to
    /// decide whether the list they have will do.
    pub fn slots_age(&self) -> Option<Duration> {
        self.slots_asked.map(|asked| asked.elapsed())
    }

    /// Every route tried by the most recent open, in order.
    pub fn last_attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// Returns and clears the accumulated wire log.
    pub fn take_wire_log(&mut self) -> Vec<WireEvent> {
        self.collect_wire_log();
        std::mem::take(&mut self.wire_log)
    }

    fn collect_wire_log(&mut self) {
        if let Some(s) = &self.session {
            self.wire_log.extend(s.transport.take_wire_log());
        }
        if self.wire_log.len() > WIRE_LOG_CAP {
            let excess = self.wire_log.len() - WIRE_LOG_CAP;
            self.wire_log.drain(..excess);
        }
    }

    /// Closes the connection. What was learned is kept.
    pub fn close(&mut self) {
        self.collect_wire_log();
        self.session = None;
        self.status.endpoint = None;
    }

    /// The device has gone away: powered off, out of range, disconnected.
    ///
    /// The connection is closed and the last confirmed mode is forgotten — it
    /// described a session that is over, and showing it after the headphones
    /// come back would be showing a guess. The same goes for the mode list,
    /// which can be edited from a phone while this PC is not looking. The
    /// firmware string and the confirmed changes are facts about the device,
    /// and are kept. The backoff is cleared so a returning device is tried
    /// straight away.
    pub fn device_gone(&mut self) {
        self.close();
        self.status.confirmed = None;
        self.status.slots = None;
        self.status.slots_problem = None;
        self.slots_asked = None;
        self.status.eq = None;
        self.status.eq_problem = None;
        self.status.battery = None;
        // Their name, multipoint and what they are connected to can all be
        // changed from another device while this one is not looking.
        self.status.name = None;
        self.status.name_problem = None;
        self.status.multipoint = None;
        self.status.multipoint_problem = None;
        self.status.paired = None;
        self.status.paired_problem = None;
        self.unit_asked = None;
        self.unit_problems_asked = None;
        self.status.last_failure = None;
        self.last_open_failure = None;
    }

    fn record_confirmed(&mut self, index: u8) {
        self.status.confirmed = Some(Confirmed {
            index,
            at: (self.now_text)(),
        });
        if preset_for_index(&self.config, index).is_none() {
            self.status.last_custom_slot = Some(index);
        }
    }

    /// Opens and validates a session if there is not one already.
    ///
    /// `force` ignores the backoff; it is set for explicit user actions.
    fn ensure_open(&mut self, force: bool) -> Result<(), LinkError> {
        if self.session.is_some() {
            return Ok(());
        }

        if !force {
            if let Some((when, why)) = &self.last_open_failure {
                if when.elapsed() < self.timing.backoff {
                    return Err(LinkError::new(
                        LinkErrorKind::BackingOff,
                        format!("{why} (not retried automatically for a short while)"),
                    ));
                }
            }
        }

        let opened = open_validated(
            &mut self.connector,
            &self.config,
            self.timing.transport,
            self.timing.pause,
        );

        let (transport, endpoint, attempts) = match opened {
            Ok(ok) => ok,
            Err(err) => {
                let summary = err.summary();
                let OpenError {
                    attempts,
                    unreachable,
                } = err;
                self.attempts = attempts;
                self.last_open_failure = Some((Instant::now(), summary.clone()));
                self.status.last_failure = Some(summary.clone());
                return Err(LinkError::new(
                    if unreachable {
                        LinkErrorKind::Unreachable
                    } else {
                        LinkErrorKind::NoEndpoint
                    },
                    summary,
                ));
            }
        };

        self.attempts = attempts;
        self.last_open_failure = None;
        self.status.last_failure = None;
        self.status.firmware = Some(endpoint.firmware.clone());

        // Before this session is allowed to write anything, check that the
        // device's current-mode reply has exactly the documented shape.
        match probe_current_mode(&transport, &self.config) {
            Ok(index) => {
                self.status.framing_problem = None;
                self.record_confirmed(index);
            }
            Err(why) => {
                self.status.framing_problem = Some(why);
            }
        }

        self.status.endpoint = Some(endpoint);
        self.session = Some(Session {
            conn: BmapConnection::new(transport.clone(), self.config.clone()),
            transport,
        });
        self.collect_wire_log();
        Ok(())
    }

    /// Reads the current mode index from the device.
    ///
    /// If an already-open session turns out to be dead, it is dropped and one
    /// fresh connection is tried. A read is safe to repeat; a write is not,
    /// and is never retried this way.
    pub fn read_mode(&mut self, force: bool) -> Result<u8, LinkError> {
        let was_open = self.session.is_some();
        self.ensure_open(force)?;

        match self.read_mode_once() {
            Ok(index) => Ok(index),
            Err(e) if was_open && is_link_failure_or_timeout(&e) => {
                // The session predates this call and has failed underneath us:
                // reconnect once and ask again.
                self.close();
                self.ensure_open(true)?;
                self.read_mode_once().map_err(|e| self.fail(e))
            }
            Err(e) => Err(self.fail(e)),
        }
    }

    fn read_mode_once(&mut self) -> Result<u8, BmapError> {
        let session = self.session.as_ref().expect("session ensured by caller");
        let result = noise::read_mode(&session.conn);
        self.collect_wire_log();
        if let Ok(index) = result {
            self.record_confirmed(index);
        }
        result
    }

    /// Converts a failure into a `LinkError`, dropping the session if the
    /// channel itself is no longer usable.
    fn fail(&mut self, e: BmapError) -> LinkError {
        if is_link_failure_or_timeout(&e) {
            self.close();
        }
        LinkError::from_bmap(e)
    }

    /// Reads the mode slots from the device and remembers them.
    ///
    /// Read-only. A list that does not match the documented layout is
    /// remembered too, as the reason nothing will be written to a slot.
    pub fn read_slots(&mut self, force: bool) -> Result<Vec<Slot>, LinkError> {
        self.ensure_open(force)?;
        let session = self.session.as_ref().expect("session ensured above");
        let result = custom::read_slots(&session.conn);
        self.slots_asked = Some(Instant::now());
        self.collect_wire_log();

        match result {
            Ok(slots) => {
                self.status.slots = Some(SlotTable {
                    slots: slots.clone(),
                    at: (self.now_text)(),
                });
                self.status.slots_problem = None;
                Ok(slots)
            }
            Err(e) => {
                if matches!(e, BmapError::Device { .. } | BmapError::Unsupported(_)) {
                    self.status.slots = None;
                    self.status.slots_problem = Some(e.to_string());
                }
                Err(match e {
                    // Listing modes sends nothing that could be refused, so a
                    // device error here is about the shape of the answer.
                    BmapError::Device { .. } => LinkError::new(
                        LinkErrorKind::UnexpectedFraming,
                        format!(
                            "The headphones' mode list could not be read as documented. {e}"
                        ),
                    ),
                    e => self.fail(e),
                })
            }
        }
    }

    /// Reads the equalizer from the device and remembers it. Read-only.
    pub fn read_eq(&mut self, force: bool) -> Result<EqLevels, LinkError> {
        self.ensure_open(force)?;
        let session = self.session.as_ref().expect("session ensured above");
        let result = eq::read_eq(&session.conn);
        self.collect_wire_log();

        match result {
            Ok(levels) => {
                self.status.eq = Some(EqReading {
                    levels,
                    at: (self.now_text)(),
                });
                self.status.eq_problem = None;
                Ok(levels)
            }
            Err(e) => {
                let refused = matches!(
                    e,
                    BmapError::Device { .. } | BmapError::Auth(_) | BmapError::Unsupported(_)
                );
                if refused {
                    self.status.eq = None;
                    self.status.eq_problem = Some(e.to_string());
                }
                Err(match e {
                    // A read sends nothing that changes anything, so a device
                    // error here is about the answer, not a refused command.
                    BmapError::Device { .. } => LinkError::new(
                        LinkErrorKind::UnexpectedFraming,
                        format!("The headphones' equalizer could not be read as documented. {e}"),
                    ),
                    e => self.fail(e),
                })
            }
        }
    }

    /// Reads the battery level from the device and remembers it. Read-only.
    ///
    /// Measured on the test headphones on 2026-10-11: `GET [2.2]` answered
    /// `0A FF FF 00`, 10 percent, with Windows reporting 10 for the same unit
    /// at the same moment.
    pub fn read_battery(&mut self, force: bool) -> Result<u8, LinkError> {
        self.ensure_open(force)?;
        let session = self.session.as_ref().expect("session ensured above");
        let result = session.conn.battery_status();
        self.collect_wire_log();

        match result.map(|status| (status.aggregate, status.readings)) {
            Ok((percent, readings)) if percent <= 100 => {
                let parts = self
                    .config
                    .battery_components
                    .iter()
                    .filter_map(|(id, name)| {
                        readings
                            .iter()
                            .find(|r| r.component_id == *id && r.level <= 100)
                            .map(|r| (*name, r.level))
                    })
                    .collect();
                self.status.battery = Some(BatteryReport {
                    percent,
                    parts,
                    at: (self.now_text)(),
                });
                Ok(percent)
            }
            // A number that is not a percentage is not a level to show.
            Ok((percent, _)) => {
                self.status.battery = None;
                Err(LinkError::new(
                    LinkErrorKind::UnexpectedFraming,
                    format!("The headphones reported a battery level of {percent}, which is not a percentage."),
                ))
            }
            Err(e) => {
                self.status.battery = None;
                Err(match e {
                    BmapError::Device { .. } => LinkError::new(
                        LinkErrorKind::UnexpectedFraming,
                        format!("The headphones' battery level could not be read as documented. {e}"),
                    ),
                    e => self.fail(e),
                })
            }
        }
    }

    /// How long ago the name, multipoint and the paired devices were last
    /// asked for. `None` if they have not been since the device was last
    /// seen to go.
    pub fn unit_age(&self) -> Option<Duration> {
        self.unit_asked.map(|at| at.elapsed())
    }

    /// Reads what the headphones are called, whether multipoint is on, and
    /// which devices they are paired with and connected to, and remembers
    /// each. Read-only.
    ///
    /// Three separate questions (and one more for each paired device). One
    /// that is refused, not answered, or answered in a shape that has not
    /// been observed is remembered as that feature's problem and does not
    /// stop the others; the link is kept. Only the link itself failing ends
    /// the reading.
    ///
    /// A feature with a problem is not asked about again on every call:
    /// headphones that do not answer a question would otherwise hold the
    /// link for a reply timeout each time. It is asked again after
    /// [`UNIT_PROBLEM_RETRY`], when `force` is set, and when the headphones
    /// have been away and come back.
    pub fn read_unit(&mut self, force: bool) -> Result<(), LinkError> {
        self.ensure_open(force)?;
        self.unit_asked = Some(Instant::now());
        let at = (self.now_text)();

        let retry = force
            || self
                .unit_problems_asked
                .map_or(true, |asked| asked.elapsed() >= UNIT_PROBLEM_RETRY);
        let had_problem = self.status.name_problem.is_some()
            || self.status.multipoint_problem.is_some()
            || self.status.paired_problem.is_some();
        if retry && had_problem {
            self.unit_problems_asked = Some(Instant::now());
        }

        if self.status.name_problem.is_none() || retry {
            let session = self.session.as_ref().expect("session ensured above");
            match unit::read_name(&session.transport) {
                Ok(name) => {
                    self.status.name = Some(NameReading { name, at: at.clone() });
                    self.status.name_problem = None;
                }
                Err(e) if is_link_failure(&e) => return Err(self.fail(e)),
                Err(e) => {
                    self.status.name = None;
                    self.status.name_problem = Some(e.to_string());
                    self.unit_problems_asked.get_or_insert_with(Instant::now);
                }
            }
        }

        if self.status.multipoint_problem.is_none() || retry {
            let session = self.session.as_ref().expect("the link did not fail");
            match unit::read_multipoint(&session.transport) {
                Ok(enabled) => {
                    self.status.multipoint = Some(MultipointReading { enabled, at: at.clone() });
                    self.status.multipoint_problem = None;
                }
                Err(e) if is_link_failure(&e) => return Err(self.fail(e)),
                Err(e) => {
                    self.status.multipoint = None;
                    self.status.multipoint_problem = Some(e.to_string());
                    self.unit_problems_asked.get_or_insert_with(Instant::now);
                }
            }
        }

        if self.status.paired_problem.is_none() || retry {
            // What came before goes to the wire log; what the next exchange
            // puts there does not, because it is other devices' Bluetooth
            // addresses.
            self.collect_wire_log();
            let session = self.session.as_ref().expect("the link did not fail");
            let paired = unit::read_paired(&session.transport);
            session.transport.take_wire_log();
            match paired {
                Ok(devices) => {
                    self.status.paired = Some(PairedReading { devices, at });
                    self.status.paired_problem = None;
                }
                Err(e) if is_link_failure(&e) => return Err(self.fail(e)),
                Err(e) => {
                    self.status.paired = None;
                    self.status.paired_problem = Some(e.to_string());
                    self.unit_problems_asked.get_or_insert_with(Instant::now);
                }
            }
        }
        Ok(())
    }

    /// Renames the headphones and reports what they then called themselves.
    pub fn set_name(&mut self, requested: &str) -> Result<NameChange, LinkError> {
        // Proves the link is alive, reconnecting once if it is not.
        self.read_mode(true)?;

        let session = self.session.as_ref().expect("read_mode left a session open");
        let result = unit::set_name(&session.transport, requested, self.timing.verify);
        self.collect_wire_log();

        let change = match result {
            Ok(change) => change,
            // Device errors only surface here from the first read — a refused
            // write comes back as a rejected change. Nothing was sent.
            Err(e @ BmapError::Device { .. }) => {
                self.status.name = None;
                self.status.name_problem = Some(e.to_string());
                return Err(LinkError::new(
                    LinkErrorKind::UnexpectedFraming,
                    format!(
                        "The headphones' name could not be read as observed, so they were not \
                         renamed. {e}"
                    ),
                ));
            }
            Err(e) => return Err(self.fail(e)),
        };

        // Whatever they last called themselves is what is remembered.
        if let Some(after) = &change.after {
            self.status.name = Some(NameReading {
                name: after.clone(),
                at: (self.now_text)(),
            });
            self.status.name_problem = None;
        }
        if change.is_verified_change() {
            self.status.name_change = Some(NameProof {
                before: change.before.clone(),
                after: change.requested.clone(),
                at: (self.now_text)(),
            });
        }
        if change.sent && change.after.is_none() {
            self.close();
        }
        Ok(change)
    }

    /// Connects the headphones to one of their paired devices, or
    /// disconnects them from it, and reports what they then said about it.
    ///
    /// Nothing of this exchange goes to the wire log: every frame of it
    /// carries another device's address.
    pub fn set_paired_connection(
        &mut self,
        key: &str,
        connect: bool,
    ) -> Result<ConnectionChange, LinkError> {
        self.read_mode(true)?;
        self.collect_wire_log();

        let policy = if connect {
            self.timing.connect_verify
        } else {
            self.timing.disconnect_verify
        };
        let session = self.session.as_ref().expect("read_mode left a session open");
        let result = unit::set_paired_connection(&session.transport, key, connect, policy);
        session.transport.take_wire_log();

        let change = match result {
            Ok(change) => change,
            Err(e @ BmapError::Device { .. }) => {
                self.status.paired = None;
                self.status.paired_problem = Some(e.to_string());
                return Err(LinkError::new(
                    LinkErrorKind::UnexpectedFraming,
                    format!(
                        "The headphones' paired devices could not be read as observed, so \
                         nothing was changed. {e}"
                    ),
                ));
            }
            Err(e) => return Err(self.fail(e)),
        };

        if change.is_verified_change() {
            self.status.paired_change = Some(PairedProof {
                connected: change.requested,
                at: (self.now_text)(),
            });
        }
        if change.closed_after || (change.sent && change.after.is_none()) {
            // The link went with it. What they are connected to is not known
            // until they are reached again.
            self.status.paired = None;
            self.unit_asked = None;
            self.close();
        } else if change.sent {
            // What they are connected to has changed, or may have: read the
            // list again now rather than show the old one until it is due.
            let session = self.session.as_ref().expect("the link did not go");
            let paired = unit::read_paired(&session.transport);
            session.transport.take_wire_log();
            match paired {
                Ok(devices) => {
                    self.status.paired = Some(PairedReading {
                        devices,
                        at: (self.now_text)(),
                    });
                    self.status.paired_problem = None;
                }
                Err(e) if is_link_failure(&e) => {
                    self.status.paired = None;
                    self.fail(e);
                }
                Err(_) => {
                    self.status.paired = None;
                    self.unit_asked = None;
                }
            }
        }
        Ok(change)
    }

    /// Switches multipoint and reports what the headphones then said.
    ///
    /// With multipoint going off the headphones keep one connection. If it
    /// is not this one they close it, and the session here is closed with it;
    /// the change is still confirmed, by the state they answered with first.
    pub fn set_multipoint(&mut self, requested: bool) -> Result<MultipointChange, LinkError> {
        self.read_mode(true)?;

        let session = self.session.as_ref().expect("read_mode left a session open");
        let result = unit::set_multipoint(&session.transport, requested, self.timing.verify);
        self.collect_wire_log();

        let change = match result {
            Ok(change) => change,
            Err(e @ BmapError::Device { .. }) => {
                self.status.multipoint = None;
                self.status.multipoint_problem = Some(e.to_string());
                return Err(LinkError::new(
                    LinkErrorKind::UnexpectedFraming,
                    format!(
                        "Multipoint could not be read as observed, so it was not switched. {e}"
                    ),
                ));
            }
            Err(e) => return Err(self.fail(e)),
        };

        if let Some(enabled) = change.after {
            self.status.multipoint = Some(MultipointReading {
                enabled,
                at: (self.now_text)(),
            });
            self.status.multipoint_problem = None;
        }
        if change.is_verified_change() {
            self.status.multipoint_change = Some(MultipointProof {
                enabled: change.requested,
                at: (self.now_text)(),
            });
        }
        // What they are connected to has just changed, or may have.
        if change.sent {
            self.status.paired = None;
            self.unit_asked = None;
        }
        if change.closed_after || (change.sent && change.after.is_none()) {
            self.close();
        }
        Ok(change)
    }

    /// Sets the equalizer and reports what the device then said.
    pub fn set_eq(&mut self, requested: EqLevels) -> Result<EqChange, LinkError> {
        // Proves the link is alive, reconnecting once if it is not.
        self.read_mode(true)?;

        let session = self.session.as_ref().expect("read_mode left a session open");
        let result = eq::set_eq(&session.conn, requested, self.timing.verify);
        self.collect_wire_log();

        let change = match result {
            Ok(change) => change,
            // Device errors only surface here from the baseline read — a
            // refused write comes back as a rejected change. Nothing was sent.
            Err(e @ BmapError::Device { .. }) => {
                self.status.eq = None;
                self.status.eq_problem = Some(e.to_string());
                return Err(LinkError::new(
                    LinkErrorKind::UnexpectedFraming,
                    format!(
                        "The headphones' equalizer could not be read as documented, so nothing \
                         was changed. {e}"
                    ),
                ));
            }
            Err(e) => return Err(self.fail(e)),
        };

        // Whatever the device last reported is what is remembered.
        if let Some(after) = change.after {
            self.status.eq = Some(EqReading {
                levels: after,
                at: (self.now_text)(),
            });
        }
        if change.is_verified_change() {
            self.status.eq_change = Some(EqProof {
                before: change.before,
                after: change.requested,
                at: (self.now_text)(),
            });
        }
        if change.sent && change.after.is_none() {
            self.close();
        }
        Ok(change)
    }

    /// Refuses to write when the device's replies have not matched the
    /// documented framing.
    fn refuse_writes_on_bad_framing(&self) -> Result<(), LinkError> {
        match &self.status.framing_problem {
            Some(why) => Err(LinkError::new(
                LinkErrorKind::UnexpectedFraming,
                format!(
                    "The headphones' mode reply does not match the documented format, so no \
                     change was sent. {why}"
                ),
            )),
            None => Ok(()),
        }
    }

    /// Requests a preset and reports what the device then said.
    pub fn change_mode(&mut self, preset: Preset) -> Result<ModeChange, LinkError> {
        self.change_to(Target::Preset(preset))
    }

    /// Requests a mode — a preset or a custom slot — and reports what the
    /// device then said.
    pub fn change_to(&mut self, target: Target) -> Result<ModeChange, LinkError> {
        // A user asked for this, so the backoff does not apply. Reading first
        // also proves the link is alive before anything is written.
        self.read_mode(true)?;
        self.refuse_writes_on_bad_framing()?;

        let session = self.session.as_ref().expect("read_mode left a session open");
        let result =
            noise::change_to(&session.conn, &session.transport, &target, self.timing.verify);
        self.collect_wire_log();

        let change = match result {
            Ok(change) => change,
            Err(e) => return Err(self.fail(e)),
        };

        // Whatever the device last reported is the confirmed state, whether
        // or not it is the one that was asked for.
        if let Some(after) = change.after {
            self.record_confirmed(after);
        }

        if change.is_verified_transition() {
            let proof = TransitionProof {
                before: change.before,
                after: change.requested_index,
                at: (self.now_text)(),
            };
            self.status.to_mode = Some(proof.clone());
            match &target {
                Target::Preset(Preset::Quiet) => self.status.to_quiet = Some(proof),
                Target::Preset(Preset::Aware) => self.status.to_aware = Some(proof),
                Target::Slot { .. } => self.status.to_custom = Some(proof),
                // A mode asked for from the list is whichever it is: Quiet or
                // Aware where the description says so, a custom mode where
                // it is one of the editable slots, and otherwise only a mode.
                Target::Listed { index, .. } => {
                    match noise::preset_for_index(&self.config, *index) {
                        Some(Preset::Quiet) => self.status.to_quiet = Some(proof),
                        Some(Preset::Aware) => self.status.to_aware = Some(proof),
                        None if self.config.editable_slots.contains(index) => {
                            self.status.to_custom = Some(proof)
                        }
                        None => {}
                    }
                }
            }
        }

        // An unanswered read-back usually means the channel has gone; make
        // the next request start from a fresh connection.
        if change.sent && change.after.is_none() {
            self.close();
        }

        Ok(change)
    }

    /// Sets the level of the custom mode in `index` — which must be the mode
    /// the headphones are in — and reports what the device then said.
    /// `raw` is on the device's own scale.
    pub fn set_custom_level(&mut self, index: u8, raw: u8) -> Result<LevelChange, LinkError> {
        self.read_mode(true)?;
        self.refuse_writes_on_bad_framing()?;
        if let Some(why) = &self.status.slots_problem {
            return Err(LinkError::new(
                LinkErrorKind::UnexpectedFraming,
                format!(
                    "The headphones' mode list does not match the documented format, so no \
                     change was sent. {why}"
                ),
            ));
        }

        let session = self.session.as_ref().expect("read_mode left a session open");
        let result = custom::set_level(
            &session.conn,
            &session.transport,
            index,
            raw,
            self.timing.level_verify,
        );
        self.collect_wire_log();

        let change = match result {
            Ok(change) => change,
            // The only device errors that surface here come from reading the
            // baseline: a write the device refused is reported as a rejected
            // change, not as an error. So nothing was sent.
            Err(e @ BmapError::Device { .. }) => {
                return Err(LinkError::new(
                    LinkErrorKind::UnexpectedFraming,
                    format!(
                        "The headphones' mode list could not be read as documented, so nothing \
                         was changed. {e}"
                    ),
                ));
            }
            Err(e) => return Err(self.fail(e)),
        };

        // Keep the remembered list in step with what the device last said.
        if let Some(after) = &change.after {
            let at = (self.now_text)();
            if let Some(table) = &mut self.status.slots {
                match table.slots.iter_mut().find(|s| s.index == after.index) {
                    Some(slot) => *slot = after.clone(),
                    None => table.slots.push(after.clone()),
                }
                table.at = at;
            }
        }

        if change.is_verified_change() {
            self.status.level = Some(LevelProof {
                slot: change.slot,
                name: change.name.clone(),
                before_raw: change.before_raw,
                after_raw: change.requested_raw,
                at: (self.now_text)(),
            });
        }

        if change.sent && change.after.is_none() {
            self.close();
        }

        Ok(change)
    }
}

/// A timeout on an established session is treated like a dead channel: the
/// device answered this exact request moments ago, so silence now means the
/// link, not the protocol.
fn is_link_failure_or_timeout(e: &BmapError) -> bool {
    is_link_failure(e) || matches!(e, BmapError::Timeout(_))
}

#[cfg(test)]
mod tests {
    use super::super::endpoint::{ConnectFailure, Opened, Route};
    use super::super::stream::testing::{data, Chunk, ScriptedStream};
    use super::*;
    use bmap::devices;
    use std::collections::VecDeque;

    const FW: &[u8] = &[0x00, 0x05, 0x03, 0x05, b'1', b'.', b'0', b'.', b'6'];

    fn status(index: u8) -> Chunk {
        data(&[0x1F, 0x03, 0x03, 0x01, index])
    }
    fn processing() -> Chunk {
        data(&[0x1F, 0x03, 0x07, 0x00])
    }

    /// A stream scripted as: firmware reply, then one reaction per later send.
    fn session(reactions: Vec<Vec<Chunk>>) -> ScriptedStream {
        let mut s = ScriptedStream::new().on_send(vec![data(FW)]);
        for r in reactions {
            s = s.on_send(r);
        }
        s
    }

    /// Hands out one prepared stream per successful connect.
    struct QueueConnector {
        streams: VecDeque<Result<ScriptedStream, ConnectFailure>>,
        connects: usize,
    }

    impl QueueConnector {
        fn new(streams: Vec<Result<ScriptedStream, ConnectFailure>>) -> Self {
            Self {
                streams: streams.into(),
                connects: 0,
            }
        }
    }

    impl Connector for QueueConnector {
        type Stream = ScriptedStream;
        fn connect(&mut self, _route: Route) -> Result<Opened<ScriptedStream>, ConnectFailure> {
            self.connects += 1;
            match self.streams.pop_front() {
                Some(Ok(stream)) => Ok(Opened {
                    stream,
                    channel: Some(8),
                }),
                Some(Err(f)) => Err(f),
                None => Err(ConnectFailure {
                    message: "no more scripted streams".into(),
                    unreachable: true,
                }),
            }
        }
    }

    fn timing(backoff: Duration) -> LinkTiming {
        LinkTiming {
            transport: Timing::instant(),
            verify: VerifyPolicy::instant(3),
            level_verify: VerifyPolicy::instant(2),
            connect_verify: VerifyPolicy::instant(3),
            disconnect_verify: VerifyPolicy::instant(3),
            backoff,
            pause: |_| {},
        }
    }

    fn link(streams: Vec<Result<ScriptedStream, ConnectFailure>>) -> BmapLink<QueueConnector> {
        BmapLink::new(
            QueueConnector::new(streams),
            devices::qc_prince(),
            timing(Duration::ZERO),
            || "2026-10-10T00:00:00Z".to_string(),
        )
    }

    fn unreachable() -> ConnectFailure {
        ConnectFailure {
            message: "timed out".into(),
            unreachable: true,
        }
    }

    // --- opening ------------------------------------------------------------

    #[test]
    fn opening_validates_then_probes_the_mode_before_anything_else() {
        let stream = session(vec![vec![status(1)], vec![status(1)]]);
        let mut l = link(vec![Ok(stream.clone())]);
        assert_eq!(l.read_mode(false).unwrap(), 1);

        let sent = stream.sent();
        assert_eq!(sent[0], vec![0x00, 0x05, 0x01, 0x00], "firmware GET first");
        assert_eq!(sent[1], vec![0x1F, 0x03, 0x01, 0x00], "then the strict mode probe");
        // Every frame so far is a GET: nothing state-changing was sent.
        assert!(sent.iter().all(|f| f[2] == 0x01));

        let st = l.status();
        assert_eq!(st.firmware.as_deref(), Some("1.0.6"));
        assert_eq!(st.endpoint.unwrap().route, Route::SerialPortService);
        assert_eq!(st.confirmed.unwrap().index, 1);
        assert!(st.framing_problem.is_none());
    }

    #[test]
    fn an_open_session_is_reused() {
        let stream = session(vec![vec![status(1)], vec![status(1)], vec![status(0)]]);
        let mut l = link(vec![Ok(stream)]);
        l.read_mode(false).unwrap();
        assert_eq!(l.read_mode(false).unwrap(), 0);
        assert_eq!(l.connector.connects, 1, "one controlled connection");
    }

    // --- failures and backoff -----------------------------------------------

    #[test]
    fn unreachable_device_is_reported_as_such() {
        let mut l = link(vec![Err(unreachable())]);
        let err = l.read_mode(false).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::Unreachable);
        assert!(l.status().last_failure.is_some());
        assert!(!l.is_open());
    }

    /// After a failed open, a background refresh must not pay for another
    /// connect attempt straight away.
    #[test]
    fn automatic_reads_back_off_after_a_failed_open() {
        let mut l = BmapLink::new(
            QueueConnector::new(vec![Err(unreachable())]),
            devices::qc_prince(),
            timing(Duration::from_secs(3600)),
            String::new,
        );
        assert_eq!(l.read_mode(false).unwrap_err().kind, LinkErrorKind::Unreachable);
        let connects = l.connector.connects;
        assert_eq!(l.read_mode(false).unwrap_err().kind, LinkErrorKind::BackingOff);
        assert_eq!(l.connector.connects, connects, "no new connect while backing off");
    }

    /// An explicit user action is never held off by the backoff.
    #[test]
    fn a_forced_read_ignores_the_backoff() {
        let good = session(vec![vec![status(0)], vec![status(0)]]);
        let mut l = BmapLink::new(
            QueueConnector::new(vec![Err(unreachable()), Ok(good)]),
            devices::qc_prince(),
            timing(Duration::from_secs(3600)),
            String::new,
        );
        assert!(l.read_mode(false).is_err());
        assert_eq!(l.read_mode(true).unwrap(), 0);
        assert!(l.status().last_failure.is_none(), "failure cleared on success");
    }

    // --- disconnection and reconnection -------------------------------------

    /// The headphones dropped the channel between two requests. The next read
    /// reconnects once and succeeds.
    #[test]
    fn a_dead_session_is_reopened_once_for_a_read() {
        let first = session(vec![vec![status(1)], vec![status(1)], vec![Chunk::Close]]);
        let second = session(vec![vec![status(1)], vec![status(0)]]);
        let mut l = link(vec![Ok(first), Ok(second)]);

        assert_eq!(l.read_mode(false).unwrap(), 1);
        // Second read hits the closed channel, reconnects, and reads again.
        assert_eq!(l.read_mode(false).unwrap(), 0);
        assert_eq!(l.connector.connects, 2);
    }

    #[test]
    fn a_session_that_stops_answering_is_reopened_once_for_a_read() {
        let first = session(vec![vec![status(1)], vec![status(1)], vec![Chunk::Silence]]);
        let second = session(vec![vec![status(1)], vec![status(1)]]);
        let mut l = link(vec![Ok(first), Ok(second)]);
        l.read_mode(false).unwrap();
        assert_eq!(l.read_mode(false).unwrap(), 1);
        assert_eq!(l.connector.connects, 2);
    }

    /// Reconnection is tried once, not in a loop.
    #[test]
    fn reconnection_is_not_retried_forever() {
        let first = session(vec![vec![status(1)], vec![status(1)], vec![Chunk::Close]]);
        let mut l = link(vec![Ok(first), Err(unreachable())]);
        l.read_mode(false).unwrap();
        assert!(l.read_mode(false).is_err());
        assert_eq!(l.connector.connects, 2);
        assert!(!l.is_open());
    }

    #[test]
    fn close_keeps_what_was_learned() {
        let stream = session(vec![vec![status(1)], vec![status(1)]]);
        let mut l = link(vec![Ok(stream)]);
        l.read_mode(false).unwrap();
        l.close();
        let st = l.status();
        assert!(st.endpoint.is_none());
        assert_eq!(st.firmware.as_deref(), Some("1.0.6"));
        assert_eq!(st.confirmed.unwrap().index, 1);
    }

    /// Headphones switched off and on again: the mode from before must not be
    /// presented as current, but what was proven about the device still holds.
    #[test]
    fn a_device_that_went_away_keeps_its_proofs_but_not_its_last_mode() {
        let mut l = BmapLink::new(
            QueueConnector::new(vec![Ok(switching_session(1, 0)), Err(unreachable())]),
            devices::qc_prince(),
            timing(Duration::from_secs(3600)),
            || "2026-10-10T00:00:00Z".to_string(),
        );
        l.change_mode(Preset::Quiet).unwrap();
        assert_eq!(l.status().confirmed.unwrap().index, 0);

        l.device_gone();
        let st = l.status();
        assert!(!l.is_open());
        assert!(st.confirmed.is_none(), "a mode from the ended session is not current");
        assert!(st.to_quiet.is_some(), "the confirmed transition is still a fact");
        assert_eq!(st.firmware.as_deref(), Some("1.0.6"));
    }

    /// A device that comes back is tried at once, not held off by a backoff
    /// earned while it was away.
    #[test]
    fn a_returning_device_is_not_held_off_by_an_old_backoff() {
        let good = session(vec![vec![status(1)], vec![status(1)]]);
        let mut l = BmapLink::new(
            QueueConnector::new(vec![Err(unreachable()), Ok(good)]),
            devices::qc_prince(),
            timing(Duration::from_secs(3600)),
            String::new,
        );
        assert!(l.read_mode(false).is_err());
        l.device_gone();
        assert_eq!(l.read_mode(false).unwrap(), 1);
    }

    // --- changing mode ------------------------------------------------------

    /// Scripted replies for: open (probe), pre-read, baseline, START, read-back.
    fn switching_session(start: u8, end: u8) -> ScriptedStream {
        session(vec![
            vec![status(start)],                      // strict probe at open
            vec![status(start)],                      // link pre-read
            vec![status(start)],                      // baseline inside change_mode
            vec![processing(), data(&[0x1F, 0x03, 0x06, 0x01, end])], // START
            vec![status(end)],                        // read-back
        ])
    }

    #[test]
    fn a_confirmed_switch_records_the_transition() {
        let mut l = link(vec![Ok(switching_session(1, 0))]);
        let change = l.change_mode(Preset::Quiet).unwrap();
        assert!(change.is_verified_transition());

        let st = l.status();
        let proof = st.to_quiet.expect("transition to Quiet recorded");
        assert_eq!((proof.before, proof.after), (1, 0));
        assert!(st.to_aware.is_none(), "only the direction actually seen");
        assert_eq!(st.confirmed.unwrap().index, 0);
    }

    #[test]
    fn both_directions_are_tracked_independently() {
        let stream = session(vec![
            vec![status(1)],
            vec![status(1)],
            vec![status(1)],
            vec![processing()],
            vec![status(0)],
            // second change, on the same session
            vec![status(0)],
            vec![status(0)],
            vec![processing()],
            vec![status(1)],
        ]);
        let mut l = link(vec![Ok(stream)]);
        l.change_mode(Preset::Quiet).unwrap();
        l.change_mode(Preset::Aware).unwrap();
        let st = l.status();
        assert!(st.to_quiet.is_some());
        assert!(st.to_aware.is_some());
        assert_eq!(l.connector.connects, 1);
    }

    /// Asking for the mode the device is already in is not evidence that
    /// switching works.
    #[test]
    fn already_active_records_no_transition() {
        let stream = session(vec![vec![status(0)], vec![status(0)], vec![status(0)]]);
        let mut l = link(vec![Ok(stream.clone())]);
        let change = l.change_mode(Preset::Quiet).unwrap();
        assert!(!change.sent);
        assert!(l.status().to_quiet.is_none());
        assert!(
            stream.sent().iter().all(|f| f[2] == 0x01),
            "no START may be sent when already in the requested mode"
        );
    }

    /// Acknowledged but not applied: no transition is recorded, and the last
    /// confirmed state is what the device actually said.
    #[test]
    fn an_unverified_switch_records_nothing_and_keeps_the_confirmed_state() {
        let stream = session(vec![
            vec![status(1)],
            vec![status(1)],
            vec![status(1)],
            vec![processing()],
            vec![status(1)],
            vec![status(1)],
            vec![status(1)],
        ]);
        let mut l = link(vec![Ok(stream)]);
        let change = l.change_mode(Preset::Quiet).unwrap();
        assert!(!change.is_verified_transition());
        let st = l.status();
        assert!(st.to_quiet.is_none());
        assert_eq!(st.confirmed.unwrap().index, 1, "still the device's own report");
    }

    /// If the device's mode reply is not the documented shape, the session
    /// stays read-only: no START is ever transmitted.
    #[test]
    fn unexpected_framing_blocks_writes() {
        let odd_status = data(&[0x1F, 0x03, 0x03, 0x02, 0x01, 0x00]);
        let stream = session(vec![vec![odd_status.clone()], vec![odd_status]]);
        let mut l = link(vec![Ok(stream.clone())]);

        let err = l.change_mode(Preset::Quiet).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::UnexpectedFraming);
        assert!(l.status().framing_problem.is_some());
        assert!(
            stream.sent().iter().all(|f| f[2] == 0x01),
            "a START was transmitted despite unexpected framing"
        );
    }

    /// The write itself is never repeated on a fresh connection.
    #[test]
    fn a_switch_is_never_resent_after_a_channel_failure() {
        let first = session(vec![
            vec![status(1)],
            vec![status(1)],
            vec![status(1)],
            vec![Chunk::Close], // channel dies on the START
        ]);
        let second = session(vec![vec![status(1)], vec![status(1)]]);
        let mut l = link(vec![Ok(first.clone()), Ok(second.clone())]);

        let err = l.change_mode(Preset::Quiet).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::ChannelFailed);
        assert!(!l.is_open());
        // The request did go out before the channel died, so the error must
        // not read as though nothing was sent.
        assert!(
            err.message.contains("not known whether the headphones received it"),
            "{}",
            err.message
        );

        let starts = |s: &ScriptedStream| s.sent().iter().filter(|f| f[2] == 0x05).count();
        assert_eq!(starts(&first), 1);
        assert_eq!(starts(&second), 0, "the START must not be replayed");
    }

    // --- custom modes -------------------------------------------------------

    fn mode_list(raw: u8) -> Chunk {
        data(&custom::testing::mode_list("Focus", raw))
    }
    /// The STATUS a device answers a mode-configuration write with.
    fn write_ack() -> Chunk {
        data(&[0x1F, 0x06, 0x03, 0x01, 0x02])
    }
    fn is_slot_write(frame: &[u8]) -> bool {
        frame[0] == 0x1F && frame[1] == 0x06 && frame[2] & 0x0F == 0x02
    }

    #[test]
    fn the_mode_list_is_read_and_remembered() {
        let stream = session(vec![vec![status(0)], vec![mode_list(6)]]);
        let mut l = link(vec![Ok(stream.clone())]);
        let slots = l.read_slots(false).unwrap();
        assert_eq!(slots.len(), 4);

        let table = l.status().slots.expect("remembered");
        assert_eq!(table.slots[2].name, "Focus");
        assert_eq!(table.slots[2].raw_level, 6);
        assert!(l.status().slots_problem.is_none());
        // Listing modes is a read: nothing that changes state went out.
        assert!(!stream.sent().iter().any(|f| is_slot_write(f)));
    }

    #[test]
    fn a_confirmed_switch_into_a_custom_slot_is_recorded() {
        let stream = session(vec![
            vec![status(0)],     // probe at open
            vec![status(0)],     // link pre-read
            vec![status(0)],     // baseline
            vec![processing()],  // START
            vec![status(2)],     // read-back
        ]);
        let mut l = link(vec![Ok(stream)]);
        let change = l
            .change_to(Target::Slot {
                index: 2,
                name: "Focus".into(),
            })
            .unwrap();
        assert!(change.is_verified_transition());

        let st = l.status();
        let proof = st.to_custom.expect("switch into the custom slot recorded");
        assert_eq!((proof.before, proof.after), (0, 2));
        assert_eq!(st.last_custom_slot, Some(2));
        assert!(st.to_quiet.is_none() && st.to_aware.is_none());
    }

    /// Scripted replies for a level change on an open link whose device is in
    /// slot 2: open (probe), pre-read, mode check, baseline list, write,
    /// read-back list.
    fn level_session(before: u8, after: u8) -> ScriptedStream {
        session(vec![
            vec![status(2)],
            vec![status(2)],
            vec![status(2)],
            vec![mode_list(before)],
            vec![write_ack()],
            vec![mode_list(after)],
        ])
    }

    #[test]
    fn a_confirmed_level_change_is_recorded() {
        let stream = level_session(6, 3);
        let mut l = link(vec![Ok(stream.clone())]);
        let change = l.set_custom_level(2, 3).unwrap();
        assert!(change.is_verified_change());

        let proof = l.status().level.expect("level change recorded");
        assert_eq!((proof.slot, proof.before_raw, proof.after_raw), (2, 6, 3));
        assert_eq!(proof.name, "Focus");
        assert_eq!(stream.sent().iter().filter(|f| is_slot_write(f)).count(), 1);
    }

    /// The remembered list follows what the device said after the write, so
    /// the next snapshot does not show the old level.
    #[test]
    fn the_remembered_list_follows_a_level_change() {
        let stream = session(vec![
            vec![status(2)],     // probe at open
            vec![mode_list(6)],  // read_slots
            vec![status(2)],     // pre-read
            vec![status(2)],     // mode check
            vec![mode_list(6)],  // baseline
            vec![write_ack()],
            vec![mode_list(3)],  // read-back
        ]);
        let mut l = link(vec![Ok(stream)]);
        l.read_slots(false).unwrap();
        l.set_custom_level(2, 3).unwrap();
        assert_eq!(l.status().slots.unwrap().slots[2].raw_level, 3);
    }

    #[test]
    fn an_unverified_level_change_records_nothing() {
        let stream = session(vec![
            vec![status(2)],
            vec![status(2)],
            vec![status(2)],
            vec![mode_list(6)],
            vec![write_ack()],
            vec![mode_list(6)],
            vec![mode_list(6)],
        ]);
        let mut l = link(vec![Ok(stream)]);
        let change = l.set_custom_level(2, 3).unwrap();
        assert!(!change.is_verified_change());
        assert!(l.status().level.is_none());
    }

    /// A mode list in a layout nobody documented blocks every slot write.
    #[test]
    fn an_undocumented_mode_list_blocks_level_writes() {
        let mut odd = vec![0x1F, 0x06, 0x03, 48];
        odd.extend(vec![0u8; 48]);
        let stream = session(vec![
            vec![status(2)],
            vec![data(&odd)], // read_slots
            vec![status(2)],  // pre-read of the level change
        ]);
        let mut l = link(vec![Ok(stream.clone())]);

        assert!(l.read_slots(false).is_err());
        assert!(l.status().slots_problem.is_some());
        assert!(l.status().slots.is_none());

        let err = l.set_custom_level(2, 3).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::UnexpectedFraming);
        assert!(!stream.sent().iter().any(|f| is_slot_write(f)));
    }

    /// A level write is never repeated on a fresh connection.
    #[test]
    fn a_level_write_is_never_resent_after_a_channel_failure() {
        let first = session(vec![
            vec![status(2)],
            vec![status(2)],
            vec![status(2)],
            vec![mode_list(6)],
            vec![Chunk::Close], // channel dies on the write
        ]);
        let second = session(vec![vec![status(2)], vec![status(2)]]);
        let mut l = link(vec![Ok(first.clone()), Ok(second.clone())]);

        let err = l.set_custom_level(2, 3).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::ChannelFailed);
        assert!(err.message.contains("not known whether"), "{}", err.message);

        let writes = |s: &ScriptedStream| s.sent().iter().filter(|f| is_slot_write(f)).count();
        assert_eq!(writes(&first), 1);
        assert_eq!(writes(&second), 0, "the write must not be replayed");
    }

    /// The mode list can be edited from a phone while the headphones are
    /// away, so it is not carried across a disconnection.
    #[test]
    fn the_age_of_the_mode_list_runs_from_when_it_was_asked_for() {
        let mut link = link(vec![Ok(session(vec![vec![status(0)], vec![mode_list(6)]]))]);
        assert_eq!(link.slots_age(), None, "never asked");
        link.read_slots(true).unwrap();
        assert!(link.slots_age().expect("asked") < Duration::from_secs(5));
        link.device_gone();
        assert_eq!(link.slots_age(), None, "the list went with the headphones");
    }

    #[test]
    fn a_device_that_went_away_takes_its_mode_list_with_it() {
        let stream = session(vec![vec![status(2)], vec![mode_list(6)]]);
        let mut l = link(vec![Ok(stream)]);
        l.read_slots(false).unwrap();
        l.device_gone();
        assert!(l.status().slots.is_none());
        assert_eq!(l.status().last_custom_slot, Some(2), "kept as a hint for next time");
    }

    // --- equalizer ----------------------------------------------------------

    fn eq_status(bass: i8, mid: i8, treble: i8) -> Chunk {
        data(&eq::testing::eq_status(bass, mid, treble))
    }
    fn eq_ack() -> Chunk {
        data(&[0x01, 0x07, 0x03, 4, 0xF6, 10, 0, 0])
    }
    fn is_eq_write(frame: &[u8]) -> bool {
        frame[0] == 0x01 && frame[1] == 0x07 && frame[2] & 0x0F == 0x02
    }

    /// The battery reply recorded from the test headphones.
    fn battery_status(percent: u8) -> Chunk {
        data(&[0x02, 0x02, 0x03, 0x04, percent, 0xFF, 0xFF, 0x00])
    }

    #[test]
    fn the_battery_level_is_read_and_remembered() {
        let stream = session(vec![vec![status(0)], vec![battery_status(10)]]);
        let mut l = link(vec![Ok(stream.clone())]);
        assert_eq!(l.read_battery(false).unwrap(), 10);
        let report = l.status().battery.unwrap();
        assert_eq!(report.percent, 10);
        assert!(report.parts.is_empty(), "one battery, no parts");
        // One question, and nothing that writes.
        let sent = stream.sent();
        assert_eq!(sent.last().unwrap(), &vec![0x02, 0x02, 0x01, 0x00]);
    }

    /// Earbuds report each bud and the case, four bytes a part: level, two
    /// bytes, the part's id. A part with no level (`FF`) is left out.
    #[test]
    fn earbuds_report_each_part_by_name() {
        let reply = data(&[
            0x02, 0x02, 0x03, 0x10, //
            80, 0xFF, 0xFF, 1, // right
            70, 0xFF, 0xFF, 2, // left
            0xFF, 0xFF, 0xFF, 3, // the case, not reported
            70, 0xFF, 0xFF, 4, // the pair
        ]);
        let stream = session(vec![vec![status(0)], vec![reply]]);
        let mut l = BmapLink::new(
            QueueConnector::new(vec![Ok(stream)]),
            devices::qc_ultra2_earbuds(),
            timing(Duration::ZERO),
            || "2026-10-16T00:00:00Z".to_string(),
        );
        assert_eq!(l.read_battery(false).unwrap(), 70);
        let report = l.status().battery.unwrap();
        assert_eq!(report.parts, vec![("Right", 80), ("Left", 70)]);
    }

    #[test]
    fn a_level_that_is_not_a_percentage_is_not_remembered() {
        let stream = session(vec![vec![status(0)], vec![battery_status(255)]]);
        let mut l = link(vec![Ok(stream)]);
        let err = l.read_battery(false).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::UnexpectedFraming);
        assert_eq!(l.status().battery, None);
    }

    #[test]
    fn the_battery_level_is_forgotten_when_the_headphones_go() {
        let stream = session(vec![vec![status(0)], vec![battery_status(55)]]);
        let mut l = link(vec![Ok(stream)]);
        l.read_battery(false).unwrap();
        l.device_gone();
        assert_eq!(l.status().battery, None);
    }

    #[test]
    fn the_equalizer_is_read_and_remembered() {
        let stream = session(vec![vec![status(0)], vec![eq_status(6, 5, -1)]]);
        let mut l = link(vec![Ok(stream.clone())]);
        let levels = l.read_eq(false).unwrap();
        assert_eq!((levels.bass, levels.mid, levels.treble), (6, 5, -1));
        assert_eq!(l.status().eq.unwrap().levels, levels);
        assert!(!stream.sent().iter().any(|f| is_eq_write(f)));
    }

    #[test]
    fn a_confirmed_equalizer_change_is_recorded() {
        let stream = session(vec![
            vec![status(0)],          // probe at open
            vec![status(0)],          // liveness read
            vec![eq_status(0, 0, 0)], // baseline
            vec![eq_ack()],
            vec![eq_ack()],
            vec![eq_ack()],
            vec![eq_status(4, -2, 3)], // read-back
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        let want = EqLevels {
            bass: 4,
            mid: -2,
            treble: 3,
        };
        let change = l.set_eq(want).unwrap();
        assert!(change.is_verified_change());

        let st = l.status();
        assert_eq!(st.eq.unwrap().levels, want);
        let proof = st.eq_change.expect("change recorded");
        assert_eq!(proof.after, want);
        assert_eq!(stream.sent().iter().filter(|f| is_eq_write(f)).count(), 3);
    }

    #[test]
    fn an_unverified_equalizer_change_records_no_proof_but_the_real_state() {
        let mut reactions = vec![
            vec![status(0)],
            vec![status(0)],
            vec![eq_status(0, 0, 0)],
            vec![eq_ack()],
            vec![eq_ack()],
            vec![eq_ack()],
        ];
        reactions.extend((0..3).map(|_| vec![eq_status(0, 0, 0)]));
        let mut l = link(vec![Ok(session(reactions))]);
        let change = l
            .set_eq(EqLevels {
                bass: 4,
                mid: 0,
                treble: 0,
            })
            .unwrap();
        assert!(!change.is_verified_change());
        let st = l.status();
        assert!(st.eq_change.is_none());
        assert_eq!(st.eq.unwrap().levels.bass, 0, "what the device said, not what was asked");
    }

    /// A device that refuses the equalizer read is remembered as such, and
    /// nothing is written to it.
    #[test]
    fn a_refused_equalizer_read_blocks_writes() {
        let refuse = data(&[0x01, 0x07, 0x04, 0x01, 0x03]);
        let stream = session(vec![
            vec![status(0)],
            vec![refuse.clone()], // read_eq
            vec![status(0)],      // liveness read of set_eq
            vec![refuse],         // its baseline
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        assert!(l.read_eq(false).is_err());
        assert!(l.status().eq_problem.is_some());

        let err = l
            .set_eq(EqLevels {
                bass: 1,
                mid: 1,
                treble: 1,
            })
            .unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::UnexpectedFraming);
        assert!(!stream.sent().iter().any(|f| is_eq_write(f)));
    }

    // --- the name, multipoint and the paired devices --------------------------

    const PHONE: [u8; 6] = [0x34, 0x11, 0x22, 0x33, 0x44, 0xF6];
    const LAPTOP: [u8; 6] = [0x9C, 0x55, 0x66, 0x77, 0x88, 0x49];

    fn name_status(name: &str) -> Chunk {
        data(&unit::testing::name_status(name))
    }
    fn multipoint_status(on: bool) -> Chunk {
        data(&unit::testing::multipoint_status(on))
    }
    fn paired_list(addresses: &[[u8; 6]]) -> Chunk {
        data(&unit::testing::paired_list(addresses))
    }
    fn paired_info(address: [u8; 6], status: u8, name: &str) -> Chunk {
        data(&unit::testing::paired_info(address, status, name))
    }
    fn is_unit_write(frame: &[u8]) -> bool {
        matches!((frame[0], frame[1]), (1, 2) | (1, 10) | (4, _)) && frame[2] & 0x0F != 0x01
    }

    #[test]
    fn the_name_multipoint_and_paired_devices_are_read_and_remembered() {
        let stream = session(vec![
            vec![status(0)], // probe at open
            vec![name_status("Aurora")],
            vec![multipoint_status(true)],
            vec![paired_list(&[PHONE, LAPTOP])],
            vec![paired_info(PHONE, 0x01, "Phone")],
            vec![paired_info(LAPTOP, 0x03, "Laptop")],
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        assert!(l.unit_age().is_none(), "not asked for yet");
        l.read_unit(false).unwrap();

        let st = l.status();
        assert_eq!(st.name.unwrap().name, "Aurora");
        assert!(st.multipoint.unwrap().enabled);
        let devices = st.paired.unwrap().devices;
        assert_eq!(devices.len(), 2);
        assert_eq!(
            (devices[0].name.as_str(), devices[0].connected, devices[0].this_device),
            ("Phone", true, false)
        );
        assert_eq!(
            (devices[1].name.as_str(), devices[1].connected, devices[1].this_device),
            ("Laptop", true, true)
        );
        assert!(l.unit_age().is_some());
        assert!(!stream.sent().iter().any(|f| is_unit_write(f)), "nothing was set");
    }

    /// The paired devices come with their Bluetooth addresses. Those are
    /// asked with, and kept out of the log of what crossed the link.
    #[test]
    fn other_devices_addresses_do_not_reach_the_wire_log() {
        let stream = session(vec![
            vec![status(0)],
            vec![name_status("Aurora")],
            vec![multipoint_status(true)],
            vec![paired_list(&[PHONE, LAPTOP])],
            vec![paired_info(PHONE, 0x01, "Phone")],
            vec![paired_info(LAPTOP, 0x03, "Laptop")],
        ]);
        let mut l = link(vec![Ok(stream)]);
        l.read_unit(false).unwrap();
        let log = l.take_wire_log();
        assert!(
            log.iter().any(|e| e.bytes == [0x01, 0x02, 0x01, 0x00]),
            "the name read is logged"
        );
        for event in &log {
            for address in [PHONE, LAPTOP] {
                assert!(
                    !event.bytes.windows(6).any(|w| w == address),
                    "an address in the log: {}",
                    event.render()
                );
            }
        }
    }

    /// Headphones with no multipoint say so; the name and the list are still
    /// read, the link is kept, and they are not asked again straight away.
    #[test]
    fn one_question_refused_does_not_stop_the_others_or_cost_the_link() {
        let refuse = data(&[0x01, 0x0A, 0x04, 0x01, 0x02]);
        let stream = session(vec![
            vec![status(0)],
            vec![name_status("Aurora")],
            vec![refuse],
            vec![paired_list(&[])],
            // The second reading: the name and the list, and no multipoint.
            vec![name_status("Aurora")],
            vec![paired_list(&[])],
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        l.read_unit(false).unwrap();
        let st = l.status();
        assert!(st.multipoint.is_none());
        assert!(st.multipoint_problem.is_some());
        assert_eq!(st.name.unwrap().name, "Aurora");
        assert_eq!(st.paired.unwrap().devices.len(), 0);
        assert!(l.is_open());

        l.read_unit(false).unwrap();
        let asked = stream.sent().iter().filter(|f| (f[0], f[1]) == (1, 10)).count();
        assert_eq!(asked, 1, "not asked again so soon");
        assert!(l.status().multipoint_problem.is_some());
    }

    #[test]
    fn a_question_left_unanswered_is_a_problem_with_that_question_not_with_the_link() {
        let stream = session(vec![
            vec![status(0)],
            vec![], // the name: silence
            vec![multipoint_status(false)],
            vec![paired_list(&[])],
        ]);
        let mut l = link(vec![Ok(stream)]);
        l.read_unit(false).unwrap();
        let st = l.status();
        assert!(st.name.is_none() && st.name_problem.is_some());
        assert!(!st.multipoint.unwrap().enabled);
        assert!(l.is_open(), "the link was not dropped over it");
    }

    #[test]
    fn headphones_that_went_away_are_asked_everything_again() {
        let first = session(vec![
            vec![status(0)],
            vec![name_status("Aurora")],
            vec![multipoint_status(true)],
            vec![paired_list(&[])],
        ]);
        let mut l = link(vec![Ok(first)]);
        l.read_unit(false).unwrap();
        l.device_gone();
        let st = l.status();
        assert!(st.name.is_none() && st.multipoint.is_none() && st.paired.is_none());
        assert!(l.unit_age().is_none());
    }

    #[test]
    fn a_confirmed_rename_is_recorded() {
        let stream = session(vec![
            vec![status(0)],             // probe at open
            vec![status(0)],             // liveness read
            vec![name_status("Aurora")], // what they are called now
            vec![name_status("Nova")],   // answer to the write
            vec![name_status("Nova")],   // read-back
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        let change = l.set_name("Nova").unwrap();
        assert!(change.is_verified_change());

        let st = l.status();
        assert_eq!(st.name.unwrap().name, "Nova");
        let proof = st.name_change.expect("rename recorded");
        assert_eq!((proof.before.as_str(), proof.after.as_str()), ("Aurora", "Nova"));
        assert_eq!(stream.sent().iter().filter(|f| is_unit_write(f)).count(), 1);
    }

    #[test]
    fn an_unconfirmed_rename_records_no_proof_but_the_real_name() {
        let mut reactions = vec![
            vec![status(0)],
            vec![status(0)],
            vec![name_status("Aurora")],
            vec![name_status("Aurora")], // answered, and nothing changed
        ];
        reactions.extend((0..3).map(|_| vec![name_status("Aurora")]));
        let mut l = link(vec![Ok(session(reactions))]);
        let change = l.set_name("Nova").unwrap();
        assert!(!change.is_verified_change());
        let st = l.status();
        assert!(st.name_change.is_none());
        assert_eq!(st.name.unwrap().name, "Aurora", "what they said, not what was asked");
    }

    #[test]
    fn a_name_that_cannot_be_read_first_is_not_written() {
        let odd = data(&[0x01, 0x02, 0x03, 0x02, 0x01, b'A']);
        let stream = session(vec![vec![status(0)], vec![status(0)], vec![odd]]);
        let mut l = link(vec![Ok(stream.clone())]);
        let err = l.set_name("Nova").unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::UnexpectedFraming);
        assert!(l.status().name_problem.is_some());
        assert!(!stream.sent().iter().any(|f| is_unit_write(f)));
    }

    #[test]
    fn a_confirmed_switch_of_multipoint_is_recorded() {
        let stream = session(vec![
            vec![status(0)],
            vec![status(0)],
            vec![multipoint_status(false)],
            vec![multipoint_status(true)], // answer to the write
            vec![multipoint_status(true)], // read-back
        ]);
        let mut l = link(vec![Ok(stream)]);
        let change = l.set_multipoint(true).unwrap();
        assert!(change.is_verified_change());
        let st = l.status();
        assert!(st.multipoint.unwrap().enabled);
        assert!(st.multipoint_change.expect("switch recorded").enabled);
        assert!(l.is_open());
    }

    /// As on the test headphones: asked from the computer, they answered
    /// "off" and closed the computer's connection.
    #[test]
    fn multipoint_going_off_may_close_this_link_and_is_still_recorded() {
        let stream = session(vec![
            vec![status(0)],
            vec![status(0)],
            vec![multipoint_status(true)],
            vec![multipoint_status(false)], // answer to the write
            vec![Chunk::Close],             // and then they were gone
        ]);
        let mut l = link(vec![Ok(stream)]);
        let change = l.set_multipoint(false).unwrap();
        assert!(change.is_verified_change());
        assert!(change.closed_after);

        let st = l.status();
        assert!(!st.multipoint.unwrap().enabled);
        assert!(!st.multipoint_change.expect("switch recorded").enabled);
        assert!(st.paired.is_none(), "what they are connected to is no longer known");
        assert!(!l.is_open(), "the session here went with it");
    }

    fn paired_ack(func: u8) -> Chunk {
        data(&[0x04, func, 0x07, 0x00])
    }
    fn key_of(devices: &[PairedDevice], name: &str) -> String {
        devices.iter().find(|d| d.name == name).expect("listed").key.clone()
    }

    /// Reads the list once to learn the key the laptop goes by, as the
    /// interface would have, then uses it.
    #[test]
    fn a_confirmed_disconnection_is_recorded_and_the_list_read_again() {
        let stream = session(vec![
            vec![status(0)], // probe at open
            // read_unit
            vec![name_status("Aurora")],
            vec![multipoint_status(true)],
            vec![paired_list(&[PHONE, LAPTOP])],
            vec![paired_info(PHONE, 0x03, "Phone")],
            vec![paired_info(LAPTOP, 0x01, "Laptop")],
            // set_paired_connection
            vec![status(0)], // liveness read
            vec![paired_list(&[PHONE, LAPTOP])],
            vec![paired_info(LAPTOP, 0x01, "Laptop")], // before
            vec![paired_ack(2)],
            vec![paired_info(LAPTOP, 0x00, "Laptop")], // read-back
            // the list, read again
            vec![paired_list(&[PHONE, LAPTOP])],
            vec![paired_info(PHONE, 0x03, "Phone")],
            vec![paired_info(LAPTOP, 0x00, "Laptop")],
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        l.read_unit(false).unwrap();
        let key = key_of(&l.status().paired.unwrap().devices, "Laptop");
        l.take_wire_log();

        let change = l.set_paired_connection(&key, false).unwrap();
        assert!(change.is_verified_change());
        assert_eq!(change.name, "Laptop");

        let st = l.status();
        assert!(!st.paired_change.expect("recorded").connected);
        let devices = st.paired.expect("read again").devices;
        assert!(!devices.iter().find(|d| d.name == "Laptop").unwrap().connected);
        assert!(l.is_open());

        // One request, to disconnect, and none to connect or remove.
        let requests: Vec<_> = stream
            .sent()
            .into_iter()
            .filter(|f| f[0] == 4 && !matches!(f[1], 4 | 5))
            .collect();
        assert_eq!(requests.len(), 1);
        assert_eq!((requests[0][1], requests[0][2] & 0x0F), (2, 0x05));

        // And none of it in the log: every frame of it holds an address.
        for event in l.take_wire_log() {
            assert_ne!(event.bytes.first(), Some(&0x04), "{}", event.render());
        }
    }

    #[test]
    fn disconnecting_this_device_closes_the_link_and_is_recorded() {
        let stream = session(vec![
            vec![status(0)],
            vec![status(0)],
            vec![paired_list(&[PHONE, LAPTOP])],
            vec![paired_info(LAPTOP, 0x03, "Laptop")], // this device
            vec![paired_ack(2)],
            vec![Chunk::Close],
        ]);
        let mut l = link(vec![Ok(stream)]);
        let key = unit::testing::key_of(&LAPTOP);
        let change = l.set_paired_connection(&key, false).unwrap();
        assert!(change.is_verified_change());
        assert!(change.closed_after);
        assert!(!l.is_open());
        assert!(l.status().paired.is_none(), "what they are connected to is no longer known");
    }

    #[test]
    fn a_device_that_is_not_on_the_list_is_not_asked_for() {
        let stream = session(vec![
            vec![status(0)],
            vec![status(0)],
            vec![paired_list(&[PHONE])],
        ]);
        let mut l = link(vec![Ok(stream.clone())]);
        let err = l
            .set_paired_connection(&unit::testing::key_of(&LAPTOP), false)
            .unwrap_err();
        assert!(err.message.contains("no longer"), "{}", err.message);
        assert!(!stream.sent().iter().any(|f| f[0] == 4 && matches!(f[1], 1..=3)));
        assert!(l.status().paired_change.is_none());
    }

    #[test]
    fn multipoint_that_cannot_be_read_first_is_not_switched() {
        let odd = data(&[0x01, 0x0A, 0x03, 0x01, 0x03]);
        let stream = session(vec![vec![status(0)], vec![status(0)], vec![odd]]);
        let mut l = link(vec![Ok(stream.clone())]);
        let err = l.set_multipoint(false).unwrap_err();
        assert_eq!(err.kind, LinkErrorKind::UnexpectedFraming);
        assert!(l.status().multipoint_problem.is_some());
        assert!(!stream.sent().iter().any(|f| is_unit_write(f)));
    }

    #[test]
    fn wire_log_survives_across_sessions() {
        let first = session(vec![vec![status(1)], vec![status(1)], vec![Chunk::Close]]);
        let second = session(vec![vec![status(1)], vec![status(1)]]);
        let mut l = link(vec![Ok(first), Ok(second)]);
        l.read_mode(false).unwrap();
        l.read_mode(false).unwrap();
        let log = l.take_wire_log();
        let firmware_gets = log
            .iter()
            .filter(|e| e.bytes == [0x00, 0x05, 0x01, 0x00])
            .count();
        assert_eq!(firmware_gets, 2, "both sessions' validation appears in the log");
    }
}
