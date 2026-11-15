//! **Reading what the edits' pages are, while their file is the version they're about** (the page matcher). A list names pages as its steps are made
//! ([`crate::edit::identity`]); this is what reads them: [`work`] says which tabs' files to read and which pages, [`read`] reads them off the session's
//! lock, and [`identified`] takes in what was read, if the file is still the version the edits are about. The app's page-identity thread runs the three
//! (`recto-app`'s `core/page_ids.rs`); nothing here keeps a thread, a handle or a buffer.
//!
//! **Never for a protected document** (the coordinator's decision, 2026-11-15): a tab opened with a password the person typed, or needing one, is never
//! read here, so no digest of it is made, kept or shown; its edits are held as before when its file changes. Nor for a file only the renderer can read,
//! as the digest is the structure reader's. Every open goes through the file wall (`recto_core::page_id::open_for_identity`).

use crate::edit::identity::{self, Known};
use crate::{DocLoader, Session, TabId};
use recto_core::page_id::DocumentIds;
use recto_platform::FileStamp;
use std::path::PathBuf;

/// Most pages one job reads, so one tab's many can't hold the thread from the others; the rest are the next job's.
pub const MAX_PAGES_A_JOB: usize = 512;

/// One tab's file to read, and which of its pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentifyJob {
    pub tab: TabId,
    pub path: PathBuf,
    /// The version the edits are about: read only if the file is still that, before and after.
    pub stamp: FileStamp,
    pub pages: Vec<u32>,
}

/// What a job read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identified {
    pub tab: TabId,
    pub stamp: FileStamp,
    pub pages: Vec<(u32, Known)>,
    pub document: Option<DocumentIds>,
}

/// Whether a tab's pages may be read for it now: its list names pages not read, its edits fit its file, whose version is known, and it isn't protected.
fn readable<L: DocLoader>(session: &Session<L>, id: TabId) -> Option<(PathBuf, FileStamp)> {
    let tab = session.tabs.tab(id)?;
    if crate::registry::withheld_tab(tab) || tab.renderer_only || !session.notices().fit_of(tab).fits() {
        return None;
    }
    Some((session.tabs.path_of(id)?, tab.file.stamp?))
}

/// Whether there's anything for [`work`]: a look at the edited tabs' lists alone, which the session thread asks after an edit.
pub fn has_work<L: DocLoader>(session: &Session<L>) -> bool {
    session.lists.iter().any(|(id, edits)| {
        identity::identities(edits).is_some_and(|ids| ids.pages().iter().any(|(_, known)| known.is_none())) && readable(session, *id).is_some()
    })
}

/// The files to read and their pages named and not read, at most [`MAX_PAGES_A_JOB`] a tab.
pub fn work<L: DocLoader>(session: &Session<L>) -> Vec<IdentifyJob> {
    session
        .lists
        .iter()
        .filter_map(|(id, edits)| {
            let mut pages = identity::wanted(edits);
            if pages.is_empty() {
                return None;
            }
            pages.truncate(MAX_PAGES_A_JOB);
            let (path, stamp) = readable(session, *id)?;
            Some(IdentifyJob { tab: *id, path, stamp, pages })
        })
        .collect()
}

/// **Reads a job's pages,** off the session's lock: `None` if the file isn't the version the edits are about, before the read or after it (so nothing
/// read is of another version), or it can't be opened. A page the file doesn't have is left out.
pub fn read(job: &IdentifyJob) -> Option<Identified> {
    let current = || recto_platform::file_stamp(&job.path).ok().is_some_and(|now| now == job.stamp);
    if !current() {
        return None;
    }
    let reader = recto_core::page_id::open_for_identity(&job.path).ok()?;
    let map = reader.page_map().ok()?;
    let read = recto_core::page_id::identify_pages(&reader, &map, &job.pages);
    let document = recto_core::page_id::document_ids(&reader);
    drop(reader);
    if !current() {
        return None;
    }
    Some(Identified {
        tab: job.tab,
        stamp: job.stamp,
        pages: job.pages.iter().zip(read).filter_map(|(page, id)| Some((*page, Known::of(&id?)))).collect(),
        document,
    })
}

/// **Takes in what was read,** if the tab's edits are still about that version of its file and it's still readable for them. Says whether anything
/// was taken in, which its journal then gets.
pub fn identified<L: DocLoader>(session: &mut Session<L>, found: &Identified) -> bool {
    if readable(session, found.tab).is_none_or(|(_, stamp)| stamp != found.stamp) {
        return false;
    }
    session
        .lists
        .get_mut(found.tab)
        .is_some_and(|edits| identity::found(edits, &found.pages, found.document))
}
