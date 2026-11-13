//! **Holds on a page view, for QA:** a first frame kept to be looked at, a busy page view made to last, and an ask held between when it was asked and
//! when it takes the lock, so a test can force an ordering the window meets only now and then.

use super::{Engine, Shared, lock};
#[cfg(any(test, feature = "qa"))]
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
#[cfg(any(test, feature = "qa"))]
use std::sync::{Condvar, Mutex, PoisonError};
#[cfg(any(test, feature = "qa"))]
use std::time::{Duration, Instant};

/// What's asked of a page view that goes by when it was asked ([`super::asked`]), each of which QA can hold between its tick and the lock
/// ([`Qa::hold_asks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// [`Engine::jump_to`] and [`Engine::jump_within`].
    Jump,
    /// [`Engine::follow_soon`].
    Follow,
    /// [`Engine::zoom`].
    Zoom,
    /// [`Engine::zoom_soon`].
    ZoomSoon,
    /// [`Engine::activate`].
    Activate,
}

/// The holds themselves: only with the `qa` feature (or in this crate's tests), so a release app has no such field.
#[cfg(any(test, feature = "qa"))]
#[derive(Default)]
pub(super) struct QaHolds {
    /// [`Qa::hold_busy`].
    busy: AtomicBool,
    /// [`Qa::hold_asks`]: a bit for each [`Ask`] held, looked at first so an ask not held takes no lock.
    asks_held: AtomicU8,
    /// How many asks wait at the hold, for [`Qa::wait_for_held_asks`].
    waiting: Mutex<u32>,
    changed: Condvar,
}

impl Shared {
    /// Whether QA holds this view busy ([`Qa::hold_busy`]). Never, in a build without the `qa` feature.
    #[inline]
    pub(super) fn held_busy(&self) -> bool {
        #[cfg(any(test, feature = "qa"))]
        {
            self.qa.busy.load(Ordering::Relaxed)
        }
        #[cfg(not(any(test, feature = "qa")))]
        {
            false
        }
    }

    /// Between an ask's tick and the lock: waits here while QA holds this kind of ask ([`Qa::hold_asks`]). Nothing, in a build without the `qa` feature.
    #[inline]
    pub(super) fn after_asking(&self, ask: Ask) {
        #[cfg(any(test, feature = "qa"))]
        {
            let bit = 1u8 << ask as u8;
            if self.qa.asks_held.load(Ordering::SeqCst) & bit == 0 {
                return;
            }
            let mut waiting = self.qa.waiting.lock().unwrap_or_else(PoisonError::into_inner);
            *waiting += 1;
            self.qa.changed.notify_all();
            while self.qa.asks_held.load(Ordering::SeqCst) & bit != 0 {
                waiting = self.qa.changed.wait(waiting).unwrap_or_else(PoisonError::into_inner);
            }
            *waiting -= 1;
        }
        #[cfg(not(any(test, feature = "qa")))]
        {
            let _ = ask;
        }
    }
}

impl Engine {
    /// For QA: nothing's drawn till this is let go, so a first frame stays to be looked at.
    pub fn hold_drawing(&self, on: bool) {
        lock(&self.shared.reader).work.hold_drawing(on);
        self.unpark_thread();
    }

    /// **QA's seams that force an ordering** (the `qa` feature only: a release app doesn't contain them).
    #[cfg(any(test, feature = "qa"))]
    #[inline]
    pub fn qa(&self) -> Qa<'_> {
        Qa { engine: self }
    }
}

/// The seams of a page view for QA, a facade borrowed from [`Engine::qa`], as the others are: its own methods, so [`Engine`] has two the fewer.
#[cfg(any(test, feature = "qa"))]
#[derive(Clone, Copy)]
pub struct Qa<'a> {
    engine: &'a Engine,
}

#[cfg(any(test, feature = "qa"))]
impl Qa<'_> {
    /// **A busy page view, made to last.** While on, what the UI thread asks of this view from another pane (synced scroll's place, the one zoom,
    /// a selection taken elsewhere) is left pending as though its own thread held its lock, and that thread leaves it there as though it hadn't come round:
    /// what a pane's thread does for as long as a drawing job takes when the UI thread's call met it holding the lock. A read of the view still counts what's
    /// pending ([`Engine::places`]). Let go, the thread takes it.
    pub fn hold_busy(&self, on: bool) {
        self.engine.shared.qa.busy.store(on, Ordering::SeqCst);
        if !on {
            self.engine.unpark_thread();
        }
    }

    /// Whether synced scroll's place is left pending for this view's own thread, so a test that forces the busy ordering can see that the follow really
    /// was left and the round didn't quietly become the free one.
    pub fn follow_is_pending(&self) -> bool {
        lock(&self.engine.shared.pending).follow.is_some()
    }

    /// **An ask held after it was asked and before it takes the lock,** as a thread preempted there, or waiting on the lock while another thread has it,
    /// holds it: while on, each `ask` of this view, from any thread, takes its tick of when it was asked and then waits here. Let go, each goes on.
    pub fn hold_asks(&self, ask: Ask, on: bool) {
        let qa = &self.engine.shared.qa;
        let bit = 1u8 << ask as u8;
        let _waiting = qa.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        if on {
            qa.asks_held.fetch_or(bit, Ordering::SeqCst);
        } else {
            qa.asks_held.fetch_and(!bit, Ordering::SeqCst);
        }
        qa.changed.notify_all();
    }

    /// Waits, at most 10 s, until `count` asks have taken their tick and wait at the hold ([`Self::hold_asks`]). Says whether they did.
    pub fn wait_for_held_asks(&self, count: u32) -> bool {
        let qa = &self.engine.shared.qa;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut waiting = qa.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        while *waiting < count {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            waiting = qa.changed.wait_timeout(waiting, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }
}
