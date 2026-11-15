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
//!    `/Resources` `/XObject` (the page's if it has none). Kinds: 0, every filter general, decoded (for a form, its content, whose names are its
//!    children); 1, a filter not general (an image codec), as stored; 2, nothing by that name, or it isn't a stream or can't be read (no length or
//!    hash, and no children); 3, general but it didn't decode, as stored; 4, **an image past its cap** (`/Subtype /Image`, whose bytes as kind 0, 1
//!    or 3 would hash more than [`Caps::xobject_bytes`], 16 MiB): the byte 4, the kind it would have been, `u64le(n)` and the SHA-256 of exactly its
//!    first `n` = 16 MiB (decoded or as stored, as that kind is), then `u64le` of its stream's stored length, then its `/Width`, `/Height` and
//!    `/BitsPerComponent`, its `/ColorSpace`'s name and its `/Filter` names as written, each framed (`image_says`). Only a form drawn by the page's
//!    content has children; one drawn by a form is a record with none.
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

use lopdf::{Dictionary, IndexedReader, Object, ObjectId, PageMapEntry};
use scan::DoScan;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::Path;

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
    /// The first four bytes of SHA-256 over `recto-frame/1`, a NUL, the file's own quarter turns and the shown box (left, bottom, right, top as `f32`
    /// little-endian), as [`crate::nav`] reads them: equal frames, equal tags. Not part of the digest.
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
/// kept) from another document, in the words only; nothing is matched by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentIds {
    pub first: [u8; 16],
    pub second: [u8; 16],
}

/// Opens a file for its pages' identities, through the file wall, as every read of a document by its path is. A file that needs a password is refused:
/// its pages are never identified.
pub fn open_for_identity(path: &Path) -> Result<IndexedReader, crate::OpenError> {
    crate::open_reader_cached(path)
}

/// [`open_for_identity`] on bytes in memory, for tests and the fuzzer.
pub fn open_bytes_for_identity(bytes: std::sync::Arc<[u8]>) -> Result<IndexedReader, crate::OpenError> {
    crate::open_bytes(bytes)
}

/// The identities of these pages (zero-based) of a document, in the order asked: `None` for a page it doesn't have, or one with no identity as it
/// reached a cap.
pub fn identify_pages(reader: &IndexedReader, map: &lopdf::PageMap, pages: &[u32]) -> Vec<Option<PageIdentity>> {
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
    u32::from_le_bytes([tag[0], tag[1], tag[2], tag[3]])
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
    if scan.overflowed() {
        return Err(Capped);
    }
    let resources = entry
        .inherited()
        .resources()
        .and_then(|owner| dict_at(reader, owner))
        .and_then(|owner| resolved_dict(reader, owner.get(b"Resources").ok()?));
    let page_xobjects = xobjects_in(reader, resources.as_ref());
    hasher.update(b"X");
    hasher.update(count(scan.names().len()));
    for name in scan.names() {
        budget.record()?;
        let mut children = DoScan::default();
        let form = xobject_record(reader, page_xobjects.as_ref(), name, budget, &mut hasher, Some(&mut children))?;
        children.end_of_stream();
        let Some(form) = form else {
            hasher.update(count(0));
            continue;
        };
        if children.overflowed() {
            return Err(Capped);
        }
        let own = form
            .get(b"Resources")
            .ok()
            .and_then(|own| resolved_dict(reader, own))
            .map(|own| xobjects_in(reader, Some(&own)));
        let lookup = own.as_ref().unwrap_or(&page_xobjects).as_ref();
        hasher.update(count(children.names().len()));
        for child in children.names() {
            budget.record()?;
            xobject_record(reader, lookup, child, budget, &mut hasher, None)?;
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
    let chain = decode::chain_of(descriptor.dictionary(), &|object| deref(reader, object));
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

/// A resource dictionary's `/XObject`, resolved: looked up by name, never copied out or sorted.
fn xobjects_in(reader: &IndexedReader, resources: Option<&Dictionary>) -> Option<Dictionary> {
    resolved_dict(reader, resources?.get(b"XObject").ok()?)
}

/// One image's or form's record, found by `name` in `xobjects`. Gives the form's dictionary back when it's a form whose content decoded, its names fed
/// to `children`.
fn xobject_record(
    reader: &IndexedReader,
    xobjects: Option<&Dictionary>,
    name: &[u8],
    budget: &mut Budget<'_>,
    hasher: &mut Sha256,
    children: Option<&mut DoScan>,
) -> Result<Option<Dictionary>, Capped> {
    hasher.update(count(name.len()));
    hasher.update(name);
    let found = match xobjects.and_then(|xobjects| xobjects.get(name).ok()) {
        Some(Object::Reference(id)) => reader.resolve_stream_descriptor(*id).ok().map(|descriptor| (*id, descriptor)),
        _ => None,
    };
    let Some((id, descriptor)) = found else {
        hasher.update([2]);
        return Ok(None);
    };
    let dict = descriptor.dictionary();
    let resolve = |object: &Object| deref(reader, object);
    let subtype = dict.get(b"Subtype").ok().and_then(resolve);
    let is_form = matches!(&subtype, Some(Object::Name(subtype)) if subtype == b"Form");
    // An image past its cap is hashed by its first cap's worth and what it says of itself; anything else past it is a cap reached.
    let prefix = matches!(&subtype, Some(Object::Name(subtype)) if subtype == b"Image");
    let limit = budget.caps.xobject_bytes;
    let general = decode::filter_names(dict, &resolve).is_some_and(|names| names.iter().all(|name| decode::is_general(name)));
    let as_stored = |budget: &mut Budget<'_>| take_stream(reader, id, &descriptor, None, limit, budget, None, prefix);
    let (kind, taken, scanned) = if general {
        let scan = children.filter(|_| is_form);
        let scanned = scan.is_some();
        match decode::chain_of(dict, &resolve) {
            Some(chain) => match take_stream(reader, id, &descriptor, Some(&chain), limit, budget, scan, prefix)? {
                Taken::Failed => (3, as_stored(budget)?, false),
                taken => (0, taken, scanned),
            },
            None => (3, as_stored(budget)?, false),
        }
    } else {
        (1, as_stored(budget)?, false)
    };
    match taken {
        Taken::Hashed(n, sub) => {
            hasher.update([kind]);
            hasher.update(n.to_le_bytes());
            hasher.update(sub.finalize());
        }
        Taken::Prefix(n, sub) => {
            hasher.update([4, kind]);
            hasher.update(n.to_le_bytes());
            hasher.update(sub.finalize());
            hasher.update(descriptor.encoded_len().unwrap_or(0).to_le_bytes());
            image_says(dict, &resolve, hasher);
        }
        Taken::Failed => {
            hasher.update([2]);
            return Ok(None);
        }
    }
    // A form whose content wasn't decoded draws nothing that can be known, so it has no children; but it's still a form, with its count.
    Ok(is_form.then(|| if scanned { dict.clone() } else { Dictionary::new() }))
}

/// What an image past its cap says of itself, after its first cap's worth: `/Width`, `/Height`, `/BitsPerComponent`, the name of its `/ColorSpace`
/// (an array's first name, as `/ICCBased` or `/Indexed`) and its `/Filter` names, as written. Each framed: 0 missing; 1 and an `i64le` integer; 2, a
/// `u32le` length and a name; 3 anything else; and for the filters 4, a `u32le` count and each name with its length.
fn image_says(dict: &Dictionary, resolve: &dyn Fn(&Object) -> Option<Object>, hasher: &mut Sha256) {
    let name = |hasher: &mut Sha256, name: &[u8]| {
        hasher.update([2]);
        hasher.update(count(name.len()));
        hasher.update(name);
    };
    for key in [&b"Width"[..], b"Height", b"BitsPerComponent"] {
        match dict.get(key).ok().map(resolve) {
            None => hasher.update([0]),
            Some(Some(Object::Integer(value))) => {
                hasher.update([1]);
                hasher.update(value.to_le_bytes());
            }
            Some(_) => hasher.update([3]),
        }
    }
    match dict.get(b"ColorSpace").ok().map(resolve) {
        None => hasher.update([0]),
        Some(Some(Object::Name(space))) => name(hasher, &space),
        Some(Some(Object::Array(items))) => match items.first().and_then(resolve) {
            Some(Object::Name(space)) => name(hasher, &space),
            _ => hasher.update([3]),
        },
        Some(_) => hasher.update([3]),
    }
    match (dict.has(b"Filter"), decode::filter_names(dict, resolve)) {
        (false, _) => hasher.update([0]),
        (true, Some(names)) => {
            hasher.update([4]);
            hasher.update(count(names.len()));
            for each in names {
                hasher.update(count(each.len()));
                hasher.update(&each);
            }
        }
        (true, None) => hasher.update([3]),
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
    // One byte past what may be hashed says a cap is reached, whatever a decoder holds back or reads ahead. A prefix is of the stream's own cap only.
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

fn dict_at(reader: &IndexedReader, id: ObjectId) -> Option<Dictionary> {
    match deref(reader, &Object::Reference(id))? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

fn resolved_dict(reader: &IndexedReader, object: &Object) -> Option<Dictionary> {
    match deref(reader, object)? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
