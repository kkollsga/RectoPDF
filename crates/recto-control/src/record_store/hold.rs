//! **The record store's hold for a removal** (`docs/design/removal.md` §4 and §10 "D2e history as built"): the acknowledgment of `Quiesce(Journals)` and
//! `Quiesce(SleepFiles)`. While a [`RemovalHold`] lives, every write, rename and delete this store makes under its folder is refused with the removal's own
//! code (`removal_pending`), so nothing of the target's history is written outside the plan after the walk was told the writers are quiet.
//!
//! **How.** [`RecordStore::hold_for_removal`] sets `held` and the cached gate to `Pending` under the progress lock, the lock every write holds across its
//! bytes. Returning is the acknowledgment: a write that holds the lock when the hold is asked finishes first, and every write that takes it afterwards sees
//! [`Progress::refusal`] and writes nothing. A write's first look (`refuses_writes`) is outside the lock, so each path **looks again under the lock before
//! its first byte**; a path that used to write before taking the lock (a sleep file, a kept copy, a checkpoint, a reopened history) now takes it first.
//! No thread, no timer, no buffer: one flag and the cached gate.
//!
//! **Release looks again.** Dropping the hold clears the cached gate, so the next ask reads the disk: a barrier begun since reads `Pending` and goes on
//! refusing; one never begun, abandoned or retired reads `Nothing`, and writes go on. A `load` while held returns `Withheld(Pending)` and touches nothing.
//!
//! **A load and a hold exclude each other.** `recto restore` loads a live session, so a `load` can run while a walk wants the hold. `load` sets `loading`
//! under the progress lock before its first write and clears it after its tails (`prune_sleep_files`, `prune_history`); a hold asked meanwhile is refused
//! (`HoldRefused::Loading`), and a `load` asked while held returns `Withheld(Pending)` and touches nothing.
//!
//! **What is not covered.** `note_going_back`'s `went-back` marker may still be written between a write's first check and its lock: it holds no document
//! data (design §10, D2c-3). `write_record`'s ledger note (`ledger::note_tabs`) is the ledger's, not a history store's. The hold refuses every write, the
//! carrier's own included: `CleanSession`'s and `ClearFindText`'s writes through a hold are the command slice's to build.

use super::*;
use std::sync::Arc;

/// A store held for a removal. Dropping it releases the hold. Holds no thread and no buffer.
#[derive(Debug)]
pub struct RemovalHold {
    store: Arc<RecordStore>,
}

/// Why a store couldn't be held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldRefused {
    /// Another process keeps this folder's record.
    OtherInstance,
    /// A removal's record is there already, or one that can't be read: nothing is written, and a hold would claim what isn't this walk's.
    Withheld,
    /// What's here couldn't be read, so nothing's to be written over it.
    Unreadable,
    /// Another hold is on this store.
    AlreadyHeld,
    /// A `load` is under way (`recto restore` loads a live session), and it moves, copies and deletes history until it ends.
    Loading,
}

impl RemovalHold {
    /// The state folder this hold is on, for a proof that pairs it with the leases of the same folder (`removal_store::purge::HistoryQuiet`).
    pub fn state_root(&self) -> &Path {
        self.store.state_root()
    }
}

impl RecordStore {
    /// Holds the store for a removal's walk. The return is the acknowledgment that no write is under way and none will start until the hold is dropped.
    /// On whatever thread the walk is on: it may look at the disk once, outside the lock.
    pub fn hold_for_removal(self: &Arc<Self>) -> Result<RemovalHold, HoldRefused> {
        if !self.holds_the_record() {
            return Err(HoldRefused::OtherInstance);
        }
        // A hold's own cached gate would read as a removal's record, so a second ask is told what it is.
        if self.progress().held {
            return Err(HoldRefused::AlreadyHeld);
        }
        if self.gate().is_some() {
            return Err(HoldRefused::Withheld);
        }
        let mut progress = self.progress();
        if progress.read_only {
            return Err(HoldRefused::Unreadable);
        }
        if progress.loading {
            return Err(HoldRefused::Loading);
        }
        // Looked at above already; here it only keeps the reason right when two holds race for the lock. Safety is the gate check below.
        if progress.held {
            return Err(HoldRefused::AlreadyHeld);
        }
        if progress.gate.flatten().is_some() {
            return Err(HoldRefused::Withheld);
        }
        progress.held = true;
        progress.gate = Some(Some(Withheld::Pending));
        Ok(RemovalHold { store: Arc::clone(self) })
    }

    /// `load`'s frame: the record taken, a hold's refusal (`Withheld(Pending)`: the reset of the store's state below would forget the hold), the state reset and
    /// `loading` set before the first write, and `loading` cleared after the last (the tails included).
    pub(super) fn while_loading(&self, load: impl FnOnce(&Self) -> Loaded) -> Loaded {
        if !self.wait_for_lock() {
            return Loaded::OtherInstance;
        }
        {
            let mut progress = self.progress();
            if progress.held {
                return Loaded::Withheld(Withheld::Pending);
            }
            *progress = Progress::default();
            progress.loading = true;
        }
        parked(&self.root, "load");
        let loaded = load(self);
        self.progress().loading = false;
        loaded
    }

    /// Removes a file of the store's folder unless a hold (or an unfinished removal) is on it, under the lock every write holds.
    pub(super) fn remove_unless_held(&self, path: &Path) {
        let progress = self.progress();
        if !progress.withholds() {
            let _ = std::fs::remove_file(path);
        }
        drop(progress);
    }
}

impl Progress {
    /// Whether a removal's hold, or its record as last looked at, refuses writes. Only the cached gate: the lock is held, so never `gate()` here.
    pub(super) fn withholds(&self) -> bool {
        matches!(self.gate, Some(Some(_)))
    }

    /// What a refused write returns, as `refuses_writes` would, from the cached gate alone.
    pub(super) fn refusal(&self) -> Option<StoreError> {
        let Some(Some(withheld)) = self.gate else { return None };
        Some(StoreError::new(withheld.code(), record::notices::words(withheld.code()).unwrap_or_default()))
    }
}

impl Drop for RemovalHold {
    fn drop(&mut self) {
        let mut progress = self.store.progress();
        progress.held = false;
        progress.gate = None;
    }
}

/// What a journal write returns when it wrote nothing: every name it was to retire is still to be retired, and why it wrote nothing.
pub(super) fn refused_journals(work: &JournalWork, error: StoreError) -> (JournalOutcome, Option<StoreError>) {
    let outcome = JournalOutcome {
        not_retired: work.retired.clone(),
        ..JournalOutcome::default()
    };
    (outcome, Some(error))
}

/// Where a test may stop a store (`park`): `"journals"`, between a journal write's first check and its lock; `"kept"`, between a kept copy and its rename;
/// `"load"`, in a `load` after it has marked itself; `"sweep"`, before each file a sweep of sleep files removes.
pub(super) fn parked(root: &Path, place: &'static str) {
    #[cfg(test)]
    park::here(root, place);
    #[cfg(not(test))]
    let _ = (root, place);
}

/// A test's way to stop a store at a named place, by the store's folder.
#[cfg(test)]
pub(super) mod park {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    type Park = Arc<dyn Fn() + Send + Sync>;
    static PARKS: Mutex<Option<HashMap<(PathBuf, &'static str), Park>>> = Mutex::new(None);

    pub(in super::super) fn set(root: &Path, place: &'static str, park: Park) {
        PARKS.lock().unwrap().get_or_insert_with(HashMap::new).insert((root.to_path_buf(), place), park);
    }

    pub(in super::super) fn clear(root: &Path) {
        if let Some(parks) = PARKS.lock().unwrap().as_mut() {
            parks.retain(|(at, _), _| at != root);
        }
    }

    pub(super) fn here(root: &Path, place: &'static str) {
        let park = PARKS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|parks| parks.get(&(root.to_path_buf(), place)).cloned());
        if let Some(park) = park {
            park();
        }
    }
}

#[cfg(test)]
mod tests;
