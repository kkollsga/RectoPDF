//! Pages into pixels and characters, behind one seam. Everything here runs inside a worker process that a hostile file may take over
//! or kill, so nothing here decides policy: limits, retries and validation live with the host in `recto-render-service`.
//!
//! The backend writes into a caller's buffer and never allocates a surface.

mod classify;
mod ink;
mod pdfium;

pub use classify::classify_bgrx;
pub use ink::{INK_PADDING_PX, INK_RANGE, box_has_ink};
pub use pdfium::{LibraryError, Pdfium, PdfiumBackend};
pub use recto_proto::{CharBox, CharRecord, FailCode, TileKind, TileRect};

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
}

/// What draws pages. PDFium today; a second engine plugs in here. One backend holds one document.
pub trait RenderBackend {
    fn page_count(&self) -> u32;

    fn page_size(&mut self, page: u32) -> Result<PagePoints, RenderError>;

    /// Draws `rect` of `page` at `scale` into `out`, which holds rows `stride` bytes apart. `rect` is in device pixels at that scale,
    /// from the page's top left. Reports what the tile holds, so the host can skip a surface for a blank one.
    fn render_tile(
        &mut self,
        page: u32,
        rect: TileRect,
        scale: f32,
        out: &mut [u8],
        stride: u32,
        cancel: &dyn Cancel,
    ) -> Result<TileOutcome, RenderError>;

    /// Reads a page's characters, in the order the backend understands them.
    fn page_text(&mut self, page: u32, options: TextOptions) -> Result<PageChars, RenderError>;
}
