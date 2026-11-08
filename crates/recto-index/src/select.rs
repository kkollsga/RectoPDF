//! Selecting text: where a page's words are as the reader sees them, which one a point is on, what a range covers and what it copies. Pure functions and one small state machine, with no toolkit, thread or renderer, so `recto select`, `recto copy` and the app all do the same thing and a hostile page can be fuzzed directly.
//!
//! **What a place is.** A place on a page is a character of its text as [`crate::read_page`] gives it, so the offsets here are `recto text`'s, and what's copied is what an agent reads: late-drawn words back on their lines, line-end hyphens decided, invisible characters gone.
//!
//! **Columns.** A range is a stretch of reading order, so a drag from one column into the next takes the rest of the first and the top of the second, as a Mac reader does. Reading order is the file's, with line-order repair. A full layout sort isn't done here for the reason it isn't done in [`crate::order`]: drawing order was right on all of the 84 two-column pages of the spike's sample that had body text (`docs/research/2026-11-06-renderer-spike.md`, §10.2), and tables are what a sort breaks. A file that draws its right column first selects that way, and `recto text` reads it that way too.
//!
//! **What's left out.** Text that looks hidden, by the same check as `recto text`, and printed characters wholly off the page: neither can be seen, so neither is hit, highlighted or copied, and [`CopiedText::hidden_left_out`] says how many went. Text with no box can't be hit either, but it stays in a copy, since a font with empty boxes doesn't make its text hidden.
//!
//! **Cost.** A page held here is about 20 B a character: over bench100's pages with text, 55 KiB at the middle, and 242 at most. Past [`MAX_SELECTABLE_CHARS`] a page selects nothing. A hit looks along the page's lines, at most [`MAX_LINES`], and then one line. The model holds two points, so a selection of a whole document costs what one word's does: nothing per page.

use crate::chrome::{self, EdgeLine};
use crate::{hidden_in_either_order, normalize, order};
use recto_proto::{CharBox, CharRecord, PageFrame};

/// The longest page that can be selected. The busiest page in bench100 has 11,093 characters.
pub const MAX_SELECTABLE_CHARS: usize = 128 * 1024;
/// Lines kept for a page. Past this the rest are one line: a real page has a few hundred.
pub const MAX_LINES: usize = 16_384;
/// Rectangles given for one page.
pub const MAX_RECTS_PER_PAGE: usize = 4096;
/// The most one copy holds, past which it stops and says where. About 5,000 pages of text.
pub const MAX_COPY_BYTES: usize = 16 * 1024 * 1024;
/// The most pages one copy goes through. A file can claim millions, with nothing on them.
pub const MAX_COPY_PAGES: u32 = 100_000;
/// How long one copy goes on for.
pub const MAX_COPY_TIME: std::time::Duration = std::time::Duration::from_secs(120);
/// In a [`Place`]: the end of the page, whatever its length, for a page whose text hasn't been read.
pub const PAGE_END: u32 = u32::MAX;

/// A gap this many line heights wide inside a line is a gutter, and a highlight doesn't bridge it.
const GUTTER: f32 = 3.0;
/// How far a line's band reaches above its baseline and below it, in heights of its tallest glyphs, which is about a capital's: together about a font's size, so a highlight looks the same with or without ascenders on the line.
const ABOVE_BASELINE: f32 = 1.05;
const BELOW_BASELINE: f32 = 0.30;
/// A line this much shorter than the page's usual one has only small letters on it, and gets the usual band. Any shorter and it's small print, with its own.
const SAME_SIZE: f32 = 0.6;
/// How much wider a line running down the page is than its glyphs, each side.
const SIDEWAYS_MARGIN: f32 = 0.15;
/// Characters per line used to find its baseline.
const SAMPLE: usize = 32;
/// How far the I-beam reaches past either end of a line, in points.
const POINTER_REACH: f32 = 2.0;
/// The furthest a word reaches either way from where it's clicked.
const MAX_WORD: usize = 1024;
/// How much further than across a stretch has to run to count as going up or down the page, which keeps a line with a hyphen joined at its end from reading that way.
const SIDEWAYS: f32 = 1.5;
/// A step back along a line of more than this, in heights, with a drop, is a new line, as when a line-end hyphen was joined away and left no break behind.
const STEP_BACK: f32 = 0.5;
const SAME_LINE: f32 = 0.9;
/// A jump this many heights off a line is off the line, going forward or back.
const OFF_THE_LINE: f32 = 2.5;

/// A rectangle on a page as it's shown: points from its top left, y going down. Scrolling and zooming can't change it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// How much a click takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    #[default]
    Char,
    Word,
    Line,
}

impl Unit {
    /// One click places, two take a word, and three or more a line.
    pub fn from_clicks(clicks: u32) -> Self {
        match clicks {
            0 | 1 => Self::Char,
            2 => Self::Word,
            _ => Self::Line,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Char => "char",
            Self::Word => "word",
            Self::Line => "line",
        }
    }
}

/// A line of a page, with its band as it's shown. Held per line of a page in hand, at most [`MAX_LINES`], asserted at 32 B or less in a test.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Line {
    start: u32,
    end: u32,
    /// `x1 < x0` when nothing on it can be seen.
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    /// It runs down or up the page as it's shown.
    sideways: bool,
    /// It runs right to left, or up.
    backward: bool,
}

impl Line {
    fn seen(&self) -> bool {
        self.x1 >= self.x0
    }

    /// How far a point is from it, squared.
    fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x0 - x).max(x - self.x1).max(0.0);
        let dy = (self.y0 - y).max(y - self.y1).max(0.0);
        dx * dx + dy * dy
    }
}

/// A page's text in reading order, each character with its box as it's shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    text: Vec<char>,
    /// One per character, `[x0, y0, x1, y1]`, with `y1 <= y0` for one with no box.
    boxes: Vec<[f32; 4]>,
    /// One per character if any is hidden, or nothing.
    hidden: Vec<bool>,
    lines: Vec<Line>,
    width: f32,
    height: f32,
    hidden_checked: bool,
    copy_restricted: bool,
    too_long: bool,
    /// The lines at its top and bottom edges that may be chrome, at most [`chrome::MAX_EDGE_LINES`]. See [`crate::chrome`].
    edges: Vec<EdgeLine>,
}

const NO_BOX: [f32; 4] = [0.0; 4];

fn has_box(view: &[f32; 4]) -> bool {
    view[3] > view[1]
}

/// A box in the page's coordinates, as it's shown: past the crop, turned, y going down. See `PageFrame`.
fn to_view(frame: PageFrame, char_box: &CharBox) -> [f32; 4] {
    let (u0, u1) = (char_box[0] - frame.left, char_box[2] - frame.left);
    let (v0, v1) = (char_box[1] - frame.bottom, char_box[3] - frame.bottom);
    let (width, height) = (frame.right - frame.left, frame.top - frame.bottom);
    let view = match frame.turns & 3 {
        0 => [u0, height - v1, u1, height - v0],
        1 => [v0, u0, v1, u1],
        2 => [width - u1, v0, width - u0, v1],
        _ => [height - v1, width - u1, height - v0, width - u0],
    };
    if view.iter().all(|edge| edge.is_finite()) && view[2] >= view[0] && view[3] > view[1] { view } else { NO_BOX }
}

/// A page's size as it's shown, and nothing if the frame says it has none.
fn frame_size(frame: PageFrame) -> Option<(f32, f32)> {
    let (width, height) = (frame.right - frame.left, frame.top - frame.bottom);
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(if frame.turns & 1 == 1 { (height, width) } else { (width, height) })
}

impl PageText {
    /// Reads a page's characters. `boxes` is what the worker gave, one per character or none, and nothing about it is trusted: a page with no usable boxes has text that can be copied in a whole-page selection, and nothing that can be hit.
    pub fn build(chars: &[CharRecord], boxes: &[CharBox], frame: PageFrame, hidden_checked: bool, copy_restricted: bool) -> Self {
        if chars.len() > MAX_SELECTABLE_CHARS {
            return Self { too_long: true, hidden_checked, copy_restricted, ..Self::default() };
        }
        let repaired = order::repair_line_order(chars, boxes);
        let reading = repaired.as_ref().map_or(chars, |repaired| &repaired.chars);
        let normalized = normalize(reading, true);
        let flagged = hidden_in_either_order(chars, repaired.as_ref());
        let original = |at: u32| repaired.as_ref().map_or(Some(at), |repaired| repaired.origin.get(at as usize).copied().filter(|from| *from != order::GENERATED));
        let (width, height) = frame_size(frame).unwrap_or((0.0, 0.0));
        let known_size = width > 0.0;
        let text: Vec<char> = normalized.text.chars().collect();
        let mut views = Vec::with_capacity(text.len());
        let mut hidden: Vec<bool> = Vec::new();
        for (at, (ch, &source)) in text.iter().zip(&normalized.source).enumerate() {
            let from = original(source);
            let view = match (known_size, from.and_then(|from| boxes.get(from as usize))) {
                (true, Some(char_box)) => to_view(frame, char_box),
                _ => NO_BOX,
            };
            let off_page = has_box(&view) && !ch.is_whitespace() && (view[2] <= 0.0 || view[0] >= width || view[3] <= 0.0 || view[1] >= height);
            let flagged_here = from.and_then(|from| flagged.get(from as usize)).copied().unwrap_or(false);
            if off_page || flagged_here {
                if hidden.is_empty() {
                    hidden = vec![false; text.len()];
                }
                hidden[at] = true;
            }
            views.push(view);
        }
        let mut page = Self { text, boxes: views, hidden, lines: Vec::new(), width, height, hidden_checked, copy_restricted, too_long: false, edges: Vec::new() };
        page.find_lines(frame.turns & 3 == 2);
        page.find_edges();
        page
    }

    fn is_hidden(&self, at: usize) -> bool {
        self.hidden.get(at).copied().unwrap_or(false)
    }

    /// Whether this character shows, so it can be hit and highlighted.
    fn shows(&self, at: usize) -> bool {
        !self.is_hidden(at) && self.boxes.get(at).is_some_and(has_box)
    }

    /// Cuts the text into lines as they're shown. A line break is one, and so is a place the characters leave a line with no break, which is what a joined hyphen leaves.
    fn find_lines(&mut self, upside_down: bool) {
        let mut spans: Vec<(u32, u32)> = Vec::new();
        let mut start = 0;
        for at in 0..=self.text.len() {
            let ends_a_line = at == self.text.len() || (self.text[at] == '\n' && spans.len() + 1 < MAX_LINES);
            if ends_a_line {
                self.split_by_shape(start, at, &mut spans);
                start = at + 1;
            }
        }
        // A real page's lines are alike, and the middle one is what "the usual line" means.
        let tallest = |(from, to): (u32, u32), page: &Self| -> f32 { (from..to).filter(|at| page.shows(*at as usize)).map(|at| page.boxes[at as usize]).map(|view| view[3] - view[1]).fold(0.0, f32::max) };
        let mut heights: Vec<f32> = spans.iter().map(|span| tallest(*span, self)).filter(|height| *height > 0.0).collect();
        heights.sort_by(f32::total_cmp);
        let usual = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
        let lines: Vec<Line> = spans.iter().filter(|span| span.0 < span.1).map(|span| self.measure(*span, usual, upside_down)).collect();
        self.lines = lines;
    }

    /// Finds the pieces of lines at the page's top and bottom edges that may be chrome: see [`crate::chrome`]. A line is cut into pieces where its letters leave a gap of [`chrome::PIECE_GAP`] sizes, so a page number set at the end of a footnote's line is a piece of its own. The pieces in the rows of a block at either edge, a few rows at most with a wide gap between them and the rest, are candidates; so is any piece in the edge bands that's a page number, set apart or not, as it can be confirmed by its neighbours' numbers. A look along the page's lines, and a sort, once.
    fn find_edges(&mut self) {
        if self.height <= 0.0 {
            return;
        }
        // Each piece of a line that shows and runs across: its characters, where its letters' tops and baseline are, and their size.
        struct Piece {
            start: u32,
            end: u32,
            x0: f32,
            top: f32,
            bottom: f32,
            baseline: f32,
            size: f32,
        }
        let mut pieces: Vec<Piece> = Vec::new();
        for line in &self.lines {
            if !line.seen() || line.sideways {
                continue;
            }
            let mut current: Option<(Piece, [f32; SAMPLE], usize, f32)> = None;
            let finish = |current: Option<(Piece, [f32; SAMPLE], usize, f32)>, end: u32, pieces: &mut Vec<Piece>| {
                if let Some((mut piece, mut bottoms, sampled, _)) = current {
                    let bottoms = &mut bottoms[..sampled];
                    bottoms.sort_by(f32::total_cmp);
                    piece.baseline = bottoms[sampled / 2];
                    piece.end = end;
                    pieces.push(piece);
                }
            };
            for at in line.start..line.end {
                let index = at as usize;
                if !self.shows(index) || self.text[index].is_whitespace() {
                    continue;
                }
                let view = self.boxes[index];
                let size = view[3] - view[1];
                // A wide gap after the last letter, going forward along the line, starts a new piece.
                let new_piece = current.as_ref().is_some_and(|(piece, _, _, right)| !line.backward && view[0] - right > chrome::PIECE_GAP * piece.size.max(size).max(1.0));
                if new_piece {
                    finish(current.take(), at, &mut pieces);
                }
                let (piece, bottoms, sampled, right) = current.get_or_insert_with(|| (Piece { start: at, end: at, x0: view[0], top: view[1], bottom: view[3], baseline: 0.0, size: 0.0 }, [0f32; SAMPLE], 0, view[2]));
                (piece.top, piece.bottom, piece.size, piece.x0) = (piece.top.min(view[1]), piece.bottom.max(view[3]), piece.size.max(size), piece.x0.min(view[0]));
                *right = right.max(view[2]);
                if *sampled < SAMPLE {
                    bottoms[*sampled] = view[3];
                    *sampled += 1;
                }
            }
            finish(current, line.end, &mut pieces);
        }
        if pieces.len() < 2 {
            return;
        }
        pieces.sort_by(|one, two| one.baseline.total_cmp(&two.baseline));
        // Rows: pieces whose baselines are within a third of their size, as a running head's left and right parts are.
        let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
        let mut from = 0;
        for at in 1..=pieces.len() {
            if at == pieces.len() || pieces[at].baseline - pieces[from].baseline > pieces[from].size.max(pieces[at].size).max(1.0) / 3.0 {
                rows.push(from..at);
                from = at;
            }
        }
        let usual = {
            let mut sizes: Vec<f32> = pieces.iter().map(|piece| piece.size).collect();
            sizes.sort_by(f32::total_cmp);
            sizes[sizes.len() / 2].max(1.0)
        };
        let band = self.height * chrome::BAND;
        let row_size = |row: &std::ops::Range<usize>| pieces[row.clone()].iter().map(|piece| piece.size).fold(0f32, f32::max).max(1.0);
        let row_baseline = |row: &std::ops::Range<usize>| pieces[row.start].baseline;
        let in_band = |piece: &Piece, top: bool| if top { piece.bottom <= band } else { piece.top >= self.height - band };
        let mut edges: Vec<EdgeLine> = Vec::new();
        let mut taken: Vec<u32> = Vec::new();
        for top in [true, false] {
            let order: Vec<&std::ops::Range<usize>> = if top { rows.iter().collect() } else { rows.iter().rev().collect() };
            // The block from the edge: rows at line spacing from each other, then a wide gap.
            let mut block = 0;
            let mut isolated = false;
            while block < order.len() {
                block += 1;
                let Some(next) = order.get(block) else { break };
                let gap = (row_baseline(next) - row_baseline(order[block - 1])).abs();
                if gap >= chrome::MIN_GAP * row_size(order[block - 1]).max(row_size(next)).max(6.0) {
                    isolated = true;
                    break;
                }
                if block >= chrome::MAX_BLOCK_ROWS {
                    break;
                }
            }
            let block_rows: &[&std::ops::Range<usize>] = if isolated && order[..block].iter().all(|row| pieces[(*row).clone()].iter().all(|piece| in_band(piece, top))) { &order[..block] } else { &[] };
            for row in block_rows {
                let mut members: Vec<&Piece> = pieces[(*row).clone()].iter().collect();
                members.sort_by(|one, two| one.x0.total_cmp(&two.x0));
                let row_key = chrome::key(members.iter().flat_map(|piece| self.visible_chars(piece.start, piece.end).chain(std::iter::once(' '))));
                for piece in members {
                    if edges.len() < chrome::MAX_EDGE_LINES {
                        edges.push(self.edge_line(top, piece.start, piece.end, if top { piece.top } else { self.height - piece.bottom }, piece.size, usual, row_key, true));
                        taken.push(piece.start);
                    }
                }
            }
            // A page number anywhere in the band, set apart or not: only its neighbours' numbers can make it chrome.
            for row in &order {
                for piece in &pieces[(*row).clone()] {
                    if edges.len() >= chrome::MAX_EDGE_LINES || !in_band(piece, top) || taken.contains(&piece.start) {
                        continue;
                    }
                    let text: String = self.visible_chars(piece.start, piece.end).collect();
                    if chrome::page_number(&text).is_some() {
                        let key = chrome::key(text.chars());
                        edges.push(self.edge_line(top, piece.start, piece.end, if top { piece.top } else { self.height - piece.bottom }, piece.size, usual, key, false));
                        taken.push(piece.start);
                    }
                }
            }
        }
        self.edges = edges;
    }

    /// One candidate, as a copy compares it.
    #[allow(clippy::too_many_arguments)]
    fn edge_line(&self, top: bool, start: u32, end: u32, from_edge: f32, size: f32, usual: f32, row_key: u64, isolated: bool) -> EdgeLine {
        let text: String = self.visible_chars(start, end).collect();
        let (left, right) = (start..end).filter(|at| self.shows(*at as usize)).map(|at| self.boxes[at as usize]).fold((f32::INFINITY, f32::NEG_INFINITY), |(left, right), view| (left.min(view[0]), right.max(view[2])));
        let middle = if left <= right { (left + right) / 2.0 } else { 0.0 };
        EdgeLine {
            top,
            from_edge,
            size,
            key: chrome::key(text.chars()),
            row_key,
            start,
            end,
            number: chrome::page_number(&text),
            relative: size / usual,
            words: text.chars().any(char::is_alphanumeric),
            digits_only: text.chars().any(|ch| ch.is_ascii_digit()) && !text.chars().any(char::is_alphabetic),
            isolated,
            dated: !text.trim().contains(char::is_whitespace) && text.chars().any(|ch| matches!(ch, '/' | '.' | ':' | '-')) && text.chars().filter(|ch| ch.is_ascii_digit()).count() >= 2,
            substantial: text.split_whitespace().count() >= 2 && text.chars().filter(|ch| ch.is_alphabetic()).count() >= 10,
            from_left: middle,
            from_right: self.width - middle,
        }
    }

    /// The characters of `[start, end)` that aren't hidden.
    fn visible_chars(&self, start: u32, end: u32) -> impl Iterator<Item = char> + '_ {
        (start as usize..(end as usize).min(self.text.len())).filter(|at| !self.is_hidden(*at)).map(|at| self.text[at])
    }

    /// The lines at its edges that may be chrome, for its neighbours to be compared with.
    pub fn edge_lines(&self) -> &[EdgeLine] {
        &self.edges
    }

    /// What a copy of this page leaves out as chrome, given its neighbours' edge lines with how many pages away each is. See [`crate::chrome`].
    pub fn chrome_spans(&self, neighbours: &[(i64, &[EdgeLine])]) -> Vec<(u32, u32)> {
        chrome::chrome_spans(&self.edges, neighbours)
    }

    /// Whether this stretch runs up or down the page as it's shown: further that way than across, between the middles of its characters.
    fn runs_sideways(&self, start: usize, end: usize) -> bool {
        let (mut low_x, mut high_x, mut low_y, mut high_y) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for at in start..end {
            if self.shows(at) {
                let view = &self.boxes[at];
                let (x, y) = ((view[0] + view[2]) / 2.0, (view[1] + view[3]) / 2.0);
                (low_x, high_x, low_y, high_y) = (low_x.min(x), high_x.max(x), low_y.min(y), high_y.max(y));
            }
        }
        high_y - low_y > (high_x - low_x).max(0.0) * SIDEWAYS
    }

    /// One stretch between line breaks, as one line or more.
    fn split_by_shape(&self, start: usize, end: usize, spans: &mut Vec<(u32, u32)>) {
        // A line in the middle of what's left past the cap would pass it.
        let room = MAX_LINES.saturating_sub(1);
        let sideways = self.runs_sideways(start, end);
        let (mut from, mut last) = (start, None);
        for at in start..end {
            // A space's box says little, and a line can't be left on one.
            if !self.shows(at) || self.text[at].is_whitespace() {
                continue;
            }
            if let Some(before) = last
                && spans.len() < room
                && Self::leaves_the_line(&self.boxes[before], &self.boxes[at], sideways)
            {
                spans.push((from as u32, at as u32));
                from = at;
            }
            last = Some(at);
        }
        spans.push((from as u32, end as u32));
    }

    fn leaves_the_line(before: &[f32; 4], here: &[f32; 4], sideways: bool) -> bool {
        if sideways {
            // Glyphs are as thick as a line here, whatever their width, so this is how far they've stepped across.
            let thick = (before[2] - before[0]).max(here[2] - here[0]);
            return (here[0] - before[0]).abs() > OFF_THE_LINE * thick;
        }
        let height = (before[3] - before[1]).max(here[3] - here[1]);
        // Letters share a baseline at one edge of their boxes, which is the other edge on a page turned upside down, and letters on another line share neither.
        let off = (here[3] - before[3]).abs().min((here[1] - before[1]).abs());
        (here[0] < before[0] - STEP_BACK * height && off > SAME_LINE * height) || (off > OFF_THE_LINE * height && here[0] < before[2])
    }

    fn measure(&self, (start, end): (u32, u32), usual: f32, upside_down: bool) -> Line {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let (mut first, mut last): (Option<[f32; 4]>, Option<[f32; 4]>) = (None, None);
        let mut edges = [0f32; SAMPLE];
        let mut sampled = 0;
        let mut tallest = 0f32;
        for at in start..end {
            if !self.shows(at as usize) {
                continue;
            }
            let view = self.boxes[at as usize];
            x0 = x0.min(view[0]);
            y0 = y0.min(view[1]);
            x1 = x1.max(view[2]);
            y1 = y1.max(view[3]);
            tallest = tallest.max(view[3] - view[1]);
            first.get_or_insert(view);
            last = Some(view);
            if sampled < SAMPLE {
                edges[sampled] = if upside_down { view[1] } else { view[3] };
                sampled += 1;
            }
        }
        let (Some(first), Some(last)) = (first, last) else {
            return Line { start, end, x0: 0.0, y0: 0.0, x1: -1.0, y1: -1.0, sideways: false, backward: false };
        };
        let (dx, dy) = ((last[0] + last[2] - first[0] - first[2]) / 2.0, (last[1] + last[3] - first[1] - first[3]) / 2.0);
        let sideways = self.runs_sideways(start as usize, end as usize);
        let backward = if sideways { dy < 0.0 } else { dx < 0.0 };
        if sideways {
            let margin = (x1 - x0) * SIDEWAYS_MARGIN;
            return Line { start, end, x0: x0 - margin, y0, x1: x1 + margin, y1, sideways, backward };
        }
        let edges = &mut edges[..sampled];
        edges.sort_by(f32::total_cmp);
        let baseline = edges[edges.len() / 2];
        let size = if tallest < usual && tallest >= usual * SAME_SIZE { usual } else { tallest };
        let (above, below) = if upside_down { (BELOW_BASELINE, ABOVE_BASELINE) } else { (ABOVE_BASELINE, BELOW_BASELINE) };
        Line { start, end, x0, y0: y0.min(baseline - size * above), x1, y1: y1.max(baseline + size * below), sideways, backward }
    }

    /// How many lines it's cut into.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// How many characters there are, which is where [`PAGE_END`] is.
    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The page's size as it's shown. Zero if nothing about it could be read.
    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    pub fn hidden_checked(&self) -> bool {
        self.hidden_checked
    }

    /// The file's permissions ask that its text isn't copied.
    pub fn copy_restricted(&self) -> bool {
        self.copy_restricted
    }

    /// It has more characters than can be selected.
    pub fn too_long(&self) -> bool {
        self.too_long
    }

    /// About what this holds.
    pub fn bytes(&self) -> usize {
        size_of::<Self>() + self.text.capacity() * size_of::<char>() + self.boxes.capacity() * size_of::<[f32; 4]>() + self.hidden.capacity() + self.lines.capacity() * size_of::<Line>() + self.edges.capacity() * size_of::<EdgeLine>()
    }

    fn line_at(&self, x: f32, y: f32) -> Option<&Line> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.lines.iter().filter(|line| line.seen()).min_by(|one, two| one.distance(x, y).total_cmp(&two.distance(x, y)))
    }

    /// Whether the point is on a line of text that shows, which is where the I-beam goes.
    pub fn is_over_text(&self, x: f32, y: f32) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        self.lines.iter().any(|line| line.seen() && x >= line.x0 - POINTER_REACH && x <= line.x1 + POINTER_REACH && y >= line.y0 && y <= line.y1)
    }

    /// The character nearest a point on the nearest line, and the place it comes to: before that character or after it, by which half the point is in, however the line runs.
    fn hit(&self, x: f32, y: f32) -> Option<(u32, u32, Line)> {
        let line = *self.line_at(x, y)?;
        let along = if line.sideways { y } else { x };
        let mut best: Option<(f32, u32)> = None;
        for at in line.start..line.end {
            if !self.shows(at as usize) {
                continue;
            }
            let view = &self.boxes[at as usize];
            let (from, to) = if line.sideways { (view[1], view[3]) } else { (view[0], view[2]) };
            let distance = (from - along).max(along - to).max(0.0);
            if best.is_none_or(|known| distance < known.0) {
                best = Some((distance, at));
            }
        }
        let (_, at) = best?;
        let view = &self.boxes[at as usize];
        let middle = if line.sideways { (view[1] + view[3]) / 2.0 } else { (view[0] + view[2]) / 2.0 };
        let past = if line.backward { along < middle } else { along > middle };
        Some((at, if past { at + 1 } else { at }, line))
    }

    /// What a click at a point takes, as `[start, end)`, or nothing if the page has nothing to hit.
    pub fn unit_at(&self, x: f32, y: f32, unit: Unit) -> Option<(u32, u32)> {
        let (at, place, line) = self.hit(x, y)?;
        Some(match unit {
            Unit::Char => (place, place),
            Unit::Word => self.word_at(at),
            Unit::Line => (line.start, line.end),
        })
    }

    /// The word holding a character. Letters and digits together, with an apostrophe between letters and a point or comma between digits, as double-clicking does on a Mac: `don't` and `3.14` are one, and `state-of-the-art` is four. A run of spaces is one too, and anything else stands alone. Han and kana have no spaces to go by, and no dictionary is held for them, so each stands alone.
    pub fn word_at(&self, at: u32) -> (u32, u32) {
        let at = at as usize;
        let Some(&ch) = self.text.get(at) else { return (at as u32, at as u32) };
        let in_word = |index: usize| -> bool { is_word(self.text[index]) && !is_ideograph(self.text[index]) && !self.is_hidden(index) };
        if ch.is_whitespace() {
            let same = |index: usize| self.text[index].is_whitespace() && self.text[index] != '\n' && !self.is_hidden(index);
            let (mut start, mut end) = (at, at + 1);
            while start > 0 && at - start < MAX_WORD && same(start - 1) {
                start -= 1;
            }
            while end < self.text.len() && end - at < MAX_WORD && same(end) {
                end += 1;
            }
            return (start as u32, end as u32);
        }
        let joins = |index: usize| -> bool {
            let (Some(before), Some(after)) = (index.checked_sub(1).and_then(|at| self.text.get(at)), self.text.get(index + 1)) else { return false };
            match self.text[index] {
                '\'' | '\u{2019}' => before.is_alphabetic() && after.is_alphabetic(),
                '.' | ',' => before.is_numeric() && after.is_numeric(),
                _ => false,
            }
        };
        if !in_word(at) && !joins(at) {
            return (at as u32, at as u32 + 1);
        }
        let (mut start, mut end) = (at, at + 1);
        while start > 0 && at - start < MAX_WORD && (in_word(start - 1) || joins(start - 1)) {
            start -= 1;
        }
        while end < self.text.len() && end - at < MAX_WORD && (in_word(end) || joins(end)) {
            end += 1;
        }
        (start as u32, end as u32)
    }

    /// Where a highlight goes for `[start, end)`: a rectangle a line, or more if the line has a gutter in it, on whole lines' bands, so it doesn't change height with the letters under it. Hidden text gets none.
    pub fn rects(&self, start: u32, end: u32) -> Vec<Rect> {
        let end = end.min(self.len());
        let mut out = Vec::new();
        if start >= end {
            return out;
        }
        let first = self.lines.partition_point(|line| line.end <= start);
        for line in &self.lines[first..] {
            if line.start >= end || out.len() >= MAX_RECTS_PER_PAGE {
                break;
            }
            if !line.seen() {
                continue;
            }
            let band = if line.sideways { line.x1 - line.x0 } else { line.y1 - line.y0 };
            let mut run: Option<(f32, f32)> = None;
            for at in line.start.max(start)..line.end.min(end) {
                if !self.shows(at as usize) {
                    continue;
                }
                let view = &self.boxes[at as usize];
                let (from, to) = if line.sideways { (view[1], view[3]) } else { (view[0], view[2]) };
                run = Some(match run {
                    Some((low, high)) if from - high <= GUTTER * band && low - to <= GUTTER * band => (low.min(from), high.max(to)),
                    Some(done) => {
                        self.push_rect(&mut out, line, done);
                        (from, to)
                    }
                    None => (from, to),
                });
            }
            if let Some(done) = run {
                self.push_rect(&mut out, line, done);
            }
        }
        out
    }

    fn push_rect(&self, out: &mut Vec<Rect>, line: &Line, (low, high): (f32, f32)) {
        if out.len() >= MAX_RECTS_PER_PAGE {
            return;
        }
        let (x0, y0, x1, y1) = if line.sideways { (line.x0, low, line.x1, high) } else { (low, line.y0, high, line.y1) };
        let (x0, x1) = (x0.max(0.0), x1.min(self.width));
        let (y0, y1) = (y0.max(0.0), y1.min(self.height));
        if x1 > x0 && y1 > y0 {
            out.push(Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 });
        }
    }

    /// Puts `[start, end)` on the end of `out`, past what's hidden and what's in `leave_out`, which is chrome, and says how many printed characters of each went. Taking words out of a line leaves their spaces either side, or a line of nothing, and that's taken out too.
    fn copy_into(&self, start: u32, end: u32, out: &mut String, room: usize, leave_out: &[(u32, u32)]) -> (u64, u64, bool) {
        let end = end.min(self.len());
        let (mut hidden_out, mut chrome_out, mut after_gap) = (0u64, 0u64, false);
        // Where `out` ends after the last printed character this put there, which is where it started if none.
        let mut last_kept = out.trim_end().len();
        for at in start..end {
            let left_out = leave_out.iter().any(|(from, to)| (*from..*to).contains(&at));
            let at = at as usize;
            let ch = self.text[at];
            if self.is_hidden(at) || left_out {
                let printed = u64::from(!ch.is_whitespace());
                if self.is_hidden(at) { hidden_out += printed } else { chrome_out += printed }
                after_gap = true;
                continue;
            }
            if after_gap && ch.is_whitespace() && (out.is_empty() || out.ends_with(char::is_whitespace)) {
                continue;
            }
            after_gap = false;
            if out.len() + ch.len_utf8() > room {
                return (hidden_out, chrome_out, true);
            }
            out.push(ch);
            if !ch.is_whitespace() {
                last_kept = out.len();
            }
        }
        // What ended in words left out leaves no line break or space behind it, as a footer would.
        if after_gap {
            out.truncate(last_kept);
        }
        (hidden_out, chrome_out, false)
    }

    /// The characters of `[start, end)` as they are, for `recto select`.
    pub fn text_of(&self, start: u32, end: u32) -> String {
        let mut out = String::new();
        self.copy_into(start, end, &mut out, usize::MAX, &[]);
        out
    }
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || is_mark(ch)
}

/// The common combining marks, which aren't letters by themselves and don't end a word.
fn is_mark(ch: char) -> bool {
    matches!(ch, '\u{300}'..='\u{36F}' | '\u{483}'..='\u{487}' | '\u{591}'..='\u{5BD}' | '\u{610}'..='\u{61A}' | '\u{64B}'..='\u{65F}' | '\u{900}'..='\u{903}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

fn is_ideograph(ch: char) -> bool {
    matches!(ch, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{20000}'..='\u{2FFFF}')
}

/// A place in a document: before this character of this page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub page: u32,
    pub at: u32,
}

/// A stretch of a document, in reading order: from `start` up to `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: Place,
    pub end: Place,
}

impl Range {
    /// A whole document.
    pub fn all(pages: u32) -> Self {
        Self { start: Place { page: 0, at: 0 }, end: Place { page: pages.saturating_sub(1), at: PAGE_END } }
    }

    /// Whole pages.
    pub fn pages(first: u32, last: u32) -> Self {
        Self { start: Place { page: first, at: 0 }, end: Place { page: last.max(first), at: PAGE_END } }
    }

    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// The part of this on one page, as `[start, end)`. Nothing if it doesn't reach that page. A page in the middle needs nothing read to say it's all of it.
    pub fn on_page(&self, page: u32) -> Option<(u32, u32)> {
        if page < self.start.page || page > self.end.page {
            return None;
        }
        Some((if page == self.start.page { self.start.at } else { 0 }, if page == self.end.page { self.end.at } else { PAGE_END }))
    }

    pub fn page_count(&self) -> u32 {
        (self.end.page - self.start.page).saturating_add(1)
    }
}

/// A point on a page as it's shown, which needn't be inside it: a drag runs off the side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PagePoint {
    pub page: u32,
    pub x: f32,
    pub y: f32,
}

/// What the model needs of a page's text. It's asked again whenever the answer might have changed, so a page that arrives late is picked up.
pub trait Texts {
    fn page_text(&self, page: u32) -> Option<&PageText>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    /// Where the pointer was, worked out against the text when it's there.
    Point(PagePoint),
    Place(Place),
}

impl End {
    fn page(&self) -> u32 {
        match self {
            Self::Point(point) => point.page,
            Self::Place(place) => place.page,
        }
    }
}

/// A selection as it's made. It holds what the pointer did, and no text: a press on a page that hasn't been read yet is worked out once it has, and a selection of a whole document is two places.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    anchor: Option<End>,
    focus: Option<End>,
    unit: Unit,
    dragging: bool,
}

impl Selection {
    /// The button goes down. With shift, the end of what's there moves here, and a click with nothing there starts from here. Otherwise it starts afresh, which clears what was selected.
    pub fn press(&mut self, point: PagePoint, clicks: u32, shift: bool) {
        self.dragging = true;
        if shift && self.anchor.is_some() {
            self.focus = Some(End::Point(point));
            return;
        }
        self.unit = Unit::from_clicks(clicks);
        self.anchor = Some(End::Point(point));
        self.focus = Some(End::Point(point));
    }

    /// The pointer moves with the button down. Ignored if it isn't, as an event can arrive with no press before it when a window comes forward.
    pub fn drag(&mut self, point: PagePoint) {
        if self.dragging {
            self.focus = Some(End::Point(point));
        }
    }

    pub fn release(&mut self) {
        self.dragging = false;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn set_range(&mut self, range: Range) {
        *self = Self { anchor: Some(End::Place(range.start)), focus: Some(End::Place(range.end)), unit: Unit::Char, dragging: false };
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn unit(&self) -> Unit {
        self.unit
    }

    /// Whether anything has been started, which isn't to say it covers anything.
    pub fn has_anchor(&self) -> bool {
        self.anchor.is_some()
    }

    /// The pages whose text says where the ends are.
    pub fn end_pages(&self) -> [Option<u32>; 2] {
        let wanted = |end: Option<End>| end.and_then(|end| matches!(end, End::Point(_)).then(|| end.page()));
        [wanted(self.anchor), wanted(self.focus)]
    }

    fn resolve(&self, end: End, texts: &impl Texts) -> Option<(Place, Place)> {
        match end {
            End::Place(place) => Some((place, place)),
            End::Point(point) => {
                let page = point.page;
                let text = texts.page_text(page)?;
                let (start, end) = text.unit_at(point.x, point.y, self.unit).unwrap_or((0, 0));
                Some((Place { page, at: start }, Place { page, at: end }))
            }
        }
    }

    /// What's selected, in reading order, whichever way it was dragged. `None` until the text at both ends has been read or if nothing has been started. An end on a page with no text counts as the top of that page, so a drag through a scanned page into the next still works, and on its own it selects nothing.
    pub fn range(&self, texts: &impl Texts) -> Option<Range> {
        let (anchor, focus) = (self.anchor?, self.focus?);
        let (from_start, from_end) = self.resolve(anchor, texts)?;
        let (to_start, to_end) = self.resolve(focus, texts)?;
        Some(Range { start: from_start.min(to_start), end: from_end.max(to_end) })
    }

    /// The most a selection reaches, by pages, with nothing read. For deciding what might need reading.
    pub fn page_span(&self) -> Option<(u32, u32)> {
        let (anchor, focus) = (self.anchor?.page(), self.focus?.page());
        Some((anchor.min(focus), anchor.max(focus)))
    }
}

/// What a copy leaves out of `page` as chrome, given a way to the edge lines of the pages around it that could be read: see [`crate::chrome`]. Whether the pick was only chrome is the copy's to decide, over all of it: see [`CopiedText::finish`].
pub fn chrome_for<'a>(page: u32, text: &PageText, pages: u32, edges_of: impl Fn(u32) -> Option<&'a [EdgeLine]>) -> Vec<(u32, u32)> {
    let neighbours: Vec<(i64, &[EdgeLine])> = neighbour_pages(page, pages).filter_map(|other| Some((i64::from(other) - i64::from(page), edges_of(other)?))).collect();
    text.chrome_spans(&neighbours)
}

/// The pages whose edge lines a copy of `page` compares with, in a document of `pages`.
pub fn neighbour_pages(page: u32, pages: u32) -> impl Iterator<Item = u32> {
    (page.saturating_sub(chrome::NEIGHBOURS)..=page.saturating_add(chrome::NEIGHBOURS).min(pages.saturating_sub(1))).filter(move |other| *other != page)
}

/// Text on its way to the clipboard, a page at a time, up to a limit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CopiedText {
    pub text: String,
    /// Printed characters left out, because they can't be seen.
    pub hidden_left_out: u64,
    /// Printed characters left out as running heads, footers and page numbers. See [`crate::chrome`].
    pub chrome_left_out: u64,
    pub pages_read: u32,
    /// Pages that couldn't be checked for hidden text, so what they hold is copied as it is.
    pub pages_unchecked: u32,
    /// Pages whose text couldn't be read at all.
    pub pages_failed: u32,
    /// Pages too long to select, so left out.
    pub pages_too_long: u32,
    /// It stopped before the end of the selection, at this page, for lack of room.
    pub stopped_at: Option<u32>,
    pub restricted: bool,
    /// The same pick with its chrome kept, for as long as nothing else has been copied: what the pick was if it turns out to be only chrome. Empty once anything else is in [`Self::text`], so it's never more than the chrome of the pages before the body starts.
    chrome_only: String,
}

impl CopiedText {
    /// Adds what a range holds of one page, with a line between pages, less `leave_out`, from [`chrome_for`]. Says whether there's room for more. [`Self::finish`] ends it.
    pub fn add_page(&mut self, page: u32, text: &PageText, range: &Range, room: usize, leave_out: &[(u32, u32)]) -> bool {
        self.pages_read += 1;
        self.restricted |= text.copy_restricted();
        self.pages_unchecked += u32::from(!text.hidden_checked() && !text.is_empty());
        self.pages_too_long += u32::from(text.too_long());
        let Some((start, end)) = range.on_page(page) else { return true };
        if start >= text.len().min(end) {
            return true;
        }
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        // Until the body starts, what the chrome was is kept beside it, in case the pick is nothing else.
        if self.text.is_empty() && !leave_out.is_empty() {
            if !self.chrome_only.is_empty() && !self.chrome_only.ends_with('\n') {
                self.chrome_only.push('\n');
            }
            text.copy_into(start, end, &mut self.chrome_only, room, &[]);
        }
        let (hidden, chrome, stopped) = text.copy_into(start, end, &mut self.text, room, leave_out);
        self.hidden_left_out += hidden;
        self.chrome_left_out += chrome;
        if !self.text.is_empty() {
            self.chrome_only = String::new();
        }
        if stopped {
            self.stopped_at = Some(page);
        }
        !stopped
    }

    /// Ends a copy. A pick that was only running heads, footers and page numbers gets them: someone who selects only a header wants it, wherever the pick runs (owner, 2026-11-07, in SPEC: "selecting only a header copies it").
    pub fn finish(&mut self) {
        if self.text.is_empty() && !self.chrome_only.is_empty() {
            self.text = std::mem::take(&mut self.chrome_only);
            self.chrome_left_out = 0;
        }
        self.chrome_only = String::new();
    }

    pub fn add_failed_page(&mut self) {
        self.pages_read += 1;
        self.pages_failed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: PageFrame = PageFrame { turns: 0, left: 0.0, bottom: 0.0, right: 612.0, top: 792.0 };

    fn add_line(chars: &mut Vec<CharRecord>, boxes: &mut Vec<CharBox>, text: &str, x: f32, y: f32) {
        for (index, ch) in text.chars().enumerate() {
            let left = x + index as f32 * 6.0;
            chars.push(CharRecord::new(ch as u32, 0));
            boxes.push(if ch == ' ' { [left, y, left + 6.0, y] } else { [left, y, left + 5.0, y + 10.0] });
        }
        chars.push(CharRecord::new('\r' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
    }

    fn page_of(lines: &[(&str, f32, f32)]) -> PageText {
        page_in(FRAME, lines)
    }

    fn page_in(frame: PageFrame, lines: &[(&str, f32, f32)]) -> PageText {
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        for (text, x, y) in lines {
            add_line(&mut chars, &mut boxes, text, *x, *y);
        }
        PageText::build(&chars, &boxes, frame, true, false)
    }

    /// A y on the page as it's shown, for a line given at this y on the page itself.
    fn shown(y: f32) -> f32 {
        792.0 - y - 5.0
    }

    /// A range in one go, as a press and a drag make it.
    fn dragged(page: &PageText, from: (f32, f32), to: (f32, f32), clicks: u32) -> Option<(u32, u32)> {
        struct One<'a>(&'a PageText);
        impl Texts for One<'_> {
            fn page_text(&self, page: u32) -> Option<&PageText> {
                (page == 0).then_some(self.0)
            }
        }
        let mut selection = Selection::default();
        selection.press(PagePoint { page: 0, x: from.0, y: from.1 }, clicks, false);
        selection.drag(PagePoint { page: 0, x: to.0, y: to.1 });
        selection.release();
        let range = selection.range(&One(page))?;
        Some((range.start.at, range.end.at))
    }

    /// A report's pages: a running head at the top, two lines of body, and the page number at the bottom.
    fn report(pages: u32) -> Vec<PageText> {
        (0..pages)
            .map(|page| {
                let (one, two, number) = (format!("Body line one of page {page}"), format!("Body line two of page {page}"), format!("{}", page + 1));
                page_of(&[("Annual report 2024", 72.0, 750.0), (&one, 72.0, 700.0), (&two, 72.0, 685.0), (&number, 540.0, 30.0)])
            })
            .collect()
    }

    /// A copy as `recto copy` and the app make it: each page with its neighbours.
    fn copy_of(pages: &[PageText], range: Range) -> CopiedText {
        let mut copied = CopiedText::default();
        for page in range.start.page..=range.end.page {
            let leave_out = chrome_for(page, &pages[page as usize], pages.len() as u32, |other| pages.get(other as usize).map(PageText::edge_lines));
            copied.add_page(page, &pages[page as usize], &range, usize::MAX, &leave_out);
        }
        copied.finish();
        copied
    }

    /// Failure: a selection across pages copies the page number and the running head into the middle of the body text, as the owner found (2026-11-07: "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text."), and the same within a page when the body is picked with them.
    #[test]
    fn a_copy_of_body_text_leaves_out_running_heads_and_page_numbers() {
        let pages = report(3);
        let at = |page: &PageText, needle: &str| page.text.iter().collect::<String>().find(needle).unwrap() as u32;
        let range = Range { start: Place { page: 0, at: at(&pages[0], "two") }, end: Place { page: 1, at: at(&pages[1], "two") } };
        let copied = copy_of(&pages, range);
        assert_eq!(copied.text, "two of page 0\nBody line one of page 1\nBody line ", "{:?}", copied.text);
        assert!(copied.chrome_left_out > 0);
        let whole = copy_of(&pages, Range::pages(1, 1));
        assert_eq!(whole.text, "Body line one of page 1\nBody line two of page 1", "a whole page, picked with its chrome: {:?}", whole.text);
        let everything = copy_of(&pages, Range::all(3));
        assert!(!everything.text.contains("Annual") && !everything.text.lines().any(|line| line.trim().parse::<u32>().is_ok()), "{:?}", everything.text);
    }

    /// Failure: a pick of only chrome that runs over a page break, from one page's number to the next page's running head, copies nothing, and a note calls it a scan (review of e3fc6e0). Whether the pick was only chrome is decided over all of it.
    #[test]
    fn chrome_picked_across_a_page_break_is_copied() {
        let pages = report(3);
        let text = |page: usize| pages[page].text.iter().collect::<String>();
        let number = text(1).rfind('2').unwrap() as u32;
        let head_end = text(2).find("2024").unwrap() as u32 + 4;
        let copied = copy_of(&pages, Range { start: Place { page: 1, at: number }, end: Place { page: 2, at: head_end } });
        assert_eq!(copied.text, "2\nAnnual report 2024", "{:?}", copied.text);
        assert_eq!(copied.chrome_left_out, 0, "nothing was left out of what's copied");
    }

    /// Failure: a page number on the same row as the last line of a page's text, or just under it, stays in a copy, as on 24 pages of the owner's part B (review of e3fc6e0), because it isn't set apart from the body.
    #[test]
    fn a_page_number_by_the_last_line_is_left_out() {
        // A page of body text down to its foot, lines 15 points apart, its number at `number_y`.
        let page = |page: u32, number_y: f32| {
            let lines: Vec<(String, f32, f32)> = (0..37).map(|line| (format!("Body line {line} of page {page}"), 72.0, 600.0 - line as f32 * 15.0)).chain(std::iter::once((format!("{}", page + 1), 540.0, number_y))).collect();
            let borrowed: Vec<(&str, f32, f32)> = lines.iter().map(|(text, x, y)| (text.as_str(), *x, *y)).collect();
            page_of(&borrowed)
        };
        for (number_y, place) in [(60.0, "on the last line's row"), (49.0, "just under it")] {
            let pages: Vec<PageText> = (0..4).map(|at| page(at, number_y)).collect();
            let copied = copy_of(&pages, Range::pages(1, 2));
            assert!(!copied.text.lines().any(|line| line.trim().parse::<u32>().is_ok()), "{place}: {:?}", copied.text);
            assert!(copied.text.contains("Body line 36 of page 1\nBody line 0 of page 2"), "{place}: {:?}", copied.text);
        }
    }


    /// Failure: someone who selects only a running head, or only a page number, gets nothing, which isn't what they picked. Or a line at the top that repeats but sits at line spacing above the body, as a table's header on each page does, is taken for chrome.
    #[test]
    fn chrome_picked_by_itself_is_copied_and_body_at_the_edge_is_kept() {
        let pages = report(3);
        let head = pages[1].text.iter().collect::<String>().find("Annual").unwrap() as u32;
        let only_head = copy_of(&pages, Range { start: Place { page: 1, at: head }, end: Place { page: 1, at: head + 18 } });
        assert_eq!(only_head.text, "Annual report 2024");
        let tables: Vec<PageText> = (0..3).map(|page| page_of(&[("Year Revenue Cost", 72.0, 750.0), (&format!("{} 12 4", 2000 + page), 72.0, 735.0), ("2010 13 5", 72.0, 720.0), ("2011 14 5", 72.0, 705.0), ("2012 15 6", 72.0, 690.0), ("words of the body here", 72.0, 500.0)])).collect();
        let copied = copy_of(&tables, Range::pages(1, 1));
        assert!(copied.text.starts_with("Year Revenue Cost"), "{:?}", copied.text);
        let alone = report(1);
        let copied = copy_of(&alone, Range::pages(0, 0));
        assert!(copied.text.starts_with("Annual report 2024") && !copied.text.ends_with('1'), "one page alone: its head can't be told from a title, and its number can: {:?}", copied.text);
    }

    /// Failure: a line grows with the page, so a page held for selection costs far more than the budget says.
    #[test]
    fn a_line_stays_small() {
        assert!(size_of::<Line>() <= 32, "{}", size_of::<Line>());
    }

    /// Failure: a click lands on the wrong word, or a double-click takes the line or half a word, because a box was read in the page's coordinates and not as it's shown, or a place was off by one.
    #[test]
    fn a_click_finds_its_word_and_a_third_click_its_line() {
        let page = page_of(&[("hello brave world", 72.0, 700.0), ("second line here", 72.0, 680.0)]);
        // `brave` starts at 6 characters in, each 6 points wide.
        let brave_x = 72.0 + 8.0 * 6.0 + 1.0;
        let y = shown(700.0);
        let (start, end) = page.unit_at(brave_x, y, Unit::Word).unwrap();
        assert_eq!(page.text_of(start, end), "brave");
        let (start, end) = page.unit_at(brave_x, y, Unit::Line).unwrap();
        assert_eq!(page.text_of(start, end), "hello brave world");
        let (start, end) = page.unit_at(brave_x, shown(680.0), Unit::Line).unwrap();
        assert_eq!(page.text_of(start, end), "second line here", "the line under the point, not the first");
        let (start, end) = page.unit_at(72.5, y, Unit::Char).unwrap();
        assert_eq!((start, end), (0, 0), "the left half of the first letter is before it");
        let (start, end) = page.unit_at(76.5, y, Unit::Char).unwrap();
        assert_eq!((start, end), (1, 1), "the right half is after it");
    }

    /// Failure: double-clicking a number or a contraction takes half of it, or a hyphenated compound comes in one piece where a Mac takes one word of it, or a space beside a word takes the word.
    #[test]
    fn a_word_is_what_a_double_click_takes_on_a_mac() {
        let page = page_of(&[("don't 3.14 state-of-art  x, y", 72.0, 700.0)]);
        let at = |needle: &str| page.text.iter().collect::<String>().find(needle).unwrap() as u32;
        let word = |index: u32| {
            let (start, end) = page.word_at(index);
            page.text_of(start, end)
        };
        assert_eq!(word(at("don't") + 2), "don't");
        assert_eq!(word(at("3.14") + 1), "3.14");
        assert_eq!(word(at("state")), "state");
        assert_eq!(word(at("of")), "of");
        assert_eq!(word(at("-of")), "-", "the hyphen stands alone");
        assert_eq!(word(at("  x")), "  ", "a run of spaces is one");
        assert_eq!(word(at(", y")), ",");
        assert_eq!(page.word_at(10_000), (10_000, 10_000), "past the end is nothing, and no panic");
    }

    /// Failure: Chinese double-clicks as a whole sentence, since it has no spaces, and the highlight jumps across half a page.
    #[test]
    fn han_stands_alone() {
        let page = page_of(&[("数据库的设计", 72.0, 700.0)]);
        assert_eq!(page.word_at(2), (2, 3));
    }

    /// Failure: a selection follows x and y instead of reading order, so a drag from the left column into the right takes bands across both, and a copy comes out as the two columns' lines interleaved.
    #[test]
    fn a_drag_across_columns_follows_reading_order() {
        let left = [("left one", 72.0, 700.0), ("left two", 72.0, 680.0), ("left three", 72.0, 660.0)];
        let right = [("right one", 340.0, 700.0), ("right two", 340.0, 680.0), ("right three", 340.0, 660.0)];
        let page = page_of(&[left[0], left[1], left[2], right[0], right[1], right[2]]);
        // From `two` in the left column to the end of the right column's second line.
        let (start, end) = dragged(&page, (72.0 + 5.0 * 6.0 + 1.0, shown(680.0)), (340.0 + 60.0, shown(680.0)), 1).unwrap();
        assert_eq!(page.text_of(start, end), "two\nleft three\nright one\nright two");
        let rects = page.rects(start, end);
        assert_eq!(rects.len(), 4, "one a line: {rects:?}");
        assert!(rects.iter().all(|rect| rect.x + rect.width <= 300.0 || rect.x >= 330.0), "nothing bridges the gutter: {rects:?}");
    }

    /// Failure: dragging up the page selects nothing or the wrong stretch, since the end comes before the start.
    #[test]
    fn dragging_backward_selects_the_same_text() {
        let page = page_of(&[("one two three", 72.0, 700.0), ("four five six", 72.0, 680.0)]);
        let (from, to) = ((72.0 + 4.0 * 6.0, shown(700.0)), (72.0 + 9.0 * 6.0 + 1.0, shown(680.0)));
        assert_eq!(dragged(&page, from, to, 1), dragged(&page, to, from, 1));
        let (start, end) = dragged(&page, to, from, 1).unwrap();
        assert_eq!(page.text_of(start, end), "two three\nfour five");
    }

    /// Failure: double-click and drag extends by letters, so the word that started it is left half-selected.
    #[test]
    fn a_double_click_drag_goes_by_words_both_ways() {
        let page = page_of(&[("alpha beta gamma delta", 72.0, 700.0)]);
        let y = shown(700.0);
        let x_of = |at: usize| 72.0 + at as f32 * 6.0 + 1.0;
        let (start, end) = dragged(&page, (x_of(7), y), (x_of(12), y), 2).unwrap();
        assert_eq!(page.text_of(start, end), "beta gamma");
        let (start, end) = dragged(&page, (x_of(12), y), (x_of(7), y), 2).unwrap();
        assert_eq!(page.text_of(start, end), "beta gamma", "and starting from the far word");
    }

    /// Failure: a shift-click starts a new selection rather than extending the old one, a click doesn't clear, or a press on a page whose text hasn't arrived is lost.
    #[test]
    fn shift_extends_a_click_clears_and_late_text_is_picked_up() {
        struct Late(Option<PageText>);
        impl Texts for Late {
            fn page_text(&self, page: u32) -> Option<&PageText> {
                if page == 0 { self.0.as_ref() } else { None }
            }
        }
        let mut late = Late(None);
        let y = shown(700.0);
        let point = |x: f32| PagePoint { page: 0, x, y };
        let mut selection = Selection::default();
        selection.press(point(72.0), 1, false);
        selection.drag(point(72.0 + 3.0 * 6.0));
        selection.release();
        assert_eq!(selection.range(&late), None, "not worked out until it's there");
        assert_eq!(selection.end_pages(), [Some(0), Some(0)], "it says what it's waiting for");
        late.0 = Some(page_of(&[("abcdefghij", 72.0, 700.0)]));
        assert_eq!(selection.range(&late).map(|range| range.end.at), Some(3));
        selection.press(point(72.0 + 6.0 * 6.0), 1, true);
        selection.release();
        let range = selection.range(&late).unwrap();
        assert_eq!((range.start.at, range.end.at), (0, 6), "the far end moved, and the anchor held");
        selection.press(point(72.0 + 2.0 * 6.0), 1, false);
        selection.release();
        assert!(selection.range(&late).unwrap().is_empty(), "a click elsewhere leaves nothing selected");
        selection.press(point(72.0 + 5.0 * 6.0), 1, true);
        selection.release();
        assert_eq!(selection.range(&late).map(|range| (range.start.at, range.end.at)), Some((2, 5)), "and shift goes on from the click");
        let mut fresh = Selection::default();
        fresh.drag(point(80.0));
        assert_eq!(fresh.range(&late), None, "a drag with no press is nothing");
    }

    /// Failure: a selection across pages needs every page between them read, which is what stops a whole document staying cheap.
    #[test]
    fn the_pages_between_need_nothing_read() {
        let range = Range::pages(2, 9000);
        assert_eq!(range.on_page(4000), Some((0, PAGE_END)));
        assert_eq!(range.on_page(1), None);
        assert_eq!(range.on_page(9001), None);
        let mut selection = Selection::default();
        selection.set_range(Range::all(8_000_000));
        assert_eq!(selection.end_pages(), [None, None], "two places, known already");
        assert_eq!(selection.range(&NoTexts).map(|range| range.page_count()), Some(8_000_000));
        assert!(size_of::<Selection>() <= 64, "{}", size_of::<Selection>());
    }

    struct NoTexts;
    impl Texts for NoTexts {
        fn page_text(&self, _page: u32) -> Option<&PageText> {
            None
        }
    }

    /// Failure: a page with no text layer, a scan, errors or panics when it's clicked, or a click on it selects something.
    #[test]
    fn a_page_with_no_text_selects_nothing() {
        let page = PageText::build(&[], &[], FRAME, true, false);
        assert_eq!(page.unit_at(100.0, 100.0, Unit::Word), None);
        assert!(!page.is_over_text(100.0, 100.0));
        assert!(page.rects(0, PAGE_END).is_empty());
        assert_eq!(page.text_of(0, PAGE_END), "");
        assert_eq!(dragged(&page, (10.0, 10.0), (500.0, 500.0), 1), Some((0, 0)));
    }

    /// Failure: a selection on a turned page lands somewhere other than the words, as `recto text` reads boxes in the page's own coordinates and the reader sees them turned. The same line, in each turn: the rectangle must be where the line ends up.
    #[test]
    fn a_turned_page_has_its_boxes_where_they_show() {
        let lines = [("rotated", 100.0, 700.0)];
        // Page space: x 100 to 142, y 700 to 710, on a page 612 by 792.
        for (turns, x, y, wide) in [(0, 100.0, 82.0, true), (1, 700.0, 100.0, false), (2, 470.0, 700.0, true), (3, 82.0, 470.0, false)] {
            let page = page_in(PageFrame { turns, ..FRAME }, &lines);
            let rects = page.rects(0, PAGE_END);
            assert_eq!(rects.len(), 1, "turn {turns}: {rects:?}");
            let rect = rects[0];
            assert!(rect.x <= x + 1.0 && rect.x + rect.width >= x + 1.0, "turn {turns}: {rect:?}");
            assert!(rect.y <= y + 1.0 && rect.y + rect.height >= y + 1.0, "turn {turns}: {rect:?}");
            assert_eq!(rect.width > rect.height, wide, "turn {turns}: {rect:?}");
            assert_eq!(page.size(), if turns & 1 == 1 { (792.0, 612.0) } else { (612.0, 792.0) });
            // The middle of the rectangle hits the line, and its far end is its last letter, however it runs.
            let (start, end) = page.unit_at(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0, Unit::Line).unwrap();
            assert_eq!(page.text_of(start, end), "rotated", "turn {turns}");
        }
    }

    /// Failure: a line running up or down the page or right to left puts places in the wrong order, so dragging along it selects from the far end.
    #[test]
    fn a_place_goes_the_way_the_line_runs() {
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        // Each letter further left than the last, as right-to-left text comes.
        for (index, ch) in "שלום".chars().enumerate() {
            let left = 300.0 - index as f32 * 6.0;
            chars.push(CharRecord::new(ch as u32, 0));
            boxes.push([left, 700.0, left + 5.0, 710.0]);
        }
        let page = PageText::build(&chars, &boxes, FRAME, true, false);
        let y = shown(700.0);
        assert_eq!(page.unit_at(305.0, y, Unit::Char), Some((0, 0)), "the right edge is its start");
        assert_eq!(page.unit_at(282.0, y, Unit::Char), Some((4, 4)), "the left edge is its end");
    }

    /// Failure: a word broken over a line by a hyphen copies with the hyphen and the break in it, or takes a highlight on only one line, or the line it's on gets taken as one with the next.
    #[test]
    fn a_word_joined_over_a_line_end_is_one_word_on_two_lines() {
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        for (index, ch) in "the valu".chars().enumerate() {
            let left = 72.0 + index as f32 * 6.0;
            chars.push(CharRecord::new(ch as u32, 0));
            boxes.push([left, 700.0, left + 5.0, 710.0]);
        }
        chars.push(CharRecord::new(0xFFFE, CharRecord::HYPHEN));
        boxes.push([120.0, 700.0, 125.0, 710.0]);
        chars.push(CharRecord::new('\r' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        for (index, ch) in "able one".chars().enumerate() {
            let left = 72.0 + index as f32 * 6.0;
            chars.push(CharRecord::new(ch as u32, 0));
            boxes.push([left, 680.0, left + 5.0, 690.0]);
        }
        let page = PageText::build(&chars, &boxes, FRAME, true, false);
        let (start, end) = page.unit_at(72.0 + 5.0 * 6.0, shown(700.0), Unit::Word).unwrap();
        assert_eq!(page.text_of(start, end), "valuable");
        assert_eq!(page.rects(start, end).len(), 2, "a piece on each line");
        let (start, end) = page.unit_at(72.0 + 6.0 * 6.0, shown(680.0), Unit::Line).unwrap();
        assert_eq!(page.text_of(start, end), "able one", "it's its own line, joined or not");
    }

    /// Failure: words drawn late, which `recto text` puts back on their lines, select in the place they were drawn: a copy of `A recipient MUST ignore` loses `MUST` to the bottom of the page, and the offsets stop matching `recto text`.
    #[test]
    fn late_words_are_selected_where_they_print() {
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        let place = |text: &str, x: f32, y: f32, chars: &mut Vec<CharRecord>, boxes: &mut Vec<CharBox>| {
            for (index, ch) in text.chars().enumerate() {
                let left = x + index as f32 * 5.0;
                chars.push(CharRecord::new(ch as u32, 0));
                boxes.push([left, y, left + 4.5, y + 7.0]);
            }
        };
        place("A recipient", 72.0, 700.0, &mut chars, &mut boxes);
        place("ignore", 72.0 + 17.0 * 5.0, 700.0, &mut chars, &mut boxes);
        chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        place("the rest of it", 72.0, 680.0, &mut chars, &mut boxes);
        chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        place("MUST", 72.0 + 12.0 * 5.0, 700.0, &mut chars, &mut boxes);
        let page = PageText::build(&chars, &boxes, FRAME, true, false);
        let read = crate::read_page(&chars, &boxes, false);
        assert_eq!(page.text_of(0, PAGE_END), read.normalized.text, "the same text as `recto text`");
        let (start, end) = page.unit_at(72.0 + 3.0 * 5.0, shown(700.0), Unit::Line).unwrap();
        assert_eq!(page.text_of(start, end), "A recipient MUST ignore");
    }

    /// Failure: text no eye can see is pasted into someone's email, or hit by a click, or highlighted over blank paper. And the count of what went is wrong, so a notice says something untrue.
    #[test]
    fn hidden_text_is_left_out_and_counted() {
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        let line = |text: &str, x: f32, y: f32, flags: u32, chars: &mut Vec<CharRecord>, boxes: &mut Vec<CharBox>| {
            for (index, ch) in text.chars().enumerate() {
                let left = x + index as f32 * 6.0;
                chars.push(CharRecord::new(ch as u32, if ch == ' ' { 0 } else { flags }));
                boxes.push([left, y, left + 5.0, y + 10.0]);
            }
        };
        line("see ", 72.0, 700.0, 0, &mut chars, &mut boxes);
        line("secret", 96.0, 700.0, CharRecord::NO_INK, &mut chars, &mut boxes);
        line(" now", 132.0, 700.0, 0, &mut chars, &mut boxes);
        chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        line("beyond", 2000.0, 680.0, 0, &mut chars, &mut boxes);
        chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
        boxes.push([0.0; 4]);
        line("last", 72.0, 660.0, 0, &mut chars, &mut boxes);
        let page = PageText::build(&chars, &boxes, FRAME, true, false);
        let mut copied = CopiedText::default();
        assert!(copied.add_page(0, &page, &Range::pages(0, 0), usize::MAX, &[]));
        assert_eq!(copied.text, "see now\nlast");
        assert_eq!(copied.hidden_left_out, 12, "`secret` and `beyond`");
        let (start, end) = page.unit_at(100.0, shown(700.0), Unit::Word).unwrap();
        assert_ne!(page.text_of(start, end), "secret", "a click can't land on it");
        let rects = page.rects(0, PAGE_END);
        assert!(rects.iter().all(|rect| rect.x + rect.width <= 612.0), "{rects:?}");
        assert!(!page.is_over_text(2010.0, shown(680.0)), "no I-beam on what's off the page");
    }

    /// Failure: the second read of a page, with the hidden-text check, moves what's already been worked out from the first: a selection jumps when the check comes back, a moment after it's made. Every character keeps its place, the hidden ones too, so a range keeps meaning the same characters. What does change is what's on them: a hidden word can't be hit or highlighted or copied, and an end that was on one goes to the nearest thing that shows.
    #[test]
    fn checking_for_hidden_text_moves_no_place() {
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        add_line(&mut chars, &mut boxes, "see the secret words now", 72.0, 700.0);
        add_line(&mut chars, &mut boxes, "and a second line", 72.0, 680.0);
        let first = PageText::build(&chars, &boxes, FRAME, false, false);
        for record in chars.iter_mut().skip(8).take(6) {
            *record = CharRecord::new(record.ch() as u32, CharRecord::NO_INK);
        }
        let checked = PageText::build(&chars, &boxes, FRAME, true, false);
        assert_eq!(first.len(), checked.len());
        assert_eq!(first.text, checked.text);
        assert_eq!(first.line_count(), checked.line_count());
        // Off the hidden word, a click lands where it did.
        assert_eq!(first.unit_at(80.0, shown(680.0), Unit::Line), checked.unit_at(80.0, shown(680.0), Unit::Line));
        assert_eq!(first.unit_at(400.0, shown(700.0), Unit::Char), checked.unit_at(400.0, shown(700.0), Unit::Char));
        assert_eq!(first.unit_at(72.0 + 16.0 * 6.0, shown(700.0), Unit::Word), checked.unit_at(72.0 + 16.0 * 6.0, shown(700.0), Unit::Word), "`words`, after it");
        // On it: the first read takes it, and the check takes it away, to what's next to it.
        let on_secret = 72.0 + 10.0 * 6.0;
        let (start, end) = first.unit_at(on_secret, shown(700.0), Unit::Word).unwrap();
        assert_eq!(first.text_of(start, end), "secret");
        let (start, end) = checked.unit_at(on_secret, shown(700.0), Unit::Word).unwrap();
        assert_ne!(checked.text_of(start, end), "secret");
        // A range over it means the same characters either way, with the hidden ones left out of what comes of it.
        assert_eq!(first.text_of(4, 20), "the secret words");
        assert_eq!(checked.text_of(4, 20), "the words");
        let (whole, without) = (first.rects(4, 20), checked.rects(4, 20));
        assert_eq!(whole.len(), 1);
        assert!(!without.is_empty() && without.iter().all(|rect| rect.y == whole[0].y && rect.height == whole[0].height), "{without:?}");
    }

    /// Failure: copying a long selection grows without end, or stops without saying where, or runs pages together with no line between.
    #[test]
    fn a_copy_stops_at_its_room_and_says_so() {
        let page = page_of(&[("twelve chars", 72.0, 700.0)]);
        let range = Range::pages(0, 1000);
        let mut copied = CopiedText::default();
        assert!(copied.add_page(0, &page, &range, 64, &[]));
        assert!(copied.add_page(1, &page, &range, 64, &[]));
        assert_eq!(copied.text.matches("twelve chars\n").count(), 2, "{:?}", copied.text);
        let mut full = CopiedText::default();
        assert!(!full.add_page(0, &page, &range, 5, &[]), "no room");
        assert_eq!(full.stopped_at, Some(0));
        assert_eq!(full.text, "twelv");
        let mut failed = CopiedText::default();
        failed.add_failed_page();
        assert_eq!((failed.pages_failed, failed.pages_read), (1, 1));
    }

    /// Failure: a page too long to select is treated as having no text, with nothing to say why, or is held whole anyway.
    #[test]
    fn a_page_too_long_holds_nothing_and_says_so() {
        let chars = vec![CharRecord::new('a' as u32, 0); MAX_SELECTABLE_CHARS + 1];
        let boxes = vec![[1.0, 1.0, 2.0, 2.0]; MAX_SELECTABLE_CHARS + 1];
        let page = PageText::build(&chars, &boxes, FRAME, true, true);
        assert!(page.too_long() && page.is_empty() && page.copy_restricted());
        assert!(page.bytes() < 1024);
        let mut copied = CopiedText::default();
        copied.add_page(0, &page, &Range::pages(0, 0), 100, &[]);
        assert_eq!((copied.pages_too_long, copied.restricted), (1, true));
    }

    /// Failure: a page of nothing but line breaks holds a line for each, past the cap this module says it keeps to: `MAX_LINES` only held for the lines geometry found.
    #[test]
    fn a_page_of_line_breaks_keeps_to_the_cap_on_lines() {
        let mut chars = Vec::new();
        let mut boxes = Vec::new();
        for at in 0..(MAX_LINES + 2000) {
            chars.push(CharRecord::new('a' as u32, 0));
            boxes.push([72.0, (at % 700) as f32, 77.0, (at % 700) as f32 + 5.0]);
            chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
            boxes.push([0.0; 4]);
        }
        let page = PageText::build(&chars, &boxes, FRAME, true, false);
        assert!(page.line_count() <= MAX_LINES, "{}", page.line_count());
        assert_eq!(page.text_of(0, PAGE_END).matches('a').count(), MAX_LINES + 2000, "and no text goes with them");
    }

    /// Failure: hit-testing a NaN from the host, a page with a zero-size frame, or an end past the text panics, as a slice would.
    #[test]
    fn nothing_from_outside_can_panic_it() {
        let page = page_of(&[("text", 72.0, 700.0)]);
        assert_eq!(page.unit_at(f32::NAN, 0.0, Unit::Char), None);
        assert!(!page.is_over_text(f32::INFINITY, 0.0));
        assert!(page.rects(3, 2).is_empty());
        assert!(page.rects(u32::MAX - 1, u32::MAX).is_empty());
        assert_eq!(page.text_of(100, 200), "");
        let no_frame = page_in(PageFrame::default(), &[("text", 72.0, 700.0)]);
        assert_eq!(no_frame.unit_at(72.0, 700.0, Unit::Word), None, "nowhere to hit, with the text still there to copy");
        assert_eq!(no_frame.text_of(0, PAGE_END), "text\n");
    }
}
