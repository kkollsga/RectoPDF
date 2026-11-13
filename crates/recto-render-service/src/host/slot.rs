//! **The heavy slot:** at most one heavy worker at a time across everything sharing it, and the ticket that holds it for as long as the heavy process lives.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The one place a heavy worker may run. A viewer takes it without waiting: whoever finds it taken draws a lower-resolution version for now. An export waits for it, on its own job
/// thread, and ahead of viewers while it does. Goes free when the heavy process has really gone, however it went.
#[derive(Clone, Debug, Default)]
pub struct HeavySlot(Arc<SlotShared>);

#[derive(Debug, Default)]
struct SlotShared {
    taken: Mutex<bool>,
    freed: Condvar,
    /// Outside the lock, so a cancel with nobody waiting stays one atomic read.
    waiting: AtomicU32,
    /// Times it's been taken, for a test or QA to see who used it.
    times_taken: AtomicU64,
}

/// Why a wait for the slot ended without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotWaitEnd {
    Cancelled,
    TimedOut,
}

impl HeavySlot {
    /// A slot of its own, for a test that shouldn't meet other tests' heavy workers.
    pub fn new() -> Self {
        Self::default()
    }

    /// The session's slot, shared by every document in this process.
    pub(super) fn session() -> Self {
        static SLOT: OnceLock<HeavySlot> = OnceLock::new();
        SLOT.get_or_init(Self::new).clone()
    }

    fn taken(&self) -> MutexGuard<'_, bool> {
        self.0.taken.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_taken(&self) -> bool {
        *self.taken()
    }

    /// How many times it's been taken, by a viewer or an export.
    pub fn times_taken(&self) -> u64 {
        self.0.times_taken.load(Ordering::SeqCst)
    }

    /// How many exports are waiting for it.
    pub fn waiting(&self) -> u32 {
        self.0.waiting.load(Ordering::SeqCst)
    }

    /// For a viewer: at once or not at all, and never ahead of an export that's waiting.
    pub(super) fn try_take(&self) -> Option<HeavyTicket> {
        let mut taken = self.taken();
        if *taken || self.waiting() > 0 {
            return None;
        }
        *taken = true;
        self.0.times_taken.fetch_add(1, Ordering::SeqCst);
        Some(HeavyTicket(self.clone()))
    }

    /// For an export: sleeps until the slot frees, the time runs out, or `is_cancelled` says so. Blocks its caller, with no polling. Its place in the wait goes with it on every way out.
    pub(super) fn wait_take(&self, limit: Duration, is_cancelled: impl Fn() -> bool) -> Result<HeavyTicket, SlotWaitEnd> {
        struct Place<'a>(&'a AtomicU32);
        impl Drop for Place<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        // Counted before `is_cancelled` is read. A canceller stores first and reads this after, so at least one of the two sees the other.
        self.0.waiting.fetch_add(1, Ordering::SeqCst);
        let _place = Place(&self.0.waiting);
        let deadline = Instant::now() + limit;
        let mut taken = self.taken();
        loop {
            if is_cancelled() {
                return Err(SlotWaitEnd::Cancelled);
            }
            if !*taken {
                *taken = true;
                self.0.times_taken.fetch_add(1, Ordering::SeqCst);
                return Ok(HeavyTicket(self.clone()));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(SlotWaitEnd::TimedOut);
            }
            taken = self.0.freed.wait_timeout(taken, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }

    /// Wakes anyone waiting, so a cancel doesn't have to wait for the slot or the time to run out. Costs one atomic read if there's nobody.
    pub(super) fn wake_waiters(&self) {
        if self.waiting() > 0 {
            // Taking the lock means a waiter is either before its check or asleep, so this can't slip between them.
            drop(self.taken());
            self.0.freed.notify_all();
        }
    }
}

/// Holds the slot. Lives as long as the heavy process: dropped by the end action when it has gone, however it went.
#[derive(Debug)]
pub(super) struct HeavyTicket(HeavySlot);

impl Drop for HeavyTicket {
    fn drop(&mut self) {
        *self.0.taken() = false;
        self.0.0.freed.notify_all();
    }
}
