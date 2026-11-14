//! **The record's keeper: the saved state, as this copy keeps it** (R3.7, S5). Reading it at the launch and once a late try has its lock, the try itself, and the one
//! rule of who may write it ([`Keeper::may_write`]): every writer of the record, its journals and the sleep files goes by that, and nothing else says it.
//!
//! **What it is:** the handles the record's work reads, owned, so the threads that do that work (the launch's read, a late try and its read, the sleep files'
//! writer) hold the keeper and not the app. The store is the record's own (the same one the saver writes through); the life holds the state of keeping it
//! (`lifecycle::keeping`, stepped only by the keeping desk); the host has the session and the notice the desk reads and sets; the queue is where what's read
//! goes; the updater is told what keeping it came to. It holds no lock, thread or buffer of its own, and nothing that holds the app or itself, so it goes
//! with the last thread that holds it.
//!
//! **What it hands out,** each with one constructor here: [`KeepsRecord`] (the question a sleep file's writer asks, by [`Keeper::asks`]) and the keeping
//! desk ([`Keeper::desk`], which carries out the machine: [`super::keeping`]).

use super::keeping::{Desk, TryBack, Verdict};
use super::{AppHost, CoreCall, CoreQueue, Lifecycle, updates};
use recto_control::record_store::{Loaded, RecordStore, StoreError};
use recto_session::record;
use std::sync::Arc;

/// **The record's keeper, the record's part of the app.** Its fields are private and it's made once, by [`Keeper::new`], as the app starts.
pub struct Keeper {
    store: Arc<RecordStore>,
    life: Arc<Lifecycle>,
    host: Arc<AppHost>,
    core: Arc<CoreQueue>,
    updates: Arc<recto_update::updater::Updater>,
}

impl Keeper {
    /// The record's part, from the app's handles: the store the saver writes through, the life, the host, the queue and the updater.
    pub(super) fn new(
        store: Arc<RecordStore>,
        life: Arc<Lifecycle>,
        host: Arc<AppHost>,
        core: Arc<CoreQueue>,
        updates: Arc<recto_update::updater::Updater>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            life,
            host,
            core,
            updates,
        })
    }

    /// The session the record is gathered from: what the journals written now are taken from ([`super::saver::Saver::write_journals_now`]).
    pub(super) fn session(&self) -> &recto_control::host::SessionHost<recto_agent::DrawableLoader> {
        &self.host.session
    }

    /// The record's store: what the sleep files' writer writes through.
    pub(super) fn store(&self) -> &Arc<RecordStore> {
        &self.store
    }

    /// **The keeping desk for a call** ([`super::keeping`]): the only way one is made.
    pub(super) fn desk(self: &Arc<Self>) -> Desk<'_> {
        Desk::new(&self.life, &self.host, &self.store, &self.core, &self.updates, self)
    }

    /// Reads the state, on a thread of its own, and hears `restore_ready` when it's applied. It's what the window waits for, up to `restore_patience_seconds`:
    /// its frame and the place on it come from what's read.
    pub(super) fn begin_restore(self: &Arc<Self>) {
        let keeper = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("recto-restore".into())
            .stack_size(1024 * 1024)
            .spawn(move || keeper.read_state());
        if spawned.is_err() {
            self.core.push(CoreCall::Restored(Box::new(Loaded::Nothing)));
        }
    }

    /// **The state read and handed to the session thread,** on a thread that can wait: at the launch ([`Self::begin_restore`]), and once a late try has got the
    /// record's lock ([`Self::read_late`]), the one way the state is read for a launch. **The lock is asked for once:** one wait of the store's patience, whose
    /// answer the updater is told ([`updates::LaunchFacts::hold_or_tell`]) and the read goes by, so a lock that comes after the wait can't be got by the read
    /// with the updater still held back (the review's blocker of 2026-11-13); it's the late try's to take. The updater's told of a copy that keeps the record
    /// before anything's read, so a version that keeps failing goes back here and not by reading on.
    fn read_state(&self) {
        let loaded = self.read();
        self.core.push(CoreCall::Restored(Box::new(loaded)));
    }

    /// The launch's state, read as [`Self::read_state`] says. A late try's read is its own ([`Self::read_late`]): it tells the updater after it reads, once it
    /// has claimed the record, so a read that runs past its patience is a second copy's.
    fn read(&self) -> Loaded {
        let misreads = self.life.update_facts.misreads();
        let keeps = self.store.wait_for_lock();
        if self.life.update_facts.hold_or_tell(keeps, &self.updates) == recto_update::updater::AtLaunch::Exit {
            std::process::exit(0);
        }
        recto_platform::slow::pause(recto_platform::slow::Seam::State);
        let loaded = if !keeps {
            Loaded::OtherInstance
        } else if misreads {
            Loaded::Nothing
        } else {
            self.store.load()
        };
        let loaded = updates::fall_back_if_unreadable(&self.updates, &self.store, loaded);
        updates::note_tabs_read(&loaded);
        loaded
    }

    /// **A try at the record's lock, on a thread of its own:** the store's one `try_lock` (an open of the lock file, which a sleeping disk can make slow), told to
    /// the session thread, which decides; with the lock, this thread then reads the saved session if it's told to, or ends, the session thread having given the
    /// lock back itself.
    pub(super) fn try_the_lock(self: &Arc<Self>, verdict: std::sync::mpsc::Sender<Verdict>, heard: &std::sync::mpsc::Receiver<Verdict>) {
        let got = self.store.try_to_keep();
        recto_platform::slow::pause(recto_platform::slow::Seam::RecordLock);
        self.core.push(CoreCall::TryBack(TryBack::new(got, verdict)));
        if !got {
            return;
        }
        // Nothing heard: the session thread gave the lock back, or ended (the app's going, and the process takes the lock with it).
        if let Ok(Verdict::Read(appearance_then)) = heard.recv() {
            self.read_late(appearance_then);
        }
    }

    /// **A late try's saved session read,** on the try's thread, and handed to the session thread, which takes it in. Read first, as a launch's is, with the lock
    /// held and nothing written by anyone else; then claimed, which only a restore whose patience isn't over can be; and only claimed, the updater's told this
    /// copy keeps the record, and falls back from a record it can't read after an update that kept ending. Not claimed (the patience ran out while it read),
    /// it's dropped, and the session thread hears this thread is out of its read and gives the lock back. A panic on the way says so too, by the guard, so the
    /// restore's end never waits on a thread that's gone.
    fn read_late(self: &Arc<Self>, appearance_then: record::Appearance) {
        struct Out<'a>(&'a CoreQueue, bool);
        impl Drop for Out<'_> {
            fn drop(&mut self) {
                if !self.1 {
                    self.0.push(CoreCall::LateRead(None));
                }
            }
        }
        let mut out = Out(&self.core, false);
        #[cfg(test)]
        assert!(
            !PANIC_IN_THE_LATE_READ.load(std::sync::atomic::Ordering::SeqCst),
            "a test's panic in the late read"
        );
        // Where a test holds the read inside it, as a slow disk does.
        recto_platform::slow::pause(recto_platform::slow::Seam::State);
        let misreads = self.life.update_facts.misreads();
        let loaded = if misreads { Loaded::Nothing } else { self.store.load() };
        // The app ended while this read (a process's end, the headless kit's kill, which lets the store go): it's told nothing more, so nothing's claimed.
        if self.store.has_let_go() {
            return;
        }
        self.desk().claimed();
        if !self.life.keeping().claims() {
            return;
        }
        if self.life.update_facts.tell(&self.updates) == recto_update::updater::AtLaunch::Exit {
            std::process::exit(0);
        }
        let loaded = updates::fall_back_if_unreadable(&self.updates, &self.store, loaded);
        updates::note_tabs_read(&loaded);
        out.1 = true;
        self.core.push(CoreCall::LateRead(Some(Box::new((loaded, appearance_then)))));
    }

    /// **Whether a writer of the record may write now:** this copy holds the record's lock and keeps the record, from the launch or since a late try's saved
    /// session is in. Not while it's trying (a try may be back with the lock and not yet decided on), restoring (what it would gather is the window before the
    /// restore), or once it's settled not to keep it. Looked at by every writer before it takes the saver's gate, so one that can't write never waits for it,
    /// and again under it. **Keeping the record with the store holding no lock** isn't skipped in silence: the writer steps the machine to "not kept", which
    /// says so (the belt for the third read's 3).
    pub(crate) fn may_write(self: &Arc<Self>) -> bool {
        let keeping = self.life.keeping();
        if !keeping.writes() {
            return false;
        }
        if self.store.holds_the_record() {
            return true;
        }
        // An ended app's straggler (its store let go, as a process's end lets it go) tells the app nothing more: no "not kept", no notice.
        if keeping == crate::lifecycle::keeping::Keeping::Keeps && !self.store.has_let_go() {
            self.desk().lock_lost();
        }
        false
    }

    /// Why a flush can't write now, if it can't: see [`Self::may_write`]. The saved session being read back after a late try at the lock isn't written over by
    /// the window before it; a copy that doesn't keep the record says so as the store does.
    pub(super) fn refused(self: &Arc<Self>) -> Option<StoreError> {
        if self.may_write() {
            return None;
        }
        Some(if self.life.keeping().refuses_agents_changes() {
            StoreError {
                code: "not_ready",
                message: "The saved session is being read back in, so nothing's written over it until it's in.".to_string(),
            }
        } else {
            StoreError {
                code: "other_instance",
                message: record::notices::words(record::notices::OTHER_INSTANCE).unwrap_or_default().to_string(),
            }
        })
    }

    /// The question, for the sleeper's writes ([`KeepsRecord`]): the one way one's made.
    pub(crate) fn asks(self: &Arc<Self>) -> KeepsRecord<'_> {
        KeepsRecord(self)
    }
}

/// **The one question a sleep file's writer asks of the record, and nothing else:** [`Keeper::may_write`], asked as each write is about to go. The sleeper's
/// writes take this and not any closure, so none can be made without asking; its field is private and it's built only by [`Keeper::asks`].
pub(crate) struct KeepsRecord<'a>(&'a Arc<Keeper>);

impl KeepsRecord<'_> {
    /// Whether this copy may write now ([`Keeper::may_write`]).
    pub(crate) fn may_write(&self) -> bool {
        self.0.may_write()
    }
}

/// A test's read that panics, in its own process (each test is one).
#[cfg(test)]
static PANIC_IN_THE_LATE_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Makes the late keep's read panic on its thread, for a test of what ends the restore then.
#[cfg(test)]
pub(crate) fn panic_in_the_late_read() {
    PANIC_IN_THE_LATE_READ.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every Rust file of the crate's sources but the core's headless tests, with its path from `src/` and its lines that aren't comments, up to its tests.
    fn sources() -> Vec<(String, Vec<String>)> {
        fn walk(folder: &Path, src: &Path, found: &mut Vec<(String, Vec<String>)>) {
            for entry in std::fs::read_dir(folder).expect("a source folder").flatten() {
                let path = entry.path();
                // The core's tests on the headless kit name what they look for (the listing of what other code can reach among it).
                if path.is_dir() && !path.ends_with("core/headless") {
                    walk(&path, src, found);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).expect("a source file");
                    // A file's own tests, at its end, name what they look for: they aren't code that says or asks it.
                    let lines = text
                        .lines()
                        .take_while(|line| line.trim() != "mod tests {")
                        .filter(|line| !line.trim_start().starts_with("//"))
                        .map(str::to_owned)
                        .collect();
                    found.push((path.strip_prefix(src).expect("under src").display().to_string(), lines));
                }
            }
        }
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        walk(&src, &src, &mut found);
        found
    }

    /// Where `what` is written in the crate's code, by file and the times in each.
    fn written(what: &str) -> Vec<(String, usize)> {
        sources()
            .into_iter()
            .filter_map(|(file, lines)| {
                let times: usize = lines.iter().map(|line| line.matches(what).count()).sum();
                (times > 0).then_some((file, times))
            })
            .collect()
    }

    /// **Failure: who may write the record is said a second time, or asked another way.** A writer that decides for itself (its own `may_write`, a copy of
    /// the rule beside the one here) can drift from it, and a copy that doesn't keep the record, or doesn't yet, writes over the saved session; a sleep file's
    /// writer handed a `KeepsRecord` made some other way than by asking the keeper writes by whatever that says. The rule is defined here and only here (the
    /// keeper's, and the question's that asks it); the question is made only by [`super::Keeper::asks`]; and the keeping desk, whose carrier is the one way the
    /// rule's "not kept" is stepped, only by [`super::Keeper::desk`]; the two keeping steps the keeper's threads make (a claim, a lost lock) only by the keeper;
    /// and the sleep files' writer's start only by `OnDisk::starts_writer`.
    #[test]
    fn who_may_write_the_record_is_said_once_and_asked_only_of_the_keeper() {
        assert_eq!(written("fn may_write("), [("core/keeper.rs".to_owned(), 2)], "where the rule is said");
        assert_eq!(written("KeepsRecord("), [("core/keeper.rs".to_owned(), 1)], "where the question is made");
        assert_eq!(written("Desk::new("), [("core/keeper.rs".to_owned(), 1)], "where the desk is made");
        // The two keeping steps the record's own threads make, each only where the keeper decides it (its read's claim, a writer's lost lock).
        assert_eq!(written(".claimed("), [("core/keeper.rs".to_owned(), 1)], "where a late keep's read is claimed");
        assert_eq!(written(".lock_lost("), [("core/keeper.rs".to_owned(), 1)], "where a lost lock is stepped");
        // The sleep files' writer's start, made only by its one constructor.
        assert_eq!(
            written("StartsWriter {"),
            [("core/sleeper.rs".to_owned(), 1)],
            "where the writer's start is made"
        );
    }
}
