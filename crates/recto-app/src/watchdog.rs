//! The UI thread's hang watchdog. The host says when its thread starts and stops working, from a run-loop observer, and one thread watches while it's busy.
//!
//! It moved here from `recto-ffi` whole, with its thread, which it starts itself on the first stretch of work; the bridge forwards the host's two calls and `qa state` reads its counts.
//!
//! **At rest this costs nothing:** the watching thread is parked while the UI thread sleeps, so no timer ticks. It wakes only for a stretch of work, and looks every quarter second for as long as that lasts.

use recto_platform::thread::{RunState, Sample, ThreadPort};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// A stall this long is noted.
pub const NOTE_MS: u64 = 1000;
/// A stall this long fails a QA run. AGENTS.md: "a stall of 2 s fails the QA run".
pub const FAIL_MS: u64 = 2000;
const LOOK_EVERY: Duration = Duration::from_millis(250);
/// The longest QA verb kept, longer ones cut: a fixed slot, nothing allocated for each one.
const VERB_MAX: usize = 24;

/// **The worst stretch seen, and what the UI thread was doing in it** (CI, 2026-11-13: a 2.3 s stall in a script, with no stack, as `sample` didn't end in its bound; waiting on something or not given a core could not be told). `cpu_ms` is the thread's own CPU time over the last `window_ms` of the stretch, from the watcher's first look to its latest, so a stretch spent waiting or starved has `cpu_ms` near zero and one spent working has it near the window. `state` is the kernel's account at the latest look: `waiting` is blocked on something, `running_or_runnable` with CPU not moving is starved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stall {
    /// When the stretch began, as the watchdog counts: tells one stretch from another.
    began: u64,
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub window_ms: u64,
    pub state: &'static str,
    /// The QA verb that was running when the stretch was first seen past a second, or empty.
    pub verb: String,
    /// Whether the stretch had ended when this was written.
    pub ended: bool,
}

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
    /// The UI thread's Mach port, taken on its first `busy`, or zero.
    port: AtomicU32,
    /// The watcher's first look at the stretch that began then, and the thread's CPU time and the time at it: what `Stall::cpu_ms` counts from.
    first_began: AtomicU64,
    first_cpu_ms: AtomicU64,
    first_at: AtomicU64,
    /// The QA verb being waited for, if any. Set by the QA channel; the UI thread never takes this lock.
    verb: Mutex<([u8; VERB_MAX], usize)>,
    worst: Mutex<Option<Stall>>,
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
            port: AtomicU32::new(0),
            first_began: AtomicU64::new(0),
            first_cpu_ms: AtomicU64::new(0),
            first_at: AtomicU64::new(0),
            verb: Mutex::new(([0; VERB_MAX], 0)),
            worst: Mutex::new(None),
        }))
    }

    /// From the UI thread, each time it wakes. An atomic, and a wake for the watcher if that's parked.
    pub fn busy(&self) {
        let now = self.0.epoch.elapsed().as_millis() as u64 + 1;
        self.0.busy_since.store(now, Ordering::Release);
        if self.0.port.load(Ordering::Relaxed) == 0 {
            self.0.port.store(ThreadPort::current().raw(), Ordering::Relaxed);
        }
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
            let stalled = now.saturating_sub(began);
            count(&self.0, began, stalled);
            if stalled >= NOTE_MS {
                // This thread's own look at itself, once, for a stretch already past a second: the CPU time it ends with.
                let sample = ThreadPort::from_raw(self.0.port.load(Ordering::Relaxed)).and_then(ThreadPort::sample);
                record(&self.0, began, now, sample, true);
            }
        }
    }

    /// The QA verb being waited for now, or none. From the QA channel's thread, before it asks the UI thread and after it answers.
    pub fn note_verb(&self, verb: Option<&str>) {
        let mut slot = self.0.verb.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes = verb.map_or(&[][..], str::as_bytes);
        let mut len = bytes.len().min(VERB_MAX);
        while len > 0 && !verb.unwrap_or("").is_char_boundary(len) {
            len -= 1;
        }
        slot.0[..len].copy_from_slice(&bytes[..len]);
        slot.1 = len;
    }

    /// The worst stretch over a second seen, and what the thread did in it.
    #[must_use]
    pub fn worst_stall(&self) -> Option<Stall> {
        self.0.worst.lock().unwrap_or_else(PoisonError::into_inner).clone()
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

    /// One look, for a test, with no sample of the thread.
    #[cfg(test)]
    fn look_at(&self, now_ms: u64) {
        look(&self.0, now_ms, None);
    }

    /// One look, for a test, with the sample the thread would have given.
    #[cfg(test)]
    fn look_sampled(&self, now_ms: u64, sample: Sample) {
        look(&self.0, now_ms, Some(sample));
    }
}

/// The watcher's look at the stretch under way.
fn look(shared: &Shared, now_ms: u64, sample: Option<Sample>) {
    let began = shared.busy_since.load(Ordering::Acquire);
    if began == 0 {
        return;
    }
    let stalled = now_ms.saturating_sub(began);
    count(shared, began, stalled);
    if let Some(sample) = sample
        && shared.first_began.swap(began, Ordering::AcqRel) != began
    {
        // The first look at this stretch: what CPU time is counted from.
        shared.first_cpu_ms.store(sample.cpu_ms, Ordering::Relaxed);
        shared.first_at.store(now_ms, Ordering::Relaxed);
    }
    if stalled >= NOTE_MS {
        record(shared, began, now_ms, sample, false);
    }
}

/// The stretch over a second, as the latest look or its end saw it: the worst one is kept. At its end the state is the look's, as the thread seeing itself is always running, and the verb is the look's, as the channel clears it on the answer.
fn record(shared: &Shared, began: u64, now_ms: u64, sample: Option<Sample>, ended: bool) {
    let verb = {
        let slot = shared.verb.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&slot.0[..slot.1]).into_owned()
    };
    let (first_cpu, first_at) = if shared.first_began.load(Ordering::Acquire) == began {
        (shared.first_cpu_ms.load(Ordering::Relaxed), shared.first_at.load(Ordering::Relaxed))
    } else {
        // Never looked at before: no window to count from.
        (sample.map_or(0, |sample| sample.cpu_ms), now_ms)
    };
    let mut worst = shared.worst.lock().unwrap_or_else(PoisonError::into_inner);
    let before = worst.as_ref().filter(|stall| stall.began == began);
    let state = match (ended, before) {
        (true, Some(before)) => before.state,
        _ => sample.map_or(RunState::Other.name(), |sample| sample.state.name()),
    };
    // The verb the stretch passed a second in is the one that stalled it, and it's kept: a stretch can run on into the next verb without the thread sleeping
    // between them (a 2.3 s stall read as `state`'s, 2026-11-13, at load 12), and the channel clears the slot as each answers.
    let verb = match before {
        Some(before) if !before.verb.is_empty() || verb.is_empty() => before.verb.clone(),
        _ => verb,
    };
    let stall = Stall {
        began,
        wall_ms: now_ms.saturating_sub(began),
        cpu_ms: sample.map_or(0, |sample| sample.cpu_ms.saturating_sub(first_cpu)),
        window_ms: now_ms.saturating_sub(first_at),
        state,
        verb,
        ended,
    };
    if worst.as_ref().is_none_or(|worst| worst.began == began || stall.wall_ms >= worst.wall_ms) {
        *worst = Some(stall);
    }
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
        let sample = ThreadPort::from_raw(shared.port.load(Ordering::Relaxed)).and_then(ThreadPort::sample);
        look(shared, shared.epoch.elapsed().as_millis() as u64 + 1, sample);
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
    fn sample(cpu_ms: u64, state: RunState) -> Sample {
        Sample { cpu_ms, state }
    }

    /// Failure: a stall can't be told as waiting from starved, as its record has the CPU time over the whole run and not the stretch, or the state of another look.
    #[test]
    fn a_stall_records_the_cpu_it_used_and_its_state() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(100, Ordering::Release);
        dog.look_sampled(350, sample(5_000, RunState::Running));
        assert_eq!(dog.worst_stall(), None, "under a second isn't recorded");
        dog.note_verb(Some("menu"));
        dog.look_sampled(1_200, sample(5_010, RunState::Waiting));
        let first = dog.worst_stall().unwrap();
        assert_eq!(
            (first.wall_ms, first.cpu_ms, first.window_ms, first.state, first.verb.as_str(), first.ended),
            (1_100, 10, 850, "waiting", "menu", false)
        );
        dog.look_sampled(2_450, sample(5_012, RunState::Waiting));
        let second = dog.worst_stall().unwrap();
        assert_eq!(
            (second.wall_ms, second.cpu_ms, second.window_ms),
            (2_350, 12, 2_100),
            "the latest look of the same stretch replaces it"
        );
    }

    /// Failure: the end of a stretch writes the state of the UI thread looking at itself (always running), or loses the verb the channel has cleared by then.
    #[test]
    fn a_stalls_end_keeps_the_look_that_saw_it_wait() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(100, Ordering::Release);
        dog.look_sampled(300, sample(0, RunState::Running));
        dog.note_verb(Some("copied"));
        dog.look_sampled(1_300, sample(2, RunState::Waiting));
        dog.note_verb(None);
        record(&dog.0, 100, 2_400, Some(sample(30, RunState::Running)), true);
        let end = dog.worst_stall().unwrap();
        assert_eq!(
            (end.state, end.verb.as_str(), end.ended, end.wall_ms, end.cpu_ms),
            ("waiting", "copied", true, 2_300, 30)
        );
    }

    /// Failure: a shorter stretch later hides the worst, or a long verb name overruns its slot.
    #[test]
    fn the_worst_stretch_is_kept_and_a_long_verb_is_cut() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(100, Ordering::Release);
        dog.look_sampled(3_100, sample(1, RunState::Waiting));
        dog.0.busy_since.store(10_000, Ordering::Release);
        dog.look_sampled(11_100, sample(2, RunState::Waiting));
        assert_eq!(dog.worst_stall().unwrap().wall_ms, 3_000);
        dog.note_verb(Some("a_verb_name_longer_than_the_slot_has_room_for"));
        dog.0.busy_since.store(20_000, Ordering::Release);
        dog.look_sampled(25_000, sample(3, RunState::Running));
        assert_eq!(dog.worst_stall().unwrap().verb, "a_verb_name_longer_than_");
        let kept = dog.worst_stall().unwrap().verb.len();
        assert!(kept <= VERB_MAX);
    }

    /// Failure (main's full check, 2026-11-13, load 12): a 2.3 s stall in `qa stall` was written down as `state`'s, as the UI thread went from one verb
    /// straight to the next without sleeping, so both were one stretch and the later look saw the later verb. The verb a stretch passed a second in stays.
    #[test]
    fn a_stretch_that_runs_on_into_the_next_verb_keeps_the_verb_that_stalled_it() {
        let dog = Watchdog::new();
        let began = 1;
        dog.0.busy_since.store(began, Ordering::Release);
        dog.note_verb(Some("stall"));
        dog.look_at(1_500);
        assert_eq!(dog.worst_stall().unwrap().verb, "stall");
        dog.note_verb(None);
        dog.note_verb(Some("state"));
        dog.look_at(2_250);
        assert_eq!(dog.worst_stall().unwrap().verb, "stall", "the next verb, in the same stretch, isn't the one that stalled it");
        dog.note_verb(None);
        dog.look_at(2_300);
        assert_eq!(dog.worst_stall().unwrap().verb, "stall");
    }
}
