//! **This page view's render workers, as seen from outside:** which are running, which said their walls were up, and what they hold. QA and the sleeper's memory look take it alone. Never the page view's lock across a call to the kernel: a list is copied, then each is asked.
//!
//! A borrow of the [`Engine`], built inline by [`Engine::workers`]: each method takes the page view's lock exactly as the `Engine` method it was until R3.6
//! did, in the same order with the same locks after it, so the engine's lock map holds as it was.

use super::{Engine, lock};
use recto_platform::isolate;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// **What the page view's thread last said of its workers, for whoever asks from outside, and the system's ask that idle ones go:** one part of the
/// engine's shared state. The thread writes it ([`Seen::publish`], [`Seen::restarted`]) and takes the ask ([`Seen::take_idle_ask`]); [`WorkerFigures`]
/// reads it. The two lists are under one lock, which is never held with the page view's ([`WorkerFigures::bytes`] reads the tab's files under the page
/// view's lock and lets it go before it takes this one) nor across a call to the kernel; the counts are atomics, so nothing waits for them.
#[derive(Default)]
pub(super) struct Seen {
    lists: Mutex<SeenLists>,
    /// Of the workers running, those that said their walls were up.
    walled: AtomicU32,
    /// Of the renderers held, the times a worker ran out of time it hadn't been given (the Mac too busy to run it), and the times one was killed at its deadline. So a page that failed says whether its worker was starved.
    too_busy: AtomicU32,
    deadline_kills: AtomicU32,
    /// The system's asked for memory back: workers with nothing to do go the next time the thread comes round.
    kill_idle: AtomicBool,
}

/// The workers running, and each by the file it has open.
#[derive(Default)]
struct SeenLists {
    pids: Vec<u32>,
    by_file: Vec<(PathBuf, u32)>,
}

impl Seen {
    /// The workers running now, by the file each has open, and how many of them said their walls were up.
    pub(super) fn publish(&self, by_file: Vec<(PathBuf, u32)>, walled: u32, starved: (u32, u32)) {
        self.walled.store(walled, Ordering::Relaxed);
        self.too_busy.store(starved.0, Ordering::Relaxed);
        self.deadline_kills.store(starved.1, Ordering::Relaxed);
        // Built before the lock is taken, as the pids always were: the lock is held for two moves.
        let pids = by_file.iter().map(|(_, pid)| *pid).collect();
        let mut lists = lock(&self.lists);
        lists.pids = pids;
        lists.by_file = by_file;
    }

    /// The thread started again after a panic: none of its workers is running, though what each file had is left as it was said last.
    pub(super) fn restarted(&self) {
        lock(&self.lists).pids = Vec::new();
        self.walled.store(0, Ordering::Relaxed);
    }

    /// The system wants memory back: idle workers go the next time the thread comes round.
    pub(super) fn ask_idle_to_go(&self) {
        self.kill_idle.store(true, Ordering::SeqCst);
    }

    /// Whether idle workers were asked to go since the last look, which takes the ask.
    pub(super) fn take_idle_ask(&self) -> bool {
        self.kill_idle.swap(false, Ordering::SeqCst)
    }
}

/// This page view's render workers, from outside.
#[derive(Clone, Copy)]
pub struct WorkerFigures<'a> {
    engine: &'a Engine,
}

// size: one reference, so taking it costs what calling a method of `Engine` did.
const _: () = assert!(size_of::<WorkerFigures<'static>>() <= size_of::<usize>(), "a facade of the engine grew");

impl Engine {
    /// This page view's render workers, from outside.
    #[inline]
    pub fn workers(&self) -> WorkerFigures<'_> {
        WorkerFigures { engine: self }
    }
}

impl WorkerFigures<'_> {
    /// What this tab's files' workers hold now, from outside. A call to the kernel for each, of which there are few.
    pub fn bytes(&self, tab: u32) -> u64 {
        let files = lock(&self.engine.shared.reader).docs.files_of(tab);
        // Copied, then asked: the lock isn't held across a call to the kernel, so the thread publishing its workers never waits on one.
        let pids: Vec<u32> = lock(&self.engine.shared.workers.lists)
            .by_file
            .iter()
            .filter(|(path, _)| files.contains(path))
            .map(|(_, pid)| *pid)
            .collect();
        pids.iter()
            .filter_map(|pid| isolate::process_memory(*pid))
            .map(|memory| memory.footprint_bytes)
            .sum()
    }

    /// Every worker running now.
    pub fn pids(&self) -> Vec<u32> {
        lock(&self.engine.shared.workers.lists).pids.clone()
    }

    /// How many of the workers running now said their walls were up: the App Sandbox on and their Mach rights dropped.
    pub fn walled(&self) -> u32 {
        self.engine.shared.workers.walled.load(Ordering::Relaxed)
    }

    /// What the renderers held have counted: workers that ran out of time with the Mac too busy to run them, and workers killed at their deadline.
    pub fn starved(&self) -> (u32, u32) {
        let workers = &self.engine.shared.workers;
        (workers.too_busy.load(Ordering::Relaxed), workers.deadline_kills.load(Ordering::Relaxed))
    }

    /// The footprint of every worker running now, from outside.
    pub fn footprint(&self) -> (u64, u64) {
        let pids = lock(&self.engine.shared.workers.lists).pids.clone();
        pids.iter()
            .filter_map(|pid| isolate::process_memory(*pid))
            .fold((0, 0), |(now, peak), memory| (now + memory.footprint_bytes, peak + memory.peak_footprint_bytes))
    }
}
