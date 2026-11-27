//! **A page's blocks as one sealed record**, in the corpus's way (`corpus/identities.rs`): sealed under the corpus's key with the page in its associated data
//! (`Purpose::Blocks`), so a record can't answer for another page, and refused whole when anything about it can't have come from this build.
//!
//! ```text
//! [format (1)] [page (4)] [blocks (2)] then each block: [kind (1)] [level (1)] [flags (1): bit 0 continues] [start (4)] [end (4)] [left, bottom, right, top (2 each)]
//! ```
//!
//! **Refused:** a record that doesn't open (the wrong key, a changed byte, another page's), longer than [`MAX_RECORD_BYTES`], with more than
//! [`MAX_BLOCKS`](super::MAX_BLOCKS), a length that isn't its count, a kind, level, flag or place that no build writes, blocks that are empty, run past
//! [`MAX_PAGE_CHARS`](super::MAX_PAGE_CHARS) or overlap, or another page's number inside. A record of another format is [`OpenError::Foreign`]: not damage,
//! so a reader keeps what it has and builds again.

use super::{Block, Kind, MAX_BLOCKS, MAX_LEVEL, MAX_PAGE_CHARS, PageBlocks};
use crate::seal::{self, CorpusSeal, Purpose};

/// What the record's first byte says. Raised when the mapping rule or the layout changes, so records made under another are built again.
pub const FORMAT: u8 = 1;
const HEAD: usize = 1 + 4 + 2;
const BLOCK: usize = 1 + 1 + 1 + 4 + 4 + 8;
/// The most a record takes on disk: the plain bytes of [`MAX_BLOCKS`](super::MAX_BLOCKS) and the seal.
pub const MAX_RECORD_BYTES: usize = HEAD + MAX_BLOCKS * BLOCK + seal::OVERHEAD;

/// Why a record isn't used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// It doesn't open, or what it says can't be true: for the caller to throw away.
    Damaged,
    /// It opens and is of another format: nothing is used and nothing is thrown away for it.
    Foreign,
}

/// What a page's blocks take on disk.
#[must_use]
pub fn record_len(blocks: usize) -> usize {
    HEAD + blocks.min(MAX_BLOCKS) * BLOCK + seal::OVERHEAD
}

/// Seals a page's blocks into `out`. A list past [`MAX_BLOCKS`](super::MAX_BLOCKS) or with a block no record may hold is refused, so what's written is what opens.
///
/// # Errors
///
/// `InvalidInput` for such a list; otherwise the seal's own.
pub fn seal_page(seal: &CorpusSeal, page: &PageBlocks, out: &mut Vec<u8>) -> std::io::Result<()> {
    if page.blocks.len() > MAX_BLOCKS || !holds(page) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "blocks a record can't hold"));
    }
    seal.seal(Purpose::Blocks(page.page), out, |out| {
        out.push(FORMAT);
        out.extend_from_slice(&page.page.to_le_bytes());
        out.extend_from_slice(&(page.blocks.len() as u16).to_le_bytes());
        for block in &page.blocks {
            out.push(block.kind.to_byte());
            out.push(block.level);
            out.push(u8::from(block.continues));
            out.extend_from_slice(&block.start.to_le_bytes());
            out.extend_from_slice(&block.end.to_le_bytes());
            for side in block.bbox {
                out.extend_from_slice(&side.to_le_bytes());
            }
        }
    })
}

/// Opens page `page`'s record where it lies. `chunk` is left as it was when it doesn't open; when it does, the plain bytes are overwritten, as a caller wants.
///
/// # Errors
///
/// [`OpenError`].
pub fn open_page(seal: &CorpusSeal, page: u32, chunk: &mut [u8]) -> Result<PageBlocks, OpenError> {
    if chunk.len() > MAX_RECORD_BYTES {
        return Err(OpenError::Damaged);
    }
    let range = seal.open_in_place(Purpose::Blocks(page), chunk).map_err(|_| OpenError::Damaged)?;
    let plain = &chunk[range];
    let (&format, rest) = plain.split_first().ok_or(OpenError::Damaged)?;
    if format != FORMAT {
        return Err(OpenError::Foreign);
    }
    if rest.len() < HEAD - 1 {
        return Err(OpenError::Damaged);
    }
    let said = u32::from_le_bytes(rest[..4].try_into().map_err(|_| OpenError::Damaged)?);
    let count = usize::from(u16::from_le_bytes(rest[4..6].try_into().map_err(|_| OpenError::Damaged)?));
    let body = &rest[6..];
    if said != page || count > MAX_BLOCKS || body.len() != count * BLOCK {
        return Err(OpenError::Damaged);
    }
    let mut blocks = Vec::with_capacity(count);
    for bytes in body.chunks_exact(BLOCK) {
        let word = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        let side = |at: usize| i16::from_le_bytes([bytes[at], bytes[at + 1]]);
        if bytes[2] > 1 {
            return Err(OpenError::Damaged);
        }
        blocks.push(Block {
            kind: Kind::from_byte(bytes[0]).ok_or(OpenError::Damaged)?,
            level: bytes[1],
            page,
            start: word(3),
            end: word(7),
            bbox: [side(11), side(13), side(15), side(17)],
            continues: bytes[2] == 1,
        });
    }
    let opened = PageBlocks { page, blocks };
    if holds(&opened) { Ok(opened) } else { Err(OpenError::Damaged) }
}

/// Whether a build could have written these: levels in range, no empty block, none past the page cap, none over another.
fn holds(page: &PageBlocks) -> bool {
    let mut spans: Vec<(u32, u32)> = Vec::with_capacity(page.blocks.len());
    for block in &page.blocks {
        if block.level > MAX_LEVEL || block.page != page.page || block.start >= block.end || block.end > MAX_PAGE_CHARS {
            return false;
        }
        spans.push((block.start, block.end));
    }
    spans.sort_unstable();
    spans.windows(2).all(|pair| pair[0].1 <= pair[1].0)
}
