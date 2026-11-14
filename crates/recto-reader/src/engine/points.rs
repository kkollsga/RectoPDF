//! **Points and pages of the tab showing, from the engine's callers:** where a point of the document is on a page and back, how many pages there are and how each is shown. The snapshot tool, the print panel and QA's clicks take it alone. Short locks and some sums.
//!
//! A borrow of the [`Engine`], built inline by [`Engine::points`]: each method takes the page view's lock exactly as the `Engine` method it was until R3.6
//! did, in the same order with the same locks after it, so the engine's lock map holds as it was.

use super::{Engine, lock};

/// Points and pages of the tab this page view shows.
#[derive(Clone, Copy)]
pub struct Points<'a> {
    engine: &'a Engine,
}

// size: one reference, so taking it costs what calling a method of `Engine` did.
const _: () = assert!(size_of::<Points<'static>>() <= size_of::<usize>(), "a facade of the engine grew");

impl Engine {
    /// Points and pages of the tab this page view shows.
    #[inline]
    pub fn points(&self) -> Points<'_> {
        Points { engine: self }
    }
}

impl Points<'_> {
    /// Where a point in the document is on a page as it's shown, with that page's size, for the snapshot tool. A short lock and some sums.
    pub fn on_page(&self, x: f64, y: f64) -> Option<(recto_index::select::PagePoint, (f32, f32))> {
        let reader = lock(&self.engine.shared.reader);
        let doc = reader.docs.shown()?;
        let point = doc.point_on_page(x, y)?;
        Some((point, doc.page_size_shown(point.page)?))
    }

    /// Where a point in the document is on `page`, whichever page it's over, past its edges if it must: a snapshot's box stays on the page it was pressed on. A short lock and some sums.
    pub fn on_given_page(&self, page: u32, x: f64, y: f64) -> Option<recto_index::select::PagePoint> {
        lock(&self.engine.shared.reader).docs.shown()?.point_on_given_page(page, x, y)
    }

    /// How many pages the tab shown has, edits and all. A short lock, for what can't wait, as the print panel can't.
    pub fn page_count(&self) -> u32 {
        lock(&self.engine.shared.reader).docs.shown().map_or(0, |doc| doc.page_count())
    }

    /// A page's size as it's shown, in its own coordinates. A short lock.
    pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)> {
        lock(&self.engine.shared.reader).docs.shown()?.page_size_shown(page)
    }

    /// How far a page is shown turned, in quarter turns clockwise. A short lock.
    pub fn page_turns(&self, page: u32) -> u8 {
        lock(&self.engine.shared.reader).docs.shown().map_or(0, |doc| doc.page_turns(page))
    }

    /// Where a point on a page is in the document.
    pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        lock(&self.engine.shared.reader).docs.shown()?.document_point(page, x, y)
    }

    /// **The contents' band for the page at the top of the tab showing's view:** the page, and the thousandths of it between which the same contents entry is
    /// current, or none. Leaving it wakes the core once, by the wake a page change has, and then not until this is set again, which the core does with each
    /// pass; and set with the top of the view already outside it, it wakes the core at once ([`crate::tiles::set_band`]). A short lock.
    pub fn set_band(&self, band: Option<(u32, u16, u16)>) {
        crate::tiles::set_band(&mut lock(&self.engine.shared.reader), band);
    }

    pub fn has_document(&self) -> bool {
        lock(&self.engine.shared.reader).docs.shown().is_some_and(|doc| doc.has_document())
    }
}
