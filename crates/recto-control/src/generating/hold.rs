//! **The generator's hold for a removal** (`docs/design/removal.md` §10, "Step 4 (quiescence) design"): the acknowledgment of `Quiesce(HeadingJobs)`. While a [`GenHold`]
//! lives, no ask is taken, from a person or an agent, for contents or for blocks, and nothing is made.
//!
//! **How.** [`Generator::hold_for_removal`] does, under the queue's own lock: refuses a second hold, sets `on_hold`, clears what waits, sets the running job's stop, and waits on
//! `ended` until the thread has gone (`!working`), at most until `until`. **The stop is never the proof:** a contents job writes its result after the run, and reads its stop
//! before that, so one stopped in between is written anyway. `working` is cleared only after that write, under the lock the hold waits on. Past `until` the hold is let go and the
//! answer says the thread was still there, and a job it stopped stays stopped. No thread, timer, poll or buffer.
//!
//! **Not covered:** the results the generator holds in memory when a file couldn't be written (`held`). Dropping them is `PublishInvalidation`'s.

use super::*;
use crate::indexing::NotQuiet;

/// The generator, held for a removal. Dropping it lets asks be taken again, and starts nothing. It's one clone of the queue's handle: no thread, timer or buffer.
#[must_use = "the generator is held for as long as this lives"]
pub struct GenHold(Generator);

impl std::fmt::Debug for GenHold {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("GenHold").finish_non_exhaustive()
    }
}

impl Drop for GenHold {
    fn drop(&mut self) {
        self.0.0.lock().on_hold = false;
    }
}

impl Generator {
    /// Holds the generator for a removal's walk. The return is the acknowledgment that no job is running or writing, and none will start until the hold is dropped. On the walk's own
    /// thread, never the UI's: it waits, at most until `until`.
    pub fn hold_for_removal(&self, until: Instant) -> Result<GenHold, NotQuiet> {
        let mut inner = self.0.lock();
        if inner.on_hold {
            return Err(NotQuiet::AlreadyHeld);
        }
        inner.on_hold = true;
        inner.waiting.clear();
        if let Some((_, _, stop, _)) = &inner.running {
            stop.store(true, Ordering::SeqCst);
        }
        let wait = until.saturating_duration_since(Instant::now());
        let (mut inner, _) = self
            .0
            .ended
            .wait_timeout_while(inner, wait, |inner| inner.working)
            .unwrap_or_else(PoisonError::into_inner);
        if inner.working {
            inner.on_hold = false;
            return Err(NotQuiet::StillRunning { building: true, uses: 0 });
        }
        Ok(GenHold(self.clone()))
    }

    /// For a test: runs `between` on the queue's thread after a contents job's run has ended and before its result is written.
    #[cfg(test)]
    pub(crate) fn between_end_and_write(&self, between: impl Fn() + Send + Sync + 'static) {
        let _ = self.0.between_end_and_write.set(Box::new(between));
    }
}

#[cfg(test)]
mod tests;
