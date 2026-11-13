//! **Holds on a page view, for QA:** a first frame kept to be looked at, and a busy page view made to last, so a test can force an ordering the window
//! meets only now and then.

use super::{Engine, Shared, lock};
#[cfg(any(test, feature = "qa"))]
use std::sync::atomic::Ordering;

impl Shared {
    /// Whether QA holds this view busy ([`Engine::hold_busy`]). Never, in a build without the `qa` feature.
    #[inline]
    pub(super) fn held_busy(&self) -> bool {
        #[cfg(any(test, feature = "qa"))]
        {
            self.qa_busy.load(Ordering::Relaxed)
        }
        #[cfg(not(any(test, feature = "qa")))]
        {
            false
        }
    }
}

impl Engine {
    /// For QA: nothing's drawn till this is let go, so a first frame stays to be looked at.
    pub fn hold_drawing(&self, on: bool) {
        lock(&self.shared.reader).work.hold_drawing(on);
        self.unpark_thread();
    }

    /// **For QA: a busy page view, made to last.** While on, what the UI thread asks of this view from another pane (synced scroll's place, the one zoom,
    /// a selection taken elsewhere) is left pending as though its own thread held its lock, and that thread leaves it there as though it hadn't come round:
    /// what a pane's thread does for as long as a drawing job takes when the UI thread's call met it holding the lock. A read of the view still counts what's
    /// pending ([`Self::places`]). Let go, the thread takes it.
    #[cfg(any(test, feature = "qa"))]
    pub fn hold_busy(&self, on: bool) {
        self.shared.qa_busy.store(on, Ordering::SeqCst);
        if !on {
            self.unpark_thread();
        }
    }

    /// For QA: whether synced scroll's place is left pending for this view's own thread, so a test that forces the busy ordering can see that the follow
    /// really was left and the round didn't quietly become the free one.
    #[cfg(any(test, feature = "qa"))]
    pub fn follow_is_pending(&self) -> bool {
        lock(&self.shared.pending).follow.is_some()
    }
}
