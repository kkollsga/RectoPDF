//! **The stretches of the window's thread over its 16 ms budget, and what each was** (SPEC §2: "the window's thread never stalls over 16 ms"; the performance review of 2026-11-16 measured 31 to 69 ms in every run and could not say what the thread was doing, as the watch only described a stall of a second).
//!
//! A *stretch* is one wake of the window's thread to its next sleep, the run loop's own bracket (or a block of the host's that the run loop didn't bracket). The thread notes the start and, when the stretch ends, keeps it if it was over [`BUDGET_US`]: the worst [`KEPT`] only, in a fixed array, so a ninth replaces the smallest and nothing grows.
//!
//! **What it costs the window's thread.** At a wake: the clock, the thread's own CPU time (one system call) and three relaxed stores. At a sleep: the clock, and for a stretch over the budget only, a second CPU reading and a `try_lock` of the kept list (never waits: a stretch that finds it held is counted in `lost`). No allocation, no thread of its own, no timer; at rest nothing runs, as the wake and sleep calls are the run loop's. The memory is [`Short`]'s size, fixed (`size_of`, in the tests and in `qa state`).
//!
//! **What it knows.** How long (µs), when (ms from the watch's start), the thread's own CPU time over it, so *busy* (CPU near the wall time) is told from *blocked* (CPU near nothing: waiting on a lock or a call, or not given a core; a short stretch is over before the watcher's quarter-second look, so these two aren't told apart here, as they are for a stretch of a second), the QA verb that was being waited for, and the last thing the window's thread said it began in the stretch ([`Short::note_doing`]: one relaxed store of up to 24 bytes, three words, that the caller makes only where it is cheap to). What it doesn't know: which of several things done in one stretch was the long one.

use recto_platform::thread::own_cpu_us;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError, TryLockError};

/// A stretch this long is over the window's budget (SPEC §2).
pub const BUDGET_US: u64 = 16_000;
/// How many stretches are kept: the worst.
pub const KEPT: usize = 8;
/// The longest label kept, longer ones cut at a character.
pub const LABEL_MAX: usize = 24;
/// Not a CPU time: the platform couldn't say.
const NO_CPU: u64 = u64::MAX;

/// A short name in a fixed slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Label {
    bytes: [u8; LABEL_MAX],
    len: u8,
}

impl Label {
    /// The name, cut to the slot at a character boundary.
    #[must_use]
    pub fn new(name: &str) -> Self {
        let mut len = name.len().min(LABEL_MAX);
        while !name.is_char_boundary(len) {
            len -= 1;
        }
        let mut bytes = [0; LABEL_MAX];
        bytes[..len].copy_from_slice(&name.as_bytes()[..len]);
        Self { bytes, len: len as u8 }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

/// One stretch over the budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlowStretch {
    pub wall_us: u64,
    /// The thread's own CPU time over the stretch, if the platform says.
    pub cpu_us: Option<u64>,
    /// When it began, in ms from the watch's start.
    pub at_ms: u64,
    /// The last thing the thread said it began in the stretch, or empty.
    pub doing: Label,
    /// The QA verb being waited for when it ended, or empty.
    pub verb: Label,
}

impl SlowStretch {
    /// What the CPU time says it was: `busy` (CPU is most of the wall time), `blocked` (waiting or not given a core: little CPU), `mixed`, or `unknown`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self.cpu_us {
            None => "unknown",
            Some(cpu) if cpu.saturating_mul(10) >= self.wall_us.saturating_mul(8) => "busy",
            Some(cpu) if cpu.saturating_mul(10) < self.wall_us.saturating_mul(2) => "blocked",
            Some(_) => "mixed",
        }
    }
}

struct Kept {
    list: [SlowStretch; KEPT],
    len: usize,
}

/// What the window's thread notes, and what it keeps.
pub struct Short {
    began_us: AtomicU64,
    began_cpu_us: AtomicU64,
    /// The label said at [`Self::note_doing`], as three words; valid for `doing_len` bytes, which is zeroed at each wake.
    doing: [AtomicU64; 3],
    doing_len: AtomicU32,
    /// Stretches over the budget, kept or not.
    over: AtomicU64,
    /// Stretches over the budget that couldn't be kept, as a reader held the list at that moment.
    lost: AtomicU64,
    kept: Mutex<Kept>,
}

impl Default for Short {
    fn default() -> Self {
        Self {
            began_us: AtomicU64::new(0),
            began_cpu_us: AtomicU64::new(NO_CPU),
            doing: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            doing_len: AtomicU32::new(0),
            over: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            kept: Mutex::new(Kept {
                list: [SlowStretch::default(); KEPT],
                len: 0,
            }),
        }
    }
}

impl Short {
    /// The window's thread wakes. `cpu_us` is its own CPU time now.
    pub fn begin(&self, now_us: u64, cpu_us: Option<u64>) {
        self.began_us.store(now_us, Ordering::Relaxed);
        self.began_cpu_us.store(cpu_us.unwrap_or(NO_CPU), Ordering::Relaxed);
        self.doing_len.store(0, Ordering::Relaxed);
    }

    /// The window's thread says what it begins: one relaxed store of up to [`LABEL_MAX`] bytes in three words. Only the window's thread calls it, and reads it at the stretch's end.
    pub fn note_doing(&self, what: &str) {
        let label = Label::new(what);
        for (index, word) in self.doing.iter().enumerate() {
            let mut eight = [0u8; 8];
            eight.copy_from_slice(&label.bytes[index * 8..index * 8 + 8]);
            word.store(u64::from_le_bytes(eight), Ordering::Relaxed);
        }
        self.doing_len.store(u32::from(label.len), Ordering::Relaxed);
    }

    fn doing_label(&self) -> Label {
        let len = (self.doing_len.load(Ordering::Relaxed) as usize).min(LABEL_MAX);
        let mut bytes = [0u8; LABEL_MAX];
        for (index, word) in self.doing.iter().enumerate() {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&word.load(Ordering::Relaxed).to_le_bytes());
        }
        Label { bytes, len: len as u8 }
    }

    /// The window's thread is about to sleep. `cpu_now` and `verb` are read only for a stretch over the budget.
    pub fn end(&self, now_us: u64, cpu_now: impl FnOnce() -> Option<u64>, verb: impl FnOnce() -> Label) {
        let began = self.began_us.load(Ordering::Relaxed);
        let wall_us = now_us.saturating_sub(began);
        if wall_us < BUDGET_US {
            return;
        }
        self.over.fetch_add(1, Ordering::Relaxed);
        let first = self.began_cpu_us.load(Ordering::Relaxed);
        let cpu_us = match (first, cpu_now()) {
            (NO_CPU, _) | (_, None) => None,
            (first, Some(now)) => Some(now.saturating_sub(first)),
        };
        let stretch = SlowStretch {
            wall_us,
            cpu_us,
            at_ms: began / 1000,
            doing: self.doing_label(),
            verb: verb(),
        };
        let mut kept = match self.kept.try_lock() {
            Ok(kept) => kept,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.lost.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        let len = kept.len;
        if len < KEPT {
            kept.list[len] = stretch;
            kept.len += 1;
        } else if let Some(smallest) = (0..KEPT).min_by_key(|&index| kept.list[index].wall_us)
            && kept.list[smallest].wall_us < stretch.wall_us
        {
            kept.list[smallest] = stretch;
        }
    }

    /// The kept stretches, the longest first.
    #[must_use]
    pub fn worst(&self) -> Vec<SlowStretch> {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let mut list = kept.list[..kept.len].to_vec();
        list.sort_by_key(|stretch| std::cmp::Reverse(stretch.wall_us));
        list
    }

    /// Stretches over the budget seen, and how many of them couldn't be kept.
    #[must_use]
    pub fn counts(&self) -> (u64, u64) {
        (self.over.load(Ordering::Relaxed), self.lost.load(Ordering::Relaxed))
    }

    /// The memory this holds, in bytes: fixed.
    #[must_use]
    pub const fn bytes() -> usize {
        std::mem::size_of::<Self>()
    }

    /// Whether a CPU reading works here (for a test's expectations).
    #[must_use]
    pub fn cpu_known() -> bool {
        own_cpu_us().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stretch(short: &Short, at_us: u64, wall_us: u64, cpu_us: u64) {
        short.begin(at_us, Some(1_000));
        short.end(at_us + wall_us, || Some(1_000 + cpu_us), Label::default);
    }

    /// Failure: a stretch under the 16 ms budget is kept (the list fills with ordinary frames and the worst are lost), or one at it is dropped.
    #[test]
    fn a_stretch_under_the_budget_isnt_kept_and_one_at_it_is() {
        let short = Short::default();
        stretch(&short, 0, BUDGET_US - 1, 0);
        assert!(short.worst().is_empty());
        assert_eq!(short.counts(), (0, 0));
        stretch(&short, 100_000, BUDGET_US, 0);
        assert_eq!(short.worst().len(), 1);
    }

    /// Failure: the list grows with each stall (memory per stall), or a ninth stall is dropped though it is worse than one kept, or replaces a worse one.
    #[test]
    fn a_ninth_stall_replaces_the_smallest_and_nothing_grows() {
        let short = Short::default();
        let before = Short::bytes();
        for index in 0..KEPT as u64 {
            stretch(&short, index * 1_000_000, 20_000 + index * 1_000, 0);
        }
        assert_eq!(short.worst().len(), KEPT);
        stretch(&short, 9_000_000, 19_000, 0);
        assert_eq!(
            short.worst().iter().map(|s| s.wall_us).min(),
            Some(20_000),
            "smaller than every kept one: not kept"
        );
        stretch(&short, 10_000_000, 50_000, 0);
        let walls: Vec<u64> = short.worst().iter().map(|s| s.wall_us).collect();
        assert_eq!(walls.len(), KEPT);
        assert_eq!(walls[0], 50_000, "the longest first");
        assert!(!walls.contains(&20_000), "the smallest went");
        assert_eq!(*walls.last().unwrap(), 21_000);
        assert_eq!(short.counts(), (10, 0), "every one over the budget counted, kept or not");
        assert_eq!(Short::bytes(), before, "fixed");
    }

    /// Failure: waiting and working are told as the same (the CPU time is the whole run's and not the stretch's, or the kind ignores it).
    #[test]
    fn cpu_time_tells_a_wait_from_work() {
        let short = Short::default();
        stretch(&short, 0, 40_000, 39_000);
        stretch(&short, 1_000_000, 41_000, 300);
        stretch(&short, 2_000_000, 42_000, 21_000);
        let by_wall = short.worst();
        assert_eq!(by_wall[0].kind(), "mixed");
        assert_eq!((by_wall[1].kind(), by_wall[1].cpu_us), ("blocked", Some(300)));
        assert_eq!((by_wall[2].kind(), by_wall[2].cpu_us), ("busy", Some(39_000)));
        let unknown = SlowStretch {
            wall_us: 40_000,
            ..SlowStretch::default()
        };
        assert_eq!(unknown.kind(), "unknown");
    }

    /// Failure: what the thread said it began is lost, kept from an earlier stretch, or runs over its slot.
    #[test]
    fn the_stretch_names_what_the_thread_began_in_it() {
        let short = Short::default();
        short.begin(0, None);
        short.note_doing("zoom_in");
        short.end(30_000, || None, || Label::new("verb_longer_than_the_slot_has_room_for"));
        let first = short.worst()[0];
        assert_eq!(
            (first.doing.as_str(), first.verb.as_str(), first.cpu_us),
            ("zoom_in", "verb_longer_than_the_slo", None)
        );
        short.begin(100_000, None);
        short.end(150_000, || None, Label::default);
        assert_eq!(short.worst()[0].doing.as_str(), "", "a new stretch starts with nothing said");
        assert_eq!(Label::new("é").as_str(), "é");
        assert_eq!(Label::new(&"é".repeat(20)).as_str().chars().count(), 12, "cut at a character, never inside one");
    }

    /// Failure: a reader holding the list makes the window's thread wait for it.
    #[test]
    fn a_held_list_is_never_waited_for() {
        let short = Short::default();
        let guard = short.kept.lock().unwrap();
        stretch(&short, 0, 30_000, 0);
        assert_eq!(short.counts(), (1, 1), "counted, and lost, without waiting");
        drop(guard);
        assert!(short.worst().is_empty());
    }
}
