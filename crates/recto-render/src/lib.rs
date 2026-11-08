//! Pages into pixels and characters, behind one seam. Everything here runs inside a worker process that a hostile file may take over
//! or kill, so nothing here decides policy: limits, retries and validation live with the host in `recto-render-service`.
//!
//! The backend writes into a caller's buffer and never allocates a surface.

mod classify;
mod ink;
mod pdfium;

pub use classify::classify_bgrx;
pub use ink::{INK_PADDING_PX, INK_RANGE, SAME_AS_GROUND, SEEN_PIXELS, adds_nothing_seen, box_has_ink, glyph_is_ground_coloured};
pub use pdfium::{LibraryError, Pdfium, PdfiumBackend};
pub use recto_proto::{CharBox, CharRecord, FailCode, PageFrame, TileKind, TileRect};

/// Asks whether the job this check belongs to has gone stale. Called between PDF objects, so it must be cheap: one atomic load.
pub trait Cancel {
    fn is_cancelled(&self) -> bool;
}

/// A check that never cancels.
pub struct NeverCancel;

impl Cancel for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl<F: Fn() -> bool> Cancel for F {
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// How a drawn tile ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileOutcome {
    Done(TileKind),
    Cancelled,
}

/// Why the backend couldn't answer. These cross the wire as they are.
pub type RenderError = FailCode;

/// A page's size in points, after its rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PagePoints {
    pub width: f32,
    pub height: f32,
}

/// What [`RenderBackend::page_text`] should do beyond reading characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextOptions {
    pub boxes: bool,
    pub hidden_check: bool,
}

/// One page's characters. `boxes` is empty, or holds one box per character.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageChars {
    pub chars: Vec<CharRecord>,
    pub boxes: Vec<CharBox>,
    pub hidden_checked: bool,
    /// The page as it's shown, if boxes were asked for.
    pub frame: PageFrame,
    pub copy_restricted: bool,
}

/// What draws pages. PDFium today; a second engine plugs in here. One backend holds one document.
pub trait RenderBackend {
    fn page_count(&self) -> u32;

    fn page_size(&mut self, page: u32) -> Result<PagePoints, RenderError>;

    /// Draws `rect` of `page` at `scale` into `out`, which holds rows `stride` bytes apart. `rect` is in device pixels at that scale,
    /// from the page's top left. Reports what the tile holds, so the host can skip a surface for a blank one.
    fn render_tile(&mut self, page: u32, rect: TileRect, scale: f32, out: &mut [u8], stride: u32, cancel: &dyn Cancel) -> Result<TileOutcome, RenderError>;

    /// Draws a region of a page in one pass, for when several tiles are needed: most of a heavy page's cost is per pass, not per pixel. `rect` lies on a grid of `tile_edge`
    /// from the page's origin. On finishing, `kinds` holds what each tile holds, in [`recto_proto::region_tiles`] order.
    #[allow(clippy::too_many_arguments)]
    fn render_region(
        &mut self,
        page: u32,
        rect: TileRect,
        scale: f32,
        tile_edge: u32,
        out: &mut [u8],
        stride: u32,
        cancel: &dyn Cancel,
        kinds: &mut Vec<TileKind>,
    ) -> Result<TileOutcome, RenderError>;

    /// The largest image on a page, in source pixels. Bounded: it looks at no more than a fixed number of objects. Loads the page, which drawing would do anyway.
    fn largest_image_pixels(&mut self, page: u32) -> Result<u64, RenderError>;

    /// Where a page's images are, as parts of the page in 65535ths, `[left, top, right, bottom]`. Bounded like [`Self::largest_image_pixels`], and at most [`recto_proto::MAX_IMAGE_BOXES`], past which they're given as one.
    fn image_boxes(&mut self, page: u32) -> Result<Vec<[u16; 4]>, RenderError>;

    /// Reads a page's characters, in the order the backend understands them.
    fn page_text(&mut self, page: u32, options: TextOptions) -> Result<PageChars, RenderError> {
        self.page_text_until(page, options, &NeverCancel)
    }

    /// As [`Self::page_text`], with the draw for the hidden-text check stopping if `cancel` says to. The text still comes back, as not checked.
    fn page_text_until(&mut self, page: u32, options: TextOptions, cancel: &dyn Cancel) -> Result<PageChars, RenderError>;

    /// The next piece of a page's contents, as a whole frame in `out`, for the host to write out as SVG. `job.seq` 0 starts again from the top. Bounded: at most a frame's worth of memory beyond what the page and its current image hold.
    fn contents_frame(&mut self, job: &recto_proto::contents::ContentsJob, out: &mut Vec<u8>, cancel: &dyn Cancel) -> Result<(), RenderError>;
}
