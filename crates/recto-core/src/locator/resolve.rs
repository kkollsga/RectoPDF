//! The resolver: where a reference's page is now, said as a status and never as a guess.
//!
//! A reference is hostile input and grants nothing; this reads only the file the caller already holds open. The pages decide the status. A page is
//! found where the reference's hint still holds a page with the same 16-byte digest prefix, found elsewhere ("moved") only when a scan that read
//! every page found exactly one, "ambiguous" when it found several, and "gone" only after every page was read and none matched. A scan stopped by
//! a cap says "incomplete", never "gone", and a nearest page is never offered in place of an answer. Document tags are information beside the
//! status, never part of it. Nothing is kept: no state, cache, thread or event, and no digest or token is ever logged.
//!
//! A hint page with the same digest prefix is `Found` without scanning, whatever the file's version. The token carries no version stamp, so
//! requiring "an unchanged original" would cost a whole scan on every resolve; identical page content retrieves identically.
//!
//! Pages are read only through [`page_id::read_page`](crate::page_id::read_page) and a frame only through `nav::frame_of`; the one other hash here
//! is the content tag of a document, taken over a page digest that was read already.

use super::{DocumentTags, Locator, MAX_PAGE, PageRef, Target};
use crate::page_id::{self, IndexedReader, PageIdentity, PageMap, PageRead};
use sha2::{Digest, Sha256};

/// Most pages listed as candidates in an answer.
pub const MAX_CANDIDATES: usize = 8;
/// Pages, from the first, searched for the page that gives a document its content tag.
pub const TAG_PAGES: usize = 16;

/// What one resolve may spend. [`ScanCaps::STANDARD`] is the definition; another value is for a test to hold a cap at its limit plus one cheaply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanCaps {
    /// Bytes of page reads (as [`PageRead::bytes`] charges them) after which no further page is read. Checked before each read, so one page can
    /// carry the total past it, by at most that page's caps: 320 MiB under [`page_id::Caps::STANDARD`] (stored lengths up to `page_read`,
    /// 256 MiB, plus hashed bytes up to `page_bytes`, 64 MiB, which already includes the content it hashes).
    pub bytes: u64,
    /// Pages read, after which no further page is read.
    pub pages: u32,
    /// The digest's own caps for each page.
    pub page: page_id::Caps,
}

impl ScanCaps {
    /// 1 GiB of page reads and as many pages as a reference can name. The byte cap bounds work (time), not memory: each page streams through
    /// `decode::CHUNK` buffers over the mapped file and nothing is kept per page, so a scan's footprint is the same at any cap (6.2 MiB peak on a
    /// 383-page report). A stream is charged each time a page draws it, so an image or form shared by many pages is charged again on every page, and
    /// a 97 MiB report costs 277 MB to read through; 1 GiB is about 3.9 times the largest measured corpus document's whole read, so a scan of any
    /// ordinary file completes and `gone` can be said. It is not the matcher's `MAX_JOB_BYTES` (256 MiB), which stays what it was.
    pub const STANDARD: Self = Self {
        bytes: 1 << 30,
        pages: MAX_PAGE,
        page: page_id::Caps::STANDARD,
    };
}

/// Whether the file is the document the reference was made in. Information only: the pages decide the status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentMatch {
    /// The `/ID` tag or the content tag is the reference's.
    Same,
    /// The file has a tag, and neither is the reference's.
    Different,
    /// The file has no tag to compare (no `/ID` and no page with an identity among the first [`TAG_PAGES`]).
    Unknown,
}

/// How far the resolve looked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Looked {
    /// The hinted page held the same content (the same 16-byte digest prefix) as the remembered one: "same content at the remembered place". Not proven
    /// unique: no scan ran, so identical pages elsewhere were not looked for. No scan.
    Hint,
    /// Every page was read.
    All,
    /// A cap or an unreadable page stopped it short of every page.
    Part,
}

/// Where the reference's page is. Pages are zero-based file pages, ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// At the hinted page.
    Found { page: u32 },
    /// Not at the hint, and the only page of the file with that digest, after every page was read.
    Moved { page: u32 },
    /// Several pages have that digest; never the nearest to the hint. Lists at most [`MAX_CANDIDATES`]; `complete` is false where more matched.
    Ambiguous { pages: Vec<u32>, complete: bool },
    /// Every page was read and none has that digest.
    Gone,
    /// The scan didn't reach every page, or a page had no identity (it reached a cap). Lists at most one page that did match.
    Incomplete { pages: Vec<u32> },
}

/// What became of the reference's target on the page found. Never a claim about text: characters are not checked here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placed {
    /// The whole page.
    Page,
    /// A spot, as a rectangle in shown points (a point has zero area), on the same frame and inside the page.
    Spot([f32; 4]),
    /// The page's crop or turn is not the reference's, so a place on it would be another place.
    SpotFrameChanged,
    /// Same frame, but the spot lies outside the page.
    SpotOutsidePage,
    /// A character range is named, and nothing here read the page's text to check it.
    CharactersNotChecked { start: u32, end: u32 },
}

/// The whole answer of [`resolve`].
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub status: Status,
    pub document: DocumentMatch,
    /// Whether the page found has the reference's frame tag. `None` unless a page was found.
    pub frame_same: Option<bool>,
    /// `None` unless a page was found.
    pub target: Option<Placed>,
    pub looked: Looked,
    /// Pages read, the document tags' pages included, none twice.
    pub pages_read: u32,
    pub bytes_read: u64,
}

/// Why a page can't be referenced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreferenceable {
    /// The file has no such page, or the number is beyond what a reference can hold.
    NoSuchPage,
    /// The page reached a cap, so it has no identity to refer by.
    NoIdentity,
    /// The document has no tag to say which document it is.
    NoDocumentTag,
    /// A cap on what the referrer may read was spent before the page (or the tags) could be read: no claim is made about the page. Asking again with
    /// more room may succeed, unlike `NoIdentity`.
    Capped,
}

/// A page read under a [`ScanCaps`], and the spending so far. The first [`TAG_PAGES`] reads and the hint's are kept so that the document tags, the
/// hint and the scan never read one twice. Not kept beyond one call.
struct Reads<'a> {
    reader: &'a IndexedReader,
    map: &'a PageMap,
    caps: &'a ScanCaps,
    first: [Option<PageRead>; TAG_PAGES],
    /// One more kept read: the page the caller named with [`Reads::keep`], when it is past the first [`TAG_PAGES`].
    kept: Option<(u32, PageRead)>,
    keep: Option<u32>,
    pages: u32,
    bytes: u64,
}

/// What asking for a page came to.
enum Got {
    /// The file has no such page.
    Absent,
    /// A cap on the whole resolve was spent before this page could be read.
    Capped,
    Read(PageRead),
}

impl<'a> Reads<'a> {
    fn new(reader: &'a IndexedReader, map: &'a PageMap, caps: &'a ScanCaps) -> Self {
        Self {
            reader,
            map,
            caps,
            first: [None; TAG_PAGES],
            kept: None,
            keep: None,
            pages: 0,
            bytes: 0,
        }
    }

    /// Keep the read of page `index` past the first [`TAG_PAGES`], so asking for it again costs nothing: the hint, which the scan meets again.
    fn keep(&mut self, index: u32) {
        self.keep = Some(index);
    }

    /// Page `index` (zero-based): from what was read already, else read now while `bytes` and `pages` allow it.
    fn get(&mut self, index: u32) -> Got {
        let Some(entry) = self.map.get(index as usize) else { return Got::Absent };
        if let Some(read) = self.first.get(index as usize).copied().flatten() {
            return Got::Read(read);
        }
        if let Some((_, read)) = self.kept.filter(|(kept, _)| *kept == index) {
            return Got::Read(read);
        }
        if self.spent_cap() {
            return Got::Capped;
        }
        let read = page_id::read_page(self.reader, entry, &self.caps.page);
        self.pages += 1;
        self.bytes = self.bytes.saturating_add(read.bytes);
        if let Some(slot) = self.first.get_mut(index as usize) {
            *slot = Some(read);
        } else if self.keep == Some(index) {
            self.kept = Some((index, read));
        }
        Got::Read(read)
    }

    /// Whether a cap on the whole is spent, so that no further page is read.
    fn spent_cap(&self) -> bool {
        self.bytes >= self.caps.bytes || self.pages >= self.caps.pages
    }

    /// The tags this document has now: the `/ID` tag, and the content tag from the first of the first [`TAG_PAGES`] pages with an identity.
    fn tags(&mut self) -> (Option<[u8; 8]>, Option<[u8; 8]>) {
        let id = page_id::document_ids(self.reader).map(|ids| first_eight(&ids.first));
        // The first of the tag pages with an identity, stopping at the first page refused for a cap: a later page read already would give another page's tag.
        let mut content = None;
        for index in 0..TAG_PAGES as u32 {
            match self.get(index) {
                Got::Read(PageRead { identity: Some(identity), .. }) => {
                    content = Some(content_tag(&identity));
                    break;
                }
                Got::Read(_) => {}
                Got::Absent | Got::Capped => break,
            }
        }
        (id, content)
    }
}

fn first_eight(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0; 8];
    out.copy_from_slice(&bytes[..8]);
    out
}

/// `SHA-256("recto-doc-c/1\0" ‖ the page's whole digest)`, first eight bytes. A document that opens on the same page as another shares its tag; the
/// tag only suggests which document, and is never used to decide a page.
fn content_tag(identity: &PageIdentity) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(b"recto-doc-c/1\0");
    hasher.update(identity.digest);
    first_eight(&hasher.finalize())
}

/// The file's tags as a reference carries them; `None` where no page among the first [`TAG_PAGES`] has an identity under `caps`.
pub fn document_tags(reader: &IndexedReader, map: &PageMap, caps: &ScanCaps) -> Option<DocumentTags> {
    let (id, content) = Reads::new(reader, map, caps).tags();
    Some(DocumentTags::new(content?, id))
}

/// A reference to the whole of zero-based file page `page`, made from its identity as [`page_id`] reads it. One page: [`Referrer`] is for many.
pub fn reference(reader: &IndexedReader, map: &PageMap, page: u32, caps: &ScanCaps) -> Result<Locator, Unreferenceable> {
    Referrer::new(reader, map, caps).page(page)
}

/// References to whole pages of one file, one after another, under one [`ScanCaps`] for the lot. The tag pages (at most
/// [`TAG_PAGES`] page reads) and a page are read once for all the references to it, so a search with 200 hits on a few pages costs those pages and no more.
/// Nothing outlives it.
pub struct Referrer<'a> {
    reads: Reads<'a>,
    /// The last page asked for, and the answer, so its next reference costs nothing.
    last: Option<(u32, Result<Locator, Unreferenceable>)>,
}

impl<'a> Referrer<'a> {
    pub fn new(reader: &'a IndexedReader, map: &'a PageMap, caps: &'a ScanCaps) -> Self {
        Self {
            reads: Reads::new(reader, map, caps),
            last: None,
        }
    }

    /// A reference to the whole of zero-based file page `page`: the page is read first, then the tags. The same page again costs nothing.
    pub fn page(&mut self, page: u32) -> Result<Locator, Unreferenceable> {
        if let Some((last, made)) = &self.last
            && *last == page
        {
            return *made;
        }
        let made = self.make(page);
        self.last = Some((page, made));
        made
    }

    /// Pages read, and bytes charged for them, so far.
    pub fn spent(&self) -> (u32, u64) {
        (self.reads.pages, self.reads.bytes)
    }

    fn make(&mut self, page: u32) -> Result<Locator, Unreferenceable> {
        if page >= MAX_PAGE {
            return Err(Unreferenceable::NoSuchPage);
        }
        let identity = match self.reads.get(page) {
            Got::Absent => return Err(Unreferenceable::NoSuchPage),
            Got::Capped => return Err(Unreferenceable::Capped),
            Got::Read(PageRead { identity: None, .. }) => return Err(Unreferenceable::NoIdentity),
            Got::Read(PageRead { identity: Some(identity), .. }) => identity,
        };
        let tags = self.tags()?;
        let page = PageRef::new(page + 1, &identity).map_err(|_| Unreferenceable::NoSuchPage)?;
        Ok(Locator::new(tags, page, Target::Page))
    }

    /// The document's tags. The tag pages are memoised by `Reads`, so asking again for each page costs the trailer lookup and no page read.
    fn tags(&mut self) -> Result<DocumentTags, Unreferenceable> {
        let (id, content) = self.reads.tags();
        match content {
            Some(content) => Ok(DocumentTags::new(content, id)),
            // Short of room, "no page had an identity" may only mean the pages were not read: no claim about the document.
            None if self.reads.spent_cap() => Err(Unreferenceable::Capped),
            None => Err(Unreferenceable::NoDocumentTag),
        }
    }
}

/// Where `locator`'s page is in this file. Reads at most what `caps` allow and keeps nothing.
pub fn resolve(locator: &Locator, reader: &IndexedReader, map: &PageMap, caps: &ScanCaps) -> Resolution {
    let mut reads = Reads::new(reader, map, caps);
    let (id, content) = reads.tags();
    let document = compare_documents(locator.document(), id, content);
    let wanted = locator.page().page_check();
    let hint = locator.page().number() - 1;
    reads.keep(hint);

    let mut found = None;
    if let Got::Read(PageRead { identity: Some(identity), .. }) = reads.get(hint)
        && identity.digest[..16] == wanted
    {
        found = Some((hint, identity));
    }
    let (status, looked, page) = match found {
        Some((page, identity)) => (Status::Found { page }, Looked::Hint, Some((page, identity))),
        None => scan(&mut reads, &wanted),
    };
    let (frame_same, target) = match page {
        Some((page, identity)) => {
            let same = identity.frame == locator.page().frame_tag();
            (Some(same), Some(place(locator.target(), same, reader, map, page)))
        }
        None => (None, None),
    };
    Resolution {
        status,
        document,
        frame_same,
        target,
        looked,
        pages_read: reads.pages,
        bytes_read: reads.bytes,
    }
}

fn compare_documents(saved: DocumentTags, id: Option<[u8; 8]>, content: Option<[u8; 8]>) -> DocumentMatch {
    if id.is_none() && content.is_none() {
        return DocumentMatch::Unknown;
    }
    if content == Some(saved.content()) || (id.is_some() && id == saved.id()) {
        DocumentMatch::Same
    } else {
        DocumentMatch::Different
    }
}

/// Reads every page in order, within the caps, for pages whose digest prefix is `wanted`. Returns the status, how far it looked, and the page
/// found when the status names exactly one.
fn scan(reads: &mut Reads, wanted: &[u8; 16]) -> (Status, Looked, Option<(u32, PageIdentity)>) {
    let mut matches = Vec::with_capacity(MAX_CANDIDATES);
    let mut count = 0usize;
    let mut first_identity = None;
    let mut complete = true;
    for index in 0..reads.map.len().min(MAX_PAGE as usize) as u32 {
        match reads.get(index) {
            Got::Absent => break,
            Got::Capped => {
                complete = false;
                break;
            }
            Got::Read(PageRead { identity: None, .. }) => complete = false,
            Got::Read(PageRead { identity: Some(identity), .. }) => {
                if identity.digest[..16] == *wanted {
                    count += 1;
                    first_identity.get_or_insert((index, identity));
                    if matches.len() < MAX_CANDIDATES {
                        matches.push(index);
                    }
                }
            }
        }
        // The answer can only be `Incomplete` now and it already holds its one candidate: further reads cannot change it.
        if !complete && !matches.is_empty() {
            break;
        }
    }
    // A map longer than a reference can name is never read to its end: the pages past it are unlooked at.
    complete &= reads.map.len() <= MAX_PAGE as usize;
    if !complete {
        matches.truncate(1);
        return (Status::Incomplete { pages: matches }, Looked::Part, None);
    }
    match (count, matches.first().copied(), first_identity) {
        (0, ..) => (Status::Gone, Looked::All, None),
        (1, Some(page), Some(found)) => (Status::Moved { page }, Looked::All, Some(found)),
        _ => (
            Status::Ambiguous {
                pages: matches,
                complete: count <= MAX_CANDIDATES,
            },
            Looked::All,
            None,
        ),
    }
}

/// The target on the page found. A spot is placed only on the reference's own frame, inside the page.
fn place(target: Target, same_frame: bool, reader: &IndexedReader, map: &PageMap, page: u32) -> Placed {
    match target {
        Target::Page => Placed::Page,
        Target::Characters(range) => {
            let (start, end) = range.offsets();
            Placed::CharactersNotChecked { start, end }
        }
        Target::Spot(_) if !same_frame => Placed::SpotFrameChanged,
        Target::Spot(spot) => match map.get(page as usize) {
            Some(entry) => spot
                .checked_shown(crate::nav::frame_of(reader, entry))
                .map_or(Placed::SpotOutsidePage, Placed::Spot),
            None => Placed::SpotOutsidePage,
        },
    }
}

#[cfg(test)]
mod tests;
