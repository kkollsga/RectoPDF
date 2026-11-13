//! **This page view's render workers, as seen from outside:** which are running, which said their walls were up, and what they hold. QA and the sleeper's memory look take it alone. Never the page view's lock across a call to the kernel: a list is copied, then each is asked.
//!
//! A borrow of the [`Engine`], built inline by [`Engine::workers`]: each method takes the page view's lock exactly as the `Engine` method it was until R3.6
//! did, in the same order with the same locks after it, so the engine's lock map holds as it was.

use super::{Engine, lock};
use recto_platform::isolate;
use std::sync::atomic::Ordering;

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
        lock(&self.engine.shared.workers_by_file)
            .iter()
            .filter(|(path, _)| files.contains(path))
            .filter_map(|(_, pid)| isolate::process_memory(*pid))
            .map(|memory| memory.footprint_bytes)
            .sum()
    }

    /// Every worker running now.
    pub fn pids(&self) -> Vec<u32> {
        lock(&self.engine.shared.worker_pids).clone()
    }

    /// How many of the workers running now said their walls were up: the App Sandbox on and their Mach rights dropped.
    pub fn walled(&self) -> u32 {
        self.engine.shared.walled_workers.load(Ordering::Relaxed)
    }

    /// The footprint of every worker running now, from outside.
    pub fn footprint(&self) -> (u64, u64) {
        let pids = lock(&self.engine.shared.worker_pids).clone();
        pids.iter()
            .filter_map(|pid| isolate::process_memory(*pid))
            .fold((0, 0), |(now, peak), memory| (now + memory.footprint_bytes, peak + memory.peak_footprint_bytes))
    }
}
