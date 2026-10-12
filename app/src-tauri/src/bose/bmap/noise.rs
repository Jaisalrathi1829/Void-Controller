//! Quiet / Aware switching, with the result confirmed by the device.
//!
//! Upstream's `BmapConnection::set_mode` sends the switch and returns once the
//! device acknowledges it. That is the right primitive, but an acknowledgement
//! is not the same thing as the mode having changed: this firmware answers
//! `START [31.3]` with `PROCESSING` and applies the change afterwards. So this
//! module wraps the primitive in the discipline the rest of the application
//! uses for every hardware control — read the state, request the change, read
//! the state again — and keeps three things apart that are easy to blur:
//!
//!   * the command was **sent**
//!   * the device **acknowledged** it
//!   * the device **reports the new mode**
//!
//! Only the third is ever described as success.

use bmap::device::DeviceConfig;
use bmap::error::{BmapError, BmapResult};
use bmap::protocol::{bmap_packet, parse_all_responses, Operator};
use bmap::{BmapConnection, Transport};
use std::time::Duration;

/// The two fixed noise-control presets this application drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Quiet,
    Aware,
}

impl Preset {
    /// The preset's key in upstream's model configs.
    pub fn key(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Aware => "aware",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Quiet => "Quiet",
            Self::Aware => "Aware",
        }
    }
}

/// What a mode change is aiming at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// One of the fixed presets.
    Preset(Preset),
    /// A user-configured mode, by the slot the device lists it under. The
    /// name is the device's own and is only ever used to describe the request.
    Slot { index: u8, name: String },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Self::Preset(p) => p.label().to_string(),
            Self::Slot { name, .. } => format!("{name:?}"),
        }
    }

    /// The mode index this target has on a given model.
    ///
    /// A slot is only accepted if it is one of the model's editable slots:
    /// the set of indices that can ever be requested is the presets plus
    /// those, never an arbitrary number.
    fn index(&self, config: &DeviceConfig) -> Result<u8, BmapError> {
        match self {
            Self::Preset(p) => preset_index(config, *p).ok_or_else(|| {
                BmapError::Unsupported(format!("{} has no {} preset", config.info.name, p.label()))
            }),
            Self::Slot { index, .. } if config.editable_slots.contains(index) => Ok(*index),
            Self::Slot { index, .. } => Err(BmapError::Unsupported(format!(
                "slot {index} is not a custom-mode slot on {}",
                config.info.name
            ))),
        }
    }
}

/// The mode index a preset has on a given model, from upstream's config.
///
/// Indices are model data, not constants of this application: they are looked
/// up rather than assumed to be 0 and 1.
pub fn preset_index(config: &DeviceConfig, preset: Preset) -> Option<u8> {
    config
        .preset_modes
        .iter()
        .find(|(name, _)| *name == preset.key())
        .map(|(_, mode)| mode.idx)
}

/// The preset a mode index corresponds to on a given model, if any. An index
/// outside the presets is a user-configured slot.
pub fn preset_for_index(config: &DeviceConfig, index: u8) -> Option<Preset> {
    [Preset::Quiet, Preset::Aware]
        .into_iter()
        .find(|p| preset_index(config, *p) == Some(index))
}

/// How a mode index reads to a person.
pub fn describe_index(config: &DeviceConfig, index: u8) -> String {
    match preset_for_index(config, index) {
        Some(p) => p.label().to_string(),
        None => format!("custom slot {index}"),
    }
}

/// Whether the device acknowledged the switch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ack {
    /// The device answered with RESULT or PROCESSING.
    Acknowledged,
    /// The request was transmitted but no acknowledgement arrived in time.
    Missing(String),
}

/// What the read-back established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The device reported a different mode before, and reports the requested
    /// mode now.
    Verified,
    /// The device already reported the requested mode. Nothing was sent.
    AlreadyActive,
    /// The request went out but the device does not report the requested mode.
    Unverified { reason: String },
    /// The device answered the request with ERROR.
    Rejected { reason: String },
}

/// The full record of one mode-change attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChange {
    pub requested: Target,
    pub requested_index: u8,
    /// Mode index the device reported before anything was sent.
    pub before: u8,
    /// Whether a switch request was transmitted at all.
    pub sent: bool,
    pub ack: Option<Ack>,
    /// Mode index from the most recent successful read-back, if any.
    pub after: Option<u8>,
    pub verdict: Verdict,
}

impl ModeChange {
    /// True only for a change the device itself confirmed.
    pub fn is_verified_transition(&self) -> bool {
        self.verdict == Verdict::Verified && self.before != self.requested_index
    }
}

/// How long to keep reading back before giving up. Bounded on both axes.
#[derive(Debug, Clone, Copy)]
pub struct VerifyPolicy {
    pub attempts: u32,
    pub interval: Duration,
}

impl VerifyPolicy {
    /// Up to thirty read-backs, 50 ms apart: roughly two seconds at most.
    ///
    /// It was five, 250 ms apart. On the test headphones the new mode is
    /// there by the first read-back, for a preset and for a custom mode
    /// alike, so the interval rarely comes into it; when the first look is
    /// too early, the next is now a twentieth of a second later rather than
    /// a quarter. A read-back is one small question, the same one asked
    /// every 150 ms all the while mode announcements are on.
    pub const fn standard() -> Self {
        Self {
            attempts: 30,
            interval: Duration::from_millis(50),
        }
    }

    /// For a custom mode's level. Each read-back lists every mode, which is
    /// a longer answer than a single mode read, so they are a little further
    /// apart and there are fewer of them: about a second and a half at most.
    pub const fn level() -> Self {
        Self {
            attempts: 16,
            interval: Duration::from_millis(60),
        }
    }

    pub const fn instant(attempts: u32) -> Self {
        Self {
            attempts,
            interval: Duration::ZERO,
        }
    }
}

/// Whether an error means the channel itself is no longer usable, as opposed
/// to the device having answered with something unwelcome.
pub fn is_link_failure(err: &BmapError) -> bool {
    matches!(
        err,
        BmapError::Connection(_) | BmapError::Desync(_) | BmapError::Busy(_)
    )
}

/// Reads the current mode index.
pub fn read_mode<T: Transport>(conn: &BmapConnection<T>) -> BmapResult<u8> {
    conn.mode_idx()
}

/// Asks the device to switch to a custom-mode slot.
///
/// Upstream's `set_mode` finds a custom mode by *name*, which is ambiguous
/// when two slots share one. This sends the same request — `START` on the
/// current-mode function, built by upstream's codec — by index instead, and
/// applies the same acceptance rule to the reply.
///
/// It is sent the way upstream sends it for a preset: one request, and the
/// first reply from the address that was asked. `raw` is the transport the
/// connection runs over, because the connection itself only offers that for
/// its own named modes. What it does offer for an arbitrary request collects
/// frames until the device has been silent for half a second — and the
/// headphones answer a switch with a single `PROCESSING` and nothing after
/// it, so that was half a second spent waiting for nothing, every time, and
/// only for the custom mode. Measured on the headphones: a switch into the
/// custom mode confirmed after about 600 ms, against 100 ms for a preset,
/// with the mode already changed by the first read-back.
fn request_slot<T: Transport>(conn: &BmapConnection<T>, raw: &T, index: u8) -> BmapResult<()> {
    let addr = conn.config().current_mode.ok_or_else(|| {
        BmapError::Unsupported(format!("{} has no mode switching", conn.config().info.name))
    })?;
    let packet = bmap_packet(addr.0, addr.1, Operator::Start, &[index, 0]);
    let replies = parse_all_responses(&raw.send_recv(&packet)?);
    for reply in replies
        .iter()
        .filter(|r| (r.fblock, r.func) == (addr.0, addr.1))
    {
        match reply.op {
            Operator::Error => {
                return Err(match reply.payload.first() {
                    Some(5) => BmapError::Auth(reply.fmt()),
                    code => BmapError::Device {
                        message: reply.fmt(),
                        code: code.copied().unwrap_or(0),
                    },
                });
            }
            Operator::Result | Operator::Processing => return Ok(()),
            _ => {}
        }
    }
    Err(BmapError::Timeout(
        "no acknowledgement of the mode switch".into(),
    ))
}

/// Requests a preset and confirms the outcome by reading the mode back.
pub fn change_mode<T: Transport>(
    conn: &BmapConnection<T>,
    raw: &T,
    preset: Preset,
    policy: VerifyPolicy,
) -> BmapResult<ModeChange> {
    change_to(conn, raw, &Target::Preset(preset), policy)
}

/// Requests a mode — a preset or a custom slot — and confirms the outcome by
/// reading the mode back.
///
/// Returns `Err` only when no meaningful report can be made — the baseline
/// could not be read, so nothing was sent, or the channel failed outright.
/// Every case in which a request went out produces a [`ModeChange`].
pub fn change_to<T: Transport>(
    conn: &BmapConnection<T>,
    raw: &T,
    target: &Target,
    policy: VerifyPolicy,
) -> BmapResult<ModeChange> {
    let requested_index = target.index(conn.config())?;

    // Baseline first. Without knowing where the device started there would be
    // no way to tell a change from a coincidence, so nothing is sent blind.
    let before = read_mode(conn)?;

    if before == requested_index {
        return Ok(ModeChange {
            requested: target.clone(),
            requested_index,
            before,
            sent: false,
            ack: None,
            after: Some(before),
            verdict: Verdict::AlreadyActive,
        });
    }

    let request = match target {
        Target::Preset(preset) => conn.set_mode(preset.key(), false),
        Target::Slot { index, .. } => request_slot(conn, raw, *index),
    };
    let ack = match request {
        Ok(()) => Ack::Acknowledged,
        // Transmitted, but nothing came back in time. The change may still
        // have been applied, so the read-back below decides.
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        // The device said no. Read back so the caller still learns the state.
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(ModeChange {
                requested: target.clone(),
                requested_index,
                before,
                sent: true,
                ack: None,
                after: read_mode(conn).ok(),
                verdict: Verdict::Rejected {
                    reason: e.to_string(),
                },
            });
        }
        // The channel failed somewhere between handing the request to the
        // socket and hearing back. Whether the device received it cannot be
        // known from here, and the error has to say so rather than imply
        // that nothing happened.
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the change to {} was being requested, so it is \
                 not known whether the headphones received it ({e})",
                target.label()
            )));
        }
        Err(e) => return Err(e),
    };

    // The switch is applied asynchronously, so poll — a bounded number of
    // times — until the device reports the requested mode.
    let mut after: Option<u8> = None;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read_mode(conn) {
            Ok(index) => {
                after = Some(index);
                last_error = None;
                if index == requested_index {
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
        Ack::Acknowledged => "The device acknowledged the request".to_string(),
        Ack::Missing(why) => format!("The device did not acknowledge the request ({why})"),
    };

    let verdict = match (after, &last_error) {
        (Some(index), _) if index == requested_index => Verdict::Verified,
        (Some(index), _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but it still reports {} after {} read-back(s).",
                describe_index(conn.config(), index),
                policy.attempts.max(1)
            ),
        },
        (None, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but the mode could not be read back: {err}"),
        },
        (None, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };

    Ok(ModeChange {
        requested: target.clone(),
        requested_index,
        before,
        sent: true,
        ack: Some(ack),
        after,
        verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bmap::devices;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// A transport that answers each request with the next scripted reply.
    /// Unlike upstream's fixed-reply mock it can model state changing between
    /// requests, which is the whole subject of this module.
    #[derive(Default)]
    struct Script {
        replies: RefCell<VecDeque<BmapResult<Vec<u8>>>>,
        sent: RefCell<Vec<Vec<u8>>>,
        /// How many requests were sent as ones whose reply is collected until
        /// the device falls silent.
        drains: std::cell::Cell<usize>,
    }

    impl Script {
        fn new(replies: Vec<BmapResult<Vec<u8>>>) -> Self {
            Self {
                replies: RefCell::new(replies.into()),
                sent: RefCell::new(Vec::new()),
                drains: std::cell::Cell::new(0),
            }
        }
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

    const GET_MODE: [u8; 4] = [0x1F, 0x03, 0x01, 0x00];
    const START_QUIET: [u8; 6] = [0x1F, 0x03, 0x05, 0x02, 0x00, 0x00];
    const START_AWARE: [u8; 6] = [0x1F, 0x03, 0x05, 0x02, 0x01, 0x00];

    fn status(index: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x03, 0x03, 0x01, index])
    }
    fn processing() -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x03, 0x07, 0x00])
    }
    fn result(index: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x03, 0x06, 0x01, index])
    }
    fn error(code: u8) -> BmapResult<Vec<u8>> {
        Ok(vec![0x1F, 0x03, 0x04, 0x01, code])
    }
    fn timeout() -> BmapResult<Vec<u8>> {
        Err(BmapError::Timeout("No reply".into()))
    }

    /// A script shared with the connection through an Rc, so a test can see
    /// what was sent after the connection has taken ownership of it — and so
    /// that the connection and the transport handed in beside it are one.
    struct Shared(Rc<Script>);
    impl Transport for Shared {
        fn send_recv(&self, p: &[u8]) -> BmapResult<Vec<u8>> {
            self.0.send_recv(p)
        }
        fn send_recv_drain(&self, p: &[u8]) -> BmapResult<Vec<u8>> {
            self.0.drains.set(self.0.drains.get() + 1);
            self.0.send_recv(p)
        }
    }

    /// A connection and the transport underneath it, as the link holds them.
    struct Rig {
        conn: BmapConnection<Shared>,
        raw: Shared,
    }

    fn rig_for(config: DeviceConfig, replies: Vec<BmapResult<Vec<u8>>>) -> (Rig, Rc<Script>) {
        let script = Rc::new(Script::new(replies));
        let rig = Rig {
            conn: BmapConnection::new(Shared(Rc::clone(&script)), config),
            raw: Shared(Rc::clone(&script)),
        };
        (rig, script)
    }

    fn prince(replies: Vec<BmapResult<Vec<u8>>>) -> Rig {
        rig_for(devices::qc_prince(), replies).0
    }

    fn shared(replies: Vec<BmapResult<Vec<u8>>>) -> (Rig, Rc<Script>) {
        rig_for(devices::qc_prince(), replies)
    }

    // The functions under test, taking the pair. Every test below calls
    // these, so each goes through exactly what the link calls.
    fn change_mode(rig: &Rig, preset: Preset, policy: VerifyPolicy) -> BmapResult<ModeChange> {
        super::change_mode(&rig.conn, &rig.raw, preset, policy)
    }
    fn change_to(rig: &Rig, target: &Target, policy: VerifyPolicy) -> BmapResult<ModeChange> {
        super::change_to(&rig.conn, &rig.raw, target, policy)
    }

    // --- model data --------------------------------------------------------

    #[test]
    fn preset_indices_come_from_the_upstream_model_config() {
        let cfg = devices::qc_prince();
        assert_eq!(preset_index(&cfg, Preset::Quiet), Some(0));
        assert_eq!(preset_index(&cfg, Preset::Aware), Some(1));
        assert_eq!(preset_for_index(&cfg, 0), Some(Preset::Quiet));
        assert_eq!(preset_for_index(&cfg, 1), Some(Preset::Aware));
        assert_eq!(preset_for_index(&cfg, 2), None, "slot 2 is user-configured");
        assert_eq!(describe_index(&cfg, 2), "custom slot 2");
    }

    /// A model without these presets must be refused, not driven by guesswork.
    #[test]
    fn a_model_without_the_preset_is_unsupported() {
        let (conn, _) = rig_for(devices::qc35(), vec![]);
        let err = change_mode(&conn, Preset::Aware, VerifyPolicy::instant(1)).unwrap_err();
        assert!(matches!(err, BmapError::Unsupported(_)), "got {err:?}");
    }

    // --- the happy path, as the device actually behaves ---------------------

    /// PROCESSING acknowledgement, then the read-back shows the new mode.
    #[test]
    fn processing_ack_then_readback_is_verified() {
        let conn = prince(vec![status(1), processing(), status(0)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.before, 1);
        assert!(change.sent);
        assert_eq!(change.ack, Some(Ack::Acknowledged));
        assert_eq!(change.after, Some(0));
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_transition());
    }

    #[test]
    fn result_ack_is_also_accepted() {
        let conn = prince(vec![status(0), result(1), status(1)]);
        let change = change_mode(&conn, Preset::Aware, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.verdict, Verdict::Verified);
    }

    /// The change is asynchronous: the first read-back can still show the old
    /// mode. That is not a failure until the bounded retries run out.
    #[test]
    fn late_application_is_verified_on_a_later_readback() {
        let conn = prince(vec![status(1), processing(), status(1), status(1), status(0)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(5)).unwrap();
        assert_eq!(change.verdict, Verdict::Verified);
        assert_eq!(change.after, Some(0));
    }

    // --- nothing is sent when nothing needs sending -------------------------

    #[test]
    fn already_in_the_requested_mode_sends_nothing() {
        let conn = prince(vec![status(0)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!change.sent);
        assert!(change.ack.is_none());
        // Being in the mode already is not evidence that switching works.
        assert!(!change.is_verified_transition());
    }

    /// If the baseline cannot be read, no switch is transmitted.
    #[test]
    fn unreadable_baseline_sends_nothing() {
        let conn = prince(vec![timeout()]);
        let err = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap_err();
        assert!(matches!(err, BmapError::Timeout(_)), "got {err:?}");
    }

    // --- acknowledged is not verified ---------------------------------------

    /// The device acknowledges but never reports the new mode. This must not
    /// be reported as success.
    #[test]
    fn acknowledged_but_unchanged_is_unverified() {
        let conn = prince(vec![status(1), processing(), status(1), status(1), status(1)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.ack, Some(Ack::Acknowledged));
        assert_eq!(change.after, Some(1));
        match &change.verdict {
            Verdict::Unverified { reason } => {
                assert!(reason.contains("acknowledged"), "{reason}");
                assert!(reason.contains("Aware"), "{reason}");
            }
            other => panic!("expected Unverified, got {other:?}"),
        }
        assert!(!change.is_verified_transition());
    }

    /// The device moved, but not to what was asked.
    #[test]
    fn changed_to_a_different_mode_is_unverified() {
        let conn = prince(vec![status(1), processing(), status(2), status(2)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(2)).unwrap();
        assert_eq!(change.after, Some(2));
        match &change.verdict {
            Verdict::Unverified { reason } => assert!(reason.contains("custom slot 2"), "{reason}"),
            other => panic!("expected Unverified, got {other:?}"),
        }
    }

    // --- sent is not acknowledged -------------------------------------------

    /// No acknowledgement, but the read-back shows the mode changed: the
    /// device's own report is what counts.
    #[test]
    fn missing_ack_but_mode_changed_is_verified_and_says_ack_was_missing() {
        let conn = prince(vec![status(1), timeout(), status(0)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert!(matches!(change.ack, Some(Ack::Missing(_))));
        assert_eq!(change.verdict, Verdict::Verified);
    }

    #[test]
    fn missing_ack_and_no_change_is_unverified() {
        let conn = prince(vec![status(1), timeout(), status(1), status(1)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(2)).unwrap();
        match &change.verdict {
            Verdict::Unverified { reason } => {
                assert!(reason.contains("did not acknowledge"), "{reason}")
            }
            other => panic!("expected Unverified, got {other:?}"),
        }
    }

    #[test]
    fn readback_that_never_answers_is_unverified_with_the_reason() {
        let conn = prince(vec![status(1), processing(), timeout(), timeout()]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(2)).unwrap();
        assert_eq!(change.after, None);
        match &change.verdict {
            Verdict::Unverified { reason } => {
                assert!(reason.contains("could not be read back"), "{reason}")
            }
            other => panic!("expected Unverified, got {other:?}"),
        }
    }

    // --- ERROR --------------------------------------------------------------

    #[test]
    fn error_reply_is_rejected_with_the_device_reason() {
        // 15 = InvalidTransition in upstream's error table.
        let conn = prince(vec![status(1), error(15), status(1)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert!(change.sent);
        assert_eq!(change.after, Some(1), "state is still read back after a rejection");
        match &change.verdict {
            Verdict::Rejected { reason } => {
                assert!(reason.contains("InvalidTransition"), "{reason}")
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn auth_error_is_rejected_not_retried() {
        // 5 = OpNotSupp(auth).
        let conn = prince(vec![status(1), error(5), status(1)]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert!(matches!(change.verdict, Verdict::Rejected { .. }));
    }

    /// An ERROR on the baseline read means nothing is sent.
    #[test]
    fn error_on_the_baseline_read_sends_nothing() {
        let conn = prince(vec![error(4)]);
        let err = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap_err();
        assert!(matches!(err, BmapError::Device { .. }), "got {err:?}");
    }

    // --- channel failure ----------------------------------------------------

    #[test]
    fn channel_failure_during_the_switch_is_an_error() {
        let conn = prince(vec![
            status(1),
            Err(BmapError::Connection("The device closed the control channel".into())),
        ]);
        let err = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap_err();
        assert!(is_link_failure(&err));
    }

    /// A dead channel during read-back stops the polling instead of hammering
    /// a closed socket.
    #[test]
    fn channel_failure_during_readback_stops_polling() {
        let conn = prince(vec![
            status(1),
            processing(),
            Err(BmapError::Connection("closed".into())),
            status(0), // must never be consumed
        ]);
        let change = change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(5)).unwrap();
        assert!(matches!(change.verdict, Verdict::Unverified { .. }));
        assert_eq!(change.after, None);
    }

    // --- exact bytes --------------------------------------------------------

    /// The frames on the wire must be the documented ones, byte for byte.
    #[test]
    fn transmits_exactly_the_documented_frames() {
        let (conn, script) = shared(vec![status(1), processing(), status(0)]);
        change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(
            *script.sent.borrow(),
            vec![GET_MODE.to_vec(), START_QUIET.to_vec(), GET_MODE.to_vec()]
        );

        let (conn, script) = shared(vec![status(0), processing(), status(1)]);
        change_mode(&conn, Preset::Aware, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(
            *script.sent.borrow(),
            vec![GET_MODE.to_vec(), START_AWARE.to_vec(), GET_MODE.to_vec()]
        );
    }

    // --- custom slots -------------------------------------------------------

    const START_SLOT_2: [u8; 6] = [0x1F, 0x03, 0x05, 0x02, 0x02, 0x00];

    fn focus(index: u8) -> Target {
        Target::Slot {
            index,
            name: "Focus".into(),
        }
    }

    /// A custom mode is requested with the same frame as a preset, carrying
    /// its slot index, and confirmed the same way.
    #[test]
    fn a_custom_slot_is_requested_by_index_and_confirmed() {
        let (conn, script) = shared(vec![status(0), processing(), status(2)]);
        let change = change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();

        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_transition());
        assert_eq!((change.before, change.requested_index), (0, 2));
        assert_eq!(
            *script.sent.borrow(),
            vec![GET_MODE.to_vec(), START_SLOT_2.to_vec(), GET_MODE.to_vec()]
        );
    }

    /// A switch is answered with one frame and nothing after it, so it is
    /// sent as a request with one reply — never as one whose reply is
    /// gathered until the device has been silent for a while. That wait was
    /// half a second, on every switch into the custom mode and on no other.
    #[test]
    fn a_switch_is_never_sent_as_a_request_to_wait_out() {
        let (conn, script) = shared(vec![status(0), processing(), status(2)]);
        change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();
        assert_eq!(script.drains.get(), 0, "into the custom mode");

        let (conn, script) = shared(vec![status(2), processing(), status(0)]);
        change_mode(&conn, Preset::Quiet, VerifyPolicy::instant(3)).unwrap();
        assert_eq!(script.drains.get(), 0, "out of it, to a preset");
    }

    /// The indices that can ever be requested are the presets and the model's
    /// editable slots. Anything else is refused before the baseline is read.
    #[test]
    fn an_index_that_is_not_a_custom_slot_is_never_requested() {
        for index in [0, 1, 4, 7, 255] {
            let (conn, script) = shared(vec![status(0)]);
            let err = change_to(&conn, &focus(index), VerifyPolicy::instant(1)).unwrap_err();
            assert!(matches!(err, BmapError::Unsupported(_)), "index {index}: {err:?}");
            assert!(script.sent.borrow().is_empty(), "index {index}: something was sent");
        }
    }

    #[test]
    fn already_in_the_custom_slot_sends_nothing() {
        let (conn, script) = shared(vec![status(2)]);
        let change = change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!change.is_verified_transition());
        assert_eq!(*script.sent.borrow(), vec![GET_MODE.to_vec()]);
    }

    #[test]
    fn a_refused_slot_switch_is_rejected_with_the_devices_reason() {
        let (conn, _) = shared(vec![status(0), error(15), status(0)]);
        let change = change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();
        assert!(matches!(change.verdict, Verdict::Rejected { .. }), "{:?}", change.verdict);
        assert_eq!(change.after, Some(0));
    }

    /// Frames arrived, but none of them acknowledged the switch. The
    /// read-back decides, exactly as for a preset.
    #[test]
    fn a_slot_switch_without_an_acknowledgement_is_settled_by_the_readback() {
        let unrelated = Ok(vec![0x1F, 0x08, 0x03, 0x02, 0x04, 0x07]);
        let (conn, _) = shared(vec![status(0), unrelated, status(2)]);
        let change = change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(matches!(change.ack, Some(Ack::Missing(_))));

        let unrelated = Ok(vec![0x1F, 0x08, 0x03, 0x02, 0x04, 0x07]);
        let (conn, script) = shared(vec![status(0), unrelated, status(0), status(0), status(0)]);
        let change = change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();
        assert!(matches!(change.verdict, Verdict::Unverified { .. }));
        let starts = script.sent.borrow().iter().filter(|f| f[2] == 0x05).count();
        assert_eq!(starts, 1, "the request is never repeated");
    }

    /// The acknowledgement can arrive in the middle of other frames.
    #[test]
    fn the_acknowledgement_is_found_among_other_frames() {
        let mut burst = vec![0x1F, 0x08, 0x03, 0x02, 0x04, 0x07];
        burst.extend([0x1F, 0x03, 0x07, 0x00]); // PROCESSING
        burst.extend([0x1F, 0x03, 0x06, 0x01, 0x02]); // RESULT
        let (conn, _) = shared(vec![status(0), Ok(burst), status(2)]);
        let change = change_to(&conn, &focus(2), VerifyPolicy::instant(3)).unwrap();
        assert_eq!(change.ack, Some(Ack::Acknowledged));
        assert_eq!(change.verdict, Verdict::Verified);
    }
}
