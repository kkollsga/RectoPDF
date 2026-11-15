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
/// Most bytes one job reads (each stream's stored length and each byte hashed, as `recto_core::page_id::read_page` counts them) before it hands in
/// what it read and the next job goes on (rule 1 and 6): bounded by bytes, not time, so a slow disk or a loaded machine costs only time, never pages.
/// A job reads at least one page; one page costs at most its caps, about 336 MiB.
pub const MAX_JOB_BYTES: u64 = 256 << 20;

/// [`MAX_JOB_BYTES`], which a test sets small on its own thread to hold it at its limit plus one.
fn job_bytes() -> u64 {
    #[cfg(test)]
    if let Some(bytes) = crate::tests::identify::JOB_BYTES.with(std::cell::Cell::get) {
        return bytes;
    }
    MAX_JOB_BYTES
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

/// What a job read. Its `Debug` says how many pages, never what they are.
#[derive(Clone, PartialEq, Eq)]
pub struct Identified {
    pub tab: TabId,
    pub stamp: FileStamp,
    pub pages: Vec<(u32, Known)>,
    pub document: Option<DocumentIds>,
    /// A page had no identity (it reached a cap): reading stopped there, and the list names no more of this version ([`identity::give_up`]).
    pub gave_up: bool,
}

impl std::fmt::Debug for Identified {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Identified")
            .field("tab", &self.tab)
            .field("pages", &self.pages.len())
            .field("gave_up", &self.gave_up)
            .finish_non_exhaustive()
    }
}

/// **A file opened for its pages' identities, kept across jobs** (the page-identity thread's, and the matcher's scan of a new version): its reader and
/// page map, built once for one version of one file, and given again only while the file is still that version. Gone with whoever holds it.
pub struct Opened {
    path: PathBuf,
    stamp: FileStamp,
    reader: recto_core::page_id::IndexedReader,
    map: recto_core::page_id::PageMap,
}

impl Opened {
    /// The file at `path`, if it's that version and opens through the file wall (`recto_core::page_id::open_for_identity`).
    pub fn open(path: &std::path::Path, stamp: FileStamp) -> Option<Self> {
        if !is_version(path, stamp) {
            return None;
        }
        let reader = recto_core::page_id::open_for_identity(path).ok()?;
        let map = reader.page_map().ok()?;
        Some(Self {
            path: path.to_path_buf(),
            stamp,
            reader,
            map,
        })
    }

    pub fn reader(&self) -> &recto_core::page_id::IndexedReader {
        &self.reader
    }

    pub fn map(&self) -> &recto_core::page_id::PageMap {
        &self.map
    }

    /// Whether this is that file at that version.
    pub fn is(&self, path: &std::path::Path, stamp: FileStamp) -> bool {
        self.path == path && self.stamp == stamp
    }
}

/// Whether the file at `path` is that version now.
fn is_version(path: &std::path::Path, stamp: FileStamp) -> bool {
    recto_platform::file_stamp(path).ok().is_some_and(|now| now == stamp)
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

/// The pages the edited tabs' lists name, and how many of them are read: counts for QA, never what a page is.
pub fn counts<L: DocLoader>(session: &Session<L>) -> (usize, usize) {
    session.lists.iter().fold((0, 0), |(named, read), (_, edits)| {
        identity::identities(edits).map_or((named, read), |ids| {
            (
                named + ids.pages().len(),
                read + ids.pages().iter().filter(|(_, known)| known.is_some()).count(),
            )
        })
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
/// read is of another version), or it can't be opened. A page the file doesn't have is left out. [`read_in`] with nothing open.
pub fn read(job: &IdentifyJob) -> Option<Identified> {
    read_in(job, &mut None)
}

/// [`read`], with the file `opened` kept from an earlier job if it's this file at this version, and kept there for the next: what the page-identity
/// thread does, so a file's page map is built once however many jobs its pages take. Reads up to [`MAX_JOB_BYTES`] (at least one page); the pages
/// left are the next job's.
pub fn read_in(job: &IdentifyJob, opened: &mut Option<Opened>) -> Option<Identified> {
    if !is_version(&job.path, job.stamp) {
        *opened = None;
        return None;
    }
    if !opened.as_ref().is_some_and(|open| open.is(&job.path, job.stamp)) {
        *opened = Opened::open(&job.path, job.stamp);
    }
    let open = opened.as_ref()?;
    let (budget, mut spent) = (job_bytes(), 0u64);
    let mut pages = Vec::with_capacity(job.pages.len());
    let mut gave_up = false;
    for page in &job.pages {
        if spent >= budget {
            break;
        }
        let Some(entry) = open.map.get(*page as usize) else { continue };
        let read = recto_core::page_id::read_page(&open.reader, entry, &recto_core::page_id::Caps::STANDARD);
        spent = spent.saturating_add(read.bytes);
        let Some(identity) = read.identity else {
            // A page past a cap has no identity: its edits can't follow it, and nothing more of this version is read.
            gave_up = true;
            break;
        };
        pages.push((*page, Known::of(&identity)));
    }
    let document = recto_core::page_id::document_ids(&open.reader);
    if !is_version(&job.path, job.stamp) {
        *opened = None;
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
    taken || found.gave_up
}
