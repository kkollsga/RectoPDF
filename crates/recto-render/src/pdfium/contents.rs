//! A page's contents, read from PDFium's objects and written as records, a frame at a time. Part of the PDFium seam, so `unsafe` is allowed here.
//!
//! **Decides nothing about limits.** The host holds memory, time and output from outside, and checks every record. What's here is only what PDFium's calls can say, and what to do when they can't.
//!
//! **What PDFium can't say goes as a picture of just that element.** PDFium has no call that reads a shading, a pattern, a blend mode or a soft mask. But `FPDFPageObj_SetIsActive` can turn every other object
//! off, and drawing then gives exactly that element, clip and all, on a transparent background. An object with a blend mode needs what's under it to mean anything, so where the file's structure says one
//! exists, it's drawn with everything before it, opaque. This changes the open page in memory only, and the page is closed when the walk ends, so nothing else can see it.
//!
//! **Text.** A character's outline comes from `FPDFFont_GetGlyphPath`, which goes by Unicode and back, so two codes for one letter, or a broken ToUnicode, can lead to the wrong glyph. Each is checked against the box
//! PDFium gives for the real character, and text whose outlines don't match, or that has none, as in a Type 3 font, goes as a picture. The real characters stay over it, selectable.

use super::*;
use recto_proto::jpeg::{HeaderFix, JpegScan, MAX_ADDED_PROFILE, fix_header, frame_components, profile_says_srgb};
use recto_proto::contents::{CONTENTS_CHUNK, ContentsJob, ImageCoding, MAX_CLIP_CHAIN, MAX_CLIPS, MAX_GLYPHS, MAX_GROUP_NESTING, MAX_IMAGE_ROW_BYTES, MAX_PATH_SEGMENTS, MAX_RUN_ITEMS, NoteCode, SEG_CURVE, SEG_LINE, SEG_MOVE, TextMode, Why, begin_contents, finish_contents, hints, put};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// A frame goes back at least this often, so a page of many small objects keeps showing the host it's alive and can be cancelled.
const MAX_CHUNK_TIME: Duration = Duration::from_millis(1000);
/// How many forms inside forms are walked. Past this, a form is drawn as a picture.
const MAX_LEVELS: usize = MAX_GROUP_NESTING as usize;
/// The most points in one clip's path. Past this, what it clips is a picture.
const MAX_CLIP_SEGMENTS: i32 = 65_536;
/// The most objects walked. A hostile page can hold millions, each costing little but all adding up.
const MAX_OBJECTS: u64 = 4_000_000;
/// How many pictures of one element a page gets, before one picture of the whole page is better: each is a draw over all the page's objects.
const MAX_PATCHES: u32 = 512;
/// Of those, how many can be drawn with everything under them, each of which also turns every object on and off again.
const MAX_BACKDROPS: u32 = 32;
/// All the pictures together, in pages' worth of pixels at the job's scale.
const MAX_PATCH_PAGES: u64 = 4;
/// How many small drawings may be made to learn whether something's in a layer that's off.
const MAX_PROBES: u32 = 1024;
/// How many layers are remembered. One past this is drawn to see each time, up to the limit on drawings.
const MAX_LAYERS: usize = 4096;
/// Most marks looked through on one object. Past this it hasn't been shown to be visible.
const MAX_MARKS: c_int = 64;
/// How large such a drawing is, at most, on its long side.
const PROBE_EDGE: f64 = 256.0;
/// A picture is drawn in strips of at most this, so its size isn't its memory.
const STRIP_BYTES: u64 = recto_proto::MAX_REGION_BYTES;
/// An image's own pixels in one bitmap, drawn to take its mask into account. Past this it's a picture at the job's scale.
const MAX_MASKED_BYTES: u64 = 256 * 1024 * 1024;
/// An image with a mask has its mask found by drawing it this small first. Below this size, it's drawn whole anyway.
const MASK_PROBE_EDGE: u32 = 64;
/// The largest colour profile read. sRGB's is a few kilobytes.
const MAX_PROFILE_BYTES: usize = 16 * 1024 * 1024;
/// How far a clip's points may reach once it's blown up to cover an image laid flat. PDFium's path filler clamps at 50,000, which bends any shape that goes past it, so a clip that would have to go further stays a picture.
const MAX_CLIP_REACH: f64 = 40_000.0;
/// How far a glyph's outline may be from the box PDFium gives for the character, as a share of the font's size. Boxes are in whole thousandths and outlines aren't hinted, but a wrong glyph is usually much further off than this.
const BOX_TOLERANCE_EM: f64 = 0.12;
/// How close two characters' origins are, as a share of the font's size, to be parts of one ligature.
const LIGATURE_EM: f64 = 0.01;
/// How thin a character's box is, on its thinner side and as a share of the font's size, for it to be a space that marks nothing. On 274 pages, 38,741 spaces are under 0.002 and 224 glyphs that do draw are over 0.02, with nothing between.
const SPACE_THIN_EM: f64 = 0.01;

/// `fpdf_edit.h`: object types not named in the parent.
const FPDF_PAGEOBJ_UNKNOWN: c_int = 0;
const FPDF_PAGEOBJ_TEXT: c_int = 1;
const FPDF_PAGEOBJ_PATH: c_int = 2;
const FPDF_PAGEOBJ_SHADING: c_int = 4;
const FPDF_SEGMENT_LINETO: c_int = 0;
const FPDF_SEGMENT_BEZIERTO: c_int = 1;
const FPDF_SEGMENT_MOVETO: c_int = 2;
const FPDF_FILLMODE_NONE: c_int = 0;
const FPDF_FILLMODE_ALTERNATE: c_int = 1;
const FPDF_FILLMODE_WINDING: c_int = 2;
const FPDF_COLORSPACE_UNKNOWN: c_int = 0;
const FPDF_COLORSPACE_DEVICEGRAY: c_int = 1;
const FPDF_COLORSPACE_DEVICERGB: c_int = 2;
const FPDF_COLORSPACE_ICCBASED: c_int = 7;
/// `fpdfview.h`: bitmap formats. `FORMAT_BGRX` is in the parent, by PDFium's own name.
const FORMAT_GRAY: c_int = 1;
const FORMAT_BGR: c_int = 2;
const FORMAT_BGRX: c_int = FPDFBitmap_BGRx;
const FORMAT_BGRA: c_int = 4;
/// `fpdf_annot.h`: annotations that draw nothing of their own in an ordinary page.
const FPDF_ANNOT_LINK: c_int = 2;
const FPDF_ANNOT_POPUP: c_int = 16;
/// ISO 32000-1, table 123.
const FONT_FIXED_PITCH: c_int = 1;
const FONT_SERIF: c_int = 1 << 1;
const FONT_ITALIC: c_int = 1 << 6;
const FONT_FORCE_BOLD: c_int = 1 << 18;

type M = [f64; 6];
const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `first` then `next`.
fn then(first: &M, next: &M) -> M {
    [
        first[0] * next[0] + first[1] * next[2],
        first[0] * next[1] + first[1] * next[3],
        first[2] * next[0] + first[3] * next[2],
        first[2] * next[1] + first[3] * next[3],
        first[4] * next[0] + first[5] * next[2] + next[4],
        first[4] * next[1] + first[5] * next[3] + next[5],
    ]
}

fn apply(m: &M, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn inverse(m: &M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    Some([m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det])
}

fn to_f32(m: &M) -> Option<Mat> {
    let out = m.map(|value| value as f32);
    out.iter().all(|value| value.is_finite()).then_some(out)
}

/// An area, left bottom right top, in page coordinates.
#[derive(Clone, Copy, Debug)]
struct Area {
    left: f64,
    bottom: f64,
    right: f64,
    top: f64,
}

impl Area {
    fn through(self, m: &M) -> Self {
        let corners = [apply(m, self.left, self.bottom), apply(m, self.right, self.bottom), apply(m, self.left, self.top), apply(m, self.right, self.top)];
        let mut out = Self { left: f64::INFINITY, bottom: f64::INFINITY, right: f64::NEG_INFINITY, top: f64::NEG_INFINITY };
        for (x, y) in corners {
            out.left = out.left.min(x);
            out.right = out.right.max(x);
            out.bottom = out.bottom.min(y);
            out.top = out.top.max(y);
        }
        out
    }

    fn is_finite(self) -> bool {
        [self.left, self.bottom, self.right, self.top].iter().all(|value| value.is_finite())
    }

    fn meets(self, other: Self) -> bool {
        self.left <= other.right && other.left <= self.right && self.bottom <= other.top && other.bottom <= self.top
    }

    /// More than touching.
    fn overlaps(self, other: Self) -> bool {
        self.left < other.right && other.left < self.right && self.bottom < other.top && other.bottom < self.top
    }

    fn inside(self, other: Self) -> bool {
        self.left >= other.left && self.right <= other.right && self.bottom >= other.bottom && self.top <= other.top
    }
}

/// One holder of objects on the way down: the page, or a form in it.
struct Level {
    form: FPDF_PAGEOBJECT,
    next: c_int,
    count: c_int,
    /// Takes this level's coordinates to the page's.
    matrix: M,
    /// What clips everything here: the clips of the forms above, and this form's own.
    clip: u32,
    clip_depth: u8,
    /// An opacity group opened for this form.
    group: bool,
    /// Something at this level has been drawn as a clip made of text. PDFium has no call that reads those, so what follows goes as pictures, which obey it.
    text_clip: bool,
    /// Only text is wanted from here down: the rest is in a picture already.
    text_only: bool,
}

/// How a picture of an element is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatchKind {
    /// Just this, on transparency. Everything else off.
    Alone,
    /// This and everything before it, opaque.
    OverBackdrop,
    /// Everything on the page, opaque, annotations too.
    Whole,
    /// The annotations alone.
    Annotations,
}

/// Bytes on their way to the host a frame at a time.
/// What a JPEG has to say about its colours for a browser to read it as the page does.
enum PageColour {
    /// As it is: sRGB, or what a browser takes as that.
    Plain,
    /// This profile, which isn't sRGB.
    Profile(Vec<u8>),
}

/// The JPEG to send once its mask has gone.
struct MaskedJpeg {
    data: Vec<u8>,
    width: u32,
    height: u32,
    wire: Mat,
}

enum Pending {
    /// A bitmap PDFium gave, freed when it's sent.
    Bitmap { bitmap: FPDF_BITMAP, format: c_int, stride: usize, crop: (u32, u32, u32, u32), channels: u8, y: u32 },
    /// A file's original bytes.
    Bytes { data: Vec<u8>, at: usize },
    /// A picture being drawn, strip by strip.
    Patch { kind: PatchKind, why: Why, rect: TileRect, full: (c_int, c_int), channels: u8, y: u32, strip: Vec<u8>, strip_top: u32, strip_rows: u32, stride: usize, object: FPDF_PAGEOBJECT, begun: bool },
}

/// Left, bottom, right, top, in ems.
type GlyphBounds = (f64, f64, f64, f64);

/// The "index" of a space PDFium made up, which no glyph or box goes with.
const NOT_DRAWN: u32 = u32::MAX;

/// What a layer's state is remembered by. A mark is never wrong and only takes more looks. A name, whole so that two can't be mistaken for one, is used only where the structure says it can stand for its layer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum LayerKey {
    Name(Box<[u16]>),
    Mark(usize),
}

/// What's known about whether an object is in a layer that's on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shows {
    On,
    Off,
    /// Past the limit on looking. Nothing unchecked goes out as itself, because what the page hides mustn't appear: the whole page is drawn at the end, and that obeys its layers.
    NotLookedUp,
}

/// What's known about a font, once.
#[derive(Clone)]
struct FontFacts {
    family: Vec<u8>,
    bold: bool,
    italic: bool,
    /// 0 serif, 1 sans-serif, 2 monospace, as on the wire.
    generic: u8,
}

/// Where the walk has got to. Lives as long as the job's frames keep coming, and takes the open page with it.
pub(super) struct Walk {
    api: &'static Api,
    document: FPDF_DOCUMENT,
    page_handle: FPDF_PAGE,
    text_page: FPDF_TEXTPAGE,
    job: ContentsJob,
    next_seq: u32,
    /// Takes page coordinates to the document's.
    display: M,
    /// Takes the document's to page coordinates, for the whole page.
    undisplay: M,
    /// The part wanted, in page coordinates.
    view: Area,
    /// What's wanted of the page, in device pixels at the job's scale, and the whole page.
    view_px: TileRect,
    /// Only part of the page was asked for, so a line crossing its edge gives up the characters outside.
    partial: bool,
    full_px: (c_int, c_int),
    page_w: f64,
    page_h: f64,
    levels: Vec<Level>,
    pending: Option<Pending>,
    /// Text whose real characters go over the picture of it that's on its way.
    overlay_next: Option<FPDF_PAGEOBJECT>,
    started: bool,
    phase: Phase,
    objects_seen: u64,
    current_clip: u32,
    clips: HashMap<u64, (u32, u8)>,
    next_clip: u32,
    glyphs: HashMap<(usize, u32), u32>,
    bounds: HashMap<(usize, u32), Option<GlyphBounds>>,
    next_glyph: u32,
    fonts: HashMap<usize, FontFacts>,
    /// Which characters each text object has, sorted, once anything needs them.
    chars: Option<Vec<(usize, u32)>>,
    /// Every top-level object is off. Forms whose children have all been turned off are in `forms_off`.
    all_off: bool,
    forms_off: Vec<FPDF_PAGEOBJECT>,
    /// Groups whose alpha has been taken off them, with what to put back, for a drawing of the whole page.
    alpha_taken: Vec<(FPDF_PAGEOBJECT, [c_uint; 4])>,
    /// How many of those are open now.
    groups_open: u32,
    /// Objects are on and off for a drawing over what's under it, rather than for one alone.
    backdrop_state: bool,
    patches: u32,
    backdrops: u32,
    patch_pixels: u64,
    flatten: bool,
    probes: u32,
    layers: HashMap<LayerKey, bool>,
    after_mask: Option<MaskedJpeg>,
    invisible_counted: HashSet<usize>,
    notes: [u32; 13],
    buffer: Vec<(u32, f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Objects,
    Annotations,
    Whole,
    Notes,
    Done,
}

impl Drop for Walk {
    fn drop(&mut self) {
        if let Some(Pending::Bitmap { bitmap, .. }) = self.pending.take() {
            // SAFETY: the bitmap came from PDFium for this walk and `take` means nothing else holds it.
            unsafe { (self.api.FPDFBitmap_Destroy)(bitmap) };
        }
    }
}

impl PdfiumBackend {
    /// The next piece of a page's contents, as a whole frame in `out`.
    pub(super) fn contents_frame(&mut self, job: &ContentsJob, out: &mut Vec<u8>, cancel: &dyn Cancel) -> Result<(), RenderError> {
        if job.seq == 0 {
            // Anything earlier may have left objects off, so it starts from a fresh page.
            self.close_page();
            self.load_page_for_walk(job.page)?;
            self.ready_form_view();
            self.walk = Some(Box::new(self.new_walk(job)?));
        }
        let Some(walk) = self.walk.as_mut() else { return Err(FailCode::BadRequest) };
        if walk.next_seq != job.seq || walk.job.page != job.page {
            return Err(FailCode::BadRequest);
        }
        walk.next_seq = walk.next_seq.wrapping_add(1);
        begin_contents(out);
        let done = walk.fill(out, cancel, self.form)?;
        finish_contents(out, job.job_id, job.seq, done).map_err(|_| FailCode::TooLarge)?;
        if done {
            // Objects may be off, and alphas taken: nothing else may see this page.
            self.close_page();
        }
        Ok(())
    }

    fn new_walk(&mut self, job: &ContentsJob) -> Result<Walk, RenderError> {
        let size = self.page_size(job.page)?;
        let open = self.open_page.as_ref().ok_or(FailCode::Internal)?;
        let (page, api) = (open.page, self.api);
        let mut bbox = FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
        // SAFETY: the page is live, and `bbox` is live for the call.
        let (has_box, rotation) = unsafe { ((api.FPDF_GetPageBoundingBox)(page, &raw mut bbox), (api.FPDFPage_GetRotation)(page)) };
        if has_box == 0 {
            return Err(FailCode::BadPage);
        }
        let (page_w, page_h) = (f64::from(size.width), f64::from(size.height));
        let display = display_matrix((f64::from(bbox.left), f64::from(bbox.bottom), f64::from(bbox.right), f64::from(bbox.top)), rotation, page_w, page_h).ok_or(FailCode::BadPage)?;
        let undisplay = inverse(&display).ok_or(FailCode::BadPage)?;
        let (full_w, full_h) = device_size(size.width, size.height, job.scale).map_err(|_| FailCode::TooLarge)?;
        let full_px = (c_int::try_from(full_w).map_err(|_| FailCode::TooLarge)?, c_int::try_from(full_h).map_err(|_| FailCode::TooLarge)?);
        let (rx, ry, rw, rh) = if job.rect == [0.0; 4] {
            (0.0, 0.0, page_w, page_h)
        } else {
            let [x, y, w, h] = job.rect.map(f64::from);
            // A box past the page's edge is the part of it that's on the page.
            let (right, bottom) = ((x + w).min(page_w), (y + h).min(page_h));
            if x >= right || y >= bottom {
                return Err(FailCode::BadRequest);
            }
            (x, y, right - x, bottom - y)
        };
        let scale_x = f64::from(full_w) / page_w;
        let scale_y = f64::from(full_h) / page_h;
        let px_left = (rx * scale_x).floor() as u32;
        let px_top = (ry * scale_y).floor() as u32;
        let px_right = (((rx + rw) * scale_x).ceil() as u32).clamp(px_left + 1, full_w);
        let px_bottom = (((ry + rh) * scale_y).ceil() as u32).clamp(px_top + 1, full_h);
        let view = Area { left: rx, bottom: ry, right: rx + rw, top: ry + rh }.through(&undisplay);
        let mut walk = Walk {
            api,
            document: self.document,
            page_handle: page,
            text_page: std::ptr::null_mut(),
            job: *job,
            next_seq: 0,
            display,
            undisplay,
            view,
            view_px: TileRect { x: px_left, y: px_top, width: px_right - px_left, height: px_bottom - px_top },
            partial: job.rect != [0.0; 4],
            full_px,
            page_w,
            page_h,
            levels: Vec::new(),
            pending: None,
            overlay_next: None,
            started: false,
            phase: Phase::Objects,
            objects_seen: 0,
            current_clip: 0,
            clips: HashMap::new(),
            next_clip: 1,
            glyphs: HashMap::new(),
            bounds: HashMap::new(),
            next_glyph: 1,
            fonts: HashMap::new(),
            chars: None,
            all_off: false,
            forms_off: Vec::new(),
            alpha_taken: Vec::new(),
            groups_open: 0,
            backdrop_state: false,
            patches: 0,
            backdrops: 0,
            patch_pixels: 0,
            flatten: false,
            probes: 0,
            layers: HashMap::new(),
            after_mask: None,
            invisible_counted: HashSet::new(),
            notes: [0; 13],
            buffer: Vec::new(),
        };
        // SAFETY: the page is live.
        let count = unsafe { (api.FPDFPage_CountObjects)(page) }.max(0);
        walk.levels.push(Level { form: std::ptr::null_mut(), next: 0, count, matrix: IDENTITY, clip: 0, clip_depth: 0, group: false, text_clip: false, text_only: false });
        // Made now, so this job's first deadline covers it. Hardly any page has no text, and every text mode needs it.
        walk.text_page = self.text_page_now()?;
        walk.job.rect = [rx as f32, ry as f32, rw as f32, rh as f32];
        Ok(walk)
    }
}

/// Takes a page's coordinates to its displayed ones, top left and y down, as PDFium itself does for this rotation. `bbox` is left, bottom, right, top.
fn display_matrix(bbox: (f64, f64, f64, f64), rotation: c_int, page_w: f64, page_h: f64) -> Option<M> {
    let (left, bottom, right, top) = bbox;
    let (box_w, box_h) = (right - left, top - bottom);
    if !(box_w > 0.0 && box_h > 0.0) {
        return None;
    }
    Some(match rotation {
        1 => {
            let (kx, ky) = (page_w / box_h, page_h / box_w);
            [0.0, ky, kx, 0.0, -bottom * kx, -left * ky]
        }
        2 => {
            let (kx, ky) = (page_w / box_w, page_h / box_h);
            [-kx, 0.0, 0.0, ky, right * kx, -bottom * ky]
        }
        3 => {
            let (kx, ky) = (page_w / box_h, page_h / box_w);
            [0.0, -ky, -kx, 0.0, top * kx, right * ky]
        }
        _ => {
            let (kx, ky) = (page_w / box_w, page_h / box_h);
            [kx, 0.0, 0.0, -ky, -left * kx, top * ky]
        }
    })
}

/// What's in the way of drawing an object directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clipped {
    Id(u32, u8),
    TooComplex,
    NotFinite,
    /// A shape the fill rule would change, and PDFium doesn't say which it has.
    RuleUnknown,
    /// A shape reaching outside a box that isn't a plain rectangle, so it can't be cut to it.
    CrossesBox,
}

impl Clipped {
    /// Why a picture has to stand in for what this clips, if one does.
    fn why(self) -> Option<Why> {
        match self {
            Self::TooComplex => Some(Why::TooComplex),
            Self::RuleUnknown => Some(Why::ClipRule),
            Self::CrossesBox => Some(Why::OutsideBox),
            Self::Id(..) | Self::NotFinite => None,
        }
    }
}

struct Hasher(u64);

impl Hasher {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

/// Which segments to read.
#[derive(Clone, Copy)]
enum SegSource {
    Path(FPDF_PAGEOBJECT),
    Clip(FPDF_CLIPPATH, c_int),
    Glyph(FPDF_GLYPHPATH),
}

impl Walk {
    /// Adds records until the frame is full enough. True when the page is finished.
    fn fill(&mut self, out: &mut Vec<u8>, cancel: &dyn Cancel, form: FPDF_FORMHANDLE) -> Result<bool, RenderError> {
        let started = Instant::now();
        if !self.started {
            self.started = true;
            let [rx, ry, rw, rh] = self.job.rect.map(f64::from);
            let root = then(&self.display, &[1.0, 0.0, 0.0, 1.0, -rx, -ry]);
            let root = to_f32(&root).ok_or(FailCode::BadPage)?;
            put::page(out, rw as f32, rh as f32, &root);
        }
        loop {
            if self.pending.is_some() {
                self.send_pending(out, cancel, form)?;
            }
            if out.len() >= CONTENTS_CHUNK || started.elapsed() >= MAX_CHUNK_TIME || cancel.is_cancelled() {
                return Ok(self.phase == Phase::Done && self.pending.is_none());
            }
            if self.pending.is_some() {
                continue;
            }
            if let Some(MaskedJpeg { data, width, height, wire }) = self.after_mask.take() {
                put::image_begin(out, ImageCoding::Jpeg, Why::JpegOverMask, 255, width, height, data.len() as u64, &wire);
                self.pending = Some(Pending::Bytes { data, at: 0 });
                continue;
            }
            if let Some(object) = self.overlay_next.take() {
                self.text_overlay_only(object, out)?;
                continue;
            }
            match self.phase {
                Phase::Objects => self.step(out, form)?,
                Phase::Annotations => {
                    self.phase = Phase::Whole;
                    if !self.flatten {
                        self.annotations_patch(out);
                    }
                }
                Phase::Whole => {
                    self.phase = Phase::Notes;
                    if self.flatten {
                        self.whole_patch(out);
                    }
                }
                Phase::Notes => {
                    self.phase = Phase::Done;
                    self.write_notes(out);
                }
                Phase::Done => return Ok(true),
            }
        }
    }

    fn note(&mut self, code: NoteCode) {
        self.notes[code as usize] = self.notes[code as usize].saturating_add(1);
    }

    fn write_notes(&mut self, out: &mut Vec<u8>) {
        for (code, index) in [NoteCode::NotFinite, NoteCode::HiddenLayer, NoteCode::InvisibleText, NoteCode::Unknown, NoteCode::LayersUnchecked, NoteCode::ObjectsSkipped, NoteCode::JpegTurned, NoteCode::JpegUnsound, NoteCode::JpegProfileRemoved, NoteCode::JpegProfileAdded, NoteCode::JpegMeaningAsPng, NoteCode::JpegCutAsPng].into_iter().map(|code| (code, code as usize)) {
            if self.notes[index] > 0 {
                put::note(out, code, self.notes[index]);
            }
        }
    }

    /// Takes the next object, or moves up a level.
    fn step(&mut self, out: &mut Vec<u8>, form: FPDF_FORMHANDLE) -> Result<(), RenderError> {
        let _ = form;
        let Some(level) = self.levels.last_mut() else {
            self.phase = Phase::Annotations;
            return Ok(());
        };
        if level.next >= level.count {
            let finished = self.levels.pop();
            if finished.is_some_and(|level| level.group) {
                put::group_end(out);
                self.current_clip = 0;
                self.groups_open -= 1;
            }
            return Ok(());
        }
        let (holder, index) = (level.form, level.next);
        level.next += 1;
        self.objects_seen += 1;
        if self.objects_seen > MAX_OBJECTS {
            self.flatten = true;
            self.note(NoteCode::ObjectsSkipped);
            self.close_levels(out);
            self.phase = Phase::Annotations;
            return Ok(());
        }
        // SAFETY: the page is live for this walk, `holder` is a form on it or null, and `index` is below the count read from it.
        let object = unsafe {
            if holder.is_null() {
                (self.api.FPDFPage_GetObject)(self.page_handle, index)
            } else {
                (self.api.FPDFFormObj_GetObject)(holder, index as c_ulong)
            }
        };
        if object.is_null() {
            self.note(NoteCode::Unknown);
            return Ok(());
        }
        self.object(object, out)
    }

    /// Leaves every form still open, closing what it opened.
    fn close_levels(&mut self, out: &mut Vec<u8>) {
        while let Some(level) = self.levels.pop() {
            if level.group {
                put::group_end(out);
                self.current_clip = 0;
                self.groups_open -= 1;
            }
        }
    }

    fn level(&self) -> &Level {
        // A caller holds a level, or there'd be no object to ask about.
        &self.levels[self.levels.len() - 1]
    }

    /// An object's area on the page, or `None` if it has none that makes sense.
    fn area_of(&self, object: FPDF_PAGEOBJECT) -> Option<Area> {
        let (mut left, mut bottom, mut right, mut top) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        // SAFETY: the object is live with its page, and the out-pointers are live for the call.
        let ok = unsafe { (self.api.FPDFPageObj_GetBounds)(object, &raw mut left, &raw mut bottom, &raw mut right, &raw mut top) };
        if ok == 0 {
            return None;
        }
        let area = Area { left: f64::from(left), bottom: f64::from(bottom), right: f64::from(right), top: f64::from(top) }.through(&self.level().matrix);
        area.is_finite().then_some(area)
    }

    fn object_matrix(&self, object: FPDF_PAGEOBJECT) -> Option<M> {
        let mut m = FS_MATRIX::default();
        // SAFETY: as above.
        if unsafe { (self.api.FPDFPageObj_GetMatrix)(object, &raw mut m) } == 0 {
            return None;
        }
        let m = [f64::from(m.a), f64::from(m.b), f64::from(m.c), f64::from(m.d), f64::from(m.e), f64::from(m.f)];
        m.iter().all(|value| value.is_finite()).then_some(m)
    }

    /// Red, green, blue, opacity.
    fn color_of(&self, object: FPDF_PAGEOBJECT, stroke: bool) -> Option<[u8; 4]> {
        let (mut r, mut g, mut b, mut a): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 0);
        // SAFETY: as above.
        let ok = unsafe {
            if stroke {
                (self.api.FPDFPageObj_GetStrokeColor)(object, &raw mut r, &raw mut g, &raw mut b, &raw mut a)
            } else {
                (self.api.FPDFPageObj_GetFillColor)(object, &raw mut r, &raw mut g, &raw mut b, &raw mut a)
            }
        };
        (ok != 0).then_some([r.min(255) as u8, g.min(255) as u8, b.min(255) as u8, a.min(255) as u8])
    }

    fn has_transparency(&self, object: FPDF_PAGEOBJECT) -> bool {
        // SAFETY: as above.
        unsafe { (self.api.FPDFPageObj_HasTransparency)(object) != 0 }
    }

    /// Whether the fill or stroke is a tiling pattern, which is the one kind PDFium will say. Costs nothing for one that isn't.
    fn is_tiling(&self, object: FPDF_PAGEOBJECT, stroke: bool) -> bool {
        // SAFETY: as above, and a bitmap handed back is ours to free, here.
        unsafe {
            let bitmap = if stroke { (self.api.FPDFPageObj_GetRenderedStrokePattern)(self.document, object) } else { (self.api.FPDFPageObj_GetRenderedFillPattern)(self.document, object) };
            if bitmap.is_null() {
                false
            } else {
                (self.api.FPDFBitmap_Destroy)(bitmap);
                true
            }
        }
    }

    /// What's clipping this object, defining what's new, in `out`.
    fn clip_for(&mut self, object: FPDF_PAGEOBJECT, out: &mut Vec<u8>) -> Clipped {
        let (mut id, mut depth) = (self.level().clip, self.level().clip_depth);
        // SAFETY: the object is live with its page, and so is its clip path, for as long as the page is, as PDFium's header says.
        let (clip, paths) = unsafe {
            let clip = (self.api.FPDFPageObj_GetClipPath)(object);
            (clip, if clip.is_null() { 0 } else { (self.api.FPDFClipPath_CountPaths)(clip).max(0) })
        };
        let matrix = self.level().matrix;
        for path in 0..paths {
            // SAFETY: as above, and `path` is below the count.
            let segments = unsafe { (self.api.FPDFClipPath_CountPathSegments)(clip, path) };
            if !(0..=MAX_CLIP_SEGMENTS).contains(&segments) {
                return Clipped::TooComplex;
            }
            let mut hash = Hasher::new();
            hash.bytes(&id.to_le_bytes());
            for value in matrix {
                hash.bytes(&value.to_le_bytes());
            }
            let source = SegSource::Clip(clip, path);
            let mut points = Vec::with_capacity(segments as usize);
            for index in 0..segments {
                let Some((kind, close, x, y)) = self.segment(source, index) else { return Clipped::NotFinite };
                hash.bytes(&[kind, u8::from(close)]);
                hash.bytes(&x.to_le_bytes());
                hash.bytes(&y.to_le_bytes());
                points.push((kind, x, y));
            }
            // The rule only matters where a shape overlaps itself. PDFium has no call that gives a clip's rule, so what it would matter to goes as a picture, which obeys the real one.
            if !is_one_plain_shape(&points) {
                return Clipped::RuleUnknown;
            }
            // A shape in the file is a shape handed over. So in a box, what reaches outside it is cut to it, or if it can't be, what it clips goes as a picture.
            let mut cut = None;
            if self.partial && !points.is_empty() {
                let area = points_area(&points, &matrix);
                if !area.inside(self.view) {
                    let Some(rect) = upright_rectangle(&points, &matrix) else { return Clipped::CrossesBox };
                    let kept = Area { left: rect.left.max(self.view.left), bottom: rect.bottom.max(self.view.bottom), right: rect.right.min(self.view.right), top: rect.top.min(self.view.top) };
                    // All of the box is in it, so it clips nothing in the file.
                    if rect.left <= self.view.left && rect.bottom <= self.view.bottom && rect.right >= self.view.right && rect.top >= self.view.top {
                        continue;
                    }
                    cut = Some(kept);
                }
            }
            if let Some(&(known, known_depth)) = self.clips.get(&hash.0) {
                id = known;
                depth = known_depth;
                continue;
            }
            if depth >= MAX_CLIP_CHAIN || self.next_clip > MAX_CLIPS {
                return Clipped::TooComplex;
            }
            let new = self.next_clip;
            if let Some(kept) = cut {
                // In page coordinates by now, and empty if the box and the shape don't meet, which clips everything, as it should.
                let Some(wire) = to_f32(&IDENTITY) else { return Clipped::NotFinite };
                put::clip_define_begin(out, new, id, &wire);
                let at = put::segs_begin(out);
                let (left, bottom) = (kept.left as f32, kept.bottom as f32);
                let (right, top) = (kept.right.max(kept.left) as f32, kept.top.max(kept.bottom) as f32);
                put::seg(out, SEG_MOVE, false, left, bottom);
                put::seg(out, SEG_LINE, false, right, bottom);
                put::seg(out, SEG_LINE, false, right, top);
                put::seg(out, SEG_LINE, true, left, top);
                put::segs_end(out, at, 4);
            } else {
                let Some(wire) = to_f32(&matrix) else { return Clipped::NotFinite };
                put::clip_define_begin(out, new, id, &wire);
                if self.write_segments(source, segments, out).is_none() {
                    // Read once already, so only a hostile library could get here, and the record must still be whole.
                    let at = put::segs_begin(out);
                    put::segs_end(out, at, 0);
                }
            }
            self.next_clip += 1;
            depth += 1;
            id = new;
            // A table capped by what it holds: past this a clip is simply defined again, which costs output and nothing else.
            if self.clips.len() < 1 << 16 {
                self.clips.insert(hash.0, (id, depth));
            }
        }
        Clipped::Id(id, depth)
    }

    /// One point, checked. `None` if it isn't finite or isn't a point.
    fn segment(&self, source: SegSource, index: c_int) -> Option<(u8, bool, f32, f32)> {
        // SAFETY: each handle is live with its page or font, and the index is below the count read from it. The out-pointers are live for the call.
        let (kind, close, x, y) = unsafe {
            let seg = match source {
                SegSource::Path(path) => (self.api.FPDFPath_GetPathSegment)(path, index),
                SegSource::Clip(clip, path) => (self.api.FPDFClipPath_GetPathSegment)(clip, path, index),
                SegSource::Glyph(glyph) => (self.api.FPDFGlyphPath_GetGlyphPathSegment)(glyph, index),
            };
            if seg.is_null() {
                return None;
            }
            let (mut x, mut y) = (0.0f32, 0.0f32);
            if (self.api.FPDFPathSegment_GetPoint)(seg, &raw mut x, &raw mut y) == 0 {
                return None;
            }
            ((self.api.FPDFPathSegment_GetType)(seg), (self.api.FPDFPathSegment_GetClose)(seg) != 0, x, y)
        };
        let kind = match kind {
            FPDF_SEGMENT_LINETO => SEG_LINE,
            FPDF_SEGMENT_BEZIERTO => SEG_CURVE,
            FPDF_SEGMENT_MOVETO => SEG_MOVE,
            _ => return None,
        };
        (x.is_finite() && y.is_finite()).then_some((kind, close, x, y))
    }

    /// Adds points as a whole list. `None` if one isn't good, with nothing added.
    fn write_segments(&self, source: SegSource, count: c_int, out: &mut Vec<u8>) -> Option<()> {
        let start = out.len();
        let at = put::segs_begin(out);
        for index in 0..count {
            let Some((kind, close, x, y)) = self.segment(source, index) else {
                out.truncate(start);
                return None;
            };
            put::seg(out, kind, close, x, y);
        }
        put::segs_end(out, at, count as u32);
        Some(())
    }

    fn set_clip(&mut self, id: u32, out: &mut Vec<u8>) {
        if self.current_clip != id {
            self.current_clip = id;
            put::clip(out, id);
        }
    }

    /// One object of any kind.
    fn object(&mut self, object: FPDF_PAGEOBJECT, out: &mut Vec<u8>) -> Result<(), RenderError> {
        // SAFETY: the object is live with its page.
        let kind = unsafe { (self.api.FPDFPageObj_GetType)(object) };
        if kind == FPDF_PAGEOBJ_UNKNOWN {
            self.note(NoteCode::Unknown);
            return Ok(());
        }
        // Before anything can pass over it: text that clips does so from wherever it is, off the page or outside the box. It applies to what comes after, not to itself.
        let clipped_by_text = self.level().text_clip;
        if kind == FPDF_PAGEOBJ_TEXT
            // SAFETY: the object is live with its page.
            && matches!(unsafe { (self.api.FPDFTextObj_GetTextRenderMode)(object) }, 4..=7)
            && let Some(level) = self.levels.last_mut()
        {
            level.text_clip = true;
        }
        let Some(area) = self.area_of(object) else {
            self.note(NoteCode::NotFinite);
            return Ok(());
        };
        // Whatever's outside the box is left out, rather than hidden.
        if !area.meets(self.view) {
            return Ok(());
        }
        // Asked of everything: a membership's rule can hide what's in it even in a document that lists no layers, and PDFium is the one that knows. What can't be shown to be visible goes as a picture PDFium draws.
        match self.layer_shows(object, area, kind) {
            Shows::On => {}
            Shows::Off => {
                self.note(NoteCode::HiddenLayer);
                return Ok(());
            }
            Shows::NotLookedUp => {
                self.note(NoteCode::LayersUnchecked);
                self.flatten = true;
                return Ok(());
            }
        }
        let text_only = self.level().text_only || self.flatten;
        if text_only {
            return match kind {
                FPDF_PAGEOBJ_TEXT => self.text_overlay_only(object, out),
                FPDF_PAGEOBJ_FORM => self.enter_form(object, out, true, None),
                _ => Ok(()),
            };
        }
        // Whether any of it shows can't be told, so what's drawn is the picture, and its words stay out of the file: they may be clipped to nothing.
        if clipped_by_text {
            self.patch(object, area, Why::TextClip, PatchKind::Alone, out);
            return Ok(());
        }
        match kind {
            FPDF_PAGEOBJ_PATH => self.path(object, area, out),
            FPDF_PAGEOBJ_TEXT => self.text(object, area, out),
            FPDF_PAGEOBJ_IMAGE => self.image(object, area, out),
            FPDF_PAGEOBJ_FORM => self.form(object, area, out),
            FPDF_PAGEOBJ_SHADING => {
                self.patch(object, area, Why::Shading, PatchKind::Alone, out);
                Ok(())
            }
            _ => {
                self.note(NoteCode::Unknown);
                Ok(())
            }
        }
    }

    /// A clip, ready to use. A picture stands in if it can't be written.
    fn apply_clip_or_patch(&mut self, object: FPDF_PAGEOBJECT, area: Area, out: &mut Vec<u8>) -> bool {
        match self.clip_for(object, out) {
            Clipped::Id(id, _) => {
                self.set_clip(id, out);
                true
            }
            Clipped::NotFinite => {
                self.note(NoteCode::NotFinite);
                false
            }
            other => {
                if let Some(why) = other.why() {
                    self.patch(object, area, why, PatchKind::Alone, out);
                }
                false
            }
        }
    }

    /// A picture of something that has, or may have, a blend mode or soft mask. PDFium's calls say only that something like that might be there. What's under a blend mode matters, so where the file's structure says this page uses one, it's drawn with everything under it. Past the limit on those, or inside a group, whose inside can't be drawn that way, the whole page is drawn at the end instead, since a picture on its own would be wrong.
    fn effects_patch(&mut self, object: FPDF_PAGEOBJECT, area: Area, why: Why, out: &mut Vec<u8>) {
        if self.job.hints & hints::BLEND == 0 {
            self.patch(object, area, why, PatchKind::Alone, out);
        } else if self.backdrops < MAX_BACKDROPS && self.groups_open == 0 {
            self.patch(object, area, why, PatchKind::OverBackdrop, out);
        } else {
            self.flatten = true;
        }
    }

    /// Whether what PDFium says about an object means it may carry a blend mode or soft mask. At full opacity, nothing else explains it. Below that, opacity would explain it, so it takes the file's own structure saying this page uses one of those.
    fn may_have_effects(&self, object: FPDF_PAGEOBJECT, opaque: bool) -> bool {
        self.has_transparency(object) && (opaque || self.job.hints & (hints::SOFT_MASK | hints::BLEND) != 0)
    }

    fn path(&mut self, object: FPDF_PAGEOBJECT, area: Area, out: &mut Vec<u8>) -> Result<(), RenderError> {
        let (mut fill_mode, mut stroke_on): (c_int, FPDF_BOOL) = (0, 0);
        // SAFETY: as above.
        let (segments, has_mode) = unsafe { ((self.api.FPDFPath_CountSegments)(object), (self.api.FPDFPath_GetDrawMode)(object, &raw mut fill_mode, &raw mut stroke_on)) };
        let (fill_on, stroke_on) = (has_mode != 0 && fill_mode != FPDF_FILLMODE_NONE, has_mode != 0 && stroke_on != 0);
        if segments <= 0 || (!fill_on && !stroke_on) {
            return Ok(());
        }
        if segments as u32 > MAX_PATH_SEGMENTS {
            self.patch(object, area, Why::TooComplex, PatchKind::Alone, out);
            return Ok(());
        }
        let fill = fill_on.then(|| self.color_of(object, false).unwrap_or([0, 0, 0, 255]));
        let stroke = stroke_on.then(|| self.color_of(object, true).unwrap_or([0, 0, 0, 255]));
        let opaque = fill.is_none_or(|color| color[3] == 255) && stroke.is_none_or(|color| color[3] == 255);
        if self.may_have_effects(object, opaque) {
            self.effects_patch(object, area, Why::BlendOrMask, out);
            return Ok(());
        }
        if (fill_on && self.is_tiling(object, false)) || (stroke_on && self.is_tiling(object, true)) {
            self.patch(object, area, Why::TilingPattern, PatchKind::Alone, out);
            return Ok(());
        }
        // A shading pattern reads as plain white, through PDFium's calls, and only the drawing can tell them apart.
        let reads_white = |color: Option<[u8; 4]>| color.is_some_and(|[r, g, b, _]| [r, g, b] == [255, 255, 255]);
        if self.job.hints & hints::SHADING_PATTERN != 0 && (reads_white(fill) || reads_white(stroke)) {
            // Past the limit on looking, this could be anything, and the whole page drawn at the end is right whichever it is.
            if self.probes >= MAX_PROBES {
                self.flatten = true;
                return Ok(());
            }
            if self.probe(object, area).1 {
                self.patch(object, area, Why::ShadingPattern, PatchKind::Alone, out);
                return Ok(());
            }
        }
        // A path's points are in the file whatever a browser shows of them. So in a box, one that isn't all inside goes as a picture cut to it.
        if self.partial && !area.inside(self.view) {
            self.patch(object, area, Why::OutsideBox, PatchKind::Alone, out);
            return Ok(());
        }
        let Some(matrix) = self.object_matrix(object).map(|m| then(&m, &self.level().matrix)).and_then(|m| to_f32(&m)) else {
            self.note(NoteCode::NotFinite);
            return Ok(());
        };
        if !self.apply_clip_or_patch(object, area, out) {
            return Ok(());
        }
        let mut width = 0.0f32;
        let mut dash_phase = 0.0f32;
        let mut dashes = [0.0f32; recto_proto::contents::MAX_DASHES];
        let mut dash_len = 0;
        // SAFETY: as above, and `dashes` holds as many as is passed.
        let (cap, join) = unsafe {
            (self.api.FPDFPageObj_GetStrokeWidth)(object, &raw mut width);
            (self.api.FPDFPageObj_GetDashPhase)(object, &raw mut dash_phase);
            let count = (self.api.FPDFPageObj_GetDashCount)(object).max(0) as usize;
            if count > 0 && count <= dashes.len() && (self.api.FPDFPageObj_GetDashArray)(object, dashes.as_mut_ptr(), count) != 0 {
                dash_len = count;
            }
            ((self.api.FPDFPageObj_GetLineCap)(object), (self.api.FPDFPageObj_GetLineJoin)(object))
        };
        let clean = |value: f32| if value.is_finite() { value } else { 0.0 };
        let dashes: Vec<f32> = dashes[..dash_len].iter().copied().map(clean).collect();
        let start = out.len();
        put::path_begin(out, &matrix, fill, fill_mode == FPDF_FILLMODE_ALTERNATE, stroke, clean(width), cap.clamp(0, 2) as u8, join.clamp(0, 2) as u8, clean(dash_phase), &dashes);
        if self.write_segments(SegSource::Path(object), segments, out).is_none() {
            out.truncate(start);
            self.note(NoteCode::NotFinite);
        }
        Ok(())
    }

    fn form(&mut self, object: FPDF_PAGEOBJECT, area: Area, out: &mut Vec<u8>) -> Result<(), RenderError> {
        if self.levels.len() >= MAX_LEVELS {
            self.patch(object, area, Why::TooComplex, PatchKind::Alone, out);
            return self.enter_form(object, out, true, None);
        }
        let alpha = self.color_of(object, false).map_or(255, |color| color[3]);
        if self.has_transparency(object) && self.job.hints & (hints::SOFT_MASK | hints::BLEND) != 0 {
            // A group, or a group with a blend mode or soft mask, and PDFium's calls can't tell which. The file says some such thing is on this page, so this one goes as a picture.
            self.effects_patch(object, area, Why::GroupEffects, out);
            return self.enter_form(object, out, true, None);
        }
        let group = (alpha < 255 && self.levels.len() < MAX_LEVELS).then_some(alpha);
        self.enter_form(object, out, false, group)
    }

    /// Goes down into a form. `text_only` if the rest of it is in a picture already.
    fn enter_form(&mut self, object: FPDF_PAGEOBJECT, out: &mut Vec<u8>, text_only: bool, group: Option<u8>) -> Result<(), RenderError> {
        if self.levels.len() >= MAX_LEVELS {
            return Ok(());
        }
        let Some(own) = self.object_matrix(object) else {
            self.note(NoteCode::NotFinite);
            return Ok(());
        };
        let matrix = then(&own, &self.level().matrix);
        let (clip, clip_depth) = if text_only {
            (0, 0)
        } else {
            match self.clip_for(object, out) {
                Clipped::Id(id, depth) => (id, depth),
                Clipped::NotFinite => {
                    self.note(NoteCode::NotFinite);
                    return Ok(());
                }
                other => {
                    if let (Some(why), Some(area)) = (other.why(), self.area_of(object)) {
                        self.patch(object, area, why, PatchKind::Alone, out);
                    }
                    return self.enter_form(object, out, true, None);
                }
            }
        };
        if let Some(opacity) = group {
            put::group_begin(out, opacity);
            self.current_clip = 0;
            // Pictures of what's in here are drawn through this form, and its opacity must count once. The page is thrown away after this walk, so nothing else sees it.
            let (mut r, mut g, mut b, mut a): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 0);
            // SAFETY: the form is live with its page, and the out-pointers are live for the call.
            unsafe {
                (self.api.FPDFPageObj_GetFillColor)(object, &raw mut r, &raw mut g, &raw mut b, &raw mut a);
                (self.api.FPDFPageObj_SetFillColor)(object, r, g, b, 255);
            }
            self.alpha_taken.push((object, [r, g, b, a]));
            self.groups_open += 1;
        }
        // SAFETY: as above.
        let count = unsafe { (self.api.FPDFFormObj_CountObjects)(object) }.max(0);
        let text_only = text_only || self.level().text_only;
        self.levels.push(Level { form: object, next: 0, count, matrix, clip, clip_depth, group: group.is_some(), text_clip: false, text_only });
        Ok(())
    }

    /// The characters of one text object, once the page's are sorted by object.
    fn chars_of(&mut self, object: FPDF_PAGEOBJECT) -> &[(usize, u32)] {
        if self.chars.is_none() {
            let mut all = Vec::new();
            // SAFETY: the text page is live, and each index is below its count.
            let count = unsafe { (self.api.FPDFText_CountChars)(self.text_page) }.max(0);
            let mut last_owner = 0;
            for index in 0..count.min(MAX_PAGE_CHARS as c_int) {
                // SAFETY: as above.
                let owner = unsafe { (self.api.FPDFText_GetTextObject)(self.text_page, index) } as usize;
                // A space PDFium made up between objects belongs to no object, so it goes with the one it follows.
                if owner != 0 {
                    last_owner = owner;
                }
                all.push((last_owner, index as u32));
            }
            all.sort_unstable();
            self.chars = Some(all);
        }
        let key = object as usize;
        let all = self.chars.as_deref().unwrap_or_default();
        let start = all.partition_point(|(owner, _)| *owner < key);
        let end = all.partition_point(|(owner, _)| *owner <= key);
        &all[start..end]
    }

    fn font_facts(&mut self, font: FPDF_FONT) -> FontFacts {
        if let Some(facts) = self.fonts.get(&(font as usize)) {
            return facts.clone();
        }
        let mut name = [0u8; 128];
        // SAFETY: the font is live with its page, and `name` holds what's passed, with PDFium writing only if it fits.
        let (len, flags, weight) = unsafe {
            if font.is_null() {
                (0, 0, 0)
            } else {
                ((self.api.FPDFFont_GetFamilyName)(font, name.as_mut_ptr().cast(), name.len()), (self.api.FPDFFont_GetFlags)(font), (self.api.FPDFFont_GetWeight)(font))
            }
        };
        let name = if len > 0 && len <= name.len() { &name[..len - 1] } else { &[][..] };
        let family = name.iter().copied().take(recto_proto::contents::MAX_FAMILY).collect();
        let flags = flags.max(0);
        let generic = if flags & FONT_FIXED_PITCH != 0 {
            2
        } else if flags & FONT_SERIF != 0 {
            0
        } else {
            1
        };
        let facts = FontFacts { family, bold: flags & FONT_FORCE_BOLD != 0 || weight >= 600, italic: flags & FONT_ITALIC != 0, generic };
        if self.fonts.len() < 4096 {
            self.fonts.insert(font as usize, facts.clone());
        }
        facts
    }

    /// One glyph's id, defining it in `out` if it's new. 0 if there's no outline to be had.
    fn glyph_id(&mut self, font: FPDF_FONT, code: u32, size: f32, out: &mut Vec<u8>) -> u32 {
        if let Some(&id) = self.glyphs.get(&(font as usize, code)) {
            return id;
        }
        let mut id = 0;
        // SAFETY: the font is live with its page, and the glyph path it hands back stays its own.
        let (glyph, count) = unsafe {
            let glyph = (self.api.FPDFFont_GetGlyphPath)(font, code, size);
            (glyph, if glyph.is_null() { -1 } else { (self.api.FPDFGlyphPath_CountGlyphSegments)(glyph) })
        };
        if count >= 0 && count as u32 <= MAX_PATH_SEGMENTS && self.next_glyph <= MAX_GLYPHS {
            let start = out.len();
            put::glyph_define_begin(out, self.next_glyph);
            if self.write_segments(SegSource::Glyph(glyph), count, out).is_some() {
                id = self.next_glyph;
                self.next_glyph += 1;
            } else {
                out.truncate(start);
            }
        }
        if self.glyphs.len() < MAX_GLYPHS as usize {
            self.glyphs.insert((font as usize, code), id);
        }
        id
    }

    /// The bounds of a glyph's outline, in ems, or `None` if it's empty. Remembered, because a page says the same letters many times.
    fn glyph_bounds(&mut self, font: FPDF_FONT, code: u32, size: f32) -> Option<GlyphBounds> {
        if let Some(known) = self.bounds.get(&(font as usize, code)) {
            return *known;
        }
        let found = self.read_glyph_bounds(font, code, size);
        if self.bounds.len() < MAX_GLYPHS as usize {
            self.bounds.insert((font as usize, code), found);
        }
        found
    }

    fn read_glyph_bounds(&self, font: FPDF_FONT, code: u32, size: f32) -> Option<GlyphBounds> {
        // SAFETY: as above.
        let (glyph, count) = unsafe {
            let glyph = (self.api.FPDFFont_GetGlyphPath)(font, code, size);
            (glyph, if glyph.is_null() { 0 } else { (self.api.FPDFGlyphPath_CountGlyphSegments)(glyph) })
        };
        let mut bounds: Option<GlyphBounds> = None;
        for index in 0..count.max(0) {
            let (_, _, x, y) = self.segment(SegSource::Glyph(glyph), index)?;
            let (x, y) = (f64::from(x), f64::from(y));
            bounds = Some(bounds.map_or((x, y, x, y), |(l, b, r, t)| (l.min(x), b.min(y), r.max(x), t.max(y))));
        }
        bounds
    }

    /// What one text object holds, as characters on the page: code, origin, the character's index.
    fn gather_chars(&mut self, object: FPDF_PAGEOBJECT) -> Vec<(u32, f64, f64, u32)> {
        let mut chars = self.all_chars_of(object);
        if self.partial {
            chars.retain(|&(_, x, y, index)| index == NOT_DRAWN || self.char_in_view(index, x, y));
        }
        // The one place all text passes, by every route, so none of it goes uncounted. Once for each object, since text under a picture is gathered again. Only if it's going in the file.
        // SAFETY: the object is live with its page.
        let mode = unsafe { (self.api.FPDFTextObj_GetTextRenderMode)(object) };
        if matches!(mode, 3 | 7) && self.job.text != TextMode::Paths && self.invisible_counted.insert(object as usize) {
            let drawn_chars = chars.iter().filter(|&&(_, _, _, index)| index != NOT_DRAWN).count();
            self.notes[NoteCode::InvisibleText as usize] = self.notes[NoteCode::InvisibleText as usize].saturating_add(drawn_chars as u32);
        }
        chars
    }

    /// Whether a character is in the part of the page that was asked for: what's outside is left out rather than hidden, a character at a time.
    fn char_in_view(&self, index: u32, x: f64, y: f64) -> bool {
        let (mut left, mut right, mut bottom, mut top) = (x, x, y, y);
        // SAFETY: the text page is live, the index came from its count, and the out-pointers are live for the call. On failure they stay at the origin.
        unsafe { (self.api.FPDFText_GetCharBox)(self.text_page, index as c_int, &raw mut left, &raw mut right, &raw mut bottom, &raw mut top) };
        // Only touching the box is nothing of it inside.
        Area { left, bottom, right, top }.overlaps(self.view)
    }

    fn all_chars_of(&mut self, object: FPDF_PAGEOBJECT) -> Vec<(u32, f64, f64, u32)> {
        let indexes: Vec<u32> = self.chars_of(object).iter().map(|(_, index)| *index).collect();
        let mut chars = Vec::with_capacity(indexes.len());
        let mut at = 0;
        while at < indexes.len() {
            let index = indexes[at] as c_int;
            at += 1;
            let (mut x, mut y) = (0.0f64, 0.0f64);
            // SAFETY: the text page is live, and the index came from its count.
            let (mut code, generated, hyphen, has_origin) = unsafe { ((self.api.FPDFText_GetUnicode)(self.text_page, index), (self.api.FPDFText_IsGenerated)(self.text_page, index), (self.api.FPDFText_IsHyphen)(self.text_page, index), (self.api.FPDFText_GetCharOrigin)(self.text_page, index, &raw mut x, &raw mut y)) };
            if generated == 1 {
                // PDFium makes these up where a gap in the text reads as a word break, as a TeX line has between all its words. Nothing draws them, and without them selected text runs together. A line break isn't kept: a run's end is one already.
                if code == 0x20
                    && let Some(&(_, last_x, last_y, _)) = chars.last()
                {
                    chars.push((code, last_x, last_y, NOT_DRAWN));
                }
                continue;
            }
            if (0xD800..0xDC00).contains(&code) && at < indexes.len() {
                // SAFETY: as above.
                let low = unsafe { (self.api.FPDFText_GetUnicode)(self.text_page, indexes[at] as c_int) };
                if (0xDC00..0xE000).contains(&low) {
                    code = 0x1_0000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                    at += 1;
                }
            }
            if hyphen == 1 || code == 0xFFFE {
                code = u32::from('-');
            }
            if char::from_u32(code).is_none() {
                code = 0xFFFD;
            }
            if has_origin != 0 && x.is_finite() && y.is_finite() {
                chars.push((code, x, y, index as u32));
            }
        }
        chars
    }

    fn text_overlay_only(&mut self, object: FPDF_PAGEOBJECT, out: &mut Vec<u8>) -> Result<(), RenderError> {
        if self.job.text == TextMode::Paths {
            return Ok(());
        }
        // A picture of it is on its way, and nothing can come between an image's bytes. This comes straight after.
        if self.pending.is_some() {
            self.overlay_next = Some(object);
            return Ok(());
        }
        let chars = self.gather_chars(object);
        let Some(run) = self.run_matrix(object, &chars) else { return Ok(()) };
        self.write_selectable(object, &chars, &run, None, out);
        Ok(())
    }

    /// The run's coordinates: thousandths of an em along the text's own direction, from its first character. `(to page, from page)`.
    fn run_matrix(&self, object: FPDF_PAGEOBJECT, chars: &[(u32, f64, f64, u32)]) -> Option<(M, M)> {
        let first = chars.first()?;
        let own = self.object_matrix(object)?;
        let mut size = 0.0f32;
        // SAFETY: as above.
        if unsafe { (self.api.FPDFTextObj_GetFontSize)(object, &raw mut size) } == 0 || !size.is_finite() || size == 0.0 {
            return None;
        }
        let whole = then(&own, &self.level().matrix);
        let k = f64::from(size) / 1000.0;
        let run = [whole[0] * k, whole[1] * k, whole[2] * k, whole[3] * k, first.1, first.2];
        let back = inverse(&run)?;
        Some((run, back))
    }

    fn write_selectable(&mut self, object: FPDF_PAGEOBJECT, chars: &[(u32, f64, f64, u32)], run: &(M, M), fill: Option<[u8; 4]>, out: &mut Vec<u8>) {
        let Some(matrix) = to_f32(&run.0) else { return };
        // SAFETY: the object is live with its page.
        let font = unsafe { (self.api.FPDFTextObj_GetFont)(object) };
        let facts = self.font_facts(font);
        for piece in chars.chunks(MAX_RUN_ITEMS as usize) {
            self.buffer.clear();
            for &(code, x, y, _) in piece {
                let (local_x, local_y) = apply(&run.1, x, y);
                self.buffer.push((code, local_x as f32, local_y as f32));
            }
            if self.buffer.iter().all(|(_, x, y)| x.is_finite() && y.is_finite()) {
                put::text_run(out, &matrix, fill, facts.bold, facts.italic, facts.generic, &facts.family, &self.buffer);
            }
        }
    }

    fn text(&mut self, object: FPDF_PAGEOBJECT, area: Area, out: &mut Vec<u8>) -> Result<(), RenderError> {
        // SAFETY: the object is live with its page.
        let mode = unsafe { (self.api.FPDFTextObj_GetTextRenderMode)(object) };
        let chars = self.gather_chars(object);
        // Modes 0-2 draw, 3 draws nothing, 4-6 draw and clip, 7 only clips.
        let draws = matches!(mode, 0 | 1 | 2 | 4 | 5 | 6);
        let strokes = matches!(mode, 1 | 2 | 5 | 6);
        let fills = matches!(mode, 0 | 2 | 4 | 6);
        if !draws {
            return self.text_overlay_only_with(object, &chars, out);
        }
        if chars.iter().all(|&(_, _, _, index)| index == NOT_DRAWN) {
            // PDFium's text page leaves out a second copy drawn to look bold. It's still drawn.
            self.patch(object, area, Why::TextOutline, PatchKind::Alone, out);
            return Ok(());
        }
        let fill = fills.then(|| self.color_of(object, false).unwrap_or([0, 0, 0, 255]));
        let stroke = strokes.then(|| self.color_of(object, true).unwrap_or([0, 0, 0, 255]));
        if self.job.text == TextMode::Selectable {
            let shown = fill.or(stroke).unwrap_or([0, 0, 0, 255]);
            return self.selectable_only(object, area, &chars, shown, out);
        }
        let opaque = fill.is_none_or(|color| color[3] == 255) && stroke.is_none_or(|color| color[3] == 255);
        if self.may_have_effects(object, opaque) {
            self.effects_patch(object, area, Why::BlendOrMask, out);
            return self.text_overlay_only_with(object, &chars, out);
        }
        // A pen's width goes by a transform in force when the text was set, which PDFium keeps to itself, so an outline's stroke can't be drawn the right width.
        if strokes {
            self.patch(object, area, Why::TextOutline, PatchKind::Alone, out);
            return self.text_overlay_only_with(object, &chars, out);
        }
        if (fills && self.is_tiling(object, false)) || (strokes && self.is_tiling(object, true)) {
            self.patch(object, area, Why::TilingPattern, PatchKind::Alone, out);
            return self.text_overlay_only_with(object, &chars, out);
        }
        let Some(run) = self.run_matrix(object, &chars) else {
            self.patch(object, area, Why::TextOutline, PatchKind::Alone, out);
            return Ok(());
        };
        // SAFETY: the object is live with its page.
        let font = unsafe { (self.api.FPDFTextObj_GetFont)(object) };
        let mut size = 0.0f32;
        // SAFETY: as above.
        unsafe { (self.api.FPDFTextObj_GetFontSize)(object, &raw mut size) };
        let Some(glyphs) = self.plan_glyphs(font, size, &chars, &run) else {
            self.patch(object, area, Why::TextOutline, PatchKind::Alone, out);
            return self.text_overlay_only_with(object, &chars, out);
        };
        let Some(items) = with_ids(&glyphs, |code| self.glyph_id(font, code, size, out)) else {
            self.patch(object, area, Why::TextOutline, PatchKind::Alone, out);
            return self.text_overlay_only_with(object, &chars, out);
        };
        if !self.apply_clip_or_patch(object, area, out) {
            return self.text_overlay_only_with(object, &chars, out);
        }
        let Some(matrix) = to_f32(&run.0) else { return Ok(()) };
        for piece in items.chunks(MAX_RUN_ITEMS as usize) {
            put::glyph_run(out, &matrix, fill, None, 0.0, piece);
        }
        self.text_overlay_only_with(object, &chars, out)
    }

    fn text_overlay_only_with(&mut self, object: FPDF_PAGEOBJECT, chars: &[(u32, f64, f64, u32)], out: &mut Vec<u8>) -> Result<(), RenderError> {
        if self.job.text == TextMode::Paths || chars.is_empty() {
            return Ok(());
        }
        if self.pending.is_some() {
            self.overlay_next = Some(object);
            return Ok(());
        }
        let Some(run) = self.run_matrix(object, chars) else { return Ok(()) };
        self.write_selectable(object, chars, &run, None, out);
        Ok(())
    }

    /// Text as text, in a color you can see, with the file's clip and no outlines at all.
    fn selectable_only(&mut self, object: FPDF_PAGEOBJECT, area: Area, chars: &[(u32, f64, f64, u32)], shown: [u8; 4], out: &mut Vec<u8>) -> Result<(), RenderError> {
        if !self.apply_clip_or_patch(object, area, out) {
            return self.text_overlay_only_with(object, chars, out);
        }
        let Some(run) = self.run_matrix(object, chars) else { return Ok(()) };
        self.write_selectable(object, chars, &run, Some(shown), out);
        Ok(())
    }

    /// Which glyph to draw where, in the run's coordinates. `None` if any of them can't be trusted: no outline, a ligature that can't be reached, or an outline that doesn't match PDFium's own box for the character.
    fn plan_glyphs(&mut self, font: FPDF_FONT, size: f32, chars: &[(u32, f64, f64, u32)], run: &(M, M)) -> Option<Vec<(u32, f32, f32)>> {
        if font.is_null() {
            return None;
        }
        let chars: Vec<(u32, f64, f64, u32)> = chars.iter().copied().filter(|&(_, _, _, index)| index != NOT_DRAWN).collect();
        let em = (run.0[0].hypot(run.0[1]).max(run.0[2].hypot(run.0[3]))) * 1000.0;
        let axis_aligned = (run.0[1].abs() < 1e-6 && run.0[2].abs() < 1e-6) || (run.0[0].abs() < 1e-6 && run.0[3].abs() < 1e-6);
        let mut plan = Vec::with_capacity(chars.len());
        let mut at = 0;
        while at < chars.len() {
            let (code, x, y, index) = chars[at];
            let mut used = 1;
            // Parts of one ligature share an origin.
            let mut parts = vec![code];
            while at + used < chars.len() && (chars[at + used].1 - x).abs() <= LIGATURE_EM * em && (chars[at + used].2 - y).abs() <= LIGATURE_EM * em {
                parts.push(chars[at + used].0);
                used += 1;
            }
            let glyph = if used > 1 { ligature_of(&parts)? } else { code };
            let (local_x, local_y) = apply(&run.1, x, y);
            let bounds = self.glyph_bounds(font, glyph, size);
            let (mut left, mut right, mut bottom, mut top) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            // SAFETY: the text page is live, the index came from its count, and the out-pointers are live for the call.
            let has_box = unsafe { (self.api.FPDFText_GetCharBox)(self.text_page, index as c_int, &raw mut left, &raw mut right, &raw mut bottom, &raw mut top) } != 0;
            let box_has_area = has_box && right - left > 0.0 && top - bottom > 0.0;
            match bounds {
                Some((em_left, em_bottom, em_right, em_top)) => {
                    if axis_aligned && box_has_area {
                        let unit = Area { left: em_left * 1000.0 + local_x, bottom: em_bottom * 1000.0 + local_y, right: em_right * 1000.0 + local_x, top: em_top * 1000.0 + local_y }.through(&run.0);
                        let tolerance = BOX_TOLERANCE_EM * em;
                        if (unit.left - left).abs() > tolerance || (unit.right - right).abs() > tolerance || (unit.bottom - bottom).abs() > tolerance || (unit.top - top).abs() > tolerance {
                            return None;
                        }
                    }
                    plan.push((glyph, local_x as f32, local_y as f32));
                }
                // Something with a real box and no outline would be lost. A space has neither, but a code that reads as a space may not be one: TeX's big brackets are at 0x20 and its bars at 0x0C.
                None if box_has_area && (!char::from_u32(code).is_some_and(char::is_whitespace) || marks_the_page(right - left, top - bottom, em, (run.0[0], run.0[1]), axis_aligned, self.advance_of(font, glyph, size, em))) => return None,
                None => {}
            }
            at += used;
        }
        Some(plan)
    }

    /// How far a character moves the text along, on the page.
    fn advance_of(&self, font: FPDF_FONT, code: u32, size: f32, em: f64) -> Option<f64> {
        let mut width = 0.0f32;
        // SAFETY: the font is live with its page, and the out-pointer is live for the call.
        let known = unsafe { (self.api.FPDFFont_GetGlyphWidth)(font, code, size, &raw mut width) } != 0;
        (known && width.is_finite() && size != 0.0).then(|| f64::from(width) * em / f64::from(size))
    }

    /// Whether an object draws anything, and whether any of it isn't plain white.
    fn probe(&mut self, object: FPDF_PAGEOBJECT, area: Area) -> (bool, bool) {
        self.probes += 1;
        let Some(rect) = self.device_rect(area) else { return (false, false) };
        let long = f64::from(rect.width.max(rect.height));
        let shrink = (PROBE_EDGE / long).min(1.0);
        let (full_w, full_h) = ((f64::from(self.full_px.0) * shrink).ceil().max(1.0), (f64::from(self.full_px.1) * shrink).ceil().max(1.0));
        let (x, y) = ((f64::from(rect.x) * shrink).floor(), (f64::from(rect.y) * shrink).floor());
        let width = (f64::from(rect.width) * shrink).ceil().max(1.0) as c_int + 1;
        let height = (f64::from(rect.height) * shrink).ceil().max(1.0) as c_int + 1;
        let stride = width * 4;
        let mut pixels = vec![0u8; stride as usize * height as usize];
        self.isolate(object);
        let drawn = self.draw_into(&mut pixels, width, height, stride, (-(x as c_int), -(y as c_int)), (full_w as c_int, full_h as c_int), FORMAT_BGRA, 0, false);
        self.release(object);
        if !drawn {
            // It didn't draw, so nothing's known. Taking it as drawing plain keeps it as it is.
            return (true, false);
        }
        let mut paints = false;
        let mut not_white = false;
        for pixel in pixels.as_chunks::<4>().0 {
            if pixel[3] != 0 {
                paints = true;
                // Edges of a shape blend toward nothing, and stay the shape's own colour here.
                if pixel[0] < 250 || pixel[1] < 250 || pixel[2] < 250 {
                    not_white = true;
                    break;
                }
            }
        }
        (paints, not_white)
    }

    /// Whether an object is in layers that are on. PDFium has no call that says, so each layer's mark is put on a plain box that always draws, and the box is drawn alone: PDFium itself decides. Nothing about the object can sway that, so an OCR layer's invisible text doesn't condemn its layer. A layer is known by its name where the document's names are all different, and otherwise by its mark, which is never wrong and only takes more looks.
    fn layer_shows(&mut self, object: FPDF_PAGEOBJECT, area: Area, kind: c_int) -> Shows {
        let Some(marks) = self.layer_marks_of(object) else { return Shows::NotLookedUp };
        for (mark, key) in marks {
            let on = match self.layers.get(&key) {
                Some(on) => Some(*on),
                None => {
                    if self.probes >= MAX_PROBES {
                        return Shows::NotLookedUp;
                    }
                    let on = self.mark_draws(mark);
                    if let Some(on) = on
                        && self.layers.len() < MAX_LAYERS
                    {
                        self.layers.insert(key, on);
                    }
                    on
                }
            };
            match on {
                Some(true) => {}
                Some(false) => return Shows::Off,
                // The mark wouldn't go on the box, so the only thing left to ask is the object.
                None if self.probe(object, area).0 => {}
                None => return Shows::Off,
            }
        }
        // Forms and images can name a layer in their own dictionaries, where no call reaches, so they're drawn to see. One that draws nothing has nothing to lose but text that can't be told from what's hidden.
        if self.job.hints & hints::OPTIONAL_CONTENT != 0 && (kind == FPDF_PAGEOBJ_FORM || kind == FPDF_PAGEOBJ_IMAGE) {
            if self.probes >= MAX_PROBES {
                return Shows::NotLookedUp;
            }
            if !self.probe(object, area).0 {
                return Shows::Off;
            }
        }
        Shows::On
    }

    /// Whether a plain box carrying this mark draws, or `None` if it wouldn't take the mark. The box is gone again after, and nothing else can see it, since everything else is off.
    fn mark_draws(&mut self, mark: FPDF_PAGEOBJECTMARK) -> Option<bool> {
        self.probes += 1;
        self.isolate_nothing();
        let whole = Area { left: 0.0, bottom: 0.0, right: self.page_w, top: self.page_h }.through(&self.undisplay);
        let mut pixels = [0u8; 16 * 16 * 4];
        // SAFETY: the page and mark are live, the new box is ours until `InsertObject` hands it to the page and `RemoveObject` hands it back, and then it's destroyed here.
        unsafe {
            let probe = (self.api.FPDFPageObj_CreateNewRect)(whole.left as f32, whole.bottom as f32, (whole.right - whole.left) as f32, (whole.top - whole.bottom) as f32);
            if probe.is_null() {
                return None;
            }
            (self.api.FPDFPageObj_SetFillColor)(probe, 0, 0, 0, 255);
            (self.api.FPDFPath_SetDrawMode)(probe, FPDF_FILLMODE_WINDING, 0);
            if (self.api.FPDFPageObj_AddExistingMark)(probe, mark) == 0 {
                (self.api.FPDFPageObj_Destroy)(probe);
                return None;
            }
            (self.api.FPDFPage_InsertObject)(self.page_handle, probe);
            let drawn = self.draw_into(&mut pixels, 16, 16, 16 * 4, (0, 0), (16, 16), FORMAT_BGRA, 0, false);
            (self.api.FPDFPage_RemoveObject)(self.page_handle, probe);
            (self.api.FPDFPageObj_Destroy)(probe);
            if !drawn {
                return None;
            }
        }
        Some(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0))
    }

    /// The `/OC` marks on an object, each with what it's known by, or `None` if they can't all be seen.
    fn layer_marks_of(&self, object: FPDF_PAGEOBJECT) -> Option<Vec<(FPDF_PAGEOBJECTMARK, LayerKey)>> {
        let mut found = Vec::new();
        let names_are_unique = self.job.hints & hints::LAYER_NAMES_UNSAFE == 0;
        // SAFETY: the object is live with its page, and so are its marks. Each buffer is live for its call, and PDFium writes only if it fits.
        unsafe {
            let count = (self.api.FPDFPageObj_CountMarks)(object);
            if count > MAX_MARKS {
                return None;
            }
            for index in 0..count.max(0) {
                let mark = (self.api.FPDFPageObj_GetMark)(object, index as c_ulong);
                if mark.is_null() {
                    return None;
                }
                let mut name = [0u16; 4];
                let mut len: c_ulong = 0;
                if (self.api.FPDFPageObjMark_GetName)(mark, name.as_mut_ptr(), 8, &raw mut len) == 0 {
                    return None;
                }
                if len != 6 || name[0] != u16::from(b'O') || name[1] != u16::from(b'C') {
                    continue;
                }
                let mut layer = [0u16; 128];
                let mut layer_len: c_ulong = 0;
                let named = names_are_unique && (self.api.FPDFPageObjMark_GetParamStringValue)(mark, c"Name".as_ptr(), layer.as_mut_ptr(), (layer.len() * 2) as c_ulong, &raw mut layer_len) != 0 && layer_len as usize <= layer.len() * 2;
                found.push((mark, if named { LayerKey::Name(layer[..layer_len as usize / 2].into()) } else { LayerKey::Mark(mark as usize) }));
            }
        }
        Some(found)
    }

    /// An image, as itself if it can go as itself.
    fn image(&mut self, object: FPDF_PAGEOBJECT, area: Area, out: &mut Vec<u8>) -> Result<(), RenderError> {
        // Only touching the box is nothing of it inside.
        if self.partial && !area.overlaps(self.view) {
            return Ok(());
        }
        let Some(own) = self.object_matrix(object) else {
            self.note(NoteCode::NotFinite);
            return Ok(());
        };
        let matrix = then(&own, &self.level().matrix);
        let alpha = self.color_of(object, false).map_or(255, |color| color[3]);
        if self.may_have_effects(object, alpha == 255) {
            self.effects_patch(object, area, Why::BlendOrMask, out);
            return Ok(());
        }
        // A turned image can't be cut to a box by its own pixels, and all of them would be in the file.
        if self.partial && !area.inside(self.view) && (matrix[1].abs() > 1e-6 || matrix[2].abs() > 1e-6) {
            self.patch(object, area, Why::OutsideBox, PatchKind::Alone, out);
            return Ok(());
        }
        let (mut width, mut height): (c_uint, c_uint) = (0, 0);
        // SAFETY: as above.
        if unsafe { (self.api.FPDFImageObj_GetImagePixelSize)(object, &raw mut width, &raw mut height) } == 0 || width == 0 || height == 0 {
            self.patch(object, area, Why::ImageUnreadable, PatchKind::Alone, out);
            return Ok(());
        }
        if u64::from(width) * 4 > MAX_IMAGE_ROW_BYTES {
            self.patch(object, area, Why::TooComplex, PatchKind::Alone, out);
            return Ok(());
        }
        if !self.apply_clip_or_patch(object, area, out) {
            return Ok(());
        }
        let mut metadata = FPDF_IMAGEOBJ_METADATA::default();
        // SAFETY: as above.
        unsafe { (self.api.FPDFImageObj_GetImageMetadata)(object, self.page_handle, &raw mut metadata) };
        let stencil = metadata.bits_per_pixel == 1 && metadata.colorspace == FPDF_COLORSPACE_UNKNOWN;
        let masked = stencil || self.has_mask(object, &own, width, height);
        // A whole page keeps a whole image, even past the page's edge, as a bleed goes. Only a box asks for less.
        let inside = !self.partial || area.inside(self.view);
        let mut said_why = false;
        let mask_comes_apart = !masked || (!stencil && self.job.hints & hints::MASK_NOT_SEPARABLE == 0 && u64::from(width) * u64::from(height) * 4 <= MAX_MASKED_BYTES);
        if mask_comes_apart
            && inside
            && let Some(meaning) = self.jpeg_meaning(object, &metadata)
        {
            // SAFETY: as above. The first call asks for the length, and the second has that much room.
            let data = unsafe {
                let len = (self.api.FPDFImageObj_GetImageDataRaw)(object, std::ptr::null_mut(), 0) as usize;
                let mut data = vec![0u8; len];
                let got = (self.api.FPDFImageObj_GetImageDataRaw)(object, data.as_mut_ptr().cast(), len as c_ulong) as usize;
                data.truncate(got.min(len));
                data
            };
            // The original bytes, all of them (owner, 2026-11-07), if they're sound as a JPEG and mean what the page means by them. If they don't only because of what's around the picture, that's corrected and the picture stays byte for byte (owner, 2026-11-07: "Cant we fix the decoding instead?"). Otherwise PNG.
            let mut scan = JpegScan::new(false);
            if scan.feed(&data, &mut Vec::new()).and_then(|()| scan.finish()).is_ok() {
                let facts = scan.facts();
                let has_own = facts.color_profiles > 0 || facts.foreign_profile;
                let fix = match &meaning {
                    PageColour::Plain => HeaderFix { upright: facts.is_turned(), drop_profile: facts.foreign_profile, add_profile: None },
                    PageColour::Profile(profile) => HeaderFix { upright: facts.is_turned(), drop_profile: has_own, add_profile: Some(profile) },
                };
                let fits = !matches!(meaning, PageColour::Profile(_)) || frame_components(&data) == Some(3);
                let is_changing = fix.upright || fix.drop_profile || fix.add_profile.is_some();
                let fixed = if !fits {
                    None
                } else if is_changing {
                    fix_header(&data, &fix)
                } else {
                    Some(data)
                };
                if let (Some(data), Some(wire)) = (fixed, to_f32(&matrix)) {
                    // What goes out of here does, so what's said of it is true.
                    if masked {
                        if let Some(mask) = self.alpha_of(object, &own, width, height) {
                            self.say_fix(&fix, has_own, is_changing);
                            put::image_begin(out, ImageCoding::Pixels { channels: 1 }, Why::JpegMask, 255, width, height, mask.len() as u64, &wire);
                            self.pending = Some(Pending::Bytes { data: mask, at: 0 });
                            // A mask is drawn with the image's opacity in it, which must count once.
                            self.after_mask = Some(MaskedJpeg { data, width, height, wire });
                            return Ok(());
                        }
                    } else {
                        self.say_fix(&fix, has_own, is_changing);
                        put::image_begin(out, ImageCoding::Jpeg, if is_changing { Why::JpegHeaderFixed } else { Why::Image }, alpha, width, height, data.len() as u64, &wire);
                        self.pending = Some(Pending::Bytes { data, at: 0 });
                        return Ok(());
                    }
                }
            } else {
                self.note(NoteCode::JpegUnsound);
                said_why = true;
            }
        }
        if !said_why && self.is_dct(object) {
            self.note(if inside { NoteCode::JpegMeaningAsPng } else { NoteCode::JpegCutAsPng });
        }
        let bitmap = if masked {
            if u64::from(width) * u64::from(height) * 4 > MAX_MASKED_BYTES {
                self.patch(object, area, Why::MaskedImage, PatchKind::Alone, out);
                return Ok(());
            }
            self.rendered_image(object, &own, width, height)
        } else {
            // SAFETY: as above, and what comes back is ours until `Pending` frees it.
            unsafe { (self.api.FPDFImageObj_GetBitmap)(object) }
        };
        if bitmap.is_null() {
            self.patch(object, area, Why::ImageUnreadable, PatchKind::Alone, out);
            return Ok(());
        }
        // SAFETY: the bitmap was just checked to be live.
        let (format, real_w, real_h, stride) = unsafe { ((self.api.FPDFBitmap_GetFormat)(bitmap), (self.api.FPDFBitmap_GetWidth)(bitmap), (self.api.FPDFBitmap_GetHeight)(bitmap), (self.api.FPDFBitmap_GetStride)(bitmap)) };
        let channels = match format {
            FORMAT_GRAY => 1,
            FORMAT_BGR | FORMAT_BGRX => 3,
            FORMAT_BGRA => 4,
            _ => 0,
        };
        let (Ok(real_w), Ok(real_h), Ok(stride)) = (u32::try_from(real_w), u32::try_from(real_h), usize::try_from(stride)) else {
            // SAFETY: as above.
            unsafe { (self.api.FPDFBitmap_Destroy)(bitmap) };
            return Ok(());
        };
        if channels == 0 || real_w == 0 || real_h == 0 {
            // SAFETY: as above.
            unsafe { (self.api.FPDFBitmap_Destroy)(bitmap) };
            self.patch(object, area, Why::ImageUnreadable, PatchKind::Alone, out);
            return Ok(());
        }
        let (crop, placed) = if self.partial { crop_to(real_w, real_h, &matrix, self.view) } else { ((0, 0, real_w, real_h), matrix) };
        let Some(wire) = to_f32(&placed) else {
            // SAFETY: as above.
            unsafe { (self.api.FPDFBitmap_Destroy)(bitmap) };
            return Ok(());
        };
        // A mask is drawn with its opacity already in it, and must count once.
        let opacity = if masked { 255 } else { alpha };
        // Drawn to apply its mask, it's not byte for byte, and a mask finer than the image is resampled, so it's told apart.
        let why = if masked { Why::ImageWithMask } else { Why::Image };
        put::image_begin(out, ImageCoding::Pixels { channels }, why, opacity, crop.2, crop.3, u64::from(crop.2) * u64::from(crop.3) * u64::from(channels), &wire);
        self.pending = Some(Pending::Bitmap { bitmap, format, stride, crop, channels, y: 0 });
        Ok(())
    }

    /// Says what was corrected around a JPEG.
    fn say_fix(&mut self, fix: &HeaderFix<'_>, had_own: bool, changing: bool) {
        if !changing {
            return;
        }
        if fix.upright {
            self.note(NoteCode::JpegTurned);
        }
        if fix.drop_profile && had_own {
            self.note(NoteCode::JpegProfileRemoved);
        }
        if fix.add_profile.is_some() {
            self.note(NoteCode::JpegProfileAdded);
        }
    }

    /// An image's mask at the image's own size, as how much of each pixel shows. `None` if it can't be had, and the image is drawn as before. PDFium has no call that gives a mask as stored, so a mask finer than its image is resampled to it.
    fn alpha_of(&mut self, object: FPDF_PAGEOBJECT, own: &M, width: u32, height: u32) -> Option<Vec<u8>> {
        let bitmap = self.rendered_image(object, own, width, height);
        if bitmap.is_null() {
            return None;
        }
        // SAFETY: the bitmap was just checked to be live, its buffer holds its stride times its height, and it's freed here.
        unsafe {
            let (w, h, stride, format) = ((self.api.FPDFBitmap_GetWidth)(bitmap), (self.api.FPDFBitmap_GetHeight)(bitmap), (self.api.FPDFBitmap_GetStride)(bitmap), (self.api.FPDFBitmap_GetFormat)(bitmap));
            let buffer = (self.api.FPDFBitmap_GetBuffer)(bitmap).cast::<u8>();
            let mut alpha = None;
            if format == FORMAT_BGRA && !buffer.is_null() && i64::from(w) == i64::from(width) && i64::from(h) == i64::from(height) && stride >= w * 4 {
                let pixels = std::slice::from_raw_parts(buffer, stride as usize * h as usize);
                let mut mask = Vec::with_capacity(w as usize * h as usize);
                for row in pixels.chunks_exact(stride as usize) {
                    mask.extend(row[..w as usize * 4].as_chunks::<4>().0.iter().map(|pixel| pixel[3]));
                }
                alpha = Some(mask);
            }
            (self.api.FPDFBitmap_Destroy)(bitmap);
            alpha
        }
    }

    /// Whether an image has a mask, by drawing it small and looking for anything not opaque. An image with no mask comes back opaque at any size.
    fn has_mask(&mut self, object: FPDF_PAGEOBJECT, own: &M, width: u32, height: u32) -> bool {
        let bitmap = self.rendered_image(object, own, width.min(MASK_PROBE_EDGE), height.min(MASK_PROBE_EDGE));
        // It couldn't be drawn to see. Saying yes leads to the drawing again, and a picture if that fails, which obeys whatever it may have.
        if bitmap.is_null() {
            return true;
        }
        // SAFETY: the bitmap was just checked to be live, its buffer holds its stride times its height, and it's freed here.
        unsafe {
            let (w, h, stride, format) = ((self.api.FPDFBitmap_GetWidth)(bitmap), (self.api.FPDFBitmap_GetHeight)(bitmap), (self.api.FPDFBitmap_GetStride)(bitmap), (self.api.FPDFBitmap_GetFormat)(bitmap));
            let buffer = (self.api.FPDFBitmap_GetBuffer)(bitmap).cast::<u8>();
            let mut masked = false;
            if format == FORMAT_BGRA && !buffer.is_null() && w > 0 && h > 0 && stride >= w * 4 {
                let pixels = std::slice::from_raw_parts(buffer, stride as usize * h as usize);
                'rows: for row in pixels.chunks_exact(stride as usize) {
                    for pixel in row[..w as usize * 4].as_chunks::<4>().0 {
                        if pixel[3] != 255 {
                            masked = true;
                            break 'rows;
                        }
                    }
                }
            }
            (self.api.FPDFBitmap_Destroy)(bitmap);
            masked
        }
    }

    /// The image drawn with its mask, at this size in its own pixels. It stands at its original place again afterwards.
    ///
    /// `FPDFImageObj_GetRenderedBitmap` also obeys the object's clip, in coordinates that mean nothing once the image is laid flat, and there's no call that takes a clip off. So the clip is blown up around a point inside it, until it covers the flat image, and put back after. A clip with no point that can be found inside it gives no bitmap, and the caller uses a picture.
    fn rendered_image(&mut self, object: FPDF_PAGEOBJECT, own: &M, width: u32, height: u32) -> FPDF_BITMAP {
        let flat = FS_MATRIX { a: width as f32, b: 0.0, c: 0.0, d: height as f32, e: 0.0, f: 0.0 };
        let back = FS_MATRIX { a: own[0] as f32, b: own[1] as f32, c: own[2] as f32, d: own[3] as f32, e: own[4] as f32, f: own[5] as f32 };
        // SAFETY: the object is live with its page, and so is its clip.
        let clip = unsafe { (self.api.FPDFPageObj_GetClipPath)(object) };
        let paths = if clip.is_null() {
            0
        } else {
            // SAFETY: as above.
            unsafe { (self.api.FPDFClipPath_CountPaths)(clip) }.max(0)
        };
        let moved = if paths > 0 {
            let Some((x, y, margin, reach)) = self.point_inside_clip(clip, paths) else { return std::ptr::null_mut() };
            // PDFium takes the clip through where the file placed the image, whatever matrix it has now, so that's what it has to cover. Pixels then stand this far apart.
            let (mid_x, mid_y) = apply(own, 0.5, 0.5);
            let corner = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].iter().map(|&(u, v)| { let (px, py) = apply(own, u, v); (px - mid_x).hypot(py - mid_y) }).fold(0.0, f64::max);
            let side = own[0].hypot(own[1]).min(own[2].hypot(own[3]));
            let pixels = f64::from(width.max(height)) / side;
            let Some(scale) = clip_blow_up(margin, reach, corner, pixels) else { return std::ptr::null_mut() };
            Some(([scale, 0.0, 0.0, scale, mid_x - scale * x, mid_y - scale * y], [1.0 / scale, 0.0, 0.0, 1.0 / scale, x - mid_x / scale, y - mid_y / scale]))
        } else {
            None
        };
        // SAFETY: as above, both matrices are live for their calls, and what comes back is the caller's to free.
        unsafe {
            if let Some(([a, b, c, d, e, f], _)) = moved {
                (self.api.FPDFPageObj_TransformClipPath)(object, a, b, c, d, e, f);
            }
            (self.api.FPDFPageObj_SetMatrix)(object, &raw const flat);
            let bitmap = (self.api.FPDFImageObj_GetRenderedBitmap)(self.document, self.page_handle, object);
            (self.api.FPDFPageObj_SetMatrix)(object, &raw const back);
            if let Some((_, [a, b, c, d, e, f])) = moved {
                (self.api.FPDFPageObj_TransformClipPath)(object, a, b, c, d, e, f);
            }
            bitmap
        }
    }

    /// A point inside every path of a clip, from a look over the first path's box. Going by each path's corners, which for a curve is close enough to find a point well inside it, and the middle of the look's cells is what's tried.
    fn point_inside_clip(&self, clip: FPDF_CLIPPATH, paths: c_int) -> Option<(f64, f64, f64, f64)> {
        let mut outlines: Vec<Vec<(f64, f64)>> = Vec::new();
        for path in 0..paths.min(MAX_CLIP_CHAIN as c_int) {
            // SAFETY: the clip is live with its page, and `path` is below the count.
            let count = unsafe { (self.api.FPDFClipPath_CountPathSegments)(clip, path) };
            if !(1..=MAX_CLIP_SEGMENTS).contains(&count) {
                return None;
            }
            let mut outline = Vec::with_capacity(count as usize);
            for index in 0..count {
                let (_, _, x, y) = self.segment(SegSource::Clip(clip, path), index)?;
                outline.push((f64::from(x), f64::from(y)));
            }
            outlines.push(outline);
        }
        let first = outlines.first()?;
        let (mut left, mut bottom, mut right, mut top) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &(x, y) in first {
            left = left.min(x);
            right = right.max(x);
            bottom = bottom.min(y);
            top = top.max(y);
        }
        // Middles first, since most clips are boxes or blobs, then the rest of a grid. As far from any path's edge as the grid can tell, so a thin sliver doesn't matter.
        const CELLS: u32 = 16;
        let mut best = None;
        let mut best_margin = 0.0;
        for row in 0..CELLS {
            for column in 0..CELLS {
                let x = left + (right - left) * (f64::from(column) + 0.5) / f64::from(CELLS);
                let y = bottom + (top - bottom) * (f64::from(row) + 0.5) / f64::from(CELLS);
                if outlines.iter().all(|outline| inside_outline(outline, x, y)) {
                    let margin = outlines.iter().map(|outline| distance_to_outline(outline, x, y)).fold(f64::INFINITY, f64::min);
                    if margin > best_margin {
                        best_margin = margin;
                        best = Some((x, y));
                    }
                }
            }
        }
        let (x, y) = best?;
        let reach = outlines.iter().flatten().map(|point| (point.0 - x).hypot(point.1 - y)).fold(0.0, f64::max);
        Some((x, y, best_margin, reach))
    }

    /// What an image's stored bytes have to mean for them to go as a JPEG, or `None` if they can't: "lossless" is the same picture, so anything else goes as PNG, in the colours PDFium gives. That's colour spaces a browser wouldn't read the same, a profile that isn't a whole RGB one, and what the file's structure says is decoded differently.
    fn jpeg_meaning(&self, object: FPDF_PAGEOBJECT, metadata: &FPDF_IMAGEOBJ_METADATA) -> Option<PageColour> {
        if self.job.hints & hints::JPEG_READ_DIFFERENTLY != 0 || !self.is_dct(object) {
            return None;
        }
        match metadata.colorspace {
            FPDF_COLORSPACE_DEVICEGRAY | FPDF_COLORSPACE_DEVICERGB => Some(PageColour::Plain),
            FPDF_COLORSPACE_ICCBASED => {
                let profile = self.icc_profile(object)?;
                if profile_says_srgb(&profile) {
                    Some(PageColour::Plain)
                } else if profile.len() <= MAX_ADDED_PROFILE && profile.get(36..40) == Some(b"acsp") && profile.get(16..20) == Some(b"RGB ") {
                    Some(PageColour::Profile(profile))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Whether an image is stored as a JPEG and nothing else.
    fn is_dct(&self, object: FPDF_PAGEOBJECT) -> bool {
        let mut name = [0u8; 16];
        // SAFETY: the object is live with its page, and `name` holds as many as is passed.
        let (count, len) = unsafe { ((self.api.FPDFImageObj_GetImageFilterCount)(object), (self.api.FPDFImageObj_GetImageFilter)(object, 0, name.as_mut_ptr().cast(), name.len() as c_ulong)) };
        count == 1 && len as usize <= name.len() && name.starts_with(b"DCTDecode\0")
    }

    fn icc_profile(&self, object: FPDF_PAGEOBJECT) -> Option<Vec<u8>> {
        let mut len = 0usize;
        // SAFETY: the object and page are live, and the second call's buffer holds what the first said, up to a limit no real profile comes near.
        unsafe {
            if (self.api.FPDFImageObj_GetIccProfileDataDecoded)(object, self.page_handle, std::ptr::null_mut(), 0, &raw mut len) == 0 || len > MAX_PROFILE_BYTES {
                return None;
            }
            let mut profile = vec![0u8; len];
            let mut got = 0usize;
            ((self.api.FPDFImageObj_GetIccProfileDataDecoded)(object, self.page_handle, profile.as_mut_ptr(), len, &raw mut got) != 0 && got == len).then_some(profile)
        }
    }

    /// Everything off but this and the forms it's in.
    fn isolate(&mut self, object: FPDF_PAGEOBJECT) {
        if self.backdrop_state {
            self.restore_everything();
        }
        if !self.all_off {
            self.all_off = true;
            // SAFETY: the page is live, and each index is below the count.
            unsafe {
                for index in 0..(self.api.FPDFPage_CountObjects)(self.page_handle).max(0) {
                    (self.api.FPDFPageObj_SetIsActive)((self.api.FPDFPage_GetObject)(self.page_handle, index), 0);
                }
            }
        }
        let chain: Vec<FPDF_PAGEOBJECT> = self.levels.iter().filter(|level| !level.form.is_null()).map(|level| level.form).collect();
        for form in chain {
            if !self.forms_off.contains(&form) {
                // SAFETY: the form is live with its page, and each index is below the count.
                unsafe {
                    for index in 0..(self.api.FPDFFormObj_CountObjects)(form).max(0) {
                        (self.api.FPDFPageObj_SetIsActive)((self.api.FPDFFormObj_GetObject)(form, index as c_ulong), 0);
                    }
                }
                self.forms_off.push(form);
            }
            // SAFETY: as above.
            unsafe { (self.api.FPDFPageObj_SetIsActive)(form, 1) };
        }
        // SAFETY: as above.
        unsafe { (self.api.FPDFPageObj_SetIsActive)(object, 1) };
    }

    fn release(&mut self, object: FPDF_PAGEOBJECT) {
        // SAFETY: each is live with its page.
        unsafe {
            (self.api.FPDFPageObj_SetIsActive)(object, 0);
            for level in &self.levels {
                if !level.form.is_null() {
                    (self.api.FPDFPageObj_SetIsActive)(level.form, 0);
                }
            }
        }
    }

    /// Every object on again, and every group's opacity as the file has it.
    fn restore_everything(&mut self) {
        if !self.all_off && !self.backdrop_state {
            return;
        }
        // SAFETY: each is live with its page, and each index below its count.
        unsafe {
            for index in 0..(self.api.FPDFPage_CountObjects)(self.page_handle).max(0) {
                (self.api.FPDFPageObj_SetIsActive)((self.api.FPDFPage_GetObject)(self.page_handle, index), 1);
            }
            for &form in &self.forms_off {
                for index in 0..(self.api.FPDFFormObj_CountObjects)(form).max(0) {
                    (self.api.FPDFPageObj_SetIsActive)((self.api.FPDFFormObj_GetObject)(form, index as c_ulong), 1);
                }
            }
        }
        self.all_off = false;
        self.backdrop_state = false;
        self.forms_off.clear();
    }

    /// Everything after the object being drawn is off, at each level. `next` has already moved past it.
    fn later_objects_off(&mut self) {
        self.restore_everything();
        self.backdrop_state = true;
        // SAFETY: each is live with its page, and each index below its count.
        unsafe {
            for level in &self.levels {
                for index in level.next..level.count {
                    let later = if level.form.is_null() { (self.api.FPDFPage_GetObject)(self.page_handle, index) } else { (self.api.FPDFFormObj_GetObject)(level.form, index as c_ulong) };
                    (self.api.FPDFPageObj_SetIsActive)(later, 0);
                }
            }
        }
        // So that putting everything back reaches what's just been turned off in these forms.
        self.forms_off = self.levels.iter().filter(|level| !level.form.is_null()).map(|level| level.form).collect();
    }

    /// The device pixels an area covers, within what's wanted. `None` if that's nothing.
    fn device_rect(&self, area: Area) -> Option<TileRect> {
        let on_screen = area.through(&self.display);
        let (scale_x, scale_y) = (f64::from(self.full_px.0) / self.page_w, f64::from(self.full_px.1) / self.page_h);
        // A pixel of margin all round, because a stroke's edge and its smoothing go past the object's own bounds.
        let left = ((on_screen.left * scale_x).floor() - 1.0).max(f64::from(self.view_px.x));
        let top = ((on_screen.bottom * scale_y).floor() - 1.0).max(f64::from(self.view_px.y));
        let right = ((on_screen.right * scale_x).ceil() + 1.0).min(f64::from(self.view_px.x + self.view_px.width));
        let bottom = ((on_screen.top * scale_y).ceil() + 1.0).min(f64::from(self.view_px.y + self.view_px.height));
        if !(right > left && bottom > top) {
            return None;
        }
        Some(TileRect { x: left as u32, y: top as u32, width: (right - left) as u32, height: (bottom - top) as u32 })
    }

    /// Where pixels of the job's scale sit, as an image's matrix in page coordinates.
    fn placement(&self, rect: TileRect) -> Option<Mat> {
        let (scale_x, scale_y) = (f64::from(self.full_px.0) / self.page_w, f64::from(self.full_px.1) / self.page_h);
        let at = |x: f64, y: f64| apply(&self.undisplay, x / scale_x, y / scale_y);
        let (x0, y0) = (f64::from(rect.x), f64::from(rect.y));
        let (x1, y1) = (x0 + f64::from(rect.width), y0 + f64::from(rect.height));
        let origin = at(x0, y1);
        let right = at(x1, y1);
        let up = at(x0, y0);
        to_f32(&[right.0 - origin.0, right.1 - origin.1, up.0 - origin.0, up.1 - origin.1, origin.0, origin.1])
    }

    /// A picture of one element. Counted, and past its limits, the whole page is drawn at the end instead.
    fn patch(&mut self, object: FPDF_PAGEOBJECT, area: Area, why: Why, kind: PatchKind, out: &mut Vec<u8>) {
        let Some(rect) = self.device_rect(area) else { return };
        let pixels = u64::from(rect.width) * u64::from(rect.height);
        let page_pixels = i64::from(self.full_px.0) as u64 * i64::from(self.full_px.1) as u64;
        if self.patches >= MAX_PATCHES || self.patch_pixels.saturating_add(pixels) > page_pixels.saturating_mul(MAX_PATCH_PAGES) {
            self.flatten = true;
            return;
        }
        self.patches += 1;
        self.patch_pixels += pixels;
        if kind == PatchKind::OverBackdrop {
            self.backdrops += 1;
            self.later_objects_off();
        } else {
            self.isolate(object);
        }
        self.set_clip(0, out);
        self.begin_patch(kind, why, rect, object);
    }

    /// A picture of what annotations and form fields draw, if any might. Anything else on the page is off, since it's in the file already.
    fn annotations_patch(&mut self, out: &mut Vec<u8>) {
        // SAFETY: the page is live, each index below its count, and each annotation closed before the next.
        let mut any = false;
        unsafe {
            for index in 0..(self.api.FPDFPage_GetAnnotCount)(self.page_handle).max(0) {
                let annot = (self.api.FPDFPage_GetAnnot)(self.page_handle, index);
                if annot.is_null() {
                    continue;
                }
                let subtype = (self.api.FPDFAnnot_GetSubtype)(annot);
                // A link draws nothing unless it brings its own look, as PDFium draws it if it does.
                let has_look = (self.api.FPDFAnnot_HasKey)(annot, c"AP".as_ptr()) != 0;
                (self.api.FPDFPage_CloseAnnot)(annot);
                if subtype != FPDF_ANNOT_POPUP && (subtype != FPDF_ANNOT_LINK || has_look) {
                    any = true;
                    break;
                }
            }
        }
        if any {
            self.isolate_nothing();
            self.set_clip(0, out);
            self.begin_patch(PatchKind::Annotations, Why::Annotations, self.view_px, std::ptr::null_mut());
        }
    }

    fn isolate_nothing(&mut self) {
        if self.backdrop_state {
            self.restore_everything();
        }
        if !self.all_off {
            self.all_off = true;
            // SAFETY: as in `isolate`.
            unsafe {
                for index in 0..(self.api.FPDFPage_CountObjects)(self.page_handle).max(0) {
                    (self.api.FPDFPageObj_SetIsActive)((self.api.FPDFPage_GetObject)(self.page_handle, index), 0);
                }
            }
        }
    }

    fn whole_patch(&mut self, out: &mut Vec<u8>) {
        self.close_levels(out);
        self.restore_everything();
        for (form, [r, g, b, a]) in std::mem::take(&mut self.alpha_taken) {
            // SAFETY: the form is live with its page.
            unsafe { (self.api.FPDFPageObj_SetFillColor)(form, r, g, b, a) };
        }
        self.set_clip(0, out);
        self.begin_patch(PatchKind::Whole, Why::Flattened, self.view_px, std::ptr::null_mut());
    }

    /// Sets a picture on its way. It's only said once its first strip is drawn, because what turns out to draw nothing isn't said at all.
    fn begin_patch(&mut self, kind: PatchKind, why: Why, rect: TileRect, object: FPDF_PAGEOBJECT) {
        let channels: u8 = if matches!(kind, PatchKind::Whole | PatchKind::OverBackdrop) { 3 } else { 4 };
        self.pending = Some(Pending::Patch { kind, why, rect, full: self.full_px, channels, y: 0, strip: Vec::new(), strip_top: 0, strip_rows: 0, stride: rect.width as usize * 4, object, begun: false });
    }

    /// Draws onto `pixels`.
    #[allow(clippy::too_many_arguments)]
    fn draw_into(&self, pixels: &mut [u8], width: c_int, height: c_int, stride: c_int, at: (c_int, c_int), full: (c_int, c_int), format: c_int, paper: c_ulong, annots: bool) -> bool {
        draw_plain(self.api, self.page_handle, std::ptr::null_mut(), pixels, width, height, stride, at, full, format, paper, annots)
    }

    /// Sends what's pending, as much as fits. On finishing, it ends the image.
    fn send_pending(&mut self, out: &mut Vec<u8>, cancel: &dyn Cancel, form: FPDF_FORMHANDLE) -> Result<(), RenderError> {
        let Some(mut pending) = self.pending.take() else { return Ok(()) };
        let room = CONTENTS_CHUNK.saturating_sub(out.len()).max(64 * 1024);
        let finished = match &mut pending {
            Pending::Bytes { data, at } => {
                let take = room.min(data.len() - *at);
                let mark = put::image_bytes_begin(out);
                out.extend_from_slice(&data[*at..*at + take]);
                put::image_bytes_end(out, mark);
                *at += take;
                *at == data.len()
            }
            Pending::Bitmap { bitmap, format, stride, crop, channels, y } => {
                let (x0, y0, width, height) = *crop;
                let row_len = width as usize * usize::from(*channels);
                let rows = (room / row_len).max(1) as u32;
                let end = (*y + rows).min(height);
                // SAFETY: the bitmap is live until this `Pending` is done with, its buffer holds its stride times its height, and the crop was made from its size.
                let source = unsafe {
                    let base = (self.api.FPDFBitmap_GetBuffer)(*bitmap).cast::<u8>().cast_const();
                    if base.is_null() {
                        (self.api.FPDFBitmap_Destroy)(*bitmap);
                        return Err(FailCode::Internal);
                    }
                    let h = (self.api.FPDFBitmap_GetHeight)(*bitmap).max(0) as usize;
                    std::slice::from_raw_parts(base, *stride * h)
                };
                let mark = put::image_bytes_begin(out);
                let in_bytes: usize = match *format {
                    FORMAT_GRAY => 1,
                    FORMAT_BGR => 3,
                    _ => 4,
                };
                for row in *y..end {
                    let start = (y0 + row) as usize * *stride + x0 as usize * in_bytes;
                    let Some(line) = source.get(start..start + width as usize * in_bytes) else {
                        // SAFETY: as above, and `pending` goes with this return, so nothing can reach it after.
                        unsafe { (self.api.FPDFBitmap_Destroy)(*bitmap) };
                        return Err(FailCode::Internal);
                    };
                    if in_bytes == 1 {
                        out.extend_from_slice(line);
                    } else {
                        for pixel in line.chunks_exact(in_bytes) {
                            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                            if *channels == 4 {
                                out.push(pixel[3]);
                            }
                        }
                    }
                }
                put::image_bytes_end(out, mark);
                *y = end;
                let done = *y == height;
                if done {
                    // SAFETY: nothing else holds it, and the `Pending` is about to go with `finished`.
                    unsafe { (self.api.FPDFBitmap_Destroy)(*bitmap) };
                    *bitmap = std::ptr::null_mut();
                }
                done
            }
            Pending::Patch { kind, why, rect, full, channels, y, strip, strip_top, strip_rows, stride, object, begun } => {
                let in_strip = *y >= *strip_top && *y < *strip_top + *strip_rows;
                if !in_strip {
                    let rows = (STRIP_BYTES / *stride as u64).clamp(1, u64::from(rect.height)) as u32;
                    let rows = rows.min(rect.height - *y);
                    let needed = *stride * rows as usize;
                    strip.clear();
                    strip.resize(needed, 0);
                    let (format, paper, annots) = match kind {
                        PatchKind::Alone => (FORMAT_BGRA, 0, false),
                        PatchKind::Annotations => (FORMAT_BGRA, 0, true),
                        // What's under it, so the annotations that come on top aren't drawn underneath too.
                        PatchKind::OverBackdrop => (FORMAT_BGRX, PAPER_WHITE, false),
                        PatchKind::Whole => (FORMAT_BGRX, PAPER_WHITE, true),
                    };
                    let at = (-(rect.x as c_int), -((rect.y + *y) as c_int));
                    let width = rect.width as c_int;
                    let form_handle = if annots { form } else { std::ptr::null_mut() };
                    if cancel.is_cancelled() {
                        // A stale job's frames stop being asked for. This one just stays where it is.
                        self.pending = Some(pending);
                        return Ok(());
                    }
                    let drawn = draw_plain(self.api, self.page_handle, form_handle, strip, width, rows as c_int, *stride as c_int, at, *full, format, paper, annots);
                    if !drawn {
                        return Err(FailCode::Internal);
                    }
                    *strip_top = *y;
                    *strip_rows = rows;
                    if !*begun {
                        let blank = rows == rect.height && matches!(kind, PatchKind::Alone | PatchKind::Annotations) && strip.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0);
                        let placed = self.placement(*rect);
                        let (Some(matrix), false) = (placed, blank) else {
                            if *kind == PatchKind::Alone {
                                let object = *object;
                                self.release(object);
                            }
                            return Ok(());
                        };
                        put::image_begin(out, ImageCoding::Pixels { channels: *channels }, *why, 255, rect.width, rect.height, u64::from(rect.width) * u64::from(rect.height) * u64::from(*channels), &matrix);
                        *begun = true;
                    }
                }
                let row_out = rect.width as usize * usize::from(*channels);
                let rows = ((room / row_out).max(1) as u32).min(*strip_top + *strip_rows - *y);
                let mark = put::image_bytes_begin(out);
                for row in 0..rows {
                    let start = (*y + row - *strip_top) as usize * *stride;
                    let Some(line) = strip.get(start..start + *stride) else { return Err(FailCode::Internal) };
                    for pixel in line.as_chunks::<4>().0 {
                        out.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                        if *channels == 4 {
                            out.push(pixel[3]);
                        }
                    }
                }
                put::image_bytes_end(out, mark);
                *y += rows;
                let done = *y == rect.height;
                if done && !object.is_null() && *kind == PatchKind::Alone {
                    let object = *object;
                    self.release(object);
                }
                done
            }
        };
        if finished {
            put::image_end(out);
        } else {
            self.pending = Some(pending);
        }
        Ok(())
    }

}


/// Whether a character with no outline and a space's code draws anything anyway, going by its box on the page. A real space's box is a line along the text, as long as the space is wide. Across or up the page that's plain to see. At an angle the box holds that line corner to corner, so it has to fit the line's own shape and be no longer than the space.
fn marks_the_page(width: f64, height: f64, em: f64, along: (f64, f64), axis_aligned: bool, advance: Option<f64>) -> bool {
    let thin = SPACE_THIN_EM * em;
    if axis_aligned {
        return width.min(height) > thin;
    }
    let length = along.0.hypot(along.1);
    if length <= 0.0 {
        return true;
    }
    let (x, y) = ((along.0 / length).abs(), (along.1 / length).abs());
    (width * y - height * x).abs() > thin || advance.is_none_or(|advance| width * x + height * y > advance + thin)
}

/// Each planned glyph with its id, or `None` if any has none. One with no id, past the limit on glyphs or on a glyph's size, can't be drawn, and leaving it out of its run would lose it with nothing said.
fn with_ids(plan: &[(u32, f32, f32)], mut id_of: impl FnMut(u32) -> u32) -> Option<Vec<(u32, f32, f32)>> {
    plan.iter()
        .map(|&(code, x, y)| {
            let id = id_of(code);
            (id != 0).then_some((id, x, y))
        })
        .collect()
}

/// How far to blow up a clip for it to cover an image, around a point `margin` inside it, with its furthest point `reach` away and the image's corners `corner` from its middle, all on the page. `pixels` is how many of the bitmap's pixels each of those units makes. Three times what straight edges need, since a curve's points only roughly say where it runs. `None` if that would go past what PDFium fills without bending.
fn clip_blow_up(margin: f64, reach: f64, corner: f64, pixels: f64) -> Option<f64> {
    if margin <= 0.0 || !pixels.is_finite() {
        return None;
    }
    let scale = (corner * 3.0 / margin).max(1.0);
    (scale.is_finite() && reach * scale * pixels <= MAX_CLIP_REACH).then_some(scale)
}

/// The area of some points on the page.
fn points_area(points: &[(u8, f32, f32)], matrix: &M) -> Area {
    let mut area = Area { left: f64::INFINITY, bottom: f64::INFINITY, right: f64::NEG_INFINITY, top: f64::NEG_INFINITY };
    for &(_, x, y) in points {
        let (x, y) = apply(matrix, f64::from(x), f64::from(y));
        area.left = area.left.min(x);
        area.right = area.right.max(x);
        area.bottom = area.bottom.min(y);
        area.top = area.top.max(y);
    }
    area
}

/// The rectangle these points make on the page, if they're just that, upright.
fn upright_rectangle(points: &[(u8, f32, f32)], matrix: &M) -> Option<Area> {
    if !(4..=5).contains(&points.len()) || points.iter().skip(1).any(|(kind, _, _)| *kind != SEG_LINE) {
        return None;
    }
    let area = points_area(points, matrix);
    let on_edge = |value: f64, low: f64, high: f64| (value - low).abs() < 1e-3 || (value - high).abs() < 1e-3;
    let mut corners = [false; 4];
    for &(_, x, y) in points {
        let (x, y) = apply(matrix, f64::from(x), f64::from(y));
        if !on_edge(x, area.left, area.right) || !on_edge(y, area.bottom, area.top) {
            return None;
        }
        corners[usize::from((x - area.left).abs() < 1e-3) * 2 + usize::from((y - area.bottom).abs() < 1e-3)] = true;
    }
    corners.iter().all(|corner| *corner).then_some(area)
}

/// Whether a path is one shape that doesn't cross itself, so that a fill rule can't change what it covers. Going by its corners, which for a curve can read as crossing when it doesn't: that only makes a picture. A subpath of one point covers nothing. Long ones aren't looked through, and count as not.
fn is_one_plain_shape(points: &[(u8, f32, f32)]) -> bool {
    const MAX_LOOKED_THROUGH: usize = 256;
    let mut starts = points.iter().enumerate().filter(|(_, (kind, _, _))| *kind == SEG_MOVE).map(|(at, _)| at);
    let start = starts.next().unwrap_or(0);
    let mut end = points.len();
    if let Some(next) = starts.next() {
        // A shape followed only by lone moves is still one shape.
        if !(next..points.len()).all(|at| points[at].0 == SEG_MOVE) {
            return false;
        }
        end = next;
    }
    let corners = &points[start..end];
    if corners.len() < 4 {
        return true;
    }
    if corners.len() > MAX_LOOKED_THROUGH {
        return false;
    }
    let side = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| f64::from(b.0 - a.0) * f64::from(c.1 - a.1) - f64::from(b.1 - a.1) * f64::from(c.0 - a.0);
    let at = |index: usize| {
        let (_, x, y) = corners[index % corners.len()];
        (x, y)
    };
    for first in 0..corners.len() {
        for second in first + 2..corners.len() {
            // Next to each other all the way round, so they meet at a corner and don't cross.
            if first == 0 && second == corners.len() - 1 {
                continue;
            }
            let (a, b, c, d) = (at(first), at(first + 1), at(second), at(second + 1));
            let crosses = side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0;
            if crosses {
                return false;
            }
        }
    }
    true
}

/// Whether a point is inside a closed outline, by the even-odd rule.
fn inside_outline(outline: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut previous = outline[outline.len() - 1];
    for &point in outline {
        if (point.1 > y) != (previous.1 > y) && x < previous.0 + (y - previous.1) / (point.1 - previous.1) * (point.0 - previous.0) {
            inside = !inside;
        }
        previous = point;
    }
    inside
}

/// How far a point is from a closed outline's edges.
fn distance_to_outline(outline: &[(f64, f64)], x: f64, y: f64) -> f64 {
    let mut nearest = f64::INFINITY;
    let mut previous = outline[outline.len() - 1];
    for &point in outline {
        let (dx, dy) = (point.0 - previous.0, point.1 - previous.1);
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 { (((x - previous.0) * dx + (y - previous.1) * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
        nearest = nearest.min((x - previous.0 - t * dx).hypot(y - previous.1 - t * dy));
        previous = point;
    }
    nearest
}

/// The single character a ligature's parts come from. `None` if it isn't one of the usual ones.
fn ligature_of(parts: &[u32]) -> Option<u32> {
    let f = u32::from('f');
    let i = u32::from('i');
    let l = u32::from('l');
    match parts {
        [a, b] if *a == f && *b == f => Some(0xFB00),
        [a, b] if *a == f && *b == i => Some(0xFB01),
        [a, b] if *a == f && *b == l => Some(0xFB02),
        [a, b, c] if *a == f && *b == f && *c == i => Some(0xFB03),
        [a, b, c] if *a == f && *b == f && *c == l => Some(0xFB04),
        _ => None,
    }
}

/// The part of an image inside `view`, in its pixels from the top left, with the matrix that places just that part. Only an upright image can be cut: a turned one is whole.
fn crop_to(width: u32, height: u32, matrix: &M, view: Area) -> ((u32, u32, u32, u32), M) {
    let whole = ((0, 0, width, height), *matrix);
    if matrix[1].abs() > 1e-6 || matrix[2].abs() > 1e-6 || matrix[0] == 0.0 || matrix[3] == 0.0 {
        return whole;
    }
    let to_x = |x: f64| (x - matrix[4]) / matrix[0];
    let to_y = |y: f64| (y - matrix[5]) / matrix[3];
    let (u0, u1) = (to_x(view.left).min(to_x(view.right)).clamp(0.0, 1.0), to_x(view.left).max(to_x(view.right)).clamp(0.0, 1.0));
    let (v0, v1) = (to_y(view.bottom).min(to_y(view.top)).clamp(0.0, 1.0), to_y(view.bottom).max(to_y(view.top)).clamp(0.0, 1.0));
    // At least one pixel, whichever edge of the image the box only touches.
    let left = ((u0 * f64::from(width)).floor() as u32).min(width - 1);
    let right = ((u1 * f64::from(width)).ceil() as u32).clamp(left + 1, width);
    // A row counts from y = 1, the image's top.
    let top = (((1.0 - v1) * f64::from(height)).floor() as u32).min(height - 1);
    let bottom = (((1.0 - v0) * f64::from(height)).ceil() as u32).clamp(top + 1, height);
    let (cut_w, cut_h) = (right - left, bottom - top);
    if cut_w == width && cut_h == height {
        return whole;
    }
    let (u_low, u_step) = (f64::from(left) / f64::from(width), f64::from(cut_w) / f64::from(width));
    let v_low = 1.0 - f64::from(bottom) / f64::from(height);
    let v_step = f64::from(cut_h) / f64::from(height);
    let placed = [matrix[0] * u_step, 0.0, 0.0, matrix[3] * v_step, matrix[4] + matrix[0] * u_low, matrix[5] + matrix[3] * v_low];
    ((left, top, cut_w, cut_h), placed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: an image cut to a box stands somewhere else than the part it shows, so a snapshot's picture slides or stretches.
    #[test]
    fn a_cut_image_stands_where_that_part_was() {
        // 100 by 200 pixels, over 10 by 20 points at (5, 7): a pixel is a tenth of a point.
        let matrix = [10.0, 0.0, 0.0, 20.0, 5.0, 7.0];
        let view = Area { left: 7.0, bottom: 10.0, right: 9.0, top: 20.0 };
        let ((x, y, w, h), placed) = crop_to(100, 200, &matrix, view);
        assert_eq!((x, w), (20, 20));
        assert_eq!((y, h), (70, 100));
        assert!((placed[0] - 2.0).abs() < 1e-9 && (placed[4] - 7.0).abs() < 1e-9, "{placed:?}");
        assert!((placed[3] - 10.0).abs() < 1e-9 && (placed[5] - 10.0).abs() < 1e-9, "{placed:?}");
        // All of it, or a turned one: nothing's cut.
        assert_eq!(crop_to(100, 200, &matrix, Area { left: 0.0, bottom: 0.0, right: 100.0, top: 100.0 }).0, (0, 0, 100, 200));
        assert_eq!(crop_to(100, 200, &[0.0, 10.0, -20.0, 0.0, 5.0, 7.0], view).0, (0, 0, 100, 200));
    }

    /// Failure: a box that only touches an image's edge starts its cut one past the last pixel, and the worker panics on an ordinary request. On every edge it must come out as something that can be drawn.
    #[test]
    fn a_box_touching_any_edge_of_an_image_cuts_something_real() {
        let matrix = [10.0, 0.0, 0.0, 20.0, 5.0, 7.0];
        for view in [
            Area { left: 15.0, bottom: 10.0, right: 20.0, top: 20.0 },
            Area { left: 0.0, bottom: 10.0, right: 5.0, top: 20.0 },
            Area { left: 7.0, bottom: 27.0, right: 9.0, top: 30.0 },
            Area { left: 7.0, bottom: 0.0, right: 9.0, top: 7.0 },
            Area { left: 15.0, bottom: 27.0, right: 20.0, top: 30.0 },
        ] {
            let ((x, y, w, h), _) = crop_to(100, 200, &matrix, view);
            assert!(w >= 1 && h >= 1 && x + w <= 100 && y + h <= 200, "{view:?}: {x} {y} {w} {h}");
        }
    }

    /// Failure: a rotated page's objects land off the document, because the matrix doesn't agree with how PDFium displays that rotation.
    #[test]
    fn each_rotation_takes_the_page_box_onto_the_document() {
        let bbox = (10.0, 20.0, 110.0, 220.0);
        for (rotation, page_w, page_h, corner) in [(0, 100.0, 200.0, (10.0, 220.0)), (1, 200.0, 100.0, (10.0, 20.0)), (2, 100.0, 200.0, (110.0, 20.0)), (3, 200.0, 100.0, (110.0, 220.0))] {
            let m = display_matrix(bbox, rotation, page_w, page_h).unwrap();
            let top_left = apply(&m, corner.0, corner.1);
            assert!(top_left.0.abs() < 1e-9 && top_left.1.abs() < 1e-9, "rotation {rotation}: {top_left:?}");
            let area = Area { left: 10.0, bottom: 20.0, right: 110.0, top: 220.0 }.through(&m);
            assert!(area.left.abs() < 1e-9 && (area.right - page_w).abs() < 1e-9 && area.bottom.abs() < 1e-9 && (area.top - page_h).abs() < 1e-9, "rotation {rotation}: {area:?}");
        }
        assert!(display_matrix((0.0, 0.0, 0.0, 10.0), 0, 1.0, 1.0).is_none(), "a box with no area");
    }

    /// Failure: a clip that a fill rule would change passes as plain, so its hole fills in; or an ordinary box or blob doesn't, and everything it clips turns into a picture for nothing.
    #[test]
    fn only_a_plain_shape_passes_as_plain() {
        let shape = |points: &[(u8, f32, f32)]| is_one_plain_shape(points);
        let square = [(SEG_MOVE, 0.0, 0.0), (SEG_LINE, 10.0, 0.0), (SEG_LINE, 10.0, 10.0), (SEG_LINE, 0.0, 10.0), (SEG_LINE, 0.0, 0.0)];
        assert!(shape(&square));
        assert!(shape(&[]), "an empty clip covers nothing, by either rule");
        let mut trailing = square.to_vec();
        trailing.push((SEG_MOVE, 3.0, 3.0));
        assert!(shape(&trailing), "a lone move covers nothing");
        let mut hole = square.to_vec();
        hole.extend([(SEG_MOVE, 2.0, 2.0), (SEG_LINE, 8.0, 2.0), (SEG_LINE, 8.0, 8.0)]);
        assert!(!shape(&hole), "a second shape");
        let star = [(SEG_MOVE, 0.0, 0.0), (SEG_LINE, 10.0, 10.0), (SEG_LINE, 10.0, 0.0), (SEG_LINE, 0.0, 10.0), (SEG_LINE, 0.0, 0.0)];
        assert!(!shape(&star), "a shape crossing itself");
    }

    /// Failure: past a limit a glyph gets no id and drops out of its run, leaving a gap in a line that's reported as vector text. The whole object has to go another way, and the ids have to be the ones given, not the codes.
    #[test]
    fn a_glyph_with_no_id_condemns_its_whole_run() {
        let plan = [(65, 1.0, 2.0), (66, 3.0, 4.0)];
        assert_eq!(with_ids(&plan, |code| code + 100), Some(vec![(165, 1.0, 2.0), (166, 3.0, 4.0)]));
        assert_eq!(with_ids(&plan, |code| if code == 66 { 0 } else { 7 }), None);
    }

    /// Failure: a clip is blown up so far that PDFium's filler bends it, so it cuts into the image it was meant to clear, and a mask has the clip's shape in it. It goes only as far as it must, and one that would have to go too far isn't tried.
    #[test]
    fn a_clip_is_blown_up_only_as_far_as_it_must_be() {
        let scale = clip_blow_up(70.0, 300.0, 144.0, 1.0).unwrap();
        assert!(70.0 * scale >= 144.0, "it covers the image from its margin");
        assert!(300.0 * scale < MAX_CLIP_REACH, "and stays where it isn't bent");
        assert_eq!(clip_blow_up(500.0, 600.0, 10.0, 1.0), Some(1.0), "a clip already past the image stays as it is");
        assert!(clip_blow_up(0.001, 800.0, 5000.0, 1.0).is_none(), "a sliver over a large image can't");
        assert!(clip_blow_up(70.0, 300.0, 144.0, 1000.0).is_none(), "nor one that a fine image's pixels would take too far");
        assert!(clip_blow_up(0.0, 800.0, 10.0, 1.0).is_none());
        assert!(clip_blow_up(70.0, 300.0, 144.0, f64::INFINITY).is_none(), "an image with no size");
    }

    /// Failure: anything with a space's code and no outline is taken for a space, so TeX's big brackets vanish from every equation; or the reverse, every real space becomes a picture of nothing. The sizes are ones from real pages.
    #[test]
    fn only_a_real_space_marks_nothing() {
        let em = 10.0;
        let upright = |width, height, em| marks_the_page(width, height, em, (1.0, 0.0), true, None);
        assert!(upright(5.2, 29.9, em), "a big bracket");
        assert!(upright(0.5, 7.0, em), "a bracket's thin bar");
        assert!(!upright(2.5, 0.01, em), "a space");
        assert!(!upright(0.01, 2.5, em), "a space on turned text");
        assert!(!upright(30.0, 0.0, 120.0), "a space in big type");
        // Text at an angle, whose box holds a space's line corner to corner.
        let side = 2.5 * std::f64::consts::FRAC_1_SQRT_2;
        assert!(!marks_the_page(side, side, em, (1.0, 1.0), false, Some(2.5)), "a space at 45 degrees");
        let (long, short) = (2.5 * 0.866, 2.5 * 0.5);
        assert!(!marks_the_page(long, short, em, (0.866, 0.5), false, Some(2.5)), "a space at 30 degrees");
        let fat = (5.2 + 29.9) * std::f64::consts::FRAC_1_SQRT_2;
        assert!(marks_the_page(fat, fat, em, (1.0, 1.0), false, Some(7.9)), "a big bracket at 45 degrees");
        assert!(marks_the_page(side, side, em, (1.0, 1.0), false, None), "and with no width to go by it isn't taken for a space");
    }

    /// Failure: a wide-gamut photo reads as sRGB and passes through, so its colours come out wrong, or a real sRGB profile doesn't and every such photo loses its original bytes. Hostile lengths in a profile mustn't panic.
    #[test]
    fn a_profile_says_srgb_only_in_its_description() {
        let profile = |text: &[u8]| {
            let mut out = vec![0u8; 128];
            out.extend_from_slice(&1u32.to_be_bytes());
            out.extend_from_slice(b"desc");
            out.extend_from_slice(&144u32.to_be_bytes());
            out.extend_from_slice(&(text.len() as u32).to_be_bytes());
            out.extend_from_slice(text);
            out
        };
        assert!(profile_says_srgb(&profile(b"descxxxxsRGB IEC61966-2.1")));
        assert!(profile_says_srgb(&profile(b"mluc\0s\0R\0G\0B\0 \0I")));
        assert!(!profile_says_srgb(&profile(b"descxxxxAdobe RGB (1998)")));
        assert!(!profile_says_srgb(b"sRGB but not a profile"));
        assert!(!profile_says_srgb(&[]));
        let mut hostile = vec![0u8; 128];
        hostile.extend_from_slice(&u32::MAX.to_be_bytes());
        hostile.extend_from_slice(b"desc");
        hostile.extend_from_slice(&u32::MAX.to_be_bytes());
        hostile.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(!profile_says_srgb(&hostile));
    }

    /// Failure: the matrix maths is wrong in some rare composition, and nested forms land in the wrong place.
    #[test]
    fn matrices_compose_in_page_order_and_invert() {
        let (a, b) = ([2.0, 0.0, 0.0, 3.0, 5.0, 7.0], [0.0, 1.0, -1.0, 0.0, 11.0, 13.0]);
        let both = then(&a, &b);
        let through = apply(&b, apply(&a, 4.0, 6.0).0, apply(&a, 4.0, 6.0).1);
        let direct = apply(&both, 4.0, 6.0);
        assert!((through.0 - direct.0).abs() < 1e-9 && (through.1 - direct.1).abs() < 1e-9);
        let back = inverse(&both).unwrap();
        let round = apply(&back, direct.0, direct.1);
        assert!((round.0 - 4.0).abs() < 1e-9 && (round.1 - 6.0).abs() < 1e-9);
        assert!(inverse(&[1.0, 2.0, 2.0, 4.0, 0.0, 0.0]).is_none());
    }

    /// Failure: a ligature's parts draw as the wrong character or are dropped, so `fi` reads badly or goes missing.
    #[test]
    fn ligature_parts_lead_to_the_ligature() {
        let code = |text: &str| text.chars().map(u32::from).collect::<Vec<_>>();
        assert_eq!(ligature_of(&code("fi")), Some(0xFB01));
        assert_eq!(ligature_of(&code("ffl")), Some(0xFB04));
        assert_eq!(ligature_of(&code("ab")), None);
    }
}
