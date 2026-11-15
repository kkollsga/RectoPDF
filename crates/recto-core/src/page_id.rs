//! **What a page is, by its content: the digest `recto-page/1`,** the identity the page matcher follows unsaved edits by when their file changes on disk
//! (owner, 2026-11-13: "we need to run a page matcher of some kind. That's the way we can ensure edits on the correct page"), and the one the locator's
//! page reference will carry (PLAN 8.1). **This is its one definition.** A change to what goes in is a new name (`recto-page/2`), never an edit of this
//! one: what's kept of it (journals, and later the corpus and tokens) compares prefixes of digests made by older builds. The golden test vectors in this
//! module's tests pin it.
//!
//! **What's hashed,** SHA-256 over, in order:
//! 1. The bytes `recto-page/1` and a NUL.
//! 2. **Content.** Each entry of the page's `/Contents` (a stream, or an array of them; references followed, a nested array not), the first
//!    [`Caps::content_streams`] (1,024) of them, each as one record: a kind byte, `u64le(n)`, then the SHA-256 of those `n` bytes. Kinds:
//!    0, decoded through the general filters ([`decode`]: Flate, LZW, ASCIIHex, ASCII85, RunLength, with PNG and TIFF predictors);
//!    1, hashed as stored, as its filters couldn't be decoded (another filter, bad data or parameters, or more stored bytes read than
//!    [`Caps::page_read`]);
//!    2, not a stream or not readable at all (`n` is 0);
//!    3, not read, as a cap was reached first (`n` is 0).
//!    The content's bytes together are at most [`Caps::content_bytes`] (16 MiB): a stream that reaches it is hashed to there, and every one after it is
//!    kind 3.
//! 3. A NUL and `X`.
//! 4. **Images and forms.** The page's effective `/Resources` `/XObject` (the page's own or the nearest ancestor's), its entries sorted by the name's
//!    bytes. For each: `u32le(name length)`, the name, a kind byte, and for kinds 0, 1 and 3 `u64le(n)` and the SHA-256 of the stream's first `n`
//!    bytes, at most [`Caps::xobject_bytes`] (16 MiB). Kinds: 0, every filter general, decoded; 1, an image codec or a filter not known, as stored;
//!    2, not a stream, or the entry isn't a reference; 3, general but it didn't decode, as stored; 4, not read, as a cap was reached first. A form
//!    (`/Subtype /Form`) of the page's own is followed by the records of its own `/Resources` `/XObject`, sorted, one level only. At most
//!    [`Caps::xobjects`] (64) records in all.
//! 5. A NUL, `E`, and a byte of flags: 1, the content had more entries than its cap or reached its byte cap; 2, the page reached its byte cap; 4, there
//!    were more images and forms than their cap.
//!
//! Every byte hashed in 2 and 4 counts toward [`Caps::page_bytes`] (64 MiB a page): past it, what's left is kind 3 or 4. Every stored byte read counts
//! toward [`Caps::page_read`] (256 MiB), which bounds a stream that decodes to little from very much. So a hostile page costs at most the caps' time, and
//! its digest is still the same every time. Memory is a few buffers of [`decode::CHUNK`]: nothing is read whole, but for a stream of an encrypted file
//! (whose bytes only the reader's decrypting path gives, whole), which is read only when it's under the content cap, and is kind 3 or 4 otherwise.
//!
//! **What's left out, deliberately:** `/Rotate`, `/MediaBox`, `/CropBox` (a turn or a crop is a view of the same page: our own turn keeps the identity),
//! `/Annots` (a highlight added or taken out keeps it), fonts (an optimiser's re-subsetting keeps it), `/StructParents`, `/Group`, metadata, and the page
//! object's number. The frame the page is shown in is said apart from the digest, by [`PageIdentity::frame`], so a highlight isn't carried onto a
//! page whose crop or turn another program changed.
//!
//! **Changed from the research's sketch** (`docs/research/2026-11-14-locator.md` §5.2), before anything kept a digest: each stream is a record of its
//! kind, length and own hash rather than its bytes after their length (a stream is read a piece at a time, so its length isn't known first); and the
//! sketch's path for a stream that doesn't decode hashed its stored bytes with no cap at all (292 MiB for the one-page USGS map, in a prototype on
//! 2026-11-15), where here every path is under the page's cap.
//!
//! **Where it runs:** over the lazy `lopdf::IndexedReader`, with no PDFium and no worker, holding nothing after it returns, on the app's page-identity
//! thread, never the UI thread. A file that needed the person's password is never given here: its edits are held as before (`recto-session`).

mod decode;

use lopdf::{Dictionary, IndexedReader, Object, ObjectId, PageMapEntry};
use sha2::{Digest, Sha256};
use std::cell::Cell;
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
        xobjects: 64,
        xobject_bytes: 16 << 20,
        page_bytes: 64 << 20,
        page_read: 256 << 20,
    };
}

/// One page's identity: its digest, the page object's number as a fast first try (our own appends keep it; other programs mostly renumber, research
/// §3), and its frame's tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageIdentity {
    /// `recto-page/1`, whole. What's kept is a prefix.
    pub digest: [u8; 32],
    pub object: (u32, u16),
    /// The first four bytes of SHA-256 over `recto-frame/1`, a NUL, the file's own quarter turns and the shown box (left, bottom, right, top as `f32`
    /// little-endian), as [`crate::nav`] reads them: equal frames, equal tags. Not part of the digest.
    pub frame: u32,
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

/// The identities of these pages (zero-based) of a document, in the order asked: `None` for a page it doesn't have.
pub fn identify_pages(reader: &IndexedReader, map: &lopdf::PageMap, pages: &[u32]) -> Vec<Option<PageIdentity>> {
    pages
        .iter()
        .map(|page| map.get(*page as usize).map(|entry| identify_page(reader, entry, &Caps::STANDARD)))
        .collect()
}

/// One page's identity, under `caps`.
pub fn identify_page(reader: &IndexedReader, entry: &PageMapEntry, caps: &Caps) -> PageIdentity {
    let id = entry.id();
    PageIdentity {
        digest: digest(reader, entry, caps),
        object: (id.0, id.1),
        frame: frame_tag(crate::nav::frame_of(reader, entry)),
    }
}

/// **One page's identity under `caps`, with what reading it cost** (its bytes hashed or its stored bytes read, whichever is more), and whether it
/// reached a cap, as the page matcher's scan of a new version counts them against its own caps (`recto-session`'s `edit::matcher`).
pub fn identify_page_costed(reader: &IndexedReader, entry: &PageMapEntry, caps: &Caps) -> (PageIdentity, u64, bool) {
    let id = entry.id();
    let (digest, cost, capped) = digest_costed(reader, entry, caps);
    let identity = PageIdentity {
        digest,
        object: (id.0, id.1),
        frame: frame_tag(crate::nav::frame_of(reader, entry)),
    };
    (identity, cost, capped)
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

/// What's left of a page's caps as it's hashed.
struct Budget<'c> {
    caps: &'c Caps,
    page_left: u64,
    content_left: u64,
    /// Stored bytes still to be read, shared by every reader made for the page.
    read_left: Cell<u64>,
    flags: u8,
}

const CONTENT_CAPPED: u8 = 1;
const PAGE_CAPPED: u8 = 2;
const XOBJECTS_CAPPED: u8 = 4;

fn digest(reader: &IndexedReader, entry: &PageMapEntry, caps: &Caps) -> [u8; 32] {
    digest_costed(reader, entry, caps).0
}

/// [`digest`], with the bytes it hashed or read (whichever is more) and whether a cap was reached.
fn digest_costed(reader: &IndexedReader, entry: &PageMapEntry, caps: &Caps) -> ([u8; 32], u64, bool) {
    let mut budget = Budget {
        caps,
        page_left: caps.page_bytes,
        content_left: caps.content_bytes,
        read_left: Cell::new(caps.page_read),
        flags: 0,
    };
    let mut hasher = Sha256::new();
    hasher.update(DIGEST_NAME.as_bytes());
    hasher.update([0]);
    let page = dict_at(reader, entry.id());
    let contents = page.as_ref().and_then(|page| page.get(b"Contents").ok().cloned());
    for (at, item) in content_entries(reader, contents).iter().enumerate() {
        if at == caps.content_streams {
            budget.flags |= CONTENT_CAPPED;
            break;
        }
        content_record(reader, item, &mut budget, &mut hasher);
    }
    hasher.update(b"\0X");
    let resources = entry
        .inherited()
        .resources()
        .and_then(|owner| dict_at(reader, owner))
        .and_then(|owner| resolved_dict(reader, owner.get(b"Resources").ok()?));
    let mut records = 0usize;
    for (name, value) in xobjects_of(reader, resources.as_ref()) {
        let form = xobject_record(reader, &name, &value, &mut budget, &mut records, &mut hasher);
        if let Some(form) = form {
            let own = resolved_dict(reader, form.get(b"Resources").ok().unwrap_or(&Object::Null));
            for (name, value) in xobjects_of(reader, own.as_ref()) {
                xobject_record(reader, &name, &value, &mut budget, &mut records, &mut hasher);
            }
        }
    }
    hasher.update(b"\0E");
    hasher.update([budget.flags]);
    let hashed = caps.page_bytes.saturating_sub(budget.page_left);
    let read = caps.page_read.saturating_sub(budget.read_left.get());
    (hasher.finalize().into(), hashed.max(read), budget.flags != 0)
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

fn content_record(reader: &IndexedReader, item: &Object, budget: &mut Budget<'_>, hasher: &mut Sha256) {
    let limit = budget.content_left.min(budget.page_left);
    let Object::Reference(id) = item else {
        record(hasher, 2, 0, &Sha256::new());
        return;
    };
    let Ok(descriptor) = reader.resolve_stream_descriptor(*id) else {
        record(hasher, 2, 0, &Sha256::new());
        return;
    };
    if limit == 0 {
        budget.flags |= CONTENT_CAPPED;
        record(hasher, 3, 0, &Sha256::new());
        return;
    }
    let chain = decode::chain_of(descriptor.dictionary(), &|object| deref(reader, object));
    let taken = match chain.and_then(|chain| take_stream(reader, *id, &descriptor, Some(&chain), limit, budget)) {
        Some(taken) => Some((0, taken)),
        None => take_stream(reader, *id, &descriptor, None, limit, budget).map(|taken| (1, taken)),
    };
    match taken {
        Some((kind, (n, sub))) => {
            budget.content_left -= n;
            budget.page_left -= n;
            if n == limit {
                budget.flags |= CONTENT_CAPPED;
                if budget.page_left == 0 {
                    budget.flags |= PAGE_CAPPED;
                }
            }
            record(hasher, kind, n, &sub);
        }
        None => record(hasher, if budget.read_left.get() == 0 { 3 } else { 2 }, 0, &Sha256::new()),
    }
}

/// The entries of a resource dictionary's `/XObject`, sorted by name.
fn xobjects_of(reader: &IndexedReader, resources: Option<&Dictionary>) -> Vec<(Vec<u8>, Object)> {
    let Some(xobjects) = resources.and_then(|resources| resolved_dict(reader, resources.get(b"XObject").ok()?)) else {
        return Vec::new();
    };
    let mut entries: Vec<(Vec<u8>, Object)> = xobjects.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// One image's or form's record. Gives the form's dictionary back when it's a form, for its own entries to follow.
fn xobject_record(
    reader: &IndexedReader,
    name: &[u8],
    value: &Object,
    budget: &mut Budget<'_>,
    records: &mut usize,
    hasher: &mut Sha256,
) -> Option<Dictionary> {
    if *records == budget.caps.xobjects {
        budget.flags |= XOBJECTS_CAPPED;
        return None;
    }
    *records += 1;
    hasher.update(u32::try_from(name.len()).unwrap_or(u32::MAX).to_le_bytes());
    hasher.update(name);
    let Object::Reference(id) = value else {
        hasher.update([2]);
        return None;
    };
    let Ok(descriptor) = reader.resolve_stream_descriptor(*id) else {
        hasher.update([2]);
        return None;
    };
    let dict = descriptor.dictionary().clone();
    let is_form = matches!(dict.get(b"Subtype").ok().and_then(|subtype| deref(reader, subtype)), Some(Object::Name(name)) if name == b"Form");
    let limit = budget.caps.xobject_bytes.min(budget.page_left);
    if limit == 0 {
        budget.flags |= PAGE_CAPPED;
        hasher.update([4]);
        return is_form.then_some(dict);
    }
    let resolve = |object: &Object| deref(reader, object);
    let general = decode::filter_names(&dict, &resolve).is_some_and(|names| names.iter().all(|name| decode::is_general(name)));
    let taken = if general {
        match decode::chain_of(&dict, &resolve).and_then(|chain| take_stream(reader, *id, &descriptor, Some(&chain), limit, budget)) {
            Some(taken) => Some((0, taken)),
            None => take_stream(reader, *id, &descriptor, None, limit, budget).map(|taken| (3, taken)),
        }
    } else {
        take_stream(reader, *id, &descriptor, None, limit, budget).map(|taken| (1, taken))
    };
    match taken {
        Some((kind, (n, sub))) => {
            budget.page_left -= n;
            if budget.page_left == 0 {
                budget.flags |= PAGE_CAPPED;
            }
            hasher.update([kind]);
            hasher.update(n.to_le_bytes());
            hasher.update(sub.finalize());
        }
        None => hasher.update([if budget.read_left.get() == 0 { 4 } else { 2 }]),
    }
    is_form.then_some(dict)
}

fn record(hasher: &mut Sha256, kind: u8, n: u64, sub: &Sha256) {
    hasher.update([kind]);
    hasher.update(n.to_le_bytes());
    hasher.update(sub.clone().finalize());
}

/// Hashes a stream's first `limit` bytes, decoded through `chain` or as stored without one. `None` if it can't be read to there: a decoding error, a read
/// error, or the page's read cap reached.
fn take_stream(
    reader: &IndexedReader,
    id: ObjectId,
    descriptor: &lopdf::IndexedStreamDescriptor,
    chain: Option<&[decode::Filter]>,
    limit: u64,
    budget: &Budget<'_>,
) -> Option<(u64, Sha256)> {
    let stored: Box<dyn Read + '_> = match descriptor.protection() {
        lopdf::EncodedStreamProtection::Plain => Box::new(Stored(descriptor.open_plain_encoded().ok()?)),
        // Only the decrypting path gives an encrypted file's bytes, and it gives them whole: taken only under the content cap.
        _ => {
            if descriptor.encoded_len()? > budget.caps.content_bytes {
                return None;
            }
            match reader.resolve_object(id).ok()? {
                Object::Stream(stream) => Box::new(io::Cursor::new(stream.content)),
                _ => return None,
            }
        }
    };
    let counted: Box<dyn Read + '_> = Box::new(Counted {
        inner: stored,
        left: &budget.read_left,
    });
    let mut source = match chain {
        Some(chain) => decode::decoded(counted, chain),
        None => counted,
    };
    let mut sub = Sha256::new();
    let mut buf = vec![0u8; decode::CHUNK];
    let mut n = 0u64;
    while n < limit {
        let want = usize::try_from((limit - n).min(decode::CHUNK as u64)).unwrap_or(decode::CHUNK);
        let read = match source.read(&mut buf[..want]) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if read == 0 {
            break;
        }
        sub.update(&buf[..read]);
        n += read as u64;
    }
    Some((n, sub))
}

/// A stream's stored bytes, a piece at a time.
struct Stored(lopdf::EncodedStreamReader);

impl Read for Stored {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read_chunk(buf).map_err(|_| io::Error::other("a stream's bytes couldn't be read"))
    }
}

/// Stored bytes counted against the page's read cap, which is an error when reached.
struct Counted<'a> {
    inner: Box<dyn Read + 'a>,
    left: &'a Cell<u64>,
}

impl Read for Counted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.left.get();
        if left == 0 {
            return Err(io::Error::other("the page's read cap"));
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buf[..want])?;
        self.left.set(left - read as u64);
        Ok(read)
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
