//! Selecting text on hostile characters and boxes. It runs in the host, on what a worker sent, and on every mouse move, so a NaN, a line of a hundred thousand characters or a frame with no size has to do no harm, and take no time.
//!
//! What must hold: a place and a range stay inside the text, a highlight is on the page and holds real numbers, a copy holds what was asked for and no more than its room, and however the pointer goes the model comes out with a start at or before its end.

use super::index::page;
use crate::guard::{Budget, within};
use crate::input::Take;
use recto_index::select::{CopiedText, MAX_LINES, MAX_RECTS_PER_PAGE, MAX_SELECTABLE_CHARS, PAGE_END, PagePoint, PageText, Range, Selection, Texts, Unit};
use recto_proto::PageFrame;
use std::time::Duration;

const BUDGET: Budget = Budget { time: Duration::from_secs(60), memory: 512 * 1024 * 1024 };
const EDGES: &[f32] = &[0.0, 0.0, 612.0, 792.0, 72.0, -100.0, 1e-30, 1e30, f32::MAX, f32::MIN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
const POINTS: &[f32] = &[0.0, 72.0, 100.0, 300.0, 700.0, 792.0, -5.0, 1e9, -1e9, f32::MAX, f32::MIN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
/// Clicks and drags tried per page, so a long page's time is its lines' and not this many passes over them.
const POINTERS: usize = 32;

struct One(PageText, PageText);

impl Texts for One {
    fn page_text(&self, page: u32) -> Option<&PageText> {
        match page {
            0 => Some(&self.0),
            1 => Some(&self.1),
            _ => None,
        }
    }
}

/// Catches: a place or a range out of bounds, a highlight off the page or not a number, a copy past its room, and anything worse than linear on a hostile page.
pub fn check(data: &[u8]) {
    // What says how the page sits and where the pointer goes comes first and is short, and the rest is the page, as in `index`.
    let (head, rest) = data.split_at(data.len().min(64));
    let mut take = Take(head);
    let frame = PageFrame { turns: take.u8() & 3, left: take.pick(EDGES), bottom: take.pick(EDGES), right: take.pick(EDGES), top: take.pick(EDGES) };
    let (chars, boxes) = page(rest);
    within("selecting on a hostile page", BUDGET, || {
        let text = PageText::build(&chars, &boxes, frame, take.u8() & 1 == 1, take.u8() & 1 == 1);
        check_page(&text, &mut take);
        let without_boxes = PageText::build(&chars, &[], frame, true, false);
        check_page(&without_boxes, &mut take);
        check_pointer(&One(text, without_boxes), &mut take);
    });
}

fn check_page(text: &PageText, take: &mut Take) {
    let len = text.len();
    assert!(len as usize <= MAX_SELECTABLE_CHARS * 2, "{len}");
    assert!(text.rects(0, PAGE_END).len() <= MAX_RECTS_PER_PAGE);
    let (width, height) = text.size();
    for rect in text.rects(0, PAGE_END) {
        assert!([rect.x, rect.y, rect.width, rect.height].iter().all(|edge| edge.is_finite()), "{rect:?}");
        assert!(rect.width > 0.0 && rect.height > 0.0, "{rect:?}");
        assert!(rect.x >= 0.0 && rect.y >= 0.0 && rect.x + rect.width <= width * 1.0001 && rect.y + rect.height <= height * 1.0001, "{rect:?} is off a page {width} by {height}");
    }
    for _ in 0..POINTERS {
        let (x, y) = (take.pick(POINTS), take.pick(POINTS));
        let _ = text.is_over_text(x, y);
        for unit in [Unit::Char, Unit::Word, Unit::Line] {
            if let Some((start, end)) = text.unit_at(x, y, unit) {
                assert!(start <= end && end <= len, "{unit:?} at {x},{y} is {start} to {end} of {len}");
                let _ = text.rects(start, end);
                let _ = text.text_of(start, end);
            }
        }
        let at = take.u32() % len.max(1);
        let (start, end) = text.word_at(at);
        assert!(start <= end && (len == 0 || end <= len), "the word at {at} is {start} to {end} of {len}");
    }
    let whole = text.text_of(0, PAGE_END);
    assert!(whole.chars().count() <= len as usize);
    let mut copied = CopiedText::default();
    copied.add_page(0, text, &Range::pages(0, 0), 64);
    assert!(copied.text.len() <= 64, "{}", copied.text.len());
    assert!(MAX_LINES >= 1);
}

fn check_pointer(texts: &One, take: &mut Take) {
    let mut selection = Selection::default();
    for _ in 0..POINTERS {
        let point = PagePoint { page: u32::from(take.u8() % 3), x: take.pick(POINTS), y: take.pick(POINTS) };
        match take.u8() % 5 {
            0 => selection.press(point, u32::from(take.u8()), take.u8() & 1 == 1),
            1 => selection.drag(point),
            2 => selection.release(),
            3 => selection.set_range(Range { start: recto_index::select::Place { page: point.page, at: take.u32() }, end: recto_index::select::Place { page: point.page.saturating_add(1), at: take.u32() } }),
            _ => selection.clear(),
        }
        if let Some(range) = selection.range(texts) {
            assert!(range.start <= range.end, "{range:?}");
            for page in 0..3 {
                if let (Some((start, end)), Some(text)) = (range.on_page(page), texts.page_text(page)) {
                    let _ = text.rects(start, end);
                    let mut copied = CopiedText::default();
                    copied.add_page(page, text, &range, 256);
                    assert!(copied.text.len() <= 256);
                }
            }
        }
    }
}
