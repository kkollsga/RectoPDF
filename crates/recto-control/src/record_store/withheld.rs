//! **What an unfinished removal's record does to this store** (`removal_store::gate`, `docs/design/removal.md` §6): looked at once, kept until the next load,
//! and the one thing every refusal to read, move, write or delete asks. The store's own steps ask it ([`RecordStore::refuses_writes`], `load`, the reads).

use super::*;

impl RecordStore {
    /// What an unfinished removal's record says, looked at once (not under the progress lock: it reads the disk) and kept until the next load.
    pub(super) fn gate(&self) -> Option<Withheld> {
        if let Some(known) = self.progress().gate {
            return known;
        }
        let looked = gate::at(&self.root);
        self.progress().gate = Some(looked);
        looked
    }

    /// The state folder this record is kept under (`StatePaths::root`), for what looks at the removal's record from outside the store (`gate::state`).
    pub fn state_root(&self) -> &Path {
        &self.root
    }

    /// What `gate` found, if it has looked: for a QA read, which never reads the disk.
    pub fn withheld(&self) -> Option<Withheld> {
        self.progress().gate.flatten()
    }

    /// The refusal of a write while a removal is unfinished, in Rust's words.
    pub(super) fn withheld_error(&self) -> Option<StoreError> {
        self.gate()
            .map(|withheld| StoreError::new(withheld.code(), record::notices::words(withheld.code()).unwrap_or_default()))
    }

    /// Why this won't write, if it won't: another copy keeps the record, an unfinished removal's record is there, or what's here couldn't be read.
    pub(super) fn refuses_writes(&self) -> Option<StoreError> {
        if !self.holds_the_record() {
            return Some(self.other_instance());
        }
        let unreadable = || {
            self.progress()
                .read_only
                .then(|| StoreError::new("record_unreadable", record::notices::words(record::notices::UNREADABLE).unwrap_or_default()))
        };
        self.withheld_error().or_else(unreadable)
    }

    /// Whether a journal can be kept at all: not when another copy keeps the record, an unfinished removal's record is there, or what's here couldn't be read.
    pub fn keeps_journals(&self) -> bool {
        self.refuses_writes().is_none()
    }
}
