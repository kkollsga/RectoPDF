//! **Fakes of what the app core is given by its host,** for tests here and in other crates (the `testkit` feature): a clock that moves only when told, and, as the seams grow, a
//! listener that counts what it heard. The headless app of step R1.3 is built from these.

use crate::clock::Clock;
use std::sync::atomic::{AtomicU64, Ordering};

/// A clock that stands still until a test moves it.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    #[must_use]
    pub fn at(ms: u64) -> Self {
        Self { now: AtomicU64::new(ms) }
    }

    /// Moves it on by `ms`, saturating.
    pub fn advance(&self, ms: u64) {
        let _ = self.now.try_update(Ordering::SeqCst, Ordering::SeqCst, |now| Some(now.saturating_add(ms)));
    }

    /// Sets it to `ms`. A test that sets it back is testing a clock that goes back, which the real one never does.
    pub fn set(&self, ms: u64) {
        self.now.store(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}
