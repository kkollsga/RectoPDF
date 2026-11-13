//! **Where the app is in its life, as a state machine on the clock** (step R1.4): the state's coming in at launch, the window's patience for it, the window going up
//! with or without it, whether the window takes input, a hand-off's freeze and the quit it comes to, a quit the host asked for (its freeze, its write and its
//! answer), and the control socket's end. The host is told to quit for a hand-off, and that the session has ended, each by an output only one transition makes,
//! from a state it leaves: so at most once a hand-off and once in all, by construction, whoever asks how often. One small value, [`Life`], and one pure function, [`Life::step`], of the state, an
//! event and `now` in milliseconds of the app's [`crate::clock::Clock`]: no lock, no thread, no listener, no I/O. What it comes to is an [`Out`] for the caller to
//! carry out. So every order the arrivals can come in (the state read before the patience, at it, after it, with an update's 20 s or without) is tested here in
//! microseconds, on numbers, and not by launching the app on a loaded machine (`tests`).
//!
//! The app keeps it in [`crate::core::Lifecycle`], under a lock taken only by [`crate::core::Lifecycle::step`] and the queries, and carries out the `Out` after
//! letting go of it. The rules it goes by are the ones they always were, in [`crate::core::updates`]: [`restore_patience`] and [`takes_input`].

use crate::core::updates::{Patience, restore_patience, takes_input};
use std::time::Duration;

/// Keeping the record when its lock comes late: tried again quietly for ten seconds.
pub mod keeping;
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
    /// The host's been told to quit for it ([`Out::HandOffQuit`]): told once, as only [`HandOff::Done`] tells, and this is where telling leaves it.
    Quitting,
}

/// Where a quit the host asked for has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quit {
    /// None asked for, or one that stayed.
    #[default]
    No,
    /// Everything's being written, on the quit's thread, with what a person does held back.
    Writing,
    /// The state couldn't be written and there are edits a quit would lose: the host asks, Quit Anyway or Stay.
    Asking,
    /// Answered go: the app's on its way out.
    Going,
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
    quit: Quit,
    /// The window went up without the state and was made again from it when it came in: what a test of a late state asks, so it can't pass on the on-time path.
    remade: bool,
    /// The host's been told the control socket's session has ended ([`Out::SessionEnded`]): once in the app's life, as only this being false tells it.
    ended: bool,
    /// Whether this copy keeps the record, and its tries when the lock didn't come at the start ([`keeping`]).
    keeping: keeping::Keeping,
    /// The record was kept late and its saved session taken in, and the window hasn't been made again from it yet ([`Self::may_remake_window`]).
    late_kept: bool,
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
    /// The updater (or QA) asks for the quit a hand-off that's written comes to.
    HandOffQuit,
    /// The control socket's server has returned, however it was stopped.
    SocketEnded,
    /// The host asks to quit, by the menu or the system (a SIGTERM is one).
    QuitAsked,
    /// The quit's write is over: the state `written` (or this copy isn't the one keeping it), or not, by the flush's end or the quit's patience; and how many
    /// tabs have edits that aren't saved.
    QuitWritten { written: bool, unsaved_tabs: usize },
    /// The quit's question was answered with Stay.
    QuitStayed,
    /// Keeping the record: the state taken in, a call taken, or a try at the lock ([`keeping::Event`]).
    Keeping(keeping::Event),
}

/// What the caller does, once it has let go of the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// The host's told the window can go up: from the state, or without it once the patience is over. `late` is a state that came after the window went up
    /// without it: the window's made again from it (which a test's slow mode holds back, `slow::Seam::Remake`).
    RestoreReady { with_state: bool, late: bool },
    /// Commands and what a person does are held back, or taken again.
    Freeze(bool),
    /// A quit begins: commands and what a person does held back, an install under way and the tools stood down, and everything written on a thread of its own,
    /// which says [`Event::QuitWritten`].
    QuitBegins,
    /// What the quit comes to, for the host (`QuitAnswer::for_quit`). Either way it was asked for, so it's no early end; when it asks, an install may go on, and
    /// what a person does is taken again first if `unfreeze`: not while a hand-off holds it.
    QuitAnswer { go: bool, unsaved_tabs: usize, unfreeze: bool },
    /// The quit stayed: the run's watched again, as one that hasn't ended.
    QuitStayed,
    /// The host's told to quit now, with no prompt, for a hand-off that's written ([`crate::Listener::hand_off_quit`]). At most once a hand-off.
    HandOffQuit,
    /// The host's told the control socket's session has ended ([`crate::Listener::session_ended`]). At most once.
    SessionEnded,
    /// Keeping the record came to something: restore the saved session now, or say the window won't be remembered ([`keeping::Out`]).
    Keeping(keeping::Out),
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
            // Made again from the state only while the window may still be: the rule the host asks (`may_remake_window`) before it makes it again.
            Event::ShownWithState => {
                self.remade |= self.shown == Shown::WithoutState && !self.takes_input(now_ms);
                self.shown = Shown::WithState;
                self.late_kept = false;
                None
            }
            Event::HandOffWriting => {
                self.hand_off = HandOff::Writing;
                let _ = self.keeping.step(keeping::Event::Stopped, now_ms);
                Some(Out::Freeze(true))
            }
            // Only a hand-off that's writing has written: one that was undone, or has told the host to quit already, isn't written again by a late word.
            Event::HandOffWritten => {
                if self.hand_off == HandOff::Writing {
                    self.hand_off = HandOff::Done;
                }
                None
            }
            // The one way to [`Out::HandOffQuit`]: from written to quitting, so asking again, from any thread, tells the host nothing more.
            Event::HandOffQuit if self.hand_off == HandOff::Done => {
                self.hand_off = HandOff::Quitting;
                Some(Out::HandOffQuit)
            }
            Event::HandOffQuit => None,
            Event::SocketEnded => {
                let first = !self.ended;
                self.ended = true;
                first.then_some(Out::SessionEnded)
            }
            // Taken again, unless a quit that's writing or going holds it.
            Event::HandOffUndone => {
                self.hand_off = HandOff::Idle;
                (!self.frozen()).then_some(Out::Freeze(false))
            }
            Event::QuitAsked => {
                self.quit = Quit::Writing;
                let _ = self.keeping.step(keeping::Event::Stopped, now_ms);
                Some(Out::QuitBegins)
            }
            Event::QuitWritten { written, unsaved_tabs } => {
                let go = written || unsaved_tabs == 0;
                self.quit = if go { Quit::Going } else { Quit::Asking };
                Some(Out::QuitAnswer {
                    go,
                    unsaved_tabs,
                    unfreeze: !self.frozen(),
                })
            }
            // Only a question asked has an answer: a Stay that comes with none changes nothing.
            Event::QuitStayed if self.quit == Quit::Asking => {
                self.quit = Quit::No;
                Some(Out::QuitStayed)
            }
            Event::QuitStayed => None,
            // Kept late: the host's told as at a launch, and makes the window again from it once ([`Self::may_remake_window`]).
            Event::Keeping(event) => match self.keeping.step(event, now_ms) {
                Some(keeping::Out::Kept) => {
                    self.late_kept = true;
                    Some(Out::RestoreReady { with_state: true, late: true })
                }
                other => other.map(Out::Keeping),
            },
        }
    }

    /// How long until the next try at the record's lock, zero once it's due: the session thread's deadline while it's trying ([`keeping::Keeping::due`]).
    #[must_use]
    pub fn keeping_due(&self, now_ms: u64) -> Option<Duration> {
        self.keeping.due(now_ms)
    }

    #[must_use]
    pub fn keeping(&self) -> keeping::Keeping {
        self.keeping
    }

    /// **Whether the window is made again from the state now,** asked by the host as it hears a state come in after its window went up ([`Out::RestoreReady`]
    /// with `late`). A state that came after the window went up without it, while the window takes no input yet (as always). A saved session taken in on a late
    /// try at the record's lock ([`keeping`]), once, unless the person has `touched` the window since the launch: then what they have stays as it is, its frame and
    /// focus kept, and only what's in it comes back. Never for a state read without the lock, which has nothing to make a window from, while it's still trying
    /// or restoring: so a launch makes its window again at most once.
    #[must_use]
    pub fn may_remake_window(&self, now_ms: u64, touched: bool) -> bool {
        if self.keeping.refuses_agents_changes() {
            return false;
        }
        if self.late_kept {
            return !touched;
        }
        !self.takes_input(now_ms)
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

    /// Whether the window went up without the state and was then made again from it.
    #[must_use]
    pub fn remade(&self) -> bool {
        self.remade
    }

    #[must_use]
    pub fn hand_off(&self) -> HandOff {
        self.hand_off
    }

    /// Whether the host's been told the control socket's session has ended.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.ended
    }

    #[must_use]
    pub fn quit(&self) -> Quit {
        self.quit
    }

    /// **Whether commands and what a person does are held back:** while a hand-off is writing or has written, and while a quit is writing or going. Either holds
    /// it, so the other's end doesn't let go of it.
    #[must_use]
    pub fn frozen(&self) -> bool {
        self.hand_off != HandOff::Idle || matches!(self.quit, Quit::Writing | Quit::Going)
    }
}
