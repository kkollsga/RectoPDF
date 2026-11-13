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
        // As `App.swift` does: made again if Rust's rule says so now, and said.
        if let Out::RestoreReady { with_state: true, late } = out {
            if late && self.life.may_remake_window(self.now, false) {
                assert_eq!(self.life.step(Event::WindowRemade, self.now), None);
            }
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

/// Failure: a quit answers go and loses edits it couldn't write, asks when nothing would be lost, or a Stay with no question asked (or a second one) takes back a
/// quit that's going. Every way the write can end, then each answer: what the host is told, where the quit stands, and that a quit after a Stay starts again.
#[test]
fn a_quit_answers_by_what_it_kept_and_stay_takes_back_only_a_question() {
    for (written, unsaved_tabs) in [(true, 0), (true, 2), (false, 0), (false, 2)] {
        let mut life = Life::default();
        assert_eq!(life.step(Event::QuitStayed, 0), None, "a Stay before any quit changes nothing");
        assert_eq!(life.step(Event::QuitAsked, 1), Some(Out::QuitBegins));
        assert_eq!(life.quit(), Quit::Writing);
        let go = written || unsaved_tabs == 0;
        assert_eq!(
            life.step(Event::QuitWritten { written, unsaved_tabs }, 2),
            Some(Out::QuitAnswer {
                go,
                unsaved_tabs,
                unfreeze: !go
            }),
            "written {written}, {unsaved_tabs} unsaved"
        );
        if go {
            assert_eq!(life.quit(), Quit::Going);
            assert_eq!(life.step(Event::QuitStayed, 3), None, "nothing was asked, so nothing stays");
            assert_eq!(life.quit(), Quit::Going);
            continue;
        }
        assert_eq!(life.quit(), Quit::Asking, "it would lose edits: asked");
        assert_eq!(life.step(Event::QuitStayed, 3), Some(Out::QuitStayed));
        assert_eq!(life.quit(), Quit::No);
        assert_eq!(life.step(Event::QuitStayed, 4), None, "a second Stay changes nothing");
        assert_eq!(life.step(Event::QuitAsked, 5), Some(Out::QuitBegins), "and a quit after it starts again");
    }
}

/// Failure: a quit at some moment of a launch changes what the launch tells the host (a window that never goes up, or a state taken as late that wasn't), or the
/// launch changes what the quit answers. A quit asked and answered before, between and after the patience and the state, in every order of the two: the launch's
/// own outcomes are what they are with no quit, and the answer is the write's.
#[test]
fn a_quit_at_any_moment_of_a_launch_leaves_the_launch_as_it_was() {
    let patience = Event::PatienceOver {
        may_be_reopened_by_update: false,
    };
    let launches = [[patience, Event::StateRead], [Event::StateRead, patience]];
    for launch in launches {
        let alone: Vec<Option<Out>> = {
            let mut life = Life::default();
            launch.iter().map(|event| life.step(*event, 1_000)).collect()
        };
        // The quit's two steps at each of the three places around the launch's two, in order.
        for asked_at in 0..=2 {
            for written_at in asked_at..=2 {
                let mut life = Life::default();
                let mut launch_outs = Vec::new();
                let mut answer = None;
                for at in 0..=2 {
                    if at == asked_at {
                        assert_eq!(life.step(Event::QuitAsked, 1_000), Some(Out::QuitBegins));
                    }
                    if at == written_at {
                        answer = life.step(
                            Event::QuitWritten {
                                written: true,
                                unsaved_tabs: 1,
                            },
                            1_000,
                        );
                    }
                    if let Some(event) = launch.get(at) {
                        launch_outs.push(life.step(*event, 1_000));
                    }
                }
                assert_eq!(launch_outs, alone, "{launch:?}, asked at {asked_at}, written at {written_at}");
                assert_eq!(
                    answer,
                    Some(Out::QuitAnswer {
                        go: true,
                        unsaved_tabs: 1,
                        unfreeze: false
                    })
                );
            }
        }
    }
}

/// Failure: a quit and a hand-off let go of each other's freeze. A quit that asks while an update's hand-off is writing takes what a person does again, and what's
/// done then is lost, as what's on disk is what the new version comes back to; or a hand-off that gives up while a quit is writing does the same to the quit. Every
/// order a hand-off's steps (writing, then written or given up, and given up after it was written) and a quit's (asked, written, and a Stay when it asks) can
/// interleave in, the quit's write ending each way: after every step, what's held back is what the rule says (held while either holds it), and what the host
/// is told comes to that.
#[test]
fn a_quit_and_a_hand_off_keep_each_others_freeze_in_every_order() {
    let hand_offs: [&[Event]; 3] = [
        &[Event::HandOffWriting, Event::HandOffUndone],
        &[Event::HandOffWriting, Event::HandOffWritten],
        &[Event::HandOffWriting, Event::HandOffWritten, Event::HandOffUndone],
    ];
    let mut orders = 0;
    for hand_off in hand_offs {
        for (written, unsaved_tabs) in [(true, 1), (false, 1)] {
            let quit = [Event::QuitAsked, Event::QuitWritten { written, unsaved_tabs }, Event::QuitStayed];
            // Every interleaving of the two, each in its own order: which of the steps are the hand-off's.
            let steps = hand_off.len() + quit.len();
            for mask in 0u32..(1 << steps) {
                if mask.count_ones() as usize != hand_off.len() {
                    continue;
                }
                orders += 1;
                let (mut life, mut held) = (Life::default(), false);
                let (mut next_hand_off, mut next_quit) = (0, 0);
                for step in 0..steps {
                    let event = if mask & (1 << step) != 0 {
                        next_hand_off += 1;
                        hand_off[next_hand_off - 1]
                    } else {
                        next_quit += 1;
                        quit[next_quit - 1]
                    };
                    match life.step(event, 0) {
                        Some(Out::Freeze(frozen)) => held = frozen,
                        Some(Out::QuitBegins) => held = true,
                        Some(Out::QuitAnswer { unfreeze: true, .. }) => held = false,
                        _ => {}
                    }
                    let rule = life.hand_off() != HandOff::Idle || matches!(life.quit(), Quit::Writing | Quit::Going);
                    assert_eq!(life.frozen(), rule, "{hand_off:?} and {quit:?} by {mask:b}, after {event:?}");
                    assert_eq!(held, rule, "what the host was told: {hand_off:?} and {quit:?} by {mask:b}, after {event:?}");
                }
            }
        }
    }
    assert!(orders > 30, "{orders} orders");
}

/// Failure: a test of a late state can't tell whether the window was made again from it, so it passes on the on-time path when the machine is quick enough to
/// read the state first (`everything_comes_back_when_the_state_is_read_late`), or an update's launch, which waits for its state, is said to have been made again.
/// [`Life::remade`] is the window having gone up without the state and been made again from it: never when the state came in time, or within an update's
/// patience, or once the window has taken input without it, as then the host keeps the window it has.
#[test]
fn the_window_is_said_to_be_made_again_only_when_it_was() {
    let patience = host_patience();
    let input = ms(INPUT_PATIENCE);
    for (reopened, state_at, remade) in [
        (false, 0, false),
        (false, patience - 1, false),
        (false, patience + 1, true),
        (false, patience + 500, true),
        (false, input - 1, true),
        (false, input, false),
        (false, input + 2_000, false),
        (true, patience + 500, false),
        // Past an update's patience the window has been taking input for ten seconds already.
        (true, ms(UPDATE_RESTORE_PATIENCE) + 1, false),
    ] {
        for patience_first in [false, true] {
            let run = launch(reopened, state_at, patience_first);
            assert_eq!(
                run.life.remade(),
                remade,
                "reopened {reopened}, the state at {state_at} ms: heard {:?}",
                run.heard
            );
        }
    }
}

/// Every way to put the steps of these sequences in one order, each as the list of events that comes of it.
fn every_interleaving(sequences: &[&[Event]]) -> Vec<Vec<Event>> {
    fn go(sequences: &[&[Event]], at: &mut Vec<usize>, order: &mut Vec<Event>, all: &mut Vec<Vec<Event>>) {
        if sequences.iter().zip(at.iter()).all(|(sequence, at)| *at == sequence.len()) {
            all.push(order.clone());
            return;
        }
        for which in 0..sequences.len() {
            if at[which] < sequences[which].len() {
                order.push(sequences[which][at[which]]);
                at[which] += 1;
                go(sequences, at, order, all);
                at[which] -= 1;
                order.pop();
            }
        }
    }
    let mut all = Vec::new();
    go(sequences, &mut vec![0; sequences.len()], &mut Vec::new(), &mut all);
    all
}

/// Failure (the `Listener`'s promises, which until R1.4's fifth slice held `hand_off_quit` and `session_ended` to once only by convention): the host is told to
/// quit for a hand-off more than once, as a QA run asks twice (its `handoff`'s own thread and `hand_off_quit`) and the updater may ask too; or told for a
/// hand-off that was undone, or before it's written; or told the session ended twice; or a quit's answer is lost or doubled among them. Every interleaving of a
/// quit (kept or not, then Stay), the socket's server returning twice, and hand-offs that are written and asked to quit twice, undone before or after being
/// written or told, and done again after an undo: each hand-off that's written tells the host at most once and only after it's written, the session's end is
/// told once, and the quit's answer once and after its begin.
#[test]
fn the_host_is_told_to_quit_for_a_hand_off_and_that_the_session_ended_at_most_once_in_every_order() {
    use Event::{HandOffQuit as Ask, HandOffUndone as Undone, HandOffWriting as Writing, HandOffWritten as Written};
    // Each hand-off's events, and how many times the host should be told to quit for them in all.
    let hand_offs: [(&[Event], usize); 6] = [
        (&[Writing, Written, Ask, Ask], 1),
        (&[Ask, Writing, Undone, Ask], 0),
        (&[Writing, Undone, Written, Ask], 0),
        (&[Writing, Written, Ask, Undone, Ask], 1),
        (&[Writing, Written, Undone, Writing, Written, Ask, Ask], 1),
        (&[Writing, Written, Ask, Undone, Writing, Written, Ask], 2),
    ];
    let ends = [Event::SocketEnded, Event::SocketEnded];
    let mut orders = 0;
    for (hand_off, told) in hand_offs {
        for written in [true, false] {
            let quit = [Event::QuitAsked, Event::QuitWritten { written, unsaved_tabs: 1 }, Event::QuitStayed];
            for order in every_interleaving(&[hand_off, &quit, &ends]) {
                orders += 1;
                let mut life = Life::default();
                let (mut heard, mut written_since_writing) = (Vec::new(), false);
                let mut told_this_hand_off = 0;
                for event in &order {
                    match event {
                        Event::HandOffWriting => (written_since_writing, told_this_hand_off) = (false, 0),
                        Event::HandOffUndone => written_since_writing = false,
                        _ => {}
                    }
                    let before = life;
                    let out = life.step(*event, 0);
                    if *event == Event::HandOffWritten && before.hand_off() == HandOff::Writing {
                        written_since_writing = true;
                    }
                    if out == Some(Out::HandOffQuit) {
                        told_this_hand_off += 1;
                        assert!(written_since_writing, "{order:?}: told to quit for a hand-off that isn't written");
                        assert_eq!(told_this_hand_off, 1, "{order:?}: told twice for one hand-off");
                    }
                    if let Some(out) = out {
                        heard.push(out);
                    }
                }
                let count = |what: fn(&Out) -> bool| heard.iter().filter(|out| what(out)).count();
                assert_eq!(count(|out| *out == Out::HandOffQuit), told, "{order:?}: {heard:?}");
                assert_eq!(count(|out| *out == Out::SessionEnded), 1, "{order:?}: {heard:?}");
                assert_eq!(count(|out| matches!(out, Out::QuitAnswer { .. })), 1, "{order:?}: {heard:?}");
                let begun = heard.iter().position(|out| *out == Out::QuitBegins);
                let answered = heard.iter().position(|out| matches!(out, Out::QuitAnswer { .. }));
                assert!(begun < answered, "{order:?}: the quit's answer before its begin: {heard:?}");
            }
        }
    }
    assert!(orders > 10_000, "{orders} orders");
}

/// A life whose state was read without the record's lock, at `0`, the window made from it: trying again for the lock.
fn trying_for_the_lock() -> Life {
    let mut life = Life::default();
    assert_eq!(
        life.step(
            Event::Keeping(keeping::Event::Read {
                keeps: false,
                as_read: 7,
                by_update: false
            }),
            0
        ),
        None
    );
    let _ = life.step(Event::StateRead, 0);
    let _ = life.step(Event::ShownWithState, 0);
    life
}

/// Failure (owner, 2026-11-13: "restored as a launch would have"; the coordinator's conditions on the window made again): a late try's restore isn't made into
/// the window, so the window keeps a frame the record doesn't; or it's made again when the person has touched the window, closing what they're using; or twice;
/// or a state read without the lock (empty) is made into the window while it tries, which makes a second remake later. Both sides, on numbers: untouched, once;
/// touched, never; never while trying or restoring; and the ordinary late state's rule as it was.
#[test]
fn a_late_keep_is_made_into_the_window_once_unless_touched() {
    for touched in [false, true] {
        let mut life = trying_for_the_lock();
        assert!(!life.may_remake_window(500, false), "nothing to make while it tries");
        assert_eq!(life.step(Event::Keeping(keeping::Event::Asking), 3_000), Some(Out::Keeping(keeping::Out::Ask)));
        assert_eq!(
            life.step(Event::Keeping(keeping::Event::Tried { got: true }), 3_000),
            Some(Out::Keeping(keeping::Out::Restore))
        );
        assert!(!life.may_remake_window(3_000, false), "nor while the saved session's read");
        assert_eq!(
            life.step(
                Event::Keeping(keeping::Event::Read {
                    keeps: true,
                    as_read: 0,
                    by_update: false
                }),
                3_050
            ),
            Some(Out::Keeping(keeping::Out::Kept { remake: true })),
            "kept, the window to be made again from it: the host's told, as at a launch"
        );
        assert_eq!(life.may_remake_window(3_060, touched), !touched, "touched {touched}");
        let _ = life.step(Event::ShownWithState, 3_070);
        assert!(!life.may_remake_window(3_080, false), "once a launch");
    }
}

/// Failure: the rule for a state that came late with the lock got at once changes with this: the window not made again from it while it takes no input yet, or
/// made again once it does.
#[test]
fn the_ordinary_late_state_is_made_into_the_window_as_before() {
    let mut life = Life::default();
    assert_eq!(
        life.step(
            Event::Keeping(keeping::Event::Read {
                keeps: true,
                as_read: 0,
                by_update: false
            }),
            0
        ),
        Some(Out::Keeping(keeping::Out::Kept { remake: false }))
    );
    let _ = life.step(
        Event::PatienceOver {
            may_be_reopened_by_update: false,
        },
        ms(RESTORE_PATIENCE),
    );
    let _ = life.step(Event::StateRead, ms(RESTORE_PATIENCE) + 200);
    assert!(life.may_remake_window(ms(RESTORE_PATIENCE) + 300, false), "late, before it takes input");
    assert!(
        life.may_remake_window(ms(RESTORE_PATIENCE) + 300, true),
        "the ordinary rule goes by input taken, not touches"
    );
    assert!(!life.may_remake_window(ms(INPUT_PATIENCE), false), "not once it takes input");
}

/// Failure (review, 2026-11-13, route 1): a quit or a hand-off begins while it tries for the lock, and a try after that takes the lock and restores under the
/// quit's write. Either one stops the trying, with nothing said.
#[test]
fn a_quit_or_a_hand_off_stops_trying_for_the_lock() {
    for event in [Event::QuitAsked, Event::HandOffWriting] {
        let mut life = trying_for_the_lock();
        let _ = life.step(event, 400);
        assert_eq!(life.keeping(), keeping::Keeping::Stopped, "{event:?}");
        assert_eq!(life.keeping_due(1_000), None, "{event:?}: no more tries");
    }
}

/// Failure (review, 2026-11-13: `record.remade`, which explains a failing launch test, must say what happened): the window went up without the state, and the
/// state came in without the record's lock while it's trying, so the host doesn't make the window again (nothing to make it from, and one may come) and says
/// only that it's shown with the state; and the record says it was made again anyway, as it was inferred from the window having gone up without the state.
/// Made again is what the host says it did: a window replaced.
#[test]
fn a_window_not_made_again_is_never_said_to_have_been() {
    let mut life = Life::default();
    let late = host_patience() + 200;
    let _ = life.step(
        Event::PatienceOver {
            may_be_reopened_by_update: false,
        },
        host_patience(),
    );
    assert_eq!(
        life.step(
            Event::Keeping(keeping::Event::Read {
                keeps: false,
                as_read: 7,
                by_update: false
            }),
            late
        ),
        None
    );
    assert_eq!(life.step(Event::StateRead, late), Some(Out::RestoreReady { with_state: true, late: true }));
    assert!(!life.may_remake_window(late, false), "not made again while it tries");
    let _ = life.step(Event::ShownWithState, late);
    assert!(!life.remade(), "and not said to have been");
}
