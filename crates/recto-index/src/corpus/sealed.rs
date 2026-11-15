//! **A corpus's pages, sealed** ([`crate::seal`]): where each page's chunk is, what's inside it before its text, and the small files sealed whole. Everything that comes from the document is in a chunk; the table says only where each chunk is and how long.

use super::{CorpusPaths, FindKey, MAX_BLOB_BYTES, PageKind, clear_text};
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

    /// What a chunk's first bytes say, if they say it of exactly `rest` bytes after them. They're inside a chunk that opened, so they're ours, but they're checked as if they weren't.
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
        (record.blob_len() == rest).then_some(record)
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

/// The corpus's seal, if its header is there and this key opens it.
pub(super) fn read_seal(find_key: &FindKey, paths: &CorpusPaths) -> Option<CorpusSeal> {
    let header = local::read_private_file(&paths.seal(), seal::HEADER_BYTES as u64).ok()??;
    CorpusSeal::open(find_key, paths.key, &header).ok()
}

/// The seal to build with: the one there, or a new one, after throwing away anything this key can't open. That's how a corpus sealed by a key that's gone, a run's own or a lost one, is built again.
pub(super) fn seal_for_building(find_key: &FindKey, paths: &CorpusPaths) -> io::Result<CorpusSeal> {
    if let Some(seal) = read_seal(find_key, paths) {
        return Ok(seal);
    }
    clear_text(paths);
    let (seal, header) = CorpusSeal::create(find_key, paths.key)?;
    local::write_private_file(&paths.seal(), &header)?;
    Ok(seal)
}
