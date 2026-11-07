//! Where pages sit, in document points: the units of the host's scrolled view at a magnification of 1.
//!
//! A hostile file can claim millions of pages, and a page's size costs a round trip to the worker, so neither is paid for up front. Every page is taken to be the size of the first one known, until it's asked about.
//! Only pages of another size are stored. A document of one page size costs nothing per page, however long, and one of mixed sizes costs [`Override`] per page visited of another size, and at most [`MAX_OVERRIDES`].

use std::collections::HashSet;

/// Space between pages, in document points.
pub const PAGE_GAP: f64 = 10.0;
/// How wide a page is drawn when there's room, in document points. Concepts: `docs/concepts/reader-light.png`.
pub const READING_WIDTH: f64 = 760.0;
/// The same in focus view. Concepts: `docs/concepts/focus-light.png`.
pub const FOCUS_WIDTH: f64 = 900.0;
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

/// A page whose size isn't the document's usual one.
// size: asserted <= 32 by `an_override_stays_small`. Held per visited page of another size, at most MAX_OVERRIDES.
#[derive(Clone, Copy, Debug)]
struct Override {
    page: u32,
    size: PageBox,
    /// The difference in points that the overrides before this one make to everything below them.
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
    focus: bool,
}

impl PageLayout {
    pub fn new(count: u32) -> Self {
        Self { count, usual: DEFAULT_PAGE, usual_known: false, overrides: Vec::new(), known: HashSet::new(), fit: 1.0, frame_width: 0.0, focus: false }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn fit(&self) -> f64 {
        self.fit
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

    /// Sets the window's reading area and focus. Returns whether anything moved.
    pub fn set_frame(&mut self, frame_width: f64, focus: bool) -> bool {
        if frame_width == self.frame_width && focus == self.focus {
            return false;
        }
        self.frame_width = frame_width;
        self.focus = focus;
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
            self.overrides.insert(at, Override { page, size, before: 0.0 });
        }
        self.reckon();
        self.refit();
        true
    }

    fn reckon(&mut self) {
        let usual = f64::from(self.usual.height);
        let mut total = 0.0;
        for entry in &mut self.overrides {
            entry.before = total;
            total += f64::from(entry.size.height) - usual;
        }
    }

    /// The fit follows the first page known, so pages keep their relative sizes, as they do on paper.
    fn refit(&mut self) -> bool {
        let measure = if self.focus { FOCUS_WIDTH } else { READING_WIDTH };
        let room = (self.frame_width - 2.0 * MIN_SIDE_MARGIN).max(1.0);
        let fit = (measure.min(room) / f64::from(self.usual.width)).clamp(1.0 / 64.0, MAX_FIT);
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

    /// How far down a page starts.
    pub fn top(&self, page: u32) -> f64 {
        let at = self.overrides.partition_point(|o| o.page < page);
        let extra = match self.overrides.get(at.wrapping_sub(1)) {
            Some(last) if at > 0 => last.before + f64::from(last.size.height) - f64::from(self.usual.height),
            _ => 0.0,
        };
        f64::from(page) * self.step() + extra * self.fit
    }

    pub fn rect(&self, page: u32) -> PageRect {
        let size = self.size(page);
        let width = f64::from(size.width) * self.fit;
        PageRect { x: ((self.document_width() - width) / 2.0).max(0.0), y: self.top(page), width, height: f64::from(size.height) * self.fit }
    }

    /// The widest a page is known to be, and the window's room.
    pub fn document_width(&self) -> f64 {
        let widest = self.overrides.iter().map(|o| o.size.width).fold(self.usual.width, f32::max);
        self.frame_width.max(f64::from(widest) * self.fit + 2.0 * MIN_SIDE_MARGIN)
    }

    pub fn document_height(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let last = self.count - 1;
        self.top(last) + f64::from(self.size(last).height) * self.fit
    }

    /// The page at or just below a height. Above the first page it's the first, and past the end the last.
    pub fn page_at(&self, y: f64) -> u32 {
        if self.count == 0 || y <= 0.0 {
            return 0;
        }
        let (mut low, mut high) = (0u32, self.count - 1);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if self.top(middle) <= y {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }

    /// Which pages a stretch of the document touches.
    pub fn pages_between(&self, from: f64, to: f64) -> std::ops::RangeInclusive<u32> {
        let first = self.page_at(from);
        let last = self.page_at(to);
        // A stretch that ends in the gap below a page needn't count the next one, and one that starts in a gap doesn't count the page above.
        let first = if first < self.count.saturating_sub(1) && self.rect(first).y + self.rect(first).height < from { first + 1 } else { first };
        first..=last.max(first)
    }

    /// Where a place in a page is, to keep it in view across a change.
    pub fn anchor_at(&self, y: f64) -> (u32, f64) {
        let page = self.page_at(y);
        let rect = self.rect(page);
        (page, ((y - rect.y) / rect.height.max(1.0)).clamp(0.0, 1.0))
    }

    pub fn anchor_y(&self, (page, within): (u32, f64)) -> f64 {
        let rect = self.rect(page.min(self.count.saturating_sub(1)));
        rect.y + within * rect.height
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
        layout.set_frame(1168.0, false);
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
        layout.set_frame(1168.0, false);
        layout.set_page_size(0, letter());
        for (page, height) in [(3, 400.0), (50, 1200.0), (51, 100.0), (199, 2000.0)] {
            layout.set_page_size(page, PageBox { width: 612.0, height });
        }
        let mut expected_top = 0.0;
        for page in 0..200 {
            assert!((layout.top(page) - expected_top).abs() < 1e-6, "page {page}: {} against {expected_top}", layout.top(page));
            let rect = layout.rect(page);
            assert_eq!(layout.page_at(rect.y + rect.height / 2.0), page);
            assert_eq!(layout.page_at(rect.y), page);
            expected_top += rect.height + PAGE_GAP;
        }
        assert!((layout.document_height() - (expected_top - PAGE_GAP)).abs() < 1e-6);
    }

    /// Failure: a tiny or a huge hostile page gives a scale off the end of what can be drawn.
    #[test]
    fn the_fit_stays_in_bounds_whatever_a_page_says() {
        for width in [0.01, 1.0, 612.0, 14_400.0, 1_000_000.0] {
            let mut layout = PageLayout::new(2);
            layout.set_frame(1168.0, false);
            layout.set_page_size(0, PageBox { width, height: 100.0 });
            assert!(layout.fit() >= 1.0 / 64.0 && layout.fit() <= MAX_FIT, "{width}: {}", layout.fit());
        }
        let mut layout = PageLayout::new(2);
        assert!(!layout.set_page_size(0, PageBox { width: f32::NAN, height: 100.0 }));
        assert!(!layout.set_page_size(0, PageBox { width: -1.0, height: 100.0 }));
        assert!(!layout.set_page_size(9, letter()), "no such page");
    }

    /// Failure: a page is drawn into the margin the agent icon needs, or stops at a width the concept doesn't, or focus view doesn't get wider.
    #[test]
    fn a_page_keeps_the_margins_and_focus_is_wider() {
        let mut layout = PageLayout::new(1);
        layout.set_page_size(0, letter());
        layout.set_frame(1168.0, false);
        let normal = layout.rect(0);
        assert!((normal.width - READING_WIDTH).abs() < 1e-6);
        assert!(normal.x >= MIN_SIDE_MARGIN);
        layout.set_frame(1440.0, true);
        assert!((layout.rect(0).width - FOCUS_WIDTH).abs() < 1e-6);
        layout.set_frame(600.0, false);
        let narrow = layout.rect(0);
        assert!(narrow.x >= MIN_SIDE_MARGIN - 1e-6 && narrow.width <= 600.0 - 2.0 * MIN_SIDE_MARGIN + 1e-6);
    }

    /// Failure: a change above the reader moves what they're reading, because the place was kept as a height rather than a place on a page.
    #[test]
    fn an_anchor_survives_a_change_above_it() {
        let mut layout = PageLayout::new(100);
        layout.set_frame(1168.0, false);
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
