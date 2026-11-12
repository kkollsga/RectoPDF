//! **Where the app is in its life, as a state machine on the clock** (step R1.4): the state's coming in at launch, the window's patience for it, the window going up
//! with or without it, whether the window takes input, and a hand-off's freeze. One small value, [`Life`], and one pure function, [`Life::step`], of the state, an
//! event and `now` in milliseconds of the app's [`crate::clock::Clock`]: no lock, no thread, no listener, no I/O. What it comes to is an [`Out`] for the caller to
//! carry out. So every order the arrivals can come in (the state read before the patience, at it, after it, with an update's 20 s or without) is tested here in
//! microseconds, on numbers, and not by launching the app on a loaded machine (`tests`).
//!
//! The app keeps it in [`crate::core::Lifecycle`], under a lock taken only by [`crate::core::Lifecycle::step`] and the queries, and carries out the `Out` after
//! letting go of it. The rules it goes by are the ones they always were, in [`crate::core::updates`]: [`restore_patience`] and [`takes_input`].

use crate::core::updates::{Patience, restore_patience, takes_input};
use std::time::Duration;

#[cfg(test)]
mod tests;

/// What the host shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shown {
    /// Nothing yet: the window's waiting for the state.
    #[default]
    Not,
    /// The window went up without the state, past its patience.
    WithoutState,
    /// What the host shows has been made from the state that was read: at once, or by making the window again when it came in late.
    WithState,
}

/// Where a hand-off has got to. Rust's to know, so the app can't quit on one that didn't happen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HandOff {
    #[default]
    Idle,
    /// Taking no commands, and no user's changes, and writing.
    Writing,
    /// Written, forced to the drive and read back. Quitting now is what's left.
    Done,
}

/// **The app's life, as it stands.** A `Copy` value of a few bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Life {
    /// The record's been read and applied, whatever it said.
    restored: bool,
    shown: Shown,
    /// When a launch an update may have opened stops waiting for the state, in milliseconds of the clock: the session thread's own deadline, with no thread of
    /// its own.
    patience_due_ms: Option<u64>,
    hand_off: HandOff,
}

/// What happens to the app's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The window's patience for the state is over: the host's second, or the core's own deadline for a launch an update may have opened, which this sets.
    PatienceOver { may_be_reopened_by_update: bool },
    /// The state's been read and taken in.
    StateRead,
    /// What the host shows has been made from the state.
    ShownWithState,
    /// A hand-off begins: nothing was in flight, and from here what's done wouldn't come back.
    HandOffWriting,
    /// The state's written, forced to the drive and read back, with still nothing in flight.
    HandOffWritten,
    /// It isn't going ahead: something came in, the state couldn't be written, or the updater gave up.
    HandOffUndone,
}

/// What the caller does, once it has let go of the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// The host's told the window can go up: from the state, or without it once the patience is over. `late` is a state that came after the window went up
    /// without it: the window's made again from it (which a test's slow mode holds back, `slow::Seam::Remake`).
    RestoreReady { with_state: bool, late: bool },
    /// Commands and what a person does are held back, or taken again.
    Freeze(bool),
}

impl Life {
    /// **The one transition.** Pure: the state, the event and `now`; what it comes to is returned for the caller to carry out.
    #[must_use]
    pub fn step(&mut self, event: Event, now_ms: u64) -> Option<Out> {
        match event {
            Event::PatienceOver { may_be_reopened_by_update } => {
                // Only for a window still waiting: once the state's in, or the window's up without it, there's nothing to wait for.
                if self.restored || self.shown != Shown::Not {
                    self.patience_due_ms = None;
                    return None;
                }
                match restore_patience(may_be_reopened_by_update, Duration::from_millis(now_ms)) {
                    Patience::WaitFor(left) => {
                        self.patience_due_ms = Some(now_ms.saturating_add(u64::try_from(left.as_millis()).unwrap_or(u64::MAX)));
                        None
                    }
                    // Marked first, then the host's told, so the state's coming later is known to be late.
                    Patience::ShowNow => {
                        self.patience_due_ms = None;
                        self.shown = Shown::WithoutState;
                        Some(Out::RestoreReady {
                            with_state: false,
                            late: false,
                        })
                    }
                }
            }
            Event::StateRead => {
                let first = !self.restored;
                self.restored = true;
                self.patience_due_ms = None;
                first.then_some(Out::RestoreReady {
                    with_state: true,
                    late: self.shown == Shown::WithoutState,
                })
            }
            Event::ShownWithState => {
                self.shown = Shown::WithState;
                None
            }
            Event::HandOffWriting => {
                self.hand_off = HandOff::Writing;
                Some(Out::Freeze(true))
            }
            Event::HandOffWritten => {
                self.hand_off = HandOff::Done;
                None
            }
            Event::HandOffUndone => {
                self.hand_off = HandOff::Idle;
                Some(Out::Freeze(false))
            }
        }
    }

    /// How long until the patience for the state is over by the core's own deadline, if it's waiting on one: zero once it's due.
    #[must_use]
    pub fn due(&self, now_ms: u64) -> Option<Duration> {
        (!self.restored && self.shown == Shown::Not)
            .then_some(self.patience_due_ms)
            .flatten()
            .map(|at| Duration::from_millis(at.saturating_sub(now_ms)))
    }

    /// Whether the window takes what a person does: once it's been made from the state, or once the state's been waited for long enough.
    #[must_use]
    pub fn takes_input(&self, now_ms: u64) -> bool {
        takes_input(self.shown == Shown::WithState, Duration::from_millis(now_ms))
    }

    #[must_use]
    pub fn restored(&self) -> bool {
        self.restored
    }

    #[must_use]
    pub fn shown(&self) -> Shown {
        self.shown
    }

    #[must_use]
    pub fn hand_off(&self) -> HandOff {
        self.hand_off
    }
}
