//! Where pages sit, in document points: the units of the host's scrolled view at a magnification of 1.
//!
//! A hostile file can claim millions of pages, and a page's size costs a round trip to the worker, so neither is paid for up front. Every page is taken to be the size of the first one known, until it's asked about.
//! Only pages of another size are stored. A document of one page size costs nothing per page, however long, and one of mixed sizes costs [`Override`] per page visited of another size, and at most [`MAX_OVERRIDES`].

use std::collections::HashSet;

/// Space between pages, in document points.
pub const PAGE_GAP: f64 = 10.0;
/// Space above the first page and below the last, a little over twice the gap between pages, so the document starts and ends on the margin's colour. Owner, 2026-11-08: "add some margin above the first page, and below the last page".
pub const END_MARGIN: f64 = 24.0;
/// How wide a page is drawn when there's room, in document points. Concepts: `docs/concepts/reader-light.png`.
pub const READING_WIDTH: f64 = 760.0;
/// The least room either side of a fitted page, so the agent icon's margin stays empty.
pub const MIN_SIDE_MARGIN: f64 = 56.0;
/// How far a fit may enlarge a tiny page, so a hostile page of a few points doesn't ask for a huge scale.
pub const MAX_FIT: f64 = 8.0;
/// Pages taken to be US Letter until one is known.
const DEFAULT_PAGE: PageBox = PageBox { width: 612.0, height: 792.0 };
/// A page of another size kept, past which they're forgotten and asked for again. Far more than anyone visits, and a file can't grow this without the reader scrolling through it.
pub const MAX_OVERRIDES: usize = 65_536;
/// Pages known to be asked about, past which they're forgotten and asked for again.
const MAX_KNOWN: usize = 65_536;

/// A page's size in points, after its rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageBox {
    pub width: f32,
    pub height: f32,
}

/// A page's place, in document points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// How pages are laid out: one to a row, or two side by side. Owner, 2026-11-07: "In the right click menu you can select side by side layout, with and without header page."
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spread {
    #[default]
    Single,
    /// Pages 1 and 2 side by side, then 3 and 4.
    TwoUp,
    /// The first page alone on the right, as a book's cover is, then 2 and 3 side by side.
    TwoUpCover,
}

/// A page whose size isn't the document's usual one.
// size: asserted <= 32 by `an_override_stays_small`. Held per visited page of another size, at most MAX_OVERRIDES.
#[derive(Clone, Copy, Debug)]
struct Override {
    page: u32,
    size: PageBox,
    /// How much taller than a usual row this page's row is, in points, which can be less than nothing. The same for every override in a row.
    row_extra: f32,
    /// The difference in points that the rows before this one's make to everything below them.
    before: f64,
}

#[derive(Debug)]
pub struct PageLayout {
    count: u32,
    /// The size every page is taken to be.
    usual: PageBox,
    /// Whether `usual` came from a real page.
    usual_known: bool,
    overrides: Vec<Override>,
    known: HashSet<u32>,
    /// Document points per PDF point.
    fit: f64,
    frame_width: f64,
    spread: Spread,
}

impl PageLayout {
    pub fn new(count: u32) -> Self {
        Self::with_spread(count, Spread::Single)
    }

    pub fn with_spread(count: u32, spread: Spread) -> Self {
        Self { count, usual: DEFAULT_PAGE, usual_known: false, overrides: Vec::new(), known: HashSet::new(), fit: 1.0, frame_width: 0.0, spread }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn fit(&self) -> f64 {
        self.fit
    }

    pub fn spread(&self) -> Spread {
        self.spread
    }

    /// Lays the pages out another way. Returns whether anything moved.
    pub fn set_spread(&mut self, spread: Spread) -> bool {
        if spread == self.spread {
            return false;
        }
        self.spread = spread;
        self.reckon();
        self.refit();
        true
    }

    /// Whether any page's real size is known yet.
    pub fn has_real_size(&self) -> bool {
        self.usual_known
    }

    pub fn is_known(&self, page: u32) -> bool {
        self.known.contains(&page)
    }

    /// Takes every page to be this size to begin with, as it was found to be the last time, so the first frame has nothing to correct. A real size that differs wins when it comes.
    pub fn seed_usual(&mut self, size: PageBox) {
        if !self.usual_known && size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0 {
            self.usual = size;
            self.usual_known = true;
            self.reckon();
            self.refit();
        }
    }

    /// The pages that aren't the usual size, and their sizes.
    pub fn other_sizes(&self) -> Vec<(u32, PageBox)> {
        self.overrides.iter().map(|known| (known.page, known.size)).collect()
    }

    /// The size every page is taken to be, if it came from a real page.
    pub fn usual_size(&self) -> Option<PageBox> {
        self.usual_known.then_some(self.usual)
    }

    /// Sets the window's reading area. Returns whether anything moved.
    pub fn set_frame(&mut self, frame_width: f64) -> bool {
        if frame_width == self.frame_width {
            return false;
        }
        self.frame_width = frame_width;
        self.refit()
    }

    /// Notes a page's real size. Returns whether anything moved.
    pub fn set_page_size(&mut self, page: u32, size: PageBox) -> bool {
        if page >= self.count || !size.width.is_finite() || !size.height.is_finite() || size.width <= 0.0 || size.height <= 0.0 {
            return false;
        }
        if self.known.len() >= MAX_KNOWN {
            self.known.clear();
        }
        self.known.insert(page);
        if !self.usual_known {
            self.usual = size;
            self.usual_known = true;
            self.reckon();
            self.refit();
            return true;
        }
        let at = self.overrides.partition_point(|o| o.page < page);
        let existing = self.overrides.get(at).is_some_and(|o| o.page == page);
        let current = if existing { self.overrides[at].size } else { self.usual };
        if current == size {
            return false;
        }
        if existing {
            if size == self.usual {
                self.overrides.remove(at);
            } else {
                self.overrides[at].size = size;
            }
        } else {
            if self.overrides.len() >= MAX_OVERRIDES {
                self.overrides.clear();
                self.known.clear();
                self.known.insert(page);
            }
            let at = self.overrides.partition_point(|o| o.page < page);
            self.overrides.insert(at, Override { page, size, row_extra: 0.0, before: 0.0 });
        }
        self.reckon();
        self.refit();
        true
    }

    /// Which row a page is in.
    fn row_of(&self, page: u32) -> u32 {
        match self.spread {
            Spread::Single => page,
            Spread::TwoUp => page / 2,
            Spread::TwoUpCover => page.div_ceil(2),
        }
    }

    /// The first page of a row.
    fn first_of_row(&self, row: u32) -> u32 {
        match self.spread {
            Spread::Single => row,
            Spread::TwoUp => row.saturating_mul(2),
            Spread::TwoUpCover if row == 0 => 0,
            Spread::TwoUpCover => row.saturating_mul(2) - 1,
        }
    }

    /// The pages of a row: one, or two side by side.
    fn row_pages(&self, row: u32) -> (u32, Option<u32>) {
        let first = self.first_of_row(row);
        let pair = match self.spread {
            Spread::Single => false,
            Spread::TwoUp => true,
            Spread::TwoUpCover => row > 0,
        };
        (first, (pair && first + 1 < self.count).then_some(first + 1))
    }

    fn rows(&self) -> u32 {
        if self.count == 0 { 0 } else { self.row_of(self.count - 1) + 1 }
    }

    /// Whether a page is on the right of its row. A cover is, as a book's is; the last page of an odd count is on the left.
    fn on_the_right(&self, page: u32) -> bool {
        match self.spread {
            Spread::Single => false,
            Spread::TwoUp => page % 2 == 1,
            Spread::TwoUpCover => page % 2 == 0,
        }
    }

    /// Each override's row and what the rows before it add, in one pass.
    fn reckon(&mut self) {
        let usual = self.usual.height;
        let mut total = 0.0;
        let mut at = 0;
        while at < self.overrides.len() {
            let row = self.row_of(self.overrides[at].page);
            let mut end = at;
            while end < self.overrides.len() && self.row_of(self.overrides[end].page) == row {
                end += 1;
            }
            let (_, second) = self.row_pages(row);
            let in_row = 1 + usize::from(second.is_some());
            let tallest = self.overrides[at..end].iter().map(|o| o.size.height).fold(if end - at < in_row { usual } else { f32::MIN }, f32::max);
            let extra = tallest - usual;
            for entry in &mut self.overrides[at..end] {
                entry.before = total;
                entry.row_extra = extra;
            }
            total += f64::from(extra);
            at = end;
        }
    }

    /// The fit follows the first page known, so pages keep their relative sizes, as they do on paper. Two side by side share the room.
    fn refit(&mut self) -> bool {
        let room = (self.frame_width - 2.0 * MIN_SIDE_MARGIN).max(1.0);
        let fit = match self.spread {
            Spread::Single => READING_WIDTH.min(room) / f64::from(self.usual.width),
            Spread::TwoUp | Spread::TwoUpCover => ((2.0 * READING_WIDTH + PAGE_GAP).min(room) - PAGE_GAP).max(1.0) / (2.0 * f64::from(self.usual.width)),
        };
        let fit = fit.clamp(1.0 / 64.0, MAX_FIT);
        let moved = fit != self.fit;
        self.fit = fit;
        moved || self.frame_width > 0.0
    }

    pub fn size(&self, page: u32) -> PageBox {
        let at = self.overrides.partition_point(|o| o.page < page);
        match self.overrides.get(at) {
            Some(entry) if entry.page == page => entry.size,
            _ => self.usual,
        }
    }

    fn step(&self) -> f64 {
        f64::from(self.usual.height) * self.fit + PAGE_GAP
    }

    /// How far down a row starts.
    fn row_top(&self, row: u32) -> f64 {
        let first = self.first_of_row(row);
        let at = self.overrides.partition_point(|o| o.page < first);
        let extra = match self.overrides.get(at.wrapping_sub(1)) {
            Some(last) if at > 0 => last.before + f64::from(last.row_extra),
            _ => 0.0,
        };
        END_MARGIN + f64::from(row) * self.step() + extra * self.fit
    }

    /// How tall a row is, in document points.
    fn row_height(&self, row: u32) -> f64 {
        let first = self.first_of_row(row);
        let at = self.overrides.partition_point(|o| o.page < first);
        let extra = match self.overrides.get(at) {
            Some(entry) if self.row_of(entry.page) == row => f64::from(entry.row_extra),
            _ => 0.0,
        };
        (f64::from(self.usual.height) + extra) * self.fit
    }

    /// How far down a page starts: its row's top, as pages side by side line up at the top.
    pub fn top(&self, page: u32) -> f64 {
        self.row_top(self.row_of(page))
    }

    pub fn rect(&self, page: u32) -> PageRect {
        let size = self.size(page);
        let width = f64::from(size.width) * self.fit;
        let middle = self.document_width() / 2.0;
        let x = match self.spread {
            Spread::Single => middle - width / 2.0,
            _ if self.on_the_right(page) => middle + PAGE_GAP / 2.0,
            _ => middle - PAGE_GAP / 2.0 - width,
        };
        PageRect { x: x.max(0.0), y: self.top(page), width, height: f64::from(size.height) * self.fit }
    }

    /// The widest a page is known to be, and the window's room.
    pub fn document_width(&self) -> f64 {
        let widest = f64::from(self.overrides.iter().map(|o| o.size.width).fold(self.usual.width, f32::max)) * self.fit;
        let pages = match self.spread {
            Spread::Single => widest,
            Spread::TwoUp | Spread::TwoUpCover => 2.0 * widest + PAGE_GAP,
        };
        self.frame_width.max(pages + 2.0 * MIN_SIDE_MARGIN)
    }

    pub fn document_height(&self) -> f64 {
        let rows = self.rows();
        if rows == 0 {
            return 0.0;
        }
        self.row_top(rows - 1) + self.row_height(rows - 1) + END_MARGIN
    }

    /// The row at or just below a height. Above the first it's the first, and past the end the last.
    fn row_at(&self, y: f64) -> u32 {
        let rows = self.rows();
        if rows == 0 || y <= 0.0 {
            return 0;
        }
        let (mut low, mut high) = (0u32, rows - 1);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if self.row_top(middle) <= y {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }

    /// The page at or just below a height: the first of its row. Above the first page it's the first, and past the end the last row's.
    pub fn page_at(&self, y: f64) -> u32 {
        self.first_of_row(self.row_at(y))
    }

    /// The page under a point, or the nearest in its row, which is what a click or a drag between two pages needs.
    pub fn page_at_point(&self, x: f64, y: f64) -> u32 {
        let (first, second) = self.row_pages(self.row_at(y));
        let Some(second) = second else { return first };
        let distance = |page: u32| {
            let rect = self.rect(page);
            if x < rect.x { rect.x - x } else if x > rect.x + rect.width { x - rect.x - rect.width } else { 0.0 }
        };
        if distance(second) < distance(first) { second } else { first }
    }

    /// Which pages a stretch of the document touches.
    pub fn pages_between(&self, from: f64, to: f64) -> std::ops::RangeInclusive<u32> {
        let rows = self.rows();
        let first = self.row_at(from);
        let last = self.row_at(to);
        // A stretch that ends in the gap below a row needn't count the next one, and one that starts in a gap doesn't count the row above.
        let first = if first + 1 < rows && self.row_top(first) + self.row_height(first) < from { first + 1 } else { first };
        let last = last.max(first);
        let (last_first, last_second) = self.row_pages(last);
        self.first_of_row(first)..=last_second.unwrap_or(last_first)
    }

    /// Where a place in a page is, to keep it in view across a change.
    pub fn anchor_at(&self, y: f64) -> (u32, f64) {
        let page = self.page_at(y);
        let rect = self.rect(page);
        (page, ((y - rect.y) / rect.height.max(1.0)).clamp(0.0, 1.0))
    }

    pub fn anchor_y(&self, (page, within): (u32, f64)) -> f64 {
        // The top of the first row is the top of the document, so a new tab or one at its start shows the margin above it.
        if within <= 0.0 && self.row_of(page.min(self.count.saturating_sub(1))) == 0 {
            return 0.0;
        }
        let rect = self.rect(page.min(self.count.saturating_sub(1)));
        rect.y + within * rect.height
    }

    /// Whether two pages are side by side in one row.
    pub fn same_row(&self, one: u32, other: u32) -> bool {
        self.row_of(one) == self.row_of(other)
    }

    /// Where going to a page scrolls to: its top, or the document's for the first page, so the margin above it shows.
    pub fn jump_y(&self, page: u32) -> f64 {
        if self.row_of(page) == 0 { 0.0 } else { self.top(page) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter() -> PageBox {
        PageBox { width: 612.0, height: 792.0 }
    }

    /// Failure: every page of a hostile count costs memory on the hot tab. The most a file can claim must cost nothing until someone looks.
    #[test]
    fn a_huge_page_count_costs_nothing_until_visited() {
        let mut layout = PageLayout::new(8_388_607);
        layout.set_frame(1168.0);
        layout.set_page_size(0, letter());
        assert!(layout.overrides.is_empty());
        assert_eq!(layout.page_at(layout.top(8_000_000)), 8_000_000);
        assert!(layout.document_height() > 1e9);
    }

    /// Failure: a field added carelessly raises the cost of every odd-sized page visited.
    #[test]
    fn an_override_stays_small() {
        assert!(size_of::<Override>() <= 32);
    }

    /// Failure: finding the page under a height disagrees with where the pages are, so the wrong tiles are drawn once a document has pages of another size. Holds it against a list worked out the slow way.
    #[test]
    fn finding_a_page_agrees_with_placing_it() {
        let mut layout = PageLayout::new(200);
        layout.set_frame(1168.0);
        layout.set_page_size(0, letter());
        for (page, height) in [(3, 400.0), (50, 1200.0), (51, 100.0), (199, 2000.0)] {
            layout.set_page_size(page, PageBox { width: 612.0, height });
        }
        let mut expected_top = END_MARGIN;
        for page in 0..200 {
            assert!((layout.top(page) - expected_top).abs() < 1e-6, "page {page}: {} against {expected_top}", layout.top(page));
            let rect = layout.rect(page);
            assert_eq!(layout.page_at(rect.y + rect.height / 2.0), page);
            assert_eq!(layout.page_at(rect.y), page);
            expected_top += rect.height + PAGE_GAP;
        }
        assert!((layout.document_height() - (expected_top - PAGE_GAP + END_MARGIN)).abs() < 1e-6);
    }

    /// The slow way: each row's top and height from every page's size, for the layout to be held against.
    fn rows_the_slow_way(layout: &PageLayout, rows: &[Vec<u32>]) -> Vec<(f64, f64)> {
        let mut top = END_MARGIN;
        rows.iter()
            .map(|pages| {
                let height = pages.iter().map(|&page| f64::from(layout.size(page).height) * layout.fit()).fold(0.0, f64::max);
                let row = (top, height);
                top += height + PAGE_GAP;
                row
            })
            .collect()
    }

    /// Failure: side by side puts pages where finding them doesn't look, so the wrong tiles are drawn or a click lands on the wrong page; pages of a pair don't line up at the top, overlap, or sit off the middle; a row is as tall as its first page when the other is taller. Held against a list worked out the slow way, with odd-sized pages on either side, for both kinds.
    #[test]
    fn side_by_side_agrees_with_placing_its_pages() {
        for (spread, rows) in [(Spread::TwoUp, (0..7u32).map(|row| (row * 2..(row * 2 + 2).min(13)).collect()).collect::<Vec<Vec<u32>>>()), (Spread::TwoUpCover, std::iter::once(vec![0]).chain((0..6u32).map(|row| vec![row * 2 + 1, row * 2 + 2])).collect())] {
            let mut layout = PageLayout::with_spread(13, spread);
            layout.set_frame(1400.0);
            layout.set_page_size(0, letter());
            for (page, height) in [(3, 400.0), (4, 1200.0), (7, 1000.0), (8, 1000.0), (12, 300.0)] {
                layout.set_page_size(page, PageBox { width: 612.0, height });
            }
            let slow = rows_the_slow_way(&layout, &rows);
            for (row, pages) in rows.iter().enumerate() {
                let (top, height) = slow[row];
                for &page in pages {
                    let rect = layout.rect(page);
                    assert!((rect.y - top).abs() < 1e-6, "{spread:?} page {page}: {} against {top}", rect.y);
                    assert_eq!(layout.page_at(top + height / 2.0), pages[0], "{spread:?} row {row}");
                    assert_eq!(layout.page_at_point(rect.x + rect.width / 2.0, rect.y + 1.0), page, "{spread:?} a click on page {page}");
                }
                if let [left, right] = pages[..] {
                    let (one, two) = (layout.rect(left), layout.rect(right));
                    assert!((one.x + one.width + PAGE_GAP - two.x).abs() < 1e-6, "{spread:?} a pair meets with the gap between");
                    assert!(((one.x + one.width + two.x) / 2.0 - layout.document_width() / 2.0).abs() < 1e-6, "{spread:?} about the middle");
                    assert_eq!(layout.pages_between(top + 1.0, top + 2.0), left..=right);
                }
            }
            let (last_top, last_height) = slow[slow.len() - 1];
            assert!((layout.document_height() - (last_top + last_height + END_MARGIN)).abs() < 1e-6, "{spread:?}");
        }
        let mut cover = PageLayout::with_spread(4, Spread::TwoUpCover);
        cover.set_frame(1400.0);
        cover.set_page_size(0, letter());
        assert!(cover.rect(0).x > cover.document_width() / 2.0, "the cover's on the right, as a book's is");
        let mut odd = PageLayout::with_spread(3, Spread::TwoUp);
        odd.set_frame(1400.0);
        odd.set_page_size(0, letter());
        assert!(odd.rect(2).x + odd.rect(2).width < odd.document_width() / 2.0, "a last page alone is on the left");
    }

    /// Failure: two pages side by side don't fit the room, or run into the agent icon's margin, or are drawn larger than one page would be.
    #[test]
    fn side_by_side_shares_the_room() {
        let mut layout = PageLayout::with_spread(10, Spread::TwoUp);
        layout.set_page_size(0, letter());
        layout.set_frame(1100.0);
        let (left, right) = (layout.rect(0), layout.rect(1));
        assert!(left.x >= MIN_SIDE_MARGIN - 1e-6 && right.x + right.width <= 1100.0 - MIN_SIDE_MARGIN + 1e-6, "{left:?} {right:?}");
        assert!(left.width <= READING_WIDTH);
        layout.set_frame(4000.0);
        assert!((layout.rect(0).width - READING_WIDTH).abs() < 1e-6, "no wider than a page alone");
    }

    /// Failure: changing the layout loses the reader's place: the page they were on, and how far down it, aren't where the view goes. Owner's brief: side by side keeps the place in a document.
    #[test]
    fn the_place_survives_a_change_of_layout() {
        let mut layout = PageLayout::new(100);
        layout.set_frame(1400.0);
        layout.set_page_size(0, letter());
        layout.set_page_size(30, PageBox { width: 612.0, height: 1500.0 });
        let rect = layout.rect(41);
        let anchor = layout.anchor_at(rect.y + rect.height * 0.4);
        assert_eq!(anchor.0, 41);
        for spread in [Spread::TwoUp, Spread::TwoUpCover, Spread::Single] {
            assert!(layout.set_spread(spread));
            let y = layout.anchor_y(anchor);
            let now = layout.rect(41);
            assert!((y - (now.y + now.height * 0.4)).abs() < 1e-6, "{spread:?}");
            assert!(layout.pages_between(y, y + 1.0).contains(&41), "{spread:?}");
        }
        assert!(!layout.set_spread(Spread::Single), "the same layout moves nothing");
    }

    /// Failure: the first page starts at the window's top edge or the last ends at its bottom, with no margin, or going to page 1 hides the margin above it. Owner, 2026-11-08: "add some margin above the first page, and below the last page".
    #[test]
    fn the_document_has_a_margin_at_each_end() {
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            let mut layout = PageLayout::with_spread(5, spread);
            layout.set_frame(1200.0);
            layout.set_page_size(0, letter());
            assert!((layout.rect(0).y - END_MARGIN).abs() < 1e-9, "{spread:?}");
            let last = layout.rect(4);
            assert!((layout.document_height() - (last.y + last.height) - END_MARGIN).abs() < 1e-9, "{spread:?}");
            assert_eq!(layout.jump_y(0), 0.0);
            assert_eq!(layout.anchor_y((0, 0.0)), 0.0, "a tab at its start shows the margin too");
            assert_eq!(layout.page_at(1.0), 0, "the margin above is the first page's");
            assert_eq!(layout.page_at(layout.document_height() + 5.0), layout.first_of_row(layout.rows() - 1));
        }
        assert!(END_MARGIN > PAGE_GAP, "it reads as the document's end, not another gap");
    }

    /// Failure: a tiny or a huge hostile page gives a scale off the end of what can be drawn.
    #[test]
    fn the_fit_stays_in_bounds_whatever_a_page_says() {
        for width in [0.01, 1.0, 612.0, 14_400.0, 1_000_000.0] {
            let mut layout = PageLayout::new(2);
            layout.set_frame(1168.0);
            layout.set_page_size(0, PageBox { width, height: 100.0 });
            assert!(layout.fit() >= 1.0 / 64.0 && layout.fit() <= MAX_FIT, "{width}: {}", layout.fit());
        }
        let mut layout = PageLayout::new(2);
        assert!(!layout.set_page_size(0, PageBox { width: f32::NAN, height: 100.0 }));
        assert!(!layout.set_page_size(0, PageBox { width: -1.0, height: 100.0 }));
        assert!(!layout.set_page_size(9, letter()), "no such page");
    }

    /// Failure: a page is drawn into the margin the agent icon needs, or stops at a width the concept doesn't, or grows past the reading width with more room, as hiding the sidebar gives.
    #[test]
    fn a_page_keeps_the_margins_and_the_reading_width() {
        let mut layout = PageLayout::new(1);
        layout.set_page_size(0, letter());
        layout.set_frame(1168.0);
        let normal = layout.rect(0);
        assert!((normal.width - READING_WIDTH).abs() < 1e-6);
        assert!(normal.x >= MIN_SIDE_MARGIN);
        layout.set_frame(1440.0);
        assert!((layout.rect(0).width - READING_WIDTH).abs() < 1e-6);
        layout.set_frame(600.0);
        let narrow = layout.rect(0);
        assert!(narrow.x >= MIN_SIDE_MARGIN - 1e-6 && narrow.width <= 600.0 - 2.0 * MIN_SIDE_MARGIN + 1e-6);
    }

    /// Failure: a change above the reader moves what they're reading, because the place was kept as a height rather than a place on a page.
    #[test]
    fn an_anchor_survives_a_change_above_it() {
        let mut layout = PageLayout::new(100);
        layout.set_frame(1168.0);
        layout.set_page_size(0, letter());
        let rect = layout.rect(40);
        let anchor = layout.anchor_at(rect.y + rect.height * 0.25);
        layout.set_page_size(10, PageBox { width: 612.0, height: 2000.0 });
        let moved = layout.anchor_y(anchor);
        let now = layout.rect(40);
        assert!((moved - (now.y + now.height * 0.25)).abs() < 1e-6);
        assert!(now.y > rect.y);
    }
}
