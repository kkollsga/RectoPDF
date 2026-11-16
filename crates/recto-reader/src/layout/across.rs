//! **Where a row of pages sits across, and how wide the document is:** the one place that decides a page's x and the width the host scrolls over.
//!
//! **A row no wider than the view is centred in it, with equal margins and no sideways scroll; a wider row starts at the document's left edge, and the view
//! scrolls over exactly its width** (owner, 2026-11-16: "when I zoom into the page only the right margin is reduced. Why? The page should always be
//! centered (if there is no scrollbar)"). The view is the window's reading area at the host's magnification ([`PageLayout::view_width`]), so zooming in
//! narrows it in document points: the margins shrink together, to nothing as the row reaches the window's width, and past that the row runs from the
//! left edge. At the fit the view is the window, and a fitted row, which keeps [`PageLayout::side`] either side, is where it always was. The margins
//! kept clear for the agent icon and the zoom level in the top right corner are the fit's alone: a page the person has zoomed wider than that room may
//! pass under them. Each row is centred on its own, so a fold-out doesn't push the other rows aside, and a pair side by side is centred as a whole.

use super::{MAX_WIDTH_PAGES, PAGE_GAP, PageLayout, PageRect, Spread};

impl PageLayout {
    /// The host's magnification, which the view's width in document points follows. Returns whether anything moved. Held to sense: a magnification that
    /// isn't a positive number is none.
    pub fn set_magnification(&mut self, magnification: f64) -> bool {
        let magnification = if magnification.is_finite() && magnification > 0.0 {
            magnification
        } else {
            1.0
        };
        if magnification == self.magnification {
            return false;
        }
        self.magnification = magnification;
        true
    }

    /// How wide the view is, in document points: the window's reading area over the magnification. What a row is centred in.
    pub fn view_width(&self) -> f64 {
        self.frame_width / self.magnification
    }

    pub fn rect(&self, page: u32) -> PageRect {
        let size = self.size(page);
        let width = f64::from(size.width) * self.fit;
        let middle = self.row_middle(page);
        let x = match self.spread {
            Spread::Single => middle - width / 2.0,
            _ if self.on_the_right(page) => middle + PAGE_GAP / 2.0,
            _ => middle - PAGE_GAP / 2.0 - width,
        };
        PageRect {
            x: x.max(0.0),
            y: self.top(page),
            width,
            height: f64::from(size.height) * self.fit,
        }
    }

    /// Where a page's row is centred across: the middle of the view, each row on its own, so one wide page, as a fold-out, doesn't push every other row
    /// off to the side. A row wider than the view starts at the document's left edge. Side by side, the middle of a pair is the gap between its pages.
    fn row_middle(&self, page: u32) -> f64 {
        let (left, right, gap, shift) = self.row_columns(page);
        self.middle_in(self.view_width(), left, right, gap, shift)
    }

    /// **A row's middle in a view this wide:** the view's middle, moved by `shift`, held inside the margins the row would have centred: the fit's
    /// [`PageLayout::side`], or less, down to none, where the row is wider than the room between them. A row that's centred is inside them by
    /// itself, so that holds only a page alone in a two-up row, whose column is centred as if its pair were there and can be wider than its pair would be
    /// (a wide last page): it's kept off the edge by the margin, as at the fit, and by less as the zoom grows. `left` and `right` are the widths either side
    /// of the middle, less half the gap between a pair.
    fn middle_in(&self, view: f64, left: f64, right: f64, gap: f64, shift: f64) -> f64 {
        let margin = self.side.min(((view - (left + right + gap)) / 2.0).max(0.0));
        (view / 2.0 + shift).min(view - margin - right - gap / 2.0).max(margin + left + gap / 2.0)
    }

    /// A row's widths left and right of its middle, the gap between them, and how far its middle is off the view's: a pair of different widths is
    /// centred as a whole, so its gap is off the middle by half the difference.
    fn row_columns(&self, page: u32) -> (f64, f64, f64, f64) {
        let width = |page: u32| f64::from(self.size(page).width) * self.fit;
        match self.spread {
            Spread::Single => (width(page) / 2.0, width(page) / 2.0, 0.0, 0.0),
            _ => {
                let (first, second) = self.row_pages(self.row_of(page));
                let pages = [Some(first), second];
                // A page alone, the cover or the last of an odd count, sits in its column as if its pair were there, so the columns line up.
                let usual = f64::from(self.usual.width) * self.fit;
                let left = pages.into_iter().flatten().find(|page| !self.on_the_right(*page)).map_or(usual, width);
                let right = pages.into_iter().flatten().find(|page| self.on_the_right(*page)).map_or(usual, width);
                let shift = if second.is_some() { (left - right) / 2.0 } else { 0.0 };
                (left, right, PAGE_GAP, shift)
            }
        }
    }

    /// Where a row's right edge is: its right-hand page's, or for a page alone in a two-up row, its own.
    fn row_right(&self, page: u32) -> f64 {
        let (first, second) = self.row_pages(self.row_of(page));
        [Some(first), second]
            .into_iter()
            .flatten()
            .map(|page| {
                let rect = self.rect(page);
                rect.x + rect.width
            })
            .fold(0.0, f64::max)
    }

    /// Where the right edge of a row of the usual pages is, a pair of them side by side.
    fn usual_row_right(&self) -> f64 {
        let usual = f64::from(self.usual.width) * self.fit;
        let view = self.view_width();
        match self.spread {
            Spread::Single => (self.middle_in(view, usual / 2.0, usual / 2.0, 0.0, 0.0) - usual / 2.0).max(0.0) + usual,
            Spread::TwoUp | Spread::TwoUpCover => self.middle_in(view, usual, usual, PAGE_GAP, 0.0) + PAGE_GAP / 2.0 + usual,
        }
    }

    /// The view's width, or the widest row's where that's wider: the most the document can be across, wherever the view is.
    pub fn document_width(&self) -> f64 {
        let widest = f64::from(self.overrides.iter().map(|o| o.size.width).fold(self.usual.width, f32::max)) * self.fit;
        let pages = match self.spread {
            Spread::Single => widest,
            Spread::TwoUp | Spread::TwoUpCover => 2.0 * widest + PAGE_GAP,
        };
        self.view_width().max(pages)
    }

    /// **The document's width for a view from `top` to `bottom`,** in document points: the view's width, and wider only where a row the view reaches
    /// runs past it, as a page zoomed past the window or a fold-out does, as far as that row's right edge. So there's room to scroll across a wide row
    /// while it shows, and none beside a row that fits (PLAN, side-by-side follow-ups). A view over more than [`MAX_WIDTH_PAGES`] pages of their own size
    /// takes the whole document's.
    pub fn document_width_for(&self, top: f64, bottom: f64) -> f64 {
        if self.count == 0 || !top.is_finite() || !bottom.is_finite() {
            return self.document_width();
        }
        let (first, last) = (self.row_at(top), self.row_at(bottom.max(top)));
        let (from, _) = self.row_pages(first);
        let to = match self.row_pages(last) {
            (page, None) => page,
            (_, Some(second)) => second,
        };
        let start = self.overrides.partition_point(|o| o.page < from);
        let end = self.overrides.partition_point(|o| o.page <= to);
        if end - start > MAX_WIDTH_PAGES {
            return self.document_width();
        }
        // Each row with a page of its own size, once; any other row in view is of the usual pages.
        let (mut right, mut rows_of_their_own, mut row_was) = (0.0, 0u32, None);
        for entry in &self.overrides[start..end] {
            let row = self.row_of(entry.page);
            if row_was != Some(row) {
                row_was = Some(row);
                rows_of_their_own += 1;
                right = self.row_right(entry.page).max(right);
            }
        }
        if rows_of_their_own <= last - first {
            right = self.usual_row_right().max(right);
        }
        self.view_width().max(right)
    }
}
