//! **Which of the file's pages a list's edits name, and what each of those pages is** (the page matcher; owner, 2026-11-13: "we need to run a page
//! matcher of some kind. That's the way we can ensure edits on the correct page"). An edit names its page by its number in the file as it was. When the
//! file changes on disk the old bytes may be gone, so what each named page *is* has to be read while they're there: its digest `recto-page/1`
//! ([`recto_core::page_id`]), its object number and its frame, kept here and in the journal, so the matcher can find the page in the new version.
//!
//! **Which pages:** every page of the tab's own file in a run a step changes, the pages either side of that run in the document, and each of those
//! pages' neighbours in the file, which tell identical pages apart (the coordinator's decision, 2026-11-15). A step notes them before it's applied and
//! its inverse after, so a page moved, the pages it left and the pages it went between are all named. A highlight names its page; a removal of the
//! file's own highlight names the page it's on. Nothing here reads a PDF: an edit stays instant. The pages named are read later, off the UI thread, by
//! the app's page-identity thread, while the file is still the version the edits are about ([`crate::identify`]).
//!
//! **Bounded** (rule 1): a tab with no edits has nothing here; a list holds at most [`MAX_IDENTIFIED`] pages, about 36 bytes each (288 KiB at the cap);
//! pages past it aren't named, which [`Identities::over_cap`] says, and an edit on one of them can't be followed when the file changes. Nothing is
//! held per idle tab: a list asleep is in its journal.

use super::{PageEdits, Step};
use crate::record::JournalOp;
use serde::{Deserialize, Serialize};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    pub digest: [u8; 16],
    pub object: (u32, u16),
    pub frame: u32,
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
    /// More pages were named than [`MAX_IDENTIFIED`] holds, or than could be read in time ([`give_up`]): an edit on a page not named can't follow it.
    over_cap: bool,
    /// The file's `/ID` pair, as read with the pages, if it has one.
    document: Option<recto_core::page_id::DocumentIds>,
}

impl std::fmt::Debug for Identities {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Identities")
            .field("pages", &self.pages.len())
            .field("read", &self.pages.iter().filter(|(_, known)| known.is_some()).count())
            .field("over_cap", &self.over_cap)
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

    pub fn document(&self) -> Option<recto_core::page_id::DocumentIds> {
        self.document
    }

    /// What's known of one page, if it was named and has been read.
    pub fn known(&self, page: u32) -> Option<Known> {
        self.pages.binary_search_by_key(&page, |(at, _)| *at).ok().and_then(|at| self.pages[at].1)
    }

    /// Adds pages, ascending and without repeats, as named and not read, up to the cap.
    fn name(&mut self, mut pages: Vec<u32>) {
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

/// **What was read of the named pages,** taken in and noted for the journal. A page not named (a read of an older wanting) is left out, and one read
/// before stays as it was. Says whether anything was taken in.
pub fn found(edits: &mut PageEdits, pages: &[(u32, Known)], document: Option<recto_core::page_id::DocumentIds>) -> bool {
    let Some(ids) = edits.identities.as_deref_mut() else { return false };
    let mut taken = Vec::new();
    for (page, known) in pages {
        if let Ok(at) = ids.pages.binary_search_by_key(page, |(at, _)| *at)
            && ids.pages[at].1.is_none()
        {
            ids.pages[at].1 = Some(*known);
            taken.push((*page, *known));
        }
    }
    let new_document = document.is_some() && ids.document != document;
    if new_document {
        ids.document = document;
    }
    if taken.is_empty() && !new_document {
        return false;
    }
    let record = IdentifiedRecord {
        pages: taken.iter().map(|(page, known)| PageIdRecord::of(*page, known)).collect(),
        document: ids.document.map(DocumentRecord::of),
    };
    edits.note(|| JournalOp::Identified(record));
    true
}

/// **Reading this file's pages took too long:** the pages named and not read are named no more, and the list says some of its pages couldn't be
/// identified, so nothing reads them again until an edit names them ([`crate::identify::MAX_JOB_TIME`]).
pub fn give_up(edits: &mut PageEdits) {
    if let Some(ids) = edits.identities.as_deref_mut() {
        ids.pages.retain(|(_, known)| known.is_some());
        ids.over_cap = true;
    }
}

/// A journal's line of what was read, replayed: what isn't named, or doesn't read as an identity, is left out, so a hostile journal can do no more than
/// leave pages unread.
pub(super) fn replayed(edits: &mut PageEdits, record: &IdentifiedRecord) {
    let Some(ids) = edits.identities.as_deref_mut() else { return };
    for page in &record.pages {
        if let Some(known) = page.known()
            && let Ok(at) = ids.pages.binary_search_by_key(&page.0, |(at, _)| *at)
        {
            ids.pages[at].1 = Some(known);
        }
    }
    if let Some(document) = record.document.as_ref().and_then(DocumentRecord::ids) {
        ids.document = Some(document);
    }
}

/// Whether operations about to be appended to a journal include what was read of pages, which an app from before can't read
/// ([`crate::record::JOURNAL_VERSION`]).
pub fn ops_have_identities(ops: &[JournalOp]) -> bool {
    ops.iter().any(|op| matches!(op, JournalOp::Identified(_)))
}

/// The list's identities as a record, for its journal's base.
pub(super) fn to_record(edits: &PageEdits) -> Option<IdentitiesRecord> {
    let ids = edits.identities.as_deref()?;
    Some(IdentitiesRecord {
        pages: ids
            .pages
            .iter()
            .map(|(page, known)| match known {
                Some(known) => PageIdRecord::of(*page, known),
                None => PageIdRecord(*page, None),
            })
            .collect(),
        over_cap: ids.over_cap,
        document: ids.document.map(DocumentRecord::of),
    })
}

/// A base's identities, brought back: pages past the file's length, out of order or repeated, or more than the cap, and the whole record goes, as
/// none of it can be trusted to be the list's. Nothing else of the list depends on them.
pub(super) fn from_record(record: Option<&IdentitiesRecord>, file_pages: u32) -> Option<Box<Identities>> {
    let record = record?;
    let ordered = record.pages.windows(2).all(|pair| pair[0].0 < pair[1].0);
    if !ordered || record.pages.len() > MAX_IDENTIFIED || record.pages.iter().any(|page| page.0 >= file_pages) {
        return None;
    }
    Some(Box::new(Identities {
        pages: record.pages.iter().map(|page| (page.0, page.known())).collect(),
        over_cap: record.over_cap,
        document: record.document.as_ref().and_then(DocumentRecord::ids),
    }))
}

/// A list's named pages as its journal's base keeps them. Its `Debug` says how many, never a digest: a digest is of a document's content, and what's
/// printed of a record can reach a log.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitiesRecord {
    #[serde(deserialize_with = "crate::record::capped::<_, _, MAX_IDENTIFIED>")]
    pub pages: Vec<PageIdRecord>,
    #[serde(default)]
    pub over_cap: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentRecord>,
}

/// A journal's line of what was read of named pages. Its `Debug`, like [`IdentitiesRecord`]'s, says how many.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifiedRecord {
    #[serde(deserialize_with = "crate::record::capped::<_, _, MAX_IDENTIFIED>")]
    pub pages: Vec<PageIdRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentRecord>,
}

impl std::fmt::Debug for IdentitiesRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentitiesRecord")
            .field("pages", &self.pages.len())
            .field("read", &self.pages.iter().filter(|page| page.1.is_some()).count())
            .field("over_cap", &self.over_cap)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for IdentifiedRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentifiedRecord")
            .field("pages", &self.pages.len())
            .finish_non_exhaustive()
    }
}

/// One named page: its number in the file (`12`), and, once read, what it is, as one string (`[12,"…"]`): the 16 bytes of its digest, its object
/// number (4 bytes) and generation (2) and its frame tag (4), little-endian, in base64url with no padding, 35 characters. Compact because a journal
/// gets it for every page an edit names (the SSD-wear budget, `recto-cli`'s `it_writes_only_what_happened`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "PageIdWire", into = "PageIdWire")]
pub struct PageIdRecord(pub u32, pub Option<String>);

/// [`PageIdRecord`] as it's written: the page alone, or the page and what it is.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum PageIdWire {
    Named(u32),
    Read(u32, String),
}

impl From<PageIdWire> for PageIdRecord {
    fn from(wire: PageIdWire) -> Self {
        match wire {
            PageIdWire::Named(page) => Self(page, None),
            PageIdWire::Read(page, packed) => Self(page, Some(packed)),
        }
    }
}

impl From<PageIdRecord> for PageIdWire {
    fn from(record: PageIdRecord) -> Self {
        match record.1 {
            None => Self::Named(record.0),
            Some(packed) => Self::Read(record.0, packed),
        }
    }
}

/// Bytes in a page's packed identity.
const PACKED: usize = 26;

impl PageIdRecord {
    fn of(page: u32, known: &Known) -> Self {
        let mut packed = [0u8; PACKED];
        packed[..16].copy_from_slice(&known.digest);
        packed[16..20].copy_from_slice(&known.object.0.to_le_bytes());
        packed[20..22].copy_from_slice(&known.object.1.to_le_bytes());
        packed[22..].copy_from_slice(&known.frame.to_le_bytes());
        Self(page, Some(base64url(&packed)))
    }

    /// What's known of the page, if it's been read and its string is a packed identity.
    fn known(&self) -> Option<Known> {
        let packed = unbase64url::<PACKED>(self.1.as_deref()?)?;
        let at = |range: std::ops::Range<usize>| packed.get(range);
        Some(Known {
            digest: at(0..16)?.try_into().ok()?,
            object: (
                u32::from_le_bytes(at(16..20)?.try_into().ok()?),
                u16::from_le_bytes(at(20..22)?.try_into().ok()?),
            ),
            frame: u32::from_le_bytes(at(22..26)?.try_into().ok()?),
        })
    }
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (at, byte)| n | (u32::from(*byte) << (16 - 8 * at)));
        for at in 0..=chunk.len() {
            out.push(char::from(BASE64URL[((n >> (18 - 6 * at)) & 63) as usize]));
        }
    }
    out
}

/// Exactly `N` bytes from base64url with no padding, or nothing: any other length or character isn't one.
fn unbase64url<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N.div_ceil(3) * 4 - (3 - N % 3) % 3 {
        return None;
    }
    let value = |c: u8| BASE64URL.iter().position(|b| *b == c).map(|at| at as u32);
    let mut out = [0u8; N];
    let mut written = 0;
    for chunk in text.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (at, c) in chunk.iter().enumerate() {
            n |= value(*c)? << (18 - 6 * at);
        }
        for at in 0..chunk.len() - 1 {
            *out.get_mut(written)? = (n >> (16 - 8 * at)) as u8;
            written += 1;
        }
    }
    (written == N).then_some(out)
}

/// A document's `/ID` pair, as two tags of 32 hex digits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRecord(pub String, pub String);

impl DocumentRecord {
    fn of(ids: recto_core::page_id::DocumentIds) -> Self {
        Self(hex(&ids.first), hex(&ids.second))
    }

    fn ids(&self) -> Option<recto_core::page_id::DocumentIds> {
        Some(recto_core::page_id::DocumentIds {
            first: unhex(&self.0)?,
            second: unhex(&self.1)?,
        })
    }
}

fn hex(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 16];
    for (at, pair) in bytes.chunks(2).enumerate() {
        out[at] = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests;
