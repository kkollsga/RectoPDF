//! **The app's life as the core keeps it** ([`Lifecycle`]): the pure machine ([`crate::lifecycle::Life`]) under its lock, the clock it steps by, and what the
//! window's and the session's threads count and note beside it.

use super::updates;
use crate::lock;
use recto_session::TabId;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// **Where the app is in its life:** launch, the state's coming in, the window going up, the person's last touch, and a hand-off with what it waits for. What step
/// R1.4 makes a state machine on the clock.
pub struct Lifecycle {
    /// What the host knows of this copy, for the thread that reads the state, held back while the record's lock hasn't come ([`updates::LaunchFacts`]).
    pub update_facts: updates::LaunchFacts,
    /// What the bridge's own readings go by: the first frame's time, its show patience and the contents' follow. The core's go by [`Self::clock`].
    pub started: Instant,
    /// The host's clock, a test's fake one: what the idle time and the window's patience for input go by (step R1.4).
    pub(crate) clock: Arc<dyn crate::clock::Clock>,
    /// **The state's coming in, the window's patience for it, what's shown and a hand-off's freeze, as one state machine** ([`crate::lifecycle::Life`]). The lock is
    /// held only inside [`Self::step`] and the queries beside it, for a copy or a transition of a few bytes: never across carrying out an `Out`, a listener call, a
    /// queue push, another lock or any I/O. So it's taken on the UI thread too, bounded. The time since input isn't in it: what the UI thread sets on every event
    /// is an atomic store ([`Self::last_input_ms`]), passed to what asks as a value.
    pub(super) life: Mutex<crate::lifecycle::Life>,
    /// Reading the file a restored active tab is on. Nothing's drawn until it's looked at, so the wrong thing can't show.
    pub holding_tab: Mutex<Option<TabId>>,
    /// In a QA run, files being read wait here, as they would for a slow disk.
    pub(crate) qa_hold_reads: AtomicBool,
    /// Files being read now, which a hand-off waits for.
    pub(crate) reads_running: AtomicU32,
    /// The one pass over the other tabs' files after a restore, while it's under way: what a test waits for, as nothing else does.
    pub(crate) others_looking: AtomicU32,
    /// A person's commands on threads of their own (`person::submit`), to the end of what came of them: what a test waits for. The host counts them for a
    /// hand-off as it counts any command.
    pub(crate) persons_running: AtomicU32,
    /// Sheets and panels over the window now, which a hand-off waits for.
    pub sheets: AtomicU32,
    /// When the user last touched the app, in milliseconds of `clock`. One number, set by what the event monitor calls, with no timer.
    pub(crate) last_input_ms: AtomicU64,
    /// The person has touched the app since the launch: a click, a key, a scroll or a pinch, which moving or resizing the window begins with. Not the pointer only
    /// passing over it.
    pub(super) touched: AtomicBool,
    /// What the window's first showing had on it.
    pub first_shown: Mutex<Option<Value>>,
    /// When the window was last put up, for how long the patience had run when it was shown.
    pub window_opened_at: Mutex<Option<Instant>>,
    /// Whether the last yes `ready_to_show` gave was the cap's, for the record to say so.
    pub shown_by_cap: AtomicBool,
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
            first_shown: Mutex::new(None),
            window_opened_at: Mutex::new(None),
            shown_by_cap: AtomicBool::new(false),
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
    #[must_use]
    pub fn step(&self, event: crate::lifecycle::Event) -> Option<crate::lifecycle::Out> {
        let now = self.now_ms();
        lock(&self.life).step(event, now)
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

    /// How long until the next try at the record's lock, while a launch that didn't get it tries again.
    pub(super) fn keeping_due(&self) -> Option<Duration> {
        let now = self.now_ms();
        self.life().keeping_due(now)
    }

    /// **A late try at the record's lock, back,** taken in with whether something of the person's is `busy` (a command under way on its own thread), looked at
    /// under this lock only for a try that got the lock while it's trying, as `person::submit` counts and then looks at the machine: a lock it got is then given
    /// back. `busy` is a load of two atomics: nothing under this lock touches the disk.
    pub(super) fn tried(&self, got: bool, busy: impl FnOnce() -> bool) -> Option<crate::lifecycle::Out> {
        let now = self.now_ms();
        let mut life = lock(&self.life);
        let busy = got && life.keeping().trying() && busy();
        life.step(crate::lifecycle::Event::Keeping(crate::lifecycle::keeping::Event::Tried { got, busy }), now)
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

    /// Whether the host's been told to quit for a hand-off that's written.
    pub(crate) fn told_to_quit(&self) -> bool {
        self.life().hand_off() == crate::lifecycle::HandOff::Quitting
    }
}
