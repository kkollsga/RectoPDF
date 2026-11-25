//! **What a page is, by its content: the digest `recto-page/1`,** the identity the page matcher follows unsaved edits by when their file changes on disk
//! (owner, 2026-11-13: "we need to run a page matcher of some kind. That's the way we can ensure edits on the correct page"), and the one the locator's
//! page reference will carry (PLAN 8.1). **This module doc, [`decode`]'s and [`scan`]'s are its one definition, and the golden vectors in this module's
//! tests (made files, run everywhere CI runs) are that definition as bytes, from 2026-11-15 on.** A change to what goes in is a new name
//! (`recto-page/2`), never an edit of this one: what's kept of it (the sidecars of journals, and later the corpus and tokens) compares digests made by
//! older builds.
//!
//! **What's hashed,** SHA-256 over, in order (`u32le` and `u64le` are little-endian integers):
//! 1. The bytes `recto-page/1` and a NUL.
//! 2. **Content:** `C`, `u32le(count)`, and a record for each entry of the page's `/Contents` (a stream, or an array of them; a reference to either
//!    followed once; a nested array is an entry that isn't a stream). A record is a kind byte, `u64le(n)` and the SHA-256 of those `n` bytes. Kinds:
//!    0, decoded through the general filters ([`decode`]; a stream with no filter is its bytes); 1, hashed as stored, as a filter isn't general, its
//!    parameters aren't taken, or it didn't decode; 2, not a stream or not readable (`n` is 0).
//! 3. **Images and forms the content draws:** `X`, `u32le(count)`, and a record for each name the content gives `Do` ([`scan`], over the decoded bytes
//!    of the kind-0 entries, in order of first use), looked up in the page's effective `/Resources` `/XObject` (its own, or the nearest ancestor's). A
//!    record is `u32le(length of the name)`, the name, a kind byte, and for kinds 0, 1 and 3 `u64le(n)` and the SHA-256 of the `n` bytes; then
//!    `u32le(children)` and a record (of the same shape, without children) for each name a form's own content gives `Do`, looked up in the form's
//!    `/Resources` `/XObject` (the page's if the form has no `/Resources`; a `/Resources` without `/XObject` is a lookup in nothing, never the
//!    page's). Kinds: 0, every filter general, decoded (for a form, its content, whose names are its children); 1, a filter not general (an image
//!    codec), as stored; 2, nothing by that name, or it isn't a stream or can't be read (no length or hash, and no children); 3, general but it didn't
//!    decode, as stored, and for a form no children (what its content's first part drew before the error is taken back); 4, **an image past its
//!    cap** (`/Subtype /Image`, whose bytes as kind 0, 1 or 3 would hash more than [`Caps::xobject_bytes`], 16 MiB): the byte 4, the kind it would
//!    have been, `u64le(n)` and the SHA-256 of exactly its first `n` = 16 MiB (decoded or as stored, as that kind is), then `u64le` of its stream's
//!    stored length, then its `/Width`, `/Height` and `/BitsPerComponent`, its `/ColorSpace`'s name and its `/Filter` names as written, each framed
//!    (`image_says`). Only a form drawn by the page's content has children; one drawn by a form is a record with none.
//!
//!    **Said exactly, where code alone said it before** (second review): a stream's *stored length* is the checked length of its bytes as lopdf's
//!    reader frames the stream in the file (`encoded_len`; a stream it can't frame safely has none and is kind 2), never `/Length` as written; an
//!    image's `/Width`, `/Height`, `/BitsPerComponent`, `/ColorSpace` and `/Filter` are taken with references resolved (an indirect `/Width 7 0 R`
//!    is its integer); a name is looked up by the bytes the scan took ([`scan`]: a `#xx` is its byte; a lone `#` is kept as it is, where lopdf ended
//!    the resource's key, so it finds nothing: kind 2); and an image that needs its 16 MiB when the page has less than that left of its 64 MiB is
//!    the page's cap reached, not a kind 4.
//!
//!    **The honest limit of kind 4** (coordinator, 2026-11-15): two images equal in their first 16 MiB, their stored length and what they say of
//!    themselves, and different after, hash alike. Big scans often share their first megabytes (a white margin, a header), which is why the stored
//!    length and the geometry are in it: they make look-alikes as unlikely as for whole content, not impossible.
//! 4. `E`.
//!
//! **A page that reaches a cap has no identity** ([`read_page`] gives `None`), but for an image past its own cap (kind 4, a definition of its own): so a
//! digest never depends on where a cap fell, on how much a decoder read ahead or on how a library buffers, only on the file. The caps,
//! [`Caps::STANDARD`], each counted exactly (one byte past is past): more than 1,024 entries of `/Contents`; more than 16 MiB hashed of the content
//! together; more than 256 records of images and forms (drawn by the page and by its forms, together; the scan keeps at most 256 names); more than
//! 16 MiB hashed of one form; more than 64 MiB hashed in all; more than 256 MiB of stored length of the streams opened (each charged its length when
//! it's opened, before a byte is read, and again if it's read again as stored); and in a file anyone may open (encrypted with an empty password), a
//! stream over 8 MiB stored, as the reader gives such a stream only whole, decrypted. So a hostile page costs at most the caps, holding a few buffers
//! of [`decode::CHUNK`], a predictor's row and the scan's names (256 of at most 127 bytes).
//!
//! **What a page holds at once:** what the digest needs of what it has resolved is small facts, and a whole parsed object (a dictionary, an array: up to
//! the reader's object limit, and an object stream's members aren't under even that) lives only in a single slot, dropped before the next is resolved,
//! so one object is parsed at a time, as it was before any of it was kept. A record kept for the page is its tail (at most 1 KiB, else it's read again),
//! the names a form drew and, for each, the id it found in the form's lookup, never a dictionary; a `/Resources` reference is one small entry saying what
//! its `/XObject` is; one `/XObject` table and one gave-up stream's descriptor are held, each until the next one is needed; and a read's references
//! keep at most 16 values of at most 4 KiB. A page's own `/XObject` is the one table held for the whole page. Together a page holds a fixed number of whole
//! objects (the page dictionary, the page's `/XObject` table, one form's table, one descriptor, and the one being parsed) plus under 0.5 MiB of facts.
//!
//! **What's left out, deliberately:** `/Rotate`, `/MediaBox`, `/CropBox` (a turn or a crop is a view of the same page: our own turn keeps the identity),
//! `/Annots` (a highlight added or taken out keeps it), fonts (an optimiser's re-subsetting keeps it), XObjects in the resources the content doesn't
//! draw (documents sharing one `/Resources` at the page tree's top would otherwise put every image in every page's digest), `/StructParents`,
//! `/Group`, metadata, and the page object's number. The frame the page is shown in is said apart from the digest, by [`PageIdentity::frame`], so a
//! highlight isn't carried onto a page whose crop or turn another program changed.
//!
//! **Changed from the research's sketch** (`docs/research/2026-11-14-locator.md` §5.2) before anything kept a digest: each stream is a record of its
//! kind, length and own hash (a stream is read a piece at a time, so its length isn't known first); every section and form is framed with counts; only
//! what the content draws is hashed; and every path is under the page's caps, where the sketch's path for a stream that didn't decode had none (292
//! MiB hashed for the one-page USGS map, in a prototype on 2026-11-15).
//!
//! **Where it runs:** over the lazy `lopdf::IndexedReader`, with no PDFium and no worker, holding nothing after it returns, on the app's page-identity
//! thread, never the UI thread. A file that needed the person's password is never given here: its edits are held as before (`recto-session`).

mod decode;
mod scan;

use lopdf::{Dictionary, Object, ObjectId, PageMapEntry};
/// The lazy reader and page map the digest reads through, for a caller that keeps a file open across reads ([`open_version_for_identity`]).
pub use lopdf::{IndexedReader, PageMap};
use scan::DoScan;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::rc::Rc;

/// The digest's name, and the first bytes hashed.
pub const DIGEST_NAME: &str = "recto-page/1";

/// The digest's caps. [`Caps::STANDARD`] is the definition; another value is only for a test to hold a cap at its limit plus one cheaply, and gives
/// digests that are not `recto-page/1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub content_streams: usize,
    pub content_bytes: u64,
    pub xobjects: usize,
    pub xobject_bytes: u64,
    pub page_bytes: u64,
    pub page_read: u64,
}

impl Caps {
    pub const STANDARD: Self = Self {
        content_streams: 1024,
        content_bytes: 16 << 20,
        xobjects: scan::MAX_NAMES,
        xobject_bytes: 16 << 20,
        page_bytes: 64 << 20,
        page_read: 256 << 20,
    };
}

/// One page's identity: its digest, the page object's number as a fast first try (our own appends keep it; other programs mostly renumber, research
/// §3), and its frame's tag. Its `Debug` says neither digest nor tag: a digest is of a document's content, and what's printed can reach a log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PageIdentity {
    /// `recto-page/1`, whole. What's kept is a prefix.
    pub digest: [u8; 32],
    pub object: (u32, u16),
    /// **The frame's tag:** the first four bytes of SHA-256 over `recto-frame/1`, a NUL, the file's own quarter turns and the shown box (left, bottom,
    /// right, top as `f32` little-endian), as [`crate::nav`] reads them, as a `u32le`, with its low two bits then replaced by the page's own quarter
    /// turns (`/Rotate`, 0 to 3): `(hash & !3) | turns`. Equal frames, equal tags; [`frame_turns`] gives the turns back, so a change of the crop
    /// (same turns, another tag) is told from a change of the page's own `/Rotate` (slice 2). Not part of the digest. Kept only in a journal's
    /// sidecar, whose format's version (2) is the one that holds tags of this definition: a sidecar of version 1, whose tags' low bits are hash, is
    /// no sidecar, so a tag of the old definition is never compared with one of this.
    pub frame: u32,
}

impl std::fmt::Debug for PageIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PageIdentity").field("object", &self.object).finish_non_exhaustive()
    }
}

/// What reading one page came to: its identity, or `None` where it reached a cap; and the bytes it cost (stored lengths charged and bytes hashed), for a
/// caller that reads many pages to a budget of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRead {
    pub identity: Option<PageIdentity>,
    pub bytes: u64,
}

/// What's said of a document as a whole: a 16-byte SHA-256 prefix of each of its trailer's `/ID` strings, where it has two. Tells a re-save (`/ID[0]`
/// kept) from another document, which the page matcher goes by (`recto-session`'s `edit::matcher`): a page is taken by its old object number alone,
/// or by a digest found once with no neighbour agreeing, only in a version with the same `/ID[0]`. Its `Debug` says nothing of the bytes: they're of
/// a document, and what's printed can reach a log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DocumentIds {
    pub first: [u8; 16],
    pub second: [u8; 16],
}

impl std::fmt::Debug for DocumentIds {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("DocumentIds").finish_non_exhaustive()
    }
}

/// **Opens exactly this version of a file for its pages' identities,** through the file wall, as every read of a document by its path is: `None`
/// unless the file opened (by its own descriptor, not its path) has the device, number, size and modification time of `stamp`, so a file renamed into
/// place for the moment of the open is never identified for another (slice 2's review). A file that needs a password is refused: its pages are never
/// identified. The only open by a path: there's none that skips the opened file's check (second review).
pub fn open_version_for_identity(path: &Path, stamp: recto_platform::FileStamp) -> Option<IndexedReader> {
    crate::open_reader_cached_at(path, stamp).ok().flatten()
}

/// [`open_version_for_identity`] on bytes in memory, for tests and the fuzzer.
pub fn open_bytes_for_identity(bytes: std::sync::Arc<[u8]>) -> Result<IndexedReader, crate::OpenError> {
    crate::open_bytes(bytes)
}

/// The identities of these pages (zero-based) of a document, in the order asked: `None` for a page it doesn't have, or one with no identity as it
/// reached a cap.
pub fn identify_pages(reader: &IndexedReader, map: &PageMap, pages: &[u32]) -> Vec<Option<PageIdentity>> {
    pages
        .iter()
        .map(|page| map.get(*page as usize).and_then(|entry| identify_page(reader, entry, &Caps::STANDARD)))
        .collect()
}

/// One page's identity under `caps`: `None` where it reached one.
pub fn identify_page(reader: &IndexedReader, entry: &PageMapEntry, caps: &Caps) -> Option<PageIdentity> {
    read_page(reader, entry, caps).identity
}

/// One page's identity under `caps`, and what reading it cost.
pub fn read_page(reader: &IndexedReader, entry: &PageMapEntry, caps: &Caps) -> PageRead {
    let mut budget = Budget {
        caps,
        page_left: caps.page_bytes,
        content_left: caps.content_bytes,
        read_left: caps.page_read,
        records: 0,
        cost: 0,
    };
    let digest = digest(reader, entry, &mut budget).ok();
    let id = entry.id();
    PageRead {
        identity: digest.map(|digest| PageIdentity {
            digest,
            object: (id.0, id.1),
            frame: frame_tag(crate::nav::frame_of(reader, entry)),
        }),
        bytes: budget.cost,
    }
}

/// The document's `/ID` pair, if it has one of two strings.
pub fn document_ids(reader: &IndexedReader) -> Option<DocumentIds> {
    let ids = reader.trailer_entry_owned(b"ID").ok()??;
    let Object::Array(ids) = ids else { return None };
    let [Object::String(first, _), Object::String(second, _)] = ids.as_slice() else {
        return None;
    };
    let tag = |bytes: &[u8]| -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(b"recto-id/1\0");
        hasher.update(bytes);
        hasher.finalize()[..16].try_into().unwrap_or([0; 16])
    };
    Some(DocumentIds {
        first: tag(first),
        second: tag(second),
    })
}

fn frame_tag(frame: crate::nav::Frame) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(b"recto-frame/1\0");
    hasher.update([frame.turns]);
    for edge in [frame.left, frame.bottom, frame.right, frame.top] {
        hasher.update(edge.to_le_bytes());
    }
    let tag = hasher.finalize();
    (u32::from_le_bytes([tag[0], tag[1], tag[2], tag[3]]) & !3) | u32::from(frame.turns & 3)
}

/// The page's own quarter turns (its `/Rotate`, 0 to 3) a frame's tag holds in its low two bits ([`PageIdentity::frame`]).
pub fn frame_turns(tag: u32) -> u8 {
    (tag & 3) as u8
}

/// **For the fuzzer:** `input` as a stream's `/Filter`, `/DecodeParms` and bytes, decoded as the digest decodes ([`decode`]) up to `limit` bytes, the
/// decoded bytes and the raw ones fed to the `Do` scan ([`scan`]). Says how many bytes it decoded (`None` where the chain isn't taken or didn't decode)
/// and how many names the scans kept. Every decoder, predictor and parameter form is reachable from it: byte 0 is the count of filters (to 9, one more
/// than taken), a byte each picks one ([`decode::FUZZ_NAMES`]), the next the form of the parameters, then each filter's `/Predictor`, `/Colors`,
/// `/BitsPerComponent`, `/Columns` (`u16le`) and `/EarlyChange`; the rest is the stream.
#[doc(hidden)]
pub fn fuzz_stream(input: &[u8], limit: u64) -> (Option<u64>, usize) {
    let (dict, data) = decode::fuzz_dictionary(input);
    let mut names = DoScan::default();
    names.feed(data);
    let mut kept = names.names().len();
    let Some(chain) = decode::chain_of(&dict, &|object| Some(object.clone())) else {
        return (None, kept);
    };
    let mut source = decode::decoded(Box::new(data), &chain);
    let mut scan = DoScan::default();
    let mut buf = vec![0u8; decode::CHUNK];
    let mut n = 0u64;
    while n <= limit {
        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => {
                n += read as u64;
                scan.feed(&buf[..read]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return (None, kept + scan.names().len()),
        }
    }
    scan.end_of_stream();
    kept += scan.names().len();
    (Some(n), kept)
}

/// A cap was reached: the page has no identity.
struct Capped;

/// What's left of a page's caps as it's hashed, and what it has cost.
struct Budget<'c> {
    caps: &'c Caps,
    page_left: u64,
    content_left: u64,
    read_left: u64,
    /// Records of images and forms so far.
    records: usize,
    /// Stored lengths charged and bytes hashed: [`PageRead::bytes`].
    cost: u64,
}

impl Budget<'_> {
    /// One more record of an image or form, within the cap.
    fn record(&mut self) -> Result<(), Capped> {
        self.records += 1;
        if self.records > self.caps.xobjects { Err(Capped) } else { Ok(()) }
    }
}

fn digest(reader: &IndexedReader, entry: &PageMapEntry, budget: &mut Budget<'_>) -> Result<[u8; 32], Capped> {
    let mut hasher = Sha256::new();
    hasher.update(DIGEST_NAME.as_bytes());
    hasher.update([0]);
    let page = dict_at(reader, entry.id());
    let contents = page.as_ref().and_then(|page| page.get(b"Contents").ok().cloned());
    let entries = content_entries(reader, contents);
    if entries.len() > budget.caps.content_streams {
        return Err(Capped);
    }
    hasher.update(b"C");
    hasher.update(count(entries.len()));
    let mut scan = DoScan::default();
    for item in &entries {
        content_record(reader, item, budget, &mut hasher, &mut scan)?;
    }
    drop(entries);
    drop(page);
    if scan.overflowed() {
        return Err(Capped);
    }
    let page_xobjects = entry
        .inherited()
        .resources()
        .and_then(|owner| dict_at(reader, owner))
        .and_then(|owner| resolved_dict(reader, owner.get(b"Resources").ok()?))
        .and_then(|resources| resolved_dict(reader, resources.get(b"XObject").ok()?));
    let mut drawn = Drawn::new(page_xobjects);
    hasher.update(b"X");
    hasher.update(count(scan.names().len()));
    for name in scan.names() {
        budget.record()?;
        let found = drawn.in_page(name);
        let Some(seen) = xobject_record(reader, found, name, budget, &mut hasher, &mut drawn, true)? else {
            hasher.update(count(0));
            continue;
        };
        if seen.overflowed {
            return Err(Capped);
        }
        hasher.update(count(seen.names.len()));
        for (child, found) in seen.names.iter().zip(&seen.kids) {
            budget.record()?;
            xobject_record(reader, *found, child, budget, &mut hasher, &mut drawn, false)?;
        }
    }
    hasher.update(b"E");
    Ok(hasher.finalize().into())
}

/// A count as it's hashed: every count here is under a cap far below `u32::MAX`.
fn count(n: usize) -> [u8; 4] {
    u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes()
}

/// The entries of `/Contents`: one for a stream, each element for an array (a reference to an array followed once).
fn content_entries(reader: &IndexedReader, contents: Option<Object>) -> Vec<Object> {
    match contents {
        None => Vec::new(),
        Some(Object::Array(items)) => items,
        Some(Object::Reference(id)) => {
            // A stream is never resolved whole: only what isn't one is, as an array.
            if reader.resolve_stream_descriptor(id).is_ok() {
                vec![Object::Reference(id)]
            } else {
                match reader.resolve_object(id) {
                    Ok(Object::Array(items)) => items,
                    _ => vec![Object::Reference(id)],
                }
            }
        }
        Some(other) => vec![other],
    }
}

/// One entry of the content, hashed, with the names it draws fed to `scan` if it decoded.
fn content_record(reader: &IndexedReader, item: &Object, budget: &mut Budget<'_>, hasher: &mut Sha256, scan: &mut DoScan) -> Result<(), Capped> {
    let readable = match item {
        Object::Reference(id) => reader.resolve_stream_descriptor(*id).ok().map(|descriptor| (*id, descriptor)),
        _ => None,
    };
    let Some((id, descriptor)) = readable else {
        record(hasher, 2, 0, Sha256::new());
        return Ok(());
    };
    let limit = budget.content_left;
    let refs = Refs::new(reader);
    let chain = decode::chain_of(descriptor.dictionary(), &|object| refs.deref(object));
    let mark = scan.mark();
    let decoded = match &chain {
        Some(chain) => take_stream(reader, id, &descriptor, Some(chain), limit, budget, Some(scan), false)?,
        None => Taken::Failed,
    };
    let (kind, taken) = match decoded {
        Taken::Failed => {
            // What it drew is what it decoded to, which it didn't.
            scan.back_to(mark);
            (1, take_stream(reader, id, &descriptor, None, limit, budget, None, false)?)
        }
        taken => (0, taken),
    };
    scan.end_of_stream();
    match taken {
        Taken::Hashed(n, sub) => {
            budget.content_left -= n;
            record(hasher, kind, n, sub);
        }
        Taken::Prefix(..) | Taken::Failed => record(hasher, 2, 0, Sha256::new()),
    }
    Ok(())
}

/// What a page has drawn, kept while it's hashed: small facts by construction, and at most one resolved table and one descriptor at a time.
struct Drawn {
    /// Every image and form this page has drawn, by object: what each one's record said after its name and what reading it cost, so a stream drawn under
    /// many names is read once. Per page and dropped with it, with an entry for each record the page makes at most ([`Caps::xobjects`]).
    seen: HashMap<ObjectId, Rc<Seen>>,
    /// The page's own `/XObject` table, held for the page as it always was.
    page: Option<Dictionary>,
    /// Each `/Resources` reference a form named: what its `/XObject` is, reduced (the dictionary itself isn't kept).
    resources: HashMap<ObjectId, Res>,
    /// The last table resolved, kept until another is needed.
    table: Option<(Table, Option<Rc<Dictionary>>)>,
    /// The last gave-up stream's descriptor, kept for its re-read.
    kept: Option<(ObjectId, Rc<lopdf::IndexedStreamDescriptor>)>,
}

/// What a `/Resources` reference's `/XObject` came to, for a form that names it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Res {
    /// Not a dictionary: the form has no resources of its own, so the page's are used.
    Page,
    /// A dictionary with no `/XObject` that is a table: a name is found nowhere.
    Nothing,
    /// A table by reference, at this object.
    Table(ObjectId),
    /// A table written in the `/Resources` dictionary itself, taken out of it (the table is read again from there if it's needed after another).
    Inline,
}

/// Where the one table kept in [`Drawn::table`] came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Table {
    /// The table at this object.
    At(ObjectId),
    /// The `/XObject` written in the `/Resources` at this object.
    In(ObjectId),
}

/// Where a form's names are looked up while its record is made.
enum Lookup<'d> {
    Page,
    Nothing,
    Table(Option<Rc<Dictionary>>),
    /// A table written in the form's own `/Resources`, which that form's descriptor holds.
    Here(&'d Dictionary),
}

/// The object `name` is in `table`, if it's a reference to one.
fn found_in(table: Option<&Dictionary>, name: &[u8]) -> Option<ObjectId> {
    match table?.get(name).ok()? {
        Object::Reference(id) => Some(*id),
        _ => None,
    }
}

impl Drawn {
    /// The page's own `/XObject` is the one table held for the whole page.
    fn new(page: Option<Dictionary>) -> Self {
        Self {
            seen: HashMap::new(),
            page,
            resources: HashMap::new(),
            table: None,
            kept: None,
        }
    }

    /// The object the page's own table gives for `name`.
    fn in_page(&self, name: &[u8]) -> Option<ObjectId> {
        found_in(self.page.as_ref(), name)
    }

    /// The table `key` names, resolved once and kept until another is needed: forms sharing a table share the copy while they're drawn in a row, and a
    /// table is never held while the next one is resolved.
    fn table(&mut self, reader: &IndexedReader, key: Table) -> Option<Rc<Dictionary>> {
        if let Some((kept, table)) = &self.table
            && *kept == key
        {
            return table.clone();
        }
        self.table = None;
        let table = match key {
            Table::At(x) => resolved_dict(reader, &Object::Reference(x)),
            Table::In(r) => match resolved_dict(reader, &Object::Reference(r)).and_then(|mut res| res.remove(b"XObject")) {
                Some(Object::Dictionary(table)) => Some(table),
                _ => None,
            },
        }
        .map(Rc::new);
        self.table = Some((key, table.clone()));
        table
    }

    /// What each of `names` finds in the lookup a form's `resources` gives (`None`: the page's, as the form has none of its own). A `/Resources`
    /// reference is resolved once per page into a [`Res`], so forms sharing one parse it once however big it is.
    fn kids(&mut self, reader: &IndexedReader, resources: Option<&Object>, names: &[Vec<u8>]) -> Vec<Option<ObjectId>> {
        let lookup = match resources {
            None => Lookup::Page,
            Some(Object::Reference(r)) => {
                let res = match self.resources.get(r) {
                    Some(res) => *res,
                    None => {
                        // A remembered failure includes a transient I/O error, which reads as a dictionary that isn't one. Under I/O errors an identity is
                        // already undefined (the same file may digest differently), so this adds no case.
                        // A table is never held while the next is resolved.
                        self.table = None;
                        let (res, inline) = match resolved_dict(reader, &Object::Reference(*r)) {
                            None => (Res::Page, None),
                            Some(mut dict) => match dict.remove(b"XObject") {
                                Some(Object::Reference(x)) => (Res::Table(x), None),
                                Some(Object::Dictionary(table)) => (Res::Inline, Some(table)),
                                _ => (Res::Nothing, None),
                            },
                        };
                        if let Some(table) = inline {
                            self.table = Some((Table::In(*r), Some(Rc::new(table))));
                        }
                        self.resources.insert(*r, res);
                        res
                    }
                };
                match res {
                    Res::Page => Lookup::Page,
                    Res::Nothing => Lookup::Nothing,
                    Res::Table(x) => Lookup::Table(self.table(reader, Table::At(x))),
                    Res::Inline => Lookup::Table(self.table(reader, Table::In(*r))),
                }
            }
            Some(Object::Dictionary(res)) => match res.get(b"XObject").ok() {
                Some(Object::Reference(x)) => Lookup::Table(self.table(reader, Table::At(*x))),
                Some(Object::Dictionary(table)) => Lookup::Here(table),
                _ => Lookup::Nothing,
            },
            Some(_) => Lookup::Page,
        };
        names
            .iter()
            .map(|name| match &lookup {
                Lookup::Page => self.in_page(name),
                Lookup::Nothing => None,
                Lookup::Table(table) => found_in(table.as_deref(), name),
                Lookup::Here(table) => found_in(Some(table), name),
            })
            .collect()
    }
}

/// What one read of an image or form came to.
struct Seen {
    /// The bytes it hashed after its name: from the kind byte through [`image_says`].
    tail: Vec<u8>,
    /// What the read took off [`Budget::read_left`] and [`Budget::page_left`], and added to [`Budget::cost`].
    read: u64,
    page: u64,
    cost: u64,
    /// The read gave up on a decode on the way (a kind 3 whose decode failed, or a stream that couldn't be read at all). A failed decode is exact for any
    /// limit at least the one it was read at: it ended in an error before reaching it. With a smaller limit it can end in a cap instead, which is no
    /// identity. So it's taken as it was only while the page has more left than any one stream may take (the limit is then the stream's own, as it was
    /// or more), and read again, with its descriptor kept, where the page has less.
    headroom: bool,
    is_form: bool,
    /// The read had a scan to feed, as a form drawn by the page does. One made for a form's own children has no names to take.
    scanned: bool,
    /// The names a form drew, and whether the scan of them overflowed.
    names: Vec<Vec<u8>>,
    /// What each name found in the form's own lookup, in the order of `names`: the object it names, as an id and never a dictionary. Empty where the form
    /// drew nothing or overflowed. Worked out while the form's descriptor is in hand, so the form's `/Resources` isn't kept for later.
    kids: Vec<Option<ObjectId>>,
    overflowed: bool,
}

/// A record whose tail is longer than this isn't kept for the page: it's read again if drawn again. Only a kind 4 (an image past its cap, with what it says
/// of itself) can be longer, and a page draws at most [`Caps::xobjects`] records, so at most 4 of them are read again for the page's whole cap.
const MAX_KEPT_TAIL: usize = 1024;

impl Seen {
    /// What a read that cost nothing and drew nothing came to: its record's bytes, and that it's no form with children.
    fn of(tail: Vec<u8>, scanned: bool) -> Self {
        Self {
            tail,
            read: 0,
            page: 0,
            cost: 0,
            headroom: false,
            is_form: false,
            scanned,
            names: Vec::new(),
            kids: Vec::new(),
            overflowed: false,
        }
    }
}

/// One image's or form's record, found by `name` in `xobjects`, hashed after the name: taken from the memo where this object was drawn already under
/// another name (its stream isn't resolved or read again, and what the read cost is charged again), else read. Gives back what was seen where it's an
/// object at all. `children` says the caller takes the names a form drew, so a form read without them isn't taken from the memo for it.
fn xobject_record(
    reader: &IndexedReader,
    found: Option<ObjectId>,
    name: &[u8],
    budget: &mut Budget<'_>,
    hasher: &mut Sha256,
    drawn: &mut Drawn,
    children: bool,
) -> Result<Option<Rc<Seen>>, Capped> {
    hasher.update(count(name.len()));
    hasher.update(name);
    let Some(id) = found else {
        hasher.update([2]);
        return Ok(None);
    };
    if let Some(seen) = drawn.seen.get(&id) {
        let wanted = !(children && seen.is_form && !seen.scanned);
        // Where it can't be taken as it was, it's read again, so a cap is reached as it always was and costs what it did.
        let affords = seen.read <= budget.read_left && seen.page <= budget.page_left && (!seen.headroom || budget.page_left > budget.caps.xobject_bytes);
        if wanted && affords {
            budget.read_left -= seen.read;
            budget.page_left -= seen.page;
            budget.cost = budget.cost.saturating_add(seen.cost);
            hasher.update(&seen.tail);
            return Ok(Some(Rc::clone(seen)));
        }
    }
    // What was kept of an earlier read that has to be made again isn't resolved again: its dictionary is the same.
    let kept = drawn.kept.as_ref().filter(|(kept, _)| *kept == id).map(|(_, descriptor)| Rc::clone(descriptor));
    #[cfg(test)]
    if kept.is_none() {
        tests::DESCRIPTORS.with(|count| count.set(count.get() + 1));
    }
    if kept.is_none() {
        drawn.kept = None;
    }
    let Some(descriptor) = kept.or_else(|| reader.resolve_stream_descriptor(id).ok().map(Rc::new)) else {
        // Resolving it parsed its whole dictionary, and what it comes to doesn't depend on what's left of any cap: it's kept like any other record.
        // A remembered failure includes a transient I/O error, which reads as a stream that isn't one: under I/O errors identity is already undefined (the
        // same file can digest differently), so remembering it adds no case.
        let seen = Rc::new(Seen::of(vec![2], children));
        hasher.update(&seen.tail);
        drawn.seen.insert(id, Rc::clone(&seen));
        return Ok(Some(seen));
    };
    let before = (budget.read_left, budget.page_left, budget.cost);
    let mut tail = Vec::new();
    let mut scan = DoScan::default();
    let read = read_xobject(reader, id, &descriptor, budget, &mut tail, children.then_some(&mut scan))?;
    scan.end_of_stream();
    let overflowed = read.is_form && scan.overflowed();
    let names = scan.names().to_vec();
    let kids = if overflowed || names.is_empty() {
        Vec::new()
    } else {
        drawn.kids(reader, descriptor.dictionary().get(b"Resources").ok(), &names)
    };
    let seen = Rc::new(Seen {
        read: before.0 - budget.read_left,
        page: before.1 - budget.page_left,
        cost: budget.cost.saturating_sub(before.2),
        headroom: read.gave_up,
        is_form: read.is_form,
        scanned: children,
        overflowed,
        names,
        kids,
        tail,
    });
    if read.gave_up {
        drawn.kept = Some((id, descriptor));
    }
    hasher.update(&seen.tail);
    if seen.tail.len() <= MAX_KEPT_TAIL {
        drawn.seen.insert(id, Rc::clone(&seen));
    }
    Ok(Some(seen))
}

/// What reading one image or form told, besides its record.
struct XRead {
    gave_up: bool,
    /// A form whose content was read. A form that wasn't, or anything that failed, is not, and draws nothing that can be known: it has no children, but
    /// it's still a form with its count.
    is_form: bool,
}

/// Reads one image or form for its record after the name, into `tail`, its content's names fed to `children` where it's a form.
fn read_xobject(
    reader: &IndexedReader,
    id: ObjectId,
    descriptor: &lopdf::IndexedStreamDescriptor,
    budget: &mut Budget<'_>,
    tail: &mut Vec<u8>,
    children: Option<&mut DoScan>,
) -> Result<XRead, Capped> {
    let dict = descriptor.dictionary();
    let refs = Refs::new(reader);
    let resolve = |object: &Object| refs.deref(object);
    let subtype = dict.get(b"Subtype").ok().and_then(resolve);
    let is_form = matches!(&subtype, Some(Object::Name(subtype)) if subtype == b"Form");
    // An image past its cap is hashed by its first cap's worth and what it says of itself; anything else past it is a cap reached.
    let prefix = matches!(&subtype, Some(Object::Name(subtype)) if subtype == b"Image");
    let limit = budget.caps.xobject_bytes;
    let general = decode::filter_names(dict, &resolve).is_some_and(|names| names.iter().all(|name| decode::is_general(name)));
    let as_stored = |budget: &mut Budget<'_>| take_stream(reader, id, descriptor, None, limit, budget, None, prefix);
    let mut gave_up = false;
    let (kind, taken) = if general {
        let mut scan = children.filter(|_| is_form);
        // What a form drew is what its content decoded to: if it didn't, the names its first part gave are taken back (as `content_record` does).
        let mark = scan.as_deref().map(DoScan::mark);
        match decode::chain_of(dict, &resolve) {
            Some(chain) => match take_stream(reader, id, descriptor, Some(&chain), limit, budget, scan.as_deref_mut(), prefix)? {
                Taken::Failed => {
                    gave_up = true;
                    if let (Some(scan), Some(mark)) = (scan, mark) {
                        scan.back_to(mark);
                    }
                    (3, as_stored(budget)?)
                }
                taken => (0, taken),
            },
            // No decode was attempted, so nothing here depends on what's left of the page: only a stream that couldn't be read at all does.
            None => (3, as_stored(budget)?),
        }
    } else {
        (1, as_stored(budget)?)
    };
    match taken {
        Taken::Hashed(n, sub) => {
            tail.push(kind);
            tail.extend_from_slice(&n.to_le_bytes());
            tail.extend_from_slice(&sub.finalize());
        }
        Taken::Prefix(n, sub) => {
            tail.extend_from_slice(&[4, kind]);
            tail.extend_from_slice(&n.to_le_bytes());
            tail.extend_from_slice(&sub.finalize());
            tail.extend_from_slice(&descriptor.encoded_len().unwrap_or(0).to_le_bytes());
            image_says(dict, &resolve, tail);
        }
        Taken::Failed => {
            tail.push(2);
            return Ok(XRead { gave_up: true, is_form: false });
        }
    }
    Ok(XRead { gave_up, is_form })
}

/// What an image past its cap says of itself, after its first cap's worth: `/Width`, `/Height`, `/BitsPerComponent`, the name of its `/ColorSpace`
/// (an array's first name, as `/ICCBased` or `/Indexed`) and its `/Filter` names, as written. Each framed: 0 missing; 1 and an `i64le` integer; 2, a
/// `u32le` length and a name; 3 anything else; and for the filters 4, a `u32le` count and each name with its length.
fn image_says(dict: &Dictionary, resolve: &dyn Fn(&Object) -> Option<Object>, out: &mut Vec<u8>) {
    let name = |out: &mut Vec<u8>, name: &[u8]| {
        out.extend_from_slice(&[2]);
        out.extend_from_slice(&count(name.len()));
        out.extend_from_slice(name);
    };
    for key in [&b"Width"[..], b"Height", b"BitsPerComponent"] {
        match dict.get(key).ok().map(resolve) {
            None => out.extend_from_slice(&[0]),
            Some(Some(Object::Integer(value))) => {
                out.extend_from_slice(&[1]);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Some(_) => out.extend_from_slice(&[3]),
        }
    }
    match dict.get(b"ColorSpace").ok().map(resolve) {
        None => out.extend_from_slice(&[0]),
        Some(Some(Object::Name(space))) => name(out, &space),
        Some(Some(Object::Array(items))) => match items.first().and_then(resolve) {
            Some(Object::Name(space)) => name(out, &space),
            _ => out.extend_from_slice(&[3]),
        },
        Some(_) => out.extend_from_slice(&[3]),
    }
    match (dict.has(b"Filter"), decode::filter_names(dict, resolve)) {
        (false, _) => out.extend_from_slice(&[0]),
        (true, Some(names)) => {
            out.extend_from_slice(&[4]);
            out.extend_from_slice(&count(names.len()));
            for each in names {
                out.extend_from_slice(&count(each.len()));
                out.extend_from_slice(&each);
            }
        }
        (true, None) => out.extend_from_slice(&[3]),
    }
}

fn record(hasher: &mut Sha256, kind: u8, n: u64, sub: Sha256) {
    hasher.update([kind]);
    hasher.update(n.to_le_bytes());
    hasher.update(sub.finalize());
}

/// What came of reading a stream for its hash.
enum Taken {
    /// Its `n` bytes (decoded, or as stored), hashed.
    Hashed(u64, Sha256),
    /// An image past its cap: its first `n` bytes (the cap's worth exactly), hashed.
    Prefix(u64, Sha256),
    /// It couldn't be decoded or read: a decoding error, a read error, or no length to read it by.
    Failed,
}

/// Hashes a stream, decoded through `chain` or as stored without one, feeding what it decodes to `scan`. Its stored length is charged to the page's read
/// cap **before** anything is read or decrypted (review S1: a stream that decodes to little pulls little through a decoder, so a cap counted as bytes
/// were pulled let a page decrypt its streams whole a thousand times over). More than `limit` bytes is a cap reached, unless `prefix` says the first
/// `limit` are to be hashed of it (an image's); more than the page has left always is.
#[allow(clippy::too_many_arguments)] // Each says one thing of this one read; a struct would be built for each call and read once.
fn take_stream(
    reader: &IndexedReader,
    id: ObjectId,
    descriptor: &lopdf::IndexedStreamDescriptor,
    chain: Option<&[decode::Filter]>,
    limit: u64,
    budget: &mut Budget<'_>,
    mut scan: Option<&mut DoScan>,
    prefix: bool,
) -> Result<Taken, Capped> {
    let Some(length) = descriptor.encoded_len() else {
        return Ok(Taken::Failed);
    };
    if length > budget.read_left {
        return Err(Capped);
    }
    budget.read_left -= length;
    budget.cost = budget.cost.saturating_add(length);
    let stored: Box<dyn Read + '_> = match descriptor.protection() {
        lopdf::EncodedStreamProtection::Plain => match descriptor.open_plain_encoded() {
            Ok(stored) => Box::new(Stored(stored)),
            Err(_) => return Ok(Taken::Failed),
        },
        // Only the decrypting path gives an encrypted file's bytes, and it gives them whole: only up to the size the other walks of such a file decrypt
        // (`crate::effects::MAX_DECRYPTED_DRAWING`), whichever part of the page it's for.
        _ => {
            if length > crate::effects::MAX_DECRYPTED_DRAWING {
                return Err(Capped);
            }
            match reader.resolve_object(id) {
                Ok(Object::Stream(stream)) => {
                    #[cfg(test)]
                    tests::DECRYPTED.with(|bytes| bytes.set(bytes.get() + stream.content.len() as u64));
                    Box::new(io::Cursor::new(stream.content))
                }
                _ => return Ok(Taken::Failed),
            }
        }
    };
    let mut source = match chain {
        Some(chain) => decode::decoded(stored, chain),
        None => stored,
    };
    // One byte past what may be hashed says a cap is reached. Each read asks for no more than that byte, and a decoder reads nothing past the unit
    // holding it (its group, row or run, as [`decode`] defines them), so what's in the stream after it never decides anything. A prefix is of the
    // stream's own cap only.
    let prefix = prefix && limit <= budget.page_left;
    let limit = limit.min(budget.page_left);
    let mut sub = Sha256::new();
    let mut buf = vec![0u8; decode::CHUNK];
    let mut n = 0u64;
    loop {
        let want = usize::try_from((limit + 1 - n).min(decode::CHUNK as u64)).unwrap_or(decode::CHUNK);
        let read = match source.read(&mut buf[..want]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Ok(Taken::Failed),
        };
        if read == 0 {
            break;
        }
        if n + read as u64 > limit {
            if !prefix {
                return Err(Capped);
            }
            let rest = usize::try_from(limit - n).unwrap_or(0);
            sub.update(&buf[..rest]);
            budget.page_left -= limit;
            budget.cost = budget.cost.saturating_add(limit);
            return Ok(Taken::Prefix(limit, sub));
        }
        n += read as u64;
        sub.update(&buf[..read]);
        if let Some(scan) = scan.as_deref_mut() {
            scan.feed(&buf[..read]);
        }
    }
    budget.page_left -= n;
    budget.cost = budget.cost.saturating_add(n);
    Ok(Taken::Hashed(n, sub))
}

/// A stream's stored bytes, a piece at a time.
struct Stored(lopdf::EncodedStreamReader);

impl Read for Stored {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read_chunk(buf).map_err(|_| io::Error::other("a stream's bytes couldn't be read"))
    }
}

fn deref(reader: &IndexedReader, object: &Object) -> Option<Object> {
    match object {
        // A stream is never resolved whole here: a reference to one isn't a value these walks need.
        Object::Reference(id) if reader.resolve_stream_descriptor(*id).is_ok() => None,
        Object::Reference(id) => reader.resolve_object(*id).ok(),
        other => Some(other.clone()),
    }
}

/// The objects one read of a stream's dictionary has followed a reference to, each parsed once, however many places in it point there (a `/Filter` of
/// references to one object, read for what it is, for its chain and for its record, was parsed for each). Most [`Refs::MAX`] kept: one more is parsed
/// each time asked for, so what a read holds stays small.
struct Refs<'r> {
    reader: &'r IndexedReader,
    seen: RefCell<HashMap<ObjectId, Option<Object>>>,
}

impl<'r> Refs<'r> {
    const MAX: usize = 16;
    /// A value is kept for a second look only if it holds at most this many bytes (`Object::retained_bytes`): a bigger one, such as a 100,000-element
    /// array a hostile file points at, is parsed again where it's asked for again. With at most [`Self::MAX`] kept, a read holds at most 64 KiB here.
    const MAX_KEPT_BYTES: u64 = 4096;

    fn new(reader: &'r IndexedReader) -> Self {
        Self {
            reader,
            seen: RefCell::new(HashMap::new()),
        }
    }

    fn deref(&self, object: &Object) -> Option<Object> {
        let Object::Reference(id) = object else { return Some(object.clone()) };
        if let Some(value) = self.seen.borrow().get(id) {
            return value.clone();
        }
        #[cfg(test)]
        tests::DEREFERENCES.with(|count| count.set(count.get() + 1));
        let value = deref(self.reader, object);
        let mut seen = self.seen.borrow_mut();
        if seen.len() < Self::MAX && value.as_ref().is_none_or(|value| value.retained_bytes() <= Self::MAX_KEPT_BYTES) {
            seen.insert(*id, value.clone());
        }
        value
    }
}

fn dict_at(reader: &IndexedReader, id: ObjectId) -> Option<Dictionary> {
    match deref(reader, &Object::Reference(id))? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

fn resolved_dict(reader: &IndexedReader, object: &Object) -> Option<Dictionary> {
    #[cfg(test)]
    if matches!(object, Object::Reference(_)) {
        tests::DICTIONARIES.with(|count| count.set(count.get() + 1));
    }
    match deref(reader, object)? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
