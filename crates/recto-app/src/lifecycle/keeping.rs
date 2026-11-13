//! **Keeping the record when its lock comes late** (owner, 2026-11-13: "A window that doesn't get the record's lock at its start tries again quietly for about
//! ten seconds and starts remembering once it has it; another RectoPDF really keeping the state is said as it is today").
//!
//! A launch asks for the record's lock as it reads the state, and waits up to the store's patience for it (600 ms, `record_store::LOCK_PATIENCE`). One that still
//! hasn't got it says nothing yet, and tries again: one `try_lock` every [`TRY_EVERY_MS`] from the moment the state was taken in, the last at [`KEEP_TRYING_MS`],
//! so [`TRIES`] tries in all, each on the session thread's own deadline ([`Keeping::due`]): no thread of its own, no timer, and nothing at all once it's settled.
//! Then one of four:
//! - **The lock comes, and the person has done nothing in the window:** this copy keeps the record from here, as if it had got it at the start ([`Out::Restore`]):
//!   the saved session is read off the session thread and taken in as a launch's is ([`Keeping::Restoring`]), and once it's in ([`Event::Read`] with the lock)
//!   the window is made again from it, once ([`Out::Kept`]). The notice never shows. The common case: the copy before it was still quitting, or an update's
//!   hand-off was letting go.
//! - **The person opens or changes something first:** trying stops at once, and the window is one that won't be remembered, said at once ([`Out::SayNotKept`]):
//!   they've started working in it, and should know. Restoring now would replace what they have, and keeping the record with their window would write over the
//!   saved session, so neither is done (the coordinator's choice on 2026-11-13, until the saved session can be added beside theirs).
//! - **The tries run out:** another RectoPDF really is keeping it, said as today ([`Out::SayNotKept`]), and nothing's tried again.
//! - **A quit or an update's hand-off begins first** ([`Event::Stopped`]): no more tries, and nothing said, as the window's going. Nothing in it can be unsaved
//!   (anything in it would have stopped the trying), so a quit always goes.
//!
//! **What "done something" is.** What the window has, as one number ([`Event::Read`]'s `as_read`, then [`Event::Looked`]): the session as the record would have it
//! (its tabs and each one's place, zoom, rotation and edits; the paper; Find; edit mode; the agent icon shown or hidden; the recent files), the appearance the
//! person chose, and any file being opened. So opening a file counts from the moment it's asked for, before it's read, a file named at the launch or dropped on
//! the app included: double-clicking a PDF while the copy before is still quitting opens that PDF in a window that won't be remembered, and the saved session
//! waits for the next launch (the narrowed rule). What doesn't count, and is replaced by the restore as a launch would set it: the window's frame, full screen and
//! focus, the sidebar shown or hidden and its width, the panel, and the Settings window.
//!
//! **While it's settling an agent changes nothing** ([`Keeping::refuses_agents_changes`]: trying, or restoring): every command of an agent's but a look at the
//! state is refused `busy`, to try again in a moment, before it's counted as under way (`core::keeping::answers_agents`). Answered: `session show`, `session
//! check`, `update status`, `settings` and `update dev-builds` with no value, `qa read`, and `qa drive`, which acts as the person in a quiet run and so counts as
//! the person's. Refused: everything else, a page's text, a measure, a wait and `session save` included. An agent's action never counts as the person's, so it
//! can neither stop the trying (and show the person a notice) nor be replaced by the restore, its tab number then naming one of the owner's tabs. A person's
//! command under way on its own thread, whose change isn't in to be looked at yet, makes a try one that didn't get the lock (`core::keeping`).
//!
//! **Nothing writes over the record between the try and the restore.** Every writer of the record writes only while this copy holds the lock and keeps the
//! record ([`Keeping::writes`]), looked at before the saver's gate and again under it, where it holds from gathering to writing: never while it tries (a try
//! may be back with the lock and not yet decided on), restores, or has settled not to (`core::saver`). So a writer either gathers the restored session or
//! writes nothing.
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
    /// It keeps the record: from the start, or since a late try's restore was taken in.
    Keeps,
    /// Trying again, quietly: since when, how many tries so far, what the window had when the state was taken in, and whether a try is out (made on a thread
    /// of its own, and not yet back).
    Trying { since_ms: u64, tries: u32, as_read: u64, asking: bool },
    /// A try got the lock, and the saved session is being read and taken in: the record's this copy's, and nothing's written to it until that's done.
    Restoring,
    /// It doesn't, for good: the tries ran out or the person started working in the window first. Said, and the updater told it's a second copy.
    NotKept,
    /// A quit or an update's hand-off began while it tried: no more tries, nothing said, and the updater told nothing, as the app is going.
    Stopped,
}

/// What happens to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The state's been taken in, with the lock (`keeps`) or without it. The first time, `as_read` is what the window had then. Taken in with the lock while it's
    /// trying or restoring, it keeps the record from here: a late try's restore, or one read by any other way.
    Read { keeps: bool, as_read: u64 },
    /// A call's been taken on the session thread: what the window has now.
    Looked { now_as: u64 },
    /// A try at the lock is going out, because [`Keeping::due`] said so and a look found the window as it was read: made off the session thread and off the
    /// life's lock, as it opens a file, so a sleeping disk holds up nothing.
    Asking,
    /// The try that went out is back, with or without the lock, and the window's been looked at again ([`Event::Looked`]) just before this, on the session
    /// thread: so anything the person did while it was out counts first.
    Tried { got: bool },
    /// A quit or an update's hand-off has begun.
    Stopped,
}

/// What the app does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// A try at the lock goes out now, on a thread of its own.
    Ask,
    /// It has the lock: the saved session is read and taken in, as at a launch.
    Restore,
    /// A try came back with the lock after the trying stopped (the person started working in the window, or a quit or a hand-off began while it was out): the
    /// lock is let go again at once, and nothing is written.
    LetGo,
    /// **It keeps the record from here,** by whatever route: told to the updater as a copy that keeps it, if its launch was held back. `remake` when a late
    /// try's saved session is in: the window is made again from it, once, as a launch's is.
    Kept { remake: bool },
    /// **The window won't be remembered:** the notice is shown, in today's words, and the updater told this is a second copy.
    SayNotKept,
}

impl Keeping {
    /// **The one transition.**
    #[must_use]
    pub fn step(&mut self, event: Event, now_ms: u64) -> Option<Out> {
        match (*self, event) {
            (Self::NotAsked, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                Some(Out::Kept { remake: false })
            }
            (Self::NotAsked, Event::Read { keeps: false, as_read }) => {
                *self = Self::Trying {
                    since_ms: now_ms,
                    tries: 0,
                    as_read,
                    asking: false,
                };
                None
            }
            (Self::Restoring, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                Some(Out::Kept { remake: true })
            }
            // Read with the lock by another way while it tries: the state can't say it's trying in a window that keeps the record.
            (Self::Trying { .. }, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                Some(Out::Kept { remake: false })
            }
            (Self::Trying { as_read, .. }, Event::Looked { now_as }) if now_as != as_read => {
                *self = Self::NotKept;
                Some(Out::SayNotKept)
            }
            (
                Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: false,
                },
                Event::Asking,
            ) => {
                *self = Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: true,
                };
                Some(Out::Ask)
            }
            (Self::Trying { asking: true, .. }, Event::Tried { got: true }) => {
                *self = Self::Restoring;
                Some(Out::Restore)
            }
            (
                Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: true,
                },
                Event::Tried { got: false },
            ) => {
                let tries = tries + 1;
                if tries >= TRIES {
                    *self = Self::NotKept;
                    return Some(Out::SayNotKept);
                }
                *self = Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: false,
                };
                None
            }
            // Back with the lock once it's for good not keeping it: let go again, as nothing's to be written with it.
            (Self::NotKept | Self::Stopped, Event::Tried { got: true }) => Some(Out::LetGo),
            (Self::Trying { .. }, Event::Stopped) => {
                *self = Self::Stopped;
                None
            }
            // A state taken in again without the lock, a look at a window as it was, a try asked for with one out or a try back with none out, a try back while
            // it keeps the record by another route (whose lock it is), or a quit once it's settled or restoring (whose writer writes nothing until the restore's
            // in): nothing changes.
            _ => None,
        }
    }

    /// How long until the next try, zero once it's due; `None` when nothing's trying, or a try is out.
    #[must_use]
    pub fn due(&self, now_ms: u64) -> Option<Duration> {
        let Self::Trying {
            since_ms,
            tries,
            asking: false,
            ..
        } = *self
        else {
            return None;
        };
        let at = since_ms.saturating_add(TRY_EVERY_MS.saturating_mul(u64::from(tries) + 1));
        Some(Duration::from_millis(at.saturating_sub(now_ms)))
    }

    /// **Whether it has settled,** keeping the record or not: every way into either is a transition that says so ([`Out::Kept`] or [`Out::SayNotKept`]),
    /// which is where the updater is told of a launch that was held back, so a launch held back and settled can't be left untold.
    #[must_use]
    pub fn settled(&self) -> bool {
        matches!(self, Self::Keeps | Self::NotKept)
    }

    /// **Whether a writer of the record may write, if this copy holds its lock:** before the state's read (when nothing's started writing yet) and once it keeps
    /// it; never while it tries, restores, or has settled not to.
    #[must_use]
    pub fn writes(&self) -> bool {
        matches!(self, Self::NotAsked | Self::Keeps)
    }

    /// Whether it's still trying.
    #[must_use]
    pub fn trying(&self) -> bool {
        matches!(self, Self::Trying { .. })
    }

    /// Whether the saved session is being read and taken in after a late try: nothing's written to the record, and what the person does waits for it, as at a
    /// launch.
    #[must_use]
    pub fn restoring(&self) -> bool {
        *self == Self::Restoring
    }

    /// **Whether an agent's change is refused now:** while it's trying or restoring. See the module's words.
    #[must_use]
    pub fn refuses_agents_changes(&self) -> bool {
        matches!(self, Self::Trying { .. } | Self::Restoring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A try sent out at `now` and back with `got`: what it comes back to.
    fn try_once(keeping: &mut Keeping, got: bool, now: u64) -> Option<Out> {
        assert_eq!(keeping.step(Event::Asking, now), Some(Out::Ask), "a try goes out from {keeping:?}");
        assert_eq!(keeping.due(now), None, "no second try while one is out");
        keeping.step(Event::Tried { got }, now)
    }

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
            said = try_once(&mut keeping, false, now);
            assert!(said.is_none() || keeping.due(now).is_none(), "said only at the end");
        }
        assert_eq!(tried_at, (1..=10).map(|second| second * 1_000).collect::<Vec<_>>());
        assert_eq!(said, Some(Out::SayNotKept));
        assert_eq!(keeping, Keeping::NotKept);
        assert_eq!(
            keeping.step(Event::Tried { got: true }, now + 1),
            Some(Out::LetGo),
            "a straggler with the lock lets it go"
        );
        assert_eq!(keeping, Keeping::NotKept);
    }

    /// Failure: a lock got on a try isn't taken as keeping the record (the saved session isn't restored, or the notice shows anyway), or the window isn't made
    /// again from it once it's in, or made again twice; or a launch that kept it from the start is ever told to restore again or that it isn't kept.
    #[test]
    fn a_lock_got_late_restores_and_is_never_said() {
        let mut keeping = trying_from(0);
        assert_eq!(try_once(&mut keeping, false, 1_000), None);
        assert_eq!(try_once(&mut keeping, true, 2_000), Some(Out::Restore));
        assert_eq!(keeping, Keeping::Restoring);
        assert_eq!(keeping.due(2_000), None, "no more tries while it restores");
        assert_eq!(
            keeping.step(Event::Looked { now_as: 99 }, 2_001),
            None,
            "nothing the window has stops a restore under way"
        );
        assert_eq!(keeping.step(Event::Stopped, 2_002), None, "nor a quit, whose write waits for it");
        assert_eq!(
            keeping.step(Event::Read { keeps: true, as_read: 0 }, 2_050),
            Some(Out::Kept { remake: true }),
            "the window made again from it"
        );
        assert_eq!(keeping, Keeping::Keeps);
        assert_eq!(keeping.step(Event::Read { keeps: true, as_read: 0 }, 2_060), None, "and only once");
        assert_eq!(
            keeping.step(Event::Looked { now_as: 99 }, 3_000),
            None,
            "what the person does after that is remembered, not said"
        );
        let mut from_the_start = Keeping::default();
        assert_eq!(
            from_the_start.step(Event::Read { keeps: true, as_read: 7 }, 0),
            Some(Out::Kept { remake: false }),
            "kept, and nothing made again"
        );
        assert_eq!(from_the_start.due(0), None);
        assert_eq!(
            from_the_start.step(Event::Read { keeps: false, as_read: 7 }, 1),
            None,
            "a second read changes nothing"
        );
        assert_eq!(from_the_start, Keeping::Keeps);
    }

    /// Failure (review, 2026-11-13: an agent's `session restore` took the lock outside the machine, which stayed trying and then said "won't be remembered" in a
    /// window that kept and wrote the record): the state says trying while the store keeps the record. Taken in with the lock by any way while it's trying, it
    /// keeps the record, with nothing restored again over it and nothing said.
    #[test]
    fn a_read_with_the_lock_while_it_tries_keeps_the_record() {
        let mut keeping = trying_from(0);
        assert_eq!(keeping.step(Event::Read { keeps: true, as_read: 0 }, 500), Some(Out::Kept { remake: false }));
        assert_eq!(keeping, Keeping::Keeps);
        assert_eq!(keeping.due(500), None);
        assert_eq!(keeping.step(Event::Looked { now_as: 8 }, 600), None, "and nothing's said after");
    }

    /// Failure (review, 2026-11-13): a quit or an update's hand-off begins while it tries, and a try after that takes the lock and restores under the quit's write,
    /// which had gathered the empty window and then writes it as the newest record; or the quit shows the notice. It stops, says nothing, and tries no more.
    #[test]
    fn a_quit_or_a_hand_off_stops_the_trying_quietly() {
        let mut keeping = trying_from(0);
        assert_eq!(keeping.step(Event::Stopped, 400), None, "nothing said");
        assert_eq!(keeping, Keeping::Stopped);
        assert_eq!(keeping.due(1_000), None, "no more tries");
        assert_eq!(keeping.step(Event::Asking, 1_000), None, "none goes out");
        assert_eq!(
            keeping.step(Event::Tried { got: true }, 1_000),
            Some(Out::LetGo),
            "nor a restore: one back with the lock lets it go"
        );
    }

    /// Failure (review, 2026-11-13, S1): a try is out on its own thread (the lock file's open on a sleeping disk) when a quit begins or the person changes
    /// something, and comes back with the lock: kept, it restores under the quit's write or over what the person has, or keeps the record in a window that says
    /// it won't be remembered. Or a second try goes out beside the first. One out at a time; what came while it was out wins, and the lock it brings is let go.
    #[test]
    fn what_comes_while_a_try_is_out_wins_and_its_lock_is_let_go() {
        for (event, settles_to) in [(Event::Looked { now_as: 8 }, Keeping::NotKept), (Event::Stopped, Keeping::Stopped)] {
            let mut keeping = trying_from(0);
            assert_eq!(keeping.step(Event::Asking, 1_000), Some(Out::Ask));
            assert_eq!(keeping.due(5_000), None, "no try's due while one is out");
            assert_eq!(keeping.step(Event::Asking, 5_000), None, "nor goes out");
            let _ = keeping.step(event, 1_001);
            assert_eq!(keeping, settles_to, "{event:?}");
            assert_eq!(keeping.step(Event::Tried { got: true }, 1_002), Some(Out::LetGo), "{event:?}");
            assert_eq!(keeping, settles_to, "{event:?}: and stays so");
        }
    }

    /// Failure (review, 2026-11-13): an agent's open or edit lands while it's trying, stopping the retry and showing the person a notice for what an agent did, or
    /// while the saved session's being read, and the restore then replaces it and its tab number names one of the owner's tabs. Refused exactly while it's trying
    /// or restoring, and never once it keeps the record or has settled not to.
    #[test]
    fn agents_changes_are_refused_while_it_settles_and_only_then() {
        let mut keeping = Keeping::default();
        assert!(!keeping.refuses_agents_changes(), "before the state's read, the socket isn't served at all");
        let _ = keeping.step(Event::Read { keeps: false, as_read: 7 }, 0);
        assert!(keeping.refuses_agents_changes(), "trying");
        let _ = try_once(&mut keeping, true, 1_000);
        assert!(keeping.refuses_agents_changes(), "restoring");
        let _ = keeping.step(Event::Read { keeps: true, as_read: 0 }, 1_050);
        assert!(!keeping.refuses_agents_changes(), "kept");
        let mut not_kept = trying_from(0);
        let _ = not_kept.step(Event::Looked { now_as: 8 }, 10);
        assert!(!not_kept.refuses_agents_changes(), "settled not to keep it");
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
        assert_eq!(keeping.step(Event::Asking, 1_000), None, "none goes out");
    }

    /// Every state the machine can be in, with what it was read with: the states to walk every event from.
    fn every_state() -> Vec<Keeping> {
        let mut states = vec![Keeping::NotAsked, Keeping::Keeps, Keeping::Restoring, Keeping::NotKept, Keeping::Stopped];
        for tries in [0, 1, TRIES - 1] {
            for asking in [false, true] {
                states.push(Keeping::Trying {
                    since_ms: 0,
                    tries,
                    as_read: 7,
                    asking,
                });
            }
        }
        states
    }

    /// Every event, each way it can come.
    fn every_event() -> Vec<Event> {
        let mut events = vec![Event::Stopped, Event::Asking, Event::Tried { got: true }, Event::Tried { got: false }];
        for keeps in [true, false] {
            events.push(Event::Read { keeps, as_read: 7 });
            events.push(Event::Read { keeps, as_read: 8 });
        }
        events.push(Event::Looked { now_as: 7 });
        events.push(Event::Looked { now_as: 8 });
        events
    }

    /// Failure (review, 2026-11-13, the blocker: a lock got in the launch's read after its first wait went to `Keeps` with nothing said, so the updater, held back
    /// for the lock, was never told of the launch: no timer, every check "still starting", and after an update the installer's two minutes ran out and it put the
    /// record from before the update over the person's): a way into keeping the record or not keeping it that doesn't say so, which is where the updater is told.
    /// Every event from every state: settling always says how, saying how is always a settling, and once settled nothing settles again.
    #[test]
    fn every_way_into_settled_says_so_and_only_once() {
        let mut walked = 0;
        for state in every_state() {
            for event in every_event() {
                let mut after = state;
                let out = after.step(event, TRY_EVERY_MS * u64::from(TRIES));
                let says = matches!(out, Some(Out::Kept { .. } | Out::SayNotKept));
                let settles = !state.settled() && after.settled();
                assert_eq!(says, settles, "{state:?} on {event:?} to {after:?} said {out:?}");
                if state.settled() {
                    assert_eq!(after, state, "{state:?} settled, and {event:?} moved it to {after:?}");
                }
                if let Some(Out::Kept { .. }) = out {
                    assert_eq!(after, Keeping::Keeps, "{state:?} on {event:?}");
                }
                if let Some(Out::SayNotKept) = out {
                    assert_eq!(after, Keeping::NotKept, "{state:?} on {event:?}");
                }
                walked += 1;
            }
        }
        assert_eq!(walked, every_state().len() * every_event().len());
    }
}
