//! Per-instance fixtures for the real completion boundary, absent from shipping state.

use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Default)]
pub(super) struct Hooks {
    hold: Mutex<Option<(Sender<()>, Receiver<()>)>>,
    pub(super) refused_spawns: AtomicUsize,
    pub(super) starts: AtomicUsize,
}

impl Hooks {
    pub(super) fn before_publish(&self) {
        let held = { lock(&self.hold).take() };
        if let Some((entered, release)) = held {
            let _ = entered.send(());
            // Only the test owns a release sender. Success and unwind both drop it; no timeout can silently remove the proof's hold.
            let _ = release.recv();
        }
    }
}

/// A real prepared file, taken through the same host handoff as `take_print`.
pub fn handed(printing: &Printing, file: File, serial: u64) -> ReadyPrint {
    *lock(&printing.ready) = Some((file, serial));
    printing.take_ready().expect("fixture fits the host cap").into()
}

/// The next actual publication waits until this sender is dropped, without holding any fixture mutex.
pub fn hold_publication(printing: &Printing) -> (Receiver<()>, Sender<()>) {
    let (entered, held) = channel();
    let (release, waiting) = channel();
    *lock(&printing.hooks.hold) = Some((entered, waiting));
    (held, release)
}

/// Fail this many worker starts without exhausting the machine's threads.
pub fn refuse_spawns(printing: &Printing, count: usize) {
    printing.hooks.refused_spawns.store(count, Ordering::SeqCst);
}

pub fn worker_starts(printing: &Printing) -> usize {
    printing.hooks.starts.load(Ordering::SeqCst)
}

pub fn remaining_refusals(printing: &Printing) -> usize {
    printing.hooks.refused_spawns.load(Ordering::SeqCst)
}
