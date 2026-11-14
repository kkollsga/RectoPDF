//! **The record's lock come late, carried out:** what [`crate::lifecycle::keeping`] decides (a launch that didn't get the lock tries again quietly, ten seconds
//! or a minute after an update), done. The UI thread never waits for any of it, and the session thread never waits on the disk for it: the app's life lock is
//! taken only for a step of the pure machine or a copy of it.
//!
//! **Every step is carried out by one function** ([`crate::lifecycle::keeping::carry`], through [`Carrier`]): the lock first, then how keeping settled, if it
//! did: the notice as the state says, and the updater told of a launch held back. Nothing between them can return early, so a lock given back as keeping
//! settles still settles (the third read's blocker, 2026-11-13).
//!
//! **The order of a try** (the reviews of 2026-11-13):
//! 1. On the session thread, at its deadline: the window is looked at, and if it's as it was read, one try goes out ([`try_the_lock`]), on a thread of its own,
//!    as the store's `try_lock` opens a file ([`recto_control::record_store::RecordStore::try_to_keep`]). None is due while one is out. With no thread to be
//!    had, or a person's command under way on its own thread (its change isn't in to be looked at), it's a try that didn't get the lock.
//! 2. Back on the session thread ([`try_back`]): the window is looked at again first, so whatever the person did while it was out counts before the lock does,
//!    then the machine decides. Kept: the try's thread reads. Otherwise a lock it brought is **given back on the session thread, before the call ends**: an
//!    unlock and a close of a descriptor already open, which wait on no disk (the open, which can, stays on the try's thread). So no later try can find a stale
//!    lock taken, or have it released under it by a thread that woke late (the third read, 3).
//! 3. Restoring, on the try's thread ([`read_late`]): the saved session is read as a launch reads it, then the read's thread claims the record (only while the
//!    patience isn't over), and only then tells the updater this copy keeps it, falls back from a record it can't read after an update that kept ending, and
//!    counts its tabs. Meanwhile every writer writes nothing ([`may_write`]), what the person does waits (the session thread holds it back, as at a launch), and
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
//! ([`super::launching::read`]). Falling back stays after telling, as it needs what telling settles.
//!
//! The read itself is about 2.3 ms for a session of 100 tabs each with a journal (median of 20, release build, this Mac under a load of about 4, 2026-11-13).

use super::*;
use crate::lifecycle::keeping::{Event as KeepingEvent, Keeping};
use std::hash::{Hash as _, Hasher as _};

/// **The keeping desk: what [`crate::lifecycle::keeping`] decides, carried out, and nothing else.** It owns one rule: every keeping event goes through
/// [`Desk::step`] or [`Desk::try_back`] and is carried by [`crate::lifecycle::keeping::carry`], with nothing between (no other code in the app makes a keeping
/// step: the life makes one only with the desk's [`Carrier`], which only this file can build). It borrows only what its methods read, each said on the method: the life, the host (the
/// session, the notice, what's under way), the record's store, the queue and the updater; and `app`, the whole app, which is there only for [`Carrier::ask`]'s
/// spawn of the try's thread, whose read (`try_the_lock`, `read_late`) takes it whole until the record's slice (S5) gives them parts. Six references, held so
/// by a check when it's built, made for a call and dropped with it: no lock, thread or allocation of its own. Each is the app's own handle (`&Arc`), so what a
/// thread needs of it can be cloned for that thread (the updater's telling now; the try's thread once S5 gives it parts) and `app` can go.
pub(super) struct Desk<'a> {
    life: &'a Arc<Lifecycle>,
    host: &'a Arc<AppHost>,
    store: &'a Arc<recto_control::record_store::RecordStore>,
    core: &'a Arc<CoreQueue>,
    updates: &'a Arc<recto_update::updater::Updater>,
    app: &'a Arc<Inner>,
}

const _: () = assert!(
    std::mem::size_of::<Desk<'_>>() == 6 * std::mem::size_of::<usize>(),
    "the desk is six references"
);

impl<'a> Desk<'a> {
    /// The desk for a call, from the app's parts.
    pub(super) fn of(app: &'a Arc<Inner>) -> Self {
        Self {
            life: &app.life,
            host: &app.host,
            store: &app.saver.store,
            core: &app.core,
            updates: &app.updates,
            app,
        }
    }

    /// **What the window has now, as one number:** the session as the record would have it, the appearance the person chose, and any file being opened. What
    /// counts and what doesn't is said in [`crate::lifecycle::keeping`]: the sidebar shown or hidden is the window's arrangement, so it's left out here, with the
    /// window's frame, focus, the sidebar's width, the panel and Settings, which aren't in the session's record at all. Reads the session and the window.
    fn as_done(&self, window: &Window, reads: &FileReads) -> u64 {
        let mut record = self.host.session.with_session(|session| session.to_record());
        record.view.sidebar_hidden = false;
        let appearance = lock(&window.app).appearance;
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
        let appearance_then = lock(&window.app).appearance;
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

/// **A try at the record's lock, on a thread of its own:** the store's one `try_lock` (an open of the lock file, which a sleeping disk can make slow), told to the
/// session thread, which decides; with the lock, this thread then reads the saved session if it's told to, or ends, the session thread having given the lock
/// back itself.
fn try_the_lock(inner: &Arc<Inner>, verdict: std::sync::mpsc::Sender<Verdict>, heard: &std::sync::mpsc::Receiver<Verdict>) {
    let got = inner.saver.store.try_to_keep();
    recto_platform::slow::pause(recto_platform::slow::Seam::RecordLock);
    inner.core.push(CoreCall::TryBack(TryBack { got, verdict }));
    if !got {
        return;
    }
    // Nothing heard: the session thread gave the lock back, or ended (the app's going, and the process takes the lock with it).
    if let Ok(Verdict::Read(appearance_then)) = heard.recv() {
        read_late(inner, appearance_then);
    }
}

/// **A late try's saved session read,** on the try's thread, and handed to the session thread, which takes it in. Read first, as a launch's is, with the lock
/// held and nothing written by anyone else; then claimed, which only a restore whose patience isn't over can be; and only claimed, the updater's told this copy
/// keeps the record, and falls back from a record it can't read after an update that kept ending. Not claimed (the patience ran out while it read), it's
/// dropped, and the session thread hears this thread is out of its read and gives the lock back. A panic on the way says so too, by the guard, so the restore's
/// end never waits on a thread that's gone.
fn read_late(inner: &Arc<Inner>, appearance_then: record::Appearance) {
    struct Out<'a>(&'a Inner, bool);
    impl Drop for Out<'_> {
        fn drop(&mut self) {
            if !self.1 {
                self.0.core.push(CoreCall::LateRead(None));
            }
        }
    }
    let mut out = Out(inner, false);
    #[cfg(test)]
    assert!(!PANIC_IN_THE_LATE_READ.load(Ordering::SeqCst), "a test's panic in the late read");
    // Where a test holds the read inside it, as a slow disk does.
    recto_platform::slow::pause(recto_platform::slow::Seam::State);
    let misreads = inner.life.update_facts.misreads();
    let loaded = if misreads { Loaded::Nothing } else { inner.saver.store.load() };
    let _ = Desk::of(inner).step(KeepingEvent::Claimed);
    if !inner.life.keeping().claims() {
        return;
    }
    if inner.life.update_facts.tell(&inner.updates) == recto_update::updater::AtLaunch::Exit {
        std::process::exit(0);
    }
    let loaded = updates::fall_back_if_unreadable(&inner.updates, &inner.saver.store, loaded);
    updates::note_tabs_read(&loaded);
    out.1 = true;
    inner.core.push(CoreCall::LateRead(Some(Box::new((loaded, appearance_then)))));
}

/// A test's read that panics, in its own process (each test is one).
#[cfg(test)]
static PANIC_IN_THE_LATE_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Makes the late keep's read panic on its thread, for a test of what ends the restore then.
#[cfg(test)]
pub(crate) fn panic_in_the_late_read() {
    PANIC_IN_THE_LATE_READ.store(true, Ordering::SeqCst);
}

/// What the session thread decided about a try that came back with the lock, for the try's thread: the one thing it does with it. A lock that isn't kept is
/// given back by the session thread itself, and the try's thread is told nothing.
#[derive(Debug)]
pub enum Verdict {
    /// Keep it: read the saved session, as a launch does. With the appearance the window had then, so one the person chooses while it's read is kept
    /// ([`window_as_it_is`]).
    Read(record::Appearance),
}

/// A try back from its thread: whether it got the lock, and where the decision about it goes.
#[derive(Debug)]
pub struct TryBack {
    got: bool,
    verdict: std::sync::mpsc::Sender<Verdict>,
}

impl TryBack {
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

/// **Whether a writer of the record may write now:** this copy holds the record's lock and keeps the record, from the launch or since a late try's saved session
/// is in. Not while it's trying (a try may be back with the lock and not yet decided on), restoring (what it would gather is the window before the restore), or
/// once it's settled not to keep it. Looked at by every writer before it takes the saver's gate, so one that can't write never waits for it, and again under it.
/// **Keeping the record with the store holding no lock** isn't skipped in silence: the writer steps the machine to "not kept", which says so (the belt for the
/// third read's 3).
pub(crate) fn may_write(inner: &Inner) -> bool {
    let keeping = inner.life.keeping();
    if !keeping.writes() {
        return false;
    }
    if inner.saver.store.holds_the_record() {
        return true;
    }
    if keeping == Keeping::Keeps
        && let Some(inner) = inner.saver.inner()
    {
        let _ = Desk::of(&inner).step(KeepingEvent::LockLost);
    }
    false
}

/// **The one question a sleep file's writer asks of the record, and nothing else:** [`may_write`], asked as each write is about to go. The sleeper's writes
/// take this and not any closure, so none can be made without asking; it's built only by [`asks`].
pub(crate) struct KeepsRecord<'a>(&'a Inner);

impl KeepsRecord<'_> {
    /// Whether this copy may write now ([`may_write`]).
    pub(crate) fn may_write(&self) -> bool {
        may_write(self.0)
    }
}

/// The question, for the sleeper's writes ([`KeepsRecord`]).
pub(crate) fn asks(inner: &Inner) -> KeepsRecord<'_> {
    KeepsRecord(inner)
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
        let asking = Arc::clone(self.desk.app);
        let sent = std::thread::Builder::new()
            .name("recto-record-lock".into())
            .stack_size(512 * 1024)
            .spawn(move || try_the_lock(&asking, verdict, &heard));
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

/// **The window as it is, kept until it's made again** from a late keep's restore (`apply_restored`): its frame, focus, sidebar width and Settings, which the
/// restore put in `from_state` for the window made again ([`Inner::window_replaced`]), so if the person touches it first and it isn't made again, the record says
/// what the real window is. And the appearance, if the person chose one while the saved session was read (it isn't `appearance_then`, what it was as the try
/// was kept): what they chose last wins over what was saved, in the record and in the window made again.
pub(super) fn window_as_it_is(app: &mut AppState, real: &AppState, appearance_then: Option<record::Appearance>) {
    if appearance_then.is_some_and(|then| then != real.appearance) {
        app.appearance = real.appearance;
        if let Some(state) = app.from_state.as_mut() {
            state.appearance = real.appearance;
        }
    }
    app.window.clone_from(&real.window);
    app.focus = real.focus;
    app.sidebar_width = real.sidebar_width;
    app.settings_window = real.settings_window;
}

#[cfg(test)]
mod tests {
    /// Failure (R3.7, S3: the desk's rule): code outside the desk steps keeping the record by itself (a `life.step(Event::Keeping(..))` or a `tried` of its
    /// own), so what the step comes to is never carried out: a lock a try brought kept, keeping settled with nothing said, or the updater never told of a launch
    /// held back. Outside the pure machine (`lifecycle/`, which makes them in its tests), a keeping event is made only in this file, by the desk, and in the
    /// life's `tried`, which only the desk calls. Comments aside.
    #[test]
    fn a_keeping_event_is_made_only_by_its_desk() {
        fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("the sources").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !path.ends_with("lifecycle") {
                        walk(&path, found);
                    }
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let name = path.to_string_lossy().replace('\\', "/");
                for (n, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    let makes = code.contains("Event::Keeping(") || code.contains(".tried(");
                    let allowed = name.ends_with("src/core/keeping.rs") || (name.ends_with("src/core/life.rs") && code.contains("Event::Keeping("));
                    if makes && !allowed {
                        found.push(format!("{name}:{}: {}", n + 1, line.trim()));
                    }
                }
            }
        }
        let mut found = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        assert!(found.is_empty(), "keeping stepped outside its desk: {found:#?}");
    }
}
