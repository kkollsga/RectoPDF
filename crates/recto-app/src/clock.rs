//! **The clock the app's own rules go by,** passed in so a rule that depends on time is tested on a fake one in milliseconds, not by waiting (restructuring step R1.2, rule 3 of the
//! development-speed review: "Time and order are inputs").
//!
//! A clock counts milliseconds from its own start, as the marks that fade did with an `Instant` of their own. Reading it is an atomic load or a system call: no lock, no allocation.

use std::time::Instant;

/// Milliseconds since the clock started. Never goes back.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The real clock: milliseconds since it was made.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    #[must_use]
    pub fn new() -> Self {
        Self { started: Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
