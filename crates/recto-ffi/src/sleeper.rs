//! **Tabs going dormant and waking, SPEC decision 25.** Owner, 2026-11-07: "It's fine to keep in memory for a while, and then as it goes dormant we store to disk." And: "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
//!
//! **One deadline, and no thread.** A tab left alone for [`DORMANT_AFTER_SECONDS`] goes dormant. There's nothing per tab: the session thread's one wait has this as well as the state's deadline, and it's the soonest of the few tabs that hold anything. Coming due, it does what's asked and works out the next. At rest with nothing warm there's no deadline at all.
//!
//! **Going dormant, once:** the tab's sleep file's written if there's anything new to say, its journal's compacted and read back, and its contents, list, worker and tiles go. A byte budget across the documents off screen and memory pressure make that happen sooner, oldest first. A tab being worked on by an agent or saved doesn't go, whatever's asking: `Session::hold` and `saving` see to that.

use crate::{CoreCall, Inner, lock};
use recto_reader::{SleepMaterial, SleepShot, SleepView};
use recto_platform::surface::PixelSurface;
use recto_reader::layout::PageBox;
use recto_reader::paper::PaperKind;
use recto_platform::FileStamp;
use recto_session::{SleepFacts, TabId};
use recto_session::record::{FileStatus, look_at_file};
use recto_session::sleep::{SleepFile, SleepPicture};
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
    woken_from_file: AtomicU64,
    pressure_events: AtomicU64,
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
    counts: Counts,
}

impl Sleeper {
    pub(crate) fn new() -> Self {
        Self { clock: Instant::now(), after_seconds: AtomicU32::new(DORMANT_AFTER_SECONDS), budget_bytes: AtomicU64::new(LIVE_BUDGET_BYTES), due: AtomicU64::new(0), last_budget_look: Mutex::new(None), write_gap_seconds: AtomicU32::new(SLEEP_WRITE_EVERY_SECONDS), last_written: Mutex::new(HashMap::new()), this_hour: Mutex::new((Instant::now(), 0)), writing: Mutex::new(()), counts: Counts::default() }
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
        let soon = lock(&self.last_written).get(&id).is_some_and(|last| last.elapsed() < Duration::from_secs(u64::from(gap)));
        let mut hour = lock(&self.this_hour);
        if hour.0.elapsed() >= Duration::from_secs(3600) {
            *hour = (Instant::now(), 0);
        }
        !soon && hour.1 < SLEEP_BYTES_PER_HOUR
    }

    fn note_written(&self, id: TabId, bytes: u64) {
        lock(&self.last_written).insert(id, Instant::now());
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
            "sleep_bytes_written": count(&counts.sleep_bytes),
            "woken_from_file": count(&counts.woken_from_file),
            "pressure_events": count(&counts.pressure_events),
        })
    }
}

impl Inner {
    /// Works out the next deadline, after anything that can have changed it. A look at the few tabs that hold anything.
    pub(crate) fn rearm_dormancy(&self) {
        let after = self.sleeper.after_seconds.load(Ordering::SeqCst);
        let wait = self.host.session.with_session(|session| session.next_dormant_at(after).map(|at| at.saturating_sub(session.seconds_now())));
        let due = wait.map_or(0, |seconds| self.sleeper.millis() + u64::from(seconds) * 1000);
        self.sleeper.due.store(due, Ordering::SeqCst);
    }

    /// It's come due, on the session thread. Whatever's due goes dormant, unless a file's being read, when it looks again shortly.
    pub(crate) fn dormancy_due(&self) -> Option<Vec<TabId>> {
        if self.reads_running.load(Ordering::SeqCst) > 0 || self.core.frozen.load(Ordering::SeqCst) {
            self.sleeper.due.store(self.sleeper.millis() + u64::try_from(BUSY_RETRY.as_millis()).unwrap_or(5000), Ordering::SeqCst);
            return None;
        }
        let after = self.sleeper.after_seconds.load(Ordering::SeqCst);
        Some(self.host.session.with_session(|session| session.due_dormant(session.seconds_now(), after)))
    }

    /// **Those tabs go dormant,** on the session thread, those that may: whatever's asking, not the one on screen or one being worked on or saved. Whatever hasn't gone to its journal goes first, or a list couldn't.
    pub(crate) fn go_dormant(&self, tabs: &[TabId], why: Why) -> usize {
        let tabs: Vec<TabId> = self.host.session.with_session(|session| tabs.iter().copied().filter(|id| session.can_go_dormant(*id)).collect());
        if tabs.is_empty() || self.core.frozen.load(Ordering::SeqCst) {
            return 0;
        }
        self.saver.write_journals_now(self);
        let mut gone = 0;
        for &id in &tabs {
            self.write_sleep_for(id, why == Why::Asked);
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
        let Some(facts) = self.host.session.with_session(|session| session.sleep_facts(id)) else { return };
        let own = Some(version_of(facts.sleep.file));
        let Some(material) = self.engine.sleep_material(id.0, own) else { return };
        self.write_with_material(facts, material, now);
    }

    /// A sleep file from what's known and what's on hand. With no pictures to hand, as with a closed window or what the system's taken, what's there already stands, if it's about the same list. Past the limits, a tab that's only moving about keeps what it has, and one with nothing yet has its facts, which is small and says what the PDF would.
    fn write_with_material(&self, facts: SleepFacts, material: SleepMaterial, now: bool) {
        let SleepFacts { id, path, sleep, pictures_allowed } = facts;
        let _one_at_a_time = lock(&self.sleeper.writing);
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
            && let Some(there) = there.filter(|there| there.list_seq == sleep.list_seq && there.pages == sleep.pages)
        {
            sleep.pictures = there.pictures;
        }
        let bytes = self.saver.store.write_sleep(&path, &sleep);
        if bytes > 0 {
            self.sleeper.note_written(id, bytes);
        }
    }

    /// Tabs pushed out of the warm ones, with their pictures, just before the page view lets go of them. A quick copy, of those whose time's come.
    pub(crate) fn pictures_of_those_leaving(&self) -> Vec<(SleepFacts, SleepMaterial)> {
        let left = self.host.session.with_session(|session| session.take_left_warm());
        left.into_iter().filter_map(|facts| {
            let own = Some(version_of(facts.sleep.file));
            let material = self.engine.sleep_material(facts.id.0, own)?;
            Some((facts, material))
        }).collect()
    }

    /// And their sleep files, on a one-shot thread: packing a photograph takes long enough to hold up whatever's asked for next. A flush waits for it.
    pub(crate) fn write_those_leaving(&self, left: Vec<(SleepFacts, SleepMaterial)>) {
        if left.is_empty() {
            return;
        }
        let Some(inner) = self.saver.inner() else { return };
        let _ = std::thread::Builder::new().name("recto-sleep".into()).stack_size(512 * 1024).spawn(move || {
            for (facts, material) in left {
                inner.write_with_material(facts, material, false);
            }
        });
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
    pub(crate) fn give_sleep_view(&self, id: TabId) {
        let read = self.host.session.with_session(|session| session.take_sleep_read(id));
        let sleep = read.or_else(|| {
            let (stamp, path) = self.host.session.with_session(|session| session.sleep_to_read_awake(id))?;
            self.saver.store.read_sleep(stamp, &path)
        });
        let Some(sleep) = sleep else { return };
        let (pages, mark) = self.host.session.with_session(|session| (session.pages_in(id), session.list_mark(id)));
        if sleep.pages != pages || sleep.list_seq != mark {
            return;
        }
        let page_box = |(width, height): (f32, f32)| PageBox { width, height };
        let mut pictures = Vec::with_capacity(sleep.pictures.len());
        for picture in &sleep.pictures {
            if let Some(surface) = surface_of(picture) {
                pictures.push(SleepShot { page: picture.page, scale: picture.scale, paper: if picture.dark { PaperKind::Dark } else { PaperKind::White }, surface });
            }
        }
        self.engine.give_sleep(id.0, SleepView { usual: sleep.usual.map(page_box), sizes: sleep.sizes.iter().map(|(page, width, height)| (*page, page_box((*width, *height)))).collect(), pictures });
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
        let costs: Vec<(TabId, u64)> = warm.iter().map(|id| (*id, self.engine.worker_bytes(id.0) + self.host.session.with_session(|session| session.page_edits(*id).map_or(0, |list| list.history_bytes() as u64)))).collect();
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

    /// The system's asked for memory back: the same as a tab's being left alone, for all that can go, and now. Counted first, so a test can see it came.
    pub(crate) fn memory_pressure(&self, critical: bool) {
        self.sleeper.counts.pressure_events.fetch_add(1, Ordering::Relaxed);
        let tabs = self.host.session.with_session(|session| session.dormant_oldest_first());
        self.go_dormant(&tabs, Why::Pressure);
        self.engine.memory_pressure(critical);
    }
}

fn with_material(mut sleep: SleepFile, material: SleepMaterial) -> SleepFile {
    sleep.usual = material.usual.map(|size| (size.width, size.height));
    sleep.sizes = material.sizes.into_iter().map(|(page, size)| (page, size.width, size.height)).collect();
    sleep.pictures = material.pictures.into_iter().map(|picture| SleepPicture { page: picture.page, scale: picture.scale, width: picture.width, height: picture.height, dark: picture.paper == PaperKind::Dark, bgra: picture.bgra }).collect();
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

/// A file's stamp as it says which version of the file this is: not the disk it's on, whose number can change while the file doesn't.
pub(crate) fn version_of(stamp: FileStamp) -> FileStamp {
    FileStamp { device: 0, ..stamp }
}

/// One thread looks at the one file a tab was woken from its sleep file on, and ends. If it's something else now, the session's told, and it's read after all.
pub(crate) fn check_woken_file(inner: &std::sync::Arc<Inner>, id: TabId) {
    let Some((path, stamp)) = inner.host.session.with_session(|session| Some((session.path_of(id)?, session.own_stamp(id)))) else { return };
    let inner = std::sync::Arc::clone(inner);
    let _ = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let status = look_at_file(&path, stamp);
        if !matches!(&status, FileStatus::Same(now) if Some(*now) == stamp) && !matches!(status, FileStatus::NotLooked) {
            inner.core.push(CoreCall::WokenFileIsOther(id, status));
        }
    });
}
