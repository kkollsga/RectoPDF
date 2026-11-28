//! **The indexer's hold for a removal** (`docs/design/removal.md` §10, "Step 4 (quiescence) design"): the acknowledgment of `Quiesce(IndexJobs)` and, within this process,
//! of `Quiesce(AgentExports)`. While an [`IndexHold`] lives, no build starts, no sweep runs, no key is handed out and no agent's folder is claimed.
//!
//! **How.** [`Indexer::hold_for_removal`] does, under the queue's own lock: refuses a second hold, sets `on_hold`, clears what waits, stops what runs (the way `cancel`
//! does), and waits on `ended` until the thread has gone (`!working`) and no use of the key is out (`uses == 0`), at most until `until`. The thread clears `working` under
//! that same lock, after the last thing it does, and a [`KeyUse`]'s end notifies the same condvar, so "quiet" is seen, never inferred from "cancel sent". Past `until` the
//! hold is let go and the answer says what's still going. No thread, no timer, no poll, no buffer: one flag, one counter and a condvar the queue has anyway.
//!
//! **Every starter refuses under the lock it takes:** `ask`, `note_asked`, the work loop's sweep and pop, `made_agent_folder`, and the counted key. A refusal looks like `stopped`
//! does: nothing's queued, nothing's said. A held `find_key` answers `None` at once and never waits, so Find reads pages live, as it does when no key is in hand.
//!
//! **On success the claims go,** this session's agent folders' locks, unlocked and closed, and nothing renamed (a retire would move what the plan pinned): the purge's probe
//! finds each `.session` free. A refused hold doesn't touch them. Dropping a hold clears `on_hold` and restarts nothing: whoever wants a build asks again.

use super::*;
use recto_agent::index::UseEnded;

/// Why the indexer couldn't be held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotQuiet {
    /// Another hold is on it.
    AlreadyHeld,
    /// Something was still going at the bound: the queue's thread is there (`building`: a build, or a sweep or a clean-up that runs on it), or `uses` of the Find key are out. A build it stopped stays stopped.
    StillRunning { building: bool, uses: u32 },
}

/// The indexer, held for a removal. Dropping it lets asks run again. It's one clone of the queue's handle: no thread, timer or buffer.
#[must_use = "the indexer is held for as long as this lives"]
pub struct IndexHold(Indexer);

impl std::fmt::Debug for IndexHold {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("IndexHold").finish_non_exhaustive()
    }
}

impl Drop for IndexHold {
    fn drop(&mut self) {
        self.0.0.lock().on_hold = false;
    }
}

impl Indexer {
    /// Holds the indexer for a removal's walk. The return is the acknowledgment that no build, sweep or clean-up is under way, no use of the Find key is out, none will start until the
    /// hold is dropped, and this session's agent folders are no longer claimed. On the walk's own thread, never the UI's: it waits, at most until `until`.
    pub fn hold_for_removal(&self, until: Instant) -> Result<IndexHold, NotQuiet> {
        let mut inner = self.0.lock();
        if inner.on_hold {
            return Err(NotQuiet::AlreadyHeld);
        }
        inner.on_hold = true;
        inner.waiting.clear();
        // What `cancel` does for a running build.
        if inner.running.is_some() {
            inner.cancelled = true;
            inner.stop_running = None;
        }
        let wait = until.saturating_duration_since(Instant::now());
        let (mut inner, _) = self
            .0
            .ended
            .wait_timeout_while(inner, wait, |inner| inner.working || inner.uses > 0)
            .unwrap_or_else(PoisonError::into_inner);
        if inner.working || inner.uses > 0 {
            inner.on_hold = false;
            return Err(NotQuiet::StillRunning {
                building: inner.working,
                uses: inner.uses,
            });
        }
        for (_, claim) in inner.agent_folders.drain(..) {
            claim.release();
        }
        Ok(IndexHold(self.clone()))
    }
}

impl Shared {
    /// Counts one use of the key, unless the indexer is held: then none, and the answer is no key, at once.
    pub(super) fn begin_use(&self) -> Option<()> {
        let mut inner = self.lock();
        if inner.on_hold {
            return None;
        }
        inner.uses += 1;
        Some(())
    }

    /// The counted use of `key`, or, with no key, the count taken back down.
    pub(super) fn counting<'a>(&'a self, key: Option<&'a FindKey>) -> Option<KeyUse<'a>> {
        match key {
            Some(key) => Some(KeyUse::counted(key, self)),
            None => {
                self.use_ended();
                None
            }
        }
    }
}

impl UseEnded for Shared {
    fn use_ended(&self) {
        let mut inner = self.lock();
        inner.uses = inner.uses.saturating_sub(1);
        if inner.uses == 0 {
            self.ended.notify_all();
        }
    }
}

#[cfg(test)]
mod tests;
