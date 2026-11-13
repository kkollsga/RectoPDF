//! **The links on the pages, from the engine's callers:** the link under the pointer and the one a click landed on, and where a link's page is now in a tab whose pages were moved. The bridge (pointer and click) and the session's jump to a link take it alone.
//!
//! A borrow of the [`Engine`], built inline by [`Engine::links`]: each method takes the page view's lock exactly as the `Engine` method it was until R3.6
//! did, in the same order with the same locks after it, so the engine's lock map holds as it was.

use super::{Engine, lock};

/// The links on this page view's pages.
#[derive(Clone, Copy)]
pub struct Links<'a> {
    engine: &'a Engine,
}

// size: one reference, so taking it costs what calling a method of `Engine` did.
const _: () = assert!(size_of::<Links<'static>>() <= size_of::<usize>(), "a facade of the engine grew");

impl Engine {
    /// The links on this page view's pages.
    #[inline]
    pub fn links(&self) -> Links<'_> {
        Links { engine: self }
    }
}

impl Links<'_> {
    /// The link under the pointer, if its page's links are here. A page not read yet is read next. A short lock, on the UI thread.
    pub fn at(&self, x: f64, y: f64) -> Option<crate::links::LinkHit> {
        let mut reader = lock(&self.engine.shared.reader);
        let hit = reader.pointer_link(x, y);
        drop(reader);
        self.engine.wake();
        hit
    }

    /// The link the last click landed on, taken once.
    pub fn take_clicked(&self) -> Option<crate::links::LinkHit> {
        lock(&self.engine.shared.reader).pointer.take_clicked_link()
    }

    /// Where a link's page is now: see [`crate::tiles::DocView::position_in_same_file`].
    pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32> {
        lock(&self.engine.shared.reader).doc()?.position_in_same_file(from, file_page)
    }

    /// Which page of the tab's own file a page of the view is, if it's one.
    pub fn own_file_page(&self, position: u32) -> Option<u32> {
        lock(&self.engine.shared.reader).doc()?.own_file_page(position)
    }
}
