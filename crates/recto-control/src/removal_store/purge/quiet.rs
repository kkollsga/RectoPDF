//! **The proof that the history's writers are quiet** (`docs/design/removal.md` §10 "Step 4 (quiescence) design"): [`HistoryQuiet`], which [`super::purge_history`]
//! can't be called without, as `purge_caches` can't be called without a `CachesQuiet`.
//!
//! It is made from two things of one state folder: this process's record store held for a removal ([`RemovalHold`]: it writes, renames and deletes nothing there)
//! and the clear's [`Leases`] (their windowless lock, `run/session-windowless.lock`, is the windowless store's own record lock, so no `recto serve` keeps a
//! windowless record there). A pair from two folders proves neither, so `of` is `None` for it. Holds no thread, no timer, no buffer and no descriptor of its own:
//! the borrows keep the hold and the leases alive for as long as the proof is.

use crate::record_store::RemovalHold;
use crate::removal_store::leftovers::leases::Leases;
use std::marker::PhantomData;
use std::path::Path;

/// What the history's writers being quiet looks like to the purge. Not `Clone`, no `Default`: it can only be had from a real hold and real leases of one state
/// folder (or, in a test, assumed).
#[derive(Debug)]
pub struct HistoryQuiet<'a> {
    /// The state folder the proof is for. `None` only for the test's assumed one, which is for every folder.
    root: Option<&'a Path>,
    _held: PhantomData<&'a RemovalHold>,
}

impl<'a> HistoryQuiet<'a> {
    /// The proof, if `hold` and `leases` are of the same state folder (plain path equality: in production both come from the same `StatePaths::root`).
    pub fn of(hold: &'a RemovalHold, leases: &'a Leases) -> Option<HistoryQuiet<'a>> {
        (hold.state_root() == leases.state_root()).then(|| HistoryQuiet {
            root: Some(hold.state_root()),
            _held: PhantomData,
        })
    }

    /// Stands for "the record store writes nothing and no windowless record is kept": a test runs no writer.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn assumed_for_tests() -> HistoryQuiet<'static> {
        HistoryQuiet {
            root: None,
            _held: PhantomData,
        }
    }

    /// Whether this proof is for the folder `roots` are in.
    pub(super) fn is_for(&self, state_root: &Path) -> bool {
        self.root.is_none_or(|root| root == state_root)
    }
}

#[cfg(test)]
mod tests;
