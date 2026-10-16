//! The headphones themselves, rather than their sound: what they are called,
//! whether they will stay connected to two devices at once, and which devices
//! they are paired with and connected to.
//!
//! Upstream describes a name for this model and nothing for the other two.
//! Every address, reply layout and behaviour here was observed on the test
//! headphones on 2026-10-13 (protocol-notes, Experiments 11 and 12), and as
//! everywhere on the control link a reply is trusted only if it has exactly
//! the shape that was observed, and a change is called applied only when the
//! headphones then report it.
//!
//! Two of the three hold other people's data: a paired device has a
//! Bluetooth address and a name. The address is used here to ask for the
//! name and is then dropped; nothing this module returns contains one. Where
//! a device has to be picked out again later — to connect it or disconnect
//! it — it goes by a key: its address hashed with this installation's salt,
//! which says nothing about the address and means nothing anywhere else.
//!
//! **Connecting and disconnecting a paired device has not been run on the
//! test headphones.** The two requests are an older model's, as documented
//! (QC35, `based-connect`); the experiment that would show whether this
//! model takes them is `hw_diag_paired_disconnect_and_reconnect`. Until it
//! has been run, what is here for them is a careful guess — it reads the
//! device's state first, sends one request, and calls the change applied
//! only if the headphones then report it — and is described as unverified
//! wherever it is described.

use super::noise::{is_link_failure, Ack, Verdict, VerifyPolicy};
use bmap::error::{BmapError, BmapResult};
use bmap::protocol::{bmap_packet, parse_all_responses, Operator};
use bmap::Transport;

/// `[1.2]`: the name. `GET` reads it, `SETGET` with the new name sets it.
const NAME: (u8, u8) = (1, 2);
/// `[1.10]`: multipoint. `GET` reads it, `SETGET` with 0 or 1 sets it.
const MULTIPOINT: (u8, u8) = (1, 10);
/// `[4.4]`: the addresses of the paired devices.
const PAIRED_LIST: (u8, u8) = (4, 4);
/// `[4.5]`: one paired device, asked for by address.
const PAIRED_INFO: (u8, u8) = (4, 5);
/// `[4.1]`: connect to a paired device. `START` with a zero and its address.
/// Not yet run on this model.
const CONNECT: (u8, u8) = (4, 1);
/// `[4.2]`: disconnect a paired device. `START` with its address. Not yet
/// run on this model.
const DISCONNECT: (u8, u8) = (4, 2);

/// The longest name that is sent, in bytes of UTF-8. Upstream's figure; the
/// headphones' own limit has not been looked for.
pub const MAX_NAME_BYTES: usize = 31;

/// More paired devices than this is not a list that has been seen.
const MOST_PAIRED: usize = 16;

fn unexpected(message: String) -> BmapError {
    BmapError::Device { message, code: 0 }
}

/// One request, and the state the headphones answer it with.
///
/// The reply has to come from the address that was asked. An `ERROR` from it
/// is the headphones saying no; anything else from it that is not a `STATUS`
/// is not an answer; and frames from elsewhere with none from it mean the
/// connection is out of step with the requests.
pub(super) fn exchange<T: Transport>(
    raw: &T,
    addr: (u8, u8),
    op: Operator,
    payload: &[u8],
) -> BmapResult<Vec<u8>> {
    let packet = bmap_packet(addr.0, addr.1, op, payload);
    let frames = parse_all_responses(&raw.send_recv(&packet)?);
    let first = frames.first().map(|f| (f.fblock, f.func));
    for frame in frames.into_iter().filter(|f| (f.fblock, f.func) == addr) {
        match frame.op {
            Operator::Status => return Ok(frame.payload),
            Operator::Error => {
                return Err(match frame.payload.first() {
                    Some(5) => BmapError::Auth(frame.fmt()),
                    code => BmapError::Device {
                        message: frame.fmt(),
                        code: code.copied().unwrap_or(0),
                    },
                });
            }
            _ => {}
        }
    }
    Err(match first {
        Some((fblock, func)) => BmapError::Desync(format!(
            "Response came from [{fblock}.{func}], expected [{}.{}]. Reopen the connection.",
            addr.0, addr.1
        )),
        None => BmapError::Timeout(format!("no answer from [{}.{}]", addr.0, addr.1)),
    })
}

// --- the name ---------------------------------------------------------------

/// A name as it may be given to the headphones: what is left after trimming,
/// or why not. Not empty, no longer than [`MAX_NAME_BYTES`], and nothing in it
/// that is not a printing character.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name cannot be empty.".into());
    }
    if name.chars().any(char::is_control) {
        return Err("A name cannot contain line breaks or control characters.".into());
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(format!(
            "That name is {} bytes long; the most the headphones are given is {MAX_NAME_BYTES}. \
             (A plain letter is one byte; accented letters and emoji are more.)",
            name.len()
        ));
    }
    Ok(name.to_string())
}

/// The name in a `[1.2]` status: one byte, 0 on the test headphones, and then
/// the name in UTF-8. A different first byte, or text that is not UTF-8, is
/// a layout that has not been described.
fn parse_name(payload: &[u8]) -> BmapResult<String> {
    let (flag, text) = payload
        .split_first()
        .ok_or_else(|| unexpected("the name reply was empty".into()))?;
    if *flag != 0 {
        return Err(unexpected(format!(
            "the name reply began with {flag:#04x} where 0x00 was observed"
        )));
    }
    let name = std::str::from_utf8(text)
        .map_err(|_| unexpected("the name the headphones reported is not UTF-8".into()))?;
    if name.is_empty() {
        return Err(unexpected("the headphones reported an empty name".into()));
    }
    Ok(name.to_string())
}

/// Reads what the headphones call themselves.
pub fn read_name<T: Transport>(raw: &T) -> BmapResult<String> {
    parse_name(&exchange(raw, NAME, Operator::Get, &[])?)
}

/// The full record of one attempt to rename the headphones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameChange {
    pub requested: String,
    /// What the headphones called themselves before anything was sent.
    pub before: String,
    pub sent: bool,
    pub ack: Option<Ack>,
    /// The most recent successful read-back, if any.
    pub after: Option<String>,
    pub verdict: Verdict,
}

impl NameChange {
    /// True only for a change the device itself confirmed.
    pub fn is_verified_change(&self) -> bool {
        self.verdict == Verdict::Verified && self.before != self.requested
    }
}

/// Renames the headphones and confirms it by reading the name back.
///
/// Returns `Err` when nothing meaningful can be reported — the name could not
/// be read first, so nothing was sent, or the channel failed outright. Every
/// case in which a write went out produces a [`NameChange`].
pub fn set_name<T: Transport>(
    raw: &T,
    requested: &str,
    policy: VerifyPolicy,
) -> BmapResult<NameChange> {
    let requested = check_name(requested).map_err(BmapError::InvalidArg)?;

    // What they are called now, and proof that the reply has the observed
    // shape, before anything is written.
    let before = read_name(raw)?;

    let record = |sent, ack, after, verdict| NameChange {
        requested: requested.clone(),
        before: before.clone(),
        sent,
        ack,
        after,
        verdict,
    };

    if before == requested {
        return Ok(record(false, None, Some(before.clone()), Verdict::AlreadyActive));
    }

    let ack = match exchange(raw, NAME, Operator::SetGet, requested.as_bytes()) {
        Ok(_) => Ack::Acknowledged,
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(record(
                true,
                None,
                read_name(raw).ok(),
                Verdict::Rejected {
                    reason: e.to_string(),
                },
            ));
        }
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the name was being written, so it is not known \
                 whether the headphones received it ({e})"
            )));
        }
        Err(e) => return Err(e),
    };

    let mut after: Option<String> = None;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read_name(raw) {
            Ok(now) => {
                let done = now == requested;
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
        Ack::Acknowledged => "The headphones acknowledged the name".to_string(),
        Ack::Missing(why) => format!("The headphones did not acknowledge the name ({why})"),
    };
    let verdict = match (&after, &last_error) {
        (Some(now), _) if *now == requested => Verdict::Verified,
        (Some(now), _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but they still call themselves {now:?} after {} read-back(s).",
                policy.attempts.max(1)
            ),
        },
        (None, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but the name could not be read back: {err}"),
        },
        (None, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };

    Ok(record(true, Some(ack), after, verdict))
}

// --- multipoint -------------------------------------------------------------

/// Whether multipoint is on, from a `[1.10]` status: one byte, `07` when it
/// is on and `06` when it is off. Those are the two values the test
/// headphones have given; any other is not read.
///
/// (Upstream reads this byte's second bit, for other models. On this one
/// that bit is set either way, and it is the first that changes.)
fn parse_multipoint(payload: &[u8]) -> BmapResult<bool> {
    match payload {
        [0x07] => Ok(true),
        [0x06] => Ok(false),
        other => Err(unexpected(format!(
            "the multipoint reply was {other:02x?} where 07 (on) or 06 (off) was observed"
        ))),
    }
}

/// Reads whether the headphones will hold two connections at once.
pub fn read_multipoint<T: Transport>(raw: &T) -> BmapResult<bool> {
    parse_multipoint(&exchange(raw, MULTIPOINT, Operator::Get, &[])?)
}

/// The full record of one attempt to switch multipoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipointChange {
    pub requested: bool,
    /// What the headphones reported before anything was sent.
    pub before: bool,
    pub sent: bool,
    pub ack: Option<Ack>,
    /// What the headphones last reported: a read-back, or failing that the
    /// state they answered the request with.
    pub after: Option<bool>,
    /// The headphones closed this connection straight after answering. They
    /// do that when multipoint goes off and the device they keep is another
    /// one.
    pub closed_after: bool,
    pub verdict: Verdict,
}

impl MultipointChange {
    /// True only for a change the device itself confirmed.
    pub fn is_verified_change(&self) -> bool {
        self.verdict == Verdict::Verified && self.before != self.requested
    }
}

/// Switches multipoint on or off, and confirms it from the headphones.
///
/// The headphones answer the request itself with their new state, and that
/// is one confirmation. A read-back afterwards is another, and is made —
/// but with multipoint going off the headphones keep one connection, and if
/// it is not this one they close it at once, so there may be nothing to read
/// back over. Then the state they answered with stands, and the record says
/// the connection was closed.
pub fn set_multipoint<T: Transport>(
    raw: &T,
    requested: bool,
    policy: VerifyPolicy,
) -> BmapResult<MultipointChange> {
    let before = read_multipoint(raw)?;

    let record = |sent, ack, after, closed_after, verdict| MultipointChange {
        requested,
        before,
        sent,
        ack,
        after,
        closed_after,
        verdict,
    };

    if before == requested {
        return Ok(record(false, None, Some(before), false, Verdict::AlreadyActive));
    }

    // What the headphones say their state is in the answer to the request.
    let mut answered: Option<bool> = None;
    let ack = match exchange(raw, MULTIPOINT, Operator::SetGet, &[u8::from(requested)]) {
        Ok(payload) => {
            answered = parse_multipoint(&payload).ok();
            Ack::Acknowledged
        }
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(record(
                true,
                None,
                read_multipoint(raw).ok(),
                false,
                Verdict::Rejected {
                    reason: e.to_string(),
                },
            ));
        }
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while multipoint was being switched, so it is not known \
                 whether the headphones received it ({e})"
            )));
        }
        Err(e) => return Err(e),
    };

    let mut read_back: Option<bool> = None;
    let mut closed = false;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read_multipoint(raw) {
            Ok(now) => {
                read_back = Some(now);
                last_error = None;
                if now == requested {
                    break;
                }
            }
            Err(e) => {
                closed = is_link_failure(&e);
                last_error = Some(e.to_string());
                if closed {
                    break;
                }
            }
        }
    }

    let after = read_back.or(answered);
    let state = |on: bool| if on { "on" } else { "off" };
    let acked = match &ack {
        Ack::Acknowledged => "The headphones acknowledged the change".to_string(),
        Ack::Missing(why) => format!("The headphones did not acknowledge the change ({why})"),
    };
    let verdict = match (read_back, answered, &last_error) {
        (Some(now), _, _) if now == requested => Verdict::Verified,
        // Nothing could be read back because the headphones closed the
        // connection, having first answered with the state asked for.
        (None, Some(now), _) if now == requested && closed => Verdict::Verified,
        (Some(now), _, _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but they report multipoint {} after {} read-back(s).",
                state(now),
                policy.attempts.max(1)
            ),
        },
        (None, _, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but multipoint could not be read back: {err}"),
        },
        (None, _, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };

    Ok(record(true, Some(ack), after, closed, verdict))
}

// --- paired devices ---------------------------------------------------------

/// A device the headphones are paired with. No address: it is asked for by
/// one, and that is the last use the address has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// Stands for the device where one has to be picked out again: see
    /// [`key_for`].
    pub key: String,
    /// What the headphones call it. May be empty.
    pub name: String,
    pub connected: bool,
    /// The device this request came from.
    pub this_device: bool,
}

/// One `[4.5]` status: the address asked for, a status byte, two bytes that
/// have not been identified, and the name.
///
/// The status byte was `03` for the device asking and `01` for the other
/// connected one: its first bit is "connected" and its second "this is you",
/// which is also how an older model's is documented, with `00` for a device
/// that is paired and not connected. Those three values are read; any other
/// is not.
fn parse_paired(address: &[u8], payload: &[u8]) -> BmapResult<PairedDevice> {
    if payload.len() < 9 {
        return Err(unexpected(format!(
            "a paired device's reply was {} byte(s) where at least 9 were observed",
            payload.len()
        )));
    }
    if &payload[..6] != address {
        return Err(unexpected(
            "a paired device's reply was about a different device from the one asked for".into(),
        ));
    }
    let status = payload[6];
    if !matches!(status, 0x00 | 0x01 | 0x03) {
        return Err(unexpected(format!(
            "a paired device's status was {status:#04x} where 00, 01 or 03 is documented"
        )));
    }
    let name = std::str::from_utf8(&payload[9..])
        .map_err(|_| unexpected("a paired device's name is not UTF-8".into()))?;
    Ok(PairedDevice {
        key: key_for(address),
        name: name.trim().to_string(),
        connected: status & 0x01 != 0,
        this_device: status & 0x02 != 0,
    })
}

/// What a paired device goes by outside this module: its address, hashed
/// with this installation's salt. The same device has the same key each time
/// it is listed, so it can be asked for again; the key cannot be turned back
/// into the address, and another installation's key for the same device is
/// different.
fn key_for(address: &[u8]) -> String {
    let hex: String = address.iter().map(|b| format!("{b:02x}")).collect();
    crate::util::stable_id(&format!("paired-device:{hex}"))
}

/// The addresses in the headphones' list of paired devices.
///
/// The list is one byte that has not been identified, then six bytes for
/// each device.
fn read_addresses<T: Transport>(raw: &T) -> BmapResult<Vec<[u8; 6]>> {
    let list = exchange(raw, PAIRED_LIST, Operator::Get, &[])?;
    let Some((_, addresses)) = list.split_first() else {
        return Err(unexpected("the list of paired devices was empty".into()));
    };
    if addresses.len() % 6 != 0 {
        return Err(unexpected(format!(
            "the list of paired devices was {} byte(s) after its first, which is not a whole \
             number of addresses",
            addresses.len()
        )));
    }
    if addresses.len() / 6 > MOST_PAIRED {
        return Err(unexpected(format!(
            "the list of paired devices had {} entries",
            addresses.len() / 6
        )));
    }
    Ok(addresses
        .chunks_exact(6)
        .map(|address| address.try_into().expect("six bytes"))
        .collect())
}

fn read_one<T: Transport>(raw: &T, address: &[u8; 6]) -> BmapResult<PairedDevice> {
    let reply = exchange(raw, PAIRED_INFO, Operator::Get, address)?;
    parse_paired(address, &reply)
}

/// Reads the devices the headphones are paired with, and which of them are
/// connected. One request for the list, and one for each device on it.
pub fn read_paired<T: Transport>(raw: &T) -> BmapResult<Vec<PairedDevice>> {
    read_addresses(raw)?
        .iter()
        .map(|address| read_one(raw, address))
        .collect()
}

/// How long the headphones are given to connect a device: they have to find
/// it first. Thirty looks, half a second apart.
pub const fn connect_policy() -> VerifyPolicy {
    VerifyPolicy {
        attempts: 30,
        interval: std::time::Duration::from_millis(500),
    }
}

/// And to disconnect one: sixteen looks, a quarter of a second apart.
pub const fn disconnect_policy() -> VerifyPolicy {
    VerifyPolicy {
        attempts: 16,
        interval: std::time::Duration::from_millis(250),
    }
}

/// The full record of one attempt to connect or disconnect a paired device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionChange {
    /// Connected (`true`) or disconnected (`false`) is what was asked for.
    pub requested: bool,
    /// What the headphones call the device.
    pub name: String,
    /// The device is the one this request came from.
    pub this_device: bool,
    /// Whether it was connected before anything was sent.
    pub before: bool,
    pub sent: bool,
    pub ack: Option<Ack>,
    /// Whether it was connected at the most recent read-back, if there was one.
    pub after: Option<bool>,
    /// The headphones closed this connection before a read-back could be
    /// made. Asked to disconnect the device asking, that is the change
    /// itself; otherwise it is what multipoint being off does when another
    /// device is connected.
    pub closed_after: bool,
    pub verdict: Verdict,
}

impl ConnectionChange {
    /// True only for a change the device itself confirmed.
    pub fn is_verified_change(&self) -> bool {
        self.verdict == Verdict::Verified && self.before != self.requested
    }
}

/// One `START` request, and whether the headphones took it.
fn start<T: Transport>(raw: &T, addr: (u8, u8), payload: &[u8]) -> BmapResult<()> {
    let packet = bmap_packet(addr.0, addr.1, Operator::Start, payload);
    let frames = parse_all_responses(&raw.send_recv(&packet)?);
    for frame in frames.iter().filter(|f| (f.fblock, f.func) == addr) {
        match frame.op {
            Operator::Error => {
                return Err(match frame.payload.first() {
                    Some(5) => BmapError::Auth(frame.fmt()),
                    code => BmapError::Device {
                        // The frame is not quoted: it can carry the address.
                        message: format!(
                            "[{}.{}] ERROR {}",
                            addr.0,
                            addr.1,
                            code.copied().unwrap_or(0)
                        ),
                        code: code.copied().unwrap_or(0),
                    },
                });
            }
            Operator::Result | Operator::Processing | Operator::Status => return Ok(()),
            _ => {}
        }
    }
    Err(BmapError::Timeout(format!(
        "no acknowledgement from [{}.{}]",
        addr.0, addr.1
    )))
}

/// Asks the headphones to connect to one of their paired devices, or to
/// disconnect from it, and confirms it from what they then report about that
/// device.
///
/// `key` is the device as [`read_paired`] gave it. The list is read again
/// here and the device looked for on it: a key that is not there — the
/// device has been unpaired since, or the key was never one of these — sends
/// nothing.
///
/// Not yet run on the test headphones: see the note at the top of this file.
pub fn set_paired_connection<T: Transport>(
    raw: &T,
    key: &str,
    connect: bool,
    policy: VerifyPolicy,
) -> BmapResult<ConnectionChange> {
    let address = read_addresses(raw)?
        .into_iter()
        .find(|address| key_for(address) == key)
        .ok_or_else(|| {
            BmapError::InvalidArg(
                "That device is no longer on the headphones' list of paired devices.".into(),
            )
        })?;

    // How it stands now, and proof that the reply has the observed shape,
    // before anything is sent.
    let before = read_one(raw, &address)?;

    let record = |sent, ack, after, closed_after, verdict| ConnectionChange {
        requested: connect,
        name: before.name.clone(),
        this_device: before.this_device,
        before: before.connected,
        sent,
        ack,
        after,
        closed_after,
        verdict,
    };

    if before.connected == connect {
        return Ok(record(false, None, Some(before.connected), false, Verdict::AlreadyActive));
    }

    let sent = if connect {
        let mut payload = vec![0x00];
        payload.extend_from_slice(&address);
        start(raw, CONNECT, &payload)
    } else {
        start(raw, DISCONNECT, &address)
    };
    let ack = match sent {
        Ok(()) => Ack::Acknowledged,
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(record(
                true,
                None,
                read_one(raw, &address).ok().map(|d| d.connected),
                false,
                Verdict::Rejected {
                    reason: e.to_string(),
                },
            ));
        }
        // Asked to drop the device that is asking, the headphones may go
        // before they answer. That is not a failure to report; it is looked
        // at below with everything else.
        Err(e) if is_link_failure(&e) && before.this_device && !connect => {
            Ack::Missing(e.to_string())
        }
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the request was being sent, so it is not known \
                 whether the headphones received it ({e})"
            )));
        }
        Err(e) => return Err(e),
    };

    let mut after: Option<bool> = None;
    let mut closed = false;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read_one(raw, &address) {
            Ok(now) => {
                after = Some(now.connected);
                last_error = None;
                if now.connected == connect {
                    break;
                }
            }
            Err(e) => {
                closed = is_link_failure(&e);
                last_error = Some(e.to_string());
                if closed {
                    break;
                }
            }
        }
    }

    let acked = match &ack {
        Ack::Acknowledged => "The headphones acknowledged the request".to_string(),
        Ack::Missing(why) => format!("The headphones did not acknowledge the request ({why})"),
    };
    let state = |on: bool| if on { "connected" } else { "not connected" };
    let verdict = match (after, &last_error) {
        (Some(now), _) if now == connect => Verdict::Verified,
        // This device was to be disconnected, the headphones took the
        // request, and this connection is gone: that is the disconnection.
        (_, _) if closed && before.this_device && !connect && ack == Ack::Acknowledged => {
            Verdict::Verified
        }
        (Some(now), _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but they report the device as {} after {} look(s).",
                state(now),
                policy.attempts.max(1)
            ),
        },
        (None, Some(_)) if closed => Verdict::Unverified {
            reason: format!(
                "{acked}, and then closed this connection before the change could be read back."
            ),
        },
        (None, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but the device could not be read back: {err}"),
        },
        (None, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };

    Ok(record(true, Some(ack), after, closed, verdict))
}

#[cfg(test)]
pub(crate) mod testing {
    /// The key a device with this address goes by.
    pub fn key_of(address: &[u8; 6]) -> String {
        super::key_for(address)
    }

    /// What the headphones answer a name read with.
    pub fn name_status(name: &str) -> Vec<u8> {
        let mut f = vec![0x01, 0x02, 0x03, (name.len() + 1) as u8, 0x00];
        f.extend_from_slice(name.as_bytes());
        f
    }

    /// What the headphones answer a multipoint read with.
    pub fn multipoint_status(on: bool) -> Vec<u8> {
        vec![0x01, 0x0A, 0x03, 0x01, if on { 0x07 } else { 0x06 }]
    }

    /// A list of paired devices: the unidentified first byte, then addresses.
    pub fn paired_list(addresses: &[[u8; 6]]) -> Vec<u8> {
        let mut f = vec![0x04, 0x04, 0x03, (1 + 6 * addresses.len()) as u8, 0x03];
        for address in addresses {
            f.extend_from_slice(address);
        }
        f
    }

    /// One paired device's reply.
    pub fn paired_info(address: [u8; 6], status: u8, name: &str) -> Vec<u8> {
        let mut f = vec![0x04, 0x05, 0x03, (9 + name.len()) as u8];
        f.extend_from_slice(&address);
        f.extend_from_slice(&[status, 0x01, 0x03]);
        f.extend_from_slice(name.as_bytes());
        f
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct Script {
        replies: RefCell<VecDeque<BmapResult<Vec<u8>>>>,
        sent: RefCell<Vec<Vec<u8>>>,
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

    fn run<R>(replies: Vec<BmapResult<Vec<u8>>>, f: impl FnOnce(&Script) -> R) -> (R, Vec<Vec<u8>>) {
        let script = Script {
            replies: RefCell::new(replies.into()),
            sent: RefCell::new(Vec::new()),
        };
        let result = f(&script);
        let sent = script.sent.borrow().clone();
        (result, sent)
    }

    const GET_NAME: [u8; 4] = [0x01, 0x02, 0x01, 0x00];
    const GET_MULTIPOINT: [u8; 4] = [0x01, 0x0A, 0x01, 0x00];
    const GET_LIST: [u8; 4] = [0x04, 0x04, 0x01, 0x00];
    const PHONE: [u8; 6] = [0x34, 0x11, 0x22, 0x33, 0x44, 0xF6];
    const LAPTOP: [u8; 6] = [0x9C, 0x55, 0x66, 0x77, 0x88, 0x49];

    fn name(name: &str) -> BmapResult<Vec<u8>> {
        Ok(name_status(name))
    }
    fn multipoint(on: bool) -> BmapResult<Vec<u8>> {
        Ok(multipoint_status(on))
    }
    fn closed() -> BmapResult<Vec<u8>> {
        Err(BmapError::Connection("The device closed the control channel".into()))
    }
    fn error_from(fblock: u8, func: u8, code: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![fblock, func, 0x04, 0x01, code])
    }
    fn policy() -> VerifyPolicy {
        VerifyPolicy::instant(3)
    }
    fn is_write(frame: &[u8]) -> bool {
        frame[2] & 0x0F == 0x02
    }

    // --- the name: reading ----------------------------------------------------

    /// The bytes the headphones answered with on 2026-10-13.
    #[test]
    fn reads_the_name_the_headphones_gave() {
        let reply = vec![0x01, 0x02, 0x03, 0x07, 0x00, 0x41, 0x75, 0x72, 0x6F, 0x72, 0x61];
        let (read, sent) = run(vec![Ok(reply)], |t| read_name(t).unwrap());
        assert_eq!(read, "Aurora");
        assert_eq!(sent, vec![GET_NAME.to_vec()], "one GET and nothing else");
    }

    #[test]
    fn a_name_need_not_be_ascii() {
        let (read, _) = run(vec![name("Señor Bose 🎧")], |t| read_name(t).unwrap());
        assert_eq!(read, "Señor Bose 🎧");
    }

    #[test]
    fn a_name_reply_of_another_shape_is_not_read() {
        // A first byte other than the one observed.
        let odd = vec![0x01, 0x02, 0x03, 0x07, 0x01, 0x41, 0x75, 0x72, 0x6F, 0x72, 0x61];
        assert!(run(vec![Ok(odd)], |t| read_name(t)).0.is_err());
        // Nothing after it.
        assert!(run(vec![Ok(vec![0x01, 0x02, 0x03, 0x01, 0x00])], |t| read_name(t)).0.is_err());
        // Nothing at all.
        assert!(run(vec![Ok(vec![0x01, 0x02, 0x03, 0x00])], |t| read_name(t)).0.is_err());
        // Not text.
        let binary = vec![0x01, 0x02, 0x03, 0x03, 0x00, 0xFF, 0xFE];
        assert!(run(vec![Ok(binary)], |t| read_name(t)).0.is_err());
    }

    #[test]
    fn a_reply_from_another_address_is_a_connection_out_of_step() {
        let (read, _) = run(vec![multipoint(true)], |t| read_name(t));
        assert!(matches!(read, Err(BmapError::Desync(_))), "{read:?}");
    }

    // --- the name: what may be asked for -----------------------------------------

    #[test]
    fn a_name_is_trimmed_and_must_be_something() {
        assert_eq!(check_name("  Aurora  ").as_deref(), Ok("Aurora"));
        assert!(check_name("").is_err());
        assert!(check_name("   ").is_err());
    }

    #[test]
    fn a_name_is_at_most_thirty_one_bytes() {
        assert!(check_name(&"x".repeat(31)).is_ok());
        assert!(check_name(&"x".repeat(32)).is_err());
        // Counted in bytes, which is what is sent: eleven of these are 33.
        assert!(check_name(&"é".repeat(15)).is_ok());
        assert!(check_name(&"é".repeat(16)).is_err());
    }

    #[test]
    fn a_name_has_no_control_characters() {
        for bad in ["Auro\nra", "Aurora\0", "A\tB", "\u{1b}[31mred"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    // --- the name: changing -------------------------------------------------------

    /// The exchange made with the headphones on 2026-10-13.
    #[test]
    fn a_rename_is_read_first_written_once_and_read_back() {
        let (change, sent) = run(
            vec![name("Aurora"), name("Aurora Test"), name("Aurora Test")],
            |t| set_name(t, "Aurora Test", policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_change());
        assert_eq!(change.before, "Aurora");
        assert_eq!(change.after.as_deref(), Some("Aurora Test"));
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0], GET_NAME);
        let mut write = vec![0x01, 0x02, 0x02, 11];
        write.extend_from_slice(b"Aurora Test");
        assert_eq!(sent[1], write, "the name as it is, with nothing before it");
        assert_eq!(sent[2], GET_NAME);
    }

    #[test]
    fn the_name_they_already_have_sends_nothing() {
        let (change, sent) = run(vec![name("Aurora")], |t| set_name(t, " Aurora ", policy()).unwrap());
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!change.sent && !change.is_verified_change());
        assert!(!sent.iter().any(|f| is_write(f)));
    }

    #[test]
    fn a_name_that_may_not_be_asked_for_sends_nothing_at_all() {
        let (result, sent) = run(vec![], |t| set_name(t, &"x".repeat(40), policy()));
        assert!(matches!(result, Err(BmapError::InvalidArg(_))));
        assert!(sent.is_empty());
    }

    #[test]
    fn nothing_is_written_when_the_name_cannot_be_read_first() {
        let (result, sent) = run(vec![error_from(1, 2, 3)], |t| set_name(t, "Nova", policy()));
        assert!(result.is_err());
        assert_eq!(sent, vec![GET_NAME.to_vec()]);
    }

    #[test]
    fn an_acknowledged_rename_that_did_not_take_is_not_called_applied() {
        let (change, sent) = run(
            vec![name("Aurora"), name("Nova"), name("Aurora"), name("Aurora"), name("Aurora")],
            |t| set_name(t, "Nova", policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }), "{:?}", change.verdict);
        assert_eq!(change.after.as_deref(), Some("Aurora"), "what they say, not what was asked");
        assert_eq!(sent.iter().filter(|f| is_write(f)).count(), 1, "the write is never repeated");
    }

    #[test]
    fn a_refused_rename_is_reported_as_refused() {
        let (change, _) = run(vec![name("Aurora"), error_from(1, 2, 4), name("Aurora")], |t| {
            set_name(t, "Nova", policy()).unwrap()
        });
        assert!(matches!(change.verdict, Verdict::Rejected { .. }));
        assert_eq!(change.after.as_deref(), Some("Aurora"));
    }

    #[test]
    fn a_link_lost_during_the_write_is_an_error_not_a_guess() {
        let (result, _) = run(vec![name("Aurora"), closed()], |t| set_name(t, "Nova", policy()));
        assert!(matches!(result, Err(BmapError::Connection(_))));
    }

    // --- multipoint: reading ---------------------------------------------------------

    /// Both bytes the headphones gave on 2026-10-13.
    #[test]
    fn reads_multipoint_on_and_off() {
        let on = vec![0x01, 0x0A, 0x03, 0x01, 0x07];
        let (read, sent) = run(vec![Ok(on)], |t| read_multipoint(t).unwrap());
        assert!(read);
        assert_eq!(sent, vec![GET_MULTIPOINT.to_vec()]);

        let off = vec![0x01, 0x0A, 0x03, 0x01, 0x06];
        assert!(!run(vec![Ok(off)], |t| read_multipoint(t).unwrap()).0);
    }

    /// Upstream's reading of this byte, for other models, would call these
    /// "on". Only the two values seen are read.
    #[test]
    fn any_other_multipoint_byte_is_not_read() {
        for byte in [0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x0F, 0xFF] {
            let reply = vec![0x01, 0x0A, 0x03, 0x01, byte];
            assert!(run(vec![Ok(reply)], |t| read_multipoint(t)).0.is_err(), "{byte:#04x}");
        }
        let two = vec![0x01, 0x0A, 0x03, 0x02, 0x07, 0x00];
        assert!(run(vec![Ok(two)], |t| read_multipoint(t)).0.is_err());
        let none = vec![0x01, 0x0A, 0x03, 0x00];
        assert!(run(vec![Ok(none)], |t| read_multipoint(t)).0.is_err());
    }

    #[test]
    fn headphones_without_multipoint_say_so() {
        let (read, _) = run(vec![error_from(1, 10, 2)], |t| read_multipoint(t));
        assert!(matches!(read, Err(BmapError::Device { .. })), "{read:?}");
    }

    // --- multipoint: changing --------------------------------------------------------

    #[test]
    fn switching_on_is_read_first_written_once_and_read_back() {
        let (change, sent) = run(
            vec![multipoint(false), multipoint(true), multipoint(true)],
            |t| set_multipoint(t, true, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_change());
        assert!(!change.closed_after);
        assert_eq!(change.after, Some(true));
        assert_eq!(
            sent,
            vec![
                GET_MULTIPOINT.to_vec(),
                vec![0x01, 0x0A, 0x02, 0x01, 0x01],
                GET_MULTIPOINT.to_vec()
            ]
        );
    }

    /// What happened on 2026-10-13: the headphones answered "off" and closed
    /// the connection to the computer that had asked, keeping the phone.
    #[test]
    fn switching_off_can_end_with_this_connection_closed_and_still_be_confirmed() {
        let (change, sent) = run(vec![multipoint(true), multipoint(false), closed()], |t| {
            set_multipoint(t, false, policy()).unwrap()
        });
        assert_eq!(change.verdict, Verdict::Verified, "they said off before they went");
        assert!(change.closed_after);
        assert_eq!(change.after, Some(false));
        assert_eq!(sent[1], vec![0x01, 0x0A, 0x02, 0x01, 0x00]);
        assert_eq!(sent.len(), 3, "one look for a read-back, and no more once the link is gone");
    }

    #[test]
    fn switching_off_with_this_connection_kept_is_read_back() {
        let (change, _) = run(vec![multipoint(true), multipoint(false), multipoint(false)], |t| {
            set_multipoint(t, false, policy()).unwrap()
        });
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(!change.closed_after);
    }

    #[test]
    fn the_state_they_are_already_in_sends_nothing() {
        let (change, sent) = run(vec![multipoint(true)], |t| set_multipoint(t, true, policy()).unwrap());
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!sent.iter().any(|f| is_write(f)));
    }

    /// A closed connection confirms nothing by itself: the headphones have to
    /// have said the new state first.
    #[test]
    fn a_closed_connection_with_no_answer_is_not_a_confirmation() {
        let (change, _) = run(
            vec![multipoint(true), Err(BmapError::Timeout("nothing".into())), closed()],
            |t| set_multipoint(t, false, policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }), "{:?}", change.verdict);
        assert_eq!(change.after, None);
    }

    #[test]
    fn an_answer_that_says_nothing_changed_is_not_called_applied() {
        let (change, sent) = run(
            vec![multipoint(true), multipoint(true), multipoint(true), multipoint(true), multipoint(true)],
            |t| set_multipoint(t, false, policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }));
        assert_eq!(change.after, Some(true));
        assert_eq!(sent.iter().filter(|f| is_write(f)).count(), 1);
    }

    #[test]
    fn a_refused_switch_is_reported_as_refused() {
        let (change, _) = run(vec![multipoint(true), error_from(1, 10, 4), multipoint(true)], |t| {
            set_multipoint(t, false, policy()).unwrap()
        });
        assert!(matches!(change.verdict, Verdict::Rejected { .. }));
        assert_eq!(change.after, Some(true));
    }

    #[test]
    fn nothing_is_switched_when_multipoint_cannot_be_read_first() {
        let odd = vec![0x01, 0x0A, 0x03, 0x01, 0x03];
        let (result, sent) = run(vec![Ok(odd)], |t| set_multipoint(t, false, policy()));
        assert!(result.is_err());
        assert_eq!(sent, vec![GET_MULTIPOINT.to_vec()]);
    }

    // --- paired devices ----------------------------------------------------------------

    fn info_request(address: [u8; 6]) -> Vec<u8> {
        let mut f = vec![0x04, 0x05, 0x01, 0x06];
        f.extend_from_slice(&address);
        f
    }

    /// The shape of what the headphones gave on 2026-10-13, with other
    /// addresses: a phone, and the computer that was asking.
    #[test]
    fn reads_each_paired_device_by_name_and_whether_it_is_connected() {
        let (devices, sent) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok({
                    let mut f = vec![0x04, 0x05, 0x03, 27];
                    f.extend_from_slice(&PHONE);
                    f.extend_from_slice(&[0x01, 0x02, 0x03]);
                    f.extend_from_slice(b"Jaisal's S24 Ultra");
                    f
                }),
                Ok(paired_info(LAPTOP, 0x03, "NEMESIS")),
            ],
            |t| read_paired(t).unwrap(),
        );
        assert_eq!(
            devices,
            vec![
                PairedDevice {
                    key: key_for(&PHONE),
                    name: "Jaisal's S24 Ultra".into(),
                    connected: true,
                    this_device: false
                },
                PairedDevice {
                    key: key_for(&LAPTOP),
                    name: "NEMESIS".into(),
                    connected: true,
                    this_device: true
                },
            ]
        );
        assert_eq!(sent, vec![GET_LIST.to_vec(), info_request(PHONE), info_request(LAPTOP)]);
    }

    #[test]
    fn a_paired_device_that_is_not_connected_is_shown_as_not_connected() {
        let (devices, _) = run(
            vec![Ok(paired_list(&[PHONE])), Ok(paired_info(PHONE, 0x00, "Old tablet"))],
            |t| read_paired(t).unwrap(),
        );
        assert_eq!(
            devices,
            vec![PairedDevice {
                key: key_for(&PHONE),
                name: "Old tablet".into(),
                connected: false,
                this_device: false
            }]
        );
    }

    #[test]
    fn no_paired_devices_is_an_empty_list() {
        let (devices, sent) = run(vec![Ok(paired_list(&[]))], |t| read_paired(t).unwrap());
        assert!(devices.is_empty());
        assert_eq!(sent.len(), 1);
    }

    /// Nothing that comes back holds an address: not in a field, and not in
    /// how it prints.
    #[test]
    fn what_is_returned_has_no_address_in_it() {
        let (devices, _) = run(
            vec![Ok(paired_list(&[PHONE])), Ok(paired_info(PHONE, 0x01, "Phone"))],
            |t| read_paired(t).unwrap(),
        );
        let printed = format!("{devices:?}").to_lowercase();
        // Not as hexadecimal run together, with separators, or as numbers.
        for form in ["3411223344f6", "34:11:22", "34-11-22", "34 11 22", "52, 17, 34", "[52"] {
            assert!(!printed.contains(form), "{form} in {printed}");
        }
        assert!(!printed.contains("address"));
    }

    /// A device goes by a key that is the same each time it is listed,
    /// different for each device, and not its address in any form.
    #[test]
    fn a_paired_device_goes_by_a_key_that_is_not_its_address() {
        let (first, _) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok(paired_info(PHONE, 0x01, "Phone")),
                Ok(paired_info(LAPTOP, 0x03, "Laptop")),
            ],
            |t| read_paired(t).unwrap(),
        );
        let (again, _) = run(
            vec![
                Ok(paired_list(&[LAPTOP, PHONE])),
                Ok(paired_info(LAPTOP, 0x03, "Laptop")),
                Ok(paired_info(PHONE, 0x00, "Phone")),
            ],
            |t| read_paired(t).unwrap(),
        );
        assert_ne!(first[0].key, first[1].key);
        assert_eq!(first[0].key, again[1].key, "the phone, whatever order or state it is in");
        assert_eq!(first[0].key.len(), 20);
        assert!(first[0].key.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!first[0].key.contains("3411"), "{}", first[0].key);
    }

    // --- connecting and disconnecting a paired device -------------------------------

    fn ack(fblock: u8, func: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![fblock, func, 0x07, 0x00])
    }
    fn disconnect_request(address: [u8; 6]) -> Vec<u8> {
        let mut f = vec![0x04, 0x02, 0x05, 0x06];
        f.extend_from_slice(&address);
        f
    }
    fn connect_request(address: [u8; 6]) -> Vec<u8> {
        let mut f = vec![0x04, 0x01, 0x05, 0x07, 0x00];
        f.extend_from_slice(&address);
        f
    }
    fn is_request(frame: &[u8]) -> bool {
        frame[0] == 4 && matches!(frame[1], 1 | 2)
    }

    #[test]
    fn a_disconnection_is_looked_up_read_first_asked_once_and_read_back() {
        let (change, sent) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok(paired_info(LAPTOP, 0x01, "Laptop")), // before: connected
                ack(4, 2),
                Ok(paired_info(LAPTOP, 0x01, "Laptop")), // not yet
                Ok(paired_info(LAPTOP, 0x00, "Laptop")), // gone
            ],
            |t| set_paired_connection(t, &key_for(&LAPTOP), false, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_change());
        assert_eq!((change.name.as_str(), change.before, change.after), ("Laptop", true, Some(false)));
        assert!(!change.closed_after);
        assert_eq!(
            sent,
            vec![
                GET_LIST.to_vec(),
                info_request(LAPTOP),
                disconnect_request(LAPTOP),
                info_request(LAPTOP),
                info_request(LAPTOP),
            ]
        );
    }

    #[test]
    fn a_connection_is_asked_for_with_a_zero_before_the_address() {
        let (change, sent) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok(paired_info(LAPTOP, 0x00, "Laptop")),
                ack(4, 1),
                Ok(paired_info(LAPTOP, 0x01, "Laptop")),
            ],
            |t| set_paired_connection(t, &key_for(&LAPTOP), true, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert_eq!(sent[2], connect_request(LAPTOP));
        assert_eq!(sent.iter().filter(|f| is_request(f)).count(), 1);
    }

    #[test]
    fn a_device_already_as_asked_is_sent_nothing() {
        let (change, sent) = run(
            vec![Ok(paired_list(&[PHONE])), Ok(paired_info(PHONE, 0x01, "Phone"))],
            |t| set_paired_connection(t, &key_for(&PHONE), true, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!sent.iter().any(|f| is_request(f)));
    }

    /// A key that is not on the list — never was, or the device has been
    /// unpaired — reaches no device: nothing but the list is asked for.
    #[test]
    fn a_key_that_is_not_on_the_list_sends_nothing() {
        for key in ["00000000000000000000", "", "not-a-key", &key_for(&LAPTOP)] {
            let (result, sent) = run(vec![Ok(paired_list(&[PHONE]))], |t| {
                set_paired_connection(t, key, false, policy())
            });
            assert!(matches!(result, Err(BmapError::InvalidArg(_))), "{key:?}: {result:?}");
            assert_eq!(sent, vec![GET_LIST.to_vec()], "{key:?}");
        }
    }

    /// An address cannot be asked for from outside: only a key can, and a key
    /// is matched against the headphones' own list.
    #[test]
    fn an_address_given_as_a_key_reaches_no_device() {
        let as_hex = "3411223344f6";
        let (result, sent) = run(vec![Ok(paired_list(&[PHONE]))], |t| {
            set_paired_connection(t, as_hex, false, policy())
        });
        assert!(result.is_err());
        assert!(!sent.iter().any(|f| is_request(f)));
    }

    #[test]
    fn an_acknowledged_request_that_changed_nothing_is_not_called_applied() {
        let mut replies = vec![
            Ok(paired_list(&[LAPTOP])),
            Ok(paired_info(LAPTOP, 0x01, "Laptop")),
            ack(4, 2),
        ];
        replies.extend((0..3).map(|_| Ok(paired_info(LAPTOP, 0x01, "Laptop"))));
        let (change, sent) = run(replies, |t| {
            set_paired_connection(t, &key_for(&LAPTOP), false, policy()).unwrap()
        });
        assert!(matches!(change.verdict, Verdict::Unverified { .. }), "{:?}", change.verdict);
        assert_eq!(change.after, Some(true), "what they say, not what was asked");
        assert_eq!(sent.iter().filter(|f| is_request(f)).count(), 1, "never asked twice");
    }

    #[test]
    fn a_refused_request_is_reported_as_refused_without_quoting_the_frame() {
        let (change, _) = run(
            vec![
                Ok(paired_list(&[LAPTOP])),
                Ok(paired_info(LAPTOP, 0x01, "Laptop")),
                error_from(4, 2, 3),
                Ok(paired_info(LAPTOP, 0x01, "Laptop")),
            ],
            |t| set_paired_connection(t, &key_for(&LAPTOP), false, policy()).unwrap(),
        );
        let Verdict::Rejected { reason } = &change.verdict else {
            panic!("{:?}", change.verdict);
        };
        assert!(reason.contains("[4.2]"), "{reason}");
        assert_eq!(change.after, Some(true));
    }

    /// Disconnecting the device that is asking ends with nothing to read
    /// back over: the headphones take the request and are gone.
    #[test]
    fn disconnecting_this_device_is_confirmed_by_the_link_going() {
        let (change, _) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok(paired_info(LAPTOP, 0x03, "Laptop")), // this device
                ack(4, 2),
                closed(),
            ],
            |t| set_paired_connection(t, &key_for(&LAPTOP), false, policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.closed_after && change.this_device);
    }

    /// But a link that goes after a request about some other device confirms
    /// nothing about that device.
    #[test]
    fn a_link_lost_after_a_request_about_another_device_confirms_nothing() {
        let (change, _) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok(paired_info(PHONE, 0x00, "Phone")),
                ack(4, 1),
                closed(),
            ],
            |t| set_paired_connection(t, &key_for(&PHONE), true, policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }), "{:?}", change.verdict);
        assert!(change.closed_after);
    }

    #[test]
    fn nothing_is_asked_when_the_device_cannot_be_read_first() {
        let (result, sent) = run(
            vec![Ok(paired_list(&[PHONE])), Ok(paired_info(PHONE, 0x07, "Phone"))],
            |t| set_paired_connection(t, &key_for(&PHONE), false, policy()),
        );
        assert!(result.is_err());
        assert!(!sent.iter().any(|f| is_request(f)));
    }

    #[test]
    fn a_list_or_a_device_of_another_shape_is_not_read() {
        // Not a whole number of addresses.
        let ragged = vec![0x04, 0x04, 0x03, 0x04, 0x03, 0x34, 0x11, 0x22];
        assert!(run(vec![Ok(ragged)], |t| read_paired(t)).0.is_err());
        // No first byte.
        assert!(run(vec![Ok(vec![0x04, 0x04, 0x03, 0x00])], |t| read_paired(t)).0.is_err());
        // A reply about a different device from the one asked for.
        let other = vec![Ok(paired_list(&[PHONE])), Ok(paired_info(LAPTOP, 0x01, "X"))];
        assert!(run(other, |t| read_paired(t)).0.is_err());
        // A status that is none of the three documented.
        for status in [0x02u8, 0x04, 0x07, 0x81] {
            let odd = vec![Ok(paired_list(&[PHONE])), Ok(paired_info(PHONE, status, "X"))];
            assert!(run(odd, |t| read_paired(t)).0.is_err(), "{status:#04x}");
        }
        // Too short to hold what follows the address.
        let mut short = vec![0x04, 0x05, 0x03, 0x07];
        short.extend_from_slice(&PHONE);
        short.push(0x01);
        assert!(run(vec![Ok(paired_list(&[PHONE])), Ok(short)], |t| read_paired(t)).0.is_err());
    }

    /// Reading the list asks for the list and each device's details, and for
    /// nothing else. (Function 3 of this block removes a device from the
    /// headphones. Nothing in this module can send it.)
    #[test]
    fn reading_the_paired_devices_only_reads() {
        let (_, sent) = run(
            vec![
                Ok(paired_list(&[PHONE, LAPTOP])),
                Ok(paired_info(PHONE, 0x01, "Phone")),
                Ok(paired_info(LAPTOP, 0x03, "Laptop")),
            ],
            |t| read_paired(t),
        );
        for frame in &sent {
            assert_eq!(frame[0], 4);
            assert!(matches!(frame[1], 4 | 5), "only the list and one device's details");
            assert_eq!(frame[2] & 0x0F, 0x01, "a GET");
        }
    }
}
