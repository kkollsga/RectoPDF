//! **The app's state on its way to disk.** SPEC decision 25: state goes to disk by what happened, not on a timer. Edits go in their journals after 2 s of quiet or every 5 s at most during a burst. Places and the like at most once a minute, and only if they've changed. Going to the background, quitting and a hand-off don't wait.
//!
//! **No thread at rest, and none waiting.** Saying something's changed is an atomic store, and the first time a nudge to the session thread, which is parked anyway. It keeps the one deadline there is, by parking with a timeout rather than for good, so there's nothing per tab and no timer. When it comes due, a one-shot thread does the writing and ends. So a scroll on the UI thread waits for nothing, and a window left reading has no extra thread for its minute.

use super::{CoreCall, CoreQueue, Inner, lock};
use crate::clock::Clock;
use crate::saver as app;
use recto_control::record_store::{FlushReport, RecordStore, StoreError};
use recto_session::JournalWork;
use recto_session::record::{EditsRecord, EditsSummary, PlaceDelta, SessionRecord};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

pub struct Saver {
    pub store: Arc<RecordStore>,
    core: Arc<CoreQueue>,
    inner: OnceLock<Weak<Inner>>,
    /// The record's been read, so what's written is what came of it. Nothing's written before that.
    started: AtomicBool,
    /// What `crate::saver`'s decisions go by: when edits and the record come due. The host's clock, a test's fake one. How long a flush has, and how long its steps took,
    /// are real time and go by `Instant`, as a wait on the disk is.
    clock: Arc<dyn Clock>,
    /// A writing thread is there, or asked for.
    writer_alive: AtomicBool,
    /// The session thread knows there's a deadline. So saying it again is one atomic.
    armed: AtomicBool,
    view_changed: AtomicBool,
    last_record_write: AtomicU64,
    urgent: AtomicBool,
    /// In milliseconds of `clock`, plus one, and zero for none.
    first_edit: AtomicU64,
    last_edit: AtomicU64,
    /// One pass at the disk at a time: held by every writer of the record from gathering what it writes to writing it, and taken only by one that may write
    /// (`keeping::may_write`, looked at before and again under it), so a writer either writes nothing or gathers the session this copy keeps.
    pub(super) gate: Mutex<()>,
    last_error: Mutex<Option<StoreError>>,
}

impl Saver {
    pub fn new(store: RecordStore, core: Arc<CoreQueue>, clock: Arc<dyn Clock>) -> Self {
        Self {
            store: Arc::new(store),
            core,
            inner: OnceLock::new(),
            started: AtomicBool::new(false),
            clock,
            writer_alive: AtomicBool::new(false),
            armed: AtomicBool::new(false),
            view_changed: AtomicBool::new(false),
            last_record_write: AtomicU64::new(0),
            urgent: AtomicBool::new(false),
            first_edit: AtomicU64::new(0),
            last_edit: AtomicU64::new(0),
            gate: Mutex::new(()),
            last_error: Mutex::new(None),
        }
    }

    pub fn inner(&self) -> Option<Arc<Inner>> {
        self.inner.get().and_then(Weak::upgrade)
    }

    pub fn attach(&self, inner: Weak<Inner>) {
        let _ = self.inner.set(inner);
    }

    /// Writing may begin.
    pub fn start(&self) {
        self.last_record_write.store(self.now(), Ordering::SeqCst);
        self.started.store(true, Ordering::SeqCst);
        self.nudge();
    }

    fn now(&self) -> u64 {
        self.clock.now_ms() + 1
    }

    /// A place, or something else in the record, has changed. Cheap enough for each scroll.
    pub fn view_changed(&self) {
        self.view_changed.store(true, Ordering::Relaxed);
        if !self.armed.swap(true, Ordering::SeqCst) {
            self.nudge();
        }
    }

    /// An edit has been made. It may be due sooner than what's waited for, so the session thread looks again.
    pub fn edit_made(&self) {
        let now = self.now();
        let _ = self.first_edit.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
        self.last_edit.store(now, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
        self.nudge();
    }

    /// What's changed shouldn't wait: the app is going to the background, or a journal has been retired.
    pub fn write_soon(&self) {
        self.urgent.store(true, Ordering::SeqCst);
        self.view_changed.store(true, Ordering::Relaxed);
        self.armed.store(true, Ordering::SeqCst);
        self.nudge();
    }

    /// The app's gone to the background: what's changed is written now ([`Self::write_soon`]), and the sleep files' cool-down hurried with it
    /// (`Sleeper::hurry_leaving_soon`), stored before `urgent` as [`take_urgent_then_hurry`] needs. The UI thread's: atomic stores and a weak reference
    /// taken up, no lock.
    pub fn went_to_background(&self) {
        if let Some(inner) = self.inner() {
            inner.sleeper.hurry_leaving_soon();
        }
        self.write_soon();
    }

    fn nudge(&self) {
        self.core.nudge();
    }

    /// How long until something's due, or `None` with nothing to do or a writer on it. What the session thread waits for, alongside its queue.
    pub fn wait_for_due(&self) -> Option<Duration> {
        if !self.started.load(Ordering::SeqCst) || self.writer_alive.load(Ordering::SeqCst) {
            return None;
        }
        let due = self.next_due();
        if due.is_none() {
            self.armed.store(false, Ordering::SeqCst);
            // Something may have said so just then.
            return self.next_due();
        }
        due
    }

    /// What's been asked of the writer since [`Self::forget_asked`]: a write at once ([`Self::write_soon`]), one in its own time ([`Self::view_changed`]), or
    /// nothing. For a test that holds the session thread, so that nothing else asks or writes meanwhile.
    #[cfg(test)]
    pub(super) fn asked(&self) -> &'static str {
        if self.urgent.load(Ordering::SeqCst) {
            "at once"
        } else if self.view_changed.load(Ordering::Relaxed) {
            "in its time"
        } else {
            "nothing"
        }
    }

    /// Forgets what's been asked of the writer, for [`Self::asked`]; what was owed goes unwritten. A test's, with the session thread held.
    #[cfg(test)]
    pub(super) fn forget_asked(&self) {
        self.urgent.store(false, Ordering::SeqCst);
        self.view_changed.store(false, Ordering::Relaxed);
    }

    /// It's come due. A one-shot thread does the writing, so that nothing else waits on the disk. On the session thread.
    pub fn spawn_writer(self: &Arc<Self>) {
        if self.writer_alive.swap(true, Ordering::SeqCst) {
            return;
        }
        let saver = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("recto-save".into()).stack_size(512 * 1024).spawn(move || {
            saver.run();
            saver.writer_alive.store(false, Ordering::SeqCst);
            saver.nudge();
        });
        if spawned.is_err() {
            self.writer_alive.store(false, Ordering::SeqCst);
        }
    }

    /// What's waiting, as the atomics say now, with the urgency given.
    fn pending(&self, urgent: bool) -> app::Pending {
        app::Pending {
            first_edit: self.first_edit.load(Ordering::SeqCst),
            last_edit: self.last_edit.load(Ordering::SeqCst),
            urgent,
            view_changed: self.view_changed.load(Ordering::Relaxed),
            last_record_write: self.last_record_write.load(Ordering::SeqCst),
        }
    }

    /// How long until something's due, or `None` with nothing to write: `crate::saver::next_due`.
    fn next_due(&self) -> Option<Duration> {
        let now = self.now();
        app::next_due(self.pending(self.urgent.load(Ordering::SeqCst)), now).map(Duration::from_millis)
    }

    fn run(&self) {
        while self.next_due().is_some_and(|wait| wait.is_zero()) {
            self.pass();
        }
    }

    /// One visit to the disk: whichever of journals and the record are due.
    fn pass(&self) {
        let now = self.now();
        let Some(inner) = self.inner() else {
            self.first_edit.store(0, Ordering::SeqCst);
            self.view_changed.store(false, Ordering::Relaxed);
            self.urgent.store(false, Ordering::SeqCst);
            return;
        };
        // Going to the background's hurry of the sleep files' cool-down, carried out here, off the UI thread, with `urgent` taken first.
        let urgent = take_urgent_then_hurry(&self.urgent, || inner.sleeper.hurry_leaving_if_asked(|| inner.start_sleep_writer()));
        let due = app::pass(self.pending(urgent), now);
        // A copy that doesn't keep the record writes nothing, and that's what it's for, not a failure: the window says it won't be remembered, and not that its
        // state couldn't be saved. Nor does one still trying for the lock, or whose saved session is being read back after a late try: what it would gather is
        // the window before the restore (`keeping::may_write`). Looked at before the gate, so it never waits for it, and again under it, where it holds from
        // gathering to writing. What's waiting goes, so nothing comes due again for it.
        let gate = super::keeping::may_write(&inner).then(|| lock(&self.gate));
        if gate.is_none() || !super::keeping::may_write(&inner) {
            self.first_edit.store(0, Ordering::SeqCst);
            self.view_changed.store(false, Ordering::Relaxed);
            self.last_record_write.store(now, Ordering::SeqCst);
            return;
        }
        let mut failed = None;
        if due.journals {
            self.first_edit.store(0, Ordering::SeqCst);
            failed = self.write_journals(&inner, false);
        }
        if due.record {
            self.view_changed.store(false, Ordering::Relaxed);
            self.last_record_write.store(self.now(), Ordering::SeqCst);
            let (record, delta) = inner.gather_record();
            recto_platform::slow::pause(recto_platform::slow::Seam::RecordWrite);
            if let Err(error) = self.store.write_record(&record, &delta, false) {
                self.store.next_write_whole();
                failed = Some(error);
            }
        }
        self.note_outcome(&inner, failed);
    }

    /// What's still to do after a failed write waits for its next due time, and the reader's told.
    fn note_outcome(&self, inner: &Inner, failed: Option<StoreError>) {
        let was = lock(&self.last_error).is_some();
        let now = failed.is_some();
        if let Some(error) = failed {
            *lock(&self.last_error) = Some(error);
            self.view_changed.store(true, Ordering::Relaxed);
            let now = self.now();
            let _ = self.first_edit.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
            self.last_edit.store(now, Ordering::SeqCst);
        } else {
            *lock(&self.last_error) = None;
        }
        if was != now {
            inner.core.push(CoreCall::Reconcile);
        }
    }

    /// Whether a writing thread is there, or asked for: started, or owed, a write being due now (as after [`Self::write_soon`]) that the session thread
    /// starts a writer for when it next looks at its deadline, which it does only once its queue is empty.
    pub fn is_writing(&self) -> bool {
        self.writer_alive.load(Ordering::SeqCst) || (self.started.load(Ordering::SeqCst) && self.next_due().is_some_and(|wait| wait.is_zero()))
    }

    pub fn has_failed(&self) -> bool {
        lock(&self.last_error).is_some()
    }

    /// What the last visit to the disk said went wrong, if it did.
    pub fn last_error(&self) -> Option<StoreError> {
        lock(&self.last_error).clone()
    }

    /// Takes what's waiting and writes it, and tells the session what got there. What didn't stays waiting, and only that goes next time. Says the first thing that went wrong.
    fn write_journals(&self, inner: &Inner, force: bool) -> Option<StoreError> {
        let work = inner.host.session.with_session(|session| session.journal_desk().work());
        self.write_work(inner, &work, force)
    }

    /// Whatever's waiting goes in its journal now, as a tab's about to go dormant, whose list can't till it's all there. With the gate, so it's one visit to the disk at a time;
    /// and by the rule every writer goes by ([`super::keeping::may_write`]), before the gate and under it.
    pub fn write_journals_now(&self, inner: &Inner) {
        if !super::keeping::may_write(inner) {
            return;
        }
        let _gate = lock(&self.gate);
        if super::keeping::may_write(inner) {
            let _ = self.write_journals(inner, false);
        }
    }

    fn write_work(&self, inner: &Inner, work: &JournalWork, force: bool) -> Option<StoreError> {
        if work.retired.is_empty() && work.appends.is_empty() {
            return None;
        }
        // Sleep files about to be removed: nothing queued for them is written after.
        inner.sleeper.retire(&work.retired);
        let (outcome, failed) = self.store.write_journals(work, force);
        let written: Vec<(recto_session::TabId, String)> = outcome.written.iter().map(|(id, _, name)| (*id, name.clone())).collect();
        inner.host.session.with_session(|session| session.journal_desk().outcome(outcome));
        for (id, name) in written {
            if self.store.should_compact(&name) {
                let checkpoint = inner.host.session.with_session(|session| session.journal_look().checkpoint_if_caught_up(id));
                if let Some((header, base)) = checkpoint {
                    let _ = self.store.compact(&name, &header, &base);
                }
            }
        }
        failed
    }

    /// **Writes everything now, reads it back, and says whether it's all there.** What's waiting and what's in memory are taken in one look, so an edit made in between can't be in one and not the other. `force` makes the drive itself flush everything a launch would read, older writes included, which only a hand-off needs. Waits for the session thread and the disk, so it's for a thread that can.
    pub fn flush(&self, force: bool, limit: Duration) -> Result<FlushReport, StoreError> {
        self.flush_by(force, limit, None)
    }

    /// **The same, asked for by `asked_by`** (`session save`): an agent's leaves the sleep files' cool-down alone, as a quit's and a hand-off's don't, and what
    /// came of the tab on screen's sleep file is kept under its ticket ([`Inner::sleep_at_flush`]).
    pub fn flush_by(&self, force: bool, limit: Duration, asked_by: Option<crate::sleeper::Asked>) -> Result<FlushReport, StoreError> {
        let Some(inner) = self.inner() else {
            return Err(StoreError {
                code: "not_ready",
                message: "The app is going away.".to_string(),
            });
        };
        if !self.started.load(Ordering::SeqCst) {
            return Err(StoreError {
                code: "not_ready",
                message: "The app hasn't finished starting.".to_string(),
            });
        }
        // Real time, not the clock: how long this flush may wait on the session thread and the disk, and how long its steps took.
        let began = Instant::now();
        // A hand-off can wait, and an update mustn't go on something unfinished. A quit is going anyway, so it writes what there is.
        if !inner.core.flush(limit) && force {
            return Err(StoreError {
                code: "still_busy",
                message: "Something the app was asked to do hasn't finished, so its state can't be said to be whole.".to_string(),
            });
        }
        // Before the gate and under it, as `pass` decides (`keeping::may_write`): a copy that doesn't keep the record, or doesn't yet, writes nothing.
        if let Some(refused) = refused(&inner) {
            return Err(refused);
        }
        let _gate = lock(&self.gate);
        if let Some(refused) = refused(&inner) {
            return Err(refused);
        }
        self.first_edit.store(0, Ordering::SeqCst);
        self.view_changed.store(false, Ordering::Relaxed);
        self.urgent.store(false, Ordering::SeqCst);
        let gathered = Instant::now();
        let before = self.store.wrote_bytes();
        let (work, record, delta, lists) = inner.gather_for_flush();
        let gather_us = micros(gathered);
        recto_platform::slow::pause(recto_platform::slow::Seam::RecordWrite);
        let written = Instant::now();
        if let Some(error) = self.write_work(&inner, &work, false) {
            return Err(error);
        }
        // The journals' names can have changed in that, if something was in the way.
        let named = inner.with_journal_names(record.clone());
        let renamed = named != record;
        let record = named;
        self.store.write_record(&record, &delta, false)?;
        let mut force_us = 0;
        if force {
            let forcing = Instant::now();
            self.store.force_to_drive()?;
            force_us = micros(forcing);
        }
        let write_us = micros(written);
        let verified = Instant::now();
        // Only if a name had to change is it asked again, as an edit since would differ from what was just written, and a hand-off has stopped those anyway.
        let lists = if renamed {
            inner.host.session.with_session(|session| session.journal_look().lists_by_journal())
        } else {
            lists
        };
        let (asleep, edited_tabs): (HashMap<String, EditsSummary>, usize) = inner
            .host
            .session
            .with_session(|session| (session.dormant().asleep_by_journal(), session.lists().edited_tabs()));
        if let Err(first) = self.store.verify_with_asleep(&record, &lists, &asleep) {
            // A line that didn't say enough is mended by the whole record. Anything else fails.
            self.store.next_write_whole();
            self.store.write_record(&record, &delta, force)?;
            self.store.verify_with_asleep(&record, &lists, &asleep).map_err(|_| first)?;
        }
        let verify_us = micros(verified);
        // The tab on screen's sleep file, for the next launch's first frame. Nothing depends on it, so it's no part of what's read back or forced to the drive, and it comes after the state: with sleep files queued for a slow disk, a quit's 5 s ran out waiting here, and it went without the state (slow mode, 2026-11-11). It waits for others being written for what's left of this flush's time, and past that is skipped and counted. If nothing's changed, nothing's written.
        // Then what's waiting in the cool-down, hurried and waited for within what's left of the time ([`Inner::sleep_at_flush`]).
        inner.sleeper.hurry_leaving_if_asked(|| inner.start_sleep_writer());
        inner.sleep_at_flush(asked_by, began + limit);
        let (record_bytes, journal_bytes) = self.store.sizes();
        self.last_record_write.store(self.now(), Ordering::SeqCst);
        *lock(&self.last_error) = None;
        Ok(FlushReport {
            tabs: record.tabs.len(),
            edited_tabs,
            record_bytes,
            journal_bytes,
            wrote_bytes: self.store.wrote_bytes() - before,
            gather_us,
            write_us,
            force_us,
            verify_us,
            total_us: micros(began),
        })
    }
}

/// Why a flush can't write now, if it can't: see [`super::keeping::may_write`]. The saved session being read back after a late try at the lock isn't written over
/// by the window before it; a copy that doesn't keep the record says so as the store does.
/// **Takes `urgent` for a pass, and then carries out the hurry of the sleep files' cool-down if it's asked** ([`super::sleeper`]'s
/// `hurry_leaving_if_asked`). Going to the background stores the hurry and then `urgent` (`Saver::went_to_background`), so taking them the other way round,
/// a pass that sees `urgent` sees the hurry too; the hurry first could miss one stored just after it's looked at, and take `urgent`, and the hurry would
/// wait for a pass that may not come till the next edit or a quit, or at all with no writer left.
fn take_urgent_then_hurry(urgent: &AtomicBool, hurry_if_asked: impl FnOnce()) -> bool {
    let urgent = urgent.swap(false, Ordering::SeqCst);
    hurry_if_asked();
    urgent
}

fn refused(inner: &Inner) -> Option<StoreError> {
    if super::keeping::may_write(inner) {
        return None;
    }
    Some(if inner.life.keeping().refuses_agents_changes() {
        StoreError {
            code: "not_ready",
            message: "The saved session is being read back in, so nothing's written over it until it's in.".to_string(),
        }
    } else {
        StoreError {
            code: "other_instance",
            message: recto_session::record::notices::words(recto_session::record::notices::OTHER_INSTANCE)
                .unwrap_or_default()
                .to_string(),
        }
    })
}

fn micros(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

impl Inner {
    /// The whole record as things stand, and what a line of the log would say of it. The session is held only for the copy.
    pub fn gather_record(&self) -> (SessionRecord, PlaceDelta) {
        self.take_places();
        let (mut record, delta) = self.host.session.with_session(|session| {
            let delta = session.take_place_delta(0);
            (session.to_record(), delta)
        });
        record.app = self.window.app_record(|| lock(&self.host.notice).clone());
        (record, delta)
    }

    /// What's waiting for the journals, the record, and the lists, all in one look at the session.
    fn gather_for_flush(&self) -> (JournalWork, SessionRecord, PlaceDelta, HashMap<String, EditsRecord>) {
        self.take_places();
        let (work, mut record, delta, lists) = self.host.session.with_session(|session| {
            let work = session.journal_desk().work();
            let delta = session.take_place_delta(0);
            (work, session.to_record(), delta, session.journal_look().lists_by_journal())
        });
        record.app = self.window.app_record(|| lock(&self.host.notice).clone());
        (work, record, delta, lists)
    }

    /// The record with its journals' names as they now are.
    fn with_journal_names(&self, mut record: SessionRecord) -> SessionRecord {
        let now = self.host.session.with_session(|session| session.to_record());
        for tab in &mut record.tabs {
            if let Some(same) = now.tabs.iter().find(|other| other.id == tab.id) {
                tab.journal.clone_from(&same.journal);
            }
        }
        record
    }
}

#[cfg(test)]
mod tests {
    use super::take_urgent_then_hurry;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// **Failure: a pass takes going to the background's `urgent` without its hurry,** when the UI thread's two stores (the hurry, then `urgent`) land
    /// between the pass's two reads: the sleep files' cool-down then isn't hurried by the write going to the background asked for, and waits for a pass that
    /// may not come till the next edit or a quit, or at all once the writer's gone. The UI's stores are put at each point between and after the reads.
    #[test]
    fn a_pass_that_takes_urgent_carries_out_the_hurry_stored_before_it() {
        for lands_after_the_hurry_is_looked_at in [false, true] {
            let (urgent, hurry) = (AtomicBool::new(false), AtomicBool::new(false));
            let going_to_the_background = || {
                hurry.store(true, Ordering::SeqCst);
                urgent.store(true, Ordering::SeqCst);
            };
            let mut hurried = false;
            let took_urgent = take_urgent_then_hurry(&urgent, || {
                if !lands_after_the_hurry_is_looked_at {
                    going_to_the_background();
                }
                hurried = hurry.swap(false, Ordering::SeqCst);
                if lands_after_the_hurry_is_looked_at {
                    going_to_the_background();
                }
            });
            let at = if lands_after_the_hurry_is_looked_at {
                "after the hurry's looked at"
            } else {
                "before it"
            };
            assert!(
                !took_urgent || hurried,
                "the stores landed {at}: urgent taken, and the hurry left for a pass that may not come"
            );
            assert!(
                took_urgent || urgent.load(Ordering::SeqCst),
                "the stores landed {at}: urgent stays for the next pass"
            );
            assert!(
                hurried || hurry.load(Ordering::SeqCst),
                "the stores landed {at}: the hurry stays for the pass urgent brings"
            );
        }
    }
}
