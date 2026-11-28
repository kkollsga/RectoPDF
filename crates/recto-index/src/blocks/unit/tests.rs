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
    fn has_text(&mut self, page: u32) -> bool {
        self.pages[page as usize].readable
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
        entry: None,
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
        let mut entry = None;
        for round in 0..200 {
            let mut one = ask(Grain::Section, place.0, Some(place.1), Some(&sections), max);
            one.from_here = place.2;
            one.entry = entry;
            let answer = unit(build(&mut doc, &one));
            assert!(answer.chars_given <= max, "cap {max}: {} given", answer.chars_given);
            assert_eq!(
                answer.entry,
                Some(1),
                "cap {max}, round {round}: every round is the section the first answer was of"
            );
            let text: String = answer.given.iter().flat_map(|piece| piece.text.iter()).collect();
            got.push_str(&text);
            if !answer.truncated {
                break;
            }
            let more = answer.refs.more.expect("a cut answer says where it goes on");
            place = (more.page, more.range.offsets().0, true);
            entry = answer.entry.map(|entry| entry as u32);
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
    let rendered = as_markdown(&answer.given);
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
        joins: false,
    };
    let (text, hidden) = as_text(&[given(0, "ab", vec![(1, 2)]), given(1, "cd", vec![(0, 1)])]);
    assert_eq!((text.as_str(), hidden), ("ab\ncd", vec![(1, 2), (3, 4)]));
}

/// Failure (review B1): a section is chosen by place but cut by character order, so on a page whose columns run against it a `more` that lands in a block sitting higher than its entry picks the
/// section before, and the rest of the section is lost. Four blocks whose places are 100, 700, 500 and 200 in character order, entries at 100 and 500: following `more` from any cap stays in the
/// second section, every round, and gives it whole once.
#[test]
fn a_more_in_a_block_above_its_entry_stays_in_its_section() {
    let text = "AAA\nBBBB\nCCC\nDDDD";
    let blocks = || {
        vec![
            block(Kind::Heading, 0, (0, 3), 100),
            block(Kind::Paragraph, 0, (4, 8), 700),
            block(Kind::Heading, 0, (9, 12), 500),
            block(Kind::Paragraph, 0, (13, 17), 200),
        ]
    };
    let sections = Sections::new(toc(&[(0, 0, 100), (0, 0, 500)]).entries());
    let mut whole = String::new();
    for max in 1..=14u32 {
        let mut doc = Doc::new(vec![(text, blocks())]);
        let mut got = String::new();
        let mut place = (4u32, false);
        for round in 0..40 {
            let mut one = ask(Grain::Section, 0, Some(place.0), Some(&sections), max);
            one.from_here = place.1;
            let answer = unit(build(&mut doc, &one));
            assert_eq!(answer.entry, Some(1), "cap {max}, round {round}: the section stayed the second");
            got.extend(answer.given.iter().flat_map(|piece| piece.text.iter()));
            if !answer.truncated {
                break;
            }
            place = (answer.refs.more.expect("a way on").range.offsets().0, true);
        }
        if whole.is_empty() {
            whole = got.clone();
        }
        assert_eq!(got, whole, "cap {max}");
    }
    assert_eq!(whole, "BBBB\nCCC\nDDDD");
}

/// Failure (review B2): a blank page right after the cap gives `truncated` with no `more`, as the one naming read is spent on a page with nothing to name. A page the index says has no text isn't a read:
/// it is skipped (at most 8), and `more` is where the next page with text begins.
#[test]
fn a_blank_page_after_the_cap_still_gives_a_more() {
    let texts: Vec<String> = (0..70u32).map(|page| format!("page {page} words")).collect();
    let mut doc = Doc::new(
        texts
            .iter()
            .enumerate()
            .map(|(page, text)| (text.as_str(), vec![block(Kind::Paragraph, page as u32, (0, 12), 100)]))
            .collect(),
    );
    doc.pages[64].readable = false;
    let sections = Sections::new(toc(&[(0, 0, 0)]).entries());
    let answer = unit(build(&mut doc, &ask(Grain::Chapter, 0, None, Some(&sections), 65_536)));
    assert!(answer.truncated);
    assert_eq!(answer.refs.more.map(|more| more.page), Some(65), "the blank page 65 is passed over");
    assert!(
        doc.reads.iter().filter(|page| **page == 64).count() == 0,
        "a page known blank isn't read to be named"
    );
}

/// Failure (review round 2): a character before its page's first cut is put in the section that starts on that page (the page's top), though it comes before the section's first character. It is in the section
/// just before that entry, which started on an earlier page.
#[test]
fn a_character_before_the_first_cut_on_its_page_is_in_the_section_before() {
    let mut doc = Doc::new(vec![
        ("first page words", vec![block(Kind::Paragraph, 0, (0, 16), 100)]),
        (
            "stray gap\nHeading\nbody",
            vec![block(Kind::Heading, 1, (10, 17), 100), block(Kind::Paragraph, 1, (18, 22), 200)],
        ),
    ]);
    let sections = Sections::new(toc(&[(0, 0, 0), (1, 0, 0)]).entries());
    let answer = unit(build(&mut doc, &ask(Grain::Section, 1, Some(2), Some(&sections), 10_000)));
    assert_eq!(answer.entry, Some(0), "the stray text before the heading belongs to the first section");
    let answer = unit(build(&mut doc, &ask(Grain::Section, 1, Some(11), Some(&sections), 10_000)));
    assert_eq!(answer.entry, Some(1));
}

/// Failure (review round 2): following `more` into a nested entry's page asks for that nested section, so the rest of the section the `more` came from, after the nested one, is lost. Beta holds a
/// nested Sub, then goes on with Tail, then Gamma: following Beta's `more` at every cap gives all of Beta, once, as Beta, every round. A number that isn't an entry of these contents, or one whose range
/// doesn't reach the reference's page, is ignored, never believed.
#[test]
fn going_on_continues_the_section_the_more_came_from_through_its_nested_ones() {
    let doc = || {
        Doc::new(vec![
            (
                "Alpha heading\nalpha body\nBeta heading\nbeta body",
                vec![
                    block(Kind::Heading, 0, (0, 13), 100),
                    block(Kind::Paragraph, 0, (14, 24), 200),
                    block(Kind::Heading, 0, (25, 37), 500),
                    block(Kind::Paragraph, 0, (38, 47), 600),
                ],
            ),
            (
                "Sub heading\nsub body",
                vec![block(Kind::Heading, 1, (0, 11), 100), block(Kind::Paragraph, 1, (12, 20), 200)],
            ),
            (
                "Tail heading\ntail body",
                vec![block(Kind::Heading, 2, (0, 12), 100), block(Kind::Paragraph, 2, (13, 22), 200)],
            ),
            ("Gamma heading", vec![block(Kind::Heading, 3, (0, 13), 100)]),
        ])
    };
    // Alpha, Beta; Beta's Sub on page 1 and Tail on page 2 (both depth 1); Gamma, a chapter, on page 3.
    let sections = Sections::new(toc(&[(0, 0, 100), (0, 0, 500), (1, 1, 100), (2, 1, 100), (3, 0, 100)]).entries());
    let mut whole_doc = doc();
    let whole: String = unit(build(&mut whole_doc, &ask(Grain::Section, 0, Some(40), Some(&sections), 10_000)))
        .given
        .iter()
        .flat_map(|piece| piece.text.iter())
        .collect();
    assert!(whole.ends_with("tail body"), "Beta runs through Sub and Tail: {whole:?}");
    for max in 1..=whole.chars().count() as u32 + 1 {
        let mut doc = doc();
        let (mut got, mut place, mut entry) = (String::new(), (0u32, 40u32, false), None);
        for round in 0..300 {
            let mut one = ask(Grain::Section, place.0, Some(place.1), Some(&sections), max);
            (one.from_here, one.entry) = (place.2, entry);
            let answer = unit(build(&mut doc, &one));
            assert_eq!(answer.entry, Some(1), "cap {max}, round {round}");
            got.extend(answer.given.iter().flat_map(|piece| piece.text.iter()));
            if !answer.truncated {
                break;
            }
            let more = answer.refs.more.expect("a way on");
            (place, entry) = ((more.page, more.range.offsets().0, true), answer.entry.map(|entry| entry as u32));
        }
        assert_eq!(got, whole, "cap {max}");
    }
    // Not an entry, or an entry that doesn't reach the page: the anchor decides, as with no number.
    let mut doc = doc();
    for bad in [99u32, 4, u32::MAX] {
        let mut one = ask(Grain::Section, 1, Some(14), Some(&sections), 10_000);
        (one.from_here, one.entry) = (true, Some(bad));
        assert_eq!(unit(build(&mut doc, &one)).entry, Some(2), "{bad}: ignored");
    }
}

/// A one-block page whose paragraph goes on at the top of the next page when `continues`.
fn leg(page: u32, text: &str, continues: bool) -> (String, Vec<Block>) {
    let len = text.chars().count() as u32;
    (
        text.to_string(),
        vec![Block {
            continues,
            ..block(Kind::Paragraph, page, (0, len), 100)
        }],
    )
}

fn legs(pages: &[(String, Vec<Block>)]) -> Doc {
    Doc::new(pages.iter().map(|(text, blocks)| (text.as_str(), blocks.clone())).collect())
}

/// Failure: a paragraph that goes on at the top of the next page is given as its first piece only (so an agent reads half a sentence and is told to go on), or its pieces are two paragraphs, or `continues`
/// stays set after the last piece, or `refs.next` still names the continuation. Two pages: the first's paragraph continues into the second's first block; the answer is both, as one paragraph, `continues`
/// is false, the unit spans two pages, and `refs.next` is the block after the continuation, on the second page.
#[test]
fn a_paragraph_that_continues_is_joined_with_its_rest_on_the_next_page() {
    let mut doc = legs(&[leg(0, "the quay was measured", true), leg(1, "twice in spring.", false)]);
    doc.pages[1].blocks.push(block(Kind::Paragraph, 1, (17, 17), 300));
    doc.pages[1].text.push_str(" Next one.");
    doc.pages[1].blocks[1] = block(Kind::Paragraph, 1, (17, 26), 300);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!(joined(&one), "the quay was measured\ntwice in spring.");
    assert_eq!((one.pages, one.continues, one.truncated), (2, false, false));
    assert_eq!((one.counts.blocks, one.chars, one.chars_given), (2, 37, 37));
    assert_eq!(one.refs.this.map(|mark| mark.page), Some(0));
    let next = one.refs.next.expect("the block after the continuation");
    assert_eq!((next.page, next.range.offsets()), (1, (17, 26)));
    let markdown = as_markdown(&one.given);
    assert_eq!(markdown, "the quay was measured\ntwice in spring.", "one paragraph, not two");
    // Asked from the continuation, the answer is the continuation alone: a unit starts at its own first block.
    let rest = unit(build(&mut doc, &ask(Grain::Paragraph, 1, Some(0), None, 1000)));
    assert_eq!((joined(&rest), rest.pages, rest.continues), ("twice in spring.".to_string(), 1, false));
}

/// Failure: only the first continuation is followed, so a paragraph over three pages is cut at the second; or a page the tab doesn't show is read into it. Pieces on pages 0, 2 and 3 with page 1 not shown (the tab
/// removed it): all are given, in order, and the unit spans pages 0 to 3.
#[test]
fn a_paragraph_over_three_pages_is_joined_over_a_page_the_tab_doesnt_show() {
    let mut doc = legs(&[
        leg(0, "first part", true),
        leg(1, "removed", true),
        leg(2, "middle part", true),
        leg(3, "last part", false),
        leg(4, "another paragraph", false),
    ]);
    doc.pages[1].shown = false;
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!(joined(&one), "first part\nmiddle part\nlast part");
    assert_eq!((one.pages, one.continues, one.counts.blocks), (4, false, 3));
    assert_eq!(one.refs.next.map(|next| next.page), Some(4));
    assert_eq!(as_markdown(&one.given), "first part\nmiddle part\nlast part");
    assert!(!doc.reads.contains(&1));
}

/// Failure: a page with text and no stored blocks is passed over, so the chain joins pieces across text it can't place (a table, a figure's words) as if the paragraph ran through it. Page 1 has text and no blocks:
/// the answer is the first piece alone, `continues` with `more` at the start of page 1, and `next` is the first block after.
#[test]
fn a_page_with_text_and_no_blocks_stops_the_join_and_names_where_it_goes_on() {
    let mut doc = legs(&[
        leg(0, "first part", true),
        ("nothing stored".to_string(), vec![]),
        leg(2, "middle part", true),
        leg(3, "last part", false),
    ]);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!(joined(&one), "first part");
    assert_eq!((one.pages, one.chars, one.continues, one.truncated), (1, 10, true, false));
    let more = one.refs.more.expect("where it goes on");
    assert_eq!((more.page, more.range.offsets().0), (1, 0));
    assert_eq!(one.refs.next.map(|next| next.page), Some(2));
}

/// Failure: the text between a joined paragraph's pieces vanishes unsaid. Page 0 ends with a footer after the paragraph and page 1 begins with a running head before its rest: neither is given, both are counted
/// in `chars_outside_blocks` (the footer's and the head's characters, no more).
#[test]
fn the_text_between_a_joined_paragraphs_pieces_is_counted_and_not_given() {
    let mut doc = Doc::new(vec![
        (
            "the quay\nfooter 1",
            vec![Block {
                continues: true,
                ..block(Kind::Paragraph, 0, (0, 8), 100)
            }],
        ),
        ("HEAD\nwas measured", vec![block(Kind::Paragraph, 1, (5, 17), 100)]),
    ]);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!(joined(&one), "the quay\nwas measured");
    assert_eq!(
        one.counts.chars_outside_blocks,
        "\nfooter 1".chars().count() as u32 + "HEAD\n".chars().count() as u32
    );
}

/// Failure: a paragraph with another block after it on its page is joined with the next page's first block anyway, so an agent is given text that isn't its rest. Page 0 holds A (continues) then B; page 1
/// holds A' then C: the answer is A alone, `continues`, `more` names where it goes on, and `next` is B.
#[test]
fn a_paragraph_that_isnt_its_pages_last_block_is_given_alone() {
    let mut doc = Doc::new(vec![
        (
            "aaaaa bbbbb",
            vec![
                Block {
                    continues: true,
                    ..block(Kind::Paragraph, 0, (0, 5), 100)
                },
                block(Kind::Paragraph, 0, (6, 11), 300),
            ],
        ),
        (
            "rest ccccc",
            vec![block(Kind::Paragraph, 1, (0, 4), 100), block(Kind::Paragraph, 1, (5, 10), 300)],
        ),
    ]);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!((joined(&one).as_str(), one.pages, one.continues), ("aaaaa", 1, true));
    let next = one.refs.next.expect("the block after it");
    assert_eq!((next.page, next.range.offsets()), (0, (6, 11)));
    let more = one.refs.more.expect("where it goes on");
    assert_eq!(more.page, 0);
}

/// Failure: a block of a kind that isn't joined (code) with `continues` stored says it goes on and names nowhere, so an agent can't follow it. It is given alone, `continues`, with `more` where text goes on.
#[test]
fn a_kind_that_isnt_joined_with_continues_set_still_says_where_it_goes_on() {
    let mut doc = legs(&[leg(0, "let x = 1;", true), leg(1, "more code", false)]);
    doc.pages[0].blocks[0].kind = Kind::Code;
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!((joined(&one).as_str(), one.pages, one.continues), ("let x = 1;", 1, true));
    assert_eq!(one.refs.more.map(|more| more.page), Some(1));
}

/// Failure: after the blank-page limit `continues` stays set with no `more` (or is dropped while the paragraph goes on). Nine blank pages (the index says no text) between the pieces, then a page with text: the
/// first piece is given alone, `continues`, and `more` is on the page that has text.
#[test]
fn a_stop_past_the_blank_page_limit_names_where_it_goes_on() {
    let mut pages = vec![leg(0, "first part", true)];
    pages.extend((1..=9).map(|_| (String::new(), vec![])));
    pages.push(leg(10, "after the blanks", false));
    let mut doc = legs(&pages);
    for page in 1..=9 {
        doc.pages[page].readable = false;
    }
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!((joined(&one).as_str(), one.continues), ("first part", true));
    assert_eq!(one.refs.more.map(|more| more.page), Some(10));
}

/// Failure: a long join uses up the page reads, so the paragraph before it can't be named (`prev` empty) though a single page would have named it. A forged chain of 70 pages asked from page 5: `prev` is the last
/// block of page 4 while the join still stops at the page cap.
#[test]
fn a_long_join_leaves_the_reads_that_prev_needs() {
    let pages: Vec<(String, Vec<Block>)> = (0..70).map(|page| leg(page, &format!("part {page:02}"), true)).collect();
    let mut doc = legs(&pages);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 5, None, None, 65_536)));
    let prev = one.refs.prev.expect("the block before it");
    assert_eq!(prev.page, 4);
    assert!(one.counts.pages_read <= MAX_PAGES_READ + 1, "{}", one.counts.pages_read);
    assert!(one.truncated && one.continues);
}

/// Failure: a store that sets `continues` on every page is followed to its end, so one `get` reads a whole book; or the chain is cut at the cap with no `more`, so an agent can't go on. A 70-page file whose every
/// paragraph continues: at most 64 pages are read for the answer and one more only to name where it goes on, the answer is `truncated` and `continues`, and `more` is the first block of the page after the last given.
#[test]
fn a_store_that_forges_continues_on_every_page_is_followed_only_to_the_page_cap() {
    let pages: Vec<(String, Vec<Block>)> = (0..70).map(|page| leg(page, &format!("part {page:02}"), true)).collect();
    let mut doc = legs(&pages);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 65_536)));
    assert!(doc.reads.len() <= 65, "{} reads", doc.reads.len());
    assert_eq!(
        one.counts.pages_read,
        MAX_PAGES_READ + 1,
        "the 64 pages given and the one read to name where it goes on"
    );
    assert_eq!(one.given.len(), 64);
    assert!(one.truncated && one.continues, "{one:?}");
    let more = one.refs.more.expect("where it goes on");
    assert_eq!((more.page, more.range.offsets()), (64, (0, 7)));
    // And going on from there gives the next stretch of the chain, on the same terms.
    let again = unit(build(&mut doc, &ask(Grain::Paragraph, 64, Some(0), None, 65_536)));
    assert_eq!(joined(&again).lines().next(), Some("part 64"));
}

/// Failure: the caller's character cap is ignored once a paragraph is joined, so an answer is longer than asked; or it's cut with no `more`, or a piece is split. Three pages of 10 characters each with a cap of
/// 25: the first two pieces fit, the third doesn't come in part, the answer is `truncated` and `continues`, `more` is the start of the third page's piece, and `pages` and `chars` count the two given.
#[test]
fn a_joined_paragraph_is_cut_at_the_callers_cap_with_a_way_on() {
    let mut doc = legs(&[leg(0, "aaaaaaaaaa", true), leg(1, "bbbbbbbbbb", true), leg(2, "cccccccccc", false)]);
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 25)));
    assert_eq!(joined(&one), "aaaaaaaaaa\nbbbbbbbbbb");
    assert!(one.truncated && one.continues, "it goes on past what was given");
    let more = one.refs.more.expect("where it goes on");
    assert_eq!((more.page, more.range.offsets()), (2, (0, 10)));
    assert_eq!((one.pages, one.chars), (2, 20), "only what was given is counted");
}

/// Failure: a paragraph is joined with whatever is at the top of the next page, so a heading is read as the rest of a sentence. A `continues` paragraph whose next page begins with a heading is the paragraph alone,
/// and it doesn't say it goes on.
#[test]
fn a_paragraph_is_not_joined_with_a_block_of_another_kind() {
    let mut doc = legs(&[leg(0, "words that stop", true), leg(1, "Heading", false)]);
    doc.pages[1].blocks[0].kind = Kind::Heading;
    let one = unit(build(&mut doc, &ask(Grain::Paragraph, 0, None, None, 1000)));
    assert_eq!((joined(&one).as_str(), one.pages, one.continues), ("words that stop", 1, false));
    assert_eq!(one.refs.next.map(|next| next.page), Some(1));
}
