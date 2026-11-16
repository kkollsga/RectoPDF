//! Where pages sit, in document points: the units of the host's scrolled view at a magnification of 1.
//!
//! A hostile file can claim millions of pages, and a page's size costs a round trip to the worker, so neither is paid for up front. Every page is taken to be the size of the first one known, until it's asked about.
//! Only pages of another size are stored. A document of one page size costs nothing per page, however long, and one of mixed sizes costs [`Override`] per page visited of another size, and at most [`MAX_OVERRIDES`].

use std::collections::HashSet;
use std::sync::Arc;

mod across;

/// Most pages of their own size [`PageLayout::document_width_for`] looks through for a view: more, as far zoomed out over a document of mixed sizes, and it takes the whole document's width.
pub const MAX_WIDTH_PAGES: usize = 4096;
/// Space between pages, in document points.
pub const PAGE_GAP: f64 = 10.0;
/// Space above the first page and below the last that's part of the document, a little over twice the gap between pages, so the document starts and ends on the margin's colour, and what a document opens at and going to page 1 show. Owner, 2026-11-08: "add some margin above the first page, and below the last page".
pub const END_MARGIN: f64 = 24.0;
/// The whole margin above the first page and below the last, as a share of that row's height: past [`END_MARGIN`], the rest is room the view can scroll into beyond the document's ends ([`PageLayout::end_room`]), so the first page can be brought down to the middle of the window and the last one up to it, and nothing about where pages are changes. Owner, 2026-11-08: "The margin above first page, and below last page should be 50% of page height". In document points, so it's half the page at any zoom.
pub const END_MARGIN_SHARE: f64 = 0.5;
/// How wide a page is drawn when there's room, in document points. Concepts: `docs/concepts/reader-light.png`.
pub const READING_WIDTH: f64 = 760.0;
/// The least room either side of a fitted page, so the agent icon's margin stays empty.
pub const MIN_SIDE_MARGIN: f64 = 56.0;
/// The least room to the right of a fitted page: the top right corner holds the agent icon and the zoom level beside it, and the owner's rule is that nothing sits over the page. The icon is 20 points, 12 from the edge, and the level's words, up to five characters, end 8 points left of it: about 84 points, and a little more so they don't touch. A margin the whole way down, as the left one is, so a page doesn't change width as it scrolls past the corner.
pub const CORNER_MARGIN: f64 = 96.0;
/// The least room either side of a page in a comparison's pane, where the window's margins are shared out among several pages side by side. The pane at the right keeps [`CORNER_MARGIN`] on its right, for the agent icon and the zoom level.
pub const PANE_MARGIN: f64 = 20.0;
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

fn swap(size: PageBox) -> PageBox {
    PageBox {
        width: size.height,
        height: size.width,
    }
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
    /// The window's reading area, unmagnified, in window points: what the fit is worked out from.
    frame_width: f64,
    /// The host's magnification: the view shows [`Self::frame_width`] over this, in document points, and rows are centred in that ([`across`]).
    magnification: f64,
    spread: Spread,
    /// Quarter turns clockwise added to each page by an edit, or none. A page drawn turned an odd number of times is as wide as it was tall.
    turns: Option<Arc<[u8]>>,
    /// Whether most pages are turned an odd number of times: `usual` is the size of those, as shown, and the others are overrides.
    base_odd: bool,
    /// How many pages are turned the other way from most. Their overrides are on top of the cap on pages of another size, as they're bounded by the edit list, which is a tab's own and capped where it's made.
    turned_other_way: u32,
    /// [`MAX_OVERRIDES`], smaller in a test so reaching it doesn't take a long time.
    cap: u32,
    /// **The room kept either side of the pages: the larger of the two margins, on both sides.** The top right corner holds the agent icon and the zoom level, so the right margin is the larger, and keeping it on the left too puts the pages in the middle of the window with equal gutters at any width (owner, 2026-11-12: "pages area should always have equal black gutters on both sides to appear centered, with and without sidebar, and ui elements like zoom and page number shouldn't impact this"). Before, a window narrower than the reading width plus both margins moved the page left of centre. The larger of [`MIN_SIDE_MARGIN`] and [`CORNER_MARGIN`], or of what a comparison's pane was given.
    side: f64,
}

impl PageLayout {
    pub fn new(count: u32) -> Self {
        Self::with_spread(count, Spread::Single)
    }

    pub fn with_spread(count: u32, spread: Spread) -> Self {
        Self {
            count,
            usual: DEFAULT_PAGE,
            usual_known: false,
            overrides: Vec::new(),
            known: HashSet::new(),
            fit: 1.0,
            frame_width: 0.0,
            magnification: 1.0,
            spread,
            turns: None,
            base_odd: false,
            turned_other_way: 0,
            cap: MAX_OVERRIDES as u32,
            side: MIN_SIDE_MARGIN.max(CORNER_MARGIN),
        }
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

    /// Takes every page to be this size to begin with, as it was found to be the last time, so the first frame has nothing to correct. A real size that differs wins when it comes. Its size as the file has it, unturned.
    pub fn seed_usual(&mut self, size: PageBox) {
        if !self.usual_known && size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0 {
            self.usual = if self.base_odd { swap(size) } else { size };
            self.usual_known = true;
            self.retune(None);
            self.refit();
        }
    }

    /// The pages that aren't the usual size, and their sizes as the file has them, unturned.
    pub fn other_sizes(&self) -> Vec<(u32, PageBox)> {
        self.overrides
            .iter()
            .filter(|known| self.known.contains(&known.page))
            .map(|known| (known.page, self.raw_size(known.page)))
            .collect()
    }

    /// The size every page is taken to be as the file has it, unturned, if it came from a real page.
    pub fn usual_size(&self) -> Option<PageBox> {
        self.usual_known.then_some(if self.base_odd { swap(self.usual) } else { self.usual })
    }

    /// A page's quarter turns, 0 to 3.
    pub fn turns_of(&self, page: u32) -> u8 {
        self.turns.as_deref().and_then(|turns| turns.get(page as usize)).map_or(0, |turns| turns % 4)
    }

    fn odd(&self, page: u32) -> bool {
        self.turns_of(page) % 2 == 1
    }

    /// A page's size as the file has it, unturned: what's drawn, before it's turned on the screen.
    pub fn raw_size(&self, page: u32) -> PageBox {
        let shown = self.size(page);
        if self.odd(page) { swap(shown) } else { shown }
    }

    /// Turns pages, as an edit has: the pages' sizes as shown follow, and what's drawn of them doesn't change. Returns whether anything moved.
    pub fn set_turns(&mut self, turns: Option<Arc<[u8]>>) -> bool {
        let turns = turns.filter(|turns| turns.iter().any(|turn| turn % 4 != 0));
        if turns.as_deref() == self.turns.as_deref() {
            return false;
        }
        let known: Vec<(u32, PageBox)> = self
            .overrides
            .iter()
            .filter(|o| self.known.contains(&o.page))
            .map(|o| (o.page, self.raw_size(o.page)))
            .collect();
        let usual_raw = if self.base_odd { swap(self.usual) } else { self.usual };
        self.turns = turns;
        let odd = self.turns.as_deref().map_or(0, |turns| turns.iter().filter(|turn| *turn % 2 == 1).count());
        self.base_odd = odd * 2 > self.count as usize;
        self.usual = if self.base_odd { swap(usual_raw) } else { usual_raw };
        self.retune(Some(known));
        self.refit();
        true
    }

    /// **The same layout, over a file that now has these pages as they're shown here,** as a save writes them: each page turned as the edit had it, so its size as the file has it is its size as shown. Nothing moves, the fit included, as the size the fit follows is the same. What was taken as a turned page's size counts as known, so a later turn keeps its shape; a real size that differs still wins when it comes.
    pub fn as_saved(&self) -> Self {
        let mut known = self.known.clone();
        known.extend(self.overrides.iter().map(|o| o.page));
        Self {
            count: self.count,
            usual: self.usual,
            usual_known: self.usual_known,
            overrides: self.overrides.clone(),
            known,
            fit: self.fit,
            frame_width: self.frame_width,
            magnification: self.magnification,
            spread: self.spread,
            turns: None,
            base_odd: false,
            turned_other_way: 0,
            cap: self.cap,
            side: self.side,
        }
    }

    /// Builds the overrides again: each known page of another size, and each page turned the other way from most, from their sizes as the file has them.
    fn retune(&mut self, known: Option<Vec<(u32, PageBox)>>) {
        let known = known.unwrap_or_else(|| {
            self.overrides
                .iter()
                .filter(|o| self.known.contains(&o.page))
                .map(|o| (o.page, self.raw_size(o.page)))
                .collect()
        });
        let usual_raw = if self.base_odd { swap(self.usual) } else { self.usual };
        let mut sizes: std::collections::BTreeMap<u32, PageBox> = known.into_iter().collect();
        self.turned_other_way = 0;
        if let Some(turns) = self.turns.as_deref() {
            for (page, turn) in turns.iter().enumerate().take(self.count as usize) {
                if (turn % 2 == 1) != self.base_odd {
                    sizes.entry(page as u32).or_insert(usual_raw);
                    self.turned_other_way += 1;
                }
            }
        }
        self.overrides = sizes
            .into_iter()
            .map(|(page, raw)| (page, if self.odd(page) { swap(raw) } else { raw }))
            .filter(|(_, shown)| *shown != self.usual)
            .take((self.cap + self.turned_other_way) as usize)
            .map(|(page, size)| Override {
                page,
                size,
                row_extra: 0.0,
                before: 0.0,
            })
            .collect();
        self.reckon();
    }

    /// The least room to the left and the right of the pages, as a pane of a comparison has it. Returns whether anything moved. Held to sense.
    pub fn set_margins(&mut self, left: f64, right: f64) -> bool {
        let fit = |value: f64| {
            if value.is_finite() {
                value.clamp(0.0, CORNER_MARGIN)
            } else {
                MIN_SIDE_MARGIN
            }
        };
        let side = fit(left).max(fit(right));
        if side == self.side {
            return false;
        }
        self.side = side;
        self.refit();
        true
    }

    /// Sets the window's reading area. Returns whether anything moved.
    pub fn set_frame(&mut self, frame_width: f64) -> bool {
        if frame_width == self.frame_width {
            return false;
        }
        self.frame_width = frame_width;
        self.refit()
    }

    /// Notes a page's real size, as the file has it, unturned. Returns whether anything moved.
    pub fn set_page_size(&mut self, page: u32, size: PageBox) -> bool {
        if page >= self.count || !size.width.is_finite() || !size.height.is_finite() || size.width <= 0.0 || size.height <= 0.0 {
            return false;
        }
        if self.known.len() >= MAX_KNOWN {
            self.known.clear();
        }
        self.known.insert(page);
        if !self.usual_known {
            self.usual = if self.base_odd { swap(size) } else { size };
            self.usual_known = true;
            self.retune(None);
            self.refit();
            if self.raw_size(page) == size {
                return true;
            }
        }
        let size = if self.odd(page) { swap(size) } else { size };
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
            if self.overrides.len() >= (self.cap + self.turned_other_way) as usize {
                // Forgotten, and asked for again as they're seen. Turned pages keep their shape, as that's from the edit, not from asking.
                self.known.clear();
                self.known.insert(page);
                self.retune(Some(vec![(page, if self.odd(page) { swap(size) } else { size })]));
                self.refit();
                return true;
            }
            let at = self.overrides.partition_point(|o| o.page < page);
            self.overrides.insert(
                at,
                Override {
                    page,
                    size,
                    row_extra: 0.0,
                    before: 0.0,
                },
            );
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
            Spread::TwoUp => !page.is_multiple_of(2),
            Spread::TwoUpCover => page.is_multiple_of(2),
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
            let tallest = self.overrides[at..end]
                .iter()
                .map(|o| o.size.height)
                .fold(if end - at < in_row { usual } else { f32::MIN }, f32::max);
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
        let room = (self.frame_width - 2.0 * self.side).max(1.0);
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

    pub fn document_height(&self) -> f64 {
        let rows = self.rows();
        if rows == 0 {
            return 0.0;
        }
        self.row_top(rows - 1) + self.row_height(rows - 1) + END_MARGIN
    }

    /// How far the view may scroll above the document's top and below its end, in document points: what makes the margin at each end [`END_MARGIN_SHARE`] of that row's height, less the [`END_MARGIN`] inside the document.
    pub fn end_room(&self) -> (f64, f64) {
        let rows = self.rows();
        if rows == 0 {
            return (0.0, 0.0);
        }
        let room = |row: u32| (self.row_height(row) * END_MARGIN_SHARE - END_MARGIN).max(0.0);
        (room(0), room(rows - 1))
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
            if x < rect.x {
                rect.x - x
            } else if x > rect.x + rect.width {
                x - rect.x - rect.width
            } else {
                0.0
            }
        };
        if distance(second) < distance(first) { second } else { first }
    }

    /// Which pages a stretch of the document touches.
    pub fn pages_between(&self, from: f64, to: f64) -> std::ops::RangeInclusive<u32> {
        let rows = self.rows();
        let first = self.row_at(from);
        let last = self.row_at(to);
        // A stretch that ends in the gap below a row needn't count the next one, and one that starts in a gap doesn't count the row above.
        let first = if first + 1 < rows && self.row_top(first) + self.row_height(first) < from {
            first + 1
        } else {
            first
        };
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

    /// Where a place is, to keep it across a change, on the page the reader's on, as the page view decides it. Side by side, that's the reader's own page of the row at this height if it reaches down this far, so a change of layout keeps their page; otherwise the row's tallest page, which reaches the whole row, so a short page beside a tall one can't hold the place to its own height.
    pub fn anchor_for(&self, y: f64, reader: u32) -> (u32, f64) {
        let (first, second) = self.row_pages(self.row_at(y));
        let reaches = |page: u32| {
            let rect = self.rect(page);
            y <= rect.y + rect.height
        };
        let page = match second {
            None => first,
            Some(_) if (reader == first || Some(reader) == second) && reaches(reader) => reader,
            Some(second) if self.rect(second).height > self.rect(first).height => second,
            Some(_) => first,
        };
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

    /// A place in the document on a page as the page is before it's turned, in its PDF points from its top left. Past the page's edges too, for a drag off its side.
    pub fn to_page(&self, page: u32, x: f64, y: f64) -> (f64, f64) {
        let rect = self.rect(page);
        let fit = self.fit.max(f64::MIN_POSITIVE);
        let (across, down) = ((x - rect.x) / fit, (y - rect.y) / fit);
        let raw = self.raw_size(page);
        let (width, height) = (f64::from(raw.width), f64::from(raw.height));
        match self.turns_of(page) {
            1 => (down, height - across),
            2 => (width - across, height - down),
            3 => (width - down, across),
            _ => (across, down),
        }
    }

    /// The other way: a place on a page, before it's turned, to the document.
    pub fn from_page(&self, page: u32, u: f64, v: f64) -> (f64, f64) {
        let rect = self.rect(page);
        let raw = self.raw_size(page);
        let (width, height) = (f64::from(raw.width), f64::from(raw.height));
        let (across, down) = match self.turns_of(page) {
            1 => (height - v, u),
            2 => (width - u, height - v),
            3 => (v, width - u),
            _ => (u, v),
        };
        (rect.x + across * self.fit, rect.y + down * self.fit)
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
            assert!(
                (layout.top(page) - expected_top).abs() < 1e-6,
                "page {page}: {} against {expected_top}",
                layout.top(page)
            );
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
        for (spread, rows) in [
            (
                Spread::TwoUp,
                (0..7u32).map(|row| (row * 2..(row * 2 + 2).min(13)).collect()).collect::<Vec<Vec<u32>>>(),
            ),
            (
                Spread::TwoUpCover,
                std::iter::once(vec![0]).chain((0..6u32).map(|row| vec![row * 2 + 1, row * 2 + 2])).collect(),
            ),
        ] {
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
                    assert_eq!(
                        layout.page_at_point(rect.x + rect.width / 2.0, rect.y + 1.0),
                        page,
                        "{spread:?} a click on page {page}"
                    );
                }
                if let [left, right] = pages[..] {
                    let (one, two) = (layout.rect(left), layout.rect(right));
                    assert!(
                        (one.x + one.width + PAGE_GAP - two.x).abs() < 1e-6,
                        "{spread:?} a pair meets with the gap between"
                    );
                    let middle = (one.x + one.width + two.x) / 2.0;
                    let clear = 1400.0 - CORNER_MARGIN - (two.x + two.width);
                    assert!(
                        (middle - 700.0).abs() < 1e-6 || clear.abs() < 1e-6,
                        "{spread:?} about the middle, or as near it as the corner allows: {middle}, {clear} clear"
                    );
                    assert_eq!(layout.pages_between(top + 1.0, top + 2.0), left..=right);
                }
            }
            let (last_top, last_height) = slow[slow.len() - 1];
            assert!((layout.document_height() - (last_top + last_height + END_MARGIN)).abs() < 1e-6, "{spread:?}");
        }
        let mut cover = PageLayout::with_spread(4, Spread::TwoUpCover);
        cover.set_frame(1400.0);
        cover.set_page_size(0, letter());
        assert!(
            (cover.rect(0).x - cover.rect(2).x).abs() < 1e-6 && cover.rect(0).x > cover.rect(1).x,
            "the cover's on the right, as a book's is"
        );
        let mut odd = PageLayout::with_spread(3, Spread::TwoUp);
        odd.set_frame(1400.0);
        odd.set_page_size(0, letter());
        assert!(
            (odd.rect(2).x - odd.rect(0).x).abs() < 1e-6 && odd.rect(2).x < odd.rect(1).x,
            "a last page alone is on the left"
        );
    }

    /// Failure: two pages side by side don't fit the room, or run into the agent icon's margin, or are drawn larger than one page would be.
    #[test]
    fn side_by_side_shares_the_room() {
        let mut layout = PageLayout::with_spread(10, Spread::TwoUp);
        layout.set_page_size(0, letter());
        layout.set_frame(1100.0);
        let (left, right) = (layout.rect(0), layout.rect(1));
        assert!(
            left.x >= MIN_SIDE_MARGIN - 1e-6 && right.x + right.width <= 1100.0 - MIN_SIDE_MARGIN + 1e-6,
            "{left:?} {right:?}"
        );
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

    /// Failure: pages turned by an edit keep the size they had, so a page turned a quarter is drawn squashed into its old shape, or the sizes said for a sleep file or a place are the turned ones and come back turned twice; or turning every page costs an entry per page. Held against working it out by hand, for every page turned, and for some.
    #[test]
    fn turned_pages_are_as_wide_as_they_were_tall() {
        let mut layout = PageLayout::new(6);
        layout.set_frame(1400.0);
        layout.set_page_size(0, letter());
        layout.set_page_size(4, PageBox { width: 612.0, height: 1008.0 });
        let before = layout.rect(2);
        assert!(layout.set_turns(Some(Arc::from(vec![1u8; 6]))), "every page a quarter");
        assert_eq!(layout.size(2), PageBox { width: 792.0, height: 612.0 });
        assert_eq!(layout.raw_size(2), letter(), "what's drawn is the page as it is");
        assert_eq!(layout.size(4), PageBox { width: 1008.0, height: 612.0 });
        assert_eq!(layout.usual_size(), Some(letter()), "a place keeps the size as the file has it");
        assert_eq!(layout.other_sizes(), vec![(4, PageBox { width: 612.0, height: 1008.0 })]);
        assert_eq!(layout.overrides.len(), 1, "turning every page costs nothing per page");
        assert!(
            layout.rect(2).width > before.width * 0.99 && layout.rect(2).height < before.height,
            "landscape now: {:?}",
            layout.rect(2)
        );
        // Some pages: those turned the other way from most are overrides, and finding a page still agrees with placing it.
        assert!(layout.set_turns(Some(Arc::from(vec![0u8, 1, 0, 2, 0, 3]))));
        assert_eq!(layout.size(1), PageBox { width: 792.0, height: 612.0 });
        assert_eq!(layout.size(3), letter(), "a half turn keeps its shape");
        let mut top = END_MARGIN;
        for page in 0..6 {
            assert!((layout.rect(page).y - top).abs() < 1e-6, "page {page}");
            assert_eq!(layout.page_at(top + 1.0), page);
            top += layout.rect(page).height + PAGE_GAP;
        }
        // A real size arriving for a turned page lands turned.
        layout.set_page_size(5, PageBox { width: 500.0, height: 700.0 });
        assert_eq!(layout.size(5), PageBox { width: 700.0, height: 500.0 });
        assert!(layout.set_turns(None));
        assert_eq!(layout.size(1), letter());
        assert_eq!(layout.size(5), PageBox { width: 500.0, height: 700.0 });
        assert!(!layout.set_turns(Some(Arc::from(vec![0u8; 6]))), "no turns is no change");
    }

    /// Failure: a click on a turned page lands on the wrong words, or a tile of it is drawn for the wrong part of the screen: the way from the document to a page and back doesn't agree, or a corner of the page isn't where the turned page's corner shows.
    #[test]
    fn a_turned_page_is_found_where_it_shows() {
        let mut layout = PageLayout::with_spread(4, Spread::TwoUp);
        layout.set_frame(1400.0);
        layout.set_page_size(0, letter());
        layout.set_turns(Some(Arc::from(vec![0u8, 1, 2, 3])));
        for page in 0..4 {
            let rect = layout.rect(page);
            for (u, v) in [(0.0, 0.0), (612.0, 0.0), (0.0, 792.0), (300.0, 100.0)] {
                let (x, y) = layout.from_page(page, u, v);
                assert!(
                    x >= rect.x - 1e-6 && x <= rect.x + rect.width + 1e-6 && y >= rect.y - 1e-6 && y <= rect.y + rect.height + 1e-6,
                    "page {page} ({u}, {v}) lands off it"
                );
                let (back_u, back_v) = layout.to_page(page, x, y);
                assert!((back_u - u).abs() < 1e-6 && (back_v - v).abs() < 1e-6, "page {page}");
            }
        }
        // A quarter turn clockwise puts the page's top left at the shown page's top right.
        let rect = layout.rect(1);
        let (x, y) = layout.from_page(1, 0.0, 0.0);
        assert!((x - (rect.x + rect.width)).abs() < 1e-6 && (y - rect.y).abs() < 1e-6);
    }

    /// Failure (review of f05c998, B3): side by side, a pair of a short page and a tall one loses the place on any change of the frame, as a resize or the sidebar's button makes: the place was measured against the row's first page, and held to its height, so 80% down the tall right-hand page came back hundreds of points up.
    #[test]
    fn a_tall_page_beside_a_short_one_keeps_the_place_through_a_resize() {
        let mut layout = PageLayout::with_spread(6, Spread::TwoUp);
        layout.set_frame(1400.0);
        layout.set_page_size(0, letter());
        layout.set_page_size(2, PageBox { width: 612.0, height: 300.0 });
        layout.set_page_size(3, PageBox { width: 612.0, height: 1400.0 });
        for reader in [2, 3] {
            let tall = layout.rect(3);
            let y = tall.y + tall.height * 0.8;
            let anchor = layout.anchor_for(y, reader);
            for frame in [1000.0, 1700.0, 1400.0] {
                layout.set_frame(frame);
                let tall = layout.rect(3);
                assert!(
                    (layout.anchor_y(anchor) - (tall.y + tall.height * 0.8)).abs() < 1e-6,
                    "reader on page {reader}, frame {frame}"
                );
            }
        }
        // On the reader's own page when it holds the place, so a change of layout keeps that page.
        let right = layout.rect(1);
        assert_eq!(layout.anchor_for(right.y + 10.0, 1).0, 1);
        assert_eq!(layout.anchor_for(right.y + 10.0, 0).0, 0);
    }

    /// Failure (review of f05c998, finding 7): past the caps on pages of another size, the overrides that give turned pages their swapped size were cleared or cut off with the rest, so pages turned the other way from most went back to their old shape.
    #[test]
    fn turned_pages_keep_their_shape_past_the_caps() {
        let count = 4_000u32;
        let mut layout = PageLayout::new(count);
        layout.cap = 1_000;
        layout.set_frame(1400.0);
        layout.set_page_size(0, letter());
        let turns: Vec<u8> = (0..count).map(|page| u8::from(page % 2 == 1)).collect();
        layout.set_turns(Some(Arc::from(turns)));
        let turned = PageBox { width: 792.0, height: 612.0 };
        assert_eq!(layout.size(count - 1), turned, "past the cap at the start");
        // Enough real sizes to reach the cap on pages of another size, which clears those.
        for page in (0..count).step_by(2).take(1_010) {
            layout.set_page_size(page, PageBox { width: 600.0, height: 800.0 });
        }
        assert_eq!(layout.size(count - 1), turned, "and after the cap clears the rest");
        assert_eq!(layout.size(1), turned);
    }

    /// Failure (second review): in a narrower window, a laptop's with the sidebar, the fitted page runs under the top right corner, so the zoom level and the agent icon sit on the page, where the owner's rule has nothing. Every layout keeps the corner's margin clear at any width, and is centred whenever there's room.
    #[test]
    fn pages_keep_clear_of_the_top_right_corner() {
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            for frame in [560.0, 740.0, 1000.0, 1180.0, 1440.0, 2200.0] {
                let mut layout = PageLayout::with_spread(6, spread);
                layout.set_frame(frame);
                layout.set_page_size(0, letter());
                for page in 0..6 {
                    let rect = layout.rect(page);
                    assert!(
                        rect.x + rect.width <= frame - CORNER_MARGIN + 1e-6,
                        "{spread:?} at {frame}: page {page} {rect:?}"
                    );
                    assert!(rect.x >= CORNER_MARGIN - 1e-6, "{spread:?} at {frame}: page {page} {rect:?}");
                }
                if spread == Spread::Single {
                    assert!(
                        (layout.rect(0).x + layout.rect(0).width / 2.0 - frame / 2.0).abs() < 1e-6,
                        "centred at every width: {frame}"
                    );
                }
            }
        }
    }

    /// A row's extent across, left edge and right: its pages, and a page alone in a two-up row with the column its pair would have, as it sits as if the pair were there.
    fn row_extent(layout: &PageLayout, page: u32) -> (f64, f64, bool) {
        let (one, two) = layout.row_pages(layout.row_of(page));
        let a = layout.rect(one);
        let usual = f64::from(layout.usual.width) * layout.fit();
        match two {
            Some(two) => {
                let b = layout.rect(two);
                (a.x.min(b.x), (a.x + a.width).max(b.x + b.width), true)
            }
            None if layout.spread() == Spread::Single => (a.x, a.x + a.width, true),
            None if layout.on_the_right(one) => (a.x - PAGE_GAP - usual, a.x + a.width, false),
            None => (a.x, a.x + a.width + PAGE_GAP + usual, false),
        }
    }

    /// Failure (owner, 2026-11-16): "when I zoom into the page only the right margin is reduced. Why? The page should always be centered (if there is no scrollbar)". Before, the layout kept its margins either side whatever the zoom, so a row zoomed wider than the room between them but narrower than the window started at the left margin and ran on to the right, and the document stayed the window's width at any zoom, so a zoomed page that fitted could still be scrolled off the middle. Over a sweep of zooms from below the room to past the window's width, in every layout, margin pair (the window's, a pane's), window width and page mix: a row no wider than the view is centred in it, with equal margins and no sideways scroll (the document's width for a view of that row is the view's); a row wider starts at the document's left edge and the view scrolls over exactly its width; and as the zoom moves, the row's left edge moves only as far as the view's width does, halved (a page alone in a two-up row, held by the margin, at most as far), so it never jumps as the row passes the room or the window. (It replaces the 2026-11-12 test that held every row centred at the fit and a row wider than the room at the margin, which this rule ends: seen failing on that placement.)
    #[test]
    fn every_row_is_centred_in_the_view_until_it_is_wider_at_any_zoom() {
        for (case, mut layout) in fitted_layouts() {
            let frame = layout.frame_width;
            let mut was: Option<(f64, Vec<(f64, bool)>)> = None;
            let mut magnification = 0.5;
            while magnification < 8.0 {
                layout.set_magnification(magnification);
                let view = layout.view_width();
                let mut lefts = Vec::new();
                for page in 0..layout.count() {
                    let (left, right, whole) = row_extent(&layout, page);
                    let rect = layout.rect(page);
                    let across = layout.document_width_for(rect.y, rect.y);
                    lefts.push((left, whole));
                    if right - left <= view + 1e-6 {
                        // A page alone in a two-up row keeps its column, centred as if its pair were there: so a page of the usual width is centred
                        // as a pair would be, and one of its own width is held inside the margins the row would have, the fit's or less.
                        if !whole && (rect.width - f64::from(layout.usual.width) * layout.fit()).abs() > 1e-6 {
                            let margin = layout.side.min(((view - (right - left)) / 2.0).max(0.0));
                            assert!(
                                left >= margin - 1e-6 && view - right >= margin - 1e-6,
                                "{case} at {magnification}: page {page} alone has margins {left} and {}, not inside {margin}",
                                view - right
                            );
                        } else {
                            assert!(
                                (left - (view - right)).abs() < 1e-6,
                                "{case} at {magnification}: page {page} has margins {left} and {} in a view {view} wide",
                                view - right
                            );
                        }
                        assert!(
                            across <= view + 1e-6,
                            "{case} at {magnification}: page {page} fits, and scrolls across {across} over {view}"
                        );
                    } else {
                        assert!(
                            left.abs() < 1e-6,
                            "{case} at {magnification}: page {page}'s row is wider than the view and starts at {left}"
                        );
                        assert!(
                            across >= rect.x + rect.width - 1e-6,
                            "{case} at {magnification}: page {page} runs past what scrolls across: {across}"
                        );
                        if whole {
                            assert!(
                                (across - right).abs() < 1e-6,
                                "{case} at {magnification}: page {page}'s row scrolls across {across}, not its width {right}"
                            );
                        }
                    }
                    if magnification == 1.0 && right - left <= frame - 2.0 * layout.side + 1e-6 {
                        assert!(left >= layout.side - 1e-6, "{case}: at the fit, page {page} keeps the margin: {left}");
                    }
                }
                if let Some((view_was, lefts_was)) = &was {
                    for (page, ((now, whole), (then, _))) in lefts.iter().zip(lefts_was).enumerate() {
                        // A centred row's edge moves half as far as the view's width; a page alone, held by a margin of the fit's, as far.
                        let rate = if *whole { 0.5 } else { 1.0 };
                        assert!(
                            (now - then).abs() <= (view - view_was).abs() * rate + 1e-6,
                            "{case} at {magnification}: page {page}'s row jumped from {then} to {now} as the view went from {view_was} to {view}"
                        );
                    }
                }
                was = Some((view, lefts));
                // Through 1.0 exactly, the fit.
                magnification = ((magnification * 1.05) * 1e6).round() / 1e6;
                if magnification > 0.97 && magnification < 1.03 {
                    magnification = 1.0;
                }
            }
        }
    }

    /// Every layout, margin pair, window width and page size mix the gutters test walks, laid out at the fit (no zoom): the layouts a pin is taken over.
    fn fitted_layouts() -> Vec<(String, PageLayout)> {
        let sizes: [(f32, f32); 6] = [(612.0, 792.0), (595.0, 842.0), (842.0, 595.0), (300.0, 400.0), (1190.0, 842.0), (432.0, 648.0)];
        let mut all = Vec::new();
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            for margins in [
                (MIN_SIDE_MARGIN, CORNER_MARGIN),
                (PANE_MARGIN, PANE_MARGIN),
                (PANE_MARGIN, CORNER_MARGIN),
                (0.0, 0.0),
            ] {
                for frame in [300.0, 480.0, 560.0, 740.0, 951.0, 1000.0, 1180.0, 1440.0, 2200.0, 3400.0] {
                    for first in 0..sizes.len() {
                        let mut layout = PageLayout::with_spread(12, spread);
                        layout.set_margins(margins.0, margins.1);
                        layout.set_frame(frame);
                        for page in 0..12u32 {
                            let (width, height) = sizes[(first + page as usize * 7) % sizes.len()];
                            layout.set_page_size(page, PageBox { width, height });
                        }
                        all.push((format!("{spread:?} {margins:?} {frame} {first}"), layout));
                    }
                }
            }
        }
        all
    }

    /// Failure: a change to where rows sit across moves a page at the fit, the view a document opens at and Fit goes back to, where the owner's rule keeps the margins as they were (owner, 2026-11-16: the margins kept at a fit are as they were). Pinned on 2026-11-16, before rows were centred in the zoomed view, at the numbers the layout gave then: every page's top, width and height, the document's height and fit, and the x of each page whose row fits the room between the margins, in every layout, margin pair, window width and page mix the gutters test walks. A row wider than that room is the new rule's business and isn't pinned. Held as a hash of the numbers' bits, so any change of a fitted page fails it: x to a billionth of a point, as a row exactly as wide as the room was held by bounds that crossed by a rounding error and stood an ulp (1e-13 points) off the middle, where it's now on it; checked against the old placement, which gave the same hash at that grain.
    #[test]
    fn a_fitted_page_is_where_it_was() {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |value: f64| {
            for byte in value.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        let mut pinned_x = 0;
        for (_, layout) in fitted_layouts() {
            feed(layout.fit());
            feed(layout.document_height());
            for page in 0..layout.count() {
                let rect = layout.rect(page);
                feed(rect.y);
                feed(rect.width);
                feed(rect.height);
                let (one, two) = layout.row_pages(layout.row_of(page));
                let (a, b) = (layout.rect(one), two.map_or(layout.rect(one), |p| layout.rect(p)));
                let usual = f64::from(layout.usual.width) * layout.fit();
                // A page alone in a two-up row sits in its column as if the usual page were beside it.
                let row = if layout.spread() != Spread::Single && two.is_none() {
                    a.width + PAGE_GAP + usual
                } else {
                    (a.x + a.width).max(b.x + b.width) - a.x.min(b.x)
                };
                if row <= layout.frame_width - 2.0 * layout.side + 1e-6 {
                    feed((rect.x * 1e9).round());
                    pinned_x += 1;
                }
            }
        }
        assert_eq!(pinned_x, 5_212, "the pages whose x is pinned");
        assert_eq!(hash, 2_155_042_483_219_912_870, "a page at the fit moved");
    }

    /// Failure (review of f05c998, finding 4): one very wide page, a fold-out, makes the document wide, and every other page or pair is centred on that and sits off to the side, out of the window, with the view at its left. Each row is centred in the window, and only the wide one runs past it, from the document's left edge (owner, 2026-11-16: a row wider than the window starts at its left edge).
    #[test]
    fn a_fold_out_doesnt_push_the_other_pages_aside() {
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            let mut layout = PageLayout::with_spread(8, spread);
            layout.set_frame(1400.0);
            layout.set_page_size(0, letter());
            layout.set_page_size(5, PageBox { width: 3000.0, height: 792.0 });
            for page in (0..8).filter(|page| !layout.same_row(*page, 5)) {
                let rect = layout.rect(page);
                assert!(
                    rect.x >= MIN_SIDE_MARGIN - 1e-6 && rect.x + rect.width <= 1400.0 - MIN_SIDE_MARGIN + 1e-6,
                    "{spread:?} page {page} {rect:?}"
                );
            }
            let wide = layout.rect(5);
            let (left, right, _) = row_extent(&layout, 5);
            assert!(
                left.abs() < 1e-6 && (layout.document_width_for(wide.y, wide.y) - right).abs() < 1e-6,
                "{spread:?} the fold-out's row, wider than the window, starts at the document's left edge and is all there is to scroll across: {wide:?}"
            );
        }
    }

    /// Failure: with a fold-out among ordinary pages, the ordinary rows move across as the fold-out comes into view and leaves it, or sit off the middle of the window, because they were centred in the document's width, which is the fold-out's only while its row is in view ([`PageLayout::document_width_for`]). Each ordinary row is centred in the view as the view is at no scroll across, wherever the view is, zoomed or not, in every layout; the fold-out's row starts at the document's left edge and is all there is to scroll across while it shows. So scrolling across to the fold-out's right takes the ordinary rows with it, out to the left, as a page wider than the others would.
    #[test]
    fn rows_beside_a_fold_out_stay_centred_in_the_view() {
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            for magnification in [1.0, 1.5, 2.5] {
                let mut layout = PageLayout::with_spread(8, spread);
                layout.set_frame(1400.0);
                layout.set_page_size(0, letter());
                layout.set_page_size(5, PageBox { width: 3000.0, height: 792.0 });
                layout.set_magnification(magnification);
                let view = layout.view_width();
                let wide = layout.rect(5);
                let (_, right_edge, _) = row_extent(&layout, 5);
                let both = layout.document_width_for(0.0, wide.y + wide.height);
                assert!(
                    (both - right_edge).abs() < 1e-6,
                    "{spread:?} at {magnification}: {both} across, the fold-out's row ends at {right_edge}"
                );
                for page in (0..8).filter(|page| !layout.same_row(*page, 5)) {
                    let (left, right, whole) = row_extent(&layout, page);
                    if !whole || right - left > view {
                        continue;
                    }
                    assert!(
                        (left - (view - right)).abs() < 1e-6,
                        "{spread:?} at {magnification}: page {page} has margins {left} and {} in the view, with the fold-out's row in view or not",
                        view - right
                    );
                }
                assert!(
                    row_extent(&layout, 5).0.abs() < 1e-6,
                    "{spread:?} at {magnification}: the fold-out's row starts at the left edge"
                );
            }
        }
    }

    /// Failure (PLAN: "a page turned wider than the view keeps its left margin and runs off the right edge"): pages turned sideways by rotate-all, wider than the fitted room but no wider than the window, start at the left margin and run on to the right; or a turned page zoomed in isn't centred. A turned row is a row like any other: centred while it fits the view, with equal margins, and from the left edge once it's wider.
    #[test]
    fn a_turned_page_no_wider_than_the_window_is_centred() {
        let mut layout = PageLayout::new(4);
        layout.set_frame(1100.0);
        layout.set_page_size(0, letter());
        // Every other page turned: the fit stays the upright pages', and a turned one is as wide as an upright one is tall.
        layout.set_turns(Some(Arc::from(vec![0u8, 1, 0, 1])));
        let fitted = layout.rect(1);
        assert!(
            fitted.width > 1100.0 - 2.0 * layout.side && fitted.width < 1100.0,
            "a letter page turned is wider than the room and narrower than the window: {fitted:?}"
        );
        for magnification in [1.0, 1.05, 1.1, 1.3] {
            layout.set_magnification(magnification);
            let (rect, view) = (layout.rect(1), layout.view_width());
            if rect.width <= view {
                assert!(
                    (rect.x - (view - rect.x - rect.width)).abs() < 1e-6,
                    "at {magnification}: {rect:?} in a view {view} wide"
                );
            } else {
                assert!(rect.x.abs() < 1e-6, "at {magnification}: {rect:?}");
            }
        }
    }

    /// Failure (PLAN, side-by-side follow-ups): one fold-out makes the document as wide as it for every row, so beside every ordinary page or pair there's empty room to scroll across into. The width for a view is the window's, and the fold-out's only where the view reaches its row, so all of it can still be scrolled to.
    #[test]
    fn only_a_view_on_the_fold_out_is_wider_than_the_window() {
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            let mut layout = PageLayout::with_spread(40, spread);
            layout.set_frame(1400.0);
            layout.set_page_size(0, letter());
            layout.set_page_size(21, PageBox { width: 3000.0, height: 792.0 });
            let wide = layout.rect(21);
            let away = layout.rect(2);
            assert_eq!(
                layout.document_width_for(away.y, away.y + 600.0),
                1400.0,
                "{spread:?}: no room beside ordinary rows"
            );
            let over = layout.document_width_for(wide.y - 100.0, wide.y + 100.0);
            assert!(
                over >= wide.x + wide.width && over <= layout.document_width() + 1e-6,
                "{spread:?}: {over} for {wide:?}"
            );
            let last = layout.rect(39);
            assert_eq!(
                layout.document_width_for(0.0, last.y + last.height),
                layout.document_width_for(wide.y, wide.y),
                "{spread:?}: a view of everything reaches it"
            );
        }
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
        const { assert!(END_MARGIN > PAGE_GAP, "it reads as the document's end, not another gap") };
    }

    /// Failure: the margin above the first page or below the last isn't half that row's height, at any width or layout, with pages of other sizes at the ends, so the first page can't be brought down to the middle of the window or the last one up to it; or the room is taken inside the document, which would move every page and where a document opens. Owner, 2026-11-08: "The margin above first page, and below last page should be 50% of page height".
    #[test]
    fn the_margin_at_each_end_is_half_a_page() {
        for spread in [Spread::Single, Spread::TwoUp, Spread::TwoUpCover] {
            for frame in [700.0, 1200.0, 2400.0] {
                let mut layout = PageLayout::with_spread(5, spread);
                layout.set_frame(frame);
                layout.set_page_size(0, letter());
                layout.set_page_size(4, PageBox { width: 612.0, height: 400.0 });
                let (above, below) = layout.end_room();
                let first = layout.rect(0);
                assert!((first.y - END_MARGIN).abs() < 1e-9, "pages stay where they were: {spread:?} at {frame}");
                assert!(
                    (above + first.y - first.height * END_MARGIN_SHARE).abs() < 1e-9,
                    "{spread:?} at {frame}: {above} above"
                );
                let last_row = (layout.first_of_row(layout.rows() - 1)..5).map(|page| layout.rect(page)).collect::<Vec<_>>();
                let row_height = last_row.iter().map(|rect| rect.height).fold(0.0, f64::max);
                let under = layout.document_height() - (last_row[0].y + row_height);
                assert!(
                    (below + under - row_height * END_MARGIN_SHARE).abs() < 1e-9,
                    "{spread:?} at {frame}: {below} below"
                );
            }
        }
        assert_eq!(PageLayout::new(0).end_room(), (0.0, 0.0), "no pages, no room");
        let mut tiny = PageLayout::new(1);
        tiny.set_frame(1200.0);
        tiny.set_page_size(0, PageBox { width: 20.0, height: 20.0 });
        let (above, below) = tiny.end_room();
        assert!(
            above >= 0.0 && below >= 0.0,
            "a page shorter than twice the margin has none extra, never less than none"
        );
    }

    /// Failure: a comparison's pane keeps the whole window's margins, the agent icon's 96 points among them, so three panes in a laptop's window leave pages a thumbnail wide; or a pane's page runs under the pane beside it.
    #[test]
    fn a_pane_has_narrower_margins_and_keeps_inside_them() {
        let mut layout = PageLayout::new(4);
        layout.set_page_size(0, letter());
        layout.set_frame(360.0);
        let whole = layout.rect(0).width;
        assert!(layout.set_margins(PANE_MARGIN, PANE_MARGIN));
        assert!(!layout.set_margins(PANE_MARGIN, PANE_MARGIN), "the same again moves nothing");
        let pane = layout.rect(0);
        assert!(pane.width > whole + 100.0, "{} against {whole}", pane.width);
        assert!(pane.x >= PANE_MARGIN - 1e-6 && pane.x + pane.width <= 360.0 - PANE_MARGIN + 1e-6, "{pane:?}");
        assert!(layout.set_margins(PANE_MARGIN, CORNER_MARGIN), "the pane at the right keeps the corner");
        let right = layout.rect(0);
        assert!(right.x + right.width <= 360.0 - CORNER_MARGIN + 1e-6, "{right:?}");
        layout.set_margins(f64::NAN, 1e9);
        assert!(layout.fit().is_finite() && layout.rect(0).width > 0.0, "nonsense is held to sense");
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
        assert!(!layout.set_page_size(
            0,
            PageBox {
                width: f32::NAN,
                height: 100.0
            }
        ));
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
