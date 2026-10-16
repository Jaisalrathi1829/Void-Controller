//! Noise cancellation as a level, as the QuietComfort 35 generation has it.
//!
//! These headphones have no modes. They have one setting, `[1.6]`, whose
//! value is the level: off `0`, high `1`, wind `2`, low `3` — upstream's
//! description of the QC35, checked there against firmware 4.8.1. A status
//! from it carries the level and, after it, which levels the headphones have,
//! one bit for each (`based-connect` documents `0B`: off, high and low).
//!
//! **Not run here.** No QC35 has been available to this project. As
//! everywhere on the control link, a reply is read only if it has the
//! documented shape, the level is written once, and a change is called made
//! only when the headphones then report it.

use super::noise::{is_link_failure, Ack, ModeChange, Target, Verdict, VerifyPolicy};
use super::unit::exchange;
use bmap::error::{BmapError, BmapResult};
use bmap::protocol::Operator;
use bmap::Transport;

/// `[1.6]`: the noise-cancellation level. `GET` reads it, `SETGET` with the
/// level sets it.
pub const ANR: (u8, u8) = (1, 6);

/// A noise-cancellation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Off,
    High,
    Wind,
    Low,
}

impl Level {
    /// In the order they are offered: the most cancellation first.
    pub const SHOWN: [Level; 4] = [Level::High, Level::Low, Level::Wind, Level::Off];

    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Off),
            1 => Some(Self::High),
            2 => Some(Self::Wind),
            3 => Some(Self::Low),
            _ => None,
        }
    }

    /// The value the headphones use for it, which is also the index a
    /// switch to it asks for.
    pub fn byte(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::High => 1,
            Self::Wind => 2,
            Self::Low => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::High => "High",
            Self::Wind => "Wind",
            Self::Low => "Low",
        }
    }
}

/// What the headphones said about their noise cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnrStatus {
    pub level: Level,
    /// The levels they say they have, when they say. In [`Level::SHOWN`]
    /// order.
    pub supported: Option<Vec<Level>>,
}

impl AnrStatus {
    /// The levels to offer: the ones the headphones listed, or where they
    /// listed none, the three upstream's description has.
    pub fn offered(&self) -> Vec<Level> {
        self.supported
            .clone()
            .unwrap_or_else(|| vec![Level::High, Level::Low, Level::Off])
    }
}

fn unexpected(message: String) -> BmapError {
    BmapError::Device { message, code: 0 }
}

/// A `[1.6]` status: the level, then optionally which levels exist. Anything
/// else is not read.
pub fn parse(payload: &[u8]) -> BmapResult<AnrStatus> {
    let level_of = |byte: u8| {
        Level::from_byte(byte).ok_or_else(|| {
            unexpected(format!(
                "the noise-cancellation reply gave level {byte:#04x}, which is none of the \
                 documented ones"
            ))
        })
    };
    match payload {
        [level] => Ok(AnrStatus {
            level: level_of(*level)?,
            supported: None,
        }),
        [level, mask] => {
            let level = level_of(*level)?;
            let supported: Vec<Level> = Level::SHOWN
                .into_iter()
                .filter(|l| mask & (1 << l.byte()) != 0)
                .collect();
            if supported.is_empty() || mask >> 4 != 0 {
                return Err(unexpected(format!(
                    "the noise-cancellation reply listed levels {mask:#04x}, which is not a set \
                     of documented ones"
                )));
            }
            Ok(AnrStatus {
                level,
                supported: Some(supported),
            })
        }
        other => Err(unexpected(format!(
            "the noise-cancellation reply was {other:02x?} where a level and the levels there \
             are were documented"
        ))),
    }
}

/// Reads the noise-cancellation level.
pub fn read<T: Transport>(raw: &T) -> BmapResult<AnrStatus> {
    parse(&exchange(raw, ANR, Operator::Get, &[])?)
}

/// Sets the noise-cancellation level and confirms it from the headphones.
///
/// Read first; nothing is sent if they are at that level already, or if it is
/// not one they have. Then one `SETGET`, and read-backs until they report the
/// level or the policy runs out. The record is a [`ModeChange`] whose indices
/// are levels.
pub fn set_level<T: Transport>(
    raw: &T,
    requested: Level,
    policy: VerifyPolicy,
) -> BmapResult<ModeChange> {
    let before = read(raw)?;
    if !before.offered().contains(&requested) {
        return Err(BmapError::InvalidArg(format!(
            "these headphones do not list the {} level",
            requested.name()
        )));
    }

    let record = |sent, ack, after: Option<Level>, verdict| ModeChange {
        requested: Target::Listed {
            index: requested.byte(),
            name: requested.name().to_string(),
        },
        requested_index: requested.byte(),
        before: before.level.byte(),
        sent,
        ack,
        after: after.map(Level::byte),
        verdict,
    };

    if before.level == requested {
        return Ok(record(false, None, Some(requested), Verdict::AlreadyActive));
    }

    let ack = match exchange(raw, ANR, Operator::SetGet, &[requested.byte()]) {
        Ok(_) => Ack::Acknowledged,
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(record(
                true,
                None,
                read(raw).ok().map(|s| s.level),
                Verdict::Rejected {
                    reason: e.to_string(),
                },
            ));
        }
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the level was being set, so it is not known \
                 whether the headphones received it ({e})"
            )));
        }
        Err(e) => return Err(e),
    };

    let mut after: Option<Level> = None;
    let mut last_error: Option<String> = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read(raw) {
            Ok(now) => {
                after = Some(now.level);
                last_error = None;
                if now.level == requested {
                    break;
                }
            }
            Err(e) => {
                let gone = is_link_failure(&e);
                last_error = Some(e.to_string());
                if gone {
                    break;
                }
            }
        }
    }

    let acked = match &ack {
        Ack::Acknowledged => "The headphones acknowledged the change".to_string(),
        Ack::Missing(why) => format!("The headphones did not acknowledge the change ({why})"),
    };
    let verdict = match (after, &last_error) {
        (Some(now), _) if now == requested => Verdict::Verified,
        (Some(now), _) => Verdict::Unverified {
            reason: format!(
                "{acked}, but they report noise cancellation {} after {} read-back(s).",
                now.name(),
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
    /// What the headphones answer a level read with, the way `based-connect`
    /// documents it: the level, then the levels there are.
    pub fn anr_status(level: u8, mask: Option<u8>) -> Vec<u8> {
        match mask {
            Some(mask) => vec![0x01, 0x06, 0x03, 0x02, level, mask],
            None => vec![0x01, 0x06, 0x03, 0x01, level],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::anr_status;
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
        let r = f(&script);
        (r, script.sent.into_inner())
    }

    const OFF_HIGH_LOW: u8 = 0x0B;

    fn is_write(frame: &[u8]) -> bool {
        frame[2] & 0x0F == 2
    }

    #[test]
    fn a_level_and_the_levels_there_are_are_read() {
        let s = parse(&[1, OFF_HIGH_LOW]).unwrap();
        assert_eq!(s.level, Level::High);
        assert_eq!(s.supported, Some(vec![Level::High, Level::Low, Level::Off]));
        assert_eq!(s.offered(), vec![Level::High, Level::Low, Level::Off]);

        let bare = parse(&[3]).unwrap();
        assert_eq!(bare.level, Level::Low);
        assert_eq!(bare.offered(), vec![Level::High, Level::Low, Level::Off], "upstream's three");
    }

    #[test]
    fn anything_else_is_not_read() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[4]).is_err(), "no such level");
        assert!(parse(&[1, 0x00]).is_err(), "no levels at all");
        assert!(parse(&[1, 0x1B]).is_err(), "a level that is not documented");
        assert!(parse(&[1, 0x0B, 0x00]).is_err());
    }

    #[test]
    fn a_level_is_set_once_and_confirmed_by_reading_it_back() {
        let (change, sent) = run(
            vec![
                Ok(anr_status(1, Some(OFF_HIGH_LOW))),
                Ok(anr_status(3, Some(OFF_HIGH_LOW))),
                Ok(anr_status(3, Some(OFF_HIGH_LOW))),
            ],
            |raw| set_level(raw, Level::Low, VerifyPolicy::instant(3)).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert_eq!((change.before, change.after), (1, Some(3)));
        assert!(change.is_verified_transition());
        let writes: Vec<_> = sent.iter().filter(|f| is_write(f)).collect();
        assert_eq!(writes, vec![&vec![0x01, 0x06, 0x02, 0x01, 0x03]], "one SETGET with the level");
        assert_eq!(sent[0], vec![0x01, 0x06, 0x01, 0x00], "read first");
    }

    #[test]
    fn the_current_level_and_an_unlisted_one_send_nothing() {
        let (change, sent) = run(vec![Ok(anr_status(0, Some(OFF_HIGH_LOW)))], |raw| {
            set_level(raw, Level::Off, VerifyPolicy::instant(3)).unwrap()
        });
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(!sent.iter().any(|f| is_write(f)));

        let (result, sent) = run(vec![Ok(anr_status(1, Some(OFF_HIGH_LOW)))], |raw| {
            set_level(raw, Level::Wind, VerifyPolicy::instant(3))
        });
        assert!(matches!(result, Err(BmapError::InvalidArg(_))), "wind is not listed");
        assert!(!sent.iter().any(|f| is_write(f)));
    }

    #[test]
    fn a_level_the_headphones_do_not_report_is_not_called_set() {
        let (change, _) = run(
            vec![
                Ok(anr_status(1, None)),
                Ok(anr_status(1, None)),
                Ok(anr_status(1, None)),
                Ok(anr_status(1, None)),
            ],
            |raw| set_level(raw, Level::Off, VerifyPolicy::instant(2)).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Unverified { .. }), "{:?}", change.verdict);
        assert_eq!(change.after, Some(1));
    }

    #[test]
    fn a_refusal_is_reported_as_one() {
        let refused = Ok(vec![0x01, 0x06, 0x04, 0x01, 0x04]);
        let (change, _) = run(
            vec![Ok(anr_status(1, None)), refused, Ok(anr_status(1, None))],
            |raw| set_level(raw, Level::Low, VerifyPolicy::instant(2)).unwrap(),
        );
        assert!(matches!(change.verdict, Verdict::Rejected { .. }), "{:?}", change.verdict);
    }
}
