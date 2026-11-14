//! **The app's life as the core keeps it** ([`Lifecycle`]): the pure machine ([`crate::lifecycle::Life`]) under its lock, the clock it steps by, and what the
//! window's and the session's threads count and note beside it.

use super::{SHOW_PATIENCE, showing, updates};
use crate::lock;
use recto_agent::DrawableLoader;
use recto_control::host::SessionHost;
use recto_session::TabId;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a keeping step comes to, carried out by the machine's one function: an `Out` that isn't keeping's carries nothing.
fn carry(out: Option<crate::lifecycle::Out>, to: &mut super::keeping::Carrier<'_>) {
    let out = match out {
        Some(crate::lifecycle::Out::Keeping(out)) => Some(out),
        _ => None,
    };
    crate::lifecycle::keeping::carry(out, to);
}

/// **The window's putting up and its first showing,** as the host says them: opened, shown, and gone (closed, or for another made in its place). The host
/// shows a window when its page checks pass or its patience runs out, and gives the keyboard back to what had it only then (Find's field after an update):
/// so a window owed its showing isn't at rest, and `qa wait-settled` waits for it (CI run 43539671770: a hand-off's report was read with the window still
/// waiting to be shown, and Find's field, whose focus comes with the showing, without the keyboard). A view of two of [`Lifecycle`]'s fields and its start,
/// so the app's state is no bigger for it: when the window was put up, what its page checks said when asked and the record of its first showing, under
/// one short lock, and what's owed of it in one atomic.
pub(crate) struct WindowShowing<'a> {
    showing: &'a Mutex<Showing>,
    flags: &'a AtomicU8,
    started: &'a Instant,
}

/// **The window's showing, under one lock:** when it was last put up, what its page checks said each time the host asked before it was shown
/// ([`showing::Asks`]), and the record of its first showing. One lock for the three, where the record and the time had one each: what the asks need is no
/// more room in the app's state.
#[derive(Default)]
struct Showing {
    /// When the window was last put up, in microseconds since the process started, plus one: 0 before any has been.
    opened_at: u64,
    asks: showing::Asks,
    /// What the window's first showing had on it ([`Lifecycle::record_window_shown`]).
    record: Option<Value>,
}

impl Showing {
    /// How long since the window was last put up, or nothing before one has been.
    fn since_opened(&self, started: &Instant) -> Option<Duration> {
        let at = self.opened_at.checked_sub(1)?;
        Some(started.elapsed().saturating_sub(Duration::from_micros(at)))
    }
}

/// Put up and not yet shown, nor gone.
const OWED: u8 = 1;
/// The last yes the page checks gave was the cap's, for the record to say so.
const BY_CAP: u8 = 2;

impl WindowShowing<'_> {
    /// The window was put up now: what its showing's patience is counted from. It's owed its showing from here, and its asks start again.
    pub(crate) fn opened(&self) {
        {
            let mut showing = lock(self.showing);
            showing.opened_at = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX - 1) + 1;
            showing.asks = showing::Asks::default();
        }
        self.flags.store(OWED, Ordering::SeqCst);
    }

    /// The window went, closed or for another made in its place: it's owed no showing, as the host's timer for it goes with it. When it was opened stays,
    /// for the backstop's count.
    pub(crate) fn gone(&self) {
        self.flags.fetch_and(!OWED, Ordering::SeqCst);
    }

    /// How long since the window was last put up, or nothing before one has been.
    fn since_opened(&self) -> Option<Duration> {
        lock(self.showing).since_opened(self.started)
    }

    /// **The page checks asked:** what the rule says of them now ([`showing::decide`]), kept with them for the record, and whether a yes was the cap's.
    fn ask(&self, checks: showing::PageChecks) -> showing::Show {
        let decision = {
            let mut showing = lock(self.showing);
            let since = showing.since_opened(self.started);
            let decision = showing::decide(since, self.started.elapsed(), checks);
            showing.asks.note(since, checks, decision);
            decision
        };
        if decision == showing::Show::Cap {
            self.flags.fetch_or(BY_CAP, Ordering::SeqCst);
        } else {
            self.flags.fetch_and(!BY_CAP, Ordering::SeqCst);
        }
        decision
    }

    /// The window's been shown: it's owed nothing. How long since it was put up, whether the last yes it was given was the cap's, and its asks.
    fn shown(&self) -> (Option<Duration>, bool, showing::Asks) {
        let flags = self.flags.fetch_and(!OWED, Ordering::SeqCst);
        let showing = lock(self.showing);
        (showing.since_opened(self.started), flags & BY_CAP != 0, showing.asks)
    }

    /// **How long the window has been up and not yet shown,** or nothing when it has been, or there's none.
    pub(crate) fn not_yet_shown(&self) -> Option<Duration> {
        (self.flags.load(Ordering::SeqCst) & OWED != 0).then(|| self.since_opened().unwrap_or_default())
    }
}

/// **Where the app is in its life:** launch, the state's coming in, the window going up, the person's last touch, and a hand-off with what it waits for. What step
/// R1.4 makes a state machine on the clock. Its fields are its own (but the launch facts, which are a part with private fields of their own): what other code
/// counts, notes or asks of them is a method here, each an atomic or a short lock of its own field.
pub struct Lifecycle {
    /// What the host knows of this copy, for the thread that reads the state, held back while the record's lock hasn't come ([`updates::LaunchFacts`]).
    pub update_facts: updates::LaunchFacts,
    /// What the bridge's own readings go by: the first frame's time, its show patience and the contents' follow. The core's go by [`Self::clock`].
    started: Instant,
    /// The host's clock, a test's fake one: what the idle time and the window's patience for input go by (step R1.4).
    clock: Arc<dyn crate::clock::Clock>,
    /// **The state's coming in, the window's patience for it, what's shown and a hand-off's freeze, as one state machine** ([`crate::lifecycle::Life`]). The lock is
    /// held only inside [`Self::step`], [`Self::step_keeping`], [`Self::tried`] and the queries beside it, for a copy or a transition of a few bytes: never
    /// across carrying out an `Out`, a listener call, a queue push, another lock or any I/O. So it's taken on the UI thread too, bounded. The time since input
    /// isn't in it: what the UI thread sets on every event is an atomic store ([`Self::last_input_ms`]), passed to what asks as a value. Private: only
    /// these methods step it.
    life: Mutex<crate::lifecycle::Life>,
    /// Reading the file a restored active tab is on. Nothing's drawn until it's looked at, so the wrong thing can't show.
    holding_tab: Mutex<Option<TabId>>,
    /// In a QA run, files being read wait here, as they would for a slow disk.
    qa_hold_reads: AtomicBool,
    /// Files being read now, which a hand-off waits for.
    reads_running: AtomicU32,
    /// The one pass over the other tabs' files after a restore, while it's under way: what a test waits for, as nothing else does.
    others_looking: AtomicU32,
    /// A person's commands on threads of their own (`person::submit`), to the end of what came of them: what a test waits for. The host counts them for a
    /// hand-off as it counts any command.
    persons_running: AtomicU32,
    /// Sheets and panels over the window now, which a hand-off waits for.
    sheets: AtomicU32,
    /// When the user last touched the app, in milliseconds of `clock`. One number, set by what the event monitor calls, with no timer.
    last_input_ms: AtomicU64,
    /// The person has touched the app since the launch: a click, a key, a scroll or a pinch, which moving or resizing the window begins with. Not the pointer only
    /// passing over it.
    touched: AtomicBool,
    /// When the window was last put up, for how long the patience had run when it was shown, what its page checks said when asked, and what its first
    /// showing had on it ([`WindowShowing`]).
    window_showing: Mutex<Showing>,
    /// Whether the window's owed its showing, and whether the last yes `ready_to_show` gave was the cap's ([`WindowShowing`]).
    window_flags: AtomicU8,
}

// `new` is public for the bridge, which builds the app; launch is no default state.
#[allow(clippy::new_without_default)]
impl Lifecycle {
    /// At launch: nothing read, nothing shown, no hand-off.
    pub fn new(clock: Arc<dyn crate::clock::Clock>) -> Self {
        Self {
            started: Instant::now(),
            clock,
            life: Mutex::new(crate::lifecycle::Life::default()),
            holding_tab: Mutex::new(None),
            window_showing: Mutex::new(Showing::default()),
            window_flags: AtomicU8::new(0),
            last_input_ms: AtomicU64::new(0),
            touched: AtomicBool::new(false),
            update_facts: updates::LaunchFacts::new(),
            reads_running: AtomicU32::new(0),
            persons_running: AtomicU32::new(0),
            others_looking: AtomicU32::new(0),
            sheets: AtomicU32::new(0),
            qa_hold_reads: AtomicBool::new(false),
        }
    }

    /// Now, by the clock, in its milliseconds.
    pub(crate) fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// **What happens to the app's life,** now by the clock. The lock's let go of before this returns: the caller carries out what it comes to.
    ///
    /// **Not keeping the record:** a keeping event is stepped only by the keeping desk, which carries out what it comes to ([`Self::step_keeping`]). One sent
    /// here is a mistake a debug build stops at; a release build steps nothing and says nothing came of it, so the machine is as it was and there's nothing
    /// left to carry out: no lock taken or given back, nothing settled untold.
    #[must_use]
    pub fn step(&self, event: crate::lifecycle::Event) -> Option<crate::lifecycle::Out> {
        debug_assert!(
            !matches!(event, crate::lifecycle::Event::Keeping(_)),
            "a keeping event stepped outside the keeping desk: {event:?}"
        );
        if let crate::lifecycle::Event::Keeping(_) = event {
            return None;
        }
        let now = self.now_ms();
        lock(&self.life).step(event, now)
    }

    /// **A step of keeping the record, and what it comes to carried out** ([`crate::lifecycle::keeping::carry`]) once the life's lock is let go. It takes the
    /// desk's [`super::keeping::Carrier`], which only the desk can make, so no other code steps keeping the record and none steps it without carrying it out.
    pub(super) fn step_keeping(&self, event: crate::lifecycle::keeping::Event, to: &mut super::keeping::Carrier<'_>) {
        let now = self.now_ms();
        let out = lock(&self.life).step(crate::lifecycle::Event::Keeping(event), now);
        carry(out, to);
    }

    /// The app's life as it stands, a copy.
    pub(super) fn life(&self) -> crate::lifecycle::Life {
        *lock(&self.life)
    }

    /// Whether the state's been read and taken in.
    pub fn state_read(&self) -> bool {
        self.life().restored()
    }

    /// Whether what the host shows has been made from the state.
    pub(crate) fn shown_with_state(&self) -> bool {
        self.life().shown() == crate::lifecycle::Shown::WithState
    }

    /// Whether the window went up without the state and was made again from it.
    pub(crate) fn remade(&self) -> bool {
        self.life().remade()
    }

    /// Whether the window takes what a person does: once it's been made from the state, or once the state's been waited for long enough. Until then it's shown
    /// and takes nothing, as it's made again from the state when that comes, and what was done to it would go with it. The UI thread asks: the life's lock,
    /// for a copy.
    pub fn window_interactive(&self) -> bool {
        let now = self.now_ms();
        self.life().takes_input(now)
    }

    /// How long until the patience for the state is over by the session thread's own deadline, if it waits on one.
    pub(crate) fn due(&self) -> Option<Duration> {
        let now = self.now_ms();
        self.life().due(now)
    }

    /// Whether this copy keeps the record, or is trying again for its lock ([`crate::lifecycle::keeping`]).
    pub(super) fn keeping(&self) -> crate::lifecycle::keeping::Keeping {
        self.life().keeping()
    }

    /// **A thread of keeping's is out:** a try at the record's lock not yet back, or a late keep's saved session being read while its restore's patience
    /// lasts, which what the person does meanwhile waits on (held on the session thread till the restore's in). What the app's settled waits for
    /// ([`crate::settled`]). Bounded in the real app: a try is one look at the lock's file, and the read is waited on for the restore's patience at most
    /// ([`crate::lifecycle::keeping::RESTORING_PATIENCE_MS`]); once that's over (`LettingGo`) nothing waits on it, and the seconds between tries are a
    /// deadline due later, never waited for. Every state says, with no default, so a new one can't be left out.
    pub(crate) fn keeping_thread_out(&self) -> bool {
        thread_out(self.keeping())
    }

    /// How long until the next try at the record's lock, while a launch that didn't get it tries again.
    pub(super) fn keeping_due(&self) -> Option<Duration> {
        let now = self.now_ms();
        self.life().keeping_due(now)
    }

    /// **A late try at the record's lock, back,** taken in with whether something of the person's is `busy` (a command under way on its own thread), looked at
    /// under this lock only for a try that got the lock while it's trying, as `person::submit` counts and then looks at the machine: a lock it got is then given
    /// back. `busy` is a load of two atomics: nothing under this lock touches the disk. What it comes to is carried out once the lock's let go, by the desk's
    /// carrier, as [`Self::step_keeping`] does.
    pub(super) fn tried(&self, got: bool, busy: impl FnOnce() -> bool, to: &mut super::keeping::Carrier<'_>) {
        let now = self.now_ms();
        let out = {
            let mut life = lock(&self.life);
            let busy = got && life.keeping().trying() && busy();
            life.step(crate::lifecycle::Event::Keeping(crate::lifecycle::keeping::Event::Tried { got, busy }), now)
        };
        carry(out, to);
    }

    /// Whether the window is made again from the state now ([`crate::lifecycle::Life::may_remake_window`]): what the host asks on the UI thread, cheap and
    /// bounded.
    pub fn may_remake_window(&self) -> bool {
        let now = self.now_ms();
        let touched = self.touched.load(Ordering::Relaxed);
        let sheet_up = self.sheets.load(Ordering::SeqCst) > 0;
        self.life().may_remake_window(now, touched, sheet_up)
    }

    /// A sheet or panel opened over the window, or closed. An update waits for it. The UI thread's: an atomic.
    pub fn sheet_changed(&self, open: bool) {
        if open {
            self.sheets.fetch_add(1, Ordering::SeqCst);
        } else {
            let _ = self.sheets.try_update(Ordering::SeqCst, Ordering::SeqCst, |count| count.checked_sub(1));
        }
    }

    /// Someone's near the app (the pointer over it), which isn't an action: the time since input starts again. The UI thread's: an atomic store.
    pub fn note_nearby(&self) {
        self.last_input_ms.store(self.now_ms(), Ordering::Relaxed);
    }

    /// The person touched the app: the time since input starts again, and the window counts as touched since the launch. The UI thread's: two atomic stores.
    pub fn note_input(&self) {
        self.last_input_ms.store(self.now_ms(), Ordering::Relaxed);
        self.touched.store(true, Ordering::Relaxed);
    }

    /// The page checks as they are now, gathered for [`showing::decide`] and the record of the window's first showing: the session's lock only tried, as the UI
    /// thread asks and doesn't wait for it, and the page view's word on its place.
    fn page_checks(&self, session: &SessionHost<DrawableLoader>, engine: &recto_reader::Engine) -> showing::PageChecks {
        let active = session.try_with_session(|session| session.tabs().active().is_some());
        showing::PageChecks {
            state_read: self.state_read(),
            holding_tab: self.holding_tab().is_some(),
            session_free: active.is_some(),
            has_tab: active.unwrap_or(false),
            on_place: engine.waking().is_on_place(),
            showable: engine.waking().is_showable(),
        }
    }

    /// Whether the window may be shown now, by [`showing::decide`] on the page checks as they are, from the session and the page view; what it said is kept
    /// for the record. The UI thread's.
    pub fn ready_to_show(&self, session: &SessionHost<DrawableLoader>, engine: &recto_reader::Engine) -> bool {
        let checks = self.page_checks(session, engine);
        self.window_showing().ask(checks) != showing::Show::Wait
    }

    /// **What the window's first showing had on it, and why it was shown.** `why` is `ready` (the page checks passed), `cap` (Rust's cap ran out: the host
    /// asked and was told yes) or `patience` (the host's timer showed it). `asks` is what the decision saw: how many times the host asked, the last ask
    /// (the deciding one for `ready` and `cap`; for `patience`, the host's ask as its timer ran out) and the last that said wait, each with its time since
    /// the window was put up, its checks and the ones that held it. The rest is the page view's report and the checks as the show's layout left them
    /// (`after_layout`, which can differ from what was decided on: the layout moves the view), gathered just after it, written once per window. The UI
    /// thread's.
    pub fn record_window_shown(&self, home: bool, tile_layers: u32, why: &str, session: &SessionHost<DrawableLoader>, engine: &recto_reader::Engine) {
        let report = engine.report().0;
        let checks = self.page_checks(session, engine);
        let (opened, by_cap, asks) = self.window_showing().shown();
        let why = if why == "ready" && by_cap { "cap" } else { why };
        lock(&self.window_showing).record = Some(json!({
            "home": home, "page": report["page"], "scroll_y": report["scroll_y"], "scroll_x": report["scroll_x"], "magnification": report["magnification"], "settled": report["settled"], "tab": report["tab"],
            "tile_layers": tile_layers, "ms_from_start": self.started.elapsed().as_secs_f64() * 1000.0,
            "why": why,
            "ms_since_window_opened": opened.map(|opened| opened.as_secs_f64() * 1000.0),
            "patience_ms": SHOW_PATIENCE.as_millis() as u64,
            "asks": { "count": asks.count, "last": asks.last.map(asked_report), "last_wait": asks.last_wait.map(asked_report) },
            "after_layout": checks_report(checks),
        }));
    }

    /// Seconds since the process started, by the instant the bridge's own readings go by (the contents' follow; not the app's clock).
    pub fn seconds_since_start(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// When the user last touched the app, or was near it, in milliseconds of the clock.
    pub(crate) fn last_input_ms(&self) -> u64 {
        self.last_input_ms.load(Ordering::Relaxed)
    }

    /// Sheets and panels over the window now.
    pub(crate) fn sheets_open(&self) -> u32 {
        self.sheets.load(Ordering::SeqCst)
    }

    /// The window's putting up and first showing ([`WindowShowing`]): what the host says of it, and what `qa wait-settled` asks.
    pub(crate) fn window_showing(&self) -> WindowShowing<'_> {
        WindowShowing {
            showing: &self.window_showing,
            flags: &self.window_flags,
            started: &self.started,
        }
    }

    /// What the window's first showing had on it, a copy taken under its short lock: the lock the UI thread takes as the window's shown, so nothing else is
    /// done holding it.
    pub(crate) fn first_shown(&self) -> Option<Value> {
        lock(&self.window_showing).record.clone()
    }

    /// The restored active tab whose file is being read, if one is: nothing's drawn of it until it's looked at.
    pub(crate) fn holding_tab(&self) -> Option<TabId> {
        *lock(&self.holding_tab)
    }

    /// The restored active tab whose file is being read, or none.
    pub(crate) fn set_holding_tab(&self, tab: Option<TabId>) {
        *lock(&self.holding_tab) = tab;
    }

    /// The tab held for its read, taken: none is held from here.
    pub(crate) fn take_holding_tab(&self) -> Option<TabId> {
        lock(&self.holding_tab).take()
    }

    /// In a QA run, whether files being read wait, as for a slow disk.
    pub(crate) fn hold_reads(&self, hold: bool) {
        self.qa_hold_reads.store(hold, Ordering::SeqCst);
    }

    /// Whether files being read wait now ([`Self::hold_reads`]).
    pub(crate) fn reads_held(&self) -> bool {
        self.qa_hold_reads.load(Ordering::SeqCst)
    }

    /// A file's read started, on a thread of its own: a hand-off waits for it until [`Self::read_finished`].
    pub(crate) fn read_started(&self) {
        self.reads_running.fetch_add(1, Ordering::SeqCst);
    }

    /// A file's read is in, or never started.
    pub(crate) fn read_finished(&self) {
        self.reads_running.fetch_sub(1, Ordering::SeqCst);
    }

    /// Files being read now.
    pub(crate) fn reads_running(&self) -> u32 {
        self.reads_running.load(Ordering::SeqCst)
    }

    /// The one pass over the other tabs' files after a restore has started, on a thread of its own, until [`Self::others_looked_at`].
    pub(crate) fn looking_at_others(&self) {
        self.others_looking.fetch_add(1, Ordering::SeqCst);
    }

    /// That pass has been round, or never started.
    pub(crate) fn others_looked_at(&self) {
        self.others_looking.fetch_sub(1, Ordering::SeqCst);
    }

    /// Whether that pass is under way: what the app's settled waits for (the kit's settle and `qa wait`, [`crate::settled`]), as nothing else does.
    pub(crate) fn others_looking(&self) -> bool {
        self.others_looking.load(Ordering::SeqCst) > 0
    }

    /// **A person's command counted as running** until the guard's dropped, however its thread ends, a panic included ([`PersonRunning`]).
    pub(crate) fn person_running(self: &Arc<Self>) -> PersonRunning {
        self.persons_running.fetch_add(1, Ordering::SeqCst);
        PersonRunning(Arc::clone(self))
    }

    /// A person's commands running on threads of their own now.
    pub(crate) fn persons_running(&self) -> u32 {
        self.persons_running.load(Ordering::SeqCst)
    }

    /// Whether the host's been told to quit for a hand-off that's written.
    pub(crate) fn told_to_quit(&self) -> bool {
        self.life().hand_off() == crate::lifecycle::HandOff::Quitting
    }
}

/// **A person's command counted as running, until this is dropped** ([`Lifecycle::person_running`]): as its thread ends, however it ends. A command that panicked
/// was counted running for good, so a hand-off waited on it to the end of its patience (review, 2026-11-13).
pub(crate) struct PersonRunning(Arc<Lifecycle>);

impl Drop for PersonRunning {
    fn drop(&mut self) {
        self.0.persons_running.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The page checks, for the record.
fn checks_report(checks: showing::PageChecks) -> Value {
    json!({ "state_read": checks.state_read, "holding_tab": checks.holding_tab, "session_free": checks.session_free, "has_tab": checks.has_tab, "on_place": checks.on_place, "showable": checks.showable })
}

/// One ask of the page checks, for the record: when, what the rule said, which checks held the window, and all of them.
fn asked_report(asked: showing::Asked) -> Value {
    json!({ "ms": f64::from(asked.micros) / 1000.0, "said": asked.said.why(), "held_by": asked.checks().held_by().collect::<Vec<_>>(), "checks": checks_report(asked.checks()) })
}

/// Whether keeping, in this state, has a thread out ([`Lifecycle::keeping_thread_out`]).
fn thread_out(keeping: crate::lifecycle::keeping::Keeping) -> bool {
    use crate::lifecycle::keeping::Keeping;
    match keeping {
        Keeping::Trying { asking, .. } | Keeping::Stopped { asking, .. } => asking,
        Keeping::Restoring { .. } => true,
        Keeping::NotAsked | Keeping::Keeps | Keeping::LettingGo | Keeping::NotKept => false,
    }
}

#[cfg(test)]
mod keeping_thread_tests {
    use crate::lifecycle::keeping::Keeping;

    /// Failure (review of `build/settled-gaps`, 2026-11-14): settled answered with a try at the record's lock out, or a late keep's read out that the
    /// person's commands were held for. Every state of keeping, each with whether a thread is out: a try that's out (trying or stopped), a restore's read
    /// (claimed or not); not one let go after its patience, and none for the rest. A state added to `Keeping` fails to compile in `thread_out` until it says.
    #[test]
    fn every_keeping_state_says_whether_a_thread_is_out() {
        let trying = |asking| Keeping::Trying {
            since_ms: 0,
            tries: 1,
            as_read: 0,
            asking,
            by_update: false,
        };
        let stopped = |asking| Keeping::Stopped {
            since_ms: 0,
            tries: 1,
            as_read: 0,
            asking,
            by_update: false,
        };
        for (state, out) in [
            (Keeping::NotAsked, false),
            (Keeping::Keeps, false),
            (trying(false), false),
            (trying(true), true),
            (stopped(false), false),
            (stopped(true), true),
            (Keeping::Restoring { since_ms: 0, claimed: false }, true),
            (Keeping::Restoring { since_ms: 0, claimed: true }, true),
            (Keeping::LettingGo, false),
            (Keeping::NotKept, false),
        ] {
            assert_eq!(super::thread_out(state), out, "{state:?}");
        }
    }
}
