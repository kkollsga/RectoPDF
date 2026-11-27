//! **What an unfinished removal does to everything that would bring data back** (`docs/design/removal.md` §6, D3b). While a transaction record is in the
//! state folder, nothing reads, moves, copies, rotates, writes or deletes the session's files, because each of those can resurrect what the removal's plan has
//! pinned and is purging: the record store's load (a `session.v<N>` coming back from a going back, a new copy of the history going forward, the
//! temporaries swept, a damaged slot rotated), a write of the session or a history, and, in `recto-update`, the one-time fall back to `update/before` and the
//! install. A record that can't be read is the same: **a broken or too-new record never becomes "nothing pending"**. Only `Nothing` lets anything go on.
//!
//! This is the only caller of [`barrier::look`](super::barrier::look) outside its own tests (a test holds it). It keeps nothing: the decoded record (up to
//! 2 MiB at its cap) is dropped before an answer is given, and the answer is two words. `recto_update::removal_record` looks at the same name from the
//! updater's side without decoding it, which a test holds to the same answer for every shape of folder.

/// Why the session's files are left as they are. It carries no path, no count and nothing from the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Withheld {
    /// A removal began and wasn't finished: its record is there, whatever step it had reached.
    Pending,
    /// Something is at the record's name that can't be read as one (damaged, from a newer RectoPDF, not ours, or on a disk this can't write to).
    Unreadable,
}

impl Withheld {
    /// The code a refused write carries, and what `qa` says.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Pending => "removal_pending",
            Self::Unreadable => "removal_unreadable",
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Withheld;
    use crate::removal_store::barrier::{self, Looked};
    use recto_core::removal::Outcome;
    use std::path::Path;

    /// `None` is `Nothing` and only `Nothing`. `root` is `StatePaths::root`. On the disk, may wait: not for the UI thread.
    pub(crate) fn at(root: &Path) -> Option<Withheld> {
        match barrier::look(root) {
            Looked::Nothing => None,
            Looked::Pending(_) => Some(Withheld::Pending),
            Looked::Damaged | Looked::TooNew | Looked::NotOurs => Some(Withheld::Unreadable),
        }
    }

    fn outcome_word(outcome: Outcome) -> &'static str {
        match outcome {
            Outcome::Unchanged => "unchanged",
            Outcome::HistoryRetiring => "history_retiring",
            Outcome::ReplacementInDoubt => "replacement_in_doubt",
            Outcome::SanitizedCleanupPending => "sanitized_cleanup_pending",
            Outcome::Complete => "complete",
            Outcome::PartlyCleared => "partly_cleared",
        }
    }

    /// For `qa removal-state`: what is there, in words, and for a pending one how far it has got. No path, and nothing from the plan beyond two counts.
    pub fn state(root: &Path) -> serde_json::Value {
        match barrier::look(root) {
            Looked::Nothing => serde_json::json!({ "state": "nothing" }),
            Looked::Damaged => serde_json::json!({ "state": "damaged" }),
            Looked::TooNew => serde_json::json!({ "state": "too_new" }),
            Looked::NotOurs => serde_json::json!({ "state": "not_ours" }),
            Looked::Pending(recorded) => serde_json::json!({
                "state": "pending",
                "outcome": outcome_word(recorded.outcome()),
                "acked": recorded.acked(),
                "steps": recorded.plan().steps().len(),
            }),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use mac::at;
#[cfg(target_os = "macos")]
pub use mac::state;

/// Nothing writes a record on any other system, and nothing is withheld there.
#[cfg(not(target_os = "macos"))]
pub(crate) fn at(_root: &std::path::Path) -> Option<Withheld> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn state(_root: &std::path::Path) -> serde_json::Value {
    serde_json::json!({ "state": "nothing" })
}

/// Whether anything is withheld: the same as `at`, for a caller that wants only the yes.
pub fn anything_there(root: &std::path::Path) -> bool {
    at(root).is_some()
}

#[cfg(test)]
mod tests;
