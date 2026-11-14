//! **The person's lane: their edits take effect in the order they asked for them** (the ⌃⌘H-then-⌘Z fault, 2026-11-14). A person's command that may wait
//! (a highlight reading its pages, a save, a print) runs on a thread of its own, and the rest on the session thread's queue; with nothing more, an Undo
//! pressed while a highlight was being read ran first and undid the step before it. Each of the person's ordered commands takes a ticket as it's asked, and
//! acts on the document only on its turn: when every ticket before it has **passed**, that is, its command has fixed the state it acts on (a highlight landed,
//! a print's pages read, a save begun) or has gone without doing anything.
//!
//! **Nothing waits.** A command whose turn hasn't come is parked here, with no thread of its own, and started by whoever passes the ticket before it, as that
//! one passes ([`Turn`]'s drop): the thread of the command before as it ends, or the session thread as it finishes one. One lane for the app, not a lane per
//! tab: an idle lane holds no buffer, and a parked edit in another tab waits at most for the one command ahead of it.
//!
//! What's ordered, and how each command is started, is the person's commands' own (`core::person`); this holds only the order.

use crate::lock;
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Mutex, Weak};

/// How many of the person's commands can be parked at once. Reached only by a key held down while a command waits: the next is refused, and said so.
pub const CAP: usize = 64;

/// What a parked command does once its turn comes, given its turn to hold until it passes.
pub type Start = Box<dyn FnOnce(Turn) + Send>;

/// The lane: the tickets handed out, how far they've passed, and the commands parked for their turn.
pub struct Lane {
    state: Mutex<State>,
}

struct State {
    /// The next ticket to be handed out.
    next: u64,
    /// Every ticket below this has passed: this one's is the next turn.
    passed: u64,
    /// Tickets that passed before their turn, as their command went without running (a freeze, the end): skipped when the turn reaches them.
    gone: BTreeSet<u64>,
    /// Tickets not yet passed, with the tab each names (0: none), so a close can ask whether an edit of its tab is still landing.
    outstanding: VecDeque<(u64, u32)>,
    /// Commands waiting for their turn, by ticket, each with the way back to the queue its turn will hold.
    parked: VecDeque<(u64, Weak<super::CoreQueue>, Start)>,
    /// [`CAP`], or a test's smaller one.
    cap: usize,
}

impl Default for Lane {
    /// Made with room for a few tickets and its lock taken once, so the window's thread allocates nothing for the person's first edits (a ticket each,
    /// held only while its command is on its way): one small buffer for the app, not one for anything idle.
    fn default() -> Self {
        let lane = Self {
            state: Mutex::new(State {
                next: 0,
                passed: 0,
                gone: BTreeSet::new(),
                outstanding: VecDeque::with_capacity(8),
                parked: VecDeque::new(),
                cap: CAP,
            }),
        };
        drop(lock(&lane.state));
        lane
    }
}

/// What came of a command that asked for its turn.
pub enum Asked {
    /// It's its turn: start it, holding the turn till it passes.
    Now(Start, Turn),
    /// Parked, to be started by whoever passes the ticket before it.
    Parked,
    /// Too many parked already: not taken. Its turn passes as it's dropped.
    Full(Start, Turn),
}

/// **A ticket in the lane, held by its command until the state it acts on is fixed:** passed as it's dropped, however the command ends (done, refused,
/// dropped by a freeze or the end, a panic), and the next command whose turn it is started.
pub struct Turn {
    /// The queue whose lane it's in: `None` once it's parked, as the ticket is then passed only by the turn its command is started with.
    queue: Option<Weak<super::CoreQueue>>,
    ticket: u64,
}

impl std::fmt::Debug for Turn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Turn({})", self.ticket)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(queue) = self.queue.take().and_then(|queue| queue.upgrade()) {
            queue.lane().pass(self.ticket);
        }
    }
}

impl Turn {
    /// The ticket it holds.
    #[must_use]
    pub fn ticket(&self) -> u64 {
        self.ticket
    }
}

impl Lane {
    /// A ticket for a command just asked, naming `tab` (0: none).
    pub(super) fn take(&self, queue: Weak<super::CoreQueue>, tab: u32) -> Turn {
        let mut state = lock(&self.state);
        let ticket = state.next;
        state.next += 1;
        state.outstanding.push_back((ticket, tab));
        Turn { queue: Some(queue), ticket }
    }

    /// **The command holding `turn` asks to act now:** it may if its turn has come, and is parked otherwise, to be started with its turn when it does.
    pub fn ask(&self, mut turn: Turn, start: Start) -> Asked {
        let mut state = lock(&self.state);
        if turn.ticket == state.passed {
            return Asked::Now(start, turn);
        }
        if state.parked.len() >= state.cap {
            drop(state);
            return Asked::Full(start, turn);
        }
        let ticket = turn.ticket;
        // Parked: the ticket is passed only by the turn it's started with, so this one goes without passing it.
        let Some(queue) = turn.queue.take() else {
            drop(state);
            return Asked::Full(start, turn);
        };
        // In order of tickets: a command parked by the session thread can be parked after one asked later on the window's thread.
        let at = state.parked.partition_point(|(parked, ..)| *parked < ticket);
        state.parked.insert(at, (ticket, queue, start));
        Asked::Parked
    }

    /// Whether `turn`'s ticket is the next turn now.
    pub fn is_turn(&self, turn: &Turn) -> bool {
        lock(&self.state).passed == turn.ticket
    }

    /// The ticket passed: the turn moves on, and the command whose turn it is now, if it's parked, is started (outside the lane's lock).
    fn pass(&self, ticket: u64) {
        let start = {
            let mut state = lock(&self.state);
            if ticket < state.passed || state.gone.contains(&ticket) {
                return;
            }
            state.outstanding.retain(|(outstanding, _)| *outstanding != ticket);
            if ticket == state.passed {
                state.passed += 1;
                loop {
                    let passed = state.passed;
                    if !state.gone.remove(&passed) {
                        break;
                    }
                    state.passed += 1;
                }
            } else {
                state.gone.insert(ticket);
            }
            match state.parked.front() {
                Some((parked, ..)) if *parked == state.passed => state.parked.pop_front(),
                _ => None,
            }
        };
        if let Some((ticket, queue, start)) = start {
            start(Turn { queue: Some(queue), ticket });
        }
    }

    /// **The queue's end** (a headless app's kill): what's parked goes, with what it holds (the app's count of it), and nothing more starts, as the queue
    /// takes nothing more.
    pub(super) fn clear(&self) {
        let parked = std::mem::take(&mut lock(&self.state).parked);
        drop(parked);
    }

    /// How many commands are parked now.
    pub fn parked(&self) -> usize {
        lock(&self.state).parked.len()
    }

    /// Whether an ordered command naming `tab` is still to pass: an edit of the tab still landing.
    pub fn naming(&self, tab: u32) -> bool {
        tab != 0 && lock(&self.state).outstanding.iter().any(|(_, named)| *named == tab)
    }

    /// A test's smaller cap.
    #[cfg(test)]
    pub fn set_cap(&self, cap: usize) {
        lock(&self.state).cap = cap;
    }
}

std::thread_local! {
    /// The turn the command running on this thread holds, if it's one of the person's ordered commands on a thread of its own ([`hold_here`]).
    static HERE: std::cell::RefCell<Option<Turn>> = const { std::cell::RefCell::new(None) };
}

/// **The turn of the command this thread runs, held while it runs:** passed as the guard goes, or before, where the command has fixed what it acts on
/// ([`pass_here`]): a print once its pages are read, a save once it has begun.
pub struct HeldHere(());

/// Holds `turn` on this thread for the command it runs.
#[must_use]
pub fn hold_here(turn: Option<Turn>) -> HeldHere {
    let before = HERE.with(|here| here.replace(turn));
    drop(before);
    HeldHere(())
}

impl Drop for HeldHere {
    fn drop(&mut self) {
        pass_here();
    }
}

/// **The command on this thread has fixed the state it acts on:** its turn passes now, and the person's next command starts. Nothing on a thread that holds
/// no turn: an agent's command, or one of the person's that isn't ordered.
pub fn pass_here() {
    // Taken out first and dropped after, as passing may start the next command.
    let turn = HERE.with(|here| here.borrow_mut().take());
    drop(turn);
}

#[cfg(test)]
mod tests {
    use super::{Asked, Start, Turn};
    use crate::queue::CoreQueue;
    use std::sync::{Arc, Mutex};

    /// What started, in order, each start keeping its turn in `held` until the test lets it go.
    struct Started {
        order: Arc<Mutex<Vec<&'static str>>>,
        held: Arc<Mutex<Vec<(&'static str, Turn)>>>,
    }

    impl Started {
        fn new() -> Self {
            Self {
                order: Arc::default(),
                held: Arc::default(),
            }
        }

        fn start(&self, name: &'static str) -> Start {
            let (order, held) = (Arc::clone(&self.order), Arc::clone(&self.held));
            Box::new(move |turn| {
                order.lock().unwrap().push(name);
                held.lock().unwrap().push((name, turn));
            })
        }

        /// Lets `name`'s turn go: it passes.
        fn pass(&self, name: &str) {
            let turn = {
                let mut held = self.held.lock().unwrap();
                let at = held.iter().position(|(held, _)| *held == name).expect("held");
                held.remove(at).1
            };
            drop(turn);
        }

        fn order(&self) -> Vec<&'static str> {
            self.order.lock().unwrap().clone()
        }
    }

    /// Asks for `name`'s turn: started now, or parked.
    fn ask(queue: &Arc<CoreQueue>, started: &Started, name: &'static str, tab: u32) -> &'static str {
        match queue.lane().ask(queue.take_turn(tab), started.start(name)) {
            Asked::Now(start, turn) => {
                start(turn);
                "now"
            }
            Asked::Parked => "parked",
            Asked::Full(..) => "full",
        }
    }

    /// Failure: the person's commands start in another order than asked: one asked after another that hasn't passed starts at once (the highlight that
    /// read before the undo queued ahead of it ran, or the undo before the highlight), or a parked one never starts once the one before passes. Each
    /// starts only as the one before passes, in the order asked, whoever passes it.
    #[test]
    fn each_starts_as_the_one_before_passes_in_the_order_asked() {
        let queue = Arc::new(CoreQueue::default());
        let started = Started::new();
        assert_eq!(ask(&queue, &started, "highlight", 1), "now");
        assert_eq!(ask(&queue, &started, "undo", 1), "parked");
        assert_eq!(ask(&queue, &started, "redo", 2), "parked");
        assert_eq!(queue.lane().parked(), 2);
        assert_eq!(started.order(), ["highlight"]);
        started.pass("highlight");
        assert_eq!(
            started.order(),
            ["highlight", "undo"],
            "the undo starts as the highlight passes, and not the redo"
        );
        started.pass("undo");
        started.pass("redo");
        assert_eq!(started.order(), ["highlight", "undo", "redo"]);
        assert_eq!(queue.lane().parked(), 0);
        assert_eq!(ask(&queue, &started, "after", 0), "now", "nothing ahead: at once");
    }

    /// Failure: a ticket dropped before its turn (its command dropped by a freeze or the queue's end, or refused at the cap) stalls the lane for good, or
    /// lets one asked after it start before the ones ahead of it. It's skipped when the turn reaches it.
    #[test]
    fn a_ticket_that_goes_without_running_is_skipped() {
        let queue = Arc::new(CoreQueue::default());
        let started = Started::new();
        assert_eq!(ask(&queue, &started, "highlight", 1), "now");
        let dropped = queue.take_turn(1);
        assert_eq!(ask(&queue, &started, "undo", 1), "parked");
        drop(dropped);
        assert_eq!(started.order(), ["highlight"], "the undo still waits for the highlight");
        started.pass("highlight");
        assert_eq!(started.order(), ["highlight", "undo"], "and starts past the one that went");
    }

    /// Failure: past the cap, a command is parked anyway (no bound) or its ticket is kept and stalls the lane. One past the cap is refused, its ticket
    /// passes as it's dropped, and the parked one still starts.
    #[test]
    fn one_past_the_cap_is_refused_and_its_ticket_passes() {
        let queue = Arc::new(CoreQueue::default());
        queue.lane().set_cap(1);
        let started = Started::new();
        assert_eq!(ask(&queue, &started, "highlight", 1), "now");
        assert_eq!(ask(&queue, &started, "undo", 1), "parked");
        assert_eq!(ask(&queue, &started, "redo", 1), "full");
        started.pass("highlight");
        started.pass("undo");
        assert_eq!(ask(&queue, &started, "next", 1), "now", "the refused one's ticket passed");
    }

    /// Failure: a close asks the person for an edit of another tab still landing, or not for its own (the coordinator's Q4, 2026-11-14). Only an
    /// outstanding ticket naming the tab counts, and only until it passes.
    #[test]
    fn an_edit_is_landing_on_the_tab_it_names_until_it_passes() {
        let queue = Arc::new(CoreQueue::default());
        let started = Started::new();
        assert_eq!(ask(&queue, &started, "highlight", 3), "now");
        assert_eq!(ask(&queue, &started, "undo", 4), "parked");
        assert!(queue.lane().naming(3) && queue.lane().naming(4) && !queue.lane().naming(5) && !queue.lane().naming(0));
        started.pass("highlight");
        assert!(!queue.lane().naming(3), "landed");
        started.pass("undo");
        assert!(!queue.lane().naming(4));
    }
}
