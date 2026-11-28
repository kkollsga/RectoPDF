//! **A page's blocks, as stored: where each paragraph, heading, list item, caption, code block or footnote sits in the page's text.** Pure: no I/O, no
//! helper, no session. What reads a file and what answers a command are built on this later (`docs/design/blocks.md`).
//!
//! A stored block holds no text. It says which characters of `recto text`'s page are the block (`[start, end)` in characters), its kind and level, its
//! box in whole points and whether it goes on onto the next page. Text always comes from `recto text`, so hidden text stays marked and distillPDF's own
//! text never reaches an agent.
//!
//! - [`codec`]: a page's list as one sealed record, refused whole when damaged, foreign or over a cap.
//! - [`map`]: distillPDF's candidate blocks mapped onto the page's text by one settled rule.
//! - [`sections`]: a contents' entries as ranges, and what holds a place.
//! - [`unit`]: the paragraph, section or chapter an agent asks for, from the page text and the stored blocks.
//! - [`markdown`]: a run of blocks as Markdown that a document can't forge structure in.

pub mod codec;
pub mod map;
pub mod markdown;
pub mod sections;
pub mod unit;

#[cfg(test)]
mod tests;

/// Most blocks stored for one page. Past it the rest of the page's candidates are dropped and counted.
pub const MAX_BLOCKS: usize = 512;
/// Most characters of a page's text a block may point into, and most a mapping reads: 1,048,576, fifty times the busiest page measured (under 20,000). A page past it maps nothing, so the mapping's buffers (8 bytes a letter, up to 3 letters a ligature) stay inside 25 MB (measured, release: 8.4 MB for a page of plain letters and 25.2 MB for a page of `ffi` ligatures, the worst).
pub const MAX_PAGE_CHARS: u32 = 1 << 20;
/// The deepest level kept for a block. Past it the level is this.
pub const MAX_LEVEL: u8 = 9;

/// What a stored block is. Tables and figures have no text to map and are not stored: a page that is a table answers as the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Paragraph,
    Heading,
    ListItem,
    Caption,
    Code,
    Footnote,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Self::Paragraph, Self::Heading, Self::ListItem, Self::Caption, Self::Code, Self::Footnote];

    /// The name an answer gives it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::Heading => "heading",
            Self::ListItem => "list_item",
            Self::Caption => "caption",
            Self::Code => "code",
            Self::Footnote => "footnote",
        }
    }

    pub(crate) const fn to_byte(self) -> u8 {
        match self {
            Self::Paragraph => 1,
            Self::Heading => 2,
            Self::ListItem => 3,
            Self::Caption => 4,
            Self::Code => 5,
            Self::Footnote => 6,
        }
    }

    pub(crate) const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::Paragraph,
            2 => Self::Heading,
            3 => Self::ListItem,
            4 => Self::Caption,
            5 => Self::Code,
            6 => Self::Footnote,
            _ => return None,
        })
    }
}

/// One stored block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    /// A heading's level or a list item's nesting, at most [`MAX_LEVEL`]; 0 when distillPDF gave none.
    pub level: u8,
    /// Zero-based page.
    pub page: u32,
    /// Characters `[start, end)` of the page's text as `recto text` gives it.
    pub start: u32,
    pub end: u32,
    /// Left, bottom, right, top in whole points, in the page's own coordinates.
    pub bbox: [i16; 4],
    /// The block goes on at the top of the next page.
    pub continues: bool,
}

/// A page's blocks, in distillPDF's reading order (which isn't always the order of their characters).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageBlocks {
    pub page: u32,
    /// The page's characters in the `recto text` numbering these blocks' offsets are in (at most [`MAX_PAGE_CHARS`]): a record read against a page of another length is stale.
    pub chars: u32,
    pub blocks: Vec<Block>,
}
