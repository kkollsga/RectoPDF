//! **Sleep files written off the session's time: one queue, one writer, and a cap on both** (AGENTS rule 1: "No thread, task, buffer or handle per idle item, caps on everything from outside"). Tabs pushed out of the warm ones, and tabs going dormant by themselves (left alone, the budget, memory pressure), queue theirs here. Each tab leaving used to get a thread of its own, which held its pictures while it waited on the one-at-a-time lock: on a slow disk 39 of them at once in `a_thousand_tabs_come_back_cold` (slow mode, 2026-11-11), so threads and memory grew with the tabs leaving. And going dormant wrote on the session thread, which with a slow disk sat behind the files queued while it served nothing else (review 1).
//!
//! **One writer, only while there's work.** The first tab queued starts it; it writes what's queued, oldest first, and ends when there's nothing left, which it finds out under the same lock a tab's queued under, so a tab queued as it ends starts the next one and is never left behind. At rest there's no thread. It's counted from its thread's start to that end, so two at once would show ([`WriterRunning`]), and one that panics leaves the way open for the next.
//!
//! **No timer per tab.** A pushed-out tab's file waits its cool-down ([`crate::sleeper::LEAVING_WAIT_MS`]) in the queue; the one writer waits on the queue's
//! condition variable with the soonest due time as its timeout, looked at again in a loop, so a word missed costs at most that wait ([`Leaving::serve`]). The
//! lock, the condition, the loop and a quit's drain are all here, beside the queue, and tested without the app on a clock the test moves.
//!
//! **Full, memory stays flat**, decided here and tested without the app:
//! - What's held is what's queued and the one being written, which is counted till its write's done, so [`MAX_LEAVING_BYTES`] is all of it.
//! - A tab queued again keeps only its newest: the older is what the newer says, less up to date. One written straight from the page view, as a flush or being asked has it, is newer than what's queued for it, which goes, or isn't written if it's already been taken for writing.
//! - Past [`MAX_LEAVING_BYTES`] of pictures, the oldest queued lose theirs first. What's left of them is small, their facts, and writing that keeps the pictures already in their files when they're still of the same pages (`write_with_material`).
//! - Past [`MAX_LEAVING_TABS`] queued, the oldest still waiting its cool-down is made due now (counted as `hurried`), so flicking through tabs writes their files
//!   rather than dropping them. Only when none is waiting (the writer behind, a slow disk) does the oldest queued go, and past [`MAX_QUEUED_TABS`] the oldest goes
//!   whatever it is (counted as `dropped`). It costs only the quick way back: that tab wakes from its PDF, or from an older file only if that's still about the
//!   same document, and shows bare paper till it's drawn. Its state is in the record, which never waits for any of this.
//! - So what's held is at most [`MAX_QUEUED_TABS`] tabs queued and one in flight, and [`MAX_LEAVING_BYTES`] of pictures across all of them, the one in flight
//!   included.

use crate::lock;
use recto_reader::SleepMaterial;
use recto_session::{Author, SleepFacts, TabId};
use serde_json::{Value, json};
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// **Tabs whose sleep files may wait their cool-down at once.** Each is a tab off screen, so a handful covers going through tabs at any pace a person does; past
/// it, the oldest still waiting is written now rather than wait, and only with none waiting (the writer behind) does the oldest queued go.
pub const MAX_LEAVING_TABS: usize = 8;
/// **Tabs queued at most, whatever they're waiting for:** past it, the oldest goes whatever it is, so what's held never grows with the tabs leaving. One more is
/// in flight, being written. Twice [`MAX_LEAVING_TABS`], so a full set waiting and a full set made due can both be held.
pub const MAX_QUEUED_TABS: usize = 2 * MAX_LEAVING_TABS;
/// Pictures that may wait to be written, in all, the one being written included, whatever comes of the tabs leaving: four tabs' worth at their most (`DocView::left_pictures` keeps 4 MiB a tab), and about twenty letter pages at a Retina window's fit, 730 KiB each.
pub const MAX_LEAVING_BYTES: u64 = 16 * 1024 * 1024;

/// One tab's sleep file, waiting.
struct Waiting {
    facts: SleepFacts,
    material: SleepMaterial,
    /// What its pictures hold.
    bytes: u64,
    /// When it may be written, in the app's clock's milliseconds: a tab pushed out of the warm ones waits ([`Leaves::PushedOut`]), and is handed
    /// back if it's shown again meanwhile; zero is now.
    due: u64,
    /// Whose command caused it: a tab an agent's command pushed out is the agent's, counted against agents' share when it's written.
    caused_by: Author,
}

/// What the writer does next.
#[derive(Debug)]
pub enum Next {
    /// Writes this one, caused by whose command, counted till [`LeavingQueue::done`]. Boxed, as it's most of the size and the others are a word.
    Write(Box<(SleepFacts, SleepMaterial, Author)>),
    /// Nothing's due yet: waits this many milliseconds of the clock, or till it's told something changed.
    Wait(u64),
    /// Nothing's queued: the writer ends.
    End,
}

/// The one the writer's taken, till its write's done.
struct InFlight {
    id: TabId,
    /// The name of the file it's for, as a retirement says it.
    name: String,
    bytes: u64,
    /// Something newer's been written for it since it was taken: it isn't written.
    stale: bool,
    /// Its file was retired since it was taken (the tab closed, saved, or its file changed): it isn't written, and if it was being written, what landed goes.
    retired: bool,
}

/// The name of the sleep file a tab's facts are for.
fn file_name(facts: &SleepFacts) -> String {
    recto_session::record::sleep_file_name(facts.sleep.file, &facts.path)
}

fn bytes_of(facts: &SleepFacts, material: &SleepMaterial) -> u64 {
    let material: usize = material.pictures.iter().map(|picture| picture.bgra.len()).sum();
    let facts: usize = facts.sleep.pictures.iter().map(|picture| picture.bgra.len()).sum();
    (material + facts) as u64
}

/// **The queue,** behind one lock in the sleeper.
#[derive(Default)]
pub struct LeavingQueue {
    queue: VecDeque<Waiting>,
    in_flight: Option<InFlight>,
    /// What's queued and in flight hold.
    bytes: u64,
    /// Whether a writer's been started and not ended, which is whether there's anything queued or being written.
    writer: bool,
    /// Writer threads between their start and their end.
    running: u32,
    peak_bytes: u64,
    peak_tabs: usize,
    peak_running: u32,
    /// Drains (a quit's, a hand-off's) waiting on the queue now.
    drains_waiting: u32,
    /// What's come of the tabs queued, for `qa state`.
    counts: Counts,
}

/// What's come of the tabs queued, counted for `qa state`. Each a `u32`: one per tab leaving at the most, which a session doesn't come near 4 billion of.
#[derive(Default)]
struct Counts {
    writers_started: u32,
    /// Tabs queued again, whose older one went for the newer.
    replaced: u32,
    /// Tabs that lost their pictures to the byte cap.
    pictures_dropped: u32,
    /// Tabs whose sleep file wasn't written, past the cap on tabs.
    dropped: u32,
    /// Taken for writing, and not written, as something newer was.
    stale_skipped: u32,
    /// Queued, and gone as their files were retired.
    retired: u32,
    /// Queued, and handed back to a tab shown again inside its wait.
    handed_back: u32,
    /// Taken out of their cool-down to be written now, past the cap on tabs.
    hurried: u32,
    writer_panics: u32,
    /// Times the writer waited on the queue's condition: each ends with a word or its timeout, so a writer that spins would show here.
    waits: u32,
}

impl LeavingQueue {
    /// Queues a tab's sleep file, due now, for the tests of the rules below.
    #[cfg(test)]
    #[must_use]
    pub fn push(&mut self, facts: SleepFacts, material: SleepMaterial) -> bool {
        self.push_due(facts, material, 0, Author::Owner)
    }

    /// **Queues a tab's sleep file,** caused by `caused_by`'s command, to be written no sooner than `due` by the clock, by the rules above. Says whether a
    /// writer has to be started: only when there isn't one.
    #[must_use]
    pub fn push_due(&mut self, mut facts: SleepFacts, material: SleepMaterial, due: u64, caused_by: Author) -> bool {
        // Nothing held that its sleep file couldn't carry.
        let material = crate::sleep_writer::allowed_material(&mut facts, material);
        if let Some(at) = self.queue.iter().position(|leaving| leaving.facts.id == facts.id) {
            self.remove(at);
            self.counts.replaced += 1;
        }
        let bytes = bytes_of(&facts, &material);
        self.queue.push_back(Waiting {
            facts,
            material,
            bytes,
            due,
            caused_by,
        });
        self.bytes += bytes;
        // Past the cap on tabs, the oldest still in its cool-down is written now rather than dropped (found 2026-11-13: flicking through thirty tabs, every
        // sleep file past the eighth was dropped while it waited, and those tabs woke bare). Only when none is waiting, so the writer is behind (a slow disk),
        // does the oldest go; and past twice the cap, the oldest goes whatever it is, so what's held never grows with the tabs leaving.
        while self.queue.len() > MAX_LEAVING_TABS {
            if self.queue.len() <= MAX_QUEUED_TABS
                && let Some(waiting) = self.queue.iter_mut().find(|leaving| leaving.due > 0)
            {
                waiting.due = 0;
                self.counts.hurried += 1;
                break;
            }
            self.remove(0);
            self.counts.dropped += 1;
        }
        // The oldest first, and the one just queued too if it's more than what's left by itself. The one in flight was under the cap when it was queued.
        let mut at = 0;
        while self.bytes > MAX_LEAVING_BYTES && at < self.queue.len() {
            let leaving = &mut self.queue[at];
            if leaving.bytes > 0 {
                leaving.material.pictures = Vec::new();
                leaving.facts.sleep.pictures = Vec::new();
                self.bytes -= leaving.bytes;
                leaving.bytes = 0;
                self.counts.pictures_dropped += 1;
            }
            at += 1;
        }
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        self.peak_tabs = self.peak_tabs.max(self.queue.len());
        if self.writer {
            return false;
        }
        self.writer = true;
        self.counts.writers_started += 1;
        true
    }

    /// The writer's next, oldest first, whatever's due. `None` is the writer's end. See [`Self::take_due`].
    pub fn take_next(&mut self) -> Option<(SleepFacts, SleepMaterial)> {
        match self.take_due(u64::MAX) {
            Next::Write(leaving) => Some((leaving.0, leaving.1)),
            Next::Wait(_) | Next::End => None,
        }
    }

    /// **The writer's next at `now`:** the oldest that's due, which stays counted till [`Self::done`]; or how long till the soonest is; or the writer's end,
    /// said here under the same lock as [`Self::push`], so whatever's queued after starts another.
    pub fn take_due(&mut self, now: u64) -> Next {
        self.done();
        if self.queue.is_empty() {
            self.writer = false;
            self.running = self.running.saturating_sub(1);
            return Next::End;
        }
        let Some(at) = self.queue.iter().position(|leaving| leaving.due <= now) else {
            let soonest = self.queue.iter().map(|leaving| leaving.due).min().unwrap_or(now);
            return Next::Wait(soonest.saturating_sub(now));
        };
        let Some(leaving) = self.queue.remove(at) else {
            return Next::Wait(0);
        };
        self.in_flight = Some(InFlight {
            id: leaving.facts.id,
            name: file_name(&leaving.facts),
            bytes: leaving.bytes,
            stale: false,
            retired: false,
        });
        Next::Write(Box::new((leaving.facts, leaving.material, leaving.caused_by)))
    }

    /// **Everything queued is due now:** a quit, a hand-off, the app going to the background, or memory pressure won't wait for the cool-down. Says whether a
    /// writer has to be started for it: something's queued and there's none, as after one panicked or couldn't be started.
    #[must_use]
    pub fn hurry(&mut self) -> bool {
        for leaving in &mut self.queue {
            leaving.due = 0;
        }
        if self.writer || self.queue.is_empty() {
            return false;
        }
        self.writer = true;
        self.counts.writers_started += 1;
        true
    }

    /// **A tab shown again while its sleep file waits:** what it was copied with comes back, for its first frame, and isn't written; the tab's queued again when
    /// it next leaves. The one in flight can't be taken back: it isn't written ([`Self::forget`]'s way), and the wake reads what's on disk.
    pub fn take_back(&mut self, id: TabId) -> Option<(SleepFacts, SleepMaterial)> {
        if let Some(flight) = self.in_flight.as_mut().filter(|flight| flight.id == id) {
            flight.stale = true;
        }
        let at = self.queue.iter().position(|leaving| leaving.facts.id == id)?;
        let leaving = self.queue.remove(at)?;
        self.bytes -= leaving.bytes;
        self.counts.handed_back += 1;
        Some((leaving.facts, leaving.material))
    }

    /// Whether nothing's queued or being written.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.in_flight.is_none()
    }

    /// Whether the one in flight is still to be written: asked with the one-at-a-time lock held, so a newer write either came before, and this isn't written, or comes after it.
    pub fn still_wanted(&mut self) -> bool {
        let stale = self.in_flight.as_ref().is_some_and(|flight| flight.stale || flight.retired);
        self.counts.stale_skipped += u32::from(stale);
        !stale
    }

    /// Whether the one in flight's file was retired while it was being written: what landed goes, as the retirement may have been carried out before it did.
    #[must_use]
    pub fn retired_in_flight(&self) -> bool {
        self.in_flight.as_ref().is_some_and(|flight| flight.retired)
    }

    /// The one in flight is written, or skipped: what it held goes.
    pub fn done(&mut self) {
        if let Some(flight) = self.in_flight.take() {
            self.bytes -= flight.bytes;
        }
    }

    /// The writer couldn't be started, or what it needs has gone. What's queued stays, and the next tab queued starts one.
    pub fn writer_gone(&mut self) {
        self.writer = false;
    }

    /// A tab's sleep file's being written from what's newer than what's queued for it: that goes, and if it's been taken for writing already, it isn't written.
    pub fn forget(&mut self, id: TabId) {
        if let Some(at) = self.queue.iter().position(|leaving| leaving.facts.id == id) {
            self.remove(at);
        }
        if let Some(flight) = self.in_flight.as_mut().filter(|flight| flight.id == id) {
            flight.stale = true;
        }
    }

    /// **These sleep files are retired** (a tab closed, saved, redacted, or its file found to be another version), about to be removed by the saver: what's
    /// queued for them goes, and the one in flight, if it's one, isn't written, or what it wrote goes ([`Self::retired_in_flight`]). Asked before the files are
    /// removed, so a write either comes after this and isn't made, or came before and is removed with the rest. Names that aren't sleep files are passed by.
    pub fn retire(&mut self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        while let Some(at) = self.queue.iter().position(|leaving| names.contains(&file_name(&leaving.facts))) {
            self.remove(at);
            self.counts.retired += 1;
        }
        if let Some(flight) = self.in_flight.as_mut().filter(|flight| names.contains(&flight.name)) {
            flight.retired = true;
        }
    }

    fn remove(&mut self, at: usize) {
        if let Some(leaving) = self.queue.remove(at) {
            self.bytes -= leaving.bytes;
        }
    }

    pub fn report(&self) -> Value {
        json!({
            "queued": self.queue.len(),
            "queued_bytes": self.bytes,
            "in_flight": self.in_flight.is_some(),
            "writer": self.writer,
            "writers_running": self.running,
            "peak_tabs": self.peak_tabs,
            "peak_bytes": self.peak_bytes,
            "peak_writers": self.peak_running,
            "writers_started": self.counts.writers_started,
            "cap_tabs": MAX_LEAVING_TABS,
            "cap_queued_tabs": MAX_QUEUED_TABS,
            "cap_bytes": MAX_LEAVING_BYTES,
            "replaced": self.counts.replaced,
            "pictures_dropped": self.counts.pictures_dropped,
            "dropped": self.counts.dropped,
            "stale_skipped": self.counts.stale_skipped,
            "retired": self.counts.retired,
            "handed_back": self.counts.handed_back,
            "hurried": self.counts.hurried,
            "writer_panics": self.counts.writer_panics,
            "waits": self.counts.waits,
            "drains_waiting": self.drains_waiting,
        })
    }
}

/// **A writer thread, from its start:** counted as running, so two at once would show in `peak_writers`. Its end is [`Next::End`]; if it panics instead, this
/// lets the next tab queued (or a drain, or a hurry) start another, as a writer marked there for good would stop every sleep file after, and says so to a drain
/// waiting on it, which would otherwise wait out its limit for nothing.
pub struct WriterRunning<'a> {
    line: &'a Leaving,
}

impl<'a> WriterRunning<'a> {
    pub fn enter(line: &'a Leaving) -> Self {
        let mut held = lock(&line.queue);
        held.running += 1;
        held.peak_running = held.peak_running.max(held.running);
        drop(held);
        Self { line }
    }
}

impl Drop for WriterRunning<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut queue = lock(&self.line.queue);
        queue.done();
        queue.writer = false;
        queue.running = queue.running.saturating_sub(1);
        queue.counts.writer_panics += 1;
        drop(queue);
        self.line.moved.notify_all();
    }
}

/// Why a tab's sleep file is queued, which says when it's due: the one place that says a tab going dormant doesn't wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaves {
    /// Pushed out of the warm ones, by whose command: it waits its cool-down ([`crate::sleeper::LEAVING_WAIT_MS`]), and is handed back if it's shown again
    /// meanwhile.
    PushedOut(Author),
    /// Going dormant by itself (left alone, the budget, memory pressure): written as soon as the writer gets to it.
    Dormant,
}

/// **The queue and what waits on it:** its one short lock, the condition the writer and a drain wait on, and the cool-down. Everything that changes what the
/// writer waits for changes it under the lock, so a word can't fall between the writer's look and its wait; the UI thread never calls here (going to the
/// background asks the saver's pass to hurry, [`crate::core::sleeper::Sleeper::hurry_leaving_soon`]). Held by the sleeper; the writer's loop is
/// [`Self::serve`].
pub struct Leaving {
    queue: Mutex<LeavingQueue>,
    /// Said to whenever the queue changes in a way the writer or a drain should look again for: something queued, hurried, taken back, retired or written.
    moved: Condvar,
    /// How long a pushed-out tab's sleep file waits, in milliseconds of the app's clock: [`crate::sleeper::LEAVING_WAIT_MS`], set small by tests.
    wait_ms: AtomicU64,
    /// A test's: the next writer panics as it starts, as one that met a bug would.
    #[cfg(test)]
    panic_next: AtomicBool,
    /// A test's: run once by the writer between its look at the queue and its wait, with the queue's lock held, as a word can land there.
    #[cfg(test)]
    before_wait: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Default for Leaving {
    fn default() -> Self {
        Self::new(crate::sleeper::LEAVING_WAIT_MS)
    }
}

impl Leaving {
    #[must_use]
    pub fn new(wait_ms: u64) -> Self {
        Self {
            queue: Mutex::new(LeavingQueue::default()),
            moved: Condvar::new(),
            wait_ms: AtomicU64::new(wait_ms),
            #[cfg(test)]
            panic_next: AtomicBool::new(false),
            #[cfg(test)]
            before_wait: Mutex::new(None),
        }
    }

    /// How long a pushed-out tab's sleep file waits, for tests.
    pub fn set_wait(&self, ms: u64) {
        self.wait_ms.store(ms, Ordering::SeqCst);
        self.moved.notify_all();
    }

    /// **Queues a tab's sleep file** at `now` by the app's clock, due by why it's leaving ([`Leaves`]). Says whether a writer has to be started.
    #[must_use]
    pub fn push(&self, facts: SleepFacts, material: SleepMaterial, leaves: Leaves, now: u64) -> bool {
        let wait = self.wait_ms.load(Ordering::SeqCst);
        let (due, caused_by) = match leaves {
            Leaves::PushedOut(by) if wait > 0 => (now.saturating_add(wait), by),
            Leaves::PushedOut(by) => (0, by),
            Leaves::Dormant => (0, Author::Owner),
        };
        let start = lock(&self.queue).push_due(facts, material, due, caused_by);
        self.moved.notify_all();
        start
    }

    /// A tab shown again: what's waiting for it comes back ([`LeavingQueue::take_back`]).
    pub fn take_back(&self, id: TabId) -> Option<(SleepFacts, SleepMaterial)> {
        let back = lock(&self.queue).take_back(id);
        self.moved.notify_all();
        back
    }

    /// A tab's file is being written from what's newer ([`LeavingQueue::forget`]).
    pub fn forget(&self, id: TabId) {
        lock(&self.queue).forget(id);
        self.moved.notify_all();
    }

    /// These sleep files are about to be removed, as retired ([`LeavingQueue::retire`]).
    pub fn retire(&self, names: &[String]) {
        lock(&self.queue).retire(names);
        self.moved.notify_all();
    }

    /// **Everything waiting is due now,** under the queue's lock, so a writer between its look and its wait sees it when it next looks, the word said after it
    /// waits. Says whether a writer has to be started for it ([`LeavingQueue::hurry`]). Takes the lock: not for the UI thread.
    #[must_use]
    pub fn hurry(&self) -> bool {
        let start = lock(&self.queue).hurry();
        self.moved.notify_all();
        start
    }

    /// The writer looks at the queue again, as the clock's moved on (a test's clock, which no timeout follows).
    pub fn poke(&self) {
        self.moved.notify_all();
    }

    /// The writer couldn't be started ([`LeavingQueue::writer_gone`]): a drain waiting on it returns.
    pub fn writer_gone(&self) {
        lock(&self.queue).writer_gone();
        self.moved.notify_all();
    }

    /// Whether the one in flight is still to be written ([`LeavingQueue::still_wanted`]): asked with the one-at-a-time lock held.
    pub fn still_wanted(&self) -> bool {
        lock(&self.queue).still_wanted()
    }

    /// Whether the one in flight's file was retired while it was being written ([`LeavingQueue::retired_in_flight`]).
    pub fn retired_in_flight(&self) -> bool {
        lock(&self.queue).retired_in_flight()
    }

    /// **Waits until nothing's queued or being written, or until `until`,** whichever's first (a quit's, a hand-off's): past it, what's left is written after or
    /// not at all, which costs those tabs only their quick way back. With no writer to wait for (one that panicked, or couldn't be started), it returns at once:
    /// waiting would only burn the limit. Says whether it all went. Real time, as it waits on another thread's disk.
    pub fn drain(&self, until: Instant) -> bool {
        let mut held = lock(&self.queue);
        held.drains_waiting += 1;
        let drained = loop {
            if held.is_idle() {
                break true;
            }
            if !held.writer {
                break false;
            }
            let Some(left) = until.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()) else {
                break false;
            };
            held = self
                .moved
                .wait_timeout(held, left)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(held, _)| held);
        };
        held.drains_waiting -= 1;
        drained
    }

    /// **The one writer's loop,** on its thread, till the queue's empty: the next due by `now` (the app's clock), handed to `write`, then let go of. With nothing
    /// due it waits on the queue's condition for the soonest due time, looked at again each time round, so a word missed costs at most that wait, and it never
    /// spins: each wait ends with a word or its timeout (counted as `waits`).
    pub fn serve(&self, now: impl Fn() -> u64, mut write: impl FnMut(SleepFacts, SleepMaterial, Author)) {
        let _running = WriterRunning::enter(self);
        #[cfg(test)]
        assert!(!self.panic_next.swap(false, Ordering::SeqCst), "a sleep writer panicking, as a test asked");
        while let Some((facts, material, caused_by)) = self.next(&now) {
            write(facts, material, caused_by);
            lock(&self.queue).done();
            self.moved.notify_all();
        }
    }

    /// The writer's next: waits for one to come due, or says there's none left (the writer's end, said under the lock a tab's queued under).
    fn next(&self, now: &impl Fn() -> u64) -> Option<(SleepFacts, SleepMaterial, Author)> {
        let mut held = lock(&self.queue);
        loop {
            match held.take_due(now()) {
                Next::Write(leaving) => return Some(*leaving),
                Next::End => return None,
                Next::Wait(ms) => {
                    held.counts.waits += 1;
                    #[cfg(test)]
                    if let Some(then) = lock(&self.before_wait).take() {
                        then();
                    }
                    held = self
                        .moved
                        .wait_timeout(held, Duration::from_millis(ms.max(1)))
                        .map_or_else(|poisoned| poisoned.into_inner().0, |(held, _)| held);
                }
            }
        }
    }

    /// The writer runs `then` once, between its next look at the queue and its wait, with the queue's lock held.
    #[cfg(test)]
    fn before_next_wait(&self, then: impl FnOnce() + Send + 'static) {
        *lock(&self.before_wait) = Some(Box::new(then));
    }

    /// The next writer panics as it starts, for a test of what's left after one.
    #[cfg(test)]
    pub fn panic_next_writer(&self) {
        self.panic_next.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn report(&self) -> Value {
        lock(&self.queue).report()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_platform::FileStamp;
    use recto_reader::RawPicture;
    use recto_reader::paper::PaperKind;
    use recto_session::sleep::SleepFile;

    fn facts(id: u32) -> SleepFacts {
        SleepFacts {
            id: TabId(id),
            path: format!("/t{id}.pdf").into(),
            sleep: SleepFile {
                file: FileStamp {
                    size: 1,
                    mtime_ns: 1,
                    device: 1,
                    inode: u64::from(id),
                },
                file_pages: 3,
                pages: 3,
                list_seq: 0,
                renderer_only: false,
                version: Some(1),
                toc: Default::default(),
                usual: None,
                sizes: Vec::new(),
                pictures: Vec::new(),
            },
            pictures_allowed: true,
        }
    }

    /// A tab's material with one picture of this many bytes, its first byte saying which of a tab's it is.
    fn material(bytes: usize, mark: u8) -> SleepMaterial {
        let mut bgra = vec![0; bytes];
        if let Some(first) = bgra.first_mut() {
            *first = mark;
        }
        SleepMaterial {
            version: Some(1),
            usual: None,
            sizes: Vec::new(),
            pictures: vec![RawPicture {
                page: 0,
                scale: 1.0,
                width: 1,
                height: 1,
                paper: PaperKind::White,
                bgra,
            }],
            in_view: vec![0],
        }
    }

    const MIB: usize = 1024 * 1024;

    /// Failure (slow mode, 2026-11-11): each tab leaving the warm ones got a writer thread of its own, holding its pictures, so with a slow disk threads and memory grew with the tabs leaving (39 threads at once). However many leave while the writer's busy, one writer is started, the tabs queued and their pictures stay under their caps, and those past the cap on tabs are counted.
    #[test]
    fn however_many_tabs_leave_there_is_one_writer_and_the_queue_is_capped() {
        let mut queue = LeavingQueue::default();
        let mut started = 0;
        for id in 0..1000 {
            started += usize::from(queue.push(facts(id), material(3 * MIB, 0)));
            assert!(queue.queue.len() <= MAX_LEAVING_TABS);
            assert!(queue.bytes <= MAX_LEAVING_BYTES, "{} bytes queued", queue.bytes);
        }
        assert_eq!(started, 1, "one writer, however many tabs leave while it writes");
        let report = queue.report();
        assert_eq!(report["dropped"], 1000 - MAX_LEAVING_TABS as u64, "{report}");
        // The newest are the ones kept, as the tab most likely to come back is the one just left.
        let kept: Vec<u32> = queue.queue.iter().map(|leaving| leaving.facts.id.0).collect();
        assert_eq!(kept, ((1000 - MAX_LEAVING_TABS as u32)..1000).collect::<Vec<_>>());
    }

    /// Failure: a tab that leaves, comes back and leaves again before its file's written is written twice, the older last, or holds two sets of pictures. Only its newest is kept, in the newest place.
    #[test]
    fn a_tab_queued_twice_keeps_only_its_newest() {
        let mut queue = LeavingQueue::default();
        assert!(queue.push(facts(1), material(MIB, 1)));
        assert!(!queue.push(facts(2), material(MIB, 0)));
        assert!(!queue.push(facts(1), material(MIB, 2)));
        assert_eq!(queue.queue.len(), 2);
        assert_eq!(queue.bytes, 2 * MIB as u64);
        assert_eq!(queue.report()["replaced"], 1);
        let (first, _) = queue.take_next().unwrap();
        assert_eq!(first.id, TabId(2), "the one queued again went to the back");
        let (second, newest) = queue.take_next().unwrap();
        assert_eq!(second.id, TabId(1));
        assert_eq!(newest.pictures[0].bgra[0], 2, "its newest pictures");
        assert!(queue.take_next().is_none());
    }

    /// Failure: the cap on bytes is held by dropping whole tabs, or the newest, so tabs that could have had their facts written have none. Over the bytes, the oldest lose their pictures first and keep their place, and the newest keeps its own.
    #[test]
    fn over_the_byte_cap_the_oldest_lose_their_pictures_first() {
        let mut queue = LeavingQueue::default();
        let each = MAX_LEAVING_BYTES as usize / 3 + 1;
        let _ = queue.push(facts(1), material(each, 0));
        let _ = queue.push(facts(2), material(each, 0));
        let _ = queue.push(facts(3), material(each, 0));
        assert_eq!(queue.queue.len(), 3, "no tab goes for its bytes");
        assert!(queue.bytes <= MAX_LEAVING_BYTES);
        assert!(queue.queue[0].material.pictures.is_empty(), "the oldest lost its pictures");
        assert!(!queue.queue[2].material.pictures.is_empty(), "the newest kept its own");
        assert_eq!(queue.report()["pictures_dropped"], 1);
        // One more than the whole cap by itself keeps its facts only.
        let _ = queue.push(facts(4), material(MAX_LEAVING_BYTES as usize + 1, 0));
        assert!(queue.bytes <= MAX_LEAVING_BYTES);
        assert_eq!(queue.queue.len(), 4);
    }

    /// Failure: the writer stays when there's nothing to write, a thread held at rest; or a tab queued as it ends finds it still there and is never written. Its end is said under the queue's lock, and the next tab starts another.
    #[test]
    fn the_writer_ends_with_the_queue_and_the_next_tab_starts_another() {
        let mut queue = LeavingQueue::default();
        assert!(queue.push(facts(1), material(10, 0)));
        assert!(queue.take_next().is_some());
        assert!(!queue.push(facts(2), material(10, 0)), "still writing");
        assert!(queue.take_next().is_some());
        assert!(queue.take_next().is_none(), "nothing left, so it ends");
        assert_eq!(queue.report()["writer"], false);
        assert!(queue.push(facts(3), material(10, 0)), "a tab after its end starts another");
        // A writer that couldn't start leaves what's queued for the next.
        queue.writer_gone();
        assert!(queue.push(facts(4), material(10, 0)));
        assert_eq!(queue.queue.len(), 2);
    }

    /// Failure: a tab whose file's written straight from the page view, as a flush or going dormant has it, is written again after from the older pictures queued for it, overwriting the newer. What's queued for it goes.
    #[test]
    fn a_tab_written_from_newer_forgets_what_is_queued_for_it() {
        let mut queue = LeavingQueue::default();
        let _ = queue.push(facts(1), material(MIB, 0));
        let _ = queue.push(facts(2), material(MIB, 0));
        queue.forget(TabId(1));
        queue.forget(TabId(9));
        assert_eq!(queue.queue.len(), 1);
        assert_eq!(queue.bytes, MIB as u64);
        assert_eq!(queue.take_next().unwrap().0.id, TabId(2));
    }

    /// Failure (the cool-down, owner 2026-11-13): a pushed-out tab's file written before its wait is up (the drive's wear for a tab about to be shown again), or
    /// left waiting past it, or a quit's hurry not taking it, or a tab shown again getting nothing back and its file written anyway. At the wait less one it
    /// waits, and says how long; at the wait it's written; one due now goes before one waiting; hurried, all are due; taken back, it's the tab's and not written.
    #[test]
    fn a_sleep_file_waits_its_cool_down_unless_hurried_or_taken_back() {
        let wait = 5;
        let mut queue = LeavingQueue::default();
        let _ = queue.push_due(facts(1), material(10, 1), 100 + wait, Author::Owner);
        assert!(matches!(queue.take_due(100 + wait - 1), Next::Wait(1)), "at the wait less one");
        let _ = queue.push_due(facts(2), material(10, 2), 0, Author::Owner);
        assert!(
            matches!(queue.take_due(101), Next::Write(leaving) if leaving.0.id == TabId(2)),
            "going dormant doesn't wait"
        );
        assert!(
            matches!(queue.take_due(100 + wait), Next::Write(leaving) if leaving.0.id == TabId(1)),
            "at the wait"
        );
        let _ = queue.push_due(facts(3), material(10, 3), 200 + wait, Author::Owner);
        let _ = queue.push_due(facts(4), material(10, 4), 200 + wait, Author::Owner);
        let (back, material) = queue.take_back(TabId(3)).expect("handed back");
        assert_eq!((back.id, material.pictures[0].bgra[0]), (TabId(3), 3));
        assert!(!queue.hurry(), "a writer's there");
        assert!(matches!(queue.take_due(200), Next::Write(leaving) if leaving.0.id == TabId(4)), "hurried");
        assert!(matches!(queue.take_due(200), Next::End));
        assert_eq!(queue.report()["handed_back"], 1);
    }

    /// Failure (the cool-down's first full check, 2026-11-13, `a_long_session_stays_flat`: 4 of 60 wakes from a sleep file): with every pushed-out tab waiting
    /// its cool-down, flicking past more than the cap on tabs dropped the oldest, unwritten, and those tabs woke bare. Past the cap, the oldest waiting is
    /// written now, not dropped; with none waiting (the writer behind) the oldest goes, as before; and what's queued stays within twice the cap.
    #[test]
    fn past_the_cap_on_tabs_the_oldest_waiting_is_written_now_not_dropped() {
        let mut queue = LeavingQueue::default();
        for id in 1..=(MAX_QUEUED_TABS as u32 + 3) {
            let _ = queue.push_due(facts(id), material(10, 0), 1_000, Author::Owner);
            assert!(queue.queue.len() <= MAX_QUEUED_TABS, "{} queued", queue.queue.len());
        }
        let report = queue.report();
        assert!(report["hurried"].as_u64().unwrap() >= MAX_LEAVING_TABS as u64, "{report}");
        assert_eq!(report["dropped"], 3, "{report}");
        assert!(
            matches!(queue.take_due(0), Next::Write(leaving) if leaving.0.id == TabId(4)),
            "the oldest left, now due"
        );
    }

    /// Failure (the sleeper's review, 2026-11-13): a tab's sleep file retired (closed, saved, its file changed) was still written from what the queue held, after
    /// the retirement was carried out, so a closed document's picture and contents stayed on disk. And the other way: a retirement that drops other tabs' files
    /// costs them their quick way back for nothing. What's queued for a retired name goes, and its bytes; the one in flight for it isn't written, and says it was
    /// retired so what it wrote goes; other tabs' and names that aren't sleep files are left alone.
    #[test]
    fn a_retired_sleep_file_is_not_written_from_the_queue_and_others_stay() {
        let mut queue = LeavingQueue::default();
        let _ = queue.push(facts(1), material(MIB, 0));
        let _ = queue.push(facts(2), material(MIB, 0));
        let _ = queue.push(facts(3), material(MIB, 0));
        assert_eq!(queue.take_next().unwrap().0.id, TabId(1));
        queue.retire(&["journal-0.log".to_string()]);
        assert!(queue.still_wanted() && !queue.retired_in_flight(), "not a name of its");
        queue.retire(&[file_name(&facts(1)), file_name(&facts(3))]);
        assert!(queue.retired_in_flight(), "what the one in flight wrote goes");
        assert!(!queue.still_wanted(), "not written if it hasn't been");
        queue.done();
        assert_eq!(queue.bytes, MIB as u64, "only tab 2's held");
        assert_eq!(queue.take_next().unwrap().0.id, TabId(2));
        assert!(queue.still_wanted() && !queue.retired_in_flight());
        assert!(queue.take_next().is_none());
        assert_eq!(queue.report()["retired"], 1);
    }

    /// Failure (review 1): the file being written isn't counted once it's taken from the queue, so what's held is the cap and one more tab's pictures, and the peak says less than there is. It counts till its write's done, and what's queued after it gives way.
    #[test]
    fn what_is_being_written_counts_against_the_cap_till_it_is_done() {
        let mut queue = LeavingQueue::default();
        let big = MAX_LEAVING_BYTES as usize * 5 / 8;
        let _ = queue.push(facts(1), material(big, 0));
        assert!(queue.take_next().is_some());
        assert_eq!(queue.bytes, big as u64, "still held while it's written");
        let _ = queue.push(facts(2), material(big, 0));
        assert!(queue.bytes <= MAX_LEAVING_BYTES, "{} held", queue.bytes);
        assert!(queue.queue[0].material.pictures.is_empty(), "what's queued gave way to what's being written");
        assert!(queue.report()["peak_bytes"].as_u64().unwrap() <= MAX_LEAVING_BYTES);
        queue.done();
        assert_eq!(queue.bytes, 0);
        let _ = queue.take_next();
        let _ = queue.take_next();
        assert_eq!(queue.bytes, 0, "nothing's held at rest");
        assert_eq!(queue.report()["in_flight"], false);
    }

    /// Failure (review 1): a tab's older pictures, taken for writing just before a flush wrote its newer ones, are written after them and overwrite them, as `forget` looked only in the queue. Marked, it isn't written; another tab's write doesn't mark it.
    #[test]
    fn a_tab_written_newer_while_its_older_is_taken_isnt_overwritten() {
        let mut queue = LeavingQueue::default();
        let _ = queue.push(facts(1), material(10, 0));
        let _ = queue.push(facts(2), material(10, 0));
        assert_eq!(queue.take_next().unwrap().0.id, TabId(1));
        queue.forget(TabId(2));
        assert!(queue.still_wanted(), "another tab's write says nothing about this one");
        queue.forget(TabId(1));
        assert!(!queue.still_wanted(), "something newer's been written for it");
        assert_eq!(queue.report()["stale_skipped"], 1);
        assert!(queue.take_next().is_none(), "and the other was forgotten");
    }

    /// Failure (review 1): a writer that panics leaves the queue saying there's a writer for good, so no sleep file is ever written again. Its guard says it's gone, what it held goes, and the next tab queued starts another.
    #[test]
    fn a_writer_that_panics_lets_the_next_one_start() {
        let line = Leaving::new(0);
        assert!(lock(&line.queue).push(facts(1), material(10, 0)));
        let ended = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _running = WriterRunning::enter(&line);
                    let _taken = lock(&line.queue).take_next();
                    panic!("a writer panicking, on purpose");
                })
                .join()
        });
        assert!(ended.is_err());
        let mut queue = lock(&line.queue);
        let report = queue.report();
        assert_eq!(report["writer"], false, "{report}");
        assert_eq!(report["writers_running"], 0);
        assert_eq!(report["queued_bytes"], 0, "what it held went with it");
        assert_eq!(report["writer_panics"], 1);
        assert!(queue.push(facts(2), material(10, 0)), "the next tab starts another");
    }

    /// Failure (review 1): two writers run at once, as one is started while the last is still ending, or one stays when there's nothing to write. Tabs queued from four threads at once, each writer counted from its thread's start: never more than one, and none at rest with nothing held.
    #[test]
    fn writers_never_overlap_and_none_is_left_at_rest() {
        let line = Leaving::new(0);
        let queue = &line.queue;
        let write = || {
            let _running = WriterRunning::enter(&line);
            loop {
                let next = lock(queue).take_next();
                if next.is_none() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        };
        std::thread::scope(|scope| {
            for from in 0..4u32 {
                let write = &write;
                scope.spawn(move || {
                    for at in 0..200 {
                        if lock(queue).push(facts(from * 1000 + at), material(1024, 0)) {
                            scope.spawn(write);
                        }
                        if at % 50 == 0 {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                    }
                });
            }
        });
        let report = lock(queue).report();
        assert_eq!(report["peak_writers"], 1, "{report}");
        assert!(report["writers_started"].as_u64().unwrap() >= 1);
        assert_eq!(report["writer"], false, "{report}");
        assert_eq!(report["writers_running"], 0);
        assert_eq!(report["queued"], 0);
        assert_eq!(report["queued_bytes"], 0);
    }

    /// Failure (the cool-down's review, 2026-11-13, S1): a document with a page from a protected file never has pictures in its sleep file, yet a pushed-out
    /// tab's pictures were held in the queue through its cool-down (up to the whole 16 MiB) and handed back to its page view if it was shown again. Queued, it
    /// holds no pictures and no bytes, and what's handed back has none.
    #[test]
    fn a_tab_not_allowed_pictures_has_none_held_or_handed_back() {
        let mut queue = LeavingQueue::default();
        let mut protected = facts(1);
        protected.pictures_allowed = false;
        let _ = queue.push_due(protected, material(MIB, 0), 1_000, Author::Owner);
        assert_eq!(queue.bytes, 0, "held for nothing");
        let (_, back) = queue.take_back(TabId(1)).expect("handed back");
        assert!(back.pictures.is_empty(), "pictures handed back that no sleep file of it may carry");
    }

    /// A clock a test moves, in milliseconds.
    #[derive(Default)]
    struct TestClock(AtomicU64);

    impl TestClock {
        fn now(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        fn set(&self, ms: u64) {
            self.0.store(ms, Ordering::SeqCst);
        }
    }

    /// Whatever a test's writer is waiting for is written as the test ends, a failed assertion included, so a scope never waits on a writer for ever.
    struct EndsTheWriter<'a>(&'a Leaving);

    impl Drop for EndsTheWriter<'_> {
        fn drop(&mut self) {
            let _ = self.0.hurry();
        }
    }

    /// Waits for `done`, bounded, polling: never a fixed sleep then a count.
    fn until(what: &str, done: impl Fn() -> bool) {
        let began = Instant::now();
        while !done() {
            assert!(began.elapsed() < Duration::from_secs(10), "{what}");
            std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
        }
    }

    /// Failure (the cool-down's review, 2026-11-13): "a tab going dormant doesn't wait" was said in two places, the queue's caller pushing with no due time and a
    /// rule with a flag no caller passed false, so a change to one left the other saying otherwise. One place: a pushed-out tab is due at its wait, a tab going
    /// dormant at once, and with no wait set, both at once.
    #[test]
    fn a_pushed_out_tab_waits_its_cool_down_and_a_dormant_one_does_not() {
        let line = Leaving::new(5);
        let _ = line.push(facts(1), material(10, 0), Leaves::PushedOut(Author::Owner), 100);
        let _ = line.push(facts(2), material(10, 0), Leaves::Dormant, 100);
        line.set_wait(0);
        let _ = line.push(facts(3), material(10, 0), Leaves::PushedOut(Author::Owner), 100);
        let dues: Vec<u64> = lock(&line.queue).queue.iter().map(|waiting| waiting.due).collect();
        assert_eq!(dues, [105, 0, 0]);
    }

    /// Failure (the cool-down's review, 2026-11-13): the writer's wait loop, untested in the hub it lived in, could spin on a clock that doesn't move, miss an item
    /// that came due with no word said, or stay when the queue's empty. With a clock the test moves and a writer that only notes what it's handed: a file due
    /// later waits, through words that change nothing, without spinning; one that comes due with no word is written at its timeout; one due now is written;
    /// and with nothing left the writer ends and says so. (The second review: the push and the change of wait both say a word, and the clock moved
    /// nanoseconds after, so a writer waiting with no timeout at all passed. Now the writer is seen waiting at the clock's zero, after the push, before the
    /// clock moves with no word said: a writer whose wait has no timeout never sees it come due, which was seen failing.)
    #[test]
    fn the_writer_waits_without_spinning_misses_nothing_due_and_ends_with_the_queue() {
        let line = Leaving::new(60_000);
        let clock = TestClock::default();
        let written = Mutex::new(Vec::new());
        let waits = || line.report()["waits"].as_u64().unwrap();
        assert!(
            line.push(facts(1), material(10, 0), Leaves::PushedOut(Author::Owner), 0),
            "the first starts a writer"
        );
        std::thread::scope(|scope| {
            let writer = scope.spawn(|| line.serve(|| clock.now(), |facts, _, _| lock(&written).push(facts.id)));
            let _ends = EndsTheWriter(&line);
            until("the writer waiting", || waits() >= 1);
            for _ in 0..3 {
                line.poke();
            }
            assert!(lock(&written).is_empty(), "written before its wait");
            // Each wait ends with a word or its timeout: the first, three words and the change of wait at most, where a spinning writer would count thousands.
            line.set_wait(1);
            assert!(waits() <= 5, "the writer spun: {}", line.report());
            // Due at 1 with the clock at 0: the writer, seen waiting after the push (its waits counted since, each a millisecond's timeout on a clock that
            // stands still), has looked at it at 0 and found it not due. Then the clock moves with no word said: only the wait's own timeout finds it.
            let _ = line.push(facts(2), material(10, 0), Leaves::PushedOut(Author::Owner), clock.now());
            let after_push = waits();
            until("the writer waiting at 0, after the push", || waits() > after_push);
            clock.set(1);
            until("the one come due written", || lock(&written).contains(&TabId(2)));
            clock.set(60_000);
            line.poke();
            writer.join().expect("the writer ends");
        });
        assert_eq!(*lock(&written), [TabId(2), TabId(1)]);
        let report = line.report();
        assert_eq!(report["writer"], false, "{report}");
        assert_eq!(report["writers_running"], 0, "{report}");
    }

    /// Failure (the cool-down's review, 2026-11-13, S4): after a writer panicked, what it left queued had no thread, and a quit's drain waited for it to its
    /// limit, then went without the tab on screen's file. A drain with no writer returns at once; a hurry with something queued and no writer says to start one.
    #[test]
    fn a_drain_with_no_writer_returns_at_once_and_a_hurry_starts_one() {
        let line = Leaving::new(1_000);
        assert!(line.push(facts(1), material(10, 0), Leaves::PushedOut(Author::Owner), 0));
        line.writer_gone();
        let began = Instant::now();
        assert!(!line.drain(began + Duration::from_secs(5)), "nothing to wait for");
        assert!(began.elapsed() < Duration::from_millis(2_500), "the drain waited for a writer there isn't");
        assert!(line.hurry(), "something's queued and there's no writer");
        assert!(!line.hurry(), "one's been started");
    }

    /// Failure (the sleep branch's second review, 2026-11-13): going to the background hurried from the UI thread with an atomic and a word said with no lock,
    /// and the writer held the queue's lock from its look at that atomic to its wait, so a word said in between was lost and the waiting files waited out their
    /// cool-down (20 s). The writer held between its look and its wait, a hurry asked there: let go, the file waiting is written now, not at its wait.
    #[test]
    fn a_hurry_landing_between_the_writers_look_and_its_wait_is_not_lost() {
        let line = Leaving::new(60_000);
        let written = Mutex::new(Vec::new());
        assert!(line.push(facts(1), material(10, 0), Leaves::PushedOut(Author::Owner), 0));
        let (at_wait, writer_at_wait) = std::sync::mpsc::channel();
        let (go, gone) = std::sync::mpsc::channel::<()>();
        line.before_next_wait(move || {
            let _ = at_wait.send(());
            let _ = gone.recv();
        });
        std::thread::scope(|scope| {
            let writer = scope.spawn(|| line.serve(|| 0, |facts, _, _| lock(&written).push(facts.id)));
            let _ends = EndsTheWriter(&line);
            writer_at_wait
                .recv_timeout(Duration::from_secs(10))
                .expect("the writer between its look and its wait");
            let hurrying = scope.spawn(|| line.hurry());
            // The hurry has its chance to land in the window: it has, or it waits for the lock, which is the fix.
            let began = Instant::now();
            while !hurrying.is_finished() && began.elapsed() < Duration::from_millis(300) {
                std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
            }
            drop(go);
            until("the file hurried, written now", || lock(&written).contains(&TabId(1)));
            hurrying.join().expect("the hurry returns");
            writer.join().expect("the writer ends");
        });
    }

    /// Failure (the cool-down's review, 2026-11-13, S4): a writer that panicked while a quit's drain waited on it left the drain waiting out its limit. Its
    /// guard tells the drain, which returns.
    #[test]
    fn a_writer_that_panics_lets_a_drain_return() {
        let line = Leaving::new(0);
        assert!(line.push(facts(1), material(10, 0), Leaves::Dormant, 0));
        assert!(!line.push(facts(2), material(10, 0), Leaves::Dormant, 0));
        let (go, gone) = std::sync::mpsc::channel::<()>();
        let line = &line;
        std::thread::scope(|scope| {
            let writer = scope.spawn(move || {
                line.serve(
                    || 0,
                    |_, _, _| {
                        let _ = gone.recv();
                        panic!("a sleep writer panicking, on purpose");
                    },
                );
            });
            let drained = scope.spawn(move || {
                let began = Instant::now();
                (line.drain(began + Duration::from_secs(5)), began.elapsed())
            });
            until("the drain waiting", || line.report()["drains_waiting"] == 1);
            drop(go);
            assert!(writer.join().is_err());
            let (all, took) = drained.join().expect("the drain returns");
            assert!(!all, "the second is left, with no writer");
            assert!(
                took < Duration::from_millis(2_500),
                "the drain waited out its limit for a writer that panicked: {took:?}"
            );
        });
        assert_eq!(line.report()["writer_panics"], 1);
    }
}
