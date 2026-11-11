//! **The app's state on its way to disk.** SPEC decision 25: state goes to disk by what happened, not on a timer. Edits go in their journals after 2 s of quiet or every 5 s at most during a burst. Places and the like at most once a minute, and only if they've changed. Going to the background, quitting and a hand-off don't wait.
//!
//! **No thread at rest, and none waiting.** Saying something's changed is an atomic store, and the first time a nudge to the session thread, which is parked anyway. It keeps the one deadline there is, by parking with a timeout rather than for good, so there's nothing per tab and no timer. When it comes due, a one-shot thread does the writing and ends. So a scroll on the UI thread waits for nothing, and a window left reading has no extra thread for its minute.

use crate::{CoreCall, CoreQueue, Inner, lock};
use recto_control::record_store::{FlushReport, RecordStore, StoreError};
use recto_session::JournalWork;
use recto_session::record::{EditsRecord, EditsSummary, PlaceDelta, PlaceRecord, Selection, SessionRecord};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// How long edits wait for a pause.
const JOURNAL_QUIET: Duration = Duration::from_secs(2);
/// The longest edits wait during a burst.
const JOURNAL_LONGEST: Duration = Duration::from_secs(5);
/// The least time between writes of places and the like.
const RECORD_EVERY: Duration = Duration::from_secs(60);

pub(crate) struct Saver {
    pub(crate) store: Arc<RecordStore>,
    core: Arc<CoreQueue>,
    inner: OnceLock<Weak<Inner>>,
    /// The record's been read, so what's written is what came of it. Nothing's written before that.
    started: AtomicBool,
    clock: Instant,
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
    /// One pass at the disk at a time.
    gate: Mutex<()>,
    last_error: Mutex<Option<StoreError>>,
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl Saver {
    pub(crate) fn new(store: RecordStore, core: Arc<CoreQueue>) -> Self {
        Self {
            store: Arc::new(store),
            core,
            inner: OnceLock::new(),
            started: AtomicBool::new(false),
            clock: Instant::now(),
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

    pub(crate) fn inner(&self) -> Option<Arc<Inner>> {
        self.inner.get().and_then(Weak::upgrade)
    }

    pub(crate) fn attach(&self, inner: Weak<Inner>) {
        let _ = self.inner.set(inner);
    }

    /// Writing may begin.
    pub(crate) fn start(&self) {
        self.last_record_write.store(self.now(), Ordering::SeqCst);
        self.started.store(true, Ordering::SeqCst);
        self.nudge();
    }

    fn now(&self) -> u64 {
        millis(self.clock.elapsed()) + 1
    }

    /// A place, or something else in the record, has changed. Cheap enough for each scroll.
    pub(crate) fn view_changed(&self) {
        self.view_changed.store(true, Ordering::Relaxed);
        if !self.armed.swap(true, Ordering::SeqCst) {
            self.nudge();
        }
    }

    /// An edit has been made. It may be due sooner than what's waited for, so the session thread looks again.
    pub(crate) fn edit_made(&self) {
        let now = self.now();
        let _ = self.first_edit.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
        self.last_edit.store(now, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
        self.nudge();
    }

    /// What's changed shouldn't wait: the app is going to the background, or a journal has been retired.
    pub(crate) fn write_soon(&self) {
        self.urgent.store(true, Ordering::SeqCst);
        self.view_changed.store(true, Ordering::Relaxed);
        self.armed.store(true, Ordering::SeqCst);
        self.nudge();
    }

    fn nudge(&self) {
        self.core.nudge();
    }

    /// How long until something's due, or `None` with nothing to do or a writer on it. What the session thread waits for, alongside its queue.
    pub(crate) fn wait_for_due(&self) -> Option<Duration> {
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

    /// It's come due. A one-shot thread does the writing, so that nothing else waits on the disk. On the session thread.
    pub(crate) fn spawn_writer(self: &Arc<Self>) {
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

    /// How long until something's due, or `None` with nothing to write.
    fn next_due(&self) -> Option<Duration> {
        let now = self.now();
        let mut due = None::<u64>;
        let first = self.first_edit.load(Ordering::SeqCst);
        if first != 0 {
            let last = self.last_edit.load(Ordering::SeqCst);
            due = Some((last + millis(JOURNAL_QUIET)).min(first + millis(JOURNAL_LONGEST)));
        }
        if self.urgent.load(Ordering::SeqCst) {
            due = Some(now);
        } else if self.view_changed.load(Ordering::Relaxed) {
            let at = self.last_record_write.load(Ordering::SeqCst) + millis(RECORD_EVERY);
            due = Some(due.map_or(at, |due| due.min(at)));
        }
        due.map(|due| Duration::from_millis(due.saturating_sub(now)))
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
        let urgent = self.urgent.swap(false, Ordering::SeqCst);
        let edit_due = {
            let first = self.first_edit.load(Ordering::SeqCst);
            first != 0 && now >= (self.last_edit.load(Ordering::SeqCst) + millis(JOURNAL_QUIET)).min(first + millis(JOURNAL_LONGEST))
        };
        let record_due = self.view_changed.load(Ordering::Relaxed) && (urgent || now >= self.last_record_write.load(Ordering::SeqCst) + millis(RECORD_EVERY));
        let _gate = lock(&self.gate);
        let mut failed = None;
        if edit_due || urgent {
            self.first_edit.store(0, Ordering::SeqCst);
            failed = self.write_journals(&inner, false);
        }
        if record_due {
            self.view_changed.store(false, Ordering::Relaxed);
            self.last_record_write.store(self.now(), Ordering::SeqCst);
            let (record, delta) = inner.gather_record();
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

    pub(crate) fn has_failed(&self) -> bool {
        lock(&self.last_error).is_some()
    }

    /// Takes what's waiting and writes it, and tells the session what got there. What didn't stays waiting, and only that goes next time. Says the first thing that went wrong.
    fn write_journals(&self, inner: &Inner, force: bool) -> Option<StoreError> {
        let work = inner.host.session.with_session(recto_session::Session::journal_work);
        self.write_work(inner, &work, force)
    }

    /// Whatever's waiting goes in its journal now, as a tab's about to go dormant, whose list can't till it's all there. With the gate, so it's one visit to the disk at a time.
    pub(crate) fn write_journals_now(&self, inner: &Inner) {
        let _gate = lock(&self.gate);
        let _ = self.write_journals(inner, false);
    }

    fn write_work(&self, inner: &Inner, work: &JournalWork, force: bool) -> Option<StoreError> {
        if work.retired.is_empty() && work.appends.is_empty() {
            return None;
        }
        let (outcome, failed) = self.store.write_journals(work, force);
        let written: Vec<(recto_session::TabId, String)> = outcome.written.iter().map(|(id, _, name)| (*id, name.clone())).collect();
        inner.host.session.with_session(|session| session.journal_outcome(outcome));
        for (id, name) in written {
            if self.store.should_compact(&name) {
                let checkpoint = inner.host.session.with_session(|session| session.checkpoint_if_caught_up(id));
                if let Some((header, base)) = checkpoint {
                    let _ = self.store.compact(&name, &header, &base);
                }
            }
        }
        failed
    }

    /// **Writes everything now, reads it back, and says whether it's all there.** What's waiting and what's in memory are taken in one look, so an edit made in between can't be in one and not the other. `force` makes the drive itself flush everything a launch would read, older writes included, which only a hand-off needs. Waits for the session thread and the disk, so it's for a thread that can.
    pub(crate) fn flush(&self, force: bool, limit: Duration) -> Result<FlushReport, StoreError> {
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
        let began = Instant::now();
        // A hand-off can wait, and an update mustn't go on something unfinished. A quit is going anyway, so it writes what there is.
        if !inner.core.flush(limit) && force {
            return Err(StoreError {
                code: "still_busy",
                message: "Something the app was asked to do hasn't finished, so its state can't be said to be whole.".to_string(),
            });
        }
        let _gate = lock(&self.gate);
        self.first_edit.store(0, Ordering::SeqCst);
        self.view_changed.store(false, Ordering::Relaxed);
        self.urgent.store(false, Ordering::SeqCst);
        let gathered = Instant::now();
        let before = self.store.wrote_bytes();
        let (work, record, delta, lists) = inner.gather_for_flush();
        let gather_us = micros(gathered);
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
            inner.host.session.with_session(|session| session.lists_by_journal())
        } else {
            lists
        };
        let asleep: HashMap<String, EditsSummary> = inner.host.session.with_session(|session| session.asleep_by_journal());
        if let Err(first) = self.store.verify_with_asleep(&record, &lists, &asleep) {
            // A line that didn't say enough is mended by the whole record. Anything else fails.
            self.store.next_write_whole();
            self.store.write_record(&record, &delta, force)?;
            self.store.verify_with_asleep(&record, &lists, &asleep).map_err(|_| first)?;
        }
        let verify_us = micros(verified);
        // The tab on screen's sleep file, for the next launch's first frame. Nothing depends on it, so it's no part of what's read back or forced to the drive, and it comes after the state: with sleep files queued for a slow disk, a quit's 5 s ran out waiting here, and it went without the state (slow mode, 2026-11-11). It waits for others being written for what's left of this flush's time, and past that is skipped and counted. If nothing's changed, nothing's written.
        if let Some(id) = inner.host.session.with_session(|session| session.active()) {
            inner.write_sleep_for_until(id, began + limit);
        }
        let (record_bytes, journal_bytes) = self.store.sizes();
        self.last_record_write.store(self.now(), Ordering::SeqCst);
        *lock(&self.last_error) = None;
        Ok(FlushReport {
            tabs: record.tabs.len(),
            edited_tabs: lists.len(),
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

fn micros(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

impl Inner {
    /// Takes in where the page view says each tab it holds is.
    pub(crate) fn take_places(&self) {
        let engines = self.panes.engines(&self.engine);
        let places: Vec<_> = engines.iter().flat_map(recto_reader::Engine::places).collect();
        if places.is_empty() {
            return;
        }
        // What's selected is the pane being read's: the session keeps one selection, on the tab it's on.
        let selection = self.panes.reading_engine(&self.engine).hot_selection();
        self.host.session.with_session(|session| {
            if let Some((tab, range)) = selection {
                session.note_selection(
                    recto_session::TabId(tab),
                    range.map(|range| Selection {
                        from_page: range.start.page,
                        from_char: range.start.at,
                        to_page: range.end.page,
                        to_char: range.end.at,
                    }),
                );
            }
            for (tab, saved) in places {
                let place = PlaceRecord {
                    page: saved.page,
                    anchor_page: saved.anchor.0,
                    within: saved.anchor.1,
                    x: saved.x,
                    y: saved.y,
                    document_height: saved.document_height,
                    zoom: saved.magnification,
                    page_width: saved.page_size.map_or(0.0, |size| size.width),
                    page_height: saved.page_size.map_or(0.0, |size| size.height),
                };
                session.note_place(recto_session::TabId(tab), place);
            }
        });
    }

    /// The whole record as things stand, and what a line of the log would say of it. The session is held only for the copy.
    pub(crate) fn gather_record(&self) -> (SessionRecord, PlaceDelta) {
        self.take_places();
        let (mut record, delta) = self.host.session.with_session(|session| {
            let delta = session.take_place_delta(0);
            (session.to_record(), delta)
        });
        record.app = self.app_record();
        (record, delta)
    }

    /// What's waiting for the journals, the record, and the lists, all in one look at the session.
    fn gather_for_flush(&self) -> (JournalWork, SessionRecord, PlaceDelta, HashMap<String, EditsRecord>) {
        self.take_places();
        let (work, mut record, delta, lists) = self.host.session.with_session(|session| {
            let work = session.journal_work();
            let delta = session.take_place_delta(0);
            (work, session.to_record(), delta, session.lists_by_journal())
        });
        record.app = self.app_record();
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
