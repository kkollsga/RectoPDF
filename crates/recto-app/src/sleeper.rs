//! **The sleeper's decisions:** when the next look at dormancy is due, whether a tab that's only moving about may have a new sleep file now (a gap per tab and a cap per hour,
//! the owner's rule on SSD wear), when the budget is looked at again, which tabs go dormant over budget, and how long the queue's writer holds back for a write someone's
//! waiting on. Each is a plain function or a small type over times in milliseconds of the app's [`crate::clock::Clock`] (plus one, so zero means none): no lock, no thread, no
//! callback, so they're tested on a fake clock.
//!
//! The sleep writer's machine, [`crate::sleep_writer::SleepWriter`], calls these unchanged and is tested over every order of its events; the tests here are
//! of the rules that machine doesn't exercise at their edges.

use recto_session::TabId;
use std::collections::HashMap;

/// Why tabs are going dormant, for QA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    LeftAlone,
    Budget,
    Pressure,
    /// `session sleep`, by whoever sent it: an agent's writes are held to its share of the hour ([`AGENT_SLEEP_BYTES_PER_HOUR`]).
    Asked(recto_agent::Actor),
}

/// A tab goes dormant once it hasn't been active or edited for this long. SPEC decision 25's five minutes, its *default*.
pub const DORMANT_AFTER_SECONDS: u32 = 5 * 60;
/// What the documents off screen may hold in all, their workers' memory and their histories', before the oldest goes dormant. SPEC section 2: "a byte budget frees the oldest first". What's on screen is another row.
pub const LIVE_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
/// The least time between sleep files for a tab that's only moving about (the owner's rule on SSD wear). A flush (a quit's, a hand-off's) doesn't wait; being asked does, as the queue's writer does. A tab left sooner has its older file, whose facts are still good, and whose pictures show only on the pages they're of; unless no picture in it is of the place it's left at on its paper, when one write goes ahead within the hour's cap ([`crate::sleep_writer::there_for_other_place`]).
pub const SLEEP_WRITE_EVERY_SECONDS: u32 = 60;
/// **How long a tab pushed out of the warm ones waits before its sleep file is written** ([`crate::sleep_queue::Leaves`] says which wait) (owner, 2026-11-13, agreeing to the coordinator's recommendation: "the
/// cool-down is on the write"). The push out stays immediate, for memory; the write waits in the one queue there is, and a tab shown again inside the wait has
/// what it was copied with back for its first frame, with nothing written. A quit, a hand-off, going to the background, memory pressure and being asked don't
/// wait. A crash inside it costs that tab only its quick way back.
pub const LEAVING_WAIT_MS: u64 = 20_000;

/// What sleep files may come to in an hour, all told, for the whole app, for the same reason. **16 MiB** (owner, 2026-11-13, late evening, on measurements: "I
/// agree with 16 mb an hour"): a scripted hour among 6 of 12 tabs with real documents reached the 4 MiB cap before it after about 19 minutes, after which about
/// half of all wakes were bare, and wrote 12.2 MB with no cap at all; a sleep file is 65 KB on average, so about 250 files an hour for all tabs, and at worst
/// 128 MiB written in an eight-hour day. Before: 1 MiB, used by ordinary switching in about 14 writes; then 4 MiB.
pub const SLEEP_BYTES_PER_HOUR: u64 = 16 * 1024 * 1024;
/// **What sleep files written at an agent's asking may come to in an hour: 2 MiB** (the cool-down's review, 2026-11-13, and the owner the same evening, "I
/// agree with ... your other recommendations"): an agent looping a move and `session sleep` used the app's whole hour of sleep files (then 4 MiB, about 57
/// writes), after which every wake of the person's was bare for the hour. An agent's take at most this, so the person's own routes (tabs pushed out of the warm
/// ones, going dormant, a quit) always have the rest, 14 MiB. Within it an agent's writes are held to the gap and the place rules like any other.
pub const AGENT_SLEEP_BYTES_PER_HOUR: u64 = 2 * 1024 * 1024;
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

/// The hour under way, and what sleep files have come to in it, all told and at agents' asking.
#[derive(Debug)]
pub struct Hour {
    began: u64,
    bytes: u64,
    /// Of `bytes`, what was written at an agent's asking.
    agents: u64,
    /// [`SLEEP_BYTES_PER_HOUR`], set small by a test of the cap.
    cap: u64,
    /// [`AGENT_SLEEP_BYTES_PER_HOUR`], set small by a test of the agent's share.
    agent_cap: u64,
}

impl Hour {
    #[must_use]
    pub fn starting(now: u64) -> Self {
        Self {
            began: now,
            bytes: 0,
            agents: 0,
            cap: SLEEP_BYTES_PER_HOUR,
            agent_cap: AGENT_SLEEP_BYTES_PER_HOUR,
        }
    }

    /// The cap, for a test of it at the cap and one past it.
    pub fn set_cap(&mut self, cap: u64) {
        self.cap = cap;
    }

    /// The agents' share, for a test of it at the share and one past it.
    pub fn set_agent_cap(&mut self, cap: u64) {
        self.agent_cap = cap;
    }

    /// Whether this hour has room for another file, for an agent's asking or not, starting a new hour first if this one's over: room in the hour, and for an
    /// agent, room in its share too.
    pub fn has_room(&mut self, now: u64, agent: bool) -> bool {
        if now.saturating_sub(self.began) >= HOUR_MS {
            self.began = now;
            self.bytes = 0;
            self.agents = 0;
        }
        self.bytes < self.cap && (!agent || self.agents < self.agent_cap)
    }

    /// Whether it's the agents' share that's used, and not the hour: what an agent is told.
    #[must_use]
    pub fn agents_share_used(&self) -> bool {
        self.agents >= self.agent_cap && self.bytes < self.cap
    }

    /// Minutes until a new hour, rounded up, for what an agent is told.
    #[must_use]
    pub fn minutes_left(&self, now: u64) -> u64 {
        HOUR_MS.saturating_sub(now.saturating_sub(self.began)).div_ceil(60_000)
    }

    pub fn add(&mut self, bytes: u64, agent: bool) {
        self.bytes += bytes;
        if agent {
            self.agents += bytes;
        }
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
