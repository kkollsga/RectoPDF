//! Bounded references into a source-file page, using the existing `recto-page/1` identity.
//!
//! This first slice provides fixed-size values, canonical token encoding and shown-frame conversions, not a resolver.
//! A token is hostile input, never authority: document/text checks only identify candidates. A later executor must
//! authorize documents and verify the file version, page, text and frame before using a target. After a version change,
//! matching the hinted page alone does not disambiguate identical pages; incomplete scanning proves neither unique nor gone.
//! Nothing here opens a file, computes a second page identity, retains per-tab state or starts work.

mod codec;
mod geometry;

pub use codec::{decode, encode};

use crate::page_id::PageIdentity;

/// Most bytes examined by the token parser, checked before it reads any part.
pub const MAX_TOKEN_BYTES: usize = 128;
/// Highest one-based source-page hint. A hint does not establish that the page exists.
pub const MAX_PAGE: u32 = 200_000;
/// Highest page-relative Unicode character offset, with the end excluded.
pub const MAX_CHARACTER: u32 = 1 << 21;
/// Highest coordinate or extent, in whole points. Larger pages remain readable without such a reference.
pub const MAX_POINT: u16 = 14_400;

/// Why a reference cannot be represented. No error retains or repeats hostile input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooLong,
    NonAscii,
    UnsupportedVersion,
    Malformed,
    OutOfRange,
    InvalidGeometry,
}

/// Candidate identifiers supplied by the authorized document layer, not a file lookup or an identity verdict.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DocumentTags {
    content: [u8; 8],
    id: Option<[u8; 8]>,
}

impl DocumentTags {
    /// The first slice fixes byte widths, not the later document-discovery/hash algorithm.
    pub const fn new(content: [u8; 8], id: Option<[u8; 8]>) -> Self {
        Self { content, id }
    }

    pub const fn content(self) -> [u8; 8] {
        self.content
    }

    pub const fn id(self) -> Option<[u8; 8]> {
        self.id
    }
}

impl std::fmt::Debug for DocumentTags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentTags").finish_non_exhaustive()
    }
}

/// A hint plus the journal-grade prefix of the existing page digest and its existing frame tag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PageRef {
    page: u32,
    check: [u8; 16],
    frame: u32,
}

impl PageRef {
    /// Names a one-based source-file page. The caller supplies the identity already read by `page_id`.
    pub fn new(page: u32, identity: &PageIdentity) -> Result<Self, Error> {
        let mut check = [0; 16];
        check.copy_from_slice(&identity.digest[..16]);
        Self::from_checks(page, check, identity.frame)
    }

    fn from_checks(page: u32, check: [u8; 16], frame: u32) -> Result<Self, Error> {
        if !(1..=MAX_PAGE).contains(&page) {
            return Err(Error::OutOfRange);
        }
        Ok(Self { page, check, frame })
    }

    pub const fn number(self) -> u32 {
        self.page
    }

    pub const fn page_check(self) -> [u8; 16] {
        self.check
    }

    pub const fn frame_tag(self) -> u32 {
        self.frame
    }
}

impl std::fmt::Debug for PageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageRef").finish_non_exhaustive()
    }
}

/// Page-relative character offsets, never byte offsets. The supplied text check is only a candidate check.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CharacterRange {
    start: u32,
    end: u32,
    check: [u8; 8],
}

impl CharacterRange {
    /// Normalization and checking actual text belong to its one producer, not another normalizer here.
    pub fn new(start: u32, end: u32, check: [u8; 8]) -> Result<Self, Error> {
        if start >= end || end > MAX_CHARACTER {
            return Err(Error::OutOfRange);
        }
        Ok(Self { start, end, check })
    }

    pub const fn offsets(self) -> (u32, u32) {
        (self.start, self.end)
    }

    pub const fn text_check(self) -> [u8; 8] {
        self.check
    }
}

impl std::fmt::Debug for CharacterRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CharacterRange").finish_non_exhaustive()
    }
}

/// A point or positive-area box in the existing shown frame. Decode validates global caps, not an unknown page's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    x: u16,
    y: u16,
    extent: Option<[u16; 2]>,
}

impl Spot {
    pub fn point(x: u16, y: u16) -> Result<Self, Error> {
        if x > MAX_POINT || y > MAX_POINT {
            return Err(Error::OutOfRange);
        }
        Ok(Self { x, y, extent: None })
    }

    pub fn area(x: u16, y: u16, width: u16, height: u16) -> Result<Self, Error> {
        if width == 0 || height == 0 || u32::from(x) + u32::from(width) > u32::from(MAX_POINT) || u32::from(y) + u32::from(height) > u32::from(MAX_POINT) {
            return Err(Error::OutOfRange);
        }
        Ok(Self {
            x,
            y,
            extent: Some([width, height]),
        })
    }

    pub const fn origin(self) -> (u16, u16) {
        (self.x, self.y)
    }

    pub const fn extent(self) -> Option<[u16; 2]> {
        self.extent
    }
}

/// One target on one page. Unsupported future grains are refused by the parser, not partly interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Page,
    Characters(CharacterRange),
    Spot(Spot),
}

/// A fixed-size value, never retained per tab by this module. Its checks contain no plaintext, not a secrecy guarantee.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Locator {
    document: DocumentTags,
    page: PageRef,
    target: Target,
}

impl Locator {
    pub const fn new(document: DocumentTags, page: PageRef, target: Target) -> Self {
        Self { document, page, target }
    }

    pub const fn document(self) -> DocumentTags {
        self.document
    }

    pub const fn page(self) -> PageRef {
        self.page
    }

    pub const fn target(self) -> Target {
        self.target
    }
}

impl std::fmt::Debug for Locator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Locator").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
