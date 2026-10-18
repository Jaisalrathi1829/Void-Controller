//! The touch shortcut, and what the headphones do when they are taken off.
//!
//! `[1.9]` is a button's gesture and the action it is set to. Upstream
//! describes it as `[button, gesture, action]` for the QC Ultra (2nd Gen) and
//! older models. The test headphones answer it too, with more after those
//! three bytes, as observed on 2026-10-17 (protocol-notes, Experiment 16):
//!
//! ```text
//! 80 09 03 | 00 01 40 08 | 00 00 00 80
//! button 0x80 "Shortcut", gesture 9 (press and hold), action 3 (battery level)
//!            | the actions the button takes, one bit each, most significant
//!            |  byte first: 3 battery level, 14 off, 16 Spotify Tap
//! ```
//!
//! Which actions a button takes is the device's to say, and only those are
//! offered. A device that does not say is not written to.
//!
//! `[1.24]` is auto play/pause — pausing when the headphones or earbuds are
//! taken off — and `[1.27]` auto-answer, each one byte, `0` or `1`. Upstream
//! describes both for the QC Ultra (2nd Gen) family; the test headphones have
//! neither (they answer `FuncNotSupp`).
//!
//! As everywhere on the control link: read in the observed shape only, write
//! once, and call a change made only when the device then reports it.

use super::noise::{is_link_failure, Ack, Verdict, VerifyPolicy};
use super::unit::exchange;
use bmap::error::{BmapError, BmapResult};
use bmap::protocol::Operator;
use bmap::Transport;

/// `[1.9]`: a button's gesture and its action.
pub const SHORTCUT: (u8, u8) = (1, 9);
/// `[1.24]`: auto play/pause when taken off.
pub const AUTO_PAUSE: (u8, u8) = (1, 24);
/// `[1.27]`: answer calls when put on.
pub const AUTO_ANSWER: (u8, u8) = (1, 27);

fn unexpected(message: String) -> BmapError {
    BmapError::Device { message, code: 0 }
}

/// A button's shortcut, as the device reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub button: u8,
    pub gesture: u8,
    pub action: u8,
    /// The actions the button takes, as the device listed them. `None` when
    /// it did not list them — and then nothing is written.
    pub offered: Option<Vec<u8>>,
}

/// What a button is called, by its id.
pub fn button_name(button: u8) -> &'static str {
    match button {
        0 => "Noise control button",
        2 => "Voice assistant button",
        3 => "Right earbud",
        4 => "Left earbud",
        16 => "Action button",
        128 => "Shortcut",
        _ => "Button",
    }
}

/// What a gesture is called, by its id.
pub fn gesture_name(gesture: u8) -> &'static str {
    match gesture {
        3 => "Short press",
        4 => "Single press",
        5 => "Press and hold",
        6 => "Double press",
        8 => "Triple press",
        9 => "Press and hold",
        10 => "Long press",
        _ => "Press",
    }
}

/// What an action is called, by its id. Upstream's list, worded as the
/// Bose app words them where it is known.
pub fn action_name(action: u8) -> &'static str {
    match action {
        0 => "Not set",
        1 => "Voice assistant",
        2 => "Noise cancelling",
        3 => "Battery level",
        4 => "Play / pause",
        5 => "More noise cancelling",
        6 => "Less noise cancelling",
        7 => "Wake word",
        8 => "Switch device",
        9 => "Conversation mode",
        10 => "Next track",
        11 => "Previous track",
        12 => "Notifications",
        13 => "Wind block",
        14 => "Off",
        15 => "App action",
        16 => "Spotify Tap",
        17 => "Switch modes",
        19 => "Immersive audio",
        20 => "Line in",
        21 => "Linking",
        _ => "Other action",
    }
}

/// A `[1.9]` status: button, gesture and action, then optionally the actions
/// the button takes as four bytes of bits, most significant first, and
/// whatever follows (not read).
pub fn parse_shortcut(payload: &[u8]) -> BmapResult<Shortcut> {
    match payload {
        [button, gesture, action] => Ok(Shortcut {
            button: *button,
            gesture: *gesture,
            action: *action,
            offered: None,
        }),
        [button, gesture, action, m0, m1, m2, m3, ..] => {
            let mask = u32::from_be_bytes([*m0, *m1, *m2, *m3]);
            let offered: Vec<u8> = (0..32u8).filter(|bit| mask & (1 << bit) != 0).collect();
            if offered.is_empty() {
                return Err(unexpected(
                    "the shortcut reply listed no actions for the button".to_string(),
                ));
            }
            Ok(Shortcut {
                button: *button,
                gesture: *gesture,
                action: *action,
                offered: Some(offered),
            })
        }
        other => Err(unexpected(format!(
            "the shortcut reply was {other:02x?} where a button, a gesture and an action were \
             documented"
        ))),
    }
}

pub fn read_shortcut<T: Transport>(raw: &T) -> BmapResult<Shortcut> {
    parse_shortcut(&exchange(raw, SHORTCUT, Operator::Get, &[])?)
}

/// One byte, `0` or `1`.
pub fn parse_flag(payload: &[u8]) -> BmapResult<bool> {
    match payload {
        [0] => Ok(false),
        [1] => Ok(true),
        other => Err(unexpected(format!(
            "the reply was {other:02x?} where 00 or 01 was documented"
        ))),
    }
}

pub fn read_flag<T: Transport>(raw: &T, addr: (u8, u8)) -> BmapResult<bool> {
    parse_flag(&exchange(raw, addr, Operator::Get, &[])?)
}

/// The record of one attempt to change a setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingChange<V> {
    pub requested: V,
    pub before: V,
    pub sent: bool,
    pub ack: Option<Ack>,
    pub after: Option<V>,
    pub verdict: Verdict,
}

impl<V: PartialEq> SettingChange<V> {
    pub fn is_verified_change(&self) -> bool {
        self.verdict == Verdict::Verified && self.before != self.requested
    }
}

/// Sends one `SETGET` and reads back until the device reports `requested`.
/// The pair is (what is asked for, what it was before).
fn write_and_confirm<T: Transport, V: Copy + PartialEq + std::fmt::Debug>(
    raw: &T,
    addr: (u8, u8),
    payload: &[u8],
    (requested, before): (V, V),
    read: impl Fn(&T) -> BmapResult<V>,
    policy: VerifyPolicy,
    what: &str,
) -> BmapResult<SettingChange<V>> {
    let record = |sent, ack, after, verdict| SettingChange {
        requested,
        before,
        sent,
        ack,
        after,
        verdict,
    };
    let ack = match exchange(raw, addr, Operator::SetGet, payload) {
        Ok(_) => Ack::Acknowledged,
        Err(BmapError::Timeout(msg)) => Ack::Missing(msg),
        Err(e @ (BmapError::Device { .. } | BmapError::Auth(_))) => {
            return Ok(record(
                true,
                None,
                read(raw).ok(),
                Verdict::Rejected {
                    reason: e.to_string(),
                },
            ));
        }
        Err(e) if is_link_failure(&e) => {
            return Err(BmapError::Connection(format!(
                "the connection failed while the {what} was being changed, so it is not known \
                 whether the device received it ({e})"
            )));
        }
        Err(e) => return Err(e),
    };

    let mut after = None;
    let mut last_error = None;
    for attempt in 0..policy.attempts.max(1) {
        if attempt > 0 && !policy.interval.is_zero() {
            std::thread::sleep(policy.interval);
        }
        match read(raw) {
            Ok(now) => {
                after = Some(now);
                last_error = None;
                if now == requested {
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
        Ack::Acknowledged => "The device acknowledged the change".to_string(),
        Ack::Missing(why) => format!("The device did not acknowledge the change ({why})"),
    };
    let verdict = match (after, &last_error) {
        (Some(now), _) if now == requested => Verdict::Verified,
        (Some(now), _) => Verdict::Unverified {
            reason: format!("{acked}, but it reports the {what} as {now:?} after reading back."),
        },
        (None, Some(err)) => Verdict::Unverified {
            reason: format!("{acked}, but the {what} could not be read back: {err}"),
        },
        (None, None) => Verdict::Unverified {
            reason: format!("{acked}, but no read-back was obtained."),
        },
    };
    Ok(record(true, Some(ack), after, verdict))
}

/// Sets the shortcut's action. Only an action the device listed for the
/// button is sent; a device that did not list them is not written to.
pub fn set_shortcut<T: Transport>(
    raw: &T,
    action: u8,
    policy: VerifyPolicy,
) -> BmapResult<SettingChange<u8>> {
    let before = read_shortcut(raw)?;
    let Some(offered) = &before.offered else {
        return Err(BmapError::InvalidArg(
            "the device does not say which actions the button takes".to_string(),
        ));
    };
    if !offered.contains(&action) {
        return Err(BmapError::InvalidArg(format!(
            "the device does not list {} for the button",
            action_name(action)
        )));
    }
    if before.action == action {
        return Ok(SettingChange {
            requested: action,
            before: action,
            sent: false,
            ack: None,
            after: Some(action),
            verdict: Verdict::AlreadyActive,
        });
    }
    write_and_confirm(
        raw,
        SHORTCUT,
        &[before.button, before.gesture, action],
        (action, before.action),
        |raw| read_shortcut(raw).map(|s| s.action),
        policy,
        "shortcut",
    )
}

/// Sets auto play/pause or auto-answer.
pub fn set_flag<T: Transport>(
    raw: &T,
    addr: (u8, u8),
    enabled: bool,
    policy: VerifyPolicy,
) -> BmapResult<SettingChange<bool>> {
    let before = read_flag(raw, addr)?;
    if before == enabled {
        return Ok(SettingChange {
            requested: enabled,
            before,
            sent: false,
            ack: None,
            after: Some(enabled),
            verdict: Verdict::AlreadyActive,
        });
    }
    write_and_confirm(
        raw,
        addr,
        &[u8::from(enabled)],
        (enabled, before),
        |raw| read_flag(raw, addr),
        policy,
        "setting",
    )
}

#[cfg(test)]
pub(crate) mod testing {
    /// The test headphones' shortcut reply, as observed, with the action
    /// replaced.
    pub fn shortcut_status(action: u8) -> Vec<u8> {
        vec![
            0x01, 0x09, 0x03, 0x0B, 0x80, 0x09, action, 0x00, 0x01, 0x40, 0x08, 0x00, 0x00, 0x00,
            0x80,
        ]
    }

    pub fn flag_status(addr: (u8, u8), on: bool) -> Vec<u8> {
        vec![addr.0, addr.1, 0x03, 0x01, u8::from(on)]
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
        let r = f(&script);
        (r, script.sent.into_inner())
    }

    fn writes(sent: &[Vec<u8>]) -> Vec<&Vec<u8>> {
        sent.iter().filter(|f| f[2] & 0x0F == 2).collect()
    }

    /// The reply recorded from the test headphones on 2026-10-17.
    #[test]
    fn the_test_headphones_shortcut_is_read() {
        let s = parse_shortcut(&[0x80, 0x09, 0x03, 0x00, 0x01, 0x40, 0x08, 0x00, 0x00, 0x00, 0x80])
            .unwrap();
        assert_eq!((s.button, s.gesture, s.action), (0x80, 9, 3));
        assert_eq!(s.offered, Some(vec![3, 14, 16]));
        assert_eq!(button_name(s.button), "Shortcut");
        assert_eq!(gesture_name(s.gesture), "Press and hold");
        let names: Vec<_> = s.offered.unwrap().into_iter().map(action_name).collect();
        assert_eq!(names, ["Battery level", "Off", "Spotify Tap"]);
    }

    #[test]
    fn other_shortcut_shapes() {
        let bare = parse_shortcut(&[0x80, 0x09, 0x03]).unwrap();
        assert_eq!(bare.offered, None);
        assert!(parse_shortcut(&[0x80, 0x09]).is_err());
        assert!(parse_shortcut(&[0x80, 0x09, 0x03, 0, 0, 0, 0]).is_err(), "no actions listed");
    }

    #[test]
    fn an_offered_action_is_set_once_and_confirmed() {
        let (change, sent) = run(
            vec![Ok(shortcut_status(3)), Ok(shortcut_status(16)), Ok(shortcut_status(16))],
            |raw| set_shortcut(raw, 16, VerifyPolicy::instant(3)).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert!(change.is_verified_change());
        assert_eq!(writes(&sent), vec![&vec![0x01, 0x09, 0x02, 0x03, 0x80, 0x09, 16]]);
    }

    #[test]
    fn an_action_not_offered_or_already_set_sends_nothing() {
        let (result, sent) = run(vec![Ok(shortcut_status(3))], |raw| {
            set_shortcut(raw, 1, VerifyPolicy::instant(3))
        });
        assert!(matches!(result, Err(BmapError::InvalidArg(_))), "voice assistant not listed");
        assert!(writes(&sent).is_empty());

        let (change, sent) = run(vec![Ok(shortcut_status(3))], |raw| {
            set_shortcut(raw, 3, VerifyPolicy::instant(3)).unwrap()
        });
        assert_eq!(change.verdict, Verdict::AlreadyActive);
        assert!(writes(&sent).is_empty());
    }

    #[test]
    fn a_device_that_does_not_list_its_actions_is_not_written() {
        let (result, sent) = run(vec![Ok(vec![0x01, 0x09, 0x03, 0x03, 0x80, 0x09, 0x03])], |raw| {
            set_shortcut(raw, 16, VerifyPolicy::instant(3))
        });
        assert!(matches!(result, Err(BmapError::InvalidArg(_))));
        assert!(writes(&sent).is_empty());
    }

    #[test]
    fn a_flag_is_read_set_and_confirmed() {
        assert!(parse_flag(&[1]).unwrap());
        assert!(!parse_flag(&[0]).unwrap());
        assert!(parse_flag(&[2]).is_err());
        assert!(parse_flag(&[1, 0]).is_err());

        let (change, sent) = run(
            vec![
                Ok(flag_status(AUTO_PAUSE, false)),
                Ok(flag_status(AUTO_PAUSE, true)),
                Ok(flag_status(AUTO_PAUSE, true)),
            ],
            |raw| set_flag(raw, AUTO_PAUSE, true, VerifyPolicy::instant(3)).unwrap(),
        );
        assert_eq!(change.verdict, Verdict::Verified);
        assert_eq!(writes(&sent), vec![&vec![0x01, 0x18, 0x02, 0x01, 0x01]]);
    }

    #[test]
    fn a_device_without_the_function_says_so() {
        let refused = Ok(vec![0x01, 0x18, 0x04, 0x01, 0x04]);
        let (result, _) = run(vec![refused], |raw| read_flag(raw, AUTO_PAUSE));
        assert!(matches!(result, Err(BmapError::Device { code: 4, .. })));
    }
}
