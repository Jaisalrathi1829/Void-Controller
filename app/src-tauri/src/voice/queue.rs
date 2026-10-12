//! The line of things waiting to be said.
//!
//! One announcement is spoken at a time, so anything that arrives while
//! another is being synthesized or played waits here. The rules for the wait
//! are fixed and small:
//!
//! * **Order.** Lowest [`priority`] first; among equals, first come first.
//! * **One per subject.** A newer announcement about the same subject replaces
//!   the one still waiting — the latest mode, the latest connection state, the
//!   latest battery level. So the line cannot grow with repeated events: at
//!   most one of each kind is ever waiting.
//! * **A disconnection clears what it makes untrue.** A mode change or battery
//!   warning for headphones that have since gone is dropped, unsaid.
//! * **Bounded anyway.** The line never holds more than [`MAX_PENDING`]. If it
//!   were ever full, the least important item gives way, and a newcomer that
//!   is itself the least important is turned away.
//!
//! [`priority`]: Announcement::priority

use super::policy::Announcement;

/// The most that can wait at once. The one-per-subject rule keeps the line
/// well short of this; the bound is there so that staying short is a property
/// of the queue and not of every caller behaving.
pub const MAX_PENDING: usize = 8;

/// What became of a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    /// Waiting to be said.
    Queued,
    /// Exactly this is already waiting.
    AlreadyWaiting,
    /// Waiting, in place of an older announcement about the same subject.
    Replaced,
    /// Not taken: the line is full of more important things.
    Refused,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// In arrival order.
    waiting: Vec<Announcement>,
}

/// Whether two announcements are about the same subject, so that the newer
/// makes the older pointless.
fn same_subject(a: &Announcement, b: &Announcement) -> bool {
    use Announcement::*;
    matches!(
        (a, b),
        (Mode(_), Mode(_))
            | (LowBattery { .. }, LowBattery { .. })
            | (Test, Test)
            | (Connected { .. } | Disconnected { .. }, Connected { .. } | Disconnected { .. })
    )
}

impl Queue {
    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    pub fn push(&mut self, new: Announcement) -> Pushed {
        if self.waiting.contains(&new) {
            return Pushed::AlreadyWaiting;
        }

        // Headphones that have gone make everything waiting to be said about
        // them untrue.
        if matches!(new, Announcement::Disconnected { .. }) {
            self.waiting
                .retain(|a| !matches!(a, Announcement::Mode(_) | Announcement::LowBattery { .. }));
        }

        let before = self.waiting.len();
        self.waiting.retain(|a| !same_subject(a, &new));
        let replaced = self.waiting.len() < before;

        if self.waiting.len() >= MAX_PENDING {
            // Full. The least important waiting item — the most recent of
            // them, if several tie — gives way, unless the newcomer is no
            // more important than it.
            let (index, least) = self
                .waiting
                .iter()
                .enumerate()
                .max_by_key(|(i, a)| (a.priority(), *i))
                .expect("a full queue is not empty");
            if new.priority() >= least.priority() {
                return Pushed::Refused;
            }
            self.waiting.remove(index);
        }

        self.waiting.push(new);
        if replaced {
            Pushed::Replaced
        } else {
            Pushed::Queued
        }
    }

    /// Takes the next announcement to say.
    pub fn pop(&mut self) -> Option<Announcement> {
        let index = self
            .waiting
            .iter()
            .enumerate()
            .min_by_key(|(i, a)| (a.priority(), *i))
            .map(|(i, _)| i)?;
        Some(self.waiting.remove(index))
    }

    /// Drops everything the application decided to say on its own, leaving a
    /// pending Test Voice request. Returns how many were dropped.
    pub fn clear_automatic(&mut self) -> usize {
        let before = self.waiting.len();
        self.waiting.retain(|a| !a.is_automatic());
        before - self.waiting.len()
    }

    /// Drops everything.
    pub fn clear(&mut self) -> usize {
        let n = self.waiting.len();
        self.waiting.clear();
        n
    }

    /// Drops waiting announcements that fail `keep`.
    pub fn retain(&mut self, keep: impl FnMut(&Announcement) -> bool) {
        self.waiting.retain(keep);
    }
}

#[cfg(test)]
mod tests {
    use super::super::policy::SpokenMode;
    use super::*;

    fn quiet() -> Announcement {
        Announcement::Mode(SpokenMode::Quiet)
    }
    fn aware() -> Announcement {
        Announcement::Mode(SpokenMode::Aware)
    }
    fn connected() -> Announcement {
        Announcement::Connected { name: "Aurora".into(), host: None }
    }
    fn disconnected() -> Announcement {
        Announcement::Disconnected { name: "Aurora".into() }
    }
    fn battery(percent: u8) -> Announcement {
        Announcement::LowBattery { percent }
    }
    fn drain(q: &mut Queue) -> Vec<Announcement> {
        std::iter::from_fn(|| q.pop()).collect()
    }

    #[test]
    fn the_most_important_is_said_first_whatever_order_things_arrived_in() {
        let mut q = Queue::default();
        q.push(Announcement::Test);
        q.push(battery(15));
        q.push(quiet());
        q.push(connected());
        assert_eq!(
            drain(&mut q),
            vec![connected(), quiet(), battery(15), Announcement::Test]
        );
    }

    /// A battery warning that arrived first does not hold up a disconnection.
    #[test]
    fn a_disconnection_is_not_delayed_by_anything_already_waiting() {
        let mut q = Queue::default();
        q.push(Announcement::Test);
        q.push(disconnected());
        assert_eq!(q.pop(), Some(disconnected()));
    }

    #[test]
    fn the_same_announcement_twice_waits_once() {
        let mut q = Queue::default();
        assert_eq!(q.push(quiet()), Pushed::Queued);
        assert_eq!(q.push(quiet()), Pushed::AlreadyWaiting);
        assert_eq!(q.push(connected()), Pushed::Queued);
        assert_eq!(q.push(connected()), Pushed::AlreadyWaiting);
        assert_eq!(q.len(), 2);
    }

    /// Quiet, then Aware, before either was spoken: only Aware is true now.
    #[test]
    fn a_newer_mode_replaces_the_one_still_waiting() {
        let mut q = Queue::default();
        q.push(quiet());
        assert_eq!(q.push(aware()), Pushed::Replaced);
        assert_eq!(drain(&mut q), vec![aware()]);
    }

    #[test]
    fn a_newer_battery_level_replaces_the_one_still_waiting() {
        let mut q = Queue::default();
        q.push(battery(20));
        assert_eq!(q.push(battery(17)), Pushed::Replaced);
        assert_eq!(drain(&mut q), vec![battery(17)]);
    }

    /// Dropped and back again before anything was said: only where things
    /// ended up is announced, not the history.
    #[test]
    fn only_the_latest_connection_state_waits() {
        let mut q = Queue::default();
        q.push(disconnected());
        assert_eq!(q.push(connected()), Pushed::Replaced);
        assert_eq!(drain(&mut q), vec![connected()]);

        q.push(connected());
        q.push(disconnected());
        assert_eq!(drain(&mut q), vec![disconnected()]);
    }

    #[test]
    fn a_disconnection_drops_what_it_makes_untrue() {
        let mut q = Queue::default();
        q.push(quiet());
        q.push(battery(12));
        q.push(Announcement::Test);
        q.push(disconnected());
        // The mode and the battery warning were about headphones that are no
        // longer there. A test request is the user's, and stays.
        assert_eq!(drain(&mut q), vec![disconnected(), Announcement::Test]);
    }

    /// However many events arrive, at most one of each kind waits.
    #[test]
    fn a_flood_of_events_cannot_grow_the_line() {
        let mut q = Queue::default();
        for i in 0..500u32 {
            q.push(if i % 2 == 0 { quiet() } else { aware() });
            q.push(battery((i % 20) as u8));
            q.push(if i % 3 == 0 { connected() } else { disconnected() });
            q.push(Announcement::Test);
            assert!(q.len() <= 4, "{} waiting after {i} rounds", q.len());
        }
        assert!(q.len() <= MAX_PENDING);
    }

    /// The bound itself, exercised directly: the one-per-subject rule never
    /// lets real traffic reach it, so the line is filled by hand here.
    #[test]
    fn a_full_line_keeps_what_matters_most() {
        let custom = |i: usize| Announcement::Mode(SpokenMode::Custom(Some(format!("m{i}"))));
        let mut q = Queue {
            waiting: (0..MAX_PENDING).map(custom).collect(),
        };

        // No more important than what is waiting: turned away.
        assert_eq!(q.push(Announcement::Test), Pushed::Refused);
        assert_eq!(q.len(), MAX_PENDING);

        // More important: taken, and the newest of the least important goes.
        assert_eq!(q.push(connected()), Pushed::Queued);
        assert_eq!(q.len(), MAX_PENDING);
        assert_eq!(q.pop(), Some(connected()));
        assert!(!q.waiting.contains(&custom(MAX_PENDING - 1)));
        assert!(q.waiting.contains(&custom(0)));
    }

    #[test]
    fn switching_voice_feedback_off_drops_what_was_automatic_and_keeps_a_test() {
        let mut q = Queue::default();
        q.push(quiet());
        q.push(battery(10));
        q.push(connected());
        q.push(Announcement::Test);
        assert_eq!(q.clear_automatic(), 3);
        assert_eq!(drain(&mut q), vec![Announcement::Test]);
    }

    #[test]
    fn stop_speaking_drops_everything() {
        let mut q = Queue::default();
        q.push(quiet());
        q.push(Announcement::Test);
        assert_eq!(q.clear(), 2);
        assert!(q.is_empty());
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn stale_announcements_can_be_weeded_out() {
        let mut q = Queue::default();
        q.push(quiet());
        q.push(battery(10));
        q.retain(|a| !matches!(a, Announcement::Mode(_)));
        assert_eq!(drain(&mut q), vec![battery(10)]);
    }
}
