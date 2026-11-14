//! **The record's lock come late, carried out:** what [`crate::lifecycle::keeping`] decides (a launch that didn't get the lock tries again quietly, ten seconds
//! or a minute after an update), done. The UI thread never waits for any of it, and the session thread never waits on the disk for it: the app's life lock is
//! taken only for a step of the pure machine or a copy of it.
//!
//! **Every step is carried out by one function** ([`crate::lifecycle::keeping::carry`], through [`Carrier`]): the lock first, then how keeping settled, if it
//! did: the notice as the state says, and the updater told of a launch held back. Nothing between them can return early, so a lock given back as keeping
//! settles still settles (the third read's blocker, 2026-11-13).
//!
//! **The order of a try** (the reviews of 2026-11-13):
//! 1. On the session thread, at its deadline: the window is looked at, and if it's as it was read, one try goes out ([`Keeper::try_the_lock`]), on a thread of its own,
//!    as the store's `try_lock` opens a file ([`recto_control::record_store::RecordStore::try_to_keep`]). None is due while one is out. With no thread to be
//!    had, or a person's command under way on its own thread (its change isn't in to be looked at), it's a try that didn't get the lock.
//! 2. Back on the session thread ([`try_back`]): the window is looked at again first, so whatever the person did while it was out counts before the lock does,
//!    then the machine decides. Kept: the try's thread reads. Otherwise a lock it brought is **given back on the session thread, before the call ends**: an
//!    unlock and a close of a descriptor already open, which wait on no disk (the open, which can, stays on the try's thread). So no later try can find a stale
//!    lock taken, or have it released under it by a thread that woke late (the third read, 3).
//! 3. Restoring, on the try's thread ([`Keeper::read_late`]): the saved session is read as a launch reads it, then the read's thread claims the record (only while the
//!    patience isn't over), and only then tells the updater this copy keeps it, falls back from a record it can't read after an update that kept ending, and
//!    counts its tabs. Meanwhile every writer writes nothing ([`Keeper::may_write`]), what the person does waits (the session thread holds it back, as at a launch), and
//!    agents are refused. For [`crate::lifecycle::keeping::RESTORING_PATIENCE_MS`] at most, until it claims: past it, the window goes on as it is, what waited
//!    runs, the notice says it won't be remembered, the updater hears it's a second copy (the installer sees the new version started, with no tab count, and the
//!    install's left for the next launch that keeps the record), and the lock is given back once the read's thread is out of its read and says so: never while
//!    it's still reading, so nothing of the read's runs without the lock (the third read, 4).
//! 4. On the session thread, as at a launch: it's taken in ([`super::session::apply_restored`]), the machine keeps the record, what the person did meanwhile
//!    runs on it, a first frame shown meanwhile is told to the updater with the tabs it came up with, and the host is told to make the window again from it,
//!    once, if the person hasn't touched it.
//!
//! **What moved for 4** (the third read, 2026-11-13): a late keep's read told the updater of the launch (`Updater::launched`, which settles the install as the
//! copy that keeps the record) before it read; now it tells it after the read and the claim. Going back for a version that keeps failing is decided there too,
//! so a late keep's read happens before that decision; a launch that got the lock at once still tells the updater before it reads, as always
//! ([`Keeper::read`]). Falling back stays after telling, as it needs what telling settles.
//!
//! The read itself is about 2.3 ms for a session of 100 tabs each with a journal (median of 20, release build, this Mac under a load of about 4, 2026-11-13).

use super::keeper::Keeper;
use super::*;
use crate::lifecycle::keeping::{Event as KeepingEvent, Keeping};
use std::hash::{Hash as _, Hasher as _};

/// **The keeping desk: what [`crate::lifecycle::keeping`] decides, carried out, and nothing else.** It owns one rule: every keeping event goes through
/// [`Desk::step`] or [`Desk::try_back`] and is carried by [`crate::lifecycle::keeping::carry`], with nothing between (no other code in the app makes a keeping
/// step: the life makes one only with the desk's [`Carrier`], which only this file can build). It borrows only what its methods read, each said on the method: the life, the host (the
/// session, the notice, what's under way), the record's store, the queue and the updater; and the record's keeper they're all borrowed from
/// ([`Keeper`]), which [`Carrier::ask`]'s spawn of the try's thread holds instead of the app (R3.7 S5: the desk's hold on the whole app is gone). Six
/// references, held so by a check when it's built, made for a call and dropped with it: no lock, thread or allocation of its own. Made only by
/// [`Keeper::desk`].
pub(super) struct Desk<'a> {
    life: &'a Arc<Lifecycle>,
    host: &'a Arc<AppHost>,
    store: &'a Arc<recto_control::record_store::RecordStore>,
    core: &'a Arc<CoreQueue>,
    updates: &'a Arc<recto_update::updater::Updater>,
    keeper: &'a Arc<Keeper>,
}

const _: () = assert!(
    std::mem::size_of::<Desk<'_>>() == 6 * std::mem::size_of::<usize>(),
    "the desk is six references"
);

impl<'a> Desk<'a> {
    /// The desk for a call, from the keeper's handles: [`Keeper::desk`]'s, and no one else's.
    pub(super) fn new(
        life: &'a Arc<Lifecycle>,
        host: &'a Arc<AppHost>,
        store: &'a Arc<recto_control::record_store::RecordStore>,
        core: &'a Arc<CoreQueue>,
        updates: &'a Arc<recto_update::updater::Updater>,
        keeper: &'a Arc<Keeper>,
    ) -> Self {
        Self {
            life,
            host,
            store,
            core,
            updates,
            keeper,
        }
    }

    /// **What the window has now, as one number:** the session as the record would have it, the appearance the person chose, and any file being opened. What
    /// counts and what doesn't is said in [`crate::lifecycle::keeping`]: the sidebar shown or hidden is the window's arrangement, so it's left out here, with the
    /// window's frame, focus, the sidebar's width, the panel and Settings, which aren't in the session's record at all. Reads the session and the window.
    fn as_done(&self, window: &Window, reads: &FileReads) -> u64 {
        let mut record = self.host.session.with_session(|session| session.to_record());
        record.view.sidebar_hidden = false;
        let appearance = window.appearance();
        let mut opening: Vec<&std::path::Path> = reads
            .running
            .iter()
            .filter_map(|reading| match reading {
                Reading::File(path) => Some(path.as_path()),
                _ => None,
            })
            .collect();
        opening.sort_unstable();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_vec(&(record, appearance)).unwrap_or_default().hash(&mut hasher);
        opening.hash(&mut hasher);
        hasher.finish()
    }

    /// **The state's been taken in** (`session::apply_restored`), with `other_instance` when it was refused for the lock. The first time, whether this copy keeps
    /// the record starts here; taken in with the lock while it's trying, restoring or settled not to (a late try's restore, or a `session restore` by anyone), it
    /// keeps it from here. Says whether the notice that came with what was read is said now (not while it's trying, nor once it's settled not to keep the record,
    /// whose own notice is said instead), and what the host is told: the window made again from a late keep's restore. Reads the store, the window (as done), the
    /// updater (whether an update opened the launch), the life, and sets the notice.
    pub(super) fn taken_in(&self, window: &Window, other_instance: bool, first_time: bool, reads: &FileReads) -> (bool, Option<crate::lifecycle::Out>) {
        let keeps = self.store.holds_the_record();
        let mut host = None;
        if first_time || keeps {
            let as_read = if keeps { 0 } else { self.as_done(window, reads) };
            // A stat or two (the update's plan or outcome), as the window's patience asks on this thread too.
            let by_update = !keeps && self.updates.may_be_reopened_by_update();
            host = self.step(KeepingEvent::Read { keeps, as_read, by_update });
        }
        let keeping = self.life.keeping();
        // Where the store answers: it can't be "not kept" with the store holding the lock, by any route. (A late read still out holds it as `LettingGo`, which
        // refuses a `session restore` until it's out.)
        debug_assert!(!(keeping == Keeping::NotKept && keeps), "not kept with the record's lock held");
        if keeping.not_kept() {
            // A restore's saved notice isn't put over the one that's true.
            *lock(&self.host.notice) = Some(record::notices::OTHER_INSTANCE.to_owned());
        }
        (!(other_instance && keeping.trying()) && !keeping.not_kept(), host)
    }

    /// After every call on the session thread, while it's trying: the person having opened or changed something stops it, and that's said now. Reads the life, and
    /// the window as done.
    pub(super) fn look_after_call(&self, window: &Window, reads: &FileReads) {
        if !self.life.keeping().trying() {
            return;
        }
        let now_as = self.as_done(window, reads);
        let _ = self.step(KeepingEvent::Looked { now_as });
    }

    /// **The session thread's deadline while it's trying or restoring:** a restore's patience over, or a look at the window and, if it's as it was read, one try at
    /// the lock sent out (see the module's order). Reads the life and the window as done; tells the queue to look again.
    pub(super) fn when_due(&self, window: &Window, reads: &FileReads) {
        if !self.life.keeping_due().is_some_and(|wait| wait.is_zero()) {
            return;
        }
        if self.life.keeping().restoring() {
            let _ = self.step(KeepingEvent::ReadPatienceOver);
            self.core.push(CoreCall::Reconcile);
            return;
        }
        let now_as = self.as_done(window, reads);
        let _ = self.step(KeepingEvent::Looked { now_as });
        if self.life.keeping().trying() {
            let _ = self.step(KeepingEvent::Asking);
        }
        self.core.push(CoreCall::Reconcile);
    }

    /// **A try is back,** on the session thread: the window's looked at first, so whatever the person did while it was out counts before the lock does; then the try's
    /// taken in, looked at under the life's lock with whatever of the person's is under way, and carried out. Reads the life, the window (as done, and its
    /// appearance), what's under way; tells the queue to look again.
    pub(super) fn try_back(&self, window: &Window, reads: &FileReads, back: TryBack) {
        if self.life.keeping().trying() {
            let now_as = self.as_done(window, reads);
            let _ = self.step(KeepingEvent::Looked { now_as });
        }
        let appearance_then = window.appearance();
        // Decided with nothing of the person's under way, looked at under the life's lock as `person::submit` looks the other way round.
        let _ = self.carried(Some((&back.verdict, appearance_then)), |carrier| {
            self.life.tried(back.got, || busy(self.life, self.host), carrier);
        });
        self.core.push(CoreCall::Reconcile);
    }

    /// **A late try's saved session, come in from its thread:** what's taken in; or, with nothing (the read's thread ended without it: a panic, or its patience ran
    /// out while it read), the machine told the thread is out of its read, which gives the lock back. Reads the life.
    pub(super) fn late_read(&self, read: Option<Box<(Loaded, record::Appearance)>>) -> Option<(Loaded, record::Appearance)> {
        let Some(loaded) = read else {
            let _ = self.step(KeepingEvent::ReadGone);
            return None;
        };
        // Only a claimed read comes in, and a claimed restore has no patience to run out: still restoring.
        self.life.keeping().restoring().then_some(*loaded)
    }

    /// A late keep's read is in, on the try's thread: claimed, if its patience isn't over ([`Keeper::read_late`]).
    pub(super) fn claimed(&self) {
        let _ = self.step(KeepingEvent::Claimed);
    }

    /// A writer found this copy keeping the record with the store holding no lock: "not kept", which says so ([`Keeper::may_write`]).
    pub(super) fn lock_lost(&self) {
        let _ = self.step(KeepingEvent::LockLost);
    }

    /// **A step of keeping the record, carried out:** every step anyone makes of it but a try's coming back ([`Self::try_back`], which carries out its own with the
    /// try's thread to tell), so the one place keeping settles is where the updater is told of a launch held back. Says what the host is told: the window made
    /// again from a late try's restore, for the caller to tell it once the session thread's call has ended.
    fn step(&self, event: KeepingEvent) -> Option<crate::lifecycle::Out> {
        self.carried(None, |carrier| self.life.step_keeping(event, carrier))
    }

    /// A step, made by `step` with the desk's carrier, which the life hands what it came to once its lock is let go ([`Lifecycle::step_keeping`]), with the
    /// try's thread to tell when a try is back (`verdict`); a launch held back and settled can't be left untold, by any route (a debug build says so if it is).
    fn carried(
        &self,
        verdict: Option<(&std::sync::mpsc::Sender<Verdict>, record::Appearance)>,
        step: impl FnOnce(&mut Carrier<'_>),
    ) -> Option<crate::lifecycle::Out> {
        let mut carrier = Carrier {
            desk: self,
            verdict,
            host: None,
        };
        step(&mut carrier);
        debug_assert!(
            !(self.life.keeping().settled() && self.life.update_facts.untold()),
            "keeping settled ({:?}) with the launch still held back from the updater",
            self.life.keeping()
        );
        carrier.host
    }
}

/// What the session thread decided about a try that came back with the lock, for the try's thread: the one thing it does with it. A lock that isn't kept is
/// given back by the session thread itself, and the try's thread is told nothing.
#[derive(Debug)]
pub enum Verdict {
    /// Keep it: read the saved session, as a launch does. With the appearance the window had then, so one the person chooses while it's read is kept
    /// ([`Window::restore`]).
    Read(record::Appearance),
}

/// A try back from its thread: whether it got the lock, and where the decision about it goes.
#[derive(Debug)]
pub struct TryBack {
    got: bool,
    verdict: std::sync::mpsc::Sender<Verdict>,
}

impl TryBack {
    /// A try back: whether it got the lock, and where the decision about it goes ([`Keeper::try_the_lock`]).
    pub(super) fn new(got: bool, verdict: std::sync::mpsc::Sender<Verdict>) -> Self {
        Self { got, verdict }
    }

    /// A try that didn't get the lock, as made on the session thread when no thread could make it: no thread hears the decision.
    fn without_the_lock() -> Self {
        Self {
            got: false,
            verdict: std::sync::mpsc::channel().0,
        }
    }

    /// One that didn't get the lock, for a test of the queue's variants.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::without_the_lock()
    }
}

/// Something under way that a late try's restore would replace or lose: a person's command on its own thread (its change isn't in to be looked at), or a
/// hand-off.
fn busy(life: &Lifecycle, host: &AppHost) -> bool {
    life.persons_running() > 0 || host.handing_off.load(Ordering::SeqCst)
}

/// **Whether an agent's command is answered while keeping the record is settling** (trying or restoring, [`crate::lifecycle::keeping::Keeping::refuses_agents_changes`]):
/// only those that are purely a look at the state, which are the ones that only ask, but `session save`, which writes. Everything else is refused `busy` before
/// it's counted as under way, so no agent's change is ever under way while it tries: a page's text, a measure and a wait included, as each reads a tab that
/// the restore is about to replace. `qa drive` acts as the person, in a quiet run, through the methods the window calls, so what it does counts as the person's:
/// it can stop the trying as their own action would, which is what it's for.
pub(crate) fn answers_agents(command: &recto_agent::Command) -> bool {
    command.only_asks() && !matches!(command, recto_agent::Command::Record(recto_agent::commands::Record::SessionSave(_)))
}

/// **Whether a `session restore` is refused now, for anyone:** while a late keep's read whose patience ran out is still inside its own read, which holds the
/// record's lock; a second read of the same folder beside it isn't made. Busy for a moment: refused as busy, to try again.
pub(crate) fn refuses_a_restore(life: &Lifecycle) -> bool {
    life.keeping() == Keeping::LettingGo
}

/// **The app's side of carrying out a step:** the try's thread to tell, when a try is back, and what the host is told once the call's over. **Only the desk
/// makes one** (its fields are private to this file), and the life steps keeping the record only with one ([`Lifecycle::step_keeping`],
/// [`Lifecycle::tried`]), which it carries what the step came to: so no other code makes a keeping step, and none is made without being carried out.
pub(super) struct Carrier<'a> {
    desk: &'a Desk<'a>,
    verdict: Option<(&'a std::sync::mpsc::Sender<Verdict>, record::Appearance)>,
    host: Option<crate::lifecycle::Out>,
}

impl crate::lifecycle::keeping::Carry for Carrier<'_> {
    /// On a thread of its own; a try that didn't get the lock with a person's command or a hand-off under way, or with no thread to be had, coming back through
    /// the queue as any try does.
    fn ask(&mut self) {
        if busy(self.desk.life, self.desk.host) {
            self.desk.core.push(CoreCall::TryBack(TryBack::without_the_lock()));
            return;
        }
        let (verdict, heard) = std::sync::mpsc::channel();
        let asking = Arc::clone(self.desk.keeper);
        let sent = std::thread::Builder::new()
            .name("recto-record-lock".into())
            .stack_size(512 * 1024)
            .spawn(move || asking.try_the_lock(verdict, &heard));
        if sent.is_err() {
            self.desk.core.push(CoreCall::TryBack(TryBack::without_the_lock()));
        }
    }

    /// The try's thread told to read; one that's gone has nothing to read.
    fn restore(&mut self) {
        if let Some((verdict, appearance_then)) = self.verdict {
            let _ = verdict.send(Verdict::Read(appearance_then));
        }
    }

    /// An unlock and a close of a descriptor already open: no disk is waited on, so it's done here, before anything else is taken.
    fn give_back(&mut self) {
        self.desk.store.give_back();
    }

    fn notice(&mut self, not_kept: bool) {
        {
            let mut notice = lock(&self.desk.host.notice);
            if not_kept {
                *notice = Some(record::notices::OTHER_INSTANCE.to_owned());
            } else if notice.as_deref() == Some(record::notices::OTHER_INSTANCE) {
                *notice = None;
            }
        }
        self.desk.core.push(CoreCall::Reconcile);
    }

    fn tell_the_updater(&mut self) {
        updates::tell_held(self.desk.life, self.desk.updates);
    }

    fn remake(&mut self) {
        self.host = Some(crate::lifecycle::Out::RestoreReady { with_state: true, late: true });
    }
}
