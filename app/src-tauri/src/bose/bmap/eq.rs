//! The hardware equalizer: three bands, read from and written to the
//! headphones themselves.
//!
//! Upstream's `eq()` and `set_eq()` do the reading and writing. This module
//! adds what the rest of the control link adds everywhere: a reply is only
//! trusted if it has exactly the documented shape, and a change is only called
//! applied when the headphones then report it.

use super::noise::{is_link_failure, Ack, Verdict, VerifyPolicy};
use bmap::error::{BmapError, BmapResult};
use bmap::{BmapConnection, Transport};

/// Every band runs from -10 to +10 on the models checked here.
pub const MIN_GAIN: i8 = -10;
pub const MAX_GAIN: i8 = 10;

/// The three band gains, as the headphones report them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EqLevels {
    pub bass: i8,
    pub mid: i8,
    pub treble: i8,
}

impl EqLevels {
    pub fn in_range(&self) -> bool {
        [self.bass, self.mid, self.treble]
            .iter()
            .all(|g| (MIN_GAIN..=MAX_GAIN).contains(g))
    }

    pub fn describe(&self) -> String {
        format!("bass {:+}, mid {:+}, treble {:+}", self.bass, self.mid, self.treble)
    }
}

fn unexpected(message: String) -> BmapError {
    BmapError::Device { message, code: 0 }
}

/// Reads the equalizer.
///
/// The reply must be exactly three bands — bass, mid, treble, once each —
/// each with the documented -10..+10 range and a value inside it. Anything
/// else is a layout that has not been described, and is refused rather than
/// read loosely.
pub fn read_eq<T: Transport>(conn: &BmapConnection<T>) -> BmapResult<EqLevels> {
    let bands = conn.eq()?;
    if bands.len() != 3 {
        return Err(unexpected(format!(
            "the equalizer reported {} band(s) where 3 are documented",
            bands.len()
        )));
    }

    let mut gains = [None::<i8>; 3];
    for band in &bands {
        let slot = gains.get_mut(band.band_id as usize).ok_or_else(|| {
            unexpected(format!("the equalizer reported an unknown band {}", band.band_id))
        })?;
        if slot.is_some() {
            return Err(unexpected(format!(
                "the equalizer reported band {} twice",
                band.band_id
            )));
        }
        if (band.min_val, band.max_val) != (MIN_GAIN, MAX_GAIN) {
            return Err(unexpected(format!(
                "the equalizer reported a range of {}..{} for {} where {MIN_GAIN}..{MAX_GAIN} is \
                 documented",
                band.min_val, band.max_val, band.name
            )));
        }
        if !(MIN_GAIN..=MAX_GAIN).contains(&band.current) {
            return Err(unexpected(format!(
                "the equalizer reported {} for {}, outside its own range",
                band.current, band.name
            )));
        }
        *slot = Some(band.current);
    }

    match gains {
        [Some(bass), Some(mid), Some(treble)] => Ok(EqLevels { bass, mid, treble }),
        _ => Err(unexpected("the equalizer did not report all three bands".into())),
    }
}

/// The full record of one attempt to change the equalizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqChange {
    pub requested: EqLevels,
    /// What the headphones reported before anything was sent.
    pub before: EqLevels,
    pub sent: bool,
    pub ack: Option<Ack>,
    /// The most recent successful read-back, if any.
    pub after: Option<EqLevels>,
    pub verdict: Verdict,
}

impl EqChange {
    /// True only for a change the device itself confirmed.
    pub fn is_verified_change(&self) -> bool {
        self.verdict == Verdict::Verified && self.before != self.requested
    }
}

/// Sets the three bands and confirms them by reading the equalizer back.
///
/// Returns `Err` when nothing meaningful can be reported — the baseline could
/// not be read, so nothing was sent, or the channel failed outright. Every
/// case in which a write went out produces an [`EqChange`].
pub fn set_eq<T: Transport>(
    conn: &BmapConnection<T>,
    requested: EqLevels,
    policy: VerifyPolicy,
) -> BmapResult<EqChange> {
    if !requested.in_range() {
        return Err(BmapError::InvalidArg(format!(
            "EQ gains must be {MIN_GAIN}..{MAX_GAIN}, got {}",
            requested.describe()
        )));
    }

    // Baseline, and proof that the reply has the documented shape, before
    // anything is written.
    let before = read_eq(conn)?;

    let record = |sent, ack, after, verdict| EqChange {
        requested,
        before,
        sent,
        ack,
        after,
        verdict,
    };

    if before == requested {
        return Ok(record(false, None, Some(before), Verdict::AlreadyActive));
    }

    let ack = match conn.set_eq(requested.bass, requested.mid, requested.treble) {
        Ok(()) => Ack::Acknowledged,
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        // The device said no to at least one band. Read back so the caller
        // learns what it holds now — some bands may have been accepted.
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(record(
                true,
                None,
                read_eq(conn).ok(),
                Verdict::Rejected {
                    reason: e.to_string(),
                },
            ));
        }
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the equalizer was being written, so it is not \
                 known how much of the change the headphones received ({e})"
            )));
        }
        Err(e) => return Err(e),
    };

    let mut after: Option<EqLevels> = None;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read_eq(conn) {
            Ok(now) => {
                after = Some(now);
                last_error = None;
                if now == requested {
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
    let verdict = match (after, &last_error) {
        (Some(now), _) if now == requested => Verdict::Verified,
        (Some(now), _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but the headphones report {} after {} read-back(s).",
                now.describe(),
                policy.attempts.max(1)
            ),
        },
        (None, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but the equalizer could not be read back: {err}"),
        },
        (None, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };

    Ok(record(true, Some(ack), after, verdict))
}

#[cfg(test)]
pub(crate) mod testing {
    /// The frame a device answers an equalizer read with: one
    /// `[min, max, current, band]` group per band.
    pub fn eq_status(bass: i8, mid: i8, treble: i8) -> Vec<u8> {
        let mut f = vec![0x01, 0x07, 0x03, 12];
        for (id, gain) in [(0u8, bass), (1, mid), (2, treble)] {
            f.extend([(-10i8) as u8, 10, gain as u8, id]);
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::testing::eq_status;
    use super::*;
    use bmap::devices;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

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

    fn run<R>(
        replies: Vec<BmapResult<Vec<u8>>>,
        f: impl FnOnce(&BmapConnection<Script>) -> R,
    ) -> (R, Vec<Vec<u8>>) {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let conn = BmapConnection::new(
            Script {
                replies: RefCell::new(replies.into()),
                sent: sent.clone(),
            },
            devices::qc_prince(),
        );
        let result = f(&conn);
        let frames = sent.borrow().clone();
        (result, frames)
    }

    const GET_EQ: [u8; 4] = [0x01, 0x07, 0x01, 0x00];

    fn eq(bass: i8, mid: i8, treble: i8) -> BmapResult<Vec<u8>> {
        Ok(eq_status(bass, mid, treble))
    }
    fn levels(bass: i8, mid: i8, treble: i8) -> EqLevels {
        EqLevels { bass, mid, treble }
    }
    fn is_write(frame: &[u8]) -> bool {
        frame[0] == 0x01 && frame[1] == 0x07 && frame[2] & 0x0F == 0x02
    }
    fn policy() -> VerifyPolicy {
        VerifyPolicy::instant(3)
    }

    // --- reading ------------------------------------------------------------

    #[test]
    fn reads_three_signed_bands() {
        let (read, frames) = run(vec![eq(6, -3, 0)], |c| read_eq(c).unwrap());
        assert_eq!(read, levels(6, -3, 0));
        assert_eq!(frames, vec![GET_EQ.to_vec()], "one GET and nothing else");
    }

    #[test]
    fn band_order_in_the_reply_does_not_matter() {
        let mut f = vec![0x01, 0x07, 0x03, 12];
        for (id, gain) in [(2u8, 5i8), (0, -4), (1, 1)] {
            f.extend([(-10i8) as u8, 10, gain as u8, id]);
        }
        let (read, _) = run(vec![Ok(f)], |c| read_eq(c).unwrap());
        assert_eq!(read, levels(-4, 1, 5));
    }

    #[test]
    fn a_reply_of_any_other_shape_is_refused() {
        // Two bands only.
        let mut two = vec![0x01, 0x07, 0x03, 8];
        two.extend([0xF6, 10, 0, 0, 0xF6, 10, 0, 1]);
        // A band this build has no name for.
        let mut unknown = eq_status(0, 0, 0);
        unknown[4 + 11] = 7;
        // The same band twice.
        let mut twice = eq_status(0, 0, 0);
        twice[4 + 7] = 0;
        // A different range.
        let mut range = eq_status(0, 0, 0);
        range[4 + 1] = 12;
        // A value outside its own range.
        let mut wild = eq_status(0, 0, 0);
        wild[4 + 2] = 50;

        for (what, frame) in [
            ("two bands", two),
            ("unknown band", unknown),
            ("duplicate band", twice),
            ("other range", range),
            ("out-of-range value", wild),
        ] {
            let (result, _) = run(vec![Ok(frame)], |c| read_eq(c));
            assert!(
                matches!(result, Err(BmapError::Device { .. })),
                "{what}: {result:?}"
            );
        }
    }

    #[test]
    fn a_device_without_the_function_reports_its_error() {
        let (result, _) = run(vec![Ok(vec![0x01, 0x07, 0x04, 0x01, 0x03])], |c| read_eq(c));
        assert!(matches!(result, Err(BmapError::Device { code: 3, .. })));
    }

    // --- writing ------------------------------------------------------------

    fn ack(band: u8, gain: i8) -> BmapResult<Vec<u8>> {
        Ok(vec![0x01, 0x07, 0x03, 4, 0xF6, 10, gain as u8, band])
    }

    #[test]
    fn a_confirmed_change_writes_each_band_in_the_documented_form() {
        let (change, frames) = run(
            vec![eq(0, 0, 0), ack(0, 4), ack(1, -2), ack(2, 3), eq(4, -2, 3)],
            |c| set_eq(c, levels(4, -2, 3), policy()).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_change());
        assert_eq!(change.before, levels(0, 0, 0));
        assert_eq!(change.after, Some(levels(4, -2, 3)));

        let writes: Vec<_> = frames.iter().filter(|f| is_write(f)).cloned().collect();
        assert_eq!(
            writes,
            vec![
                vec![0x01, 0x07, 0x02, 0x02, 4, 0],
                vec![0x01, 0x07, 0x02, 0x02, 0xFE, 1], // -2 as a signed byte
                vec![0x01, 0x07, 0x02, 0x02, 3, 2],
            ]
        );
        assert_eq!(frames.first().unwrap(), &GET_EQ.to_vec(), "baseline first");
    }

    #[test]
    fn asking_for_what_it_already_has_sends_nothing() {
        let (change, frames) = run(vec![eq(4, -2, 3)], |c| {
            set_eq(c, levels(4, -2, 3), policy()).unwrap()
        });
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!change.sent && !change.is_verified_change());
        assert!(!frames.iter().any(|f| is_write(f)));
    }

    #[test]
    fn out_of_range_gains_are_refused_before_anything_is_sent() {
        for bad in [levels(11, 0, 0), levels(0, -11, 0), levels(0, 0, 127)] {
            let (result, frames) = run(vec![], |c| set_eq(c, bad, policy()));
            assert!(matches!(result, Err(BmapError::InvalidArg(_))));
            assert!(frames.is_empty());
        }
    }

    #[test]
    fn an_unreadable_or_odd_baseline_sends_nothing() {
        let mut odd = eq_status(0, 0, 0);
        odd[4 + 1] = 12;
        for baseline in [Err(BmapError::Timeout("No reply".into())), Ok(odd)] {
            let (result, frames) = run(vec![baseline], |c| set_eq(c, levels(1, 1, 1), policy()));
            assert!(result.is_err());
            assert!(!frames.iter().any(|f| is_write(f)));
        }
    }

    #[test]
    fn acknowledged_but_unchanged_is_unverified() {
        let (change, frames) = run(
            vec![
                eq(0, 0, 0),
                ack(0, 4),
                ack(1, 0),
                ack(2, 0),
                eq(0, 0, 0),
                eq(0, 0, 0),
                eq(0, 0, 0),
            ],
            |c| set_eq(c, levels(4, 0, 0), policy()).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }));
        assert_eq!(change.after, Some(levels(0, 0, 0)), "the device's own report");
        assert_eq!(frames.iter().filter(|f| is_write(f)).count(), 3, "never re-sent");
    }

    /// One band taking and another not is not a success, and the read-back
    /// says exactly where things stand.
    #[test]
    fn a_partly_applied_change_is_unverified_and_reports_the_real_state() {
        let (change, _) = run(
            vec![
                eq(0, 0, 0),
                ack(0, 4),
                ack(1, 0),
                ack(2, 0),
                eq(4, 0, 0),
                eq(4, 0, 0),
                eq(4, 0, 0),
            ],
            |c| set_eq(c, levels(4, 2, 0), policy()).unwrap(),
        );
        let Verdict::Unverified { reason } = &change.verdict else {
            panic!("expected Unverified");
        };
        assert!(reason.contains("bass +4, mid +0"), "{reason}");
        assert_eq!(change.after, Some(levels(4, 0, 0)));
    }

    #[test]
    fn a_refused_band_is_rejected_and_the_state_is_read_back() {
        let refuse = Ok(vec![0x01, 0x07, 0x04, 0x01, 0x02]);
        let (change, frames) = run(vec![eq(0, 0, 0), ack(0, 4), refuse, eq(4, 0, 0)], |c| {
            set_eq(c, levels(4, 2, 0), policy()).unwrap()
        });
        assert!(matches!(change.verdict, Verdict::Rejected { .. }));
        assert_eq!(change.after, Some(levels(4, 0, 0)), "bass did go through");
        // Upstream stops at the refusal: treble is never sent.
        assert_eq!(frames.iter().filter(|f| is_write(f)).count(), 2);
    }

    #[test]
    fn a_channel_failure_during_the_write_says_the_outcome_is_unknown() {
        let (result, _) = run(
            vec![eq(0, 0, 0), ack(0, 4), Err(BmapError::Connection("closed".into()))],
            |c| set_eq(c, levels(4, 2, 0), policy()),
        );
        let err = result.unwrap_err();
        assert!(matches!(err, BmapError::Connection(_)));
        assert!(err.to_string().contains("not known how much"), "{err}");
    }
}
