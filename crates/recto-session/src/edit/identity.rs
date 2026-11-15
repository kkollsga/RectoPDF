//! **Which of the file's pages a list's edits name, and what each of those pages is** (the page matcher; owner, 2026-11-13: "we need to run a page
//! matcher of some kind. That's the way we can ensure edits on the correct page"). An edit names its page by its number in the file as it was. When the
//! file changes on disk the old bytes may be gone, so what each named page *is* has to be read while they're there: its digest `recto-page/1`
//! ([`recto_core::page_id`]), its object number and its frame, kept here and in the journal's sidecar ([`sidecar`]), so the matcher can find the page in
//! the new version.
//!
//! **Which pages:** every page of the tab's own file in a run a step changes, the pages either side of that run in the document, and each of those
//! pages' neighbours in the file, which tell identical pages apart (the coordinator's decision, 2026-11-15). A step notes them before it's applied and
//! its inverse after, so a page moved, the pages it left and the pages it went between are all named. A highlight names its page; a removal of the
//! file's own highlight names the page it's on. Nothing here reads a PDF: an edit stays instant. The pages named are read later, off the UI thread, by
//! the app's page-identity thread, while the file is still the version the edits are about ([`crate::identify`]).
//!
//! **Kept beside the journal, never in it** (the coordinator's decision B1, 2026-11-15): a journal is byte for byte what it is without the page
//! matcher, so an app from before reads every journal it read before. What's read of a page goes to its journal's sidecar once, appended; which pages
//! are named comes back with the journal's steps, as replaying a step names its pages again. A list restored from a journal's base (a checkpoint)
//! has the pages its sidecar holds; one whose sidecar is lost, stale or another's has none of them, so its edits are held when the file changes, as
//! before the matcher: losing a sidecar costs only following.
//!
//! **Bounded** (rule 1): a tab with no edits has nothing here; a list holds at most [`MAX_IDENTIFIED`] pages, 36 bytes each in memory (288 KiB at the
//! cap) and 35 in the sidecar (280 KiB at the cap), and while a read's entries wait for the sidecar, 4 bytes more each. Pages past the cap aren't named,
//! which [`Identities::over_cap`] says, and an edit on one of them can't be followed when the file changes. Nothing is held per idle tab: a list asleep
//! is in its journal and sidecar.

pub mod sidecar;

use super::{PageEdits, Step};
use recto_core::page_id::DocumentIds;

/// Most of the file's pages one list names.
pub const MAX_IDENTIFIED: usize = 8_192;

/// What a named page costs, which [`MAX_IDENTIFIED`] multiplies: failure caught, a field added to [`Known`] that grows every edited tab's cap.
const _: () = assert!(std::mem::size_of::<(u32, Option<Known>)>() <= 36);

/// The cap, which a test sets small on its own thread to hold it at its limit plus one.
fn cap() -> usize {
    #[cfg(test)]
    if let Some(cap) = tests::CAP.with(std::cell::Cell::get) {
        return cap;
    }
    MAX_IDENTIFIED
}

/// What one page of the file is: the first 16 bytes of its `recto-page/1` digest, its object number, and its frame's tag ([`recto_core::page_id`]).
/// Its `Debug` says only the object number: a digest is of a document's content, and what's printed can reach a log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub digest: [u8; 16],
    pub object: (u32, u16),
    pub frame: u32,
}

impl std::fmt::Debug for Known {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Known").field("object", &self.object).finish_non_exhaustive()
    }
}

impl Known {
    /// What's kept of a page's identity as read.
    pub fn of(id: &recto_core::page_id::PageIdentity) -> Self {
        let mut digest = [0u8; 16];
        digest.copy_from_slice(&id.digest[..16]);
        Self {
            digest,
            object: id.object,
            frame: id.frame,
        }
    }
}

/// The pages of the file a list's edits name, ascending, with what each is once that's been read.
// size: the `pages` entries are 36 bytes each, at most MAX_IDENTIFIED of them, edited tabs only. `Debug` counts, and prints no digest.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Identities {
    pages: Vec<(u32, Option<Known>)>,
    /// More pages were named than [`MAX_IDENTIFIED`] holds, or a page had no identity ([`give_up`]): an edit on a page not named can't follow it.
    over_cap: bool,
    /// The file's `/ID` pair, as read with the pages, if it has one.
    document: Option<DocumentIds>,
    /// Pages read whose entries aren't in the sidecar yet, in the order read.
    unwritten: Vec<u32>,
    /// The `/ID` pair isn't in the sidecar yet.
    document_unwritten: bool,
    /// The sidecar is to be written whole: what it holds is of another version of the file, or another header.
    whole: bool,
    /// A page of this version had no identity: no more pages are named until the version changes, so a hostile file can't keep the reader busy.
    closed: bool,
    /// The list followed its file onto this version and the person hasn't been told yet (slice 2's; kept in the sidecar's header).
    followed_untold: bool,
    /// The file was found to be another version, this one, and the edits may follow their pages into it ([`crate::version_change`]); `tried` once
    /// that's been tried and they were held, so it isn't tried again for this version. Not kept: a restart finds the version again when it reads it.
    follow: Option<Follow>,
}

/// A version of the file the edits may follow their pages into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Follow {
    pub to: recto_platform::FileStamp,
    pub tried: bool,
}

impl std::fmt::Debug for Identities {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Identities")
            .field("pages", &self.pages.len())
            .field("read", &self.pages.iter().filter(|(_, known)| known.is_some()).count())
            .field("over_cap", &self.over_cap)
            .field("unwritten", &self.unwritten.len())
            .finish_non_exhaustive()
    }
}

impl Identities {
    /// The pages named, ascending, and what each is if it's been read.
    pub fn pages(&self) -> &[(u32, Option<Known>)] {
        &self.pages
    }

    /// Whether more pages were named than the cap holds, or could be read in time.
    pub fn over_cap(&self) -> bool {
        self.over_cap
    }

    pub fn document(&self) -> Option<DocumentIds> {
        self.document
    }

    /// The version the edits may follow their pages into, if one's been found.
    pub fn follow(&self) -> Option<Follow> {
        self.follow
    }

    /// What's known of one page, if it was named and has been read.
    pub fn known(&self, page: u32) -> Option<Known> {
        self.pages.binary_search_by_key(&page, |(at, _)| *at).ok().and_then(|at| self.pages[at].1)
    }

    /// Adds pages, ascending and without repeats, as named and not read, up to the cap; none once a page of this version had no identity.
    fn name(&mut self, mut pages: Vec<u32>) {
        if self.closed {
            return;
        }
        pages.sort_unstable();
        pages.dedup();
        let mut merged = Vec::with_capacity(self.pages.len() + pages.len());
        let mut new = pages.into_iter().peekable();
        for entry in std::mem::take(&mut self.pages) {
            while let Some(page) = new.next_if(|page| *page < entry.0) {
                merged.push((page, None));
            }
            new.next_if_eq(&entry.0);
            merged.push(entry);
        }
        merged.extend(new.map(|page| (page, None)));
        let cap = cap();
        if merged.len() > cap {
            // The pages named first stay; the newest, which go past the cap, aren't named. Which those are doesn't matter: an edit on any page not
            // named is set aside, never guessed.
            let mut kept: Vec<(u32, Option<Known>)> = Vec::with_capacity(cap);
            let before: std::collections::HashSet<u32> = self.pages.iter().map(|(page, _)| *page).collect();
            let mut room = cap.saturating_sub(before.len());
            for entry in merged {
                if before.contains(&entry.0) {
                    kept.push(entry);
                } else if room > 0 {
                    room -= 1;
                    kept.push(entry);
                } else {
                    self.over_cap = true;
                }
            }
            merged = kept;
        }
        self.pages = merged;
    }
}

/// **Names the pages a step is about,** in the list as it stands: called with a step before it's applied, and with its inverse after. A step that puts
/// pages back names the pages either side of where they go; any other, the pages of its run and either side of it.
pub(super) fn note(edits: &mut PageEdits, step: &Step) {
    let (from, to) = match step {
        Step::Rotate { start, len, .. } | Step::Move { start, len, .. } | Step::Remove { start, len } => (*start, start.saturating_add(*len)),
        Step::Restore { start, .. } => (*start, *start),
        Step::Mark(step) => {
            let mut pages: Vec<u32> = step.adds().iter().filter(|mark| mark.source == 0).map(|mark| mark.page).collect();
            pages.extend(step.files().iter().filter(|file| file.key.source == 0).map(|file| file.key.page));
            name_with_neighbours(edits, pages);
            return;
        }
    };
    let last = edits.slots.len().saturating_sub(1);
    let (lo, hi) = ((from as usize).saturating_sub(1), (to as usize).min(last));
    let room = cap().saturating_add(1);
    let pages: Vec<u32> = edits
        .slots
        .get(lo..=hi)
        .unwrap_or_default()
        .iter()
        .filter(|slot| slot.source == 0)
        .map(|slot| slot.page)
        .take(room)
        .collect();
    name_with_neighbours(edits, pages);
}

fn name_with_neighbours(edits: &mut PageEdits, pages: Vec<u32>) {
    if pages.is_empty() {
        return;
    }
    let file_pages = edits.file_pages();
    let mut named = Vec::with_capacity(pages.len() * 3);
    for page in pages {
        named.extend(
            [page.checked_sub(1), Some(page), page.checked_add(1)]
                .into_iter()
                .flatten()
                .filter(|page| *page < file_pages),
        );
    }
    edits.identities.get_or_insert_with(Box::default).name(named);
}

/// **A save landed:** page `n` of the document is page `n` of the saved file. Each page named goes to where it is now, and is to be read again there
/// (a turn the save wrote changes its frame; a rewrite, its object number); a page the save took out isn't named. Called before the list's pages are
/// made the saved file's own, as an edit made after the save was asked and before it landed names pages by the file as it was.
pub(super) fn saved(edits: &mut PageEdits) {
    let Some(ids) = edits.identities.take() else { return };
    let named: Vec<u32> = ids.pages.iter().map(|(page, _)| *page).collect();
    let mut now: Vec<u32> = edits
        .slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.source == 0 && named.binary_search(&slot.page).is_ok())
        .filter_map(|(position, _)| u32::try_from(position).ok())
        .collect();
    now.sort_unstable();
    let mut moved = Identities {
        over_cap: ids.over_cap,
        ..Identities::default()
    };
    moved.name(now);
    if !moved.pages.is_empty() || moved.over_cap {
        edits.identities = Some(Box::new(moved));
    }
}

/// The pages named and not yet read, ascending: what the page-identity thread is asked to read while the file is the edits' version.
pub fn wanted(edits: &PageEdits) -> Vec<u32> {
    edits
        .identities
        .as_deref()
        .map(|ids| ids.pages.iter().filter(|(_, known)| known.is_none()).map(|(page, _)| *page).collect())
        .unwrap_or_default()
}

/// The list's pages named, if any.
pub fn identities(edits: &PageEdits) -> Option<&Identities> {
    edits.identities.as_deref()
}

/// **What was read of the named pages,** taken in and noted for the sidecar. A page not named (a read of an older wanting) is left out, and one read
/// before stays as it was. Says whether anything was taken in.
pub fn found(edits: &mut PageEdits, pages: &[(u32, Known)], document: Option<DocumentIds>) -> bool {
    let Some(ids) = edits.identities.as_deref_mut() else { return false };
    let mut taken = false;
    for (page, known) in pages {
        if let Ok(at) = ids.pages.binary_search_by_key(page, |(at, _)| *at)
            && ids.pages[at].1.is_none()
        {
            ids.pages[at].1 = Some(*known);
            ids.unwritten.push(*page);
            taken = true;
        }
    }
    if document.is_some() && ids.document != document {
        ids.document = document;
        ids.document_unwritten = true;
        taken = true;
    }
    taken
}

/// **A page of this version of the file had no identity** (it reached a cap, `recto_core::page_id`): the pages named and not read are named no more,
/// none is named again until the version changes (a save, or the file followed to a new one), and the list says some of its pages can't be followed.
/// So a hostile file costs one capped page's read, and an agent's edits can't keep the reader busy on it.
pub fn give_up(edits: &mut PageEdits) {
    if let Some(ids) = edits.identities.as_deref_mut() {
        ids.pages.retain(|(_, known)| known.is_some());
        ids.over_cap = true;
        ids.closed = true;
    }
}

/// **The file was found to be another version, `to`:** the edits may follow their pages into it, once, unless they've been tried against it already.
/// Says whether that's new. A list that names no page has nothing to follow by, and is held as it is.
pub(crate) fn may_follow(edits: &mut PageEdits, to: recto_platform::FileStamp) -> bool {
    let Some(ids) = edits.identities.as_deref_mut() else { return false };
    if ids.follow.is_some_and(|follow| follow.to == to) {
        return false;
    }
    ids.follow = Some(Follow { to, tried: false });
    true
}

/// What's wanted of the list: whether it may follow its pages into a version not yet tried. A look at its own field, for the session thread after
/// every call (no tab looked up).
pub fn follow_wanted(edits: &PageEdits) -> Option<recto_platform::FileStamp> {
    edits.identities.as_deref()?.follow.filter(|follow| !follow.tried).map(|follow| follow.to)
}

/// Following was tried against `to` and the edits were held (or it no longer applies): not tried again for it.
pub(crate) fn follow_tried(edits: &mut PageEdits, to: Option<recto_platform::FileStamp>) {
    if let Some(ids) = edits.identities.as_deref_mut() {
        match to {
            Some(to) if ids.follow.is_some_and(|follow| follow.to == to) => ids.follow = Some(Follow { to, tried: true }),
            Some(_) => {}
            None => ids.follow = None,
        }
    }
}

/// **The file was followed to a new version** (slice 2's rebase): `pages` (by their numbers in the new file) are what those pages are there, and the
/// whole of what's known is of the new version, so the sidecar is written again whole, with the new version's stamp, at the next write; what it held of
/// the old version is gone with it. A page past the new file's length or past the cap isn't kept. Pages stay as named only by these, read.
pub fn followed(edits: &mut PageEdits, pages: &[(u32, Known)], document: Option<DocumentIds>) {
    let file_pages = edits.file_pages();
    let mut pages: Vec<(u32, Known)> = pages.iter().copied().filter(|(page, _)| *page < file_pages).collect();
    pages.sort_unstable_by_key(|(page, _)| *page);
    pages.dedup_by_key(|(page, _)| *page);
    let over_cap = pages.len() > MAX_IDENTIFIED;
    pages.truncate(MAX_IDENTIFIED);
    let ids = Identities {
        unwritten: pages.iter().map(|(page, _)| *page).collect(),
        pages: pages.into_iter().map(|(page, known)| (page, Some(known))).collect(),
        over_cap,
        document,
        document_unwritten: document.is_some(),
        whole: true,
        closed: false,
        followed_untold: false,
        follow: None,
    };
    edits.identities = Some(Box::new(ids));
}

/// **After [`followed`]:** each known page's neighbours in the new version are named too, to be read there, so the next change of the file can tell
/// copies apart again (slice 2). Nothing is read here.
pub(super) fn name_neighbours(edits: &mut PageEdits) {
    let pages: Vec<u32> = edits
        .identities
        .as_deref()
        .map(|ids| ids.pages.iter().map(|(page, _)| *page).collect())
        .unwrap_or_default();
    name_with_neighbours(edits, pages);
}

/// Whether the list followed its file onto this version and the person hasn't been told yet: what its sidecar's header says (slice 2 sets and clears it).
pub fn followed_untold(edits: &PageEdits) -> bool {
    edits.identities.as_deref().is_some_and(|ids| ids.followed_untold)
}

/// Says the list followed its file onto this version and the person hasn't been told (`true`), or has been (`false`). A change goes in the sidecar's
/// header, which is written whole at the next write for it.
pub fn set_followed_untold(edits: &mut PageEdits, untold: bool) {
    let ids = edits.identities.get_or_insert_with(Box::default);
    if ids.followed_untold != untold {
        ids.followed_untold = untold;
        ids.whole = true;
    }
}

/// **What a journal's sidecar held, taken in for a list restored from it** (by the record store, after the journal's replay, and only for a sidecar
/// whose header is this journal's and this version's): each page is named, if the cap has room, and known, unless it was read already. Nothing is to be
/// written of them again. What a hostile sidecar can do is bounded here too: a page past the file's length is left out.
pub fn restored(edits: &mut PageEdits, pages: &[(u32, Known)], document: Option<DocumentIds>, followed_untold: bool) {
    let file_pages = edits.file_pages();
    let valid: Vec<u32> = pages.iter().map(|(page, _)| *page).filter(|page| *page < file_pages).collect();
    if valid.is_empty() && document.is_none() && !followed_untold {
        return;
    }
    let ids = edits.identities.get_or_insert_with(Box::default);
    ids.name(valid);
    for (page, known) in pages {
        if let Ok(at) = ids.pages.binary_search_by_key(page, |(at, _)| *at)
            && ids.pages[at].1.is_none()
        {
            ids.pages[at].1 = Some(*known);
        }
    }
    if document.is_some() {
        ids.document = document;
    }
    ids.followed_untold = followed_untold;
}

/// **What's waiting to go in the sidecar:** the entries of the pages read since the last write (or all known, when it's to be written whole), encoded.
pub fn sidecar_waiting(edits: &PageEdits) -> Option<sidecar::Waiting> {
    let ids = edits.identities.as_deref()?;
    if !ids.whole && ids.unwritten.is_empty() && !ids.document_unwritten {
        return None;
    }
    let entries: Vec<(u32, Known)> = if ids.whole {
        ids.pages.iter().filter_map(|(page, known)| Some((*page, (*known)?))).collect()
    } else {
        ids.unwritten.iter().filter_map(|page| Some((*page, ids.known(*page)?))).collect()
    };
    let document = (ids.whole || ids.document_unwritten).then_some(ids.document).flatten();
    if entries.is_empty() && document.is_none() && !ids.whole {
        return None;
    }
    Some(sidecar::Waiting {
        whole: ids.whole,
        bytes: sidecar::entries(&entries, document),
        unwritten: ids.unwritten.len(),
        document: ids.document_unwritten,
    })
}

/// **What came of a sidecar write:** what `waiting` held is there (the first `waiting.unwritten` entries of those then waiting, and the `/ID` pair if
/// it was waiting), or the sidecar is to be written whole next time, as what's on disk isn't this list's.
pub fn sidecar_written(edits: &mut PageEdits, waiting: &sidecar::Waiting, done: sidecar::Done) {
    let Some(ids) = edits.identities.as_deref_mut() else { return };
    match done {
        sidecar::Done::Written => {
            ids.unwritten.drain(..waiting.unwritten.min(ids.unwritten.len()));
            if waiting.document {
                ids.document_unwritten = false;
            }
            if waiting.whole {
                ids.whole = false;
            }
        }
        sidecar::Done::WantsWhole => ids.whole = true,
    }
}

/// Whether anything waits for the sidecar, which a list going to disk mustn't leave behind.
pub fn sidecar_caught_up(edits: &PageEdits) -> bool {
    edits
        .identities
        .as_deref()
        .is_none_or(|ids| !ids.whole && ids.unwritten.is_empty() && !ids.document_unwritten)
}

#[cfg(test)]
mod tests;
