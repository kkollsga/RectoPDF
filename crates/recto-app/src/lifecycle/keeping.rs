//! **Keeping the record when its lock comes late** (owner, 2026-11-13: "A window that doesn't get the record's lock at its start tries again quietly for about
//! ten seconds and starts remembering once it has it; another RectoPDF really keeping the state is said as it is today").
//!
//! A launch asks for the record's lock once, as it reads the state, and waits up to the store's patience for it (600 ms, `record_store::LOCK_PATIENCE`). One
//! that still hasn't got it says nothing yet, and tries again: one `try_lock` every [`TRY_EVERY_MS`] from the moment the state was taken in, the last at
//! [`KEEP_TRYING_MS`], so [`TRIES`] tries in all, each due on the session thread's own deadline ([`Keeping::due`]) and made on a short thread of its own, one at
//! a time ([`Event::Asking`], [`Event::Tried`]): no timer, and nothing at all once it's settled.
//! Then one of four:
//! - **The lock comes, and the person has done nothing in the window:** this copy keeps the record from here, as if it had got it at the start ([`Lock::Restore`]):
//!   the saved session is read off the session thread ([`Keeping::Restoring`], for [`RESTORING_PATIENCE_MS`] at most, then given up:
//!   [`Event::ReadPatienceOver`]); once it's read, the read's thread claims the record ([`Event::Claimed`]) and only then tells the updater this copy keeps it;
//!   and once it's in ([`Event::Read`] with the lock) the window is made again from it, once ([`Settle::Kept`]). The notice never shows. The common case: the
//!   copy before it was still quitting, or an update's hand-off was letting go.
//! - **The person opens or changes something first:** trying stops at once, and the window is one that won't be remembered, said at once ([`Settle::NotKept`]):
//!   they've started working in it, and should know. Restoring now would replace what they have, and keeping the record with their window would write over the
//!   saved session, so neither is done (the coordinator's choice on 2026-11-13, until the saved session can be added beside theirs).
//! - **The tries run out:** another RectoPDF really is keeping it, said as today ([`Settle::NotKept`]), and nothing's tried again. A launch an update opened
//!   tries for a minute ([`KEEP_TRYING_AFTER_UPDATE_MS`], the coordinator's decision of 2026-11-13), as its installer is waiting to hear it came up; as a second
//!   copy it still tells the installer it started, if it drew a page, so an update isn't rolled back for the lock (`recto_update::updater::Updater::first_frame`).
//! - **A quit or an update's hand-off begins first** ([`Event::Stopped`]): no more tries, and nothing said, as the window's going. Nothing in it can be unsaved
//!   (anything in it would have stopped the trying), so a quit always goes. A hand-off that's undone, or a quit that stays, takes the trying up again
//!   ([`Event::Resumed`]).
//!
//! **What the step returns is everything to carry out** ([`Out`]): what's done with a try's lock (sent, read with, given back) and how keeping settled, if it
//! did, each or both; and one function carries out all of it ([`carry`]), in that order, with nothing between them that can return early. So a lock given back
//! as the tries run out still settles: said, and the updater told (the third read's blocker, 2026-11-13).
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
//! command under way on its own thread, whose change isn't in to be looked at yet, makes a try that got the lock give it back ([`Event::Tried`]'s `busy`).
//!
//! **Nothing writes over the record between the try and the restore.** Every writer of the record writes only while this copy holds the lock and keeps the
//! record ([`Keeping::writes`]), looked at before the saver's gate and again under it, where it holds from gathering to writing: never while it tries (a try
//! may be back with the lock and not yet decided on), restores, or has settled not to (`core::saver`). So a writer either gathers the restored session or
//! writes nothing. A writer that finds it keeps the record and the store holding no lock says so ([`Event::LockLost`]): the state can't sit there silently.
//!
//! Pure: the state, an event and `now` in milliseconds of the app's clock. The app holds it in [`super::Life`] and carries out the [`Out`] (`core::keeping`).

use std::time::Duration;

/// How long a launch goes on trying for the lock after the state's been taken in without it.
pub const KEEP_TRYING_MS: u64 = 10_000;
/// The time between tries.
pub const TRY_EVERY_MS: u64 = 1_000;
/// How many tries in all: one a second, the last at ten seconds.
pub const TRIES: u32 = (KEEP_TRYING_MS / TRY_EVERY_MS) as u32;
/// **How long a launch an update opened goes on trying** (the coordinator's decision, 2026-11-13): its installer is waiting for it to say it came up, and a
/// late keep tells it with the tabs it restored, so it tries for longer, while the window's untouched. A minute: half the installer's two minutes
/// (`recto_update::plan::START_LIMIT_MS`), which leave the other half for the launch itself (the installer gives an app it hasn't seen run a quarter of its
/// limit, 30 s, to appear), the update launch's 20 s of patience for the state, and the first frame's drawing of a page, so the installer hears either way
/// well inside its limit: kept and restored, or a second copy that started (`recto_update::updater::Updater::first_frame`).
pub const KEEP_TRYING_AFTER_UPDATE_MS: u64 = 60_000;
/// **How long a late try's saved session has to be read** once the try got the lock (about 2.3 ms for 100 tabs), on a thread of its own, while what the person
/// does waits for it and agents are refused. Past it, the window goes on as it is, as a launch's does when its state doesn't come, and the record is left on
/// disk for the next launch: said, the updater told this is a second copy, and the lock let go once the read's thread is out of its read
/// ([`Event::ReadPatienceOver`], [`Keeping::LettingGo`]). Five seconds: room for a starved machine (a 1.25 s stall of a thread was seen on CI's runner,
/// 2026-11-13), and a bound on the person's wait if the disk is dead. Once the read's in hand and claimed ([`Event::Claimed`]) the patience is over: what's
/// left is telling the updater and taking it in.
pub const RESTORING_PATIENCE_MS: u64 = 5_000;
/// How many tries in all for a launch an update opened.
pub const TRIES_AFTER_UPDATE: u32 = (KEEP_TRYING_AFTER_UPDATE_MS / TRY_EVERY_MS) as u32;
// Half the installer's limit at most, held when it's built: a change to either that breaks it doesn't compile.
const _: () = assert!(KEEP_TRYING_AFTER_UPDATE_MS * 2 <= recto_update::plan::START_LIMIT_MS);

/// Whether this copy keeps the record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keeping {
    /// The state hasn't been taken in yet.
    #[default]
    NotAsked,
    /// It keeps the record: from the start, or since a late try's restore was taken in, or a restore by any route took the lock.
    Keeps,
    /// Trying again, quietly: since when, how many tries so far, what the window had when the state was taken in, whether a try is out (made on a thread
    /// of its own, and not yet back), and whether an update opened the launch (which tries for longer).
    Trying {
        since_ms: u64,
        tries: u32,
        as_read: u64,
        asking: bool,
        by_update: bool,
    },
    /// A try got the lock (since when), and the saved session is being read and taken in: the record's this copy's, and nothing's written to it until that's
    /// done. For [`RESTORING_PATIENCE_MS`] at most, until `claimed`: the read's in hand and its thread has claimed the record, and tells the updater so.
    Restoring { since_ms: u64, claimed: bool },
    /// **The restore's patience ran out with its read still out:** not kept, said, and the updater told it's a second copy; the lock stays with the read's thread
    /// until it's out of its read ([`Event::ReadGone`]), so nothing of the read's runs without it.
    LettingGo,
    /// It doesn't: the tries ran out, the person started working in the window first, a restore's read didn't come, or the lock was found gone. Said, and the
    /// updater told it's a second copy. A restore that takes the lock later keeps the record from there.
    NotKept,
    /// A quit or an update's hand-off began while it tried: no more tries, nothing said, and the updater told nothing, as the app is going. What it had, to take
    /// up again if the hand-off is undone or the quit stays.
    Stopped {
        since_ms: u64,
        tries: u32,
        as_read: u64,
        asking: bool,
        by_update: bool,
    },
}

/// What happens to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The state's been taken in, with the lock (`keeps`) or without it. The first time, `as_read` is what the window had then. Taken in with the lock while it's
    /// trying, restoring or not kept, it keeps the record from here: a late try's restore, or one read by any other way (`session restore`).
    /// `by_update`: the launch may be one an update opened ([`KEEP_TRYING_AFTER_UPDATE_MS`]).
    Read { keeps: bool, as_read: u64, by_update: bool },
    /// A call's been taken on the session thread: what the window has now.
    Looked { now_as: u64 },
    /// A try at the lock is going out, because [`Keeping::due`] said so and a look found the window as it was read: made off the session thread and off the
    /// life's lock, as it opens a file, so a sleeping disk holds up nothing.
    Asking,
    /// The try that went out is back, with or without the lock, and the window's been looked at again ([`Event::Looked`]) just before this, on the session
    /// thread: so anything the person did while it was out counts first. `busy`: a person's command was under way on its own thread as it came back, looked at
    /// under the life's lock, so a lock it got is given back, as a try that didn't get it.
    Tried { got: bool, busy: bool },
    /// A quit or an update's hand-off has begun.
    Stopped,
    /// The hand-off was undone, or the quit stayed: the trying is taken up again, if it was stopped.
    Resumed,
    /// The late try's saved session is read, and its thread claims the record before it tells the updater this copy keeps it: only while the patience isn't
    /// over.
    Claimed,
    /// The restore's patience is over and its read isn't in hand.
    ReadPatienceOver,
    /// The restore's read thread has ended without the read taken in: it panicked, or it was out when the patience ran out.
    ReadGone,
    /// A writer found this copy keeping the record and the store holding no lock.
    LockLost,
}

/// What's done with the record's lock, or a try at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// A try at the lock goes out now, on a thread of its own.
    Ask,
    /// The try that's back has the lock: the saved session is read on its thread, as at a launch.
    Restore,
    /// The lock a try or a read brought is given back, on the session thread, before anything else is taken: unlocked and closed, not for good, so a later try or
    /// a `session restore` may take it again.
    GiveBack,
}

/// How keeping settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// **It keeps the record from here,** by whatever route: the notice that it won't be remembered goes, and the updater's told, if its launch was held back.
    /// `remake` when a late try's saved session is in: the window is made again from it, once, as a launch's is.
    Kept { remake: bool },
    /// **The window won't be remembered:** the notice is shown, in today's words, and the updater told this is a second copy.
    NotKept,
}

/// **Everything a step comes to,** for [`carry`]: what's done with the lock, and how keeping settled, each or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Out {
    pub lock: Option<Lock>,
    pub settle: Option<Settle>,
}

impl Out {
    const fn lock(lock: Lock) -> Option<Self> {
        Some(Self {
            lock: Some(lock),
            settle: None,
        })
    }

    const fn settle(settle: Settle) -> Option<Self> {
        Some(Self {
            lock: None,
            settle: Some(settle),
        })
    }

    const fn both(lock: Option<Lock>, settle: Settle) -> Option<Self> {
        Some(Self { lock, settle: Some(settle) })
    }
}

/// **What the app does for [`carry`]:** each thing an [`Out`] can ask for, done by the app (`core::keeping`).
pub trait Carry {
    /// A try at the lock sent out on a thread of its own.
    fn ask(&mut self);
    /// The try's thread told to read the saved session.
    fn restore(&mut self);
    /// The lock given back, on this thread.
    fn give_back(&mut self);
    /// The notice as keeping settled: shown when the window won't be remembered, gone when it will.
    fn notice(&mut self, not_kept: bool);
    /// The updater told of a launch held back, now that keeping's settled.
    fn tell_the_updater(&mut self);
    /// The window made again from a late keep's restore.
    fn remake(&mut self);
}

/// **The one function that carries out a step,** every part of it, in order: the lock first, so the updater hears what's true of it; then how it settled. No
/// early return between them.
pub fn carry(out: Option<Out>, to: &mut impl Carry) {
    let Some(Out { lock, settle }) = out else { return };
    match lock {
        Some(Lock::Ask) => to.ask(),
        Some(Lock::Restore) => to.restore(),
        Some(Lock::GiveBack) => to.give_back(),
        None => {}
    }
    if let Some(settle) = settle {
        to.notice(settle == Settle::NotKept);
        to.tell_the_updater();
        if settle == (Settle::Kept { remake: true }) {
            to.remake();
        }
    }
}

impl Keeping {
    /// **The one transition.**
    #[must_use]
    pub fn step(&mut self, event: Event, now_ms: u64) -> Option<Out> {
        match (*self, event) {
            (Self::NotAsked, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                Out::settle(Settle::Kept { remake: false })
            }
            (
                Self::NotAsked,
                Event::Read {
                    keeps: false,
                    as_read,
                    by_update,
                },
            ) => {
                *self = Self::Trying {
                    since_ms: now_ms,
                    tries: 0,
                    as_read,
                    asking: false,
                    by_update,
                };
                None
            }
            (Self::Restoring { .. }, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                Out::settle(Settle::Kept { remake: true })
            }
            // Read with the lock by another way while it tries, or once it's settled not to keep it (a `session restore` that took the lock, the other copy
            // gone): the state can't say it isn't keeping the record in a window that holds its lock.
            (Self::Trying { .. } | Self::NotKept, Event::Read { keeps: true, .. }) => {
                *self = Self::Keeps;
                Out::settle(Settle::Kept { remake: false })
            }
            (Self::Trying { as_read, .. }, Event::Looked { now_as }) if now_as != as_read => {
                *self = Self::NotKept;
                Out::settle(Settle::NotKept)
            }
            (
                Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: false,
                    by_update,
                },
                Event::Asking,
            ) => {
                *self = Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: true,
                    by_update,
                };
                Out::lock(Lock::Ask)
            }
            (Self::Restoring { since_ms, claimed: false }, Event::Claimed) => {
                *self = Self::Restoring { since_ms, claimed: true };
                None
            }
            // The read didn't come in time: as a launch whose state never came, the window goes on as it is. Said, as the window won't be remembered; the
            // lock goes once the read's thread is out of its read, and a read that comes in after this is dropped.
            (Self::Restoring { claimed: false, .. }, Event::ReadPatienceOver) => {
                *self = Self::LettingGo;
                Out::settle(Settle::NotKept)
            }
            // Its thread ended without the read in (a panic): not kept, said, and the lock given back.
            (Self::Restoring { .. }, Event::ReadGone) => {
                *self = Self::NotKept;
                Out::both(Some(Lock::GiveBack), Settle::NotKept)
            }
            (Self::LettingGo, Event::ReadGone) => {
                *self = Self::NotKept;
                Out::lock(Lock::GiveBack)
            }
            (Self::Keeps, Event::LockLost) => {
                *self = Self::NotKept;
                Out::settle(Settle::NotKept)
            }
            (_, Event::Tried { got, busy }) => self.back(got, busy, now_ms),
            (_, Event::Stopped | Event::Resumed) => self.stop_or_resume(event, now_ms),
            // A state taken in again without the lock, a look at a window as it was, a try asked for with one out, a claim or a patience once it's claimed or
            // over, or a read taken in while the late read is still out (refused): nothing changes.
            _ => None,
        }
    }

    /// **A try back** ([`Event::Tried`]): with the lock and nothing of the person's under way, read with; otherwise a try that didn't get it, a lock it brought
    /// given back, and the last one settles, and says so, whichever it was. Back once it's for good not keeping the record, or stopped: a lock it brought is
    /// given back, as nothing's to be written with it.
    fn back(&mut self, got: bool, busy: bool, now_ms: u64) -> Option<Out> {
        let lock = got.then_some(Lock::GiveBack);
        match *self {
            Self::Trying { asking: true, .. } if got && !busy => {
                *self = Self::Restoring {
                    since_ms: now_ms,
                    claimed: false,
                };
                Out::lock(Lock::Restore)
            }
            Self::Trying {
                since_ms,
                tries,
                as_read,
                asking: true,
                by_update,
            } => {
                let tries = tries + 1;
                if tries >= if by_update { TRIES_AFTER_UPDATE } else { TRIES } {
                    *self = Self::NotKept;
                    return Out::both(lock, Settle::NotKept);
                }
                *self = Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking: false,
                    by_update,
                };
                lock.and_then(Out::lock)
            }
            Self::NotKept => lock.and_then(Out::lock),
            Self::Stopped {
                since_ms,
                tries,
                as_read,
                asking: true,
                by_update,
            } => {
                *self = Self::Stopped {
                    since_ms,
                    tries,
                    as_read,
                    asking: false,
                    by_update,
                };
                lock.and_then(Out::lock)
            }
            // None was out, or it keeps the record by another route (whose lock it is), or it's restoring or letting go: nothing changes.
            _ => None,
        }
    }

    /// **A quit or a hand-off begun, or undone** ([`Event::Stopped`], [`Event::Resumed`]): stopped where it was; taken up again where it was, the next try a second
    /// from now, not a burst of the tries the stop held back.
    fn stop_or_resume(&mut self, event: Event, now_ms: u64) -> Option<Out> {
        match (*self, event) {
            (
                Self::Trying {
                    since_ms,
                    tries,
                    as_read,
                    asking,
                    by_update,
                },
                Event::Stopped,
            ) => {
                *self = Self::Stopped {
                    since_ms,
                    tries,
                    as_read,
                    asking,
                    by_update,
                };
            }
            (
                Self::Stopped {
                    tries,
                    as_read,
                    asking,
                    by_update,
                    ..
                },
                Event::Resumed,
            ) => {
                *self = Self::Trying {
                    since_ms: now_ms.saturating_sub(TRY_EVERY_MS.saturating_mul(u64::from(tries))),
                    tries,
                    as_read,
                    asking,
                    by_update,
                };
            }
            // A quit once it's settled or restoring (whose writer writes nothing until the restore's in), or nothing to take up: nothing changes.
            _ => {}
        }
        None
    }

    /// How long until the next try, zero once it's due, or until a restore's patience is over; `None` when nothing's trying or restoring unclaimed, or a try
    /// is out.
    #[must_use]
    pub fn due(&self, now_ms: u64) -> Option<Duration> {
        if let Self::Restoring { since_ms, claimed: false } = *self {
            return Some(Duration::from_millis(since_ms.saturating_add(RESTORING_PATIENCE_MS).saturating_sub(now_ms)));
        }
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

    /// **Whether it has settled,** keeping the record or not: every way into either is a transition that says so ([`Out`]'s `settle`), which is where the updater
    /// is told of a launch that was held back, so a launch held back and settled can't be left untold.
    #[must_use]
    pub fn settled(&self) -> bool {
        matches!(self, Self::Keeps | Self::NotKept | Self::LettingGo)
    }

    /// Whether it has settled not to keep the record: the notice is what it says.
    #[must_use]
    pub fn not_kept(&self) -> bool {
        matches!(self, Self::NotKept | Self::LettingGo)
    }

    /// **Whether a writer of the record may write, if this copy holds its lock:** before the state's read (when nothing's started writing yet) and once it keeps
    /// it; never while it tries, restores, or has settled not to.
    #[must_use]
    pub fn writes(&self) -> bool {
        matches!(self, Self::NotAsked | Self::Keeps)
    }

    /// **Whether this copy claims the record, if it holds its lock,** as the updater's told of the launch: at the launch's own read, once it keeps it, and once a
    /// late keep's read is in hand and claimed; never while it tries, before the read's in, or once it's settled not to, whatever lock a try or a read still
    /// holds then.
    #[must_use]
    pub fn claims(&self) -> bool {
        matches!(self, Self::NotAsked | Self::Keeps | Self::Restoring { claimed: true, .. })
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
        matches!(self, Self::Restoring { .. })
    }

    /// **Whether an agent's change is refused now:** while it's trying or restoring. See the module's words.
    #[must_use]
    pub fn refuses_agents_changes(&self) -> bool {
        matches!(self, Self::Trying { .. } | Self::Restoring { .. })
    }
}

#[cfg(test)]
mod tests;
