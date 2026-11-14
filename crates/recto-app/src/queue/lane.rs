//! **The person's lane: their edits take effect in the order they asked for them** (the ⌃⌘H-then-⌘Z fault, 2026-11-14). A person's command that may wait
//! (a highlight reading its pages, a save, a print) runs on a thread of its own, and the rest on the session thread's queue; with nothing more, an Undo
//! pressed while a highlight was being read ran first and undid the step before it. Each of the person's ordered commands takes a ticket as it's asked, and
//! acts on the document only when **every ticket before it has passed or names another tab**: passed, that is, its command has fixed the state it acts on
//! (a highlight landed, a print's pages read, a save begun) or has gone without doing anything.
//!
//! **What a ticket names.** A command that runs on a thread of its own names its tab as it's asked; one for the session thread that says "the tab showing"
//! is named when that thread takes it, as the tab it is then. Until it's named a ticket holds everything after it, which lasts only until the session thread
//! gets to it. So an edit waits only for edits of its own tab asked before it (review of the lane, D2): a highlight's long read in one tab holds up nothing
//! in another.
//!
//! **Nothing waits.** A command whose turn hasn't come is parked here, with no thread of its own, and started by whoever passes or names the ticket that held
//! it, as that one does ([`Turn`]'s drop, [`Lane::name`]): the thread of the command before as it ends, or the session thread. One small lane for the app,
//! not one per tab: an idle lane holds no buffer, and at most [`CAP`] are parked in all.
//!
//! What's ordered, and how each command is started, is the person's commands' own (`core::person_lane`); this holds only the order.

use crate::lock;
use std::collections::VecDeque;
use std::sync::{Mutex, Weak};

/// How many of the person's commands can be parked at once, over every tab. Reached only by a key held down while a command waits: the next is refused,
/// and said so.
pub const CAP: usize = 64;

/// What a parked command does once its turn comes, given its turn to hold until it passes.
pub type Start = Box<dyn FnOnce(Turn) + Send>;

/// A ticket's tab before it's named: it holds every ticket after it.
const UNNAMED: u32 = 0;

/// The lane: the tickets not yet passed, and the commands parked for their turn.
pub struct Lane {
    state: Mutex<State>,
}

/// A ticket not yet passed.
#[derive(Clone, Copy)]
struct Outstanding {
    ticket: u64,
    /// The tab its command acts on, or [`UNNAMED`] till that's known.
    tab: u32,
    /// Its command can change the document when it lands, after the session has published what it holds: a close asks the person meanwhile.
    landing: bool,
}

struct State {
    /// The next ticket to be handed out.
    next: u64,
    /// Tickets not yet passed, in the order they were handed out.
    outstanding: VecDeque<Outstanding>,
    /// Commands waiting for their turn, in the order of their tickets, each with the way back to the queue its turn will hold.
    parked: VecDeque<(u64, Weak<super::CoreQueue>, Start)>,
    /// [`CAP`], or a test's smaller one.
    cap: usize,
}

impl State {
    /// **The lane's rule:** whether `ticket` may act now: every ticket before it has passed or names another tab (and it names its own).
    fn may_run(&self, ticket: u64) -> bool {
        let Some(own) = self.outstanding.iter().find(|outstanding| outstanding.ticket == ticket) else {
            return true;
        };
        self.outstanding
            .iter()
            .take_while(|outstanding| outstanding.ticket < ticket)
            .all(|before| before.tab != UNNAMED && own.tab != UNNAMED && before.tab != own.tab)
    }

    /// The first parked command that may act now, taken out.
    fn runnable(&mut self) -> Option<(u64, Weak<super::CoreQueue>, Start)> {
        let at = self.parked.iter().position(|(ticket, ..)| self.may_run(*ticket))?;
        self.parked.remove(at)
    }
}

impl Default for Lane {
    /// Made with room for a few tickets and its lock taken once, so the window's thread allocates nothing for the person's first edits (a ticket each,
    /// held only while its command is on its way): one small buffer for the app, not one for anything idle.
    fn default() -> Self {
        let lane = Self {
            state: Mutex::new(State {
                next: 0,
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
    /// Parked, to be started by whoever passes or names the ticket that holds it.
    Parked,
    /// Too many parked already: not taken. Its turn passes as it's dropped.
    Full(Start, Turn),
}

/// **A ticket in the lane, held by its command until the state it acts on is fixed:** passed as it's dropped, however the command ends (done, refused,
/// dropped by the end, a panic), and whatever it held up started.
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
    /// A ticket for a command just asked, naming `tab` ([`UNNAMED`], 0, till it's known), and whether it's an edit that lands later.
    pub(super) fn take(&self, queue: Weak<super::CoreQueue>, tab: u32, landing: bool) -> Turn {
        let mut state = lock(&self.state);
        let ticket = state.next;
        state.next += 1;
        state.outstanding.push_back(Outstanding { ticket, tab, landing });
        Turn { queue: Some(queue), ticket }
    }

    /// **`turn`'s command acts on `tab`,** known now (the session thread has taken it): whatever its being unnamed held up, in other tabs, starts.
    pub fn name(&self, turn: &Turn, tab: u32) {
        if let Some(outstanding) = lock(&self.state).outstanding.iter_mut().find(|outstanding| outstanding.ticket == turn.ticket) {
            outstanding.tab = tab;
        }
        self.start_runnable();
    }

    /// Whether `turn`'s tab is named yet.
    pub fn is_named(&self, turn: &Turn) -> bool {
        lock(&self.state)
            .outstanding
            .iter()
            .any(|outstanding| outstanding.ticket == turn.ticket && outstanding.tab != UNNAMED)
    }

    /// **The command holding `turn` asks to act now:** it may if nothing before it holds it, and is parked otherwise, to be started with its turn when
    /// that's so.
    pub fn ask(&self, mut turn: Turn, start: Start) -> Asked {
        let mut state = lock(&self.state);
        if state.may_run(turn.ticket) {
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

    /// Whether `turn`'s command may act now ([`State::may_run`]).
    pub fn may_run(&self, turn: &Turn) -> bool {
        lock(&self.state).may_run(turn.ticket)
    }

    /// The ticket passed: whatever it held up starts (outside the lane's lock).
    fn pass(&self, ticket: u64) {
        lock(&self.state).outstanding.retain(|outstanding| outstanding.ticket != ticket);
        self.start_runnable();
    }

    /// Every parked command that may act now, started, one at a time outside the lane's lock (a start may pass a ticket of its own).
    fn start_runnable(&self) {
        loop {
            let Some((ticket, queue, start)) = lock(&self.state).runnable() else { return };
            start(Turn { queue: Some(queue), ticket });
        }
    }

    /// **What's parked goes, all of it together,** with what it holds (the app's count of it): at the queue's end (a headless app's kill), as the queue
    /// takes nothing more; and as a quit's patience ends with it still parked, so no later edit is had without an earlier one (review of the lane, D4).
    /// What's under way goes on; what was parked behind it never starts.
    pub fn drop_parked(&self) {
        let parked = {
            let mut state = lock(&self.state);
            let parked = std::mem::take(&mut state.parked);
            state
                .outstanding
                .retain(|outstanding| !parked.iter().any(|(ticket, ..)| *ticket == outstanding.ticket));
            parked
        };
        drop(parked);
    }

    /// How many commands are parked now.
    pub fn parked(&self) -> usize {
        lock(&self.state).parked.len()
    }

    /// **Whether an edit of `tab` the person asked for is still landing:** one that can change the document (a highlight or its removal), in flight or
    /// parked. Not an undo or a turn waiting for the session thread, a print, or a close.
    pub fn landing(&self, tab: u32) -> bool {
        tab != UNNAMED
            && lock(&self.state)
                .outstanding
                .iter()
                .any(|outstanding| outstanding.landing && outstanding.tab == tab)
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
        match queue.lane().ask(queue.take_turn(tab, true), started.start(name)) {
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
        assert_eq!(ask(&queue, &started, "redo", 1), "parked");
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
        let dropped = queue.take_turn(1, true);
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

    /// Failure: a close asks the person for an edit of another tab still landing, or not for its own (the coordinator's Q4, 2026-11-14); or for a
    /// command that changes nothing after it lands (an undo waiting for the session thread, a print: review of the lane, D1). Only an outstanding ticket
    /// of an edit that lands later naming the tab counts, and only until it passes.
    #[test]
    fn an_edit_is_landing_on_the_tab_it_names_until_it_passes() {
        let queue = Arc::new(CoreQueue::default());
        let started = Started::new();
        assert_eq!(ask(&queue, &started, "highlight", 3), "now");
        assert_eq!(ask(&queue, &started, "removal", 4), "now");
        let undo = queue.take_turn(5, false);
        assert!(queue.lane().landing(3) && queue.lane().landing(4) && !queue.lane().landing(6) && !queue.lane().landing(0));
        assert!(!queue.lane().landing(5), "an undo isn't an edit landing");
        started.pass("highlight");
        assert!(!queue.lane().landing(3), "landed");
        started.pass("removal");
        assert!(!queue.lane().landing(4));
        drop(undo);
    }

    /// Failure (review of the lane, D2): an edit waits for another tab's, so a highlight's long read in one tab parks every edit in the others; or one of
    /// the same tab goes before an edit asked ahead of it. Each waits only for the tickets before it of its own tab, and for none not yet named.
    #[test]
    fn an_edit_waits_only_for_its_own_tabs() {
        let queue = Arc::new(CoreQueue::default());
        let started = Started::new();
        assert_eq!(ask(&queue, &started, "highlight in 1", 1), "now");
        assert_eq!(ask(&queue, &started, "undo in 2", 2), "now", "another tab: at once");
        assert_eq!(ask(&queue, &started, "undo in 1", 1), "parked", "its own tab's highlight first");
        assert_eq!(ask(&queue, &started, "turn in 2", 2), "parked", "its own tab's undo first");
        started.pass("undo in 2");
        assert_eq!(
            started.order(),
            ["highlight in 1", "undo in 2", "turn in 2"],
            "started as its tab's last passed"
        );
        started.pass("highlight in 1");
        assert_eq!(started.order(), ["highlight in 1", "undo in 2", "turn in 2", "undo in 1"]);
        started.pass("undo in 1");
        started.pass("turn in 2");
        assert_eq!(queue.lane().parked(), 0);
    }

    /// Failure (D2): a ticket whose tab isn't known yet (an undo of "the tab showing", before the session thread takes it) lets an edit asked after it go
    /// first in what may be the same tab; or, named, still holds up another tab's. It holds every ticket after it till it's named, and then only its tab's.
    #[test]
    fn a_ticket_not_yet_named_holds_everything_after_it_till_it_is() {
        let queue = Arc::new(CoreQueue::default());
        let started = Started::new();
        let undo = queue.take_turn(0, false);
        assert_eq!(ask(&queue, &started, "highlight in 2", 2), "parked", "the undo may be of tab 2");
        assert_eq!(ask(&queue, &started, "highlight in 1", 1), "parked");
        queue.lane().name(&undo, 1);
        assert_eq!(started.order(), ["highlight in 2"], "named tab 1: tab 2's goes, tab 1's waits");
        drop(undo);
        assert_eq!(started.order(), ["highlight in 2", "highlight in 1"]);
        started.pass("highlight in 1");
        started.pass("highlight in 2");
    }
}
