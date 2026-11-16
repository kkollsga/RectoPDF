//! **Points, pages, place and zoom of a tab the page view holds:** where a point of the document is on a page as it's shown, and back, which page of which
//! file a position is, and where the reader is in it and at what zoom. Each reads that one tab's view and nothing else of the page view, so they're
//! [`DocView`]'s own; the engine reaches the tab showing's through [`super::Docs::shown`].

use super::doc::DocView;
use recto_index::select::PagePoint;

impl DocView {
    /// Where a point in the document is on a page as it's shown. It stays with the page it's nearest, past the page's edges if it must, which is what a drag off the side needs.
    pub(crate) fn point_on_page(&self, x: f64, y: f64) -> Option<PagePoint> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        if self.layout.count() == 0 {
            return None;
        }
        let page = self.layout.page_at_point(x, y);
        let (u, v) = self.layout.to_page(page, x, y);
        Some(PagePoint {
            page,
            x: u as f32,
            y: v as f32,
        })
    }

    /// Where a point in the document is in the coordinates of `page`, whichever page it's over: past the page's edges if it's off it, for the caller to hold to them. A snapshot's box dragged across a page break stays on the page it was pressed on this way.
    pub(crate) fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<PagePoint> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        if page >= self.layout.count() {
            return None;
        }
        let (u, v) = self.layout.to_page(page, x, y);
        Some(PagePoint {
            page,
            x: u as f32,
            y: v as f32,
        })
    }

    /// Whether a point is on its page, and not in the margin or the gap between pages.
    pub(crate) fn is_on_its_page(&self, point: PagePoint) -> bool {
        let size = self.layout.raw_size(point.page);
        point.x >= 0.0 && point.y >= 0.0 && point.x <= size.width && point.y <= size.height
    }

    /// How many pages the tab has, edits and all.
    pub(crate) fn page_count(&self) -> u32 {
        self.layout.count()
    }

    /// A page's size as it's shown, in its own coordinates, before any unsaved turn: what a point from [`Self::point_on_page`] is held to.
    pub(crate) fn page_size_shown(&self, page: u32) -> Option<(f32, f32)> {
        (page < self.layout.count()).then(|| {
            let size = self.layout.raw_size(page);
            (size.width, size.height)
        })
    }

    /// How far a page is shown turned, in quarter turns clockwise: its unsaved turn and the view's together. What a snapshot of it is turned by.
    pub(crate) fn page_turns(&self, page: u32) -> u8 {
        self.layout.turns_of(page)
    }

    /// The other way, for QA, which says where a click goes as an agent found it from `recto select`, and then does what the mouse does.
    pub(crate) fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        if page >= self.layout.count() {
            return None;
        }
        Some(self.layout.from_page(page, x, y))
    }

    /// Where a file page of the same file as the page at `from` is now, counted as the view counts: what a link on that page means by its page. Nothing if it's not in the document.
    pub(crate) fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32> {
        match &self.map.slots {
            None => (file_page < self.layout.count()).then_some(file_page),
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
    pub(crate) fn own_file_page(&self, position: u32) -> Option<u32> {
        self.page_in_file(position).filter(|(_, own)| *own).map(|(page, _)| page)
    }

    /// Which page of its file a page of the view is, and whether that file is the tab's own.
    pub(crate) fn page_in_file(&self, position: u32) -> Option<(u32, bool)> {
        match &self.map.slots {
            None => (position < self.layout.count()).then_some((position, true)),
            Some(slots) => slots.get(position as usize).map(|(file, page)| (*page, *file == 0)),
        }
    }

    pub(crate) fn has_document(&self) -> bool {
        self.layout.count() > 0
    }

    /// The tab's fit: document points per PDF point, what a zoom level is worked out from.
    pub(crate) fn fit(&self) -> f64 {
        self.layout.fit()
    }

    /// The tab's magnification.
    pub(crate) fn magnification(&self) -> f64 {
        self.place.magnification
    }

    /// **The host's window width and magnification, taken into the layout, and where across the view goes with them.** A row is centred in the view, which
    /// is the window's width over the zoom in document points ([`crate::layout::PageLayout::view_width`]), so a zoom or a resize moves rows across, and the
    /// host has the view across where it was under the old places. Measured before anything moves: the point of the page at the view's middle, or the row's
    /// place across. After a resize or the sidebar shown or hidden (the window's width changed), whose host keeps its left edge, the view goes where that
    /// point of the page is at its middle again; after a zoom alone, which the host made about a point (the pointer, or the view's middle), it goes as far
    /// across as that row moved, so the point stays. Held to what the document has to scroll across: none while the row fits, which centres it, and a row
    /// wider than the view from its left edge. Says whether the window's width changed the layout, and where the view goes across, if it's to move: nowhere
    /// while `jump` (a jump or a new window on its way, whose place across is sent with it), with no change, or from a view the host hadn't shown yet.
    /// A few lookups of the layout.
    pub(super) fn take_view(&mut self, was: &super::Viewport, viewport: &super::Viewport, jump: bool) -> (bool, Option<f64>) {
        let resized = was.frame_width != viewport.frame_width;
        let from = if resized { was } else { viewport };
        let page = self.layout.page_at(from.y + from.height / 2.0);
        let (before, fit) = (self.layout.rect(page).x, self.layout.fit());
        let point = (from.x + from.width / 2.0 - before) / fit.max(f64::MIN_POSITIVE);
        let moved_frame = self.layout.set_frame(viewport.frame_width) && resized;
        let zoomed = self.layout.set_magnification(viewport.magnification);
        if jump || self.layout.count() == 0 || !(moved_frame || zoomed) || (resized && was.width <= 0.0) {
            return (moved_frame, None);
        }
        let now = self.layout.rect(page).x;
        let (x, y) = if moved_frame {
            (now + point * self.layout.fit() - self.layout.view_width() / 2.0, self.place_y())
        } else {
            (viewport.x + now - before, viewport.y)
        };
        let room = (self.layout.document_width_for(y, y + viewport.height) - self.layout.view_width()).max(0.0);
        let x = x.clamp(0.0, room);
        (moved_frame, ((x - viewport.x).abs() > 1e-9).then_some(x))
    }

    /// Where the reader is in the tab: a page and how far down it, as a change of layout keeps it.
    pub(crate) fn reading_anchor(&self) -> (u32, f64) {
        self.place.anchor
    }

    /// How far down the tab's place is: to the pixel if the layout's what it was, and otherwise as far down its page.
    pub(crate) fn place_y(&self) -> f64 {
        match self.place.exact_y {
            Some((y, height)) if height == self.layout.document_height() => y,
            _ => self.layout.anchor_y(self.place.anchor),
        }
    }
}
