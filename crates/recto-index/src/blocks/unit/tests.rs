//! What a unit must hold to, with no file, helper or app: a made-up document of pages whose text and stored blocks are given, read through [`Pages`]. Each test names the failure it catches.

use super::*;
use crate::blocks::markdown;
use recto_core::locator::characters;
use recto_core::{Toc, TocLine};

/// A frame of a page 612 by 1000 points whose crop box starts at 0, shown upright: a box's middle `top` points down is `top` thousandths.
fn upright() -> Frame {
    Frame {
        turns: 0,
        left: 0.0,
        bottom: 0.0,
        right: 612.0,
        top: 1000.0,
    }
}

/// A block of `page` over characters `[start, end)`, whose middle is `top` thousandths down an upright page.
fn block(kind: Kind, page: u32, (start, end): (u32, u32), top: i16) -> Block {
    Block {
        kind,
        level: 1,
        page,
        start,
        end,
        bbox: [0, 995 - top, 100, 1005 - top],
        continues: false,
    }
}

struct FakePage {
    text: String,
    blocks: Vec<Block>,
    shown: bool,
    readable: bool,
}

struct Doc {
    pages: Vec<FakePage>,
    /// The pages read, in order.
    reads: Vec<u32>,
    /// Live time is spent after this many reads.
    over_after: Option<usize>,
    stale: bool,
}

impl Doc {
    fn new(pages: Vec<(&str, Vec<Block>)>) -> Self {
        Self {
            pages: pages
                .into_iter()
                .map(|(text, blocks)| FakePage {
                    text: text.to_string(),
                    blocks,
                    shown: true,
                    readable: true,
                })
                .collect(),
            reads: Vec::new(),
            over_after: None,
            stale: false,
        }
    }
}

impl Pages for Doc {
    fn pages(&self) -> u32 {
        self.pages.len() as u32
    }
    fn read(&mut self, page: u32, chars: &mut Vec<char>, hidden: &mut Vec<(u32, u32)>) -> bool {
        self.reads.push(page);
        chars.clear();
        hidden.clear();
        let one = &self.pages[page as usize];
        chars.extend(one.text.chars());
        one.readable
    }
    fn blocks(&mut self, page: u32, _: u32) -> Result<Vec<Block>, Fault> {
        if self.stale {
            return Err(Fault);
        }
        Ok(self.pages[page as usize].blocks.clone())
    }
    fn frame(&mut self, _: u32) -> Option<Frame> {
        Some(upright())
    }
    fn shown(&self, page: u32) -> bool {
        self.pages[page as usize].shown
    }
    fn over(&mut self) -> bool {
        self.over_after.is_some_and(|after| self.reads.len() >= after)
    }
}

fn toc(lines: &[(u32, u8, u16)]) -> Toc {
    let entries: Vec<TocLine<'_>> = lines
        .iter()
        .map(|(page, depth, _)| TocLine {
            title: "t",
            page: *page,
            parent: recto_core::NONE,
            depth: *depth,
        })
        .collect();
    let tops: Vec<u16> = lines.iter().map(|line| line.2).collect();
    Toc::from_lines(&entries, false).with_tops(&tops)
}

fn ask<'a>(grain: Grain, page: u32, start: Option<u32>, sections: Option<&'a Sections>, max_chars: u32) -> Ask<'a> {
    Ask {
        grain,
        page,
        start,
        from_here: false,
        max_chars,
        sections,
    }
}

fn unit(outcome: Result<Outcome, Fault>) -> Box<Unit> {
    match outcome.expect("blocks open") {
        Outcome::Unit(unit) => unit,
        Outcome::Page(why) => panic!("answered the page: {why:?}"),
    }
}

fn joined(unit: &Unit) -> String {
    as_text(&unit.given).0
}

/// Two chapters on page 0 (an alpha heading at top 100, a body, a beta heading at 500, a body), a page 1 with a nested entry, and a page 2 with no stored blocks that the next chapter starts on.
fn book() -> (Doc, Sections) {
    let page0 = "Alpha heading\nalpha body\nBeta heading\nbeta body";
    let page1 = "Sub heading\nsub body";
    let page2 = "Gamma, and no blocks here";
    let doc = Doc::new(vec![
        (
            page0,
            vec![
                block(Kind::Heading, 0, (0, 13), 100),
                block(Kind::Paragraph, 0, (14, 24), 200),
                block(Kind::Heading, 0, (25, 37), 500),
                block(Kind::Paragraph, 0, (38, 47), 600),
            ],
        ),
        (page1, vec![block(Kind::Heading, 1, (0, 11), 100), block(Kind::Paragraph, 1, (12, 20), 200)]),
        (page2, vec![]),
    ]);
    // Alpha; Beta; Beta's sub-section on page 1; Gamma, a chapter on page 2.
    let sections = Sections::new(toc(&[(0, 0, 100), (0, 0, 500), (1, 1, 100), (2, 0, 0)]).entries());
    (doc, sections)
}

/// Failure: with the page's height only, a page whose crop box starts above 0 (or a turned page) puts every block at the top or the bottom, so in the wrong section. The same box on the same page is 250
/// or 750 thousandths down by turn, as the page is shown.
#[test]
fn a_blocks_place_is_on_the_page_as_shown() {
    let boxed = |bbox| Block {
        bbox,
        ..block(Kind::Paragraph, 0, (0, 1), 0)
    };
    // A 612 by 792 crop box whose corner is at (100, 792) in the page's own coordinates.
    let frame = |turns| Frame {
        turns,
        left: 100.0,
        bottom: 792.0,
        right: 712.0,
        top: 1584.0,
    };
    // Middle at 594 above the crop box's bottom and 153 right of its left: a quarter of the shown page down for turns 0 and 1, three quarters for 2 and 3.
    let one = boxed([243, 1376, 263, 1396]);
    for (turns, down) in [(0u8, 250u16), (1, 250), (2, 750), (3, 750)] {
        assert_eq!(Place::of_block(&one, frame(turns)).top, down, "turned {turns} quarters");
    }
    assert_eq!(
        Place::of_block(&one, Frame { right: 100.0, ..frame(0) }).top,
        Place::MIDDLE,
        "a frame with no area"
    );
}

/// Failure: a section takes the previous one's tail or loses its own first paragraph, or an entry on a page with no blocks leaves its page out. Two entries on one page (tops 100 and 500), a nesting, and
/// an end entry on a page with no blocks (the whole page is given, and counted).
#[test]
fn a_section_is_cut_at_the_first_block_at_or_after_its_entry() {
    let (mut doc, sections) = book();
    let alpha = unit(build(&mut doc, &ask(Grain::Section, 0, Some(14), Some(&sections), 10_000)));
    assert_eq!(joined(&alpha), "Alpha heading\nalpha body\n", "up to the beta heading's first character");
    assert_eq!(alpha.entry, Some(0));

    // Beta is chapter and section: page 0 from its heading, all of page 1 (its nested entry is inside it), and page 2 up to the next chapter's cut, which is the whole page (no blocks on it).
    let (mut doc, _) = book();
    let beta = unit(build(&mut doc, &ask(Grain::Section, 0, Some(40), Some(&sections), 10_000)));
    assert_eq!(joined(&beta), "Beta heading\nbeta body\nSub heading\nsub body\nGamma, and no blocks here");
    assert_eq!(beta.counts.whole_edge_pages, 1);
    assert_eq!(beta.counts.pages_read, 4, "its three pages and the previous chapter's page, read to name it");
    assert!(!beta.truncated);

    // The nested entry is its own section: page 1, and page 2 up to the next chapter's start, which has no blocks to cut at, so the whole page.
    let (mut doc, _) = book();
    let sub = unit(build(&mut doc, &ask(Grain::Section, 1, Some(14), Some(&sections), 10_000)));
    assert_eq!(joined(&sub), "Sub heading\nsub body\nGamma, and no blocks here");
    assert_eq!(sub.headings, vec![1, 2], "its chapter, then itself");
    // As a chapter it is Beta's.
    let (mut doc, _) = book();
    let chapter = unit(build(&mut doc, &ask(Grain::Chapter, 1, Some(14), Some(&sections), 10_000)));
    assert_eq!(chapter.entry, Some(1));
    assert_eq!(chapter.pages, 2, "by its entries: pages 0 and 1 (the next chapter starts at the top of page 2)");
}

/// Failure: an agent following `more` with `--from-here` misses or repeats text. For every cap from 1 to the section's length, the pieces given over successive answers are the whole section, once.
#[test]
fn following_more_gives_the_whole_unit_once() {
    let (mut doc, sections) = book();
    let one = unit(build(&mut doc, &ask(Grain::Section, 0, Some(40), Some(&sections), 10_000)));
    let whole: String = one.given.iter().flat_map(|piece| piece.text.iter()).collect();
    for max in 1..=whole.chars().count() as u32 + 1 {
        let (mut doc, _) = book();
        let mut got = String::new();
        let mut place = (0u32, 40u32, false);
        for round in 0..200 {
            let mut one = ask(Grain::Section, place.0, Some(place.1), Some(&sections), max);
            one.from_here = place.2;
            let answer = unit(build(&mut doc, &one));
            assert!(answer.chars_given <= max, "cap {max}: {} given", answer.chars_given);
            let text: String = answer.given.iter().flat_map(|piece| piece.text.iter()).collect();
            got.push_str(&text);
            if !answer.truncated {
                break;
            }
            let more = answer.refs.more.expect("a cut answer says where it goes on");
            place = (more.page, more.range.offsets().0, true);
            assert!(round < 199, "cap {max}: never ends");
        }
        assert_eq!(got, whole, "cap {max}");
    }
}

/// Failure: the cut is in the middle of a piece when a whole one fits, or a first piece over the cap alone is cut at a place that isn't a line end when there is one. The order is the end of the last whole
/// piece, then the last line end, then the cap.
#[test]
fn the_cut_is_at_a_block_end_then_a_line_end_then_the_cap() {
    let (mut doc, sections) = book();
    // The beta heading (12), the gap line end (1), and a body of 9 that doesn't fit in 14.
    let answer = unit(build(&mut doc, &ask(Grain::Section, 0, Some(25), Some(&sections), 14)));
    assert_eq!((answer.chars_given, answer.truncated), (13, true));
    assert_eq!(answer.refs.more.map(|more| more.range.offsets().0), Some(38));

    let mut doc = Doc::new(vec![("line one\nline two", vec![block(Kind::Paragraph, 0, (0, 17), 100)])]);
    let answer = unit(build(&mut doc, &ask(Grain::Paragraph, 0, Some(0), None, 12)));
    assert_eq!(joined(&answer), "line one", "at the last line end inside the cap");
    let mut doc = Doc::new(vec![("abcdefghij", vec![block(Kind::Paragraph, 0, (0, 10), 100)])]);
    let answer = unit(build(&mut doc, &ask(Grain::Paragraph, 0, Some(0), None, 4)));
    assert_eq!(joined(&answer), "abcd", "no line end: at the cap");
    assert_eq!(answer.refs.more.map(|more| more.range.offsets()), Some((4, 10)));
}

/// Failure: a section's own heading is `#` whatever its depth, or a stray heading takes an entry's depth.
#[test]
fn a_heading_at_an_entry_takes_its_depth() {
    let text = "Top\nbody\nNested\nbody two\nStray\nlast";
    let mut doc = Doc::new(vec![(
        text,
        vec![
            block(Kind::Heading, 0, (0, 3), 100),
            block(Kind::Paragraph, 0, (4, 8), 150),
            block(Kind::Heading, 0, (9, 15), 300),
            block(Kind::Paragraph, 0, (16, 24), 350),
            Block {
                level: 3,
                ..block(Kind::Heading, 0, (25, 30), 600)
            },
            block(Kind::Paragraph, 0, (31, 35), 650),
        ],
    )]);
    let sections = Sections::new(toc(&[(0, 0, 100), (0, 1, 300)]).entries());
    let answer = unit(build(&mut doc, &ask(Grain::Chapter, 0, Some(5), Some(&sections), 10_000)));
    let rendered = markdown::render(&render_pieces(&answer.given));
    assert!(rendered.starts_with("# Top\n\nbody\n\n## Nested"), "{rendered}");
    assert!(
        rendered.contains("\n\n### Stray\n"),
        "a heading no entry starts keeps its own level: {rendered}"
    );
}

/// Failure: the per-block scan of every entry is back (S1's review: 65,536 entries by 512 blocks by 64 pages). With a page of the most entries and the most blocks, the steps taken stay within a small
/// multiple of the entries and blocks added, not multiplied. It asserts the rule, not a speed.
#[test]
fn boundaries_are_one_sweep_a_page() {
    use crate::blocks::sections::MAX_ENTRIES;
    let lines: Vec<(u32, u8, u16)> = (0..MAX_ENTRIES + 1).map(|at| (0, if at == 0 { 0 } else { 1 }, (at % 1000) as u16)).collect();
    let sections = Sections::new(toc(&lines).entries());
    assert_eq!(sections.len(), MAX_ENTRIES, "the entry past the cap is left out");
    let blocks: Vec<Block> = (0..crate::blocks::MAX_BLOCKS as u32)
        .map(|at| block(Kind::Heading, 0, (at * 4, at * 4 + 3), (at * 1000 / 512) as i16))
        .collect();
    let text = "abc ".repeat(crate::blocks::MAX_BLOCKS);
    let mut doc = Doc::new(vec![(&text, blocks)]);
    take_steps();
    let answer = unit(build(&mut doc, &ask(Grain::Chapter, 0, None, Some(&sections), 10_000)));
    let steps = take_steps();
    assert!(answer.counts.blocks > 0);
    let bound = 4 * (MAX_ENTRIES + crate::blocks::MAX_BLOCKS) as u64;
    assert!(
        steps <= bound,
        "{steps} steps for {MAX_ENTRIES} entries and {} blocks on one page (at most {bound})",
        crate::blocks::MAX_BLOCKS
    );
}

/// Failure: a 500-page chapter reads 500 pages, or a slow file holds an agent's `get` without bound. At 64 pages all are read; at 65 the answer is cut and says where to go on, at the 65th page's first
/// characters; and when live time is spent after 3 reads it stops there. Nothing sleeps: the page source says when.
#[test]
fn an_answer_reads_at_most_64_pages_and_stops_when_live_reads_run_out() {
    let make = |pages: usize| {
        let docs: Vec<(String, Vec<Block>)> = (0..pages as u32)
            .map(|page| (format!("page {page} words"), vec![block(Kind::Paragraph, page, (0, 12), 100)]))
            .collect();
        Doc::new(docs.iter().map(|(text, blocks)| (text.as_str(), blocks.clone())).collect())
    };
    let sections = Sections::new(toc(&[(0, 0, 0)]).entries());
    let mut doc = make(64);
    let answer = unit(build(&mut doc, &ask(Grain::Chapter, 0, None, Some(&sections), 65_536)));
    assert_eq!((answer.counts.pages_read, answer.truncated), (64, false));

    let mut doc = make(65);
    let answer = unit(build(&mut doc, &ask(Grain::Chapter, 0, None, Some(&sections), 65_536)));
    assert!(answer.truncated);
    let more = answer.refs.more.expect("it says where to go on");
    assert_eq!((more.page, more.range.offsets().0), (64, 0));
    assert!(doc.reads.len() <= 65, "{} pages read", doc.reads.len());
    assert_eq!(doc.reads.iter().filter(|page| **page == 64).count(), 1, "page 65 is read once, to name it");
    assert!(answer.given.iter().all(|piece| piece.page < 64), "its text is not in the answer");

    let mut doc = make(10);
    doc.over_after = Some(3);
    let answer = unit(build(&mut doc, &ask(Grain::Chapter, 0, None, Some(&sections), 65_536)));
    assert!(answer.truncated);
    assert_eq!(answer.refs.more.map(|more| more.page), Some(3));
    assert_eq!(answer.given.iter().map(|piece| piece.page).collect::<Vec<_>>(), vec![0, 1, 2]);
}

/// Failure: characters outside any stored block are given as a "paragraph" (decision C). A page reference takes the first block, characters in no block say `no_block_here`, and the next and previous
/// paragraph skip pages with no blocks and pages the tab doesn't show, and are none past the page cap. A ref is the page's own check of those characters.
#[test]
fn a_paragraph_is_only_where_a_block_checks() {
    let two = vec![block(Kind::Paragraph, 0, (0, 5), 100), block(Kind::Paragraph, 0, (10, 15), 200)];
    let mut doc = Doc::new(vec![
        ("hello.....world.....", two),
        ("nothing stored", vec![]),
        ("hidden by edits", vec![block(Kind::Paragraph, 2, (0, 5), 100)]),
        ("later words", vec![block(Kind::Paragraph, 3, (0, 11), 100)]),
    ]);
    doc.pages[2].shown = false;
    let none = build(&mut doc, &ask(Grain::Paragraph, 0, Some(7), None, 100));
    assert_eq!(none, Ok(Outcome::Page(Why::NoBlockHere)));
    let first = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 100)));
    assert_eq!(joined(&first), "hello");
    assert_eq!(first.refs.prev, None);
    assert_eq!(first.refs.next.map(|next| next.range), characters("hello.....world.....", 10, 15).ok());
    let last = unit(build(&mut doc, &ask(Grain::Paragraph, 0, Some(12), None, 100)));
    assert_eq!(joined(&last), "world");
    let next = last.refs.next.expect("the next shown page with a block");
    assert_eq!((next.page, next.range.offsets()), (3, (0, 11)));
    assert_eq!(last.refs.up, last.refs.this);

    // Only the last page has a block, 69 pages on: past the cap on pages read, there is no next.
    let texts: Vec<String> = (0..70).map(|page| format!("page {page}")).collect();
    let mut far = Doc::new(
        texts
            .iter()
            .enumerate()
            .map(|(page, text)| {
                (
                    text.as_str(),
                    if page == 0 || page == 69 {
                        vec![block(Kind::Paragraph, page as u32, (0, 6), 100)]
                    } else {
                        vec![]
                    },
                )
            })
            .collect(),
    );
    let answer = unit(build(&mut far, &ask(Grain::Paragraph, 0, None, None, 100)));
    assert_eq!(answer.refs.next, None);
    assert!(far.reads.len() <= 64);
}

/// Failure: blocks that don't open as the page's (stale or damaged) are used anyway instead of being reported to be thrown away.
#[test]
fn blocks_that_dont_open_are_a_fault_not_an_answer() {
    let (mut doc, sections) = book();
    doc.stale = true;
    assert_eq!(build(&mut doc, &ask(Grain::Section, 0, Some(5), Some(&sections), 100)), Err(Fault));
}

/// Failure: the hidden stretches of the page are moved wrongly into the pieces or the joined text, so hidden words show as plain ones.
#[test]
fn hidden_stretches_follow_the_text_into_the_pieces_and_the_joined_text() {
    assert_eq!(clip_spans(&[(2, 6), (10, 12)], 4, 11), vec![(0, 2), (6, 7)]);
    // A stretch wholly before the range (or after it) is skipped. `then_some` computed its offsets first and overflowed in a debug build, which made `get` an internal error on a page with hidden text.
    assert_eq!(clip_spans(&[(0, 2), (20, 30)], 5, 9), Vec::<(u32, u32)>::new());
    let block = Block {
        start: 10,
        end: 15,
        ..block(Kind::Paragraph, 0, (10, 15), 100)
    };
    assert_eq!(markdown::spans_within(&block, &[(0, 4), (12, 13), (40, 50)]), vec![(2, 3)]);
    let given = |page, text: &str, hidden| Given {
        page,
        kind: Kind::Paragraph,
        level: 0,
        depth: None,
        text: text.chars().collect(),
        hidden,
    };
    let (text, hidden) = as_text(&[given(0, "ab", vec![(1, 2)]), given(1, "cd", vec![(0, 1)])]);
    assert_eq!((text.as_str(), hidden), ("ab\ncd", vec![(1, 2), (3, 4)]));
}
