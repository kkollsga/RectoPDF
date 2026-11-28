//! What one `get` of a paragraph or a section holds while it reads, counted by live bytes with a counting allocator (a freed block stays in `phys_footprint` on this macOS, so the footprint can't tell held
//! memory from freed). Its own binary, so nothing else allocates beside it.

#![allow(unsafe_code)]

use recto_core::nav::Frame;
use recto_core::{Toc, TocLine};
use recto_index::blocks::sections::Sections;
use recto_index::blocks::unit::{Ask, Fault, Grain, Outcome, Pages, build};
use recto_index::blocks::{Block, Kind};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator and only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
        PEAK.fetch_max(live, Ordering::SeqCst);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            let live = LIVE.fetch_add(new_size - layout.size(), Ordering::SeqCst) + new_size - layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
        }
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const PAGES: u32 = 64;
const CHARS: usize = 2_000_000;

/// A document of [`PAGES`] pages of [`CHARS`] characters each. Blocks are on the first and the last pages only.
struct Big;

impl Pages for Big {
    fn pages(&self) -> u32 {
        PAGES
    }
    fn read(&mut self, _: u32, chars: &mut Vec<char>, hidden: &mut Vec<(u32, u32)>) -> bool {
        chars.clear();
        hidden.clear();
        chars.extend(std::iter::repeat_n('a', CHARS));
        true
    }
    fn blocks(&mut self, page: u32, _: u32) -> Result<Vec<Block>, Fault> {
        let at = |start: u32, end: u32, bbox: [i16; 4]| Block {
            kind: Kind::Paragraph,
            level: 0,
            page,
            start,
            end,
            bbox,
            continues: false,
        };
        // The first page has a block at its start and one at its tail (low on the page, at about 917 thousandths); the last has one at its start.
        Ok(match page {
            0 => vec![at(0, 100, [0, 100, 100, 200]), at(CHARS as u32 - 100, CHARS as u32, [0, 60, 100, 70])],
            p if p == PAGES - 1 => vec![at(0, 100, [0, 100, 100, 200])],
            _ => Vec::new(),
        })
    }
    fn frame(&mut self, _: u32) -> Option<Frame> {
        Some(Frame {
            turns: 0,
            left: 0.0,
            bottom: 0.0,
            right: 612.0,
            top: 792.0,
        })
    }
    fn shown(&self, _: u32) -> bool {
        true
    }
    fn over(&mut self) -> bool {
        false
    }
}

fn peak_of(step: impl FnOnce()) -> usize {
    PEAK.store(LIVE.load(Ordering::SeqCst), Ordering::SeqCst);
    let before = LIVE.load(Ordering::SeqCst);
    step();
    PEAK.load(Ordering::SeqCst) - before
}

/// Failure: an answer holds every page it reads (64 pages of 2,000,000 characters is 512 MB as `Vec<char>`). A section cut at 65,536 characters and a paragraph whose next block is 63 pages on each read
/// pages one at a time: the peak is about one page's characters (8 MB) and what's given, whatever the pages read. The bound is stated beside the number measured (`MEASURED`).
#[test]
fn one_answer_holds_one_page_and_what_it_gives() {
    // One chapter, whose entry is low on the first page (900 thousandths): it begins at that page's tail and goes on over the next page, which is 2,000,000 characters.
    let toc = Toc::from_lines(
        &[TocLine {
            title: "t",
            page: 0,
            parent: recto_core::NONE,
            depth: 0,
        }],
        false,
    )
    .with_tops(&[900]);
    let sections = Sections::new(toc.entries());
    let one_page = CHARS * std::mem::size_of::<char>();
    let mut section_peak = 0;
    let mut paragraph_peak = 0;
    let mut pages_read = 0;
    section_peak = section_peak.max(peak_of(|| {
        let ask = Ask {
            grain: Grain::Chapter,
            page: 0,
            start: Some(CHARS as u32 - 50),
            from_here: false,
            entry: None,
            max_chars: 65_536,
            sections: Some(&sections),
            chrome: recto_index::blocks::unit::heads::Rule::default(),
        };
        let built = build(&mut Big, &ask);
        let Ok(Outcome::Unit(unit)) = built else { panic!("a unit, not {built:?}") };
        assert_eq!(unit.chars_given, 100, "the tail block; the next page doesn't fit whole");
        assert!(unit.counts.pages_read >= 2, "the unit crosses onto the next page: {:?}", unit.counts);
        let more = unit.refs.more.expect("a way on");
        let again = Ask {
            page: more.page,
            start: Some(more.range.offsets().0),
            from_here: true,
            ..ask
        };
        let Ok(Outcome::Unit(unit)) = build(&mut Big, &again) else { panic!("a unit") };
        assert_eq!(unit.chars_given, 65_536, "going on from there, the rest of the page is cut at the cap");
    }));
    paragraph_peak = paragraph_peak.max(peak_of(|| {
        let ask = Ask {
            grain: Grain::Paragraph,
            page: 0,
            start: Some(CHARS as u32 - 50),
            from_here: false,
            entry: None,
            max_chars: 65_536,
            sections: None,
            chrome: recto_index::blocks::unit::heads::Rule::default(),
        };
        let Ok(Outcome::Unit(unit)) = build(&mut Big, &ask) else { panic!("a unit") };
        pages_read = unit.counts.pages_read;
        assert!(unit.refs.next.is_some(), "the last page's block is the next, 63 pages on");
    }));
    eprintln!("MEASURED: one page {one_page} bytes; section peak {section_peak}; paragraph peak {paragraph_peak} over {pages_read} pages read");
    assert_eq!(pages_read, PAGES, "the paragraph's neighbour is looked for over every page up to the cap");
    // One page's characters, the given text (65,536 characters, 256 KiB) and small things: under a page and a half. Holding two pages would be over.
    let bound = one_page + one_page / 2;
    assert!(
        section_peak < bound && paragraph_peak < bound,
        "section {section_peak}, paragraph {paragraph_peak}, bound {bound}"
    );
}
