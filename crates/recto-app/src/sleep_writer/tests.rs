//! **Every order the sleep writer can be told things in,** on numbers (tier 1): tabs asked to go for each reason, held or not, frozen or not; writes by a flush,
//! by being asked and by the queue's writer, around the gap's passing, the hour's turning and the hour filling; flushes wanting the files' lock around the
//! writer's next; the system's pressure in a quiet run and not. What comes out is held to what the rules say, the owner's on SSD wear among them, not to one
//! run's timings. The bugs fixed in the sleep writer since 2026-11-11 are named on the tests that would fail on them.

use super::*;
use crate::sleeper::{OTHERS_FIRST_FOR_MS, SLEEP_BYTES_PER_HOUR};

const WHYS: [Why; 4] = [Why::LeftAlone, Why::Budget, Why::Pressure, Why::Asked];
const HOUR_MS: u64 = 3_600_000;

/// Every way to put `lengths[i]` steps of each of the sequences in one order: each as a list of which sequence's next step comes.
fn interleavings(lengths: &[usize]) -> Vec<Vec<usize>> {
    fn go(left: &mut [usize], order: &mut Vec<usize>, all: &mut Vec<Vec<usize>>) {
        if left.iter().all(|left| *left == 0) {
            all.push(order.clone());
            return;
        }
        for which in 0..left.len() {
            if left[which] > 0 {
                left[which] -= 1;
                order.push(which);
                go(left, order, all);
                order.pop();
                left[which] += 1;
            }
        }
    }
    let mut all = Vec::new();
    go(&mut lengths.to_vec(), &mut Vec::new(), &mut all);
    all
}

/// Every permutation of `0..n`.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    interleavings(&vec![1; n])
}

/// Failure (9988d7c, review 1 of the sleep queue): going dormant by itself (left alone, the budget, memory pressure) wrote each tab's file on the session thread,
/// which with a slow disk waited behind the queue's writer, file after file, serving nothing else. Only being asked, which an agent looks at straight after,
/// writes in place; every other reason queues, and the journals go first whichever it is.
#[test]
fn going_dormant_by_itself_is_queued_and_only_being_asked_writes_in_place() {
    for why in WHYS {
        let mut machine = SleepWriter::default();
        let out = machine.step(
            Event::GoDormant {
                why,
                tabs: vec![(TabId(1), true), (TabId(2), true)],
                frozen: false,
            },
            1,
        );
        assert_eq!(
            out,
            Some(Out::GoDormant {
                tabs: vec![TabId(1), TabId(2)],
                in_place: why == Why::Asked
            }),
            "{why:?}"
        );
    }
}

/// Failure (SPEC decision 25, `Session::hold` and saving): a tab an agent is working on, one being saved, or the one on screen goes dormant when the budget,
/// pressure or being asked wants it, so its list or its worker goes from under the command; or something goes while a quit or a hand-off has frozen the app.
/// Every reason, every choice of which of three tabs the session lets go, frozen and not: only those it lets go, and none frozen.
#[test]
fn a_tab_held_never_goes_whatever_asks_and_nothing_goes_frozen() {
    let mut orders = 0;
    for why in WHYS {
        for mask in 0u32..8 {
            for frozen in [false, true] {
                orders += 1;
                let tabs: Vec<(TabId, bool)> = (0..3).map(|at| (TabId(at), mask & (1 << at) != 0)).collect();
                let may: Vec<TabId> = tabs.iter().filter(|(_, may)| *may).map(|(id, _)| *id).collect();
                let out = SleepWriter::default().step(Event::GoDormant { why, tabs, frozen }, 1);
                match out {
                    None => assert!(frozen || may.is_empty(), "{why:?} {mask:b}: nothing went where {may:?} could"),
                    Some(Out::GoDormant { tabs, .. }) => {
                        assert!(!frozen, "{why:?} {mask:b}: went while frozen");
                        assert_eq!(tabs, may, "{why:?} {mask:b}");
                    }
                    other => panic!("{other:?}"),
                }
            }
        }
    }
    assert_eq!(orders, 64);
}

/// Failure: a look at dormancy that comes while a file's being read, or while the app's frozen, drops its deadline, so the tabs left alone stay awake, holding
/// their memory, until something else happens to rearm it; or it looks again at once, in a loop. It comes again [`BUSY_RETRY_MS`] later, as many times as it's
/// busy, and then asks the session which are due; and a deadline worked out is the clock plus the session's wait, or none with nothing to wait for.
#[test]
fn a_busy_look_at_dormancy_comes_again_and_is_never_dropped() {
    let mut machine = SleepWriter::default();
    let mut now = 1_000;
    for _ in 0..3 {
        assert_eq!(machine.step(Event::DueNow { busy: true }, now), Some(Out::Due(now + BUSY_RETRY_MS)));
        now += BUSY_RETRY_MS;
    }
    assert_eq!(machine.step(Event::DueNow { busy: false }, now), Some(Out::TakeDue));
    assert_eq!(machine.step(Event::Rearm { in_seconds: Some(7) }, now), Some(Out::Due(now + 7_000)));
    assert_eq!(
        machine.step(Event::Rearm { in_seconds: None }, now),
        Some(Out::Due(0)),
        "nothing to wait for, no deadline"
    );
}

/// One step of an order of writes: a write asked for, by whom and of which tab, or the clock moving on.
#[derive(Clone, Copy, Debug)]
enum Happens {
    Write(TabId, By),
    Advance(u64),
    /// Another tab's big file, which fills the hour.
    FillTheHour,
}

/// What the rules say a write comes to, kept beside the machine as the rules are written: a flush never waits; anything else with a file there gets new
/// pictures only past its gap since its own last write, and while the hour (which starts again when an hour has gone since it began, at the next look) has
/// room.
#[derive(Default)]
struct Rules {
    last: std::collections::HashMap<TabId, u64>,
    hour_began: u64,
    hour_bytes: u64,
}

impl Rules {
    fn may(&mut self, id: TabId, by: By, gap: u64, now: u64) -> bool {
        if by == By::Flush {
            return true;
        }
        let past_gap = self.last.get(&id).is_none_or(|at| now - at >= gap);
        if now - self.hour_began >= HOUR_MS {
            (self.hour_began, self.hour_bytes) = (now, 0);
        }
        past_gap && self.hour_bytes < SLEEP_BYTES_PER_HOUR
    }

    fn wrote(&mut self, id: TabId, bytes: u64, now: u64) {
        self.last.insert(id, now);
        self.hour_bytes += bytes;
    }
}

/// Failure (the owner's SSD rule, "edits come in bursts", and the review that made being asked count against the hour): a tab that's only moving about gets a
/// new sleep file with pictures within its gap or past the hour's cap; or being asked writes past them as a flush does; or a flush, which the next launch's
/// first frame is, is held back by them; or the cap never lifts with a new hour, or the gap with its passing. Writes by the queue, by being asked and by a
/// flush, of two tabs, the gap's passing, the hour's turning and another tab filling the hour, in all 5,040 orders: each write comes to what the rules say.
#[test]
fn every_order_of_writes_the_gap_and_the_hour_keeps_the_ssd_rule() {
    let gap_ms = u64::from(SLEEP_WRITE_EVERY_SECONDS) * 1000;
    let (one, two, filler) = (TabId(1), TabId(2), TabId(9));
    let steps = [
        Happens::Write(one, By::Queue),
        Happens::Write(one, By::Asked),
        Happens::Write(one, By::Flush),
        Happens::Write(two, By::Queue),
        Happens::Advance(gap_ms),
        Happens::Advance(HOUR_MS),
        Happens::FillTheHour,
    ];
    let (mut put_off, mut with_pictures) = (0, 0);
    for order in permutations(steps.len()) {
        let mut machine = SleepWriter::default();
        let mut rules = Rules {
            hour_began: 1,
            ..Rules::default()
        };
        let mut now = 1;
        for &at in &order {
            match steps[at] {
                Happens::Advance(ms) => now += ms,
                Happens::FillTheHour => {
                    let _ = rules.may(filler, By::Flush, gap_ms, now);
                    let _ = machine.step(
                        Event::Wrote {
                            id: filler,
                            bytes: SLEEP_BYTES_PER_HOUR,
                        },
                        now,
                    );
                    rules.wrote(filler, SLEEP_BYTES_PER_HOUR, now);
                }
                Happens::Write(id, by) => {
                    let expected = rules.may(id, by, gap_ms, now);
                    let out = machine.step(
                        Event::Write {
                            id,
                            by,
                            there: true,
                            pictures_allowed: true,
                        },
                        now,
                    );
                    let want = if expected { Write::WithPictures } else { Write::PutOff };
                    assert_eq!(out, Some(Out::Write(want)), "{id:?} by {by:?}, in the order {order:?}, at {now}");
                    if expected {
                        with_pictures += 1;
                        let _ = machine.step(Event::Wrote { id, bytes: 8 * 1024 }, now);
                        rules.wrote(id, 8 * 1024, now);
                    } else {
                        put_off += 1;
                    }
                }
            }
        }
    }
    assert!(
        put_off > 1_000 && with_pictures > 1_000,
        "both ways were reached: {put_off} put off, {with_pictures} written"
    );
}

/// Failure: a tab with no sleep file yet is put off by the limits, so it wakes from its PDF when its facts, small, would have said what the PDF does; or a
/// protected file's pages reach a sleep file, by a flush or otherwise. With no file there, past the limits it's facts only; pictures not allowed, it's facts
/// only whoever writes it.
#[test]
fn a_tab_with_no_file_gets_its_facts_and_a_protected_one_never_gets_pictures() {
    let mut machine = SleepWriter::default();
    let write = |machine: &mut SleepWriter, by, there, pictures_allowed| {
        machine.step(
            Event::Write {
                id: TabId(1),
                by,
                there,
                pictures_allowed,
            },
            10,
        )
    };
    let _ = machine.step(Event::Wrote { id: TabId(1), bytes: 100 }, 10);
    assert_eq!(
        write(&mut machine, By::Queue, false, true),
        Some(Out::Write(Write::FactsOnly)),
        "within its gap, no file"
    );
    assert_eq!(
        write(&mut machine, By::Queue, true, true),
        Some(Out::Write(Write::PutOff)),
        "within its gap, a file"
    );
    for by in [By::Flush, By::Asked, By::Queue] {
        assert_eq!(
            write(&mut machine, by, false, false),
            Some(Out::Write(Write::FactsOnly)),
            "protected, by {by:?}"
        );
    }
    assert_eq!(write(&mut machine, By::Flush, true, false), Some(Out::Write(Write::FactsOnly)));
    assert_eq!(machine.counts().writes_put_off, 1, "only the one put off is counted as put off");
    // A gap of none writes every time, the hour's cap with it.
    let _ = machine.step(Event::SetWriteGap(0), 10);
    let _ = machine.step(
        Event::Wrote {
            id: TabId(5),
            bytes: SLEEP_BYTES_PER_HOUR,
        },
        10,
    );
    assert_eq!(write(&mut machine, By::Queue, true, true), Some(Out::Write(Write::WithPictures)));
}

/// Failure (d65da39 and 80af1aa, slow mode 2026-11-11: a quit after opening tabs lost them, as its flush waited behind every sleep file queued): the queue's
/// writer takes the files' lock again before a flush waiting for it, every time, so the flush runs out of its time; or it holds back with nobody waiting, or past
/// its bound; or a flush that ran out of time skips the tab on screen's file without a trace, so the next launch's bare first frame is unexplained. Two flushes
/// (a quit's and a hand-off's), one getting the lock and one running out of time, and the writer asking three times, in all 210 orders: the writer holds back
/// exactly while one is waiting, never past its bound, and the one out of time is counted.
#[test]
fn a_flush_waiting_goes_before_the_writer_in_every_order_and_one_out_of_time_is_counted() {
    let orders = interleavings(&[2, 2, 3]);
    assert_eq!(orders.len(), 210);
    for order in orders {
        let mut machine = SleepWriter::default();
        let (mut next, mut waiting) = ([0usize; 2], 0u32);
        for &who in &order {
            match who {
                0 | 1 => {
                    let event = if next[who] == 0 {
                        waiting += 1;
                        Event::LockWanted
                    } else {
                        waiting -= 1;
                        Event::LockWaited { got: who == 0 }
                    };
                    next[who] += 1;
                    assert_eq!(machine.step(event, 1), None);
                }
                _ => {
                    assert_eq!(machine.holds_back(0), waiting > 0, "{order:?}: {waiting} waiting");
                    assert!(!machine.holds_back(OTHERS_FIRST_FOR_MS), "{order:?}: bounded");
                }
            }
        }
        assert!(!machine.holds_back(0), "{order:?}: nobody's waiting at the end");
        assert_eq!(machine.counts().writes_skipped_busy, 1, "{order:?}: the flush out of time is counted");
    }
}

/// Failure (f1de855, the 24-hour soak of 2026-11-08 went through 3,068 tabs): the machine remembers when every tab that ever had a sleep file last wrote it,
/// closed ones too, without a cap. However many write, it remembers those within the gap only, and a tab past it may write again.
#[test]
fn what_is_remembered_of_writes_stays_within_the_gap() {
    let mut machine = SleepWriter::default();
    let gap_ms = u64::from(SLEEP_WRITE_EVERY_SECONDS) * 1000;
    for id in 0..3_000 {
        let _ = machine.step(Event::Wrote { id: TabId(id), bytes: 1 }, 1 + u64::from(id));
    }
    assert!(machine.remembered() <= 3_000);
    let now = 1 + 3_000 + gap_ms;
    let _ = machine.step(Event::Wrote { id: TabId(9_999), bytes: 1 }, now);
    assert_eq!(machine.remembered(), 1, "those older than the gap have gone");
    let write = Event::Write {
        id: TabId(5),
        by: By::Queue,
        there: true,
        pictures_allowed: true,
    };
    assert_eq!(machine.step(write, now), Some(Out::Write(Write::WithPictures)));
    assert_eq!(
        machine.step(Event::Wrote { id: TabId(1), bytes: 0 }, now),
        None,
        "nothing new written is no write"
    );
    assert_eq!(machine.counts().sleep_files, 3_001);
}

/// Failure (0f58dba, full check 2026-11-12: a test's tabs went to sleep under the kernel's only warning that hour): a quiet run acts on the machine's own memory
/// pressure; or the app a person uses stops acting on it; or a test's own pressure doesn't put every tab that can go to sleep. And it's counted before it's
/// acted on, so a test can see it came.
#[test]
fn the_systems_pressure_in_a_quiet_run_is_counted_and_acted_on_otherwise() {
    let mut machine = SleepWriter::default();
    for critical in [false, true] {
        assert_eq!(machine.step(Event::SystemPressure { critical, quiet: true }, 1), None);
        assert_eq!(machine.step(Event::SystemPressure { critical, quiet: false }, 1), Some(Out::Pressure(critical)));
        assert_eq!(machine.step(Event::Pressure { critical }, 1), Some(Out::AllGo(critical)));
    }
    let counts = machine.counts();
    assert_eq!((counts.system_warning, counts.system_critical, counts.pressure_events), (1, 1, 2));
}

/// Failure (SPEC section 2, "a byte budget frees the oldest first"): the budget's looked at on every change, a call to the kernel per worker each time, or with
/// nothing off screen; or a change of tab waits out the second; or over it the newest goes. And what went is counted by its reason.
#[test]
fn the_budget_is_looked_at_once_a_second_or_at_a_change_of_tab_and_the_oldest_go() {
    let mut machine = SleepWriter::default();
    let look = |machine: &mut SleepWriter, active, off_screen, now| machine.step(Event::LookAtBudget { active, off_screen }, now);
    assert_eq!(look(&mut machine, Some(TabId(1)), false, 1), None, "nothing off screen");
    assert_eq!(look(&mut machine, Some(TabId(1)), true, 1), Some(Out::CountCosts));
    assert_eq!(look(&mut machine, Some(TabId(1)), true, 500), None, "within the second");
    assert_eq!(look(&mut machine, Some(TabId(2)), true, 500), Some(Out::CountCosts), "another tab showing");
    let _ = machine.step(Event::SetBudget(70), 500);
    let costs = vec![(TabId(1), 50), (TabId(2), 30), (TabId(3), 40)];
    assert_eq!(machine.step(Event::Costs(costs), 500), Some(Out::SleepTabs(vec![TabId(1)], Why::Budget)));
    assert_eq!(machine.step(Event::Costs(vec![(TabId(1), 70)]), 500), None, "within it");
    for why in WHYS {
        let _ = machine.step(Event::WentDormant { why, gone: 2 }, 500);
    }
    let counts = machine.counts();
    assert_eq!((counts.left_alone, counts.budget, counts.pressure, counts.asked), (2, 2, 2, 2));
}
