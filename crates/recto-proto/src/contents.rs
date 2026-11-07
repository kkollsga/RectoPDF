//! A page's contents on the wire: what's drawn, as records, for the host to write out as SVG. Same rules as the rest of this crate: every decoder is total on arbitrary bytes, every length is proved against
//! the frame before it's used, and everything the worker claims is checked ([`StreamCheck`]) before anything is written.
//!
//! **Streaming.** The host asks for the next piece, and gets a frame of records of at most [`MAX_CONTENTS_FRAME`], usually about [`CONTENTS_CHUNK`]. So nothing piles up on either side: a page of any size
//! costs the host one frame at a time. Only an image's bytes can run across frames. A path, a clip or a run of text is whole in its one frame, and one too large for that goes as a picture instead.

use crate::jpeg::{JpegFacts, JpegScan};
use crate::{HEADER_LEN, Header, Kind, ProtoError, Reader, frame};

/// Largest frame. It holds [`MAX_PATH_SEGMENTS`] and room to spare.
pub const MAX_CONTENTS_FRAME: u32 = 16 * 1024 * 1024;
/// A worker stops adding to a frame past this. An image's piece is this long.
pub const CONTENTS_CHUNK: usize = 1024 * 1024;
/// The most points in one path. A real map's busiest are tens of thousands. Past this, the path is drawn as a picture.
pub const MAX_PATH_SEGMENTS: u32 = 1_000_000;
/// The most characters or glyphs in one run. A longer one goes as several.
pub const MAX_RUN_ITEMS: u32 = 4096;
/// The most dash lengths kept. PDF sets no limit, and nothing real comes close.
pub const MAX_DASHES: usize = 32;
/// The longest font name kept.
pub const MAX_FAMILY: usize = 64;
/// How many clips can stack up on an object. SVG readers follow these by recursion, so it's held down. Past this, the object is drawn as a picture.
pub const MAX_CLIP_CHAIN: u8 = 32;
/// The most clips one page may define.
pub const MAX_CLIPS: u32 = 1 << 20;
/// The longest row an image may have. What's being written holds a row or two of it, so this is what a worker can make the host hold, before anything has reached a file or a cap on one. Past this, the worker draws a picture instead.
pub const MAX_IMAGE_ROW_BYTES: u64 = 8 * 1024 * 1024;
/// The most glyphs one page may define.
pub const MAX_GLYPHS: u32 = 1 << 20;
/// How deep opacity groups nest. Same reason as [`MAX_CLIP_CHAIN`], and a form this deep is drawn as a picture.
pub const MAX_GROUP_NESTING: u32 = 16;

/// How text comes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TextMode {
    /// Exact outlines, with invisible real text over them: looks right and can be selected. Largest.
    Both = 0,
    /// Exact outlines only. Can't be selected.
    Paths = 1,
    /// Real text only, in the named font if the reader has it. Smallest, can be selected, and can look different.
    Selectable = 2,
}

/// What a page uses that can't be seen from PDFium's objects, from the file's own structure. Each only makes the worker look harder or use a picture.
pub mod hints {
    /// Some graphics state has a soft mask.
    pub const SOFT_MASK: u8 = 1;
    /// Some graphics state has a blend mode other than Normal.
    pub const BLEND: u8 = 2;
    /// Some pattern is a shading: a fill that reads as plain white through PDFium's calls.
    pub const SHADING_PATTERN: u8 = 4;
    /// The document has optional content that's off by default.
    pub const HIDDEN_LAYERS: u8 = 8;
    /// Two layers may share a name, so a name can't stand for a layer.
    pub const LAYER_NAMES_REPEAT: u8 = 16;
    /// Some JPEG on the page has a `/Decode` or `/ColorTransform` that changes what its bytes mean, so its bytes alone wouldn't be the same picture. PDFium's calls don't show those.
    pub const JPEG_READ_DIFFERENTLY: u8 = 32;
    /// Every hint. For a file the structure reader couldn't read, which says nothing about what it uses.
    pub const ALL: u8 = SOFT_MASK | BLEND | SHADING_PATTERN | HIDDEN_LAYERS | LAYER_NAMES_REPEAT | JPEG_READ_DIFFERENTLY;
}

/// Ask for a page's contents, or the next piece of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentsJob {
    pub generation: u64,
    pub job_id: u32,
    pub page: u32,
    /// 0 starts from the top. Then each counts up, and a worker that has lost its place answers [`crate::FailCode::BadRequest`].
    pub seq: u32,
    /// The scale pictures are drawn at, where a picture has to stand in.
    pub scale: f32,
    /// Part of the page, in points from its top left, as `x, y, width, height`. All zero is the whole page.
    pub rect: [f32; 4],
    pub text: TextMode,
    pub hints: u8,
}

pub const CONTENTS_JOB_LEN: usize = 8 + 4 + 4 + 4 + 4 + 16 + 1 + 1;

impl ContentsJob {
    /// Refuses a job that can't be done. The host checks before sending, and the worker checks again.
    pub fn check(&self) -> Result<(), ProtoError> {
        if !(crate::MIN_SCALE..=crate::MAX_SCALE).contains(&self.scale) {
            return Err(ProtoError("scale out of range"));
        }
        if self.hints & !hints::ALL != 0 {
            return Err(ProtoError("unknown hints"));
        }
        if self.rect != [0.0; 4] {
            let [x, y, width, height] = self.rect;
            if !self.rect.iter().all(|value| value.is_finite()) || x < 0.0 || y < 0.0 || width <= 0.0 || height <= 0.0 || x + width > crate::MAX_PAGE_POINTS || y + height > crate::MAX_PAGE_POINTS {
                return Err(ProtoError("a rectangle that can't be on a page"));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(CONTENTS_JOB_LEN);
        p.extend_from_slice(&self.generation.to_le_bytes());
        for value in [self.job_id, self.page, self.seq] {
            p.extend_from_slice(&value.to_le_bytes());
        }
        p.extend_from_slice(&self.scale.to_le_bytes());
        for value in self.rect {
            p.extend_from_slice(&value.to_le_bytes());
        }
        p.push(self.text as u8);
        p.push(self.hints);
        frame(Kind::ContentsRequest, &p)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader(payload);
        let job = Self {
            generation: r.u64()?,
            job_id: r.u32()?,
            page: r.u32()?,
            seq: r.u32()?,
            scale: r.f32()?,
            rect: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
            text: match r.u8()? {
                0 => TextMode::Both,
                1 => TextMode::Paths,
                2 => TextMode::Selectable,
                _ => return Err(ProtoError("unknown text mode")),
            },
            hints: r.u8()?,
        };
        r.end()?;
        job.check()?;
        Ok(job)
    }
}

/// One piece of a page's contents: whether it's the last, then records.
const CONTENTS_HEADER_LEN: usize = 4 + 4 + 1;

/// Starts a reply frame, for a worker to add records to.
pub fn begin_contents(out: &mut Vec<u8>) {
    out.clear();
    out.resize(HEADER_LEN + CONTENTS_HEADER_LEN, 0);
}

/// Finishes a reply frame started with [`begin_contents`]. It's an error if it's grown past what any reply may be.
pub fn finish_contents(out: &mut [u8], job_id: u32, seq: u32, done: bool) -> Result<(), ProtoError> {
    let len = out.len().checked_sub(HEADER_LEN).ok_or(ProtoError("payload too short"))?;
    if len > MAX_CONTENTS_FRAME as usize {
        return Err(ProtoError("contents frame too long"));
    }
    out[..HEADER_LEN].copy_from_slice(&Header { kind: Kind::Contents, len: len as u32 }.encode());
    out[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&job_id.to_le_bytes());
    out[HEADER_LEN + 4..HEADER_LEN + 8].copy_from_slice(&seq.to_le_bytes());
    out[HEADER_LEN + 8] = u8::from(done);
    Ok(())
}

/// A reply's fixed part, and where its records start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentsHeader {
    pub job_id: u32,
    pub seq: u32,
    pub done: bool,
}

/// Reads a reply's fixed part and checks every record in it for form, so what uses them can't meet a malformed one. Returns the header and the records.
pub fn decode_contents(payload: &[u8]) -> Result<(ContentsHeader, &[u8]), ProtoError> {
    let mut r = Reader(payload);
    let (job_id, seq, done) = (r.u32()?, r.u32()?, r.u8()?);
    let done = match done {
        0 => false,
        1 => true,
        _ => return Err(ProtoError("unknown contents flag")),
    };
    let records = r.0;
    let mut check = Records::new(records);
    while check.next_record()?.is_some() {}
    Ok((ContentsHeader { job_id, seq, done }, records))
}

/// A transform: `[a, b, c, d, e, f]`, as PDF writes it.
pub type Mat = [f32; 6];

/// A path's look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintStyle<'a> {
    /// Red, green, blue, opacity. `None` paints nothing.
    pub fill: Option<[u8; 4]>,
    pub even_odd: bool,
    pub stroke: Option<[u8; 4]>,
    pub width: f32,
    pub cap: u8,
    pub join: u8,
    pub dash_phase: f32,
    /// [`f32`]s, little-endian, already known finite.
    dashes: &'a [u8],
}

impl PaintStyle<'_> {
    pub fn dashes(&self) -> impl Iterator<Item = f32> + '_ {
        self.dashes.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes))
    }
}

/// One point of a path. Three [`SEG_CURVE`] in a row are one curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireSeg {
    /// [`SEG_MOVE`], [`SEG_LINE`] or [`SEG_CURVE`].
    pub kind: u8,
    pub close: bool,
    pub x: f32,
    pub y: f32,
}

pub const SEG_MOVE: u8 = 0;
pub const SEG_LINE: u8 = 1;
pub const SEG_CURVE: u8 = 2;
const SEG_LEN: usize = 9;
const SEG_CLOSE: u8 = 0x80;

/// Points already known to be well formed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segs<'a>(&'a [u8]);

impl Segs<'_> {
    pub fn iter(&self) -> impl Iterator<Item = WireSeg> + '_ {
        self.0.as_chunks::<SEG_LEN>().0.iter().map(|bytes| WireSeg {
            kind: bytes[0] & 3,
            close: bytes[0] & SEG_CLOSE != 0,
            x: f32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
            y: f32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
        })
    }

    pub fn len(&self) -> usize {
        self.0.len() / SEG_LEN
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

const ITEM_LEN: usize = 12;

/// `(value, x, y)` in a run: a glyph's id or a character's code, already known to be well formed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Items<'a>(&'a [u8]);

impl Items<'_> {
    pub fn iter(&self) -> impl Iterator<Item = (u32, f32, f32)> + Clone + '_ {
        self.0.as_chunks::<ITEM_LEN>().0.iter().map(|bytes| (u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]), f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]])))
    }

    pub fn len(&self) -> usize {
        self.0.len() / ITEM_LEN
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How an image's bytes are coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageCoding {
    /// A file's own JPEG bytes.
    Jpeg,
    /// Rows of raw pixels, left to right and top to bottom: 1 grey, 3 red green blue, 4 with alpha.
    Pixels { channels: u8 },
}

/// Why something that's an image in the SVG isn't an image in the file, or is one that couldn't go as itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Why {
    /// An image in the file, as itself.
    Image = 0,
    /// PDFium has no call that reads a shading.
    Shading = 1,
    /// A fill or stroke with a tiling pattern.
    TilingPattern = 2,
    /// A fill or stroke with a shading pattern.
    ShadingPattern = 3,
    /// A blend mode or soft mask on one object.
    BlendOrMask = 4,
    /// A group that may carry a blend mode or soft mask.
    GroupEffects = 5,
    /// Text whose outlines PDFium doesn't give, such as a Type 3 font.
    TextOutline = 6,
    /// Something clipped by text.
    TextClip = 7,
    /// Too many points, clips or nested forms for the file to hold directly.
    TooComplex = 8,
    /// Annotations and form fields.
    Annotations = 9,
    /// Everything left on the page, because so much had to be a picture that one picture was better.
    Flattened = 10,
    /// An image with a mask too large to take apart at its own resolution.
    MaskedImage = 11,
    /// An image PDFium could draw but not hand over.
    ImageUnreadable = 12,
    /// Something crossing the edge of a box, so that nothing outside it is in the file.
    OutsideBox = 13,
    /// Something clipped by a shape whose fill rule would change it, which PDFium doesn't give.
    ClipRule = 14,
    /// An image as itself, at its own size, but through PDFium's drawing, to apply its mask.
    ImageWithMask = 15,
}

impl Why {
    fn from_u8(value: u8) -> Result<Self, ProtoError> {
        Ok(match value {
            0 => Self::Image,
            1 => Self::Shading,
            2 => Self::TilingPattern,
            3 => Self::ShadingPattern,
            4 => Self::BlendOrMask,
            5 => Self::GroupEffects,
            6 => Self::TextOutline,
            7 => Self::TextClip,
            8 => Self::TooComplex,
            9 => Self::Annotations,
            10 => Self::Flattened,
            11 => Self::MaskedImage,
            12 => Self::ImageUnreadable,
            13 => Self::OutsideBox,
            14 => Self::ClipRule,
            15 => Self::ImageWithMask,
            _ => return Err(ProtoError("unknown image reason")),
        })
    }
}

/// Something the worker wants said about the export, as a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NoteCode {
    /// Objects with numbers that aren't finite, left out.
    NotFinite = 1,
    /// Objects in a layer that's off, left out.
    HiddenLayer = 2,
    /// Characters of text that draws nothing, kept as selectable text.
    InvisibleText = 3,
    /// Objects PDFium couldn't say anything about, left out.
    Unknown = 4,
    /// Objects whose layer wasn't looked up, past the limit. They're in the picture of the whole page, which obeys it, and any text in them isn't kept.
    LayersUnchecked = 5,
    /// Objects past the limit, on a page drawn whole instead.
    ObjectsSkipped = 6,
    /// JPEGs whose EXIF says to turn them, which a page takes no notice of and a reader does, so they went as PNG.
    JpegTurned = 7,
    /// JPEGs that aren't sound as JPEGs, so they went as PNG.
    JpegUnsound = 8,
}

impl NoteCode {
    fn from_u8(value: u8) -> Result<Self, ProtoError> {
        Ok(match value {
            1 => Self::NotFinite,
            2 => Self::HiddenLayer,
            3 => Self::InvisibleText,
            4 => Self::Unknown,
            5 => Self::LayersUnchecked,
            6 => Self::ObjectsSkipped,
            7 => Self::JpegTurned,
            8 => Self::JpegUnsound,
            _ => return Err(ProtoError("unknown note")),
        })
    }
}

/// One thing on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Record<'a> {
    /// First, once. The document's size in points, and what takes the page's own coordinates there.
    Page { width: f32, height: f32, matrix: Mat },
    Path { matrix: Mat, style: PaintStyle<'a>, segs: Segs<'a> },
    /// Defines clip `id`, inside `parent`. Ids count up from 1, and 0 is no clip.
    ClipDefine { id: u32, parent: u32, matrix: Mat, segs: Segs<'a> },
    /// What's next is clipped by this.
    Clip { id: u32 },
    GroupBegin { opacity: u8 },
    GroupEnd,
    /// Defines glyph `id`, in ems, y up. Ids count up from 1.
    GlyphDefine { id: u32, segs: Segs<'a> },
    GlyphRun { matrix: Mat, fill: Option<[u8; 4]>, stroke: Option<[u8; 4]>, stroke_width: f32, items: Items<'a> },
    /// Real text, in thousandths of an em along `matrix`. Each item's value is a scalar value, already checked.
    TextRun { matrix: Mat, fill: Option<[u8; 4]>, bold: bool, italic: bool, generic: u8, family: &'a [u8], items: Items<'a> },
    ImageBegin { coding: ImageCoding, why: Why, opacity: u8, width: u32, height: u32, total: u64, matrix: Mat },
    ImageBytes(&'a [u8]),
    ImageEnd,
    Note { code: NoteCode, count: u32 },
}

const TAG_PAGE: u8 = 1;
const TAG_PATH: u8 = 2;
const TAG_CLIP_DEFINE: u8 = 3;
const TAG_CLIP: u8 = 4;
const TAG_GROUP_BEGIN: u8 = 5;
const TAG_GROUP_END: u8 = 6;
const TAG_GLYPH_DEFINE: u8 = 7;
const TAG_GLYPH_RUN: u8 = 8;
const TAG_TEXT_RUN: u8 = 9;
const TAG_IMAGE_BEGIN: u8 = 10;
const TAG_IMAGE_BYTES: u8 = 11;
const TAG_IMAGE_END: u8 = 12;
const TAG_NOTE: u8 = 13;

const FLAG_FILL: u8 = 1;
const FLAG_EVEN_ODD: u8 = 2;
const FLAG_STROKE: u8 = 4;
const FLAG_BOLD: u8 = 8;
const FLAG_ITALIC: u8 = 16;

/// Reads records one at a time from checked bytes or hostile ones alike. Anything malformed is an error, not a panic.
pub struct Records<'a>(Reader<'a>);

impl<'a> Records<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(Reader(bytes))
    }

    fn finite(&mut self) -> Result<f32, ProtoError> {
        let value = self.0.f32()?;
        if value.is_finite() { Ok(value) } else { Err(ProtoError("a number that isn't finite")) }
    }

    fn matrix(&mut self) -> Result<Mat, ProtoError> {
        Ok([self.finite()?, self.finite()?, self.finite()?, self.finite()?, self.finite()?, self.finite()?])
    }

    fn rgba(&mut self) -> Result<[u8; 4], ProtoError> {
        self.0.take()
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], ProtoError> {
        let (head, rest) = self.0.0.split_at_checked(len).ok_or(ProtoError("payload too short"))?;
        self.0.0 = rest;
        Ok(head)
    }

    fn segs(&mut self) -> Result<Segs<'a>, ProtoError> {
        let count = self.0.u32()?;
        if count > MAX_PATH_SEGMENTS {
            return Err(ProtoError("a path with too many points"));
        }
        let bytes = self.bytes(count as usize * SEG_LEN)?;
        for seg in bytes.as_chunks::<SEG_LEN>().0 {
            if seg[0] & 0x7F > SEG_CURVE {
                return Err(ProtoError("unknown segment kind"));
            }
            let (x, y) = (f32::from_le_bytes([seg[1], seg[2], seg[3], seg[4]]), f32::from_le_bytes([seg[5], seg[6], seg[7], seg[8]]));
            if !x.is_finite() || !y.is_finite() {
                return Err(ProtoError("a number that isn't finite"));
            }
        }
        Ok(Segs(bytes))
    }

    fn items(&mut self, chars: bool) -> Result<Items<'a>, ProtoError> {
        let count = self.0.u32()?;
        if count > MAX_RUN_ITEMS {
            return Err(ProtoError("a run with too many items"));
        }
        let bytes = self.bytes(count as usize * ITEM_LEN)?;
        for item in bytes.as_chunks::<ITEM_LEN>().0 {
            let value = u32::from_le_bytes([item[0], item[1], item[2], item[3]]);
            if chars && char::from_u32(value).is_none() {
                return Err(ProtoError("character isn't a scalar value"));
            }
            let (x, y) = (f32::from_le_bytes([item[4], item[5], item[6], item[7]]), f32::from_le_bytes([item[8], item[9], item[10], item[11]]));
            if !x.is_finite() || !y.is_finite() {
                return Err(ProtoError("a number that isn't finite"));
            }
        }
        Ok(Items(bytes))
    }

    /// The next record, or `None` at the end of the bytes.
    pub fn next_record(&mut self) -> Result<Option<Record<'a>>, ProtoError> {
        if self.0.0.is_empty() {
            return Ok(None);
        }
        Ok(Some(match self.0.u8()? {
            TAG_PAGE => Record::Page { width: self.finite()?, height: self.finite()?, matrix: self.matrix()? },
            TAG_PATH => {
                let matrix = self.matrix()?;
                let flags = self.0.u8()?;
                if flags & !(FLAG_FILL | FLAG_EVEN_ODD | FLAG_STROKE) != 0 {
                    return Err(ProtoError("unknown path flags"));
                }
                let (fill, stroke) = (self.rgba()?, self.rgba()?);
                let (width, cap, join, dash_phase) = (self.finite()?, self.0.u8()?, self.0.u8()?, self.finite()?);
                let dash_count = usize::from(self.0.u8()?);
                if dash_count > MAX_DASHES {
                    return Err(ProtoError("too many dashes"));
                }
                let dashes = self.bytes(dash_count * 4)?;
                if dashes.as_chunks::<4>().0.iter().any(|bytes| !f32::from_le_bytes(*bytes).is_finite()) {
                    return Err(ProtoError("a number that isn't finite"));
                }
                let style = PaintStyle { fill: (flags & FLAG_FILL != 0).then_some(fill), even_odd: flags & FLAG_EVEN_ODD != 0, stroke: (flags & FLAG_STROKE != 0).then_some(stroke), width, cap, join, dash_phase, dashes };
                Record::Path { matrix, style, segs: self.segs()? }
            }
            TAG_CLIP_DEFINE => Record::ClipDefine { id: self.0.u32()?, parent: self.0.u32()?, matrix: self.matrix()?, segs: self.segs()? },
            TAG_CLIP => Record::Clip { id: self.0.u32()? },
            TAG_GROUP_BEGIN => Record::GroupBegin { opacity: self.0.u8()? },
            TAG_GROUP_END => Record::GroupEnd,
            TAG_GLYPH_DEFINE => Record::GlyphDefine { id: self.0.u32()?, segs: self.segs()? },
            TAG_GLYPH_RUN => {
                let matrix = self.matrix()?;
                let flags = self.0.u8()?;
                if flags & !(FLAG_FILL | FLAG_STROKE) != 0 {
                    return Err(ProtoError("unknown run flags"));
                }
                let (fill, stroke, stroke_width) = (self.rgba()?, self.rgba()?, self.finite()?);
                Record::GlyphRun { matrix, fill: (flags & FLAG_FILL != 0).then_some(fill), stroke: (flags & FLAG_STROKE != 0).then_some(stroke), stroke_width, items: self.items(false)? }
            }
            TAG_TEXT_RUN => {
                let matrix = self.matrix()?;
                let flags = self.0.u8()?;
                if flags & !(FLAG_FILL | FLAG_BOLD | FLAG_ITALIC) != 0 {
                    return Err(ProtoError("unknown text flags"));
                }
                let fill = self.rgba()?;
                let generic = self.0.u8()?;
                if generic > 2 {
                    return Err(ProtoError("unknown font family"));
                }
                let family_len = usize::from(self.0.u8()?);
                if family_len > MAX_FAMILY {
                    return Err(ProtoError("a font name too long"));
                }
                let family = self.bytes(family_len)?;
                Record::TextRun { matrix, fill: (flags & FLAG_FILL != 0).then_some(fill), bold: flags & FLAG_BOLD != 0, italic: flags & FLAG_ITALIC != 0, generic, family, items: self.items(true)? }
            }
            TAG_IMAGE_BEGIN => {
                let coding = match self.0.u8()? {
                    0 => ImageCoding::Jpeg,
                    channels @ (1 | 3 | 4) => ImageCoding::Pixels { channels },
                    _ => return Err(ProtoError("unknown image coding")),
                };
                let why = Why::from_u8(self.0.u8()?)?;
                Record::ImageBegin { coding, why, opacity: self.0.u8()?, width: self.0.u32()?, height: self.0.u32()?, total: self.0.u64()?, matrix: self.matrix()? }
            }
            TAG_IMAGE_BYTES => {
                let len = self.0.u32()?;
                if len > MAX_CONTENTS_FRAME {
                    return Err(ProtoError("too many image bytes at once"));
                }
                Record::ImageBytes(self.bytes(len as usize)?)
            }
            TAG_IMAGE_END => Record::ImageEnd,
            TAG_NOTE => Record::Note { code: NoteCode::from_u8(self.0.u8()?)?, count: self.0.u32()? },
            _ => return Err(ProtoError("unknown record")),
        }))
    }
}

/// Adds records to a frame. The worker's side, so a layout lives in one place.
pub mod put {
    use super::*;

    fn matrix(out: &mut Vec<u8>, m: &Mat) {
        for value in m {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    pub fn page(out: &mut Vec<u8>, width: f32, height: f32, m: &Mat) {
        out.push(TAG_PAGE);
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        matrix(out, m);
    }

    /// Adds one point. A caller writes the count with [`segs_begin`] and fills it in with [`segs_end`], so points can come straight from the renderer.
    pub fn seg(out: &mut Vec<u8>, kind: u8, close: bool, x: f32, y: f32) {
        out.push(kind | if close { SEG_CLOSE } else { 0 });
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }

    /// Leaves room for a count of points, and says where.
    pub fn segs_begin(out: &mut Vec<u8>) -> usize {
        let at = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        at
    }

    pub fn segs_end(out: &mut [u8], at: usize, count: u32) {
        out[at..at + 4].copy_from_slice(&count.to_le_bytes());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn path_begin(out: &mut Vec<u8>, m: &Mat, fill: Option<[u8; 4]>, even_odd: bool, stroke: Option<[u8; 4]>, width: f32, cap: u8, join: u8, dash_phase: f32, dashes: &[f32]) {
        out.push(TAG_PATH);
        matrix(out, m);
        out.push((u8::from(fill.is_some()) * FLAG_FILL) | (u8::from(even_odd) * FLAG_EVEN_ODD) | (u8::from(stroke.is_some()) * FLAG_STROKE));
        out.extend_from_slice(&fill.unwrap_or_default());
        out.extend_from_slice(&stroke.unwrap_or_default());
        out.extend_from_slice(&width.to_le_bytes());
        out.push(cap);
        out.push(join);
        out.extend_from_slice(&dash_phase.to_le_bytes());
        let dashes = &dashes[..dashes.len().min(MAX_DASHES)];
        out.push(dashes.len() as u8);
        for dash in dashes {
            out.extend_from_slice(&dash.to_le_bytes());
        }
    }

    pub fn clip_define_begin(out: &mut Vec<u8>, id: u32, parent: u32, m: &Mat) {
        out.push(TAG_CLIP_DEFINE);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&parent.to_le_bytes());
        matrix(out, m);
    }

    pub fn clip(out: &mut Vec<u8>, id: u32) {
        out.push(TAG_CLIP);
        out.extend_from_slice(&id.to_le_bytes());
    }

    pub fn group_begin(out: &mut Vec<u8>, opacity: u8) {
        out.push(TAG_GROUP_BEGIN);
        out.push(opacity);
    }

    pub fn group_end(out: &mut Vec<u8>) {
        out.push(TAG_GROUP_END);
    }

    pub fn glyph_define_begin(out: &mut Vec<u8>, id: u32) {
        out.push(TAG_GLYPH_DEFINE);
        out.extend_from_slice(&id.to_le_bytes());
    }

    fn items(out: &mut Vec<u8>, items: &[(u32, f32, f32)]) {
        out.extend_from_slice(&(items.len() as u32).to_le_bytes());
        for (value, x, y) in items {
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
    }

    pub fn glyph_run(out: &mut Vec<u8>, m: &Mat, fill: Option<[u8; 4]>, stroke: Option<[u8; 4]>, stroke_width: f32, glyphs: &[(u32, f32, f32)]) {
        out.push(TAG_GLYPH_RUN);
        matrix(out, m);
        out.push((u8::from(fill.is_some()) * FLAG_FILL) | (u8::from(stroke.is_some()) * FLAG_STROKE));
        out.extend_from_slice(&fill.unwrap_or_default());
        out.extend_from_slice(&stroke.unwrap_or_default());
        out.extend_from_slice(&stroke_width.to_le_bytes());
        items(out, glyphs);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_run(out: &mut Vec<u8>, m: &Mat, fill: Option<[u8; 4]>, bold: bool, italic: bool, generic: u8, family: &[u8], chars: &[(u32, f32, f32)]) {
        out.push(TAG_TEXT_RUN);
        matrix(out, m);
        out.push((u8::from(fill.is_some()) * FLAG_FILL) | (u8::from(bold) * FLAG_BOLD) | (u8::from(italic) * FLAG_ITALIC));
        out.extend_from_slice(&fill.unwrap_or_default());
        out.push(generic);
        let family = &family[..family.len().min(MAX_FAMILY)];
        out.push(family.len() as u8);
        out.extend_from_slice(family);
        items(out, chars);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn image_begin(out: &mut Vec<u8>, coding: ImageCoding, why: Why, opacity: u8, width: u32, height: u32, total: u64, m: &Mat) {
        out.push(TAG_IMAGE_BEGIN);
        out.push(match coding {
            ImageCoding::Jpeg => 0,
            ImageCoding::Pixels { channels } => channels,
        });
        out.push(why as u8);
        out.push(opacity);
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        matrix(out, m);
    }

    /// Leaves room for some image bytes, and says where the length goes, for bytes that come straight from a bitmap.
    pub fn image_bytes_begin(out: &mut Vec<u8>) -> usize {
        out.push(TAG_IMAGE_BYTES);
        let at = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        at
    }

    pub fn image_bytes_end(out: &mut [u8], at: usize) {
        let len = (out.len() - at - 4) as u32;
        out[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }

    pub fn image_end(out: &mut Vec<u8>) {
        out.push(TAG_IMAGE_END);
    }

    pub fn note(out: &mut Vec<u8>, code: NoteCode, count: u32) {
        out.push(TAG_NOTE);
        out.push(code as u8);
        out.extend_from_slice(&count.to_le_bytes());
    }
}

/// An image in progress, as the host follows it.
#[derive(Clone, Debug)]
struct OpenImage {
    left: u64,
    jpeg: Option<JpegScan>,
}

/// The rules of a page's stream of records, held from outside the worker. Bounded on purpose: at most a byte per clip, under a cap.
#[derive(Debug, Default)]
pub struct StreamCheck {
    page_seen: bool,
    image: Option<OpenImage>,
    groups: u32,
    /// How many clips are stacked in each one defined so far, so a chain can't grow past the limit by way of a worker's word for it. At most [`MAX_CLIPS`] bytes.
    clip_depths: Vec<u8>,
    glyphs: u32,
    image_bytes_said: u64,
    jpeg_facts: JpegFacts,
}

impl StreamCheck {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks the next record against everything before it. A violation kills the worker.
    pub fn accept(&mut self, record: &Record<'_>) -> Result<(), ProtoError> {
        if let Some(open) = self.image.as_mut() {
            return match record {
                Record::ImageBytes(bytes) => {
                    if let Some(jpeg) = open.jpeg.as_mut() {
                        jpeg.feed(bytes, &mut Vec::new())?;
                    }
                    open.left = open.left.checked_sub(bytes.len() as u64).ok_or(ProtoError("more image bytes than it said"))?;
                    Ok(())
                }
                Record::ImageEnd if open.left == 0 => {
                    if let Some(jpeg) = open.jpeg.as_ref() {
                        jpeg.finish()?;
                        self.jpeg_facts.add(&jpeg.facts());
                    }
                    self.image = None;
                    Ok(())
                }
                Record::ImageEnd => Err(ProtoError("an image short of bytes")),
                _ => Err(ProtoError("something else in the middle of an image")),
            };
        }
        if !self.page_seen {
            return match record {
                Record::Page { width, height, .. } => {
                    for edge in [*width, *height] {
                        if edge <= 0.0 || edge > crate::MAX_PAGE_POINTS {
                            return Err(ProtoError("page size out of range"));
                        }
                    }
                    self.page_seen = true;
                    Ok(())
                }
                _ => Err(ProtoError("contents before the page")),
            };
        }
        match record {
            Record::Page { .. } => Err(ProtoError("a second page")),
            Record::Path { .. } | Record::GlyphRun { .. } | Record::TextRun { .. } | Record::Note { .. } => {
                if let Record::GlyphRun { items, .. } = record
                    && items.iter().any(|(id, _, _)| id == 0 || id > self.glyphs)
                {
                    return Err(ProtoError("a glyph nobody defined"));
                }
                Ok(())
            }
            Record::ClipDefine { id, parent, .. } => {
                if *id != self.clip_depths.len() as u32 + 1 || *id > MAX_CLIPS {
                    return Err(ProtoError("clips out of order"));
                }
                let depth = match parent {
                    0 => 1,
                    parent => self.clip_depths.get(*parent as usize - 1).ok_or(ProtoError("a clip nobody defined"))?.saturating_add(1),
                };
                if depth > MAX_CLIP_CHAIN {
                    return Err(ProtoError("clips stacked too deep"));
                }
                self.clip_depths.push(depth);
                Ok(())
            }
            Record::Clip { id } => {
                if *id as usize > self.clip_depths.len() { Err(ProtoError("a clip nobody defined")) } else { Ok(()) }
            }
            Record::GroupBegin { .. } => {
                self.groups += 1;
                if self.groups > MAX_GROUP_NESTING { Err(ProtoError("groups nested too deep")) } else { Ok(()) }
            }
            Record::GroupEnd => {
                self.groups = self.groups.checked_sub(1).ok_or(ProtoError("a group that wasn't begun"))?;
                Ok(())
            }
            Record::GlyphDefine { id, .. } => {
                if *id != self.glyphs + 1 || *id > MAX_GLYPHS {
                    return Err(ProtoError("glyphs out of order"));
                }
                self.glyphs = *id;
                Ok(())
            }
            Record::ImageBegin { coding, width, height, total, .. } => {
                let (left, jpeg) = match coding {
                    ImageCoding::Jpeg => (*total, Some(JpegScan::new(false))),
                    ImageCoding::Pixels { channels } => {
                        if *width == 0 || *height == 0 {
                            return Err(ProtoError("an image with no size"));
                        }
                        let row = u64::from(*width).saturating_mul(u64::from(*channels));
                        if row > MAX_IMAGE_ROW_BYTES {
                            return Err(ProtoError("an image's row is too long"));
                        }
                        let bytes = row.checked_mul(u64::from(*height)).ok_or(ProtoError("an image's bytes differ from its size"))?;
                        if bytes != *total {
                            return Err(ProtoError("an image's bytes differ from its size"));
                        }
                        (bytes, None)
                    }
                };
                self.image_bytes_said = self.image_bytes_said.saturating_add(left);
                self.image = Some(OpenImage { left, jpeg });
                Ok(())
            }
            Record::ImageBytes(_) | Record::ImageEnd => Err(ProtoError("image bytes with no image")),
        }
    }

    /// Checks how a stream ended. `done` from a worker that left an image open is a violation.
    pub fn finish(&self) -> Result<(), ProtoError> {
        if self.image.is_some() {
            return Err(ProtoError("an image left open"));
        }
        if !self.page_seen {
            return Err(ProtoError("no page at all"));
        }
        if self.groups != 0 {
            return Err(ProtoError("a group left open"));
        }
        Ok(())
    }

    /// What the JPEGs so far carry besides their pictures, as read from their bytes and not from a worker's word.
    pub fn jpeg_facts(&self) -> JpegFacts {
        self.jpeg_facts
    }

    /// Raw bytes of all images so far, by what they said. For a caller holding its own cap.
    pub fn image_bytes_total(&self) -> u64 {
        self.image_bytes_said
    }
}

/// Checks a reply against the job that asked.
pub fn validate_contents(job: &ContentsJob, header: &ContentsHeader) -> Result<(), ProtoError> {
    if header.job_id != job.job_id || header.seq != job.seq {
        return Err(ProtoError("contents answer a different job"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Mat {
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    }

    fn sample_stream() -> Vec<u8> {
        let mut out = Vec::new();
        put::page(&mut out, 612.0, 792.0, &identity());
        put::clip_define_begin(&mut out, 1, 0, &identity());
        let at = put::segs_begin(&mut out);
        put::seg(&mut out, SEG_MOVE, false, 0.0, 0.0);
        put::seg(&mut out, SEG_LINE, true, 1.0, 1.0);
        put::segs_end(&mut out, at, 2);
        put::clip(&mut out, 1);
        put::path_begin(&mut out, &identity(), Some([1, 2, 3, 255]), true, Some([4, 5, 6, 128]), 1.5, 1, 2, 0.5, &[1.0, 2.0]);
        let at = put::segs_begin(&mut out);
        put::seg(&mut out, SEG_MOVE, false, 3.0, 4.0);
        put::seg(&mut out, SEG_CURVE, false, 5.0, 6.0);
        put::segs_end(&mut out, at, 2);
        put::group_begin(&mut out, 200);
        put::glyph_define_begin(&mut out, 1);
        let at = put::segs_begin(&mut out);
        put::seg(&mut out, SEG_MOVE, false, 0.1, 0.1);
        put::segs_end(&mut out, at, 1);
        put::glyph_run(&mut out, &identity(), Some([0, 0, 0, 255]), None, 0.0, &[(1, 0.0, 0.0), (1, 500.0, 0.0)]);
        put::text_run(&mut out, &identity(), None, true, false, 1, b"Font", &[('a' as u32, 0.0, 0.0)]);
        put::group_end(&mut out);
        put::image_begin(&mut out, ImageCoding::Pixels { channels: 3 }, Why::Image, 255, 2, 2, 12, &identity());
        let at = put::image_bytes_begin(&mut out);
        out.extend_from_slice(&[9; 12]);
        put::image_bytes_end(&mut out, at);
        put::image_end(&mut out);
        put::note(&mut out, NoteCode::HiddenLayer, 3);
        out
    }

    fn all(bytes: &[u8]) -> Result<u32, ProtoError> {
        let mut records = Records::new(bytes);
        let mut check = StreamCheck::new();
        let mut count = 0;
        while let Some(record) = records.next_record()? {
            check.accept(&record)?;
            count += 1;
        }
        check.finish()?;
        Ok(count)
    }

    /// Failure: what the worker writes and what the host reads are different layouts, so every export fails or comes out garbled.
    #[test]
    fn every_record_reads_back_as_written() {
        assert_eq!(all(&sample_stream()), Ok(13));
        let bytes = sample_stream();
        let mut records = Records::new(&bytes);
        let mut path = None;
        while let Some(record) = records.next_record().unwrap() {
            if let Record::Path { style, segs, .. } = record {
                path = Some((style.fill, style.stroke, style.width, style.dashes().collect::<Vec<_>>(), segs.iter().collect::<Vec<_>>()));
            }
        }
        let (fill, stroke, width, dashes, segs) = path.unwrap();
        assert_eq!((fill, stroke, width), (Some([1, 2, 3, 255]), Some([4, 5, 6, 128]), 1.5));
        assert_eq!(dashes, vec![1.0, 2.0]);
        assert_eq!(segs[1], WireSeg { kind: SEG_CURVE, close: false, x: 5.0, y: 6.0 });
    }

    /// Failure: a decoder panics or reads out of bounds on a compromised worker's bytes, taking the host with it. Every cut of real bytes, and bits flipped at random, must come back as a value or an error.
    #[test]
    fn hostile_bytes_are_an_error_and_never_a_panic() {
        let good = sample_stream();
        for cut in 0..good.len() {
            let _ = all(&good[..cut]);
        }
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize
        };
        for _ in 0..20_000 {
            let mut bad = good.clone();
            for _ in 0..1 + next() % 4 {
                let at = next() % bad.len();
                bad[at] = next() as u8;
            }
            let _ = all(&bad);
            let _ = decode_contents(&bad);
            let _ = ContentsJob::decode(&bad);
        }
    }

    /// Failure: a claimed count allocates or walks past the frame, so a hostile worker's small frame costs the host a great deal.
    #[test]
    fn a_claimed_count_is_proved_by_the_frame() {
        let mut out = Vec::new();
        put::path_begin(&mut out, &identity(), None, false, None, 0.0, 0, 0, 0.0, &[]);
        out.extend_from_slice(&MAX_PATH_SEGMENTS.to_le_bytes());
        assert_eq!(Records::new(&out).next_record(), Err(ProtoError("payload too short")));
        out.truncate(out.len() - 4);
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Records::new(&out).next_record(), Err(ProtoError("a path with too many points")));
    }

    fn with_page(extra: impl Fn(&mut Vec<u8>)) -> Result<u32, ProtoError> {
        let mut out = Vec::new();
        put::page(&mut out, 10.0, 10.0, &identity());
        extra(&mut out);
        all(&out)
    }

    /// Failure: a hostile worker asks for what SVG readers choke on, or what the writer would write wrong: a clip that doesn't exist, a chain past the limit, a glyph nobody defined, an image that isn't the size it says, or a group left open.
    #[test]
    fn the_stream_is_held_to_its_rules() {
        assert_eq!(with_page(|out| put::clip(out, 1)), Err(ProtoError("a clip nobody defined")));
        let clip = |out: &mut Vec<u8>, id, parent| {
            put::clip_define_begin(out, id, parent, &identity());
            let at = put::segs_begin(out);
            put::segs_end(out, at, 0);
        };
        assert_eq!(with_page(|out| clip(out, 5, 0)), Err(ProtoError("clips out of order")));
        assert_eq!(with_page(|out| clip(out, 1, 9)), Err(ProtoError("a clip nobody defined")));
        let chain = |count: u32| {
            with_page(move |out| {
                for id in 1..=count {
                    clip(out, id, id - 1);
                }
            })
        };
        assert!(chain(u32::from(MAX_CLIP_CHAIN)).is_ok());
        assert_eq!(chain(u32::from(MAX_CLIP_CHAIN) + 1), Err(ProtoError("clips stacked too deep")));
        assert_eq!(with_page(|out| put::glyph_run(out, &identity(), None, None, 0.0, &[(1, 0.0, 0.0)])), Err(ProtoError("a glyph nobody defined")));
        assert_eq!(with_page(|out| put::image_begin(out, ImageCoding::Pixels { channels: 3 }, Why::Image, 255, 2, 2, 1, &identity())), Err(ProtoError("an image's bytes differ from its size")));
        // Three rows of 8 GiB in the host, filled by frames, with nothing reaching a cap on the file until a row ends. And at the largest of everything, no overflow in the checking.
        let row_limit = (MAX_IMAGE_ROW_BYTES / 4) as u32;
        assert_eq!(with_page(|out| put::image_begin(out, ImageCoding::Pixels { channels: 4 }, Why::Image, 255, row_limit + 1, 1, u64::from(row_limit + 1) * 4, &identity())), Err(ProtoError("an image's row is too long")));
        assert_eq!(with_page(|out| put::image_begin(out, ImageCoding::Pixels { channels: 4 }, Why::Image, 255, u32::MAX, u32::MAX, u64::MAX, &identity())), Err(ProtoError("an image's row is too long")));
        assert!(with_page(|out| put::image_begin(out, ImageCoding::Pixels { channels: 4 }, Why::Image, 255, row_limit, 1, u64::from(row_limit) * 4, &identity())).is_err_and(|error| error == ProtoError("an image left open")), "a row at the limit is fine");
        assert_eq!(
            with_page(|out| {
                put::image_begin(out, ImageCoding::Pixels { channels: 1 }, Why::Image, 255, 2, 2, 4, &identity());
                put::image_end(out);
            }),
            Err(ProtoError("an image short of bytes"))
        );
        assert_eq!(
            with_page(|out| {
                put::image_begin(out, ImageCoding::Jpeg, Why::Image, 255, 2, 2, 3, &identity());
                let at = put::image_bytes_begin(out);
                out.extend_from_slice(b"<sv");
                put::image_bytes_end(out, at);
                put::image_end(out);
            }),
            Err(ProtoError("a JPEG that doesn't start as one"))
        );
        assert_eq!(with_page(put::image_end), Err(ProtoError("image bytes with no image")));
        assert_eq!(with_page(|out| put::group_begin(out, 1)), Err(ProtoError("a group left open")));
        assert_eq!(with_page(put::group_end), Err(ProtoError("a group that wasn't begun")));
        assert_eq!(all(&[]), Err(ProtoError("no page at all")));
        let mut no_page = Vec::new();
        put::group_end(&mut no_page);
        assert_eq!(all(&no_page), Err(ProtoError("contents before the page")));
    }

    /// Failure: a NaN or an infinity from a compromised worker gets as far as the writer.
    #[test]
    fn numbers_that_are_not_finite_are_refused_at_the_door() {
        let mut out = Vec::new();
        put::page(&mut out, f32::NAN, 10.0, &identity());
        assert!(Records::new(&out).next_record().is_err());
        let mut out = Vec::new();
        put::path_begin(&mut out, &identity(), None, false, None, 0.0, 0, 0, 0.0, &[f32::INFINITY]);
        assert!(Records::new(&out).next_record().is_err());
        let mut out = Vec::new();
        put::glyph_define_begin(&mut out, 1);
        let at = put::segs_begin(&mut out);
        put::seg(&mut out, SEG_MOVE, false, 0.0, f32::NEG_INFINITY);
        put::segs_end(&mut out, at, 1);
        assert!(Records::new(&out).next_record().is_err());
    }

    /// Failure: a job with a rectangle or scale that can't be drawn gets to a worker, or a job and its reply don't round-trip.
    #[test]
    fn jobs_round_trip_and_bad_ones_are_refused() {
        let job = ContentsJob { generation: 9, job_id: 3, page: 2, seq: 7, scale: 2.0, rect: [1.0, 2.0, 3.0, 4.0], text: TextMode::Paths, hints: hints::BLEND };
        let bytes = job.encode();
        assert_eq!(ContentsJob::decode(&bytes[HEADER_LEN..]), Ok(job));
        for bad in [ContentsJob { scale: 0.0, ..job }, ContentsJob { rect: [f32::NAN, 0.0, 1.0, 1.0], ..job }, ContentsJob { rect: [0.0, 0.0, -1.0, 1.0], ..job }, ContentsJob { rect: [-1.0, 0.0, 1.0, 1.0], ..job }, ContentsJob { hints: 0xF0, ..job }] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        let mut frame = Vec::new();
        begin_contents(&mut frame);
        put::group_end(&mut frame);
        finish_contents(&mut frame, 3, 7, true).unwrap();
        assert_eq!(Header::decode(frame[..HEADER_LEN].try_into().unwrap()).unwrap().kind, Kind::Contents);
        let (header, records) = decode_contents(&frame[HEADER_LEN..]).unwrap();
        assert_eq!(header, ContentsHeader { job_id: 3, seq: 7, done: true });
        assert_eq!(records.len(), 1);
        validate_contents(&job, &header).unwrap();
        assert!(validate_contents(&ContentsJob { seq: 8, ..job }, &header).is_err());
    }
}
