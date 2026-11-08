//! A hang or a huge allocation is a failure, not just a panic. This counts every allocation in the process and ends it past 1 GiB, whatever libFuzzer or a sanitizer is doing, and [`within`] holds one check to its own limits.
//!
//! Live bytes are one global count, so a limit is only exact with one check running: [`crate::smoke`] takes a lock, and libFuzzer runs one input at a time.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// All anything may hold at once. The owner's line: a 1 GiB allocation is a failure.
pub const HARD_LIMIT: usize = 1 << 30;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static DYING: AtomicBool = AtomicBool::new(false);

/// The counting allocator. It's this crate's alone, and nothing that ships depends on this crate.
pub struct Counting;

fn note_grown(by: usize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
    // Writing to stderr can itself allocate, the first time, and that mustn't come back here.
    if live > HARD_LIMIT && !DYING.swap(true, Ordering::Relaxed) {
        // Not a panic: this can be inside the allocator, on any thread, in a job that catches panics.
        let _ = writeln!(
            std::io::stderr(),
            "recto-fuzz: {live} bytes live, past the {HARD_LIMIT} limit (this allocation: {by})"
        );
        std::process::abort();
    }
}

// SAFETY: every call goes straight to `System` with the arguments it was given. The counters only add and subtract numbers, and they never allocate. Writing to stderr on the way to an abort doesn't return to the caller.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_grown(layout.size());
        // SAFETY: the caller's layout, passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_grown(layout.size());
        // SAFETY: the caller's layout, passed on unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller's pointer and layout, passed on unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            note_grown(new_size - layout.size());
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        // SAFETY: the caller's pointer, layout and size, passed on unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// What one check may use.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub time: Duration,
    /// Bytes on top of what was live when it started.
    pub memory: usize,
}

/// The most any one check has used, for a run's report.
pub static MOST_MEMORY: AtomicUsize = AtomicUsize::new(0);
/// The longest any one check has taken, in microseconds, for a run's report.
pub static MOST_MICROS: AtomicUsize = AtomicUsize::new(0);

/// How much a run of code used at its peak, in bytes above what was live when it started.
pub fn peak_of<R>(run: impl FnOnce() -> R) -> (R, usize) {
    let before = LIVE.load(Ordering::Relaxed);
    // These nest, and an outer one mustn't forget what it had seen.
    let outer = PEAK.swap(before, Ordering::Relaxed);
    let result = run();
    let peak = PEAK.fetch_max(outer, Ordering::Relaxed);
    (result, peak.saturating_sub(before))
}

/// Runs a check, and fails it if it took too long or held too much. A budget is far above anything real, so this is about hangs and bombs, not about how fast this machine is today.
pub fn within<R>(what: &str, budget: Budget, run: impl FnOnce() -> R) -> R {
    let started = Instant::now();
    let (result, peak) = peak_of(run);
    let took = started.elapsed();
    MOST_MEMORY.fetch_max(peak, Ordering::Relaxed);
    MOST_MICROS.fetch_max(usize::try_from(took.as_micros()).unwrap_or(usize::MAX), Ordering::Relaxed);
    assert!(took <= budget.time, "{what} took {took:?}, past its {:?}", budget.time);
    assert!(peak <= budget.memory, "{what} held {peak} bytes, past its {}", budget.memory);
    result
}
