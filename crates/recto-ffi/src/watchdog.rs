//! The UI thread's hang watchdog. The host says when its thread starts and stops working, from a run-loop observer, and one thread watches while it's busy.
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
    thread: Mutex<Option<Thread>>,
}

#[derive(Clone)]
pub struct Watchdog(Arc<Shared>);

impl Watchdog {
    pub fn new() -> Self {
        Self(Arc::new(Shared { epoch: Instant::now(), busy_since: AtomicU64::new(0), over_note: AtomicU64::new(0), over_fail: AtomicU64::new(0), worst_ms: AtomicU64::new(0), thread: Mutex::new(None) }))
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
                if let Ok(handle) = std::thread::Builder::new().name("recto-watchdog".into()).stack_size(64 * 1024).spawn(move || watch(&shared)) {
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
            self.0.worst_ms.fetch_max(now.saturating_sub(began), Ordering::Relaxed);
        }
    }

    pub fn report(&self) -> (u64, u64, u64) {
        (self.0.over_note.load(Ordering::Relaxed), self.0.over_fail.load(Ordering::Relaxed), self.0.worst_ms.load(Ordering::Relaxed))
    }

    /// One look, for the thread and for a test.
    #[cfg(test)]
    fn look_at(&self, now_ms: u64, counted: &mut (u64, bool, bool)) {
        look(&self.0, now_ms, counted);
    }
}

/// `counted` remembers which stretch was already counted, so one stall counts once. It's the stretch's start, and whether each threshold has been passed.
fn look(shared: &Shared, now_ms: u64, counted: &mut (u64, bool, bool)) {
    let began = shared.busy_since.load(Ordering::Acquire);
    if began == 0 {
        return;
    }
    if counted.0 != began {
        *counted = (began, false, false);
    }
    let stalled = now_ms.saturating_sub(began);
    shared.worst_ms.fetch_max(stalled, Ordering::Relaxed);
    if stalled >= NOTE_MS && !counted.1 {
        counted.1 = true;
        shared.over_note.fetch_add(1, Ordering::Relaxed);
    }
    if stalled >= FAIL_MS && !counted.2 {
        counted.2 = true;
        shared.over_fail.fetch_add(1, Ordering::Relaxed);
    }
}

fn watch(shared: &Shared) {
    let mut counted = (0, false, false);
    loop {
        if shared.busy_since.load(Ordering::Acquire) == 0 {
            std::thread::park();
            continue;
        }
        std::thread::sleep(LOOK_EVERY);
        look(shared, shared.epoch.elapsed().as_millis() as u64 + 1, &mut counted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a stall goes uncounted, is counted on every look so one stall reads as many, or short stretches of work add up to a stall.
    #[test]
    fn one_stall_counts_once_at_each_threshold() {
        let dog = Watchdog::new();
        let mut counted = (0, false, false);
        dog.0.busy_since.store(100, Ordering::Release);
        for now in [200, 1099, 1100, 1500, 2099, 2100, 2500, 9000] {
            dog.look_at(now, &mut counted);
        }
        assert_eq!(dog.report().0, 1);
        assert_eq!(dog.report().1, 1);
        dog.0.busy_since.store(0, Ordering::Release);
        dog.look_at(20_000, &mut counted);
        assert_eq!(dog.report().1, 1, "asleep isn't stalled");
        dog.0.busy_since.store(30_000, Ordering::Release);
        dog.look_at(30_500, &mut counted);
        assert_eq!(dog.report().0, 1, "a short stretch");
        dog.look_at(32_100, &mut counted);
        assert_eq!(dog.report(), (2, 2, 8900));
    }
}
