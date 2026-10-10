//! Custom noise cancellation: the user-configurable mode slots.
//!
//! Besides the fixed Quiet and Aware presets, the headphones hold a small
//! number of editable modes, each with a name and a noise-cancellation level.
//! This module reads them and adjusts the level of the one the headphones are
//! currently in.
//!
//! The mode configuration format and its builder come from upstream. What is
//! added here is the same discipline as for mode switching — read, write,
//! read back — plus one rule specific to editing something the user saved:
//!
//! **A write may differ from what the device holds in exactly one byte.**
//! A mode configuration carries a name, a voice-prompt id and other settings
//! alongside the level, and they all travel in the same frame. Before a write
//! goes out, the frame is compared field by field against the configuration
//! the device just reported. If anything but the level would change — a name
//! that does not survive re-encoding, a setting this build does not model —
//! nothing is sent.
//!
//! Modes are never created, renamed or deleted here.

use super::noise::{describe_index, is_link_failure, Ack, Verdict, VerifyPolicy};
use bmap::device::DeviceConfig;
use bmap::error::{BmapError, BmapResult};
use bmap::protocol::{bmap_packet, BmapResponse, Operator};
use bmap::{BmapConnection, Transport};
use std::ops::Range;

/// The device's level scale runs 0..=10.
pub const MAX_RAW_LEVEL: u8 = 10;

/// Converts between the device's scale and the one shown to people.
///
/// The device counts *down*: raw 0 is the most cancellation and raw 10 lets
/// the most ambient sound through. A slider labelled "noise cancellation"
/// has to run the other way, so strength = 10 − raw. The function is its own
/// inverse.
pub fn flip_level(level: u8) -> u8 {
    MAX_RAW_LEVEL - level.min(MAX_RAW_LEVEL)
}

/// Where a mode configuration's fields sit: in the STATUS a device sends, and
/// in the SETGET payload it accepts.
///
/// Upstream's parser and builder encode these offsets but do not export them,
/// and the guard on writes needs them. They are transcribed from upstream's
/// notes for the model and were checked against the test unit before any
/// write was implemented.
struct Layout {
    status_len: usize,
    setget_len: usize,
    status_name: Range<usize>,
    setget_name: Range<usize>,
    status_level: usize,
    setget_level: usize,
    status_auto: usize,
    setget_auto: usize,
    status_spatial: usize,
    setget_spatial: usize,
    status_wind: usize,
    setget_wind: usize,
}

/// QuietComfort Headphones ("prince"): 47-byte STATUS, 39-byte SETGET.
const PRINCE: Layout = Layout {
    status_len: 47,
    setget_len: 39,
    status_name: 6..38,
    setget_name: 3..35,
    status_level: 42,
    setget_level: 35,
    status_auto: 43,
    setget_auto: 36,
    status_spatial: 44,
    setget_spatial: 37,
    status_wind: 46,
    setget_wind: 38,
};

/// The layout for a model, if it is one whose layout has been checked here.
fn layout_for(config: &DeviceConfig) -> Option<&'static Layout> {
    match config.info.codename {
        "prince" => Some(&PRINCE),
        _ => None,
    }
}

fn unsupported_model(config: &DeviceConfig) -> BmapError {
    BmapError::Unsupported(format!(
        "custom modes have not been checked on {}",
        config.info.name
    ))
}

/// One mode slot, as the headphones report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub index: u8,
    pub name: String,
    /// The device's own scale: 0 = most cancellation, 10 = most ambient.
    pub raw_level: u8,
    pub wind_block: bool,
    pub editable: bool,
    pub spatial: u8,
    pub prompt: (u8, u8),
    /// The STATUS payload this was parsed from, kept so a write can be
    /// checked against exactly what the device said.
    pub raw: Vec<u8>,
}

impl Slot {
    /// Whether this is a custom mode that can be selected and adjusted: one
    /// of the model's editable slots, flagged editable by the device, and
    /// actually set up. An empty or "None" name is how a free slot looks.
    pub fn is_usable(&self, config: &DeviceConfig) -> bool {
        let name = self.name.trim();
        self.editable
            && config.editable_slots.contains(&self.index)
            && !name.is_empty()
            && !name.eq_ignore_ascii_case("none")
    }

    /// Noise-cancellation strength as shown to people: 10 = most.
    pub fn strength(&self) -> u8 {
        flip_level(self.raw_level)
    }
}

/// The custom modes among a set of slots, lowest index first.
pub fn usable_slots<'a>(config: &DeviceConfig, slots: &'a [Slot]) -> Vec<&'a Slot> {
    let mut usable: Vec<&Slot> = slots.iter().filter(|s| s.is_usable(config)).collect();
    usable.sort_by_key(|s| s.index);
    usable
}

/// Reads every mode slot from the device.
///
/// Read-only: `START` on the mode-list function asks the device to send its
/// configurations and changes nothing. Each one must be exactly the documented
/// length — a different length is a layout nobody has described, and reading
/// fields out of it by offset would be guessing.
pub fn read_slots<T: Transport>(conn: &BmapConnection<T>) -> BmapResult<Vec<Slot>> {
    let config = conn.config();
    let layout = layout_for(config).ok_or_else(|| unsupported_model(config))?;
    let (Some(list), Some(mode_config), Some(parser)) = (
        config.get_all_modes,
        config.mode_config,
        config.parse_mode_config,
    ) else {
        return Err(unsupported_model(config));
    };

    let replies = conn.start_drain(list, &[])?;

    let mut slots = Vec::new();
    for reply in &replies {
        let from_list = (reply.fblock, reply.func) == (list.0, list.1);
        let from_config = (reply.fblock, reply.func) == (mode_config.0, mode_config.1);

        if (from_list || from_config) && reply.op == Operator::Error {
            return Err(error_from(reply));
        }
        if !from_config || reply.op != Operator::Status {
            continue;
        }
        if reply.payload.len() != layout.status_len {
            return Err(BmapError::Device {
                message: format!(
                    "a mode configuration carried {} bytes where {} are documented",
                    reply.payload.len(),
                    layout.status_len
                ),
                code: 0,
            });
        }
        let parsed = parser(&reply.payload).ok_or_else(|| BmapError::Device {
            message: "a mode configuration could not be parsed".into(),
            code: 0,
        })?;
        // The layout table above and upstream's parser must agree on where
        // the level lives. If an update to either ever moved it, the guard on
        // writes would be comparing the wrong bytes — so stop here instead.
        if reply.payload[layout.status_level] != parsed.cnc_level {
            return Err(BmapError::Device {
                message: "the mode-configuration layout does not match the parser".into(),
                code: 0,
            });
        }
        slots.push(Slot {
            index: parsed.mode_idx,
            name: parsed.name,
            raw_level: parsed.cnc_level,
            wind_block: parsed.wind_block,
            editable: parsed.editable,
            spatial: parsed.spatial,
            prompt: (parsed.prompt_b1, parsed.prompt_b2),
            raw: reply.payload.clone(),
        });
    }

    if slots.is_empty() {
        return Err(BmapError::Device {
            message: "the headphones did not list any mode configurations".into(),
            code: 0,
        });
    }
    Ok(slots)
}

/// Same mapping upstream applies to an ERROR frame.
fn error_from(reply: &BmapResponse) -> BmapError {
    match reply.payload.first() {
        Some(5) => BmapError::Auth(reply.fmt()),
        code => BmapError::Device {
            message: reply.fmt(),
            code: code.copied().unwrap_or(0),
        },
    }
}

/// The full record of one attempt to change a custom mode's level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelChange {
    pub slot: u8,
    pub name: String,
    pub requested_raw: u8,
    /// Level the device reported before anything was sent.
    pub before_raw: u8,
    /// Whether a write was transmitted at all.
    pub sent: bool,
    pub ack: Option<Ack>,
    /// The slot as most recently read back, if it could be.
    pub after: Option<Slot>,
    pub verdict: Verdict,
}

impl LevelChange {
    /// True only for a change the device itself confirmed.
    pub fn is_verified_change(&self) -> bool {
        self.verdict == Verdict::Verified && self.before_raw != self.requested_raw
    }
}

/// Checks that `payload` would change nothing on the device but the level.
fn check_minimal_edit(
    layout: &Layout,
    slot: &Slot,
    payload: &[u8],
    requested_raw: u8,
) -> Result<(), String> {
    let held = &slot.raw;
    if held.len() != layout.status_len || payload.len() != layout.setget_len {
        return Err("the configuration is not the documented size".into());
    }
    if payload[0] != held[0] || payload[1..3] != held[1..3] {
        return Err("the mode index or voice prompt would change".into());
    }
    if payload[layout.setget_name.clone()] != held[layout.status_name.clone()] {
        return Err(format!(
            "the name {:?} cannot be written back byte for byte",
            slot.name
        ));
    }
    if payload[layout.setget_auto] != held[layout.status_auto] {
        return Err(
            "the mode uses automatic noise cancellation, which would be switched off".into(),
        );
    }
    if payload[layout.setget_spatial] != held[layout.status_spatial] {
        return Err("the spatial-audio setting would change".into());
    }
    if payload[layout.setget_wind] != held[layout.status_wind] {
        return Err("the wind-block setting would change".into());
    }
    if payload[layout.setget_level] != requested_raw {
        return Err("the level was not encoded where it is documented".into());
    }
    Ok(())
}

/// What else changed between two readings of a slot, besides its level.
fn collateral_changes(before: &Slot, after: &Slot) -> Vec<&'static str> {
    let mut changed = Vec::new();
    if before.name != after.name {
        changed.push("name");
    }
    if before.wind_block != after.wind_block {
        changed.push("wind block");
    }
    if before.spatial != after.spatial {
        changed.push("spatial audio");
    }
    if before.prompt != after.prompt {
        changed.push("voice prompt");
    }
    changed
}

/// Sets the level of the custom mode the headphones are currently in, and
/// confirms it by reading the mode back.
///
/// Only the active mode is edited, so the change is one the user can hear,
/// and only if it is an editable, configured slot. Returns `Err` when no
/// meaningful report can be made — the baseline could not be read, or a guard
/// refused, so nothing was sent — or when the channel failed outright. Every
/// case in which a write went out produces a [`LevelChange`].
pub fn set_level<T: Transport>(
    conn: &BmapConnection<T>,
    index: u8,
    requested_raw: u8,
    policy: VerifyPolicy,
) -> BmapResult<LevelChange> {
    if requested_raw > MAX_RAW_LEVEL {
        return Err(BmapError::InvalidArg(format!(
            "level must be 0-{MAX_RAW_LEVEL}, got {requested_raw}"
        )));
    }
    let config = conn.config();
    let layout = layout_for(config).ok_or_else(|| unsupported_model(config))?;
    let (Some(mode_config), Some(builder)) = (config.mode_config, config.build_mode_config) else {
        return Err(unsupported_model(config));
    };

    // The headphones must still be in the mode being edited.
    let current = conn.mode_idx()?;
    if current != index {
        return Err(BmapError::InvalidArg(format!(
            "the headphones are in {} now, so the custom mode's level was left alone",
            describe_index(config, current)
        )));
    }

    // Baseline, read from the device.
    let before = read_slots(conn)?;
    let slot = before
        .iter()
        .find(|s| s.index == index)
        .cloned()
        .ok_or_else(|| BmapError::Device {
            message: format!("the headphones did not list mode slot {index}"),
            code: 0,
        })?;
    if !slot.is_usable(config) {
        return Err(BmapError::Unsupported(format!(
            "mode slot {index} is not an editable custom mode"
        )));
    }
    if slot.raw_level > MAX_RAW_LEVEL {
        return Err(BmapError::Unsupported(format!(
            "the headphones report level {} for {:?}, outside the documented 0-{MAX_RAW_LEVEL}",
            slot.raw_level, slot.name
        )));
    }

    let record = |sent, ack, after: Option<Slot>, verdict| LevelChange {
        slot: index,
        name: slot.name.clone(),
        requested_raw,
        before_raw: slot.raw_level,
        sent,
        ack,
        after,
        verdict,
    };

    if slot.raw_level == requested_raw {
        return Ok(record(false, None, Some(slot.clone()), Verdict::AlreadyActive));
    }

    // Upstream's builder, with every field but the level taken from what the
    // device just reported.
    let payload = builder(
        index,
        &slot.name,
        requested_raw,
        slot.spatial,
        slot.wind_block,
        false,
        slot.prompt.0,
        slot.prompt.1,
    );
    check_minimal_edit(layout, &slot, &payload, requested_raw).map_err(|why| {
        BmapError::Unsupported(format!(
            "the level of {:?} was not changed, because writing it would alter more than the \
             level: {why}",
            slot.name
        ))
    })?;

    let reread = |conn: &BmapConnection<T>| -> BmapResult<Slot> {
        read_slots(conn)?
            .into_iter()
            .find(|s| s.index == index)
            .ok_or_else(|| BmapError::Device {
                message: format!("the headphones did not list mode slot {index}"),
                code: 0,
            })
    };

    let packet = bmap_packet(mode_config.0, mode_config.1, Operator::SetGet, &payload);
    let ack = match conn.send_raw(&packet) {
        Ok(replies) => {
            let mut from_config = replies
                .iter()
                .filter(|r| (r.fblock, r.func) == (mode_config.0, mode_config.1));
            if let Some(refusal) = from_config.clone().find(|r| r.op == Operator::Error) {
                // The device said no. Read back so the caller still learns
                // what the mode holds.
                let reason = error_from(refusal).to_string();
                return Ok(record(
                    true,
                    None,
                    reread(conn).ok(),
                    Verdict::Rejected { reason },
                ));
            }
            if from_config.any(|r| r.op == Operator::Status) {
                Ack::Acknowledged
            } else {
                Ack::Missing("no STATUS reply to the write".into())
            }
        }
        // Transmitted, but nothing came back in time. The read-back decides.
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the level of {:?} was being written, so it is not \
                 known whether the headphones received it ({e})",
                slot.name
            )));
        }
        Err(e) => return Err(e),
    };

    // Read back, a bounded number of times.
    let mut after: Option<Slot> = None;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match reread(conn) {
            Ok(now) => {
                let done = now.raw_level == requested_raw;
                after = Some(now);
                last_error = None;
                if done {
                    break;
                }
            }
            Err(e) => {
                let dead = is_link_failure(&e);
                last_error = Some(e.to_string());
                if dead {
                    break;
                }
            }
        }
    }

    let acked = match &ack {
        Ack::Acknowledged => "The device acknowledged the change".to_string(),
        Ack::Missing(why) => format!("The device did not acknowledge the change ({why})"),
    };

    let verdict = match (&after, &last_error) {
        (Some(now), _) if now.raw_level == requested_raw => {
            let also = collateral_changes(&slot, now);
            if also.is_empty() {
                Verdict::Verified
            } else {
                // The level is right, but this module's one promise — that
                // nothing else is touched — did not hold. Say so.
                Verdict::Unverified {
                    reason: format!(
                        "The headphones report the new level, but other settings of {:?} changed \
                         as well: {}.",
                        slot.name,
                        also.join(", ")
                    ),
                }
            }
        }
        (Some(now), _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but {:?} still reports level {} after {} read-back(s).",
                slot.name,
                now.strength(),
                policy.attempts.max(1)
            ),
        },
        (None, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but the level could not be read back: {err}"),
        },
        (None, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };

    Ok(record(true, Some(ack), after, verdict))
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A 47-byte mode configuration STATUS payload in the prince layout.
    pub fn status_payload(index: u8, name: &str, raw_level: u8, editable: bool) -> Vec<u8> {
        let configured = editable && !name.is_empty();
        let mut p = vec![0u8; 47];
        p[0] = index;
        p[2] = if editable { 0x0D } else { index + 1 };
        p[3] = editable as u8;
        p[4] = configured as u8;
        p[5] = 1;
        p[6..6 + name.len()].copy_from_slice(name.as_bytes());
        p[41] = if editable { 0x09 } else { 0x00 };
        p[42] = raw_level;
        p
    }

    /// The frame a device sends for one mode configuration.
    pub fn status_frame(index: u8, name: &str, raw_level: u8, editable: bool) -> Vec<u8> {
        let payload = status_payload(index, name, raw_level, editable);
        let mut f = vec![0x1F, 0x06, 0x03, payload.len() as u8];
        f.extend(payload);
        f
    }

    /// The burst a device answers the mode-list request with, as captured
    /// from the test unit: bracketing frames around one STATUS per slot.
    pub fn mode_list(custom_name: &str, custom_raw: u8) -> Vec<u8> {
        let mut burst = vec![0x1F, 0x01, 0x07, 0x00]; // [31.1] PROCESSING
        burst.extend([0x1F, 0x06, 0x07, 0x00]); // [31.6] PROCESSING
        burst.extend(status_frame(0, "Quiet", 0, false));
        burst.extend(status_frame(1, "Aware", 10, false));
        burst.extend(status_frame(2, custom_name, custom_raw, true));
        burst.extend(status_frame(3, "", 0, true));
        burst.extend([0x1F, 0x06, 0x06, 0x00]); // [31.6] RESULT
        burst.extend([0x1F, 0x01, 0x06, 0x00]); // [31.1] RESULT
        burst
    }

    /// A slot as `read_slots` would produce it.
    pub fn slot(index: u8, name: &str, raw_level: u8, editable: bool) -> Slot {
        Slot {
            index,
            name: name.to_string(),
            raw_level,
            wind_block: false,
            editable,
            spatial: 0,
            prompt: (0, if editable { 0x0D } else { index + 1 }),
            raw: status_payload(index, name, raw_level, editable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use bmap::devices;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Answers each request with the next scripted reply, and keeps what was
    /// sent reachable after the connection has taken ownership of it.
    struct Script {
        replies: RefCell<VecDeque<BmapResult<Vec<u8>>>>,
        sent: Rc<RefCell<Vec<Vec<u8>>>>,
    }

    impl Transport for Script {
        fn send_recv(&self, packet: &[u8]) -> BmapResult<Vec<u8>> {
            self.sent.borrow_mut().push(packet.to_vec());
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(BmapError::Timeout("script exhausted".into())))
        }
        fn send_recv_drain(&self, packet: &[u8]) -> BmapResult<Vec<u8>> {
            self.send_recv(packet)
        }
    }

    const LIST_MODES: [u8; 4] = [0x1F, 0x01, 0x05, 0x00];

    /// Runs `f` against a scripted connection for `config` and returns its
    /// result together with every frame that was transmitted.
    fn run_on<R>(
        config: DeviceConfig,
        replies: Vec<BmapResult<Vec<u8>>>,
        f: impl FnOnce(&BmapConnection<Script>) -> R,
    ) -> (R, Vec<Vec<u8>>) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let transport = Script {
            replies: RefCell::new(replies.into()),
            sent: sent.clone(),
        };
        let conn = BmapConnection::new(transport, config);
        let result = f(&conn);
        let frames = sent.borrow().clone();
        (result, frames)
    }

    fn run<R>(
        replies: Vec<BmapResult<Vec<u8>>>,
        f: impl FnOnce(&BmapConnection<Script>) -> R,
    ) -> (R, Vec<Vec<u8>>) {
        run_on(devices::qc_prince(), replies, f)
    }

    fn mode(index: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x03, 0x03, 0x01, index])
    }
    fn list(name: &str, raw: u8) -> BmapResult<Vec<u8>> {
        Ok(mode_list(name, raw))
    }
    /// The STATUS a device answers a mode-configuration write with.
    fn write_ack() -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x06, 0x03, 0x01, 0x02])
    }
    fn write_error(code: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x06, 0x04, 0x01, code])
    }
    fn timeout() -> BmapResult<Vec<u8>> {
        Err(BmapError::Timeout("No reply".into()))
    }
    fn is_write(frame: &[u8]) -> bool {
        frame[0] == 0x1F && frame[1] == 0x06 && frame[2] & 0x0F == 0x02
    }
    fn policy() -> VerifyPolicy {
        VerifyPolicy::instant(3)
    }

    // --- the level scale ----------------------------------------------------

    #[test]
    fn the_shown_scale_is_the_devices_scale_upside_down() {
        assert_eq!(flip_level(0), 10, "raw 0 is the most cancellation");
        assert_eq!(flip_level(10), 0, "raw 10 is the most ambient");
        for level in 0..=10 {
            assert_eq!(flip_level(flip_level(level)), level);
        }
    }

    // --- reading slots ------------------------------------------------------

    #[test]
    fn reads_every_slot_out_of_the_captured_burst() {
        let (slots, frames) = run(vec![list("Focus", 6)], |c| read_slots(c).unwrap());
        assert_eq!(frames, vec![LIST_MODES.to_vec()], "one read-only request");

        let names: Vec<_> = slots.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Quiet", "Aware", "Focus", ""]);
        let focus = &slots[2];
        assert_eq!((focus.index, focus.raw_level, focus.editable), (2, 6, true));
        assert_eq!(focus.strength(), 4);
        assert_eq!(focus.raw.len(), 47);
    }

    #[test]
    fn only_a_named_editable_slot_is_a_usable_custom_mode() {
        let config = devices::qc_prince();
        let (slots, _) = run(vec![list("Focus", 6)], |c| read_slots(c).unwrap());
        let usable = usable_slots(&config, &slots);
        assert_eq!(usable.len(), 1);
        assert_eq!(usable[0].name, "Focus");

        // The presets are not editable; slot 3 is editable but empty.
        assert!(!slots[0].is_usable(&config));
        assert!(!slots[1].is_usable(&config));
        assert!(!slots[3].is_usable(&config));
        // "None" is how a deleted profile's slot is left.
        assert!(!slot(2, "None", 0, true).is_usable(&config));
        // Editable by flag, but not one of the model's editable slots.
        assert!(!slot(0, "Odd", 0, true).is_usable(&config));
    }

    /// A configuration of any other length is a layout nobody has described.
    /// Its fields are not read by offset.
    #[test]
    fn a_configuration_of_undocumented_length_is_refused() {
        let mut burst = mode_list("Focus", 6);
        // Append a 48-byte configuration, as a different model would send.
        let mut odd = vec![0x1F, 0x06, 0x03, 48];
        odd.extend(vec![0u8; 48]);
        burst.extend(odd);

        let (result, _) = run(vec![Ok(burst)], |c| read_slots(c));
        let err = result.unwrap_err();
        assert!(matches!(err, BmapError::Device { .. }), "{err:?}");
        assert!(err.to_string().contains("48 bytes where 47"), "{err}");
    }

    #[test]
    fn an_error_reply_to_the_mode_list_is_reported() {
        let (result, _) = run(vec![Ok(vec![0x1F, 0x01, 0x04, 0x01, 0x04])], |c| read_slots(c));
        assert!(matches!(result.unwrap_err(), BmapError::Device { code: 4, .. }));
    }

    #[test]
    fn a_reply_with_no_configurations_is_not_an_empty_success() {
        let (result, _) = run(vec![Ok(vec![0x1F, 0x01, 0x06, 0x00])], |c| read_slots(c));
        assert!(result.is_err());
    }

    /// Models whose layout has not been checked here get nothing read by
    /// offset, whatever upstream's parser would accept.
    #[test]
    fn an_unchecked_model_is_unsupported() {
        let (result, frames) = run_on(devices::qc_ultra2(), vec![list("Focus", 6)], |c| {
            read_slots(c)
        });
        assert!(matches!(result.unwrap_err(), BmapError::Unsupported(_)));
        assert!(frames.is_empty(), "nothing is even asked of an unchecked model");
    }

    // --- changing the level -------------------------------------------------

    /// The whole happy path: mode check, baseline, one write, read-back.
    #[test]
    fn a_confirmed_level_change() {
        let (change, frames) = run(
            vec![mode(2), list("Focus", 6), write_ack(), list("Focus", 3)],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_change());
        assert_eq!((change.before_raw, change.requested_raw), (6, 3));
        assert_eq!(change.after.as_ref().unwrap().raw_level, 3);
        assert_eq!(change.name, "Focus");

        assert_eq!(frames.iter().filter(|f| is_write(f)).count(), 1);
    }

    /// The write is upstream's 39-byte payload, and differs from what the
    /// device reported in the level byte and nothing else.
    #[test]
    fn the_write_changes_the_level_byte_and_nothing_else() {
        let (_, frames) = run(
            vec![mode(2), list("Focus", 6), write_ack(), list("Focus", 3)],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        let write = frames.iter().find(|f| is_write(f)).unwrap();
        assert_eq!(&write[..4], &[0x1F, 0x06, 0x02, 39]);
        let payload = &write[4..];

        let held = status_payload(2, "Focus", 6, true);
        assert_eq!(payload[0], 2);
        assert_eq!(payload[1..3], held[1..3], "voice prompt carried over");
        assert_eq!(payload[3..35], held[6..38], "name carried over");
        assert_eq!(payload[35], 3, "the new level");
        assert_eq!(payload[36], held[43]);
        assert_eq!(payload[37], held[44]);
        assert_eq!(payload[38], held[46]);
    }

    #[test]
    fn asking_for_the_level_it_already_has_sends_nothing() {
        let (change, frames) = run(vec![mode(2), list("Focus", 6)], |c| {
            set_level(c, 2, 6, policy()).unwrap()
        });
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!change.sent);
        assert!(!change.is_verified_change());
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    /// Only the mode the headphones are in is edited. If they have moved on,
    /// the saved mode is left alone.
    #[test]
    fn a_mode_the_headphones_are_not_in_is_left_alone() {
        let (result, frames) = run(vec![mode(0)], |c| set_level(c, 2, 3, policy()));
        let err = result.unwrap_err();
        assert!(matches!(err, BmapError::InvalidArg(_)), "{err:?}");
        assert!(err.to_string().contains("Quiet"), "{err}");
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    #[test]
    fn an_out_of_range_level_is_refused_before_anything_is_sent() {
        let (result, frames) = run(vec![], |c| set_level(c, 2, 11, policy()));
        assert!(matches!(result.unwrap_err(), BmapError::InvalidArg(_)));
        assert!(frames.is_empty());
    }

    #[test]
    fn a_preset_cannot_be_edited() {
        let (result, frames) = run(vec![mode(0), list("Focus", 6)], |c| {
            set_level(c, 0, 3, policy())
        });
        assert!(matches!(result.unwrap_err(), BmapError::Unsupported(_)));
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    #[test]
    fn an_empty_slot_cannot_be_edited() {
        let (result, frames) = run(vec![mode(3), list("Focus", 6)], |c| {
            set_level(c, 3, 3, policy())
        });
        assert!(matches!(result.unwrap_err(), BmapError::Unsupported(_)));
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    #[test]
    fn an_unreadable_baseline_sends_nothing() {
        let (result, frames) = run(vec![mode(2), timeout()], |c| set_level(c, 2, 3, policy()));
        assert!(result.is_err());
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    /// A mode with a setting the write would silently reset is not written.
    #[test]
    fn a_mode_using_automatic_cancellation_is_not_overwritten() {
        let mut burst = mode_list("Focus", 6);
        // Set the auto-CNC byte of slot 2's configuration. Each frame is a
        // 4-byte header plus 47 bytes, after two 4-byte bracketing frames.
        let slot2_payload = 8 + 2 * 51 + 4;
        burst[slot2_payload + 43] = 1;

        let (result, frames) = run(vec![mode(2), Ok(burst)], |c| set_level(c, 2, 3, policy()));
        let err = result.unwrap_err();
        assert!(matches!(err, BmapError::Unsupported(_)), "{err:?}");
        assert!(err.to_string().contains("automatic noise cancellation"), "{err}");
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    /// A name that would come back different after re-encoding is not written.
    #[test]
    fn a_name_that_would_not_survive_the_round_trip_is_not_overwritten() {
        let mut burst = mode_list("Focus", 6);
        let slot2_payload = 8 + 2 * 51 + 4;
        // Invalid UTF-8 in the name: parsing replaces it, so writing the
        // parsed name back would change the stored bytes.
        burst[slot2_payload + 6 + 5] = 0xFF;

        let (result, frames) = run(vec![mode(2), Ok(burst)], |c| set_level(c, 2, 3, policy()));
        let err = result.unwrap_err();
        assert!(matches!(err, BmapError::Unsupported(_)), "{err:?}");
        assert!(err.to_string().contains("byte for byte"), "{err}");
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    #[test]
    fn acknowledged_but_unchanged_is_unverified() {
        let (change, frames) = run(
            vec![
                mode(2),
                list("Focus", 6),
                write_ack(),
                list("Focus", 6),
                list("Focus", 6),
                list("Focus", 6),
            ],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }));
        assert!(!change.is_verified_change());
        assert_eq!(change.after.unwrap().raw_level, 6, "the device's own report");
        assert_eq!(frames.iter().filter(|f| is_write(f)).count(), 1, "never re-sent");
    }

    #[test]
    fn a_change_applied_late_is_verified_on_a_later_readback() {
        let (change, _) = run(
            vec![mode(2), list("Focus", 6), write_ack(), list("Focus", 6), list("Focus", 3)],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
    }

    #[test]
    fn a_missing_acknowledgement_is_settled_by_the_readback() {
        let (change, _) = run(
            vec![mode(2), list("Focus", 6), timeout(), list("Focus", 3)],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(matches!(change.ack, Some(Ack::Missing(_))));
    }

    #[test]
    fn an_error_reply_is_rejected_with_the_devices_reason() {
        let (change, _) = run(
            vec![mode(2), list("Focus", 6), write_error(1), list("Focus", 6)],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Rejected { .. }), "{:?}", change.verdict);
        assert!(change.sent);
        assert_eq!(change.after.unwrap().raw_level, 6);
    }

    #[test]
    fn a_channel_failure_during_the_write_says_the_outcome_is_unknown() {
        let (result, _) = run(
            vec![
                mode(2),
                list("Focus", 6),
                Err(BmapError::Connection("closed".into())),
            ],
            |c| set_level(c, 2, 3, policy()),
        );
        let err = result.unwrap_err();
        assert!(matches!(err, BmapError::Connection(_)));
        assert!(err.to_string().contains("not known whether"), "{err}");
    }

    /// The level is right, but something else about the user's saved mode
    /// changed too. That is not reported as a clean success.
    #[test]
    fn a_change_that_touched_more_than_the_level_is_not_a_clean_success() {
        let mut after = mode_list("Focus", 3);
        let slot2_payload = 8 + 2 * 51 + 4;
        after[slot2_payload + 46] = 1; // wind block now on

        let (change, _) = run(
            vec![mode(2), list("Focus", 6), write_ack(), Ok(after)],
            |c| set_level(c, 2, 3, policy()).unwrap(),
        );
        let Verdict::Unverified { reason } = change.verdict else {
            panic!("expected Unverified");
        };
        assert!(reason.contains("wind block"), "{reason}");
    }
}
