//! Selecting text on hostile characters and boxes. It runs in the host, on what a worker sent, and on every mouse move, so a NaN, a line of a hundred thousand characters or a frame with no size has to do no harm, and take no time.
//!
//! What must hold: a place and a range stay inside the text, a highlight is on the page and holds real numbers, a copy holds what was asked for and no more than its room, and however the pointer goes the model comes out with a start at or before its end.

use super::index::page;
use crate::guard::{Budget, within};
use crate::input::Take;
use recto_index::select::{CopiedText, MAX_LINES, MAX_RECTS_PER_PAGE, MAX_SELECTABLE_CHARS, PAGE_END, PagePoint, PageText, Place, Range, Selection, Texts, Unit, chrome_for, neighbour_pages};
use recto_proto::PageFrame;
use recto_reader::selection::{Selecting, Urgency};
use std::sync::Arc;
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
    // What says how the page sits and where the pointer goes comes first, up to a third of the input, and the rest is the page, as in `index`. Running out reads as zeros, which is one op over and over, so it needs a real share.
    let (head, rest) = data.split_at((data.len() / 3).clamp(data.len().min(64), 2048));
    let mut take = Take(head);
    let frame = PageFrame { turns: take.u8() & 3, left: take.pick(EDGES), bottom: take.pick(EDGES), right: take.pick(EDGES), top: take.pick(EDGES) };
    let (chars, boxes) = page(rest);
    within("selecting on a hostile page", BUDGET, || {
        let text = PageText::build(&chars, &boxes, frame, take.u8() & 1 == 1, take.u8() & 1 == 1);
        check_page(&text, &mut take);
        let without_boxes = PageText::build(&chars, &[], frame, true, false);
        check_page(&without_boxes, &mut take);
        let (text, without_boxes) = (Arc::new(text), Arc::new(without_boxes));
        check_page_view(&text, &without_boxes, &mut take);
        check_pointer(&One((*text).clone(), (*without_boxes).clone()), &mut take);
    });
}

fn check_page(text: &PageText, take: &mut Take) {
    let len = text.len();
    assert!(len as usize <= MAX_SELECTABLE_CHARS * 2, "{len}");
    assert!(text.rects(0, PAGE_END).len() <= MAX_RECTS_PER_PAGE);
    assert!(text.line_count() <= MAX_LINES, "{}", text.line_count());
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
    // Its own edge lines as its neighbour's, which a hostile file can make repeat however it likes.
    let leave_out = chrome_for(0, text, &Range::pages(0, 0), &[text.edge_lines()]);
    assert!(text.edge_lines().len() <= recto_index::chrome::MAX_EDGE_LINES);
    assert!(leave_out.iter().all(|(start, end)| start <= end && *end <= len), "{leave_out:?} of {len}");
    copied.add_page(0, text, &Range::pages(0, 0), 64, &leave_out);
    assert!(copied.text.len() <= 64, "{}", copied.text.len());
}

/// What the page view holds, through any order of what can happen to it: a pointer that does anything, text arriving or failing for any page at any time, a copy started and asked to go on, a window closing. Catches a subtraction that only holds if a selection stays put, which killed the thread that draws, and a copy that never comes to rest.
fn check_page_view(text: &Arc<PageText>, other: &Arc<PageText>, take: &mut Take) {
    const PAGES: u32 = 8;
    let mut selecting = Selecting::default();
    let visible = [1, 2, 3];
    // Half the time it starts with something selected over a few pages and on its way to the clipboard, the button still down, because that's where the state is that a random walk rarely reaches.
    if take.u8() & 1 == 1 {
        selecting.press(PagePoint { page: 4, x: 0.0, y: 0.0 }, 1, false);
        selecting.drag(PagePoint { page: 2, x: 1e9, y: 1e9 });
        // No check yet, so the copy has to wait for what comes next, with the selection free to move under it.
        for page in [2, 3, 4] {
            selecting.page_read(page, Arc::clone(text), false, &visible);
        }
        selecting.start_copy(false, PAGES, |_| false);
    }
    for _ in 0..POINTERS * 4 {
        let page = u32::from(take.u8()) % (PAGES + 2);
        let point = PagePoint { page, x: take.pick(POINTS), y: take.pick(POINTS) };
        let heavy = take.u8() & 7 == 0;
        match take.u8() % 14 {
            0 => selecting.press(point, u32::from(take.u8()), take.u8() & 1 == 1),
            1..=3 => selecting.drag(point),
            4 => selecting.release(),
            5 => selecting.set_range(Range { start: Place { page, at: take.u32() }, end: Place { page: page.saturating_add(u32::from(take.u8())), at: take.u32() } }),
            6 => selecting.clear(),
            7 => selecting.start_copy(take.u8() & 1 == 1, PAGES, |_| heavy),
            8 => selecting.advance_copy(PAGES, |_| heavy),
            9 => selecting.page_read(page, Arc::clone(if take.u8() & 1 == 1 { text } else { other }), take.u8() & 1 == 1, &visible),
            10 => selecting.page_failed(page, &visible),
            11 => selecting.drop_texts(),
            12 => {
                let _ = selecting.is_over_text(point);
            }
            _ => selecting.reset(),
        }
        for &on in &visible {
            let _ = selecting.layer_update(on);
        }
        let _ = selecting.take_copied();
        let _ = selecting.take_copy_signal();
        let _ = selecting.text_now(PAGES);
        let _ = selecting.can_offer(PAGES);
        let _ = selecting.notice();
        let _ = selecting.report();
        if let Some(range) = selecting.range() {
            assert!(range.start <= range.end, "{range:?}");
        }
        for urgency in [Urgency::Now, Urgency::WhenIdle] {
            if let Some(wanted) = selecting.wanted(urgency, &visible, PAGES, |_| heavy) {
                assert!(wanted.page <= u32::from(u8::MAX).saturating_mul(2) + PAGES, "{wanted:?}");
            }
        }
    }
    // Left to itself, with every page it asks for given to it, a copy comes to an end.
    selecting.release();
    for _ in 0..u32::from(u8::MAX) * 4 {
        let Some(wanted) = selecting.wanted(Urgency::Now, &visible, PAGES, |_| false).or_else(|| selecting.wanted(Urgency::WhenIdle, &visible, PAGES, |_| false)) else { break };
        selecting.page_read(wanted.page, Arc::clone(text), true, &visible);
        selecting.advance_copy(PAGES, |_| false);
    }
    assert!(!selecting.is_copying(), "a copy never came to rest");
}

fn check_pointer(texts: &One, take: &mut Take) {
    let mut selection = Selection::default();
    for _ in 0..POINTERS {
        let point = PagePoint { page: u32::from(take.u8() % 3), x: take.pick(POINTS), y: take.pick(POINTS) };
        match take.u8() % 5 {
            0 => selection.press(point, u32::from(take.u8()), take.u8() & 1 == 1),
            1 => selection.drag(point),
            2 => selection.release(),
            3 => selection.set_range(Range { start: Place { page: point.page, at: take.u32() }, end: Place { page: point.page.saturating_add(1), at: take.u32() } }),
            _ => selection.clear(),
        }
        if let Some(range) = selection.range(texts) {
            assert!(range.start <= range.end, "{range:?}");
            for page in 0..3 {
                if let (Some((start, end)), Some(text)) = (range.on_page(page), texts.page_text(page)) {
                    let _ = text.rects(start, end);
                    let mut copied = CopiedText::default();
                    let neighbours: Vec<&[recto_index::chrome::EdgeLine]> = neighbour_pages(page, 3).filter_map(|other| texts.page_text(other).map(PageText::edge_lines)).collect();
                    copied.add_page(page, text, &range, 256, &chrome_for(page, text, &range, &neighbours));
                    assert!(copied.text.len() <= 256);
                }
            }
        }
    }
}
