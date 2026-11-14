//! **A page's look on dark paper, as the thread that draws decides it** ([`page_look`]): once a page while its renderer lives, from a picture of the whole page that's at hand or a small one drawn for it, and kept in [`Looks`], so each of the page's tiles, its preview and the picture it sleeps as go by the same look. What decides from the picture is [`PageLook::decide`]; this is where the picture comes from, and which answers are kept.
//!
//! What it asks of a renderer is [`LookSource`], which [`DocRenderer`] is, and a test's stand-in can be.

use crate::paper::PageLook;
use crate::tiles::{Outcome, TRY_AGAIN_AFTER};
use recto_platform::surface::PixelSurface;
use recto_proto::TileRect;
use recto_render_service::{DocRenderer, RenderFailure, TileRequest, TileResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Pages whose look is remembered. Many more than the tiles and previews kept, so a page can't have tiles on the screen from before it was forgotten. Each is at most 64 boxes.
const MAX_LOOKS: usize = 4096;
/// The longest side of the picture of a page that's drawn to judge it, if there's no preview to use. Enough to see paper between lines of text, and a mebibyte of memory for a moment.
const LOOK_PICTURE_EDGE: f32 = 512.0;
/// The same, for a page with no images, when nothing drawn so far covers the page (a wake onto a sleep picture, which stands for the page's preview; zoomed out past where a preview is drawn, with the page in more than one tile; the looks forgotten while the preview is kept; a preview drawn only at a lower resolution, which isn't taken for a picture of the page): all that's asked of it is whether its background is dark, which a picture this small answers as the look picture does (the survey's `sizes`, 0 pages apart at 128), for a fraction of the draw.
const IMAGELESS_LOOK_EDGE: f32 = 128.0;
/// **How many times a page's picture may fail for a reason that may pass before the page takes [`PageLook::undecided`] and keeps it** (review of 2026-11-13, S2), the tries spaced by the page view's own wait for a page that couldn't be drawn for a reason that may pass, `TRY_AGAIN_AFTER`, 3 s (second review, 2026-11-14: counted per job, three back-to-back tile draws used them up in milliseconds, under just the memory pressure that makes a surface fail, and a dark slide turned light for the renderer's life). After a try fails the job ends as [`Outcome::LookLater`]: its tile isn't shown, nothing's kept, and the page view asks nothing of the page till the wait is over, so no tile's drawn to be thrown away meanwhile; what's shown of the page stays, the sleep picture or the preview, with no note, as nothing's failed. A try asked before its time, as by a second tab on the same file, isn't one: nothing's drawn for it and the job ends the same way. Three tries over at least 6 s: a heavy page whose larger limit is busy for a moment, or a Mac short of memory for a few seconds, gets it, and a page that can never be drawn small still draws, after three tiles' worth of work.
const LOOK_TRIES: u8 = 3;

/// What the look is decided with: a file's renderer, or a test's stand-in for one.
pub(super) trait LookSource {
    /// The places its images were decided from have been forgotten since this was last asked, so no look decided from them may stand.
    fn take_image_places_cleared(&mut self) -> bool;
    fn image_boxes(&mut self, page: u32, generation: u64) -> Result<Option<Vec<[u16; 4]>>, RenderFailure>;
    fn is_stale(&self, generation: u64) -> bool;
    fn page_size(&mut self, page: u32) -> Result<(f32, f32), RenderFailure>;
    fn render_tile(&mut self, request: &TileRequest, surface: &PixelSurface) -> Result<TileResult, RenderFailure>;
}

impl LookSource for DocRenderer {
    fn take_image_places_cleared(&mut self) -> bool {
        DocRenderer::take_image_places_cleared(self)
    }

    fn image_boxes(&mut self, page: u32, generation: u64) -> Result<Option<Vec<[u16; 4]>>, RenderFailure> {
        DocRenderer::image_boxes(self, page, generation)
    }

    fn is_stale(&self, generation: u64) -> bool {
        DocRenderer::is_stale(self, generation)
    }

    fn page_size(&mut self, page: u32) -> Result<(f32, f32), RenderFailure> {
        DocRenderer::page_size(self, page)
    }

    fn render_tile(&mut self, request: &TileRequest, surface: &PixelSurface) -> Result<TileResult, RenderFailure> {
        DocRenderer::render_tile(self, request, surface)
    }
}

/// What's been decided about a file's pages on dark paper, and the pages whose picture has failed for a reason that may pass, how many times and when it may be tried next. Of the file that's open now: a replacement clears it before anything goes by it.
#[derive(Debug, Default)]
pub(super) struct Looks {
    decided: HashMap<u32, Arc<PageLook>>,
    tries: HashMap<u32, Tries>,
}

/// A page's tries at a picture that failed for a reason that may pass.
#[derive(Clone, Copy, Debug)]
struct Tries {
    made: u8,
    /// Not tried again before this.
    next: Instant,
}

impl Looks {
    pub(super) fn clear(&mut self) {
        self.decided.clear();
        self.tries.clear();
    }

    /// Keeps a page's look, at most [`MAX_LOOKS`] of them.
    fn keep(&mut self, page: u32, look: PageLook) -> Arc<PageLook> {
        if self.decided.len() >= MAX_LOOKS {
            self.decided.clear();
        }
        self.tries.remove(&page);
        let look = Arc::new(look);
        self.decided.insert(page, Arc::clone(&look));
        look
    }

    /// Whether a page's next try is still to wait at `now`.
    fn waiting(&self, page: u32, now: Instant) -> bool {
        self.tries.get(&page).is_some_and(|tries| now < tries.next)
    }

    /// A failure of a page's picture that may pass, at `now`: whether it may be tried again, rather than the page taking what a page with no picture takes. It counts as a try only once the last one's wait is over.
    fn try_again(&mut self, page: u32, now: Instant) -> bool {
        if self.tries.len() >= MAX_LOOKS && !self.tries.contains_key(&page) {
            self.tries.clear();
        }
        let tries = self.tries.entry(page).or_insert(Tries { made: 0, next: now });
        if now >= tries.next {
            tries.made = tries.made.saturating_add(1);
            tries.next = now + TRY_AGAIN_AFTER;
        }
        tries.made < LOOK_TRIES
    }
}

/// Why there's no look for a page from this job, and nothing's kept: neither is about the page.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum NoLook {
    /// The job went stale on the way.
    Stale,
    /// What it's decided from wasn't asked, as the job may use only the worker that's there ([`recto_render_service::Workers::OnlyTheOneThere`]) and that one couldn't, a heavy one if `heavy`.
    Unstarted { heavy: bool },
    /// The picture it's decided from failed for a reason that may pass ([`RenderFailure::NotDrawableNow`], or a surface that couldn't be made or read), or the page is waiting to be tried again: it's tried again once its wait is over, at most [`LOOK_TRIES`] times.
    Again,
}

/// What a picture of the page came to.
enum Picture {
    Decided(PageLook),
    /// It failed in a way that may pass: see [`NoLook::Again`].
    Passing,
    /// It can't be had, and won't be for this renderer: the page takes [`PageLook::undecided`], as it always has.
    Final,
}

/// **What a tile drawn for dark paper does with its page's look** ([`page_look`]): goes by it, or ends as the outcome given, its tile not handed on and nothing kept: [`Outcome::Cancelled`] for a job gone stale, [`Outcome::LookLater`] for a look to be tried again, which holds the page till then, and [`Outcome::Unstarted`] for want of a worker.
pub(super) fn tile_look(
    doc: &mut impl LookSource,
    looks: &mut Looks,
    page: u32,
    generation: u64,
    pictures: (Option<(&PixelSurface, TileRect)>, Option<(&PixelSurface, TileRect, f32)>),
    now: Instant,
) -> Result<Arc<PageLook>, Outcome> {
    page_look(doc, looks, page, generation, pictures.0, pictures.1, now).map_err(|no| match no {
        NoLook::Stale => Outcome::Cancelled,
        NoLook::Again => Outcome::LookLater,
        NoLook::Unstarted { heavy } => Outcome::Unstarted { heavy },
    })
}

/// The look of a page: known, or decided now from what's on hand, a preview if this job's drawn one, otherwise a small picture drawn for it. A page with no images is decided too, as its background may be dark (owner, 2026-11-13: a page already dark is never turned over), from the preview or, if this job's tile covers the whole page, from that, and only with neither from a picture [`IMAGELESS_LOOK_EDGE`] long. Only a look decided from answers is kept: one whose image boxes, size or picture were refused for want of a worker isn't decided at all, as `None` for its images would judge the page whole, which a page with a photograph isn't; one whose picture failed for a reason that may pass is tried again once its wait is over, `now` being when this is asked ([`NoLook::Again`], [`LOOK_TRIES`]); a picture that can't be had at all gives [`PageLook::undecided`], kept.
pub(super) fn page_look(
    doc: &mut impl LookSource,
    looks: &mut Looks,
    page: u32,
    generation: u64,
    whole: Option<(&PixelSurface, TileRect)>,
    tile: Option<(&PixelSurface, TileRect, f32)>,
    now: Instant,
) -> Result<Arc<PageLook>, NoLook> {
    // The places of its images are what a look was decided from, and they may not come out the same twice, so neither of a page's tiles may go by an old answer.
    if doc.take_image_places_cleared() {
        looks.clear();
    }
    if let Some(known) = looks.decided.get(&page) {
        return Ok(Arc::clone(known));
    }
    let boxes = doc.image_boxes(page, generation).map_err(unstarted)?;
    if doc.is_stale(generation) {
        return Err(NoLook::Stale);
    }
    let image_less = boxes.as_ref().is_some_and(Vec::is_empty);
    let decide = |surface: &PixelSurface, rect: TileRect| {
        let stride = surface.stride() as usize;
        surface
            .with_pixels(|pixels| PageLook::decide(boxes.as_deref(), pixels, stride, rect.width, rect.height))
            .map_or(Picture::Passing, Picture::Decided)
    };
    // A tile that is the whole page, as one zoomed out past a preview is, is as good a picture of a page with no images. Its size is asked only when it would decide.
    let at_hand = whole.or_else(|| {
        tile.filter(|&(_, rect, scale)| image_less && covers_page(doc, page, scale, rect))
            .map(|(surface, rect, _)| (surface, rect))
    });
    let picture = match at_hand {
        Some((surface, rect)) => decide(surface, rect),
        // Nothing's drawn for a page still waiting to be tried again.
        None if looks.waiting(page, now) => return Err(NoLook::Again),
        None => match small_picture(doc, page, generation, image_less)? {
            Ok((surface, drawn)) => decide(&surface, drawn),
            Err(failed) => failed,
        },
    };
    let look = match picture {
        Picture::Decided(look) => look,
        Picture::Passing if looks.try_again(page, now) => return Err(NoLook::Again),
        Picture::Passing | Picture::Final => PageLook::undecided(image_less),
    };
    Ok(looks.keep(page, look))
}

/// A small picture of the whole page, drawn to decide its look, and the part of it that was drawn: [`LOOK_PICTURE_EDGE`] long, or [`IMAGELESS_LOOK_EDGE`] for a page with no images. Or why there's none, as what it gives the page.
fn small_picture(doc: &mut impl LookSource, page: u32, generation: u64, image_less: bool) -> Result<Result<(PixelSurface, TileRect), Picture>, NoLook> {
    let (width, height) = match doc.page_size(page) {
        Ok(size) => size,
        Err(failure @ RenderFailure::WouldStart { .. }) => return Err(unstarted(failure)),
        Err(_) => return Ok(Err(Picture::Final)),
    };
    let edge = if image_less { IMAGELESS_LOOK_EDGE } else { LOOK_PICTURE_EDGE };
    let scale = (edge / width.max(height).max(1.0)).clamp(recto_proto::MIN_SCALE, recto_proto::MAX_SCALE);
    let Ok((pixel_width, pixel_height)) = recto_proto::device_size(width, height, scale) else {
        return Ok(Err(Picture::Final));
    };
    // A surface that can't be made now, for want of memory or handles, may be made at the next job.
    let Ok(surface) = PixelSurface::create(pixel_width, pixel_height) else {
        return Ok(Err(Picture::Passing));
    };
    let rect = TileRect {
        x: 0,
        y: 0,
        width: pixel_width,
        height: pixel_height,
    };
    let request = TileRequest {
        page,
        rect,
        scale,
        stride: surface.stride(),
        generation,
    };
    match doc.render_tile(&request, &surface) {
        Ok(TileResult::Done(tile)) => {
            let drawn = tile.low_res.map_or(rect, |low| low.rect);
            Ok(Ok((surface, drawn)))
        }
        Ok(TileResult::Cancelled { .. }) => Err(NoLook::Stale),
        Err(failure @ RenderFailure::WouldStart { .. }) => Err(unstarted(failure)),
        // The one failure of a draw the tile path treats as one that may pass.
        Err(RenderFailure::NotDrawableNow { .. }) => Ok(Err(Picture::Passing)),
        Err(_) => Ok(Err(Picture::Final)),
    }
}

/// A refusal for want of a worker, as [`page_look`] says it. Only [`RenderFailure::WouldStart`] comes here.
fn unstarted(failure: RenderFailure) -> NoLook {
    NoLook::Unstarted {
        heavy: matches!(failure, RenderFailure::WouldStart { heavy: true }),
    }
}

/// Whether `rect`, drawn at `scale`, is the whole of a page: from its top left corner, and the page's size at that scale. A size that can't be had now is no.
fn covers_page(doc: &mut impl LookSource, page: u32, scale: f32, rect: TileRect) -> bool {
    rect.x == 0
        && rect.y == 0
        && doc
            .page_size(page)
            .ok()
            .and_then(|(width, height)| recto_proto::device_size(width, height, scale).ok())
            == Some((rect.width, rect.height))
}

#[cfg(test)]
mod tests;
