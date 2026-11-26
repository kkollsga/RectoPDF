//! What `recto local` and `recto session` give a save: its lineage noted in a ledger that is already there, and nothing else (`docs/design/removal.md` §10).
//!
//! Those routes save in a process of their own, with no backups, journals or record, but into the person's real files and, unless the state folder is a test
//! one, their real state folder. A save there that the ledger never heard of leaves a crashed Save As's temp that a later removal would call "none found".
//! It notes only into a ledger that has begun: the one that begins it hands the older journals to a thread to adopt, which a process this short-lived
//! would cut off at exit, leaving a ledger that never adopts them. With no ledger the removal refuses anyway (`Gap::Missing`), and the app, which lives
//! long enough, begins it.

use super::{DIR, FILE, note_save};
use crate::state::StatePaths;
use recto_agent::saving::{SaveNote, SaveSupport};
use std::path::PathBuf;

/// Notes a save into the ledger of the state folder, if it has begun. Keeps no backup and writes no journal: a save here is as it always was.
pub struct ProvenanceOnly {
    root: PathBuf,
}

impl ProvenanceOnly {
    /// For the state folder `paths` names.
    pub fn of(paths: &StatePaths) -> Self {
        Self { root: paths.root.clone() }
    }
}

impl SaveSupport for ProvenanceOnly {
    fn backups(&self) -> Option<PathBuf> {
        None
    }

    fn provenance(&self, note: &SaveNote<'_>) {
        let begun = std::fs::symlink_metadata(self.root.join(DIR).join(FILE)).is_ok_and(|meta| meta.is_file());
        if begun {
            note_save(&self.root.join("backups.noindex"), note);
        }
    }
}
