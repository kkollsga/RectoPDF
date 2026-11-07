//! **The app's state on its way to disk.** SPEC decision 25: state goes to disk by what happened, not on a timer. Edits go in their journals after 2 s of quiet or every 5 s at most during a burst. Places and the like at most once a minute, and only if they've changed. Going to the background, quitting and a hand-off don't wait.
//!
//! One thread does the writing, and it exists only while there's something to write: at rest there's no thread, no timer and no wake-up, and nothing per idle tab. Saying something's changed is an atomic and, the first time, one push to the session thread, so the UI thread can say it on every scroll without waiting on anything.

use crate::{CoreCall, CoreQueue, Inner, lock};
use recto_control::record_store::{FlushReport, RecordStore, StoreError};
use recto_session::record::{EditsRecord, PlaceDelta, PlaceRecord, SessionRecord};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// How long edits wait for a pause.
const JOURNAL_QUIET: Duration = Duration::from_secs(2);
/// The longest edits wait during a burst.
const JOURNAL_LONGEST: Duration = Duration::from_secs(5);
/// The least time between writes of places and the like.
const RECORD_EVERY: Duration = Duration::from_secs(60);

pub(crate) struct Saver {
    pub(crate) store: RecordStore,
    core: Arc<CoreQueue>,
    inner: OnceLock<Weak<Inner>>,
    /// The record's been read, so what's written is what came of it. Nothing's written before that.
    started: AtomicBool,
    clock: Instant,
    /// The writer thread is there, or asked for.
    writer_alive: AtomicBool,
    writer: Mutex<Option<Thread>>,
    /// In milliseconds of `clock`, plus one, and zero for none.
    view_changed: AtomicBool,
    last_record_write: AtomicU64,
    urgent: AtomicBool,
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
        Self { store, core, inner: OnceLock::new(), started: AtomicBool::new(false), clock: Instant::now(), writer_alive: AtomicBool::new(false), writer: Mutex::new(None), view_changed: AtomicBool::new(false), last_record_write: AtomicU64::new(0), urgent: AtomicBool::new(false), first_edit: AtomicU64::new(0), last_edit: AtomicU64::new(0), gate: Mutex::new(()), last_error: Mutex::new(None) }
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
    }

    fn now(&self) -> u64 {
        millis(self.clock.elapsed()) + 1
    }

    /// A place, or something else in the record, has changed. Cheap enough for each scroll.
    pub(crate) fn view_changed(&self) {
        self.view_changed.store(true, Ordering::Relaxed);
        self.ensure_writer();
    }

    /// An edit has been made.
    pub(crate) fn edit_made(&self) {
        let now = self.now();
        let _ = self.first_edit.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
        self.last_edit.store(now, Ordering::SeqCst);
        self.ensure_writer();
        // It may be asleep for a place's minute, and this is due sooner.
        if let Some(thread) = lock(&self.writer).as_ref() {
            thread.unpark();
        }
    }

    /// What's changed shouldn't wait: the app is going to the background, or a journal has been retired.
    pub(crate) fn write_soon(&self) {
        self.urgent.store(true, Ordering::SeqCst);
        self.view_changed.store(true, Ordering::Relaxed);
        self.ensure_writer();
        if let Some(thread) = lock(&self.writer).as_ref() {
            thread.unpark();
        }
    }

    fn ensure_writer(&self) {
        if !self.started.load(Ordering::Relaxed) || self.writer_alive.swap(true, Ordering::SeqCst) {
            return;
        }
        self.core.push(CoreCall::StartWriter);
    }

    /// Starts the thread. On the session thread, so that nothing on the UI thread waits for a thread to start.
    pub(crate) fn spawn_writer(self: &Arc<Self>) {
        let saver = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("recto-save".into()).stack_size(512 * 1024).spawn(move || saver.run());
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

    fn run(self: Arc<Self>) {
        *lock(&self.writer) = Some(std::thread::current());
        loop {
            while let Some(wait) = self.next_due() {
                if wait.is_zero() {
                    self.pass();
                } else {
                    std::thread::park_timeout(wait);
                }
            }
            *lock(&self.writer) = None;
            self.writer_alive.store(false, Ordering::SeqCst);
            // Something came in as this was going.
            if self.next_due().is_none() || self.writer_alive.swap(true, Ordering::SeqCst) {
                return;
            }
            *lock(&self.writer) = Some(std::thread::current());
        }
    }

    /// One visit to the disk: whichever of journals and the record are due.
    fn pass(&self) {
        let now = self.now();
        let Some(inner) = self.inner.get().and_then(Weak::upgrade) else {
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
            if let Err(error) = self.write_journals(&inner, false) {
                failed = Some(error);
            }
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

    fn write_journals(&self, inner: &Inner, force: bool) -> Result<(), StoreError> {
        let work = inner.host.session.with_session(recto_session::Session::journal_work);
        if work.retired.is_empty() && work.appends.is_empty() {
            return Ok(());
        }
        let written = match self.store.write_journals(&work, force) {
            Ok(written) => written,
            Err(error) => {
                // What was retired is still to go. The operations were copied and are still in the lists.
                inner.host.session.with_session(|session| session.retire_again(work.retired.clone()));
                return Err(error);
            }
        };
        let names: Vec<(recto_session::TabId, String)> = work.appends.iter().map(|append| (append.tab, append.name.clone())).collect();
        inner.host.session.with_session(|session| {
            for (id, through) in written {
                session.journal_written(id, through);
            }
        });
        for (id, name) in names {
            if self.store.should_compact(&name) {
                self.compact(inner, id, &name);
            }
        }
        Ok(())
    }

    /// Moves a journal's checkpoint, if there's nothing still waiting that the checkpoint would pass.
    fn compact(&self, inner: &Inner, id: recto_session::TabId, name: &str) {
        let checkpoint = inner.host.session.with_session(|session| session.checkpoint_if_caught_up(id));
        if let Some((header, base)) = checkpoint {
            let _ = self.store.compact(name, &header, &base);
        }
    }

    /// **Writes everything now, reads it back, and says whether it's all there.** The updater won't go ahead without this. `force` makes the drive itself flush, which only a hand-off needs. Waits for the session thread and the disk, so it's for a thread that can.
    pub(crate) fn flush(&self, force: bool, limit: Duration) -> Result<FlushReport, StoreError> {
        let Some(inner) = self.inner.get().and_then(Weak::upgrade) else { return Err(StoreError { code: "not_ready", message: "The app is going away.".to_string() }) };
        if !self.started.load(Ordering::SeqCst) {
            return Err(StoreError { code: "not_ready", message: "The app hasn't finished starting.".to_string() });
        }
        let began = Instant::now();
        inner.core.flush(limit);
        let _gate = lock(&self.gate);
        self.first_edit.store(0, Ordering::SeqCst);
        self.view_changed.store(false, Ordering::Relaxed);
        self.urgent.store(false, Ordering::SeqCst);
        let gathered = Instant::now();
        let before = self.store.wrote_bytes();
        self.write_journals(&inner, force)?;
        let (record, delta, lists) = inner.gather_for_flush();
        let gather_us = micros(gathered);
        let written = Instant::now();
        self.store.write_record(&record, &delta, force)?;
        let write_us = micros(written);
        let verified = Instant::now();
        if let Err(first) = self.store.verify(&record, &lists) {
            // A line that didn't say enough is mended by the whole record. Anything else fails.
            self.store.next_write_whole();
            self.store.write_record(&record, &delta, force)?;
            self.store.verify(&record, &lists).map_err(|_| first)?;
        }
        let verify_us = micros(verified);
        let (record_bytes, journal_bytes) = self.store.sizes();
        self.last_record_write.store(self.now(), Ordering::SeqCst);
        *lock(&self.last_error) = None;
        Ok(FlushReport { tabs: record.tabs.len(), edited_tabs: lists.len(), record_bytes, journal_bytes, wrote_bytes: self.store.wrote_bytes() - before, gather_us, write_us, verify_us, total_us: micros(began) })
    }
}

fn micros(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

impl Inner {
    /// Takes in where the page view says each tab it holds is.
    pub(crate) fn take_places(&self) {
        let places = self.engine.places();
        if places.is_empty() {
            return;
        }
        self.host.session.with_session(|session| {
            for (tab, saved) in places {
                let place = PlaceRecord { page: saved.page, anchor_page: saved.anchor.0, within: saved.anchor.1, x: saved.x, y: saved.y, document_height: saved.document_height, zoom: saved.magnification, page_width: saved.page_size.map_or(0.0, |size| size.width), page_height: saved.page_size.map_or(0.0, |size| size.height) };
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

    fn gather_for_flush(&self) -> (SessionRecord, PlaceDelta, HashMap<String, EditsRecord>) {
        self.take_places();
        let (mut record, delta, lists) = self.host.session.with_session(|session| {
            let delta = session.take_place_delta(0);
            (session.to_record(), delta, session.lists_by_journal())
        });
        record.app = self.app_record();
        (record, delta, lists)
    }
}
