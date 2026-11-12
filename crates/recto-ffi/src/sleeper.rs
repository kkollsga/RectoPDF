//! **Tabs going dormant and waking, SPEC decision 25.** Owner, 2026-11-07: "It's fine to keep in memory for a while, and then as it goes dormant we store to disk." And: "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
//!
//! **One deadline, and no thread.** A tab left alone for [`DORMANT_AFTER_SECONDS`] goes dormant. There's nothing per tab: the session thread's one wait has this as well as the state's deadline, and it's the soonest of the few tabs that hold anything. Coming due, it does what's asked and works out the next. At rest with nothing warm there's no deadline at all.
//!
//! **Going dormant, once:** the tab's sleep file's written if there's anything new to say, its journal's compacted and read back, and its contents, list, worker and tiles go. A byte budget across the documents off screen and memory pressure make that happen sooner, oldest first. A tab being worked on by an agent or saved doesn't go, whatever's asking: `Session::hold` and `saving` see to that.

use crate::{CoreCall, Inner, lock};
use recto_platform::surface::PixelSurface;
use recto_reader::layout::PageBox;
use recto_reader::paper::PaperKind;
use recto_reader::{SleepMaterial, SleepShot, SleepView};
use recto_session::record::{FileStatus, look_at_file};
use recto_session::sleep::{SleepFile, SleepPicture};
use recto_session::{SleepFacts, TabId};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A tab goes dormant once it hasn't been active or edited for this long. SPEC decision 25's five minutes, its *default*.
pub(crate) const DORMANT_AFTER_SECONDS: u32 = 5 * 60;
/// What the documents off screen may hold in all, their workers' memory and their histories', before the oldest goes dormant. SPEC section 2: "a byte budget frees the oldest first". What's on screen is another row.
pub(crate) const LIVE_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
/// The least time between sleep files for a tab that's only moving about (the owner's rule on SSD wear). A quit and a hand-off don't wait, and neither does being asked. A tab left sooner has its older file, whose facts are still good, and whose pictures show only on the pages they're of.
const SLEEP_WRITE_EVERY_SECONDS: u32 = 60;
/// What sleep files may come to in an hour, all told, for the same reason: this is about 8 MiB on a heavy day's eight hours, inside SPEC section 2's "About 10 MiB of state". A file that's text is about 8 KiB, and one of a photograph about 100.
const SLEEP_BYTES_PER_HOUR: u64 = 1024 * 1024;
/// How often the budget's looked at, at the most, as each look is a call to the kernel per worker.
const BUDGET_LOOK_EVERY: Duration = Duration::from_secs(1);
/// How often a flush looks again for the sleep files' lock, while it may wait for it.
const WRITING_LOOK_EVERY: Duration = Duration::from_millis(10);
/// The most the queue's writer holds back for a write someone's waiting on, each time.
const OTHERS_FIRST_FOR: Duration = Duration::from_secs(1);
/// How soon a pass that couldn't go ahead, as something's being read, tries again.
const BUSY_RETRY: Duration = Duration::from_secs(5);

/// Why tabs are going dormant, for QA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    LeftAlone,
    Budget,
    Pressure,
    Asked,
}

#[derive(Default)]
struct Counts {
    left_alone: AtomicU64,
    budget: AtomicU64,
    pressure: AtomicU64,
    asked: AtomicU64,
    sleep_bytes: AtomicU64,
    sleep_files: AtomicU64,
    writes_put_off: AtomicU64,
    /// The tab on screen's file at a flush, not written as another was being written past the flush's time.
    writes_skipped_busy: AtomicU64,
    woken_from_file: AtomicU64,
    pressure_events: AtomicU64,
    /// The system's own pressure in a quiet run, counted and not acted on: warnings, and critical.
    system_warning: AtomicU64,
    system_critical: AtomicU64,
}

pub(crate) struct Sleeper {
    clock: Instant,
    after_seconds: AtomicU32,
    budget_bytes: AtomicU64,
    /// When the soonest is due, in milliseconds of `clock` plus one, and zero for nothing.
    due: AtomicU64,
    last_budget_look: Mutex<Option<(Instant, Option<TabId>)>>,
    write_gap_seconds: AtomicU32,
    /// When each tab's sleep file was last written, for those that have been. Next to no bytes per tab.
    last_written: Mutex<HashMap<TabId, Instant>>,
    /// The hour this began, and what's been written in it.
    this_hour: Mutex<(Instant, u64)>,
    /// One sleep file being written at a time, so a flush has what's on its way.
    writing: Mutex<()>,
    /// Writes waiting for [`Self::writing`] that someone's waiting on, a flush's or being asked's: the queue's writer lets them go first.
    others_waiting: AtomicU32,
    /// Sleep files written off the session's time, waiting for their one writer: [`recto_app::sleep_queue`].
    leaving: Mutex<recto_app::sleep_queue::LeavingQueue>,
    counts: Counts,
    /// Looks at files under way, each on a thread of its own: one per tab at most, and [`MAX_LOOKS`] in all.
    looks: std::sync::Arc<LookSlots>,
}

/// Looks at files that can be under way at once. A look is a stat, microseconds, unless the file's on a volume that's stopped answering, where it may never end: then this many threads are all that's ever held for it, however many times the app comes to the front.
pub(crate) const MAX_LOOKS: usize = 8;

/// **Which tabs have a look at their file under way.** A tab already being looked at doesn't get a second thread, and past [`MAX_LOOKS`] no more start, so coming back to the app over and over with a hung disk adds no threads (review O3). A look asked for while one's under way isn't lost: it's noted, and when the look under way ends it's asked for again, as the one under way may have looked before the file changed.
#[derive(Debug, Default)]
pub(crate) struct LookSlots {
    /// Each tab being looked at, and whether another look was asked for meanwhile.
    running: Mutex<Vec<(TabId, bool)>>,
}

impl LookSlots {
    /// Takes a slot for a look at this tab's file, if it hasn't one and one's free. If it has one, another look is noted for when that ends.
    pub(crate) fn start(&self, id: TabId) -> bool {
        let mut running = lock(&self.running);
        if let Some(entry) = running.iter_mut().find(|(tab, _)| *tab == id) {
            entry.1 = true;
            return false;
        }
        if running.len() >= MAX_LOOKS {
            return false;
        }
        running.push((id, false));
        true
    }

    /// The look's done, however it ended: whether another was asked for meanwhile, which the caller asks for again.
    pub(crate) fn finish(&self, id: TabId) -> bool {
        let mut running = lock(&self.running);
        let again = running.iter().any(|(tab, again)| *tab == id && *again);
        running.retain(|(tab, _)| *tab != id);
        again
    }

    pub(crate) fn running(&self) -> usize {
        lock(&self.running).len()
    }
}

impl Sleeper {
    pub(crate) fn new() -> Self {
        Self {
            clock: Instant::now(),
            after_seconds: AtomicU32::new(DORMANT_AFTER_SECONDS),
            budget_bytes: AtomicU64::new(LIVE_BUDGET_BYTES),
            due: AtomicU64::new(0),
            last_budget_look: Mutex::new(None),
            write_gap_seconds: AtomicU32::new(SLEEP_WRITE_EVERY_SECONDS),
            last_written: Mutex::new(HashMap::new()),
            this_hour: Mutex::new((Instant::now(), 0)),
            writing: Mutex::new(()),
            others_waiting: AtomicU32::new(0),
            leaving: Mutex::new(recto_app::sleep_queue::LeavingQueue::default()),
            counts: Counts::default(),
            looks: std::sync::Arc::new(LookSlots::default()),
        }
    }

    fn millis(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX) + 1
    }

    /// How long until something's due, for the session thread's wait, or `None`. An atomic: it's asked with the queue's lock held.
    pub(crate) fn wait_for_due(&self) -> Option<Duration> {
        match self.due.load(Ordering::SeqCst) {
            0 => None,
            due => Some(Duration::from_millis(due.saturating_sub(self.millis()))),
        }
    }

    pub(crate) fn set_after(&self, seconds: u32) {
        self.after_seconds.store(seconds, Ordering::SeqCst);
    }

    /// How long a tab that's only moving about waits for its next sleep file. None at all takes the hourly limit with it.
    pub(crate) fn set_write_gap(&self, seconds: u32) {
        self.write_gap_seconds.store(seconds, Ordering::SeqCst);
    }

    /// Whether a tab that's only moving about may have a new picture written now. Asked before anything's packed, as that's work too.
    fn may_write_pictures(&self, id: TabId) -> bool {
        let gap = self.write_gap_seconds.load(Ordering::SeqCst);
        if gap == 0 {
            return true;
        }
        let soon = lock(&self.last_written)
            .get(&id)
            .is_some_and(|last| last.elapsed() < Duration::from_secs(u64::from(gap)));
        let mut hour = lock(&self.this_hour);
        if hour.0.elapsed() >= Duration::from_secs(3600) {
            *hour = (Instant::now(), 0);
        }
        !soon && hour.1 < SLEEP_BYTES_PER_HOUR
    }

    /// The one-at-a-time lock on writing sleep files, for a write someone's waiting on: waited for, or until `until` and no further, and counted meanwhile so the queue's writer lets it go first. A flush's thread or being asked's, never the UI's.
    fn take_writing(&self, until: Option<Instant>) -> Option<std::sync::MutexGuard<'_, ()>> {
        self.others_waiting.fetch_add(1, Ordering::SeqCst);
        let Some(until) = until else {
            let taken = lock(&self.writing);
            self.others_waiting.fetch_sub(1, Ordering::SeqCst);
            return Some(taken);
        };
        let taken = loop {
            match self.writing.try_lock() {
                Ok(guard) => break Some(guard),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => break Some(poisoned.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() >= until => break None,
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(WRITING_LOOK_EVERY),
            }
        };
        self.others_waiting.fetch_sub(1, Ordering::SeqCst);
        taken
    }

    /// Before the queue's writer takes the lock for its next, a write someone's waiting on goes first: a flush looks only every [`WRITING_LOOK_EVERY`], and a lock isn't fair, so the writer, going from one to the next, would otherwise have it again first, every time. Bounded, as a flush has a limit of its own.
    fn let_others_go_first(&self) {
        let began = Instant::now();
        while self.others_waiting.load(Ordering::SeqCst) > 0 && began.elapsed() < OTHERS_FIRST_FOR {
            std::thread::sleep(WRITING_LOOK_EVERY);
        }
    }

    fn note_written(&self, id: TabId, bytes: u64) {
        let gap = Duration::from_secs(u64::from(self.write_gap_seconds.load(Ordering::SeqCst)));
        let mut last = lock(&self.last_written);
        // Only a write within the gap holds the next one back, so older ones go: without this every tab ever written stayed, closed ones too, a day's soak of them.
        last.retain(|_, at| at.elapsed() < gap);
        last.insert(id, Instant::now());
        drop(last);
        lock(&self.this_hour).1 += bytes;
        self.counts.sleep_files.fetch_add(1, Ordering::Relaxed);
        self.counts.sleep_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn set_budget(&self, bytes: u64) {
        self.budget_bytes.store(bytes, Ordering::SeqCst);
    }

    pub(crate) fn report(&self) -> Value {
        let count = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let counts = &self.counts;
        json!({
            "after_seconds": self.after_seconds.load(Ordering::SeqCst),
            "budget_bytes": self.budget_bytes.load(Ordering::SeqCst),
            "due_in_ms": self.wait_for_due().map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
            "gone_dormant": { "left_alone": count(&counts.left_alone), "budget": count(&counts.budget), "pressure": count(&counts.pressure), "asked": count(&counts.asked) },
            "sleep_files_written": count(&counts.sleep_files),
            "writes_put_off": count(&counts.writes_put_off),
            "writes_skipped_busy": count(&counts.writes_skipped_busy),
            "sleep_bytes_written": count(&counts.sleep_bytes),
            "woken_from_file": count(&counts.woken_from_file),
            "pressure_events": count(&counts.pressure_events),
            "system_pressure": { "warning": count(&counts.system_warning), "critical": count(&counts.system_critical) },
            "looks_running": self.looks.running(),
            "leaving": lock(&self.leaving).report(),
        })
    }
}

impl Inner {
    /// Works out the next deadline, after anything that can have changed it. A look at the few tabs that hold anything.
    pub(crate) fn rearm_dormancy(&self) {
        let after = self.sleeper.after_seconds.load(Ordering::SeqCst);
        let wait = self
            .host
            .session
            .with_session(|session| session.next_dormant_at(after).map(|at| at.saturating_sub(session.seconds_now())));
        let due = wait.map_or(0, |seconds| self.sleeper.millis() + u64::from(seconds) * 1000);
        self.sleeper.due.store(due, Ordering::SeqCst);
    }

    /// It's come due, on the session thread. Whatever's due goes dormant, unless a file's being read, when it looks again shortly.
    pub(crate) fn dormancy_due(&self) -> Option<Vec<TabId>> {
        if self.reads_running.load(Ordering::SeqCst) > 0 || self.core.frozen.load(Ordering::SeqCst) {
            self.sleeper
                .due
                .store(self.sleeper.millis() + u64::try_from(BUSY_RETRY.as_millis()).unwrap_or(5000), Ordering::SeqCst);
            return None;
        }
        let after = self.sleeper.after_seconds.load(Ordering::SeqCst);
        Some(self.host.session.with_session(|session| session.due_dormant(session.seconds_now(), after)))
    }

    /// **Those tabs go dormant,** on the session thread, those that may: whatever's asking, not the one on screen or one being worked on or saved. Whatever hasn't gone to its journal goes first, or a list couldn't.
    pub(crate) fn go_dormant(&self, tabs: &[TabId], why: Why) -> usize {
        let tabs: Vec<TabId> = self
            .host
            .session
            .with_session(|session| tabs.iter().copied().filter(|id| session.can_go_dormant(*id)).collect());
        if tabs.is_empty() || self.core.frozen.load(Ordering::SeqCst) {
            return 0;
        }
        self.saver.write_journals_now(self);
        let mut gone = 0;
        for &id in &tabs {
            // Being asked is an agent's or `recto session sleep`'s, which looks at the file straight after, and waits behind the one being written at most. The rest are the session's own, which never waits on a disk: queued, as those leaving the warm ones are (review 1 of `build/one-sleep-writer`). Being asked counts against the hour too: only a quit and a hand-off don't wait (review).
            if why == Why::Asked {
                self.write_sleep_for(id, false);
            } else {
                self.queue_sleep_for(id);
            }
            let list = self.host.session.with_session(|session| session.list_to_sleep(id));
            // Compacted, and read back. If that doesn't say it's all there, the list stays in memory, and it's looked at again next time it's left alone.
            let settled = list.is_none_or(|(name, header, base)| self.saver.store.settle_list(&name, &header, &base));
            // If it's not gone, it's looked at again next time it's been left alone, and not till then.
            let went = self.host.session.with_session(|session| {
                let went = settled && session.go_dormant(id);
                if !went {
                    session.touch(id);
                }
                went
            });
            gone += usize::from(went);
        }
        let counts = &self.sleeper.counts;
        let counter = match why {
            Why::LeftAlone => &counts.left_alone,
            Why::Budget => &counts.budget,
            Why::Pressure => &counts.pressure,
            Why::Asked => &counts.asked,
        };
        counter.fetch_add(gone as u64, Ordering::Relaxed);
        gone
    }

    /// Writes a tab's sleep file from what's known of it now, if that's anything new. `now` is a quit, a hand-off or being asked, which don't wait for the limits.
    pub(crate) fn write_sleep_for(&self, id: TabId, now: bool) {
        self.write_sleep_now_for(id, now, None);
    }

    /// The same, as a quit or a hand-off has it, waiting for another sleep file being written only until `until`, and counted as skipped past it. These are only a way to be quick, and the state's already written by then: a quit waited its 5 s behind sleep files queued for a slow disk and went without the state (slow mode, 2026-11-11). Skipped, the next launch's first frame is bare paper, which the count says.
    pub(crate) fn write_sleep_for_until(&self, id: TabId, until: Instant) {
        self.write_sleep_now_for(id, true, Some(until));
    }

    fn write_sleep_now_for(&self, id: TabId, now: bool, until: Option<Instant>) {
        let Some(facts) = self.host.session.with_session(|session| session.sleep_facts(id)) else {
            return;
        };
        let own = Some(version_of(facts.sleep.file));
        let Some(material) = self.panes.engine_for_tab(&self.engine, id).sleep_material(id.0, own) else {
            return;
        };
        // What's queued for it is older than this.
        lock(&self.sleeper.leaving).forget(id);
        self.write_with_material(facts, material, now, until);
    }

    /// A sleep file from what's known and what's on hand. With no pictures to hand, as with a closed window or what the system's taken, what's there already stands, if it's about the same list. Past the limits, a tab that's only moving about keeps what it has, and one with nothing yet has its facts, which is small and says what the PDF would.
    fn write_with_material(&self, facts: SleepFacts, material: SleepMaterial, now: bool, until: Option<Instant>) {
        let Some(_one_at_a_time) = self.sleeper.take_writing(until) else {
            self.sleeper.counts.writes_skipped_busy.fetch_add(1, Ordering::Relaxed);
            return;
        };
        self.write_holding_lock(facts, material, now);
    }

    /// The same, with the one-at-a-time lock held.
    fn write_holding_lock(&self, facts: SleepFacts, material: SleepMaterial, now: bool) {
        let SleepFacts {
            id,
            path,
            sleep,
            pictures_allowed,
        } = facts;
        let there = self.saver.store.read_sleep(sleep.file, &path);
        let may = now || self.sleeper.may_write_pictures(id);
        if !may && there.is_some() {
            self.sleeper.counts.writes_put_off.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut sleep = with_material(sleep, material);
        if !pictures_allowed || !may {
            sleep.pictures.clear();
        } else if sleep.pictures.is_empty()
            && let Some(there) = there
                .filter(|there| there.list_seq == sleep.list_seq && there.pages == sleep.pages && there.version.is_some() && there.version == sleep.version)
        {
            sleep.pictures = there.pictures;
        }
        let bytes = self.saver.store.write_sleep(&path, &sleep);
        if bytes > 0 {
            self.sleeper.note_written(id, bytes);
        }
    }

    /// **Tabs pushed out of the warm ones, their sleep files queued,** with their pictures copied just before the page view lets go of them: a quick copy of each, of those whose time's come, queued as it's made, so what's held is the queue's cap and the one being copied. Packing a photograph takes long enough to hold up whatever's asked for next, so the one writer does that ([`recto_app::sleep_queue`]). A flush doesn't wait for these: it writes the state first, and then the tab on screen's file, waiting behind the one being written only for what's left of its time.
    pub(crate) fn queue_those_leaving(&self) {
        let left = self.host.session.with_session(|session| session.take_left_warm());
        let mut start = false;
        for facts in left {
            let own = Some(version_of(facts.sleep.file));
            if let Some(material) = self.panes.engine_for_tab(&self.engine, facts.id).sleep_material(facts.id.0, own) {
                start |= lock(&self.sleeper.leaving).push(facts, material);
            }
        }
        if start {
            self.start_sleep_writer();
        }
    }

    /// A tab going dormant by itself: its sleep file, from what's on hand now, queued for the one writer.
    fn queue_sleep_for(&self, id: TabId) {
        let Some(facts) = self.host.session.with_session(|session| session.sleep_facts(id)) else {
            return;
        };
        let own = Some(version_of(facts.sleep.file));
        let Some(material) = self.panes.engine_for_tab(&self.engine, id).sleep_material(id.0, own) else {
            return;
        };
        if lock(&self.sleeper.leaving).push(facts, material) {
            self.start_sleep_writer();
        }
    }

    /// The queue's said there's no writer: one's started.
    fn start_sleep_writer(&self) {
        let Some(inner) = self.saver.inner() else {
            lock(&self.sleeper.leaving).writer_gone();
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("recto-sleep".into())
            .stack_size(512 * 1024)
            .spawn(move || inner.write_leaving());
        if spawned.is_err() {
            lock(&self.sleeper.leaving).writer_gone();
        }
    }

    /// **The queue's one writer:** oldest first, until there's none, and then it ends. Each is taken from the queue only as it's about to be written, and counted till it's done, so what's held is what the queue's caps allow. A write someone's waiting on goes first. One that something newer's been written for since it was taken isn't written: that's asked with the one-at-a-time lock held, so the newer either came before or comes after.
    fn write_leaving(&self) {
        let queue = &self.sleeper.leaving;
        let _running = recto_app::sleep_queue::WriterRunning::enter(queue);
        loop {
            // Its own statement, so the queue's lock is let go before the write: in a `while let` it would be held through it.
            let next = lock(queue).take_next();
            let Some((facts, material)) = next else { return };
            self.sleeper.let_others_go_first();
            let one_at_a_time = lock(&self.sleeper.writing);
            if lock(queue).still_wanted() {
                self.write_holding_lock(facts, material, false);
            }
            drop(one_at_a_time);
            lock(queue).done();
        }
    }

    /// Tabs just woken from their sleep files: nothing's asked of the PDF on the way, as it may be on a disk that's gone away. Each is looked at on a thread of its own, and if it's turned out to be something else, it's read after all.
    pub(crate) fn check_woken_files(&self) {
        let woken = self.host.session.with_session(|session| session.take_woken_from_file());
        if woken.is_empty() {
            return;
        }
        self.sleeper.counts.woken_from_file.fetch_add(woken.len() as u64, Ordering::Relaxed);
        if let Some(inner) = self.saver.inner() {
            for id in woken {
                check_woken_file(&inner, id);
            }
        }
    }

    /// What a tab's first frame shows, for the page view: from the file just read, or its sleep file if nobody's read it, whichever way this tab's been asked for. Only what's about the document as it is now. Each picture says what paper it's for, and only goes on that. On the session thread, with no lock on the page view while the surfaces are made.
    pub(crate) fn give_sleep_view(&self, engine: &recto_reader::Engine, id: TabId) {
        let read = self.host.session.with_session(|session| session.take_sleep_read(id));
        let sleep = read.or_else(|| {
            let (stamp, path) = self.host.session.with_session(|session| session.sleep_to_read_awake(id))?;
            self.saver.store.read_sleep(stamp, &path)
        });
        let Some(sleep) = sleep else { return };
        // Read for a file that's since been found to be another: nothing in it's about this one.
        if self
            .host
            .session
            .with_session(|session| session.own_stamp(id))
            .is_none_or(|now| !(now.same_contents(sleep.file) && now.inode == sleep.file.inode))
        {
            return;
        }
        let (pages, mark) = self.host.session.with_session(|session| (session.pages_in(id), session.list_mark(id)));
        if sleep.pages != pages || sleep.list_seq != mark {
            return;
        }
        let page_box = |(width, height): (f32, f32)| PageBox { width, height };
        let mut pictures = Vec::with_capacity(sleep.pictures.len());
        for picture in &sleep.pictures {
            if let Some(surface) = surface_of(picture) {
                pictures.push(SleepShot {
                    page: picture.page,
                    scale: picture.scale,
                    paper: if picture.dark { PaperKind::Dark } else { PaperKind::White },
                    surface,
                });
            }
        }
        engine.give_sleep(
            id.0,
            SleepView {
                version: sleep.version,
                usual: sleep.usual.map(page_box),
                sizes: sleep.sizes.iter().map(|(page, width, height)| (*page, page_box((*width, *height)))).collect(),
                pictures,
            },
        );
    }

    /// The documents off screen against their budget. At most once a second, and only with something off screen, which most of the time there's little of.
    pub(crate) fn look_at_budget(&self) {
        let (warm, active) = self.host.session.with_session(|session| (session.dormant_oldest_first(), session.active()));
        if warm.is_empty() {
            return;
        }
        {
            // A change of tab is when a document comes to be off screen, so that's looked at straight away.
            let mut last = lock(&self.sleeper.last_budget_look);
            if last.as_ref().is_some_and(|(when, was)| *was == active && when.elapsed() < BUDGET_LOOK_EVERY) {
                return;
            }
            *last = Some((Instant::now(), active));
        }
        let budget = self.sleeper.budget_bytes.load(Ordering::SeqCst);
        let costs: Vec<(TabId, u64)> = warm
            .iter()
            .map(|id| {
                (
                    *id,
                    self.panes.engine_for_tab(&self.engine, *id).worker_bytes(id.0)
                        + self
                            .host
                            .session
                            .with_session(|session| session.page_edits(*id).map_or(0, |list| list.history_bytes() as u64)),
                )
            })
            .collect();
        let mut total: u64 = costs.iter().map(|(_, cost)| cost).sum();
        let mut over = Vec::new();
        for (id, cost) in costs {
            if total <= budget {
                break;
            }
            over.push(id);
            total -= cost;
        }
        if !over.is_empty() {
            self.core.push(CoreCall::SleepTabs(over, Why::Budget));
        }
    }

    /// The system's memory pressure, from the host's dispatch source (and `qa memory-pressure system-warning`): acted on, or in a quiet run only counted ([`recto_app::pressure::heard`]). A quiet run is the one with a QA bridge.
    pub(crate) fn system_pressure(&self, critical: bool) {
        match recto_app::pressure::heard(self.qa.is_some(), recto_app::pressure::Source::System) {
            recto_app::pressure::Heard::Act => self.core.push(CoreCall::MemoryPressure(critical)),
            recto_app::pressure::Heard::Count => {
                let counts = &self.sleeper.counts;
                let counter = if critical { &counts.system_critical } else { &counts.system_warning };
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// The system's asked for memory back: the same as a tab's being left alone, for all that can go, and now. Counted first, so a test can see it came.
    pub(crate) fn memory_pressure(&self, critical: bool) {
        self.sleeper.counts.pressure_events.fetch_add(1, Ordering::Relaxed);
        let tabs = self.host.session.with_session(|session| session.dormant_oldest_first());
        self.go_dormant(&tabs, Why::Pressure);
        for engine in self.panes.engines(&self.engine) {
            engine.memory_pressure(critical);
        }
    }
}

fn with_material(mut sleep: SleepFile, material: SleepMaterial) -> SleepFile {
    sleep.version = material.version;
    sleep.usual = material.usual.map(|size| (size.width, size.height));
    sleep.sizes = material.sizes.into_iter().map(|(page, size)| (page, size.width, size.height)).collect();
    sleep.pictures = material
        .pictures
        .into_iter()
        .map(|picture| SleepPicture {
            page: picture.page,
            scale: picture.scale,
            width: picture.width,
            height: picture.height,
            dark: picture.paper == PaperKind::Dark,
            bgra: picture.bgra,
        })
        .collect();
    sleep
}

/// A surface of its own for a picture. Rows of a surface can be padded.
fn surface_of(picture: &SleepPicture) -> Option<PixelSurface> {
    let surface = PixelSurface::create(picture.width, picture.height).ok()?;
    let stride = surface.stride() as usize;
    let row = picture.width as usize * 4;
    surface
        .with_pixels(|target| {
            for (line, pixels) in target.chunks_exact_mut(stride).zip(picture.bgra.chunks_exact(row)) {
                line[..row].copy_from_slice(pixels);
            }
        })
        .ok()?;
    Some(surface)
}

pub(crate) use recto_app::version_of;

/// **Looks at a tab's file before it's shown, for as long as a frame can wait for it.** Another file there means what's held of it is of something else, so the tab's read again, and nothing drawn of the old one is shown: the page view's told its version's changed. Past the time, which is a disk that's gone away, it's shown as it was, and the look goes on on its own thread. A stat, which is microseconds.
pub(crate) fn look_before_showing(inner: &Inner, id: TabId) {
    let Some((path, stamp)) = inner.host.session.with_session(|session| Some((session.path_of(id)?, session.own_stamp(id)))) else {
        return;
    };
    // One look per tab, and so many in all: with a disk that's stopped answering, showing tab after tab adds no threads past them.
    if !inner.sleeper.looks.start(id) {
        return;
    }
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let core = std::sync::Arc::clone(&inner.core);
    let slots = std::sync::Arc::clone(&inner.sleeper.looks);
    let looking = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let status = look_at_file(&path, stamp);
        // A look asked for meanwhile may be about a change made after this one looked: the files showing are looked at again.
        if slots.finish(id) {
            core.push(CoreCall::LookAtShown);
        }
        let other = !matches!(&status, FileStatus::Same(now) if Some(*now) == stamp) && !matches!(status, FileStatus::NotLooked);
        // Too late for the frame: it's said the usual way.
        if send.try_send((other, status.clone())).is_err() && other {
            core.push(CoreCall::WokenFileIsOther(id, status));
        }
    });
    if looking.is_err() {
        inner.sleeper.looks.finish(id);
        return;
    }
    if let Ok((true, status)) = receive.recv_timeout(LOOK_PATIENCE) {
        inner.host.session.with_session(|session| {
            session.apply_file_status(id, status);
            session.forget_read(id);
        });
        inner.core.push(CoreCall::WokenFileIsOther(id, FileStatus::NotLooked));
    }
}

/// How long a tab being shown waits for a look at its file.
const LOOK_PATIENCE: Duration = Duration::from_millis(30);

/// One thread looks at the one file a tab was woken from its sleep file on, or that's showing when the app comes to the front, and ends. If it's something else now, the session's told, and it's read after all. A tab with a look under way already, or past [`MAX_LOOKS`] in all, isn't looked at now: see [`LookSlots`].
pub(crate) fn check_woken_file(inner: &std::sync::Arc<Inner>, id: TabId) {
    let Some((path, stamp)) = inner.host.session.with_session(|session| Some((session.path_of(id)?, session.own_stamp(id)))) else {
        return;
    };
    if !inner.sleeper.looks.start(id) {
        return;
    }
    let (slots, core) = (std::sync::Arc::clone(&inner.sleeper.looks), std::sync::Arc::clone(&inner.core));
    let spawned = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let status = look_at_file(&path, stamp);
        if !matches!(&status, FileStatus::Same(now) if Some(*now) == stamp) && !matches!(status, FileStatus::NotLooked) {
            core.push(CoreCall::WokenFileIsOther(id, status));
        }
        // A look asked for meanwhile may be about a change made after this one looked: the files showing are looked at again.
        if slots.finish(id) {
            core.push(CoreCall::LookAtShown);
        }
    });
    if spawned.is_err() {
        inner.sleeper.looks.finish(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: coming back to the app with a file on a disk that's stopped answering starts another look thread per tab each time, without end (review O3): a tab already being looked at must not get a second thread, and no more than `MAX_LOOKS` at once. And a look asked for while one is under way is lost, so a file changed after that one looked isn't noticed when the app comes to the front (seen once in the opening shots, under load).
    #[test]
    fn looks_are_one_per_tab_and_capped() {
        let slots = LookSlots::default();
        assert!(slots.start(TabId(1)));
        assert!(!slots.start(TabId(1)), "one look per tab");
        for id in 2..=(MAX_LOOKS as u32 + 4) {
            slots.start(TabId(id));
        }
        assert_eq!(slots.running(), MAX_LOOKS, "capped in all");
        assert!(slots.finish(TabId(1)), "the look asked for meanwhile is asked for again");
        assert!(slots.start(TabId(1)), "a slot comes back when a look ends");
        assert!(!slots.finish(TabId(1)), "nothing was asked for meanwhile this time");
    }

    /// Failure: when each tab's sleep file was last written is kept for every tab that ever had one, closed tabs included, so a session that opens and closes tabs all day grows without a cap (the 24-hour soak of 2026-11-08 went through 3,068 tabs). Only those written within the gap are kept, and the gap still holds a tab back.
    #[test]
    fn last_written_keeps_only_writes_within_the_gap() {
        let sleeper = Sleeper::new();
        sleeper.set_write_gap(1);
        for id in 0..500 {
            sleeper.note_written(TabId(id), 1);
        }
        assert!(!sleeper.may_write_pictures(TabId(499)), "a tab just written waits for the gap");
        std::thread::sleep(Duration::from_millis(1_100));
        sleeper.note_written(TabId(1_000), 1);
        assert_eq!(lock(&sleeper.last_written).len(), 1, "those older than the gap have gone");
        assert!(sleeper.may_write_pictures(TabId(499)), "a tab past the gap may write again");
    }
}
