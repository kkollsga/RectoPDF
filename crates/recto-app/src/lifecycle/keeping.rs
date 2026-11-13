//! **Keeping the record when its lock comes late** (owner, 2026-11-13: "A window that doesn't get the record's lock at its start tries again quietly for about
//! ten seconds and starts remembering once it has it; another RectoPDF really keeping the state is said as it is today").
//!
//! A launch asks for the record's lock as it reads the state, and waits up to the store's patience for it (600 ms, `record_store::LOCK_PATIENCE`). One that still
//! hasn't got it says nothing yet, and tries again: one `try_lock` every [`TRY_EVERY_MS`] from the moment the state was taken in, the last at [`KEEP_TRYING_MS`],
//! so [`TRIES`] tries in all, each on the session thread's own deadline ([`Keeping::due`]): no thread of its own, no timer, and nothing at all once it's settled.
//! Then one of three:
//! - **The lock comes, and the person has done nothing in the window:** this copy keeps the record from here, as if it had got it at the start, and the saved
//!   session is read and restored as a launch would have ([`Out::Restore`]). The notice never shows. The common case: the copy before it was still quitting, or
//!   an update's hand-off was letting go.
//! - **The person opens or changes something first:** trying stops at once, and the window is one that won't be remembered, said at once ([`Out::SayNotKept`]):
//!   they've started working in it, and should know. Restoring now would replace what they have, and keeping the record with their window would write over the
//!   saved session, so neither is done (the coordinator's choice on 2026-11-13, until the saved session can be added beside theirs).
//! - **The tries run out:** another RectoPDF really is keeping it, said as today ([`Out::SayNotKept`]), and nothing's tried again.
//!
//! "Done something" is what the window has, as one number ([`Event::Read`]'s `as_read`): the session as it would be recorded and the app's settings a person
//! chooses, so anything a person can do that a restore would replace counts, and nothing the host does by itself does.
//!
//! Pure: the state, an event and `now` in milliseconds of the app's clock. The app holds it in [`super::Life`] and carries out the [`Out`]
//! (`core::keeping`).

use std::time::Duration;

/// How long a launch goes on trying for the lock after the state's been taken in without it.
pub const KEEP_TRYING_MS: u64 = 10_000;
/// The time between tries.
pub const TRY_EVERY_MS: u64 = 1_000;
/// How many tries in all: one a second, the last at ten seconds.
pub const TRIES: u32 = (KEEP_TRYING_MS / TRY_EVERY_MS) as u32;

/// Whether this copy keeps the record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keeping {
    /// The state hasn't been taken in yet.
    #[default]
    NotAsked,
    /// It keeps the record: from the start, or since a late try.
    Keeps,
    /// Trying again, quietly: since when, how many tries so far, and what the window had when the state was taken in.
    Trying { since_ms: u64, tries: u32, as_read: u64 },
    /// It doesn't, and that's been said: the tries ran out, or the person started working in the window first. For good.
    NotKept,
}

/// What happens to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The state's been taken in for the first time, with the lock or without it. `as_read` is what the window had then.
    Read { keeps: bool, as_read: u64 },
    /// A call's been taken on the session thread: what the window has now.
    Looked { now_as: u64 },
    /// A try at the lock, made because [`Keeping::due`] said so, with nothing taken between the [`Event::Looked`] before it and this.
    Tried { got: bool },
}

/// What the app does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// It keeps the record now: the saved session is read and restored, as at a launch.
    Restore,
    /// The window won't be remembered: the notice is shown, in today's words.
    SayNotKept,
}

impl Keeping {
    /// **The one transition.**
    #[must_use]
    pub fn step(&mut self, event: Event, now_ms: u64) -> Option<Out> {
        match (*self, event) {
            (Self::NotAsked, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                None
            }
            (Self::NotAsked, Event::Read { keeps: false, as_read }) => {
                *self = Self::Trying {
                    since_ms: now_ms,
                    tries: 0,
                    as_read,
                };
                None
            }
            (Self::Trying { as_read, .. }, Event::Looked { now_as }) if now_as != as_read => {
                *self = Self::NotKept;
                Some(Out::SayNotKept)
            }
            (Self::Trying { .. }, Event::Tried { got: true }) => {
                *self = Self::Keeps;
                Some(Out::Restore)
            }
            (Self::Trying { since_ms, tries, as_read }, Event::Tried { got: false }) => {
                let tries = tries + 1;
                if tries >= TRIES {
                    *self = Self::NotKept;
                    return Some(Out::SayNotKept);
                }
                *self = Self::Trying { since_ms, tries, as_read };
                None
            }
            // A second state taken in (`session restore`), a look at a window as it was, or a try that comes when nothing's trying: nothing changes.
            _ => None,
        }
    }

    /// How long until the next try, zero once it's due; `None` when nothing's trying.
    #[must_use]
    pub fn due(&self, now_ms: u64) -> Option<Duration> {
        let Self::Trying { since_ms, tries, .. } = *self else { return None };
        let at = since_ms.saturating_add(TRY_EVERY_MS.saturating_mul(u64::from(tries) + 1));
        Some(Duration::from_millis(at.saturating_sub(now_ms)))
    }

    /// Whether it's still trying.
    #[must_use]
    pub fn trying(&self) -> bool {
        matches!(self, Self::Trying { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trying_from(now: u64) -> Keeping {
        let mut keeping = Keeping::default();
        assert_eq!(keeping.step(Event::Read { keeps: false, as_read: 7 }, now), None, "nothing said while it tries");
        keeping
    }

    /// Failure (owner, 2026-11-13): a launch whose lock comes late tries in a busy loop, or for ever, or more often or less than it says; or gives up before ten
    /// seconds, so an old copy still quitting leaves the window unremembered. Ten tries, one a second from the state's taking in, the last at ten seconds, and
    /// then it's said and over: no deadline is left.
    #[test]
    fn it_tries_once_a_second_for_ten_seconds_and_then_says_so() {
        let start = 50_000;
        let mut keeping = trying_from(start);
        let mut tried_at = Vec::new();
        let mut now = start;
        let mut said = None;
        while let Some(wait) = keeping.due(now) {
            assert!(tried_at.len() <= TRIES as usize, "more tries than it says: {tried_at:?}");
            now += u64::try_from(wait.as_millis()).unwrap();
            tried_at.push(now - start);
            said = keeping.step(Event::Tried { got: false }, now);
            assert!(said.is_none() || keeping.due(now).is_none(), "said only at the end");
        }
        assert_eq!(tried_at, (1..=10).map(|second| second * 1_000).collect::<Vec<_>>());
        assert_eq!(said, Some(Out::SayNotKept));
        assert_eq!(keeping, Keeping::NotKept);
        assert_eq!(keeping.step(Event::Tried { got: true }, now + 1), None, "a straggler changes nothing");
    }

    /// Failure: a lock got on a try isn't taken as keeping the record (the saved session isn't restored, or the notice shows anyway), or a launch that kept it from
    /// the start is ever told to restore again or that it isn't kept.
    #[test]
    fn a_lock_got_late_restores_and_is_never_said() {
        let mut keeping = trying_from(0);
        assert_eq!(keeping.step(Event::Tried { got: false }, 1_000), None);
        assert_eq!(keeping.step(Event::Tried { got: true }, 2_000), Some(Out::Restore));
        assert_eq!(keeping, Keeping::Keeps);
        assert_eq!(keeping.due(2_000), None);
        assert_eq!(
            keeping.step(Event::Looked { now_as: 99 }, 3_000),
            None,
            "what the person does after that is remembered, not said"
        );
        let mut from_the_start = Keeping::default();
        assert_eq!(from_the_start.step(Event::Read { keeps: true, as_read: 7 }, 0), None);
        assert_eq!(from_the_start.due(0), None);
        assert_eq!(
            from_the_start.step(Event::Read { keeps: false, as_read: 7 }, 1),
            None,
            "a second read changes nothing"
        );
        assert_eq!(from_the_start, Keeping::Keeps);
    }

    /// Failure (the coordinator's choice, 2026-11-13): the person starts working in the window before the lock comes, and it goes on trying, so a late lock would
    /// restore over what they have or write their window over the saved session; or it stops and says nothing until the ten seconds are up. It stops at once and
    /// says so, and a look at a window as it was changes nothing.
    #[test]
    fn the_person_working_in_the_window_stops_it_and_says_so_at_once() {
        let mut keeping = trying_from(0);
        assert_eq!(keeping.step(Event::Looked { now_as: 7 }, 300), None, "the window as it was");
        assert!(keeping.trying());
        assert_eq!(keeping.step(Event::Looked { now_as: 8 }, 400), Some(Out::SayNotKept));
        assert_eq!(keeping, Keeping::NotKept);
        assert_eq!(keeping.due(400), None, "no more tries");
        assert_eq!(keeping.step(Event::Tried { got: true }, 1_000), None, "nor a restore");
    }
}
