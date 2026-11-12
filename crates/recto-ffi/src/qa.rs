//! **The quiet run's guard's calls.** What they say is `crate::core::qa`'s.

use crate::AppCore;
use crate::core::qa::policy_nudge;

#[uniffi::export]
impl AppCore {
    /// [`policy_nudge`], in milliseconds, for the host's quiet-run guard.
    pub fn qa_policy_nudge_ms(&self) -> Option<u64> {
        policy_nudge(self.qa_mode).map(|nudge| nudge.every.as_millis() as u64)
    }

    /// Whether [`policy_nudge`] also lands a change inside the guard's put-back.
    pub fn qa_policy_nudge_in_put_back(&self) -> bool {
        policy_nudge(self.qa_mode).is_some_and(|nudge| nudge.in_put_back)
    }
}
