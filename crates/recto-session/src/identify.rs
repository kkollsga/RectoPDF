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
/// How long one job reads before it gives up on its file (rule 1 and 6): a real page takes about a millisecond and its caps bound a hostile one to
/// about a second, but 8,192 hostile pages would hold a core for hours. Past this, the pages the tab names and hasn't had read are no longer named, so
/// nothing reads them again until an edit names them; their edits can't follow a change, which the list says ([`identity::Identities::over_cap`]).
pub const MAX_JOB_TIME: std::time::Duration = std::time::Duration::from_secs(2);

/// [`MAX_JOB_TIME`], which a test sets to nothing on its own thread to hold it without a hostile file.
fn job_time() -> std::time::Duration {
    #[cfg(test)]
    if let Some(time) = crate::tests::identify::JOB_TIME.with(std::cell::Cell::get) {
        return time;
    }
    MAX_JOB_TIME
}

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
    /// It stopped at [`MAX_JOB_TIME`], before every page asked was read.
    pub gave_up: bool,
}

/// Whether a tab's pages may be read for it now: its list names pages not read, its edits fit its file, whose version is known, and it isn't protected.
fn readable<L: DocLoader>(session: &Session<L>, id: TabId) -> Option<(PathBuf, FileStamp)> {
    let tab = session.tabs.tab(id)?;
    if crate::registry::withheld_tab(tab) || tab.renderer_only || !session.notices().fit_of(tab).fits() {
        return None;
    }
    Some((session.tabs.path_of(id)?, tab.file.stamp?))
}

/// **The version a new journal starts at:** [`crate::record::JOURNAL_VERSION`] where its list names pages that will be read, as what's read of them is
/// written to it within moments of its first lines, and a journal made at an older version then has its header written again over everything it holds
/// so far (`recto-control`'s record store, `journal_marked_at`), which the SSD-wear budget counts; `otherwise` where nothing will be.
pub fn journal_version(tab: &crate::TabRecord, edits: &crate::edit::PageEdits, otherwise: u32) -> u32 {
    let readable = !crate::registry::withheld_tab(tab) && !tab.renderer_only;
    if readable && identity::identities(edits).is_some_and(|ids| !ids.pages().is_empty()) {
        crate::record::JOURNAL_VERSION
    } else {
        otherwise
    }
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
    let (began, limit) = (std::time::Instant::now(), job_time());
    let mut pages = Vec::with_capacity(job.pages.len());
    let mut gave_up = false;
    for page in &job.pages {
        if began.elapsed() > limit {
            gave_up = true;
            break;
        }
        if let Some(entry) = map.get(*page as usize) {
            let Some(identity) = recto_core::page_id::identify_page(&reader, entry, &recto_core::page_id::Caps::STANDARD) else {
                // A page past a cap has no identity: its edits can't follow it.
                gave_up = true;
                break;
            };
            pages.push((*page, Known::of(&identity)));
        }
    }
    let document = recto_core::page_id::document_ids(&reader);
    drop(reader);
    if !current() {
        return None;
    }
    Some(Identified {
        tab: job.tab,
        stamp: job.stamp,
        pages,
        document,
        gave_up,
    })
}

/// **Takes in what was read,** if the tab's edits are still about that version of its file and it's still readable for them. Says whether anything
/// was taken in, which its journal then gets.
pub fn identified<L: DocLoader>(session: &mut Session<L>, found: &Identified) -> bool {
    if readable(session, found.tab).is_none_or(|(_, stamp)| stamp != found.stamp) {
        return false;
    }
    let Some(edits) = session.lists.get_mut(found.tab) else { return false };
    let taken = identity::found(edits, &found.pages, found.document);
    if found.gave_up {
        identity::give_up(edits);
    }
    taken
}
