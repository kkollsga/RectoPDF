//! **Every order the app's life can be told things in,** on numbers (tier 1). A launch is driven as the session thread drives it: the host's second of patience,
//! the core's own deadline for a launch an update may have opened, fired at the moment it's due, the state coming in at a chosen moment, before, at and after
//! each, both ways round where two land on the same millisecond, and the host making its window from the state once it hears. What the host hears and when, and
//! whether the window takes input at each moment, are held to what the rules say, not to one run's timings.

use super::*;
use crate::core::RESTORE_PATIENCE;
use crate::core::updates::{INPUT_PATIENCE, UPDATE_RESTORE_PATIENCE, out_of_sight_when_restored};

fn ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The host's own patience: its timer, a second after the launch.
fn host_patience() -> u64 {
    ms(RESTORE_PATIENCE)
}

/// One launch, driven as the session thread and the host drive it.
struct Launch {
    life: Life,
    now: u64,
    reopened: bool,
    /// What the host was told, and when.
    heard: Vec<(u64, Out)>,
    /// Whether the window took input, at each moment it was asked.
    asked: Vec<(u64, bool, Shown)>,
}

impl Launch {
    fn new(reopened: bool) -> Self {
        Self {
            life: Life::default(),
            now: 0,
            reopened,
            heard: Vec::new(),
            asked: Vec::new(),
        }
    }

    /// An event now, and what it came to, as the core carries it out: the host makes its window from the state as soon as it hears it's in.
    fn deliver(&mut self, event: Event) {
        let Some(out) = self.life.step(event, self.now) else { return };
        if out
            == (Out::RestoreReady {
                with_state: false,
                late: false,
            })
        {
            assert_eq!(
                self.life.shown(),
                Shown::WithoutState,
                "the window going up without the state is marked before the host's told, so a state that comes later is known to be late"
            );
        }
        self.heard.push((self.now, out));
        if let Out::RestoreReady { with_state: true, .. } = out {
            assert_eq!(self.life.step(Event::ShownWithState, self.now), None);
        }
    }

    /// The window's patience over, as the host's timer or the core's deadline says.
    fn patience_over(&mut self) {
        self.deliver(Event::PatienceOver {
            may_be_reopened_by_update: self.reopened,
        });
    }

    /// Time moves on to `to`, asking at every millisecond worth asking whether the window takes input, and the core's deadline fires when it's due, as the session
    /// thread's wait ends then: on `to` itself only if `deadline_first`.
    fn until(&mut self, to: u64, deadline_first: bool) {
        while let Some(wait) = self.life.due(self.now) {
            let at = self.now.saturating_add(ms(wait));
            if at > to || (at == to && !deadline_first) {
                break;
            }
            self.look(at);
            self.patience_over();
        }
        self.look(to);
    }

    /// Asks whether the window takes input at `at`, and just before.
    fn look(&mut self, at: u64) {
        for moment in [at.saturating_sub(1), at] {
            if moment >= self.now {
                self.now = moment;
                self.asked.push((moment, self.life.takes_input(moment), self.life.shown()));
            }
        }
    }

    fn without_state(&self) -> Vec<u64> {
        self.heard
            .iter()
            .filter(|(_, out)| matches!(out, Out::RestoreReady { with_state: false, .. }))
            .map(|(at, _)| *at)
            .collect()
    }

    fn with_state(&self) -> Vec<(u64, bool)> {
        self.heard
            .iter()
            .filter_map(|(at, out)| match out {
                Out::RestoreReady { with_state: true, late } => Some((*at, *late)),
                _ => None,
            })
            .collect()
    }
}

/// A launch, with the state coming in at `state_at`, and the host's patience, the deadline and the state in that order where they land together.
fn launch(reopened: bool, state_at: u64, patience_first: bool) -> Launch {
    let mut run = Launch::new(reopened);
    let patience = host_patience();
    if state_at < patience || (state_at == patience && !patience_first) {
        run.until(state_at, false);
        run.deliver(Event::StateRead);
        run.until(patience, false);
        run.patience_over();
    } else {
        run.until(patience, false);
        run.patience_over();
        run.until(state_at, patience_first);
        run.deliver(Event::StateRead);
    }
    run.until(state_at.max(patience) + ms(UPDATE_RESTORE_PATIENCE) + 1_000, false);
    run
}

/// Failure: the window goes up without the state when it should have waited, or waits when it shouldn't, or a state that comes later isn't made into the window,
/// or the window takes input before it's made from the state:
/// - **f399c45** (full check under load, 2026-11-11): a launch an update may have opened went up without its state at the second's patience, so a closed window
///   came back open and one in the Dock shown. It waits up to [`UPDATE_RESTORE_PATIENCE`], and any other launch a second.
/// - **f03b3e5** (CI, 2026-11-11): a state read after the window went up without it wasn't made into the window, and the sidebar's contents had no height. It's
///   said late, and the window's made again from it.
/// - **6aa9559** (CI, main at faa9180): ⌘F in the first second took the keyboard on a window the late state then made again. A window up without the state
///   takes no input until it's made from it, or [`INPUT_PATIENCE`] has gone by.
/// - **`show_without_state`'s order**: the window going up without the state is marked before the host hears (asserted as each is delivered).
///
/// Every launch: an update's or not, the state at, before and after the host's second and the core's 20 s, both ways round on a tie.
#[test]
fn every_order_of_the_state_and_the_patience_comes_to_what_the_rules_say() {
    let patience = host_patience();
    let update = ms(UPDATE_RESTORE_PATIENCE);
    let input = ms(INPUT_PATIENCE);
    let moments = [
        0,
        1,
        patience - 1,
        patience,
        patience + 1,
        2_500,
        input - 1,
        input,
        input + 1,
        update - 1,
        update,
        update + 1,
        update + 5_000,
    ];
    let mut launches = 0;
    for reopened in [false, true] {
        for state_at in moments {
            for patience_first in [false, true] {
                let run = launch(reopened, state_at, patience_first);
                launches += 1;
                let said = format!(
                    "reopened by an update {reopened}, the state at {state_at} ms, patience first {patience_first}: heard {:?}",
                    run.heard
                );
                // How long this launch waits for the state, and whether it came in time.
                let waits_until = if reopened { update } else { patience };
                let in_time = state_at < waits_until || (state_at == waits_until && !patience_first);
                assert_eq!(
                    run.with_state(),
                    vec![(state_at, !in_time)],
                    "told once, when the state's in, and late only if the window went up without it: {said}"
                );
                let expected_without: Vec<u64> = if in_time { Vec::new() } else { vec![waits_until] };
                assert_eq!(
                    run.without_state(),
                    expected_without,
                    "up without the state only once the patience is over, and at once then: {said}"
                );
                for (at, takes, shown) in &run.asked {
                    assert_eq!(
                        *takes,
                        *shown == Shown::WithState || *at >= input,
                        "at {at} ms, shown {shown:?}: input only once made from the state, or {input} ms on: {said}"
                    );
                }
                assert_eq!(run.life.due(run.now), None, "no deadline left once the state's in: {said}");
            }
        }
    }
    assert_eq!(launches, 52);
}

/// Failure (the core's own deadline, owner's decision 2026-11-12, in place of a thread that slept): the state coming in exactly at the 20 s deadline of a launch an
/// update may have opened is taken one way or the other, never both and never neither, and so is one a millisecond either side. On the tie, whichever the session
/// thread takes first decides.
#[test]
fn a_state_at_before_and_after_the_deadline_is_taken_one_way() {
    let update = ms(UPDATE_RESTORE_PATIENCE);
    for (state_at, deadline_first, up_without) in [
        (update - 1, true, false),
        (update, false, false),
        (update, true, true),
        (update + 1, false, true),
    ] {
        let run = launch(true, state_at, deadline_first);
        assert_eq!(
            !run.without_state().is_empty(),
            up_without,
            "the state at {state_at}, deadline first {deadline_first}: {:?}",
            run.heard
        );
        assert_eq!(run.with_state().len(), 1, "{:?}", run.heard);
        if up_without {
            assert_eq!(run.without_state(), vec![update], "exactly at the deadline");
        }
    }
}

/// Failure: the deadline's there when nothing waits on it, which wakes the session thread for nothing, or isn't when something does. Only a launch an update may
/// have opened, with the state not in, past the host's second.
#[test]
fn the_deadline_is_there_only_while_a_launch_waits_on_it() {
    let mut life = Life::default();
    assert_eq!(life.due(0), None, "nothing waits before the host's patience");
    assert_eq!(
        life.step(
            Event::PatienceOver {
                may_be_reopened_by_update: true
            },
            1_000
        ),
        None
    );
    assert_eq!(life.due(1_000), Some(UPDATE_RESTORE_PATIENCE - Duration::from_secs(1)));
    assert_eq!(life.due(25_000), Some(Duration::ZERO), "due, and past it");
    let _ = life.step(Event::StateRead, 2_000);
    assert_eq!(life.due(2_000), None, "the state's in");
    let mut life = Life::default();
    let _ = life.step(
        Event::PatienceOver {
            may_be_reopened_by_update: false,
        },
        1_000,
    );
    assert_eq!(life.due(1_000), None, "any other launch goes up at once");
}

/// Failure (63cc07b, review 1): a state that comes after the host has shown or hidden things without it writes the saved "hidden", "closed" or "in the Dock"
/// back while the app's on the screen, so the next update hides it. In both orders: the host decides before the state, and what it did stands; the state first,
/// and what was saved is what the host decides on.
#[test]
fn a_state_after_the_host_decided_keeps_what_the_host_did_in_either_order() {
    for saved in [false, true] {
        // The host's patience first: up without the state, so shown, and decided.
        let mut life = Life::default();
        let up = life.step(
            Event::PatienceOver {
                may_be_reopened_by_update: false,
            },
            1_000,
        );
        assert_eq!(
            up,
            Some(Out::RestoreReady {
                with_state: false,
                late: false
            })
        );
        let decided = Some(false);
        assert_eq!(life.step(Event::StateRead, 1_500), Some(Out::RestoreReady { with_state: true, late: true }));
        assert!(!out_of_sight_when_restored(saved, decided), "saved {saved}: what the host did stands");
        // The state first: nothing decided, so what was saved.
        let mut life = Life::default();
        assert_eq!(life.step(Event::StateRead, 500), Some(Out::RestoreReady { with_state: true, late: false }));
        assert_eq!(out_of_sight_when_restored(saved, None), saved, "what was saved, to decide on");
    }
}

/// Failure: a hand-off's freeze outlives a hand-off that didn't go ahead, or the app quits on one that wasn't written. In every order a hand-off's steps come in:
/// written, then undone (the updater gave up after it); undone before it was written (something came in, or it couldn't be saved); undone twice.
#[test]
fn a_hand_off_freezes_and_frees_in_every_order() {
    let mut life = Life::default();
    assert_eq!(life.step(Event::HandOffWriting, 0), Some(Out::Freeze(true)));
    assert_eq!(life.hand_off(), HandOff::Writing, "not yet to quit on");
    assert_eq!(life.step(Event::HandOffWritten, 1), None);
    assert_eq!(life.hand_off(), HandOff::Done, "quitting is what's left");
    assert_eq!(
        life.step(Event::HandOffUndone, 2),
        Some(Out::Freeze(false)),
        "given up after it was written: taken again"
    );
    assert_eq!(life.hand_off(), HandOff::Idle, "and nothing to quit on");
    assert_eq!(life.step(Event::HandOffWriting, 3), Some(Out::Freeze(true)));
    assert_eq!(life.step(Event::HandOffUndone, 4), Some(Out::Freeze(false)), "undone before it was written");
    assert_eq!(life.step(Event::HandOffUndone, 5), Some(Out::Freeze(false)), "and again, which still frees it");
    assert_eq!(life.hand_off(), HandOff::Idle);
}
