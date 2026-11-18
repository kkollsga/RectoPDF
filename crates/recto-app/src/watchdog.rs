//! The UI thread's hang watchdog. The host says when its thread starts and stops working, from a run-loop observer, and one thread watches while it's busy.
//!
//! It moved here from `recto-ffi` whole, with its thread, which it starts itself on the first stretch of work; the bridge forwards the host's two calls and `qa state` reads its counts.
//!
//! **At rest this costs nothing:** the watching thread is parked while the UI thread sleeps, so no timer ticks. It wakes only for a stretch of work, and looks every quarter second for as long as that lasts.

mod short;
pub mod startup;
mod waiting;

pub use short::{BUDGET_US, KEPT, Label, Short, SlowStretch};

use recto_platform::thread::{RunState, Sample, ThreadPort, own_cpu_us};
use std::cell::{Cell, RefCell};
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
/// What `doing` in a kept stretch is, said in `qa state` and `qa budget` beside the records so a reader needn't look it up.
const DOING_IS: &str = "doing: the last thing the window's thread said it began in that run-loop pass (a control's kind, or a bridge call: scroll, select_press, select_drag, select_release, clipboard_data), empty if the pass began nothing named. A pass may do several things and only the last is named, so doing is where to look first, not proof it was the long one.";

thread_local! {
    /// The id of the watch this thread is the window's thread of: set at each wake ([`Watchdog::busy`]), which only the window's thread makes. A plain
    /// thread-local of no destructor, so a read is a load from the thread's own block, no system call.
    static WINDOW_OF: Cell<u64> = const { Cell::new(0) };
    /// The watch's record, kept by the window's thread so a feature's own function can name its pass ([`note_doing_here`]) without being handed the window. Set
    /// only when the thread's watch changes (once, in the app), so a wake costs the id compare it already had.
    static WINDOW_WATCH: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
}
/// **The window's thread says what it begins, from inside the feature's own function:** the pass it is in is named for a stall's record, as
/// [`Watchdog::note_doing`] does, without the function being handed the window. A thread that is not a window's thread has no watch of its own here, so a call
/// from the QA channel's or the session's thread does nothing, and the last word of a pass is the one kept.
pub fn note_doing_here(what: &'static str) {
    let _ = WINDOW_WATCH.try_with(|watch| {
        if let Some(shared) = watch.borrow().as_ref() {
            shared.short.note_doing(what);
        }
    });
}

/// Gives each watch an id of its own (never 0), so a thread's [`WINDOW_OF`] can't name another watch's, as a reused address could.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// **The worst stretch seen, and what the UI thread was doing in it** (CI, 2026-11-13: a 2.3 s stall in a script, with no stack, as `sample` didn't end in its bound; waiting on something or not given a core could not be told). `cpu_ms` is the thread's own CPU time over the last `window_ms` of the stretch, from the watcher's first look to its latest, so a stretch spent waiting or starved has `cpu_ms` near zero and one spent working has it near the window. `state` is the kernel's account at the latest look: `waiting` is blocked on something, `running_or_runnable` with CPU not moving is starved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stall {
    /// When the stretch began, as the watchdog counts: tells one stretch from another.
    began: u64,
    pub wall_ms: u64,
    pub cpu_ms: u64,
    pub window_ms: u64,
    /// What most of the watcher's looks at the stretch saw the thread doing: see `waiting_looks`.
    pub state: &'static str,
    /// Of the watcher's looks at this stretch, from its first, how many saw the thread waiting, and how many saw it running or ready to run. One look says
    /// little: a thread that slept two seconds and then waited for a core on a starved machine is `running_or_runnable` at its last look (CI, 2026-11-13).
    pub waiting_looks: u32,
    pub running_looks: u32,
    /// The QA verb that was running when the stretch was first seen past a second, or empty.
    pub verb: String,
    /// Whether the stretch had ended when this was written.
    pub ended: bool,
}

struct Shared {
    id: u64,
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
    /// What the watcher saw of the stretch it is looking at.
    seen: Seen,
    /// The QA verb being waited for, if any. Set by the QA channel; the UI thread never takes this lock.
    verb: Mutex<([u8; VERB_MAX], usize)>,
    worst: Mutex<Option<Stall>>,
    /// The stretches over the 16 ms budget: see [`short`].
    short: Short,
}

/// The watcher's first look at the stretch that began at `began`, and the thread's CPU time and the time at it (what `Stall::cpu_ms` counts from), and its
/// looks at the stretch by what they saw: every look from the first, not only those past a second.
#[derive(Default)]
struct Seen {
    began: AtomicU64,
    cpu_ms: AtomicU64,
    at: AtomicU64,
    waiting: AtomicU32,
    running: AtomicU32,
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
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
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
            seen: Seen::default(),
            verb: Mutex::new(([0; VERB_MAX], 0)),
            worst: Mutex::new(None),
            short: Short::default(),
        }))
    }

    /// From the UI thread, each time it wakes. An atomic, and a wake for the watcher if that's parked.
    pub fn busy(&self) {
        if WINDOW_OF.get() != self.0.id {
            WINDOW_OF.set(self.0.id);
            let _ = WINDOW_WATCH.try_with(|watch| *watch.borrow_mut() = Some(Arc::clone(&self.0)));
        }
        let elapsed = self.0.epoch.elapsed();
        let now = elapsed.as_millis() as u64 + 1;
        self.0.short.begin(elapsed.as_micros() as u64, own_cpu_us());
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
            let elapsed = self.0.epoch.elapsed();
            let now = elapsed.as_millis() as u64 + 1;
            // A stretch over the 16 ms budget is kept with what it was: the verb by a `try_lock`, as the window's thread waits for nothing here.
            self.0.short.end(elapsed.as_micros() as u64, own_cpu_us, || try_verb(&self.0));
            // Counted here too, as it ends: the watcher looks every quarter second, and on a starved machine (CI, 2026-11-11: a 2.3 s stall passed a script) it may not look between 2 s and the end.
            let stalled = now.saturating_sub(began);
            count(&self.0, began, stalled);
            if stalled >= NOTE_MS {
                // This thread's own look at itself, once, for a stretch already past a second: the CPU time it ends with.
                let sample = ThreadPort::from_raw(self.0.port.load(Ordering::Relaxed)).and_then(ThreadPort::sample);
                record(&self.0, began, now, sample, true);
            }
            // Cancel its timed wait while this thread is already awake, rather than leave a timer to wake it after we've slept.
            // Only this thread installs the handle. A diagnostic holding the slot must never make the UI wait for it.
            if let Ok(thread) = self.0.thread.try_lock()
                && let Some(watcher) = &*thread
            {
                watcher.unpark();
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

    #[cfg(test)]
    pub(crate) fn current_verb(&self) -> Label {
        try_verb(&self.0)
    }

    /// The window's thread says what it begins, cheaply (a thread-local read, then one relaxed store of up to 24 bytes), for the stretch's record if it runs
    /// over the 16 ms budget. **Does nothing off the window's thread** (the QA channel's thread runs `act` too, and its word would name the wrong work), and
    /// takes a `&'static str` so no caller can allocate for it. The last word of a stretch is the one kept.
    pub fn note_doing(&self, what: &'static str) {
        if WINDOW_OF.get() == self.0.id {
            self.0.short.note_doing(what);
        }
    }

    /// The worst stretches over the 16 ms budget, the longest first (at most [`KEPT`]), and how many stretches were over it and how many of those couldn't be kept.
    #[must_use]
    pub fn slow_stretches(&self) -> (Vec<SlowStretch>, u64, u64) {
        let (over, lost) = self.0.short.counts();
        (self.0.short.worst(), over, lost)
    }

    /// `qa state`'s `hangs.over_budget`: the stretches over the 16 ms budget, as [`Self::slow_stretches`] has them.
    #[must_use]
    pub fn slow_json(&self) -> serde_json::Value {
        let (kept, over, lost) = self.slow_stretches();
        let worst: Vec<serde_json::Value> = kept
            .iter()
            .map(|stretch| {
                serde_json::json!({
                    "wall_us": stretch.wall_us, "cpu_us": stretch.cpu_us, "at_ms": stretch.at_ms, "kind": stretch.kind(),
                    "doing": stretch.doing.as_str(), "verb": stretch.verb.as_str(),
                })
            })
            .collect();
        serde_json::json!({ "budget_us": BUDGET_US, "kept_max": KEPT, "record_bytes": Short::bytes(), "over": over, "lost": lost, "doing_is": DOING_IS, "worst": worst })
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

/// The QA verb being waited for, without waiting for the lock that holds it: the window's thread calls this.
fn try_verb(shared: &Shared) -> Label {
    match shared.verb.try_lock() {
        Ok(slot) => Label::new(&String::from_utf8_lossy(&slot.0[..slot.1])),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            let slot = poisoned.into_inner();
            Label::new(&String::from_utf8_lossy(&slot.0[..slot.1]))
        }
        Err(std::sync::TryLockError::WouldBlock) => Label::default(),
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
        && shared.seen.began.swap(began, Ordering::AcqRel) != began
    {
        // The first look at this stretch: what CPU time is counted from.
        shared.seen.cpu_ms.store(sample.cpu_ms, Ordering::Relaxed);
        shared.seen.at.store(now_ms, Ordering::Relaxed);
        shared.seen.waiting.store(0, Ordering::Relaxed);
        shared.seen.running.store(0, Ordering::Relaxed);
    }
    if let Some(sample) = sample {
        match sample.state {
            RunState::Waiting | RunState::Uninterruptible => shared.seen.waiting.fetch_add(1, Ordering::Relaxed),
            RunState::Running => shared.seen.running.fetch_add(1, Ordering::Relaxed),
            RunState::Stopped | RunState::Other => 0,
        };
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
    let (first_cpu, first_at) = if shared.seen.began.load(Ordering::Acquire) == began {
        (shared.seen.cpu_ms.load(Ordering::Relaxed), shared.seen.at.load(Ordering::Relaxed))
    } else {
        // Never looked at before: no window to count from.
        (sample.map_or(0, |sample| sample.cpu_ms), now_ms)
    };
    let mut worst = shared.worst.lock().unwrap_or_else(PoisonError::into_inner);
    let before = worst.as_ref().filter(|stall| stall.began == began);
    // The looks counted since the stretch's first: a sleep of a second and a bit is seen waiting by the looks before the second, and the one after may find the thread ready to run.
    let (waiting_looks, running_looks) = if shared.seen.began.load(Ordering::Acquire) == began {
        (shared.seen.waiting.load(Ordering::Relaxed), shared.seen.running.load(Ordering::Relaxed))
    } else {
        (0, 0)
    };
    let state = if waiting_looks > 0 && waiting_looks >= running_looks {
        RunState::Waiting.name()
    } else if running_looks > 0 {
        RunState::Running.name()
    } else {
        before.map_or(RunState::Other.name(), |before| before.state)
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
        waiting_looks,
        running_looks,
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
    let mut waiting = waiting::Waiting::default();
    loop {
        match waiting.next(shared.busy_since.load(Ordering::Acquire) != 0, shared.epoch.elapsed()) {
            waiting::Action::Park => std::thread::park(),
            waiting::Action::Wait(for_time) => std::thread::park_timeout(for_time),
            waiting::Action::Sample => {
                let sample = ThreadPort::from_raw(shared.port.load(Ordering::Relaxed)).and_then(ThreadPort::sample);
                look(shared, shared.epoch.elapsed().as_millis() as u64 + 1, sample);
            }
        }
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
        assert_eq!(
            dog.worst_stall().unwrap().verb,
            "stall",
            "the next verb, in the same stretch, isn't the one that stalled it"
        );
        dog.note_verb(None);
        dog.look_at(2_300);
        assert_eq!(dog.worst_stall().unwrap().verb, "stall");
    }

    /// Failure (CI run 43382166812, 2026-11-13): a 2.3 s sleep read as `running_or_runnable`, as the record kept only its last look, taken after the sleep
    /// had ended and while the thread waited for a core. Every look past a second is counted, and the state is what most of them saw.
    #[test]
    fn a_stretch_is_what_most_looks_saw_not_the_last_one() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(1, Ordering::Release);
        let waiting = sample(10, RunState::Waiting);
        let runnable = sample(10, RunState::Running);
        for now in [1_250, 1_500, 1_750, 2_000, 2_250] {
            dog.look_sampled(now, waiting);
        }
        dog.look_sampled(2_600, runnable);
        let worst = dog.worst_stall().unwrap();
        assert_eq!((worst.waiting_looks, worst.running_looks), (5, 1));
        assert_eq!(worst.state, "waiting", "five looks saw it wait and the last saw it ready to run");
        // A thread that never waited is running, however long.
        let busy = Watchdog::new();
        busy.0.busy_since.store(1, Ordering::Release);
        for now in [1_250, 1_500, 4_000] {
            busy.look_sampled(now, runnable);
        }
        assert_eq!(busy.worst_stall().unwrap().state, "running_or_runnable");
    }

    /// Failure (CI run 43711366303, 2026-11-15): a stretch of 1.2 s was judged from the one look after its second, which found the thread ready to run, as
    /// looks before the second weren't counted. A thread that slept through three looks and then waited for a core must read as having waited, and one
    /// that was ready to run in every look, using a little CPU, as starved.
    #[test]
    fn looks_before_the_second_count_for_what_the_stretch_was() {
        let dog = Watchdog::new();
        dog.0.busy_since.store(1, Ordering::Release);
        for now in [250, 500, 750] {
            dog.look_sampled(now, sample(10, RunState::Waiting));
        }
        assert_eq!(dog.worst_stall(), None, "under a second isn't recorded");
        dog.look_sampled(1_250, sample(11, RunState::Running));
        let slept = dog.worst_stall().unwrap();
        assert_eq!((slept.waiting_looks, slept.running_looks, slept.state), (3, 1, "waiting"));
        let starved = Watchdog::new();
        starved.0.busy_since.store(1, Ordering::Release);
        for now in [250, 500, 750, 1_000, 1_250] {
            starved.look_sampled(now, sample(10, RunState::Running));
        }
        let starved = starved.worst_stall().unwrap();
        assert_eq!((starved.waiting_looks, starved.running_looks), (0, 5));
    }

    /// Waits for `ms` on a blocked thread, as a lock or a call would: no CPU, whatever the machine does. (A park in a loop, as one can end early.)
    fn wait_blocked(ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            std::thread::park_timeout(left);
        }
    }

    /// Works until the thread's own CPU clock has moved `ms`, whatever the machine does with the other cores.
    fn work_cpu(ms: u64) {
        let first = own_cpu_us().unwrap_or(0);
        while own_cpu_us().unwrap_or(u64::MAX) < first + ms * 1000 {
            std::hint::spin_loop();
        }
    }

    /// Failure (the performance review, 2026-11-16: 31 to 69 ms stalls in every run that no record could explain): a stall of the window's thread over the 16 ms
    /// budget is kept without a name, or as the wrong length, or a wait reads as work (or work as a wait), or a QA verb being waited for isn't named. Made
    /// with a real wake and sleep of the watch and a thread that stalls 40 ms in a named call: once blocked, once working.
    #[test]
    fn a_made_stall_of_forty_ms_names_its_call_and_whether_it_waited_or_worked() {
        let dog = Watchdog::new();
        assert!(dog.enter(false));
        dog.note_doing("zoom_in");
        dog.note_verb(Some("zoom"));
        // Another thread of the process works through the wait, as the session's does while the window's waits on a lock: a clock that reads the whole process's
        // CPU time would then call the wait busy, and the thread's own does not. (With no core to spare it works little, and the check is then no stricter.)
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spinner = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("test-spinner".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
                .expect("a thread")
        };
        wait_blocked(40);
        stop.store(true, Ordering::Relaxed);
        spinner.join().expect("the other thread of the process");
        dog.idle();
        assert!(dog.enter(false));
        dog.note_doing("page_down");
        work_cpu(40);
        dog.idle();
        // A frame under the budget between them is not kept.
        assert!(dog.enter(false));
        dog.note_doing("scroll");
        dog.idle();
        let (kept, over, lost) = dog.slow_stretches();
        assert_eq!((kept.len(), over, lost), (2, 2, 0), "{kept:?}");
        let waited = kept.iter().find(|stretch| stretch.doing.as_str() == "zoom_in").expect("named by the call");
        let worked = kept.iter().find(|stretch| stretch.doing.as_str() == "page_down").expect("named by the call");
        assert!(waited.wall_us >= 40_000 && worked.wall_us >= 40_000, "{kept:?}");
        assert_eq!(waited.verb.as_str(), "zoom", "the verb being waited for");
        assert_eq!(worked.verb.as_str(), "zoom", "still set: the channel clears it when its ask returns");
        if Short::cpu_known() {
            // Held by the thread's own CPU clock, which a starved machine doesn't move; the busy/blocked rule itself is `cpu_time_tells_a_wait_from_work`'s.
            assert!(worked.cpu_us.unwrap() >= 40_000, "{worked:?}");
            // A parked thread has almost no CPU however starved the machine is, so this holds at any speed; it fails on a clock that reads the whole process's or
            // another thread's time (or the wall clock) in place of this thread's.
            assert_eq!(waited.kind(), "blocked", "{waited:?}");
        } else {
            assert_eq!(waited.kind(), "unknown", "{waited:?}");
        }
        assert_eq!(dog.report().1, 0, "and what a stall of a second means is as it was");
        assert!(dog.worst_stall().is_none(), "the stall record of a second or more is untouched");
    }

    /// Failure: a word said from another thread (the QA channel's runs `act` too) is kept as the window's pass, naming work the window's thread never did;
    /// or one said on the window's thread is lost.
    #[test]
    fn a_word_is_kept_only_from_the_window_thread() {
        let dog = Watchdog::new();
        assert!(dog.enter(false));
        dog.note_doing("window_work");
        let other = dog.clone();
        std::thread::Builder::new()
            .name("test-qa-channel".into())
            .spawn(move || other.note_doing("qa_save_as"))
            .expect("a thread")
            .join()
            .expect("the other thread");
        wait_blocked(20);
        dog.idle();
        let (kept, _, _) = dog.slow_stretches();
        assert_eq!(kept[0].doing.as_str(), "window_work", "{kept:?}");
    }

    /// Failure: the memory of the record grows (a field added that scales with stalls), against the number the docs and `qa state` state.
    #[test]
    fn the_stall_record_is_fixed_in_size() {
        assert_eq!(
            Short::bytes(),
            1224,
            "the kept list is {KEPT} slots of {} bytes",
            std::mem::size_of::<SlowStretch>()
        );
    }

    /// What a wake and a sleep of the window's thread cost, in ns, the least of 7 rounds of 200,000 pairs (release build: `cargo test --release -p recto-app
    /// -- --ignored wake_and_sleep_cost --nocapture`). Not a gate: a number for the hand-back, before and after a change to what a wake does.
    #[test]
    #[ignore = "a measurement, run by hand on a release build"]
    fn wake_and_sleep_cost() {
        let dog = Watchdog::new();
        let mut least = f64::MAX;
        for _ in 0..7 {
            let began = Instant::now();
            for _ in 0..200_000 {
                assert!(dog.enter(false));
                dog.idle();
            }
            least = least.min(began.elapsed().as_nanos() as f64 / 200_000.0);
        }
        println!("wake and sleep: {least:.0} ns a pair");
        // The same with a word said inside (the window's thread: the thread-local read and the stores), and one said off the window's thread's watch (the read alone).
        let (mut said, mut ignored) = (f64::MAX, f64::MAX);
        let other = Watchdog::new();
        for _ in 0..7 {
            let began = Instant::now();
            for _ in 0..200_000 {
                assert!(dog.enter(false));
                dog.note_doing("select_press");
                dog.idle();
            }
            said = said.min(began.elapsed().as_nanos() as f64 / 200_000.0);
            let began = Instant::now();
            for _ in 0..200_000 {
                assert!(dog.enter(false));
                other.note_doing("select_press");
                dog.idle();
            }
            ignored = ignored.min(began.elapsed().as_nanos() as f64 / 200_000.0);
        }
        println!("wake, a word, sleep: {said:.0} ns a pair; a word to another watch: {ignored:.0} ns a pair");
    }
}
