//! **Points and pages:** where a point of the document is on a page as it's shown, and back, and which page of which file a position is.

use super::Reader;
use recto_index::select::PagePoint;

impl Reader {
    /// Where a point in the document is on a page as it's shown. It stays with the page it's nearest, past the page's edges if it must, which is what a drag off the side needs.
    pub fn point_on_page(&self, x: f64, y: f64) -> Option<PagePoint> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let doc = self.doc()?;
        if doc.layout.count() == 0 {
            return None;
        }
        let page = doc.layout.page_at_point(x, y);
        let (u, v) = doc.layout.to_page(page, x, y);
        Some(PagePoint {
            page,
            x: u as f32,
            y: v as f32,
        })
    }

    /// Where a point in the document is in the coordinates of `page`, whichever page it's over: past the page's edges if it's off it, for the caller to hold to them. A snapshot's box dragged across a page break stays on the page it was pressed on this way.
    pub fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<PagePoint> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let doc = self.doc()?;
        if page >= doc.layout.count() {
            return None;
        }
        let (u, v) = doc.layout.to_page(page, x, y);
        Some(PagePoint {
            page,
            x: u as f32,
            y: v as f32,
        })
    }

    /// How many pages the tab shown has, edits and all: 0 with none shown.
    pub fn page_count(&self) -> u32 {
        self.doc().map_or(0, |doc| doc.layout.count())
    }

    /// A page's size as it's shown, in its own coordinates, before any unsaved turn: what a point from [`Self::point_on_page`] is held to.
    pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)> {
        let doc = self.doc()?;
        (page < doc.layout.count()).then(|| {
            let size = doc.layout.raw_size(page);
            (size.width, size.height)
        })
    }

    /// How far a page is shown turned, in quarter turns clockwise: its unsaved turn and the view's together. What a snapshot of it is turned by.
    pub fn page_turns(&self, page: u32) -> u8 {
        self.doc().map_or(0, |doc| doc.layout.turns_of(page))
    }

    /// The other way, for QA, which says where a click goes as an agent found it from `recto select`, and then does what the mouse does.
    pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        let doc = self.doc()?;
        if page >= doc.layout.count() {
            return None;
        }
        Some(doc.layout.from_page(page, x, y))
    }

    /// Where a file page of the same file as the page at `from` is now, counted as the view counts: what a link on that page means by its page. Nothing if it's not in the document.
    pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32> {
        let doc = self.doc()?;
        match &doc.map.slots {
            None => (file_page < doc.layout.count()).then_some(file_page),
            Some(slots) => {
                let (file, _) = *slots.get(from as usize)?;
                slots
                    .iter()
                    .position(|(other, page)| *other == file && *page == file_page)
                    .and_then(|at| u32::try_from(at).ok())
            }
        }
    }

    /// Whether a page of the view is from the tab's own file, and which page of it.
    pub fn own_file_page(&self, position: u32) -> Option<u32> {
        let doc = self.doc()?;
        match &doc.map.slots {
            None => (position < doc.layout.count()).then_some(position),
            Some(slots) => slots.get(position as usize).filter(|(file, _)| *file == 0).map(|(_, page)| *page),
        }
    }

    pub fn has_document(&self) -> bool {
        self.doc().is_some_and(|doc| doc.layout.count() > 0)
    }
}
