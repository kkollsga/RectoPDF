//! The UI thread's hang watchdog. The host says when its thread starts and stops working, from a run-loop observer, and one thread watches while it's busy.
//!
//! It moved here from `recto-ffi` whole, with its thread, which it starts itself on the first stretch of work; the bridge forwards the host's two calls and `qa state` reads its counts.
//!
//! **At rest this costs nothing:** the watching thread is parked while the UI thread sleeps, so no timer ticks. It wakes only for a stretch of work, and looks every quarter second for as long as that lasts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// A stall this long is noted.
pub const NOTE_MS: u64 = 1000;
/// A stall this long fails a QA run. AGENTS.md: "a stall of 2 s fails the QA run".
pub const FAIL_MS: u64 = 2000;
const LOOK_EVERY: Duration = Duration::from_millis(250);

struct Shared {
    epoch: Instant,
    /// When the current stretch of work began, in ms from the epoch plus one, or zero while the thread sleeps.
    busy_since: AtomicU64,
    /// Counted once per stall, at each threshold.
    over_note: AtomicU64,
    over_fail: AtomicU64,
    worst_ms: AtomicU64,
    /// Blocks of the host's that ran on the UI thread with no stretch under way: work the run loop's wake and sleep didn't bracket.
    unbracketed: AtomicU64,
    /// The stretch each threshold was last counted for, by its start, so the watcher and the stretch's own end count it once between them.
    noted_stretch: AtomicU64,
    failed_stretch: AtomicU64,
    thread: Mutex<Option<Thread>>,
}

#[derive(Clone)]
pub struct Watchdog(Arc<Shared>);

impl Default for Watchdog {
    fn default() -> Self {
        Self::new()
    }
}

impl Watchdog {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Shared {
            epoch: Instant::now(),
            busy_since: AtomicU64::new(0),
            over_note: AtomicU64::new(0),
            over_fail: AtomicU64::new(0),
            worst_ms: AtomicU64::new(0),
            unbracketed: AtomicU64::new(0),
            noted_stretch: AtomicU64::new(0),
            failed_stretch: AtomicU64::new(0),
            thread: Mutex::new(None),
        }))
    }

    /// From the UI thread, each time it wakes. An atomic, and a wake for the watcher if that's parked.
    pub fn busy(&self) {
        let now = self.0.epoch.elapsed().as_millis() as u64 + 1;
        self.0.busy_since.store(now, Ordering::Release);
        let mut thread = self.0.thread.lock().unwrap_or_else(PoisonError::into_inner);
        match &*thread {
            Some(watcher) => watcher.unpark(),
            None => {
                let shared = Arc::clone(&self.0);
                if let Ok(handle) = std::thread::Builder::new()
                    .name("recto-watchdog".into())
                    .stack_size(64 * 1024)
                    .spawn(move || watch(&shared))
                {
                    *thread = Some(handle.thread().clone());
                }
            }
        }
    }

    /// From the UI thread, each time it's about to sleep.
    pub fn idle(&self) {
        let began = self.0.busy_since.swap(0, Ordering::AcqRel);
        if began != 0 {
            let now = self.0.epoch.elapsed().as_millis() as u64 + 1;
            // Counted here too, as it ends: the watcher looks every quarter second, and on a starved machine (CI, 2026-11-11: a 2.3 s stall passed a script) it may not look between 2 s and the end.
            count(&self.0, began, now.saturating_sub(began));
        }
    }

    /// From the UI thread, as it wakes, or (`block`) as a block of the host's begins on it. The run loop's wake and sleep bracket most work, but not all of
    /// it: on CI (run 43315142752, 2026-11-12) a launch's blocks ran with no stretch under way, and a 2.3 s stall in one was never seen. So a block begins
    /// the stretch itself when none is under way, and is counted as one the run loop missed. True if this began the stretch, for the caller to end it with
    /// [`Self::idle`]; a stretch under way is left as it is. Only the UI thread begins and ends stretches, so the look and the store don't race.
    pub fn enter(&self, block: bool) -> bool {
        if self.0.busy_since.load(Ordering::Acquire) != 0 {
            return false;
        }
        if block {
            self.0.unbracketed.fetch_add(1, Ordering::Relaxed);
        }
        self.busy();
        true
    }

    /// How many blocks began a stretch themselves: what the run loop's wake and sleep didn't bracket.
    #[must_use]
    pub fn unbracketed(&self) -> u64 {
        self.0.unbracketed.load(Ordering::Relaxed)
    }

    /// Stalls past [`NOTE_MS`], stalls past [`FAIL_MS`], and the worst in ms.
    #[must_use]
    pub fn report(&self) -> (u64, u64, u64) {
        (
            self.0.over_note.load(Ordering::Relaxed),
            self.0.over_fail.load(Ordering::Relaxed),
            self.0.worst_ms.load(Ordering::Relaxed),
        )
    }

    /// One look, for the thread and for a test.
    #[cfg(test)]
    fn look_at(&self, now_ms: u64) {
        look(&self.0, now_ms);
    }
}

/// The watcher's look at the stretch under way.
fn look(shared: &Shared, now_ms: u64) {
    let began = shared.busy_since.load(Ordering::Acquire);
    if began == 0 {
        return;
    }
    count(shared, began, now_ms.saturating_sub(began));
}

/// A stretch that began then has lasted this long: each threshold it's passed is counted, once for the stretch, whoever sees it first.
fn count(shared: &Shared, began: u64, stalled: u64) {
    shared.worst_ms.fetch_max(stalled, Ordering::Relaxed);
    if stalled >= NOTE_MS && shared.noted_stretch.swap(began, Ordering::AcqRel) != began {
        shared.over_note.fetch_add(1, Ordering::Relaxed);
    }
    if stalled >= FAIL_MS && shared.failed_stretch.swap(began, Ordering::AcqRel) != began {
        shared.over_fail.fetch_add(1, Ordering::Relaxed);
    }
}

fn watch(shared: &Shared) {
    loop {
        if shared.busy_since.load(Ordering::Acquire) == 0 {
            std::thread::park();
            continue;
        }
        std::thread::sleep(LOOK_EVERY); // ratchet: poll (the watcher's look while the UI thread works; parked at rest)
        look(shared, shared.epoch.elapsed().as_millis() as u64 + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a stall goes uncounted, is counted on every look so one stall reads as many, or short stretches of work add up to a stall.
    #[test]
    fn one_stall_counts_once_at_each_threshold() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(100, Ordering::Release);
        for now in [200, 1099, 1100, 1500, 2099, 2100, 2500, 9000] {
            dog.look_at(now);
        }
        assert_eq!(dog.report().0, 1);
        assert_eq!(dog.report().1, 1);
        dog.0.busy_since.store(0, Ordering::Release);
        dog.look_at(20_000);
        assert_eq!(dog.report().1, 1, "asleep isn't stalled");
        dog.0.busy_since.store(30_000, Ordering::Release);
        dog.look_at(30_500);
        assert_eq!(dog.report().0, 1, "a short stretch");
        dog.look_at(32_100);
        assert_eq!(dog.report(), (2, 2, 8900));
    }

    /// Failure (CI, 2026-11-11): a 2.3 s stall passed a QA script, as the watcher, looking each quarter second on a starved machine, didn't look between 2 s and the stall's end. The end counts it too, once between them.
    #[test]
    fn a_stall_the_watcher_didnt_see_is_counted_as_it_ends() {
        let dog = Watchdog::new();
        let began = 1;
        dog.0.busy_since.store(began, Ordering::Release);
        dog.look_at(1_500);
        assert_eq!(dog.report().0, 1, "seen at 1.5 s");
        // It ends at 2.3 s, with no look since.
        count(&dog.0, began, 2_300);
        dog.0.busy_since.store(0, Ordering::Release);
        assert_eq!((dog.report().0, dog.report().1), (1, 1), "counted at its end, and the note only once");
        count(&dog.0, began, 2_400);
        assert_eq!(dog.report().1, 1, "and the failure only once");
    }

    /// Failure (CI, run 43315142752, 2026-11-12): a block the host ran on the UI thread stalled it for 2.3 s and nothing was counted, as the run loop had said
    /// the thread was asleep: the worst stretch seen in that launch was 6 ms. A block with no stretch under way begins and ends one, and is counted as such.
    #[test]
    fn a_block_the_run_loop_didnt_bracket_is_a_stretch_of_its_own() {
        let dog = Watchdog::new();
        let began = dog.enter(true);
        assert!(began, "asleep by the run loop's account, so the block begins the stretch");
        let since = dog.0.busy_since.load(Ordering::Acquire);
        assert_ne!(since, 0);
        dog.look_at(since + 2_300);
        dog.idle();
        assert_eq!(dog.report().1, 1, "the stall in it is counted");
        assert_eq!(dog.0.busy_since.load(Ordering::Acquire), 0, "and the stretch ends with the block");
        assert_eq!(dog.unbracketed(), 1);
        assert!(dog.enter(false), "the run loop's own wake begins a stretch");
        assert_eq!(dog.unbracketed(), 1, "and isn't counted as missed");
    }

    /// Failure: a block inside a stretch the run loop began ends that stretch early, so a stall after it in the same wake goes unseen, or cuts one stall into two.
    #[test]
    fn a_block_inside_a_stretch_leaves_it_alone() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(100, Ordering::Release);
        assert!(!dog.enter(true), "so the block doesn't end it");
        assert_eq!(dog.0.busy_since.load(Ordering::Acquire), 100, "the run loop's stretch goes on");
        dog.look_at(2_200);
        assert_eq!((dog.report().1, dog.unbracketed()), (1, 0));
    }
}
