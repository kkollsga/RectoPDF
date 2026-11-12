//! **The sleeper's decisions:** when the next look at dormancy is due, whether a tab that's only moving about may have a new sleep file now (a gap per tab and a cap per hour,
//! the owner's rule on SSD wear), when the budget is looked at again, which tabs go dormant over budget, and how long the queue's writer holds back for a write someone's
//! waiting on. Each is a plain function or a small type over times in milliseconds of the app's [`crate::clock::Clock`] (plus one, so zero means none): no lock, no thread, no
//! callback, so they're tested on a fake clock.
//!
//! `recto-ffi`'s `Sleeper` keeps these in its own short locks and atomics, as before, and its `impl Inner` does what they say: the page views, the session, the writer thread
//! and the real waits (the try-lock loop for the sleep files' lock, which waits on the real clock) stay there. **The intent** (restructuring step R1.4): the sleep writer becomes
//! a state machine, `handle(input, now) -> intents`, and calls these unchanged.

use recto_session::TabId;
use std::collections::HashMap;

/// A tab goes dormant once it hasn't been active or edited for this long. SPEC decision 25's five minutes, its *default*.
pub const DORMANT_AFTER_SECONDS: u32 = 5 * 60;
/// What the documents off screen may hold in all, their workers' memory and their histories', before the oldest goes dormant. SPEC section 2: "a byte budget frees the oldest first". What's on screen is another row.
pub const LIVE_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
/// The least time between sleep files for a tab that's only moving about (the owner's rule on SSD wear). A quit and a hand-off don't wait, and neither does being asked. A tab left sooner has its older file, whose facts are still good, and whose pictures show only on the pages they're of.
pub const SLEEP_WRITE_EVERY_SECONDS: u32 = 60;
/// What sleep files may come to in an hour, all told, for the same reason: this is about 8 MiB on a heavy day's eight hours, inside SPEC section 2's "About 10 MiB of state". A file that's text is about 8 KiB, and one of a photograph about 100.
pub const SLEEP_BYTES_PER_HOUR: u64 = 1024 * 1024;
/// How often the budget's looked at, at the most, as each look is a call to the kernel per worker.
pub const BUDGET_LOOK_EVERY_MS: u64 = 1_000;
/// The most the queue's writer holds back for a write someone's waiting on, each time.
pub const OTHERS_FIRST_FOR_MS: u64 = 1_000;
const HOUR_MS: u64 = 3_600_000;

/// When the next look at dormancy is due, for a wait of `seconds` from `now`, or zero with nothing to wait for.
#[must_use]
pub fn due_at(now: u64, seconds: Option<u32>) -> u64 {
    seconds.map_or(0, |seconds| now + u64::from(seconds) * 1000)
}

/// How long until `due`, in milliseconds, or `None` when it's zero (nothing due).
#[must_use]
pub fn wait_for(due: u64, now: u64) -> Option<u64> {
    (due != 0).then(|| due.saturating_sub(now))
}

/// When each tab's sleep file was last written, for those written within the gap. Next to no bytes per tab.
#[derive(Debug, Default)]
pub struct Written {
    at: HashMap<TabId, u64>,
}

impl Written {
    /// This tab's file was written less than `gap_ms` ago.
    #[must_use]
    pub fn soon(&self, id: TabId, gap_ms: u64, now: u64) -> bool {
        self.at.get(&id).is_some_and(|at| now.saturating_sub(*at) < gap_ms)
    }

    /// This tab's file was written now. Only a write within the gap holds the next one back, so older ones go: without this every tab ever written stayed, closed ones too.
    pub fn note(&mut self, id: TabId, gap_ms: u64, now: u64) {
        self.at.retain(|_, at| now.saturating_sub(*at) < gap_ms);
        self.at.insert(id, now);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.at.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
}

/// The hour under way, and what sleep files have come to in it.
#[derive(Debug)]
pub struct Hour {
    began: u64,
    bytes: u64,
}

impl Hour {
    #[must_use]
    pub fn starting(now: u64) -> Self {
        Self { began: now, bytes: 0 }
    }

    /// Whether this hour has room for another file, starting a new hour first if this one's over.
    pub fn has_room(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.began) >= HOUR_MS {
            *self = Self::starting(now);
        }
        self.bytes < SLEEP_BYTES_PER_HOUR
    }

    pub fn add(&mut self, bytes: u64) {
        self.bytes += bytes;
    }
}

/// When the budget was last looked at, and which tab was showing then.
#[derive(Debug, Default)]
pub struct BudgetLook {
    last: Option<(u64, Option<TabId>)>,
}

impl BudgetLook {
    /// Whether to look at the budget now, noting it if so: at most every [`BUDGET_LOOK_EVERY_MS`], and straight away on a change of tab, which is when a document comes to be off screen.
    pub fn now(&mut self, active: Option<TabId>, now: u64) -> bool {
        if self
            .last
            .is_some_and(|(when, was)| was == active && now.saturating_sub(when) < BUDGET_LOOK_EVERY_MS)
        {
            return false;
        }
        self.last = Some((now, active));
        true
    }
}

/// Which tabs go dormant to bring `costs` (oldest first, each tab's bytes) within `budget`: the oldest, until what's left fits.
#[must_use]
pub fn over_budget(costs: &[(TabId, u64)], budget: u64) -> Vec<TabId> {
    let mut total: u64 = costs.iter().map(|(_, cost)| cost).sum();
    let mut over = Vec::new();
    for (id, cost) in costs {
        if total <= budget {
            break;
        }
        over.push(*id);
        total -= cost;
    }
    over
}

/// Whether the queue's writer still holds back, `waited_ms` into holding back, for writes someone's waiting on. A flush looks for the lock only now and then, and a lock isn't
/// fair, so the writer, going from one file to the next, would otherwise have it again first every time. Bounded, as a flush has a limit of its own.
#[must_use]
pub fn hold_back_for_others(others_waiting: u32, waited_ms: u64) -> bool {
    others_waiting > 0 && waited_ms < OTHERS_FIRST_FOR_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use crate::testkit::FakeClock;

    /// Failure: when each tab's sleep file was last written is kept for every tab that ever had one, closed tabs included, so a session that opens and closes tabs all day grows without a cap (the 24-hour soak of 2026-11-08 went through 3,068 tabs). Only those written within the gap are kept, and the gap still holds a tab back. (It slept 1.1 s in recto-ffi; here the clock moves.)
    #[test]
    fn last_written_keeps_only_writes_within_the_gap() {
        let clock = FakeClock::at(0);
        let gap = 1_000;
        let mut written = Written::default();
        for id in 0..500 {
            written.note(TabId(id), gap, clock.now_ms() + 1);
        }
        assert!(written.soon(TabId(499), gap, clock.now_ms() + 1), "a tab just written waits for the gap");
        clock.advance(1_100);
        written.note(TabId(1_000), gap, clock.now_ms() + 1);
        assert_eq!(written.len(), 1, "those older than the gap have gone");
        assert!(!written.soon(TabId(499), gap, clock.now_ms() + 1), "a tab past the gap may write again");
    }

    /// Failure (the owner's SSD rule): sleep files keep being written past the hour's cap, or the cap never lifts, so after one busy hour no tab moving about ever gets a new file again. The cap holds within the hour and a new hour starts empty. The app tests that reach the gap and the cap today set the gap through QA in `crates/recto-cli/tests/long_session.rs` and `restore.rs`.
    #[test]
    fn the_hourly_cap_holds_and_lifts_with_the_hour() {
        let clock = FakeClock::at(0);
        let mut hour = Hour::starting(clock.now_ms() + 1);
        assert!(hour.has_room(clock.now_ms() + 1));
        hour.add(SLEEP_BYTES_PER_HOUR);
        clock.advance(HOUR_MS - 1);
        assert!(!hour.has_room(clock.now_ms() + 1), "full until the hour's up");
        clock.advance(1);
        assert!(hour.has_room(clock.now_ms() + 1), "a new hour");
    }

    /// Failure: the budget is looked at on every change (a call to the kernel per worker each time), or a change of tab, when a document comes to be off screen, waits out the second.
    #[test]
    fn the_budget_is_looked_at_each_second_or_at_a_change_of_tab() {
        let clock = FakeClock::at(5_000);
        let mut look = BudgetLook::default();
        assert!(look.now(Some(TabId(1)), clock.now_ms()), "the first look");
        clock.advance(500);
        assert!(!look.now(Some(TabId(1)), clock.now_ms()), "within the second");
        assert!(look.now(Some(TabId(2)), clock.now_ms()), "another tab showing");
        clock.advance(BUDGET_LOOK_EVERY_MS);
        assert!(look.now(Some(TabId(2)), clock.now_ms()), "the second's up");
    }

    /// Failure (SPEC section 2: "a byte budget frees the oldest first"): over budget, the newest goes dormant, or more go than it takes, or one goes within budget. The app test that reaches it today: `restore.rs`'s `pressure_and_the_budget_do_what_being_left_alone_does`.
    #[test]
    fn over_budget_the_oldest_go_until_the_rest_fit() {
        let costs = [(TabId(1), 50), (TabId(2), 30), (TabId(3), 40)];
        assert_eq!(over_budget(&costs, 120), Vec::<TabId>::new());
        assert_eq!(over_budget(&costs, 70), vec![TabId(1)]);
        assert_eq!(over_budget(&costs, 40), vec![TabId(1), TabId(2)]);
        assert_eq!(over_budget(&costs, 39), vec![TabId(1), TabId(2), TabId(3)]);
        assert_eq!(over_budget(&costs, 0), vec![TabId(1), TabId(2), TabId(3)]);
    }

    /// Failure: the queue's writer takes the sleep files' lock again before a flush waiting for it, every time, so a quit's flush runs out of its time; or it holds back for good when nobody's waiting, or past its second.
    #[test]
    fn the_writer_holds_back_only_for_someone_waiting_and_only_a_while() {
        assert!(hold_back_for_others(1, 0));
        assert!(!hold_back_for_others(0, 0), "nobody waiting");
        assert!(!hold_back_for_others(2, OTHERS_FIRST_FOR_MS), "bounded");
    }

    /// Failure: nothing due still gives the session thread a deadline, so it wakes at rest; or a wait comes out longer than asked.
    #[test]
    fn nothing_due_is_no_wait() {
        assert_eq!(due_at(1_000, None), 0);
        assert_eq!(wait_for(0, 1_000), None);
        let due = due_at(1_000, Some(300));
        assert_eq!(wait_for(due, 1_000), Some(300_000));
        assert_eq!(wait_for(due, due + 5), Some(0), "late is now");
    }
}
