//! **A corpus's pages, sealed** ([`crate::seal`]): where each page's chunk is, what's inside it before its text, and the small files sealed whole. Everything that comes from the document is in a chunk; the table says only where each chunk is and how long.

#[cfg(any(test, feature = "test-seams"))]
use super::{CorpusKey, MAX_PAGES};
use super::{CorpusPaths, FLAG_EDGES, FindKey, MAX_BLOB_BYTES, PageKind, clear_text};
use crate::chrome::stored;
use crate::seal::{self, CorpusSeal, Purpose};
use recto_platform::local;
use serde_json::Value;
use std::fs::File;
use std::io;
use std::path::Path;

/// A table entry: where a page's sealed chunk is in the pack, and how long. All that's on disk outside a chunk.
pub(super) const RECORD_LEN: usize = 12;
/// What a page's chunk holds before its text: its kind, flags, reason, characters and lengths.
pub(super) const INNER_LEN: usize = 16;

/// What's known about one page, kept at the start of its sealed chunk, so nothing of it is on disk in the clear. In memory only while its page is in hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PageRecord {
    pub(super) text_len: u32,
    pub(super) aux_len: u32,
    pub(super) chars: u32,
    pub(super) kind: PageKind,
    pub(super) flags: u8,
    pub(super) reason: u8,
}

impl PageRecord {
    pub(super) fn write_inner(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[self.kind as u8, self.flags, self.reason, 0]);
        out.extend_from_slice(&self.chars.to_le_bytes());
        out.extend_from_slice(&self.text_len.to_le_bytes());
        out.extend_from_slice(&self.aux_len.to_le_bytes());
    }

    /// What a chunk's first bytes say, if they say it of exactly `rest` bytes after them (and, with the edges flag, a region of whole pieces after the text and what's beside it). They're inside a chunk that opened, so they're ours, but they're checked as if they weren't.
    pub(super) fn read_inner(bytes: &[u8], rest: usize) -> Option<Self> {
        let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        let kind = match bytes.first()? {
            1 => PageKind::Text,
            2 => PageKind::NoText,
            3 => PageKind::Failed,
            _ => return None,
        };
        let record = Self {
            kind,
            flags: *bytes.get(1)?,
            reason: *bytes.get(2)?,
            chars: word(4)?,
            text_len: word(8)?,
            aux_len: word(12)?,
        };
        // With edges, what's left after the text and what's beside it is exactly a region of whole pieces: the region's own bytes are checked when the page is read, where its text is.
        let fits = match rest.checked_sub(record.blob_len()) {
            Some(0) => record.flags & FLAG_EDGES == 0,
            Some(edges) => record.flags & FLAG_EDGES != 0 && stored::length_fits(edges),
            None => false,
        };
        fits.then_some(record)
    }

    pub(super) fn blob_len(&self) -> usize {
        self.text_len as usize + self.aux_len as usize
    }
}

/// Where a page's chunk is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) offset: u64,
    pub(super) len: u32,
}

impl Entry {
    pub(super) fn encode(self) -> [u8; RECORD_LEN] {
        let mut out = [0; RECORD_LEN];
        out[..8].copy_from_slice(&self.offset.to_le_bytes());
        out[8..].copy_from_slice(&self.len.to_le_bytes());
        out
    }

    pub(super) fn decode(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            offset: u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?),
            len: u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?),
        })
    }

    pub(super) fn end(self) -> u64 {
        // What's on disk can say anything, and saturating lands past the end of any file.
        self.offset.saturating_add(u64::from(self.len))
    }

    /// Whether a chunk this long could be a page's.
    pub(super) fn is_bounded(self) -> bool {
        (self.len as usize) <= MAX_BLOB_BYTES + INNER_LEN + seal::OVERHEAD
    }
}

/// Reads one page's chunk into `buffer` and opens it there. The page's text is then in `buffer`, at the range given, and nowhere else.
pub(super) fn open_page(
    pack: &File,
    pack_len: u64,
    seal: &CorpusSeal,
    page: u32,
    entry: Entry,
    buffer: &mut Vec<u8>,
) -> Option<(PageRecord, std::ops::Range<usize>)> {
    if !entry.is_bounded() || entry.end() > pack_len {
        return None;
    }
    // A page longer than any before it grows the buffer, which frees the old room as it was: what's there is overwritten first.
    if entry.len as usize > buffer.capacity() {
        zeroize::Zeroize::zeroize(buffer);
    }
    buffer.resize(entry.len as usize, 0);
    recto_platform::read_exact_at(pack, buffer, entry.offset).ok()?;
    let plain = seal.open_in_place(Purpose::Page(page), buffer).ok()?;
    let inner = buffer.get(plain.start..plain.start + INNER_LEN)?;
    let record = PageRecord::read_inner(inner, plain.len().checked_sub(INNER_LEN)?)?;
    Some((record, plain.start + INNER_LEN..plain.end))
}

/// Seals a page: its record, then its text and what's beside it.
pub(super) fn seal_page(seal: &CorpusSeal, page: u32, record: &PageRecord, blob: &[u8], chunk: &mut Vec<u8>) -> io::Result<()> {
    seal.seal(Purpose::Page(page), chunk, |out| {
        record.write_inner(out);
        out.extend_from_slice(blob);
    })
}

/// A small file sealed whole: the summary, or the title and outline.
pub(super) fn write_sealed(seal: &CorpusSeal, path: &Path, purpose: Purpose, bytes: &[u8], synced: bool) -> io::Result<()> {
    let mut chunk = Vec::with_capacity(bytes.len() + seal::OVERHEAD);
    seal.seal(purpose, &mut chunk, |out| out.extend_from_slice(bytes))?;
    if synced {
        local::write_private_file(path, &chunk)
    } else {
        local::write_private_file_unsynced(path, &chunk)
    }
}

pub(super) fn read_sealed_json(seal: &CorpusSeal, path: &Path, purpose: Purpose) -> Option<Value> {
    let mut chunk = local::read_private_file(path, 4 * 1024 * 1024 + seal::OVERHEAD as u64).ok()??;
    let plain = seal.open_in_place(purpose, &mut chunk).ok()?;
    serde_json::from_slice(&chunk[plain]).ok()
}

/// What `seal.bin` gave: nothing there, its bytes, or a failure to read it, which says nothing about whose it is.
enum Header {
    Absent,
    Read(Vec<u8>),
    Unreadable,
}

fn read_header(paths: &CorpusPaths) -> Header {
    match local::read_private_file(&paths.seal(), seal::HEADER_BYTES as u64) {
        Ok(None) => Header::Absent,
        Ok(Some(bytes)) => Header::Read(bytes),
        Err(_) => Header::Unreadable,
    }
}

/// The corpus's seal, if its header is there and this key opens it.
pub(super) fn read_seal(find_key: &FindKey, paths: &CorpusPaths) -> Option<CorpusSeal> {
    let Header::Read(header) = read_header(paths) else { return None };
    CorpusSeal::open(find_key, paths.key, &header).ok()
}

/// Whether a corpus is sealed with a key that isn't this one, and if so whether that key lasts, as its header says (read without a key: a hint, which anyone who can write here could change, and changing it can only get a corpus thrown away or kept). `None` if there's no header, or one that reads and isn't one: only those may lead to a clear. A header that's there and can't be read just now (a mode the private read refuses, a file past its size, an error from the disk) is taken as the lasting key's, so a run without that key leaves it (the second review's S1, 2026-11-15).
pub(super) fn sealed_elsewhere(paths: &CorpusPaths) -> Option<bool> {
    match read_header(paths) {
        Header::Absent => None,
        Header::Read(header) => CorpusSeal::lasting(&header),
        Header::Unreadable => Some(true),
    }
}

/// The seal to build with: the one there, or a new one, after throwing away anything this key can't open. That's how a corpus sealed by a key that's gone, a run's own or a lost one, is built again. Never over what a lasting key sealed when this run's lack of it is only for now: that's refused before anything is thrown away.
pub(super) fn seal_for_building(find_key: &FindKey, paths: &CorpusPaths) -> io::Result<CorpusSeal> {
    // A key bound to another corpus can open nothing here, which would read as a corpus to throw away: it's refused before anything goes.
    if !crate::seal::may_key(find_key, paths.key) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a key bound to another corpus"));
    }
    if let Some(seal) = read_seal(find_key, paths) {
        return Ok(seal);
    }
    if sealed_elsewhere(paths) == Some(true) && !find_key.source().may_replace_lasting() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sealed by the lasting key, out of reach for now",
        ));
    }
    clear_text(paths);
    let (seal, header) = CorpusSeal::create(find_key, paths.key)?;
    local::write_private_file(&paths.seal(), &header)?;
    Ok(seal)
}

/// **What's kept of a document, as its key reads it:** the text of every page of a corpus folder that opens, finished or not, in order up to the first that doesn't, then its title and outline if they're there. For a check that a document's words are on disk, or gone, now that a look at the bytes can't say: `recto-cli`'s index tests. Nothing in the app reads text this way.
#[cfg(any(test, feature = "test-seams"))]
pub fn opened_text(find_key: &FindKey, root: &Path, key: CorpusKey) -> Vec<String> {
    let paths = CorpusPaths::new(root, key);
    let Some(seal) = read_seal(find_key, &paths) else { return Vec::new() };
    let mut found = Vec::new();
    let table = local::read_private_file(&paths.table, u64::from(MAX_PAGES) * RECORD_LEN as u64)
        .ok()
        .flatten()
        .unwrap_or_default();
    if let Ok(pack) = local::open_private_file(&paths.pack) {
        let pack_len = pack.metadata().map_or(0, |meta| meta.len());
        let mut buffer = crate::seal::Plain::default();
        for (page, bytes) in table.as_chunks::<RECORD_LEN>().0.iter().enumerate() {
            let Some((record, blob)) = Entry::decode(bytes).and_then(|entry| open_page(&pack, pack_len, &seal, page as u32, entry, &mut buffer)) else {
                break;
            };
            found.push(String::from_utf8_lossy(&buffer[blob][..record.text_len as usize]).into_owned());
        }
    }
    if let Some(meta) = read_sealed_json(&seal, &paths.meta, Purpose::Meta) {
        found.push(meta.to_string());
    }
    found
}
