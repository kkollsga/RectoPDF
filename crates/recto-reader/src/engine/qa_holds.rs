//! **Holds on a page view, for QA:** a first frame kept to be looked at, and a busy page view made to last, so a test can force an ordering the window
//! meets only now and then.

use super::{Engine, lock};
use std::sync::atomic::Ordering;

impl Engine {
    /// For QA: nothing's drawn till this is let go, so a first frame stays to be looked at.
    pub fn hold_drawing(&self, on: bool) {
        lock(&self.shared.reader).work.hold_drawing(on);
        self.wake();
    }

    /// **For QA: a busy page view, made to last.** While on, what the UI thread asks of this view from another pane (synced scroll's place, the one zoom,
    /// a selection taken elsewhere) is left pending as though its own thread held its lock, and that thread leaves it there as though it hadn't come round:
    /// what a pane's thread does for as long as a drawing job takes when the UI thread's call met it holding the lock. A read of the view still counts what's
    /// pending ([`Self::places`]). Let go, the thread takes it.
    pub fn hold_busy(&self, on: bool) {
        self.shared.qa_busy.store(on, Ordering::SeqCst);
        if !on {
            self.wake();
        }
    }
}
