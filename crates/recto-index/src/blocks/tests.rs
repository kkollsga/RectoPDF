//! What a page's blocks must hold to. Each test names the failure it catches.

use super::codec::{self, FORMAT, MAX_RECORD_BYTES, OpenError};
use super::map::{self, Candidate, MAX_LETTERS, Shape, map_page};
use super::markdown::{self, Piece};
use super::sections::{Place, Sections};
use super::{Block, Kind, MAX_BLOCKS, PageBlocks};
use crate::corpus::CorpusKey;
use crate::seal::{CorpusSeal, Purpose};
use recto_core::{Toc, TocLine};
use recto_platform::find_key::FindKey;

fn seal_for(byte: u8) -> CorpusSeal {
    let key = FindKey::for_tests(1);
    CorpusSeal::create(&key, CorpusKey::from_hex(&format!("{byte:02x}").repeat(32)).unwrap())
        .unwrap()
        .0
}

fn block(kind: Kind, start: u32, end: u32) -> Block {
    Block {
        kind,
        level: 1,
        page: 3,
        start,
        end,
        bbox: [-5, 10, 300, 700],
        continues: false,
    }
}

fn page_of(blocks: Vec<Block>) -> PageBlocks {
    PageBlocks { page: 3, blocks }
}

fn sealed(seal: &CorpusSeal, page: &PageBlocks) -> Vec<u8> {
    let mut out = Vec::new();
    codec::seal_page(seal, page, &mut out).unwrap();
    out
}

/// A record of any plain bytes, sealed as the page's own: what a hostile writer with the key could make, so what past the seal must refuse.
fn raw(seal: &CorpusSeal, plain: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    seal.seal(Purpose::Blocks(3), &mut out, |out| out.extend_from_slice(plain)).unwrap();
    out
}

fn plain_of(count: u16, page: u32, blocks: &[[u8; 19]]) -> Vec<u8> {
    let mut plain = vec![FORMAT];
    plain.extend_from_slice(&page.to_le_bytes());
    plain.extend_from_slice(&count.to_le_bytes());
    for one in blocks {
        plain.extend_from_slice(one);
    }
    plain
}

fn one_block(kind: u8, level: u8, flags: u8, start: u32, end: u32) -> [u8; 19] {
    let mut bytes = [0u8; 19];
    (bytes[0], bytes[1], bytes[2]) = (kind, level, flags);
    bytes[3..7].copy_from_slice(&start.to_le_bytes());
    bytes[7..11].copy_from_slice(&end.to_le_bytes());
    bytes
}

/// Failure: a record doesn't read back as it was written, so an answer cites other characters, a box or a continuation than the build found.
#[test]
fn a_record_reads_back_as_written() {
    let seal = seal_for(1);
    let mut continued = block(Kind::Code, 40, 90);
    continued.continues = true;
    let page = page_of(vec![
        block(Kind::Heading, 0, 10),
        block(Kind::ListItem, 20, 30),
        continued,
        block(Kind::Footnote, 12, 18),
    ]);
    let mut chunk = sealed(&seal, &page);
    assert_eq!(chunk.len(), codec::record_len(4));
    assert_eq!(codec::open_page(&seal, 3, &mut chunk), Ok(page));
    let empty = page_of(Vec::new());
    assert_eq!(codec::open_page(&seal, 3, &mut sealed(&seal, &empty)), Ok(empty));
}

/// Failure: a changed, cut, extended or moved record is used: any byte turned, any cut, another page's, another corpus's or another purpose's chunk.
#[test]
fn a_damaged_or_foreign_record_is_refused() {
    let seal = seal_for(1);
    let page = page_of(vec![block(Kind::Paragraph, 0, 10), block(Kind::Heading, 10, 20)]);
    let chunk = sealed(&seal, &page);
    for at in 0..chunk.len() {
        let mut bad = chunk.clone();
        bad[at] ^= 1;
        assert_eq!(codec::open_page(&seal, 3, &mut bad), Err(OpenError::Damaged), "byte {at} turned");
    }
    for len in 0..chunk.len() {
        assert_eq!(codec::open_page(&seal, 3, &mut chunk[..len].to_vec()), Err(OpenError::Damaged), "cut to {len}");
    }
    let mut longer = chunk.clone();
    longer.push(0);
    assert_eq!(codec::open_page(&seal, 3, &mut longer), Err(OpenError::Damaged));
    assert_eq!(codec::open_page(&seal, 4, &mut chunk.clone()), Err(OpenError::Damaged), "another page's record");
    assert_eq!(
        codec::open_page(&seal_for(2), 3, &mut chunk.clone()),
        Err(OpenError::Damaged),
        "another corpus's key"
    );
    let mut other_purpose = Vec::new();
    seal.seal(Purpose::Identity(3), &mut other_purpose, |out| out.extend_from_slice(&plain_of(0, 3, &[])))
        .unwrap();
    assert_eq!(codec::open_page(&seal, 3, &mut other_purpose), Err(OpenError::Damaged), "an identity record");
}

/// Failure: a record under another format is thrown away as damage (a later build's blocks lost to an older one), or used as this build's.
#[test]
fn another_format_is_foreign_not_damage() {
    let seal = seal_for(1);
    let mut plain = plain_of(0, 3, &[]);
    plain[0] = FORMAT + 1;
    assert_eq!(codec::open_page(&seal, 3, &mut raw(&seal, &plain)), Err(OpenError::Foreign));
}

/// Failure: a record past a cap or saying what no build writes is used (the signs: 513 blocks, a count that isn't the length, a kind or flag or level out of range, an empty, overlapping or off-the-page block, another page's number inside).
#[test]
fn what_no_build_writes_is_refused_after_the_seal() {
    let seal = seal_for(1);
    let good = one_block(1, 1, 0, 0, 10);
    let refused = |plain: Vec<u8>, what: &str| assert_eq!(codec::open_page(&seal, 3, &mut raw(&seal, &plain)), Err(OpenError::Damaged), "{what}");
    assert!(codec::open_page(&seal, 3, &mut raw(&seal, &plain_of(1, 3, &[good]))).is_ok());
    refused(
        plain_of(MAX_BLOCKS as u16 + 1, 3, &vec![one_block(1, 1, 0, 0, 1); MAX_BLOCKS + 1]),
        "513 blocks",
    );
    refused(plain_of(2, 3, &[good]), "a count past the bytes");
    refused(plain_of(0, 3, &[good]), "bytes past the count");
    refused(plain_of(1, 4, &[good]), "another page's number");
    refused(plain_of(1, 3, &[one_block(0, 1, 0, 0, 10)]), "kind 0");
    refused(plain_of(1, 3, &[one_block(7, 1, 0, 0, 10)]), "kind 7");
    refused(plain_of(1, 3, &[one_block(1, 1, 2, 0, 10)]), "flag bit 1");
    refused(plain_of(1, 3, &[one_block(1, 10, 0, 0, 10)]), "level 10");
    refused(plain_of(1, 3, &[one_block(1, 1, 0, 5, 5)]), "an empty block");
    refused(plain_of(1, 3, &[one_block(1, 1, 0, 9, 5)]), "an end before the start");
    refused(
        plain_of(1, 3, &[one_block(1, 1, 0, 0, super::MAX_PAGE_CHARS + 1)]),
        "past the page's characters",
    );
    refused(plain_of(2, 3, &[one_block(1, 1, 0, 0, 10), one_block(1, 1, 0, 9, 20)]), "overlapping blocks");
    refused(plain_of(2, 3, &[one_block(1, 1, 0, 0, 10), one_block(1, 1, 0, 0, 10)]), "the same block twice");
    let mut long = raw(&seal, &plain_of(0, 3, &[]));
    long.resize(MAX_RECORD_BYTES + 1, 0);
    assert_eq!(
        codec::open_page(&seal, 3, &mut long),
        Err(OpenError::Damaged),
        "a chunk past the longest record"
    );
}

/// Failure: the 512-block cap is off by one on the way in or out: 512 must seal and read back, 513 must be refused at the writer.
#[test]
fn the_block_cap_holds_at_the_limit_and_one_over() {
    let seal = seal_for(1);
    let make = |count: usize| page_of((0..count as u32).map(|at| block(Kind::Paragraph, at * 2, at * 2 + 1)).collect());
    let at_cap = make(MAX_BLOCKS);
    let mut chunk = sealed(&seal, &at_cap);
    assert!(chunk.len() <= MAX_RECORD_BYTES);
    assert_eq!(codec::open_page(&seal, 3, &mut chunk), Ok(at_cap));
    assert!(codec::seal_page(&seal, &make(MAX_BLOCKS + 1), &mut Vec::new()).is_err());
    let mut overlapping = make(2);
    overlapping.blocks[1].start = 0;
    overlapping.blocks[1].end = 2;
    assert!(
        codec::seal_page(&seal, &overlapping, &mut Vec::new()).is_err(),
        "a list a reader would refuse is never written"
    );
}

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

fn para(text: &str) -> Candidate {
    Candidate {
        shape: Shape::Paragraph,
        level: 0,
        bbox: [0.0, 0.0, 10.0, 10.0],
        text: text.into(),
        continues: false,
    }
}

fn span(mapped: &map::Mapped, at: usize) -> (u32, u32) {
    (mapped.blocks.blocks[at].start, mapped.blocks.blocks[at].end)
}

/// Failure: a two-column page (the file draws the right column first, distillPDF reads the left first) maps only what is in order, so the page's other half answers as the whole page.
#[test]
fn a_block_before_the_previous_one_is_found_by_its_one_occurrence() {
    let page = chars("Right column words\nLeft column words");
    let mapped = map_page(0, &page, &[para("Left column words"), para("Right column words")]);
    assert_eq!(mapped.counts.stored_total(), 2, "{:?}", mapped.counts);
    assert_eq!(span(&mapped, 0), (19, 36));
    assert_eq!(span(&mapped, 1), (0, 18), "kept in distillPDF's order, placed where the text has it");
}

/// Failure: a running head that repeats on the page maps to whichever copy comes first or last, so an agent is told the wrong place; or a phrase that is also inside an earlier block is taken from there.
#[test]
fn a_repeated_run_or_an_overlapping_one_is_not_stored() {
    let page = chars("Annual Report\nbody one here\nAnnual Report\nbody two here");
    let mapped = map_page(0, &page, &[para("body two here"), para("Annual Report")]);
    assert_eq!((mapped.counts.stored_total(), mapped.counts.why.ambiguous), (1, 1), "{:?}", mapped.counts);
    // The same repeat found after the previous block is the first one there: the rule's step 1, which a page's order makes the right one.
    let ordered = map_page(0, &page, &[para("Annual Report"), para("body one here")]);
    assert_eq!(ordered.counts.stored_total(), 2);
    assert_eq!(span(&ordered, 0), (0, 13));
    let nested = chars("alpha beta gamma");
    let mapped = map_page(0, &nested, &[para("alpha beta"), para("beta")]);
    assert_eq!(
        (mapped.counts.stored_total(), mapped.counts.why.not_found),
        (1, 1),
        "a block over a mapped one isn't stored: {:?}",
        mapped.counts
    );
}

/// Failure: a limit is off by one: a block of 64 Ki letters must map, and one of 64 Ki + 1 must be dropped and counted, not stored cut short.
#[test]
fn a_block_of_the_most_letters_maps_and_one_more_does_not() {
    let page = chars(&"a".repeat(MAX_LETTERS + 1));
    let mapped = map_page(0, &page, &[para(&"a".repeat(MAX_LETTERS))]);
    assert_eq!(span(&mapped, 0), (0, MAX_LETTERS as u32));
    let mapped = map_page(0, &page, &[para(&"a".repeat(MAX_LETTERS + 1))]);
    assert_eq!((mapped.counts.stored_total(), mapped.counts.why.too_long), (0, 1));
    assert!(mapped.blocks.blocks.is_empty());
}

/// Failure: a page's 513th block is stored (the record can't hold it) or the count of what was dropped is wrong.
#[test]
fn a_page_stores_at_most_512_blocks() {
    let words: Vec<String> = (0..=MAX_BLOCKS).map(|at| format!("w{at}z")).collect();
    let page = chars(&words.join(" "));
    let candidates: Vec<Candidate> = words.iter().map(|word| para(word)).collect();
    let mapped = map_page(0, &page, &candidates);
    assert_eq!(
        (mapped.blocks.blocks.len(), mapped.counts.why.over_cap, mapped.counts.dropped_total()),
        (MAX_BLOCKS, 1, 1)
    );
    assert!(
        codec::seal_page(&seal_for(1), &mapped.blocks, &mut Vec::new()).is_ok(),
        "what is mapped is what a record holds"
    );
}

/// Failure: a hostile page (two million of one letter) and a block that nearly matches everywhere makes the search take minutes: the comparisons are bounded and the block is dropped as unplaced.
#[test]
fn a_page_built_to_make_the_search_slow_is_cut_short() {
    let page = vec!['a'; 2_000_000];
    let mut text = "a".repeat(239);
    text.push('b');
    let mapped = map_page(0, &page, &[para(&text)]);
    assert_eq!((mapped.counts.stored_total(), mapped.counts.why.out_of_budget), (0, 1), "{:?}", mapped.counts);
}

/// Failure: tables and figures, empty blocks or ligature spellings are mishandled: a table is stored with no text behind it, or `ﬁne` in the page never meets `fine` from the helper.
#[test]
fn kinds_and_spellings() {
    let page = chars("The \u{FB01}ne print");
    let mut table = para("fine print");
    table.shape = Shape::Table;
    let mut figure = para("fine print");
    figure.shape = Shape::Figure;
    let mapped = map_page(0, &page, &[table, figure, para("  ... "), para("Fine PRINT")]);
    assert_eq!((mapped.counts.stored_total(), mapped.counts.dropped_total()), (1, 3));
    assert_eq!((mapped.counts.why.not_stored_kind, mapped.counts.why.empty), (2, 1));
    assert_eq!(span(&mapped, 0), (4, 13));
    assert_eq!(mapped.counts.dropped[Shape::Table.index()], 1);
}

/// Failure: a helper's box with NaN, infinity or huge numbers is stored as it came (a record that doesn't read back), or its sides are the wrong way round.
#[test]
fn a_box_is_whole_points_in_range_and_in_order() {
    let mut odd = para("word");
    odd.bbox = [f32::NAN, 1e12, 100.4, -1e12];
    let mapped = map_page(0, &chars("word"), &[odd]);
    let bbox = mapped.blocks.blocks[0].bbox;
    assert!(bbox[0] <= bbox[2] && bbox[1] <= bbox[3], "{bbox:?}");
    assert_eq!(bbox, [0, i16::MIN, 100, i16::MAX]);
}

/// Failure: a line from the helper that is hostile (too long, not an object, an unknown kind, wrong types) panics or is believed.
#[test]
fn a_candidate_line_is_read_with_care() {
    let line = br#"{"kind":"heading","level":2,"box":[1,2,3,4],"text":"Intro","continues":true}"#;
    let got = Candidate::from_line(line).unwrap();
    assert_eq!(
        (got.shape, got.level, got.bbox, got.text.as_str(), got.continues),
        (Shape::Heading, 2, [1.0, 2.0, 3.0, 4.0], "Intro", true)
    );
    assert_eq!(
        Candidate::from_line(br#"{"kind":"para","level":99,"box":"x","text":5}"#).unwrap().level,
        super::MAX_LEVEL
    );
    for bad in [&b""[..], b"[]", b"{}", br#"{"kind":"banner"}"#, b"{\"kind\":", &[0xff, 0xfe]] {
        assert!(Candidate::from_line(bad).is_none(), "{bad:?}");
    }
    assert!(Candidate::from_line(&vec![b' '; map::MAX_LINE_BYTES + 1]).is_none());
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

fn at(page: u32, top: u16) -> Place {
    Place { page, top }
}

/// Failure: a section ends in the wrong place (at the next entry of any depth, or not at the next of the same or shallower), a place on a boundary is in the wrong section, or the chapter and headings above a place are wrong.
#[test]
fn sections_run_to_the_next_entry_of_the_same_or_shallower_depth() {
    // A / A1 / A2 / B / B1
    let toc = toc(&[(0, 0, 0), (0, 1, 500), (1, 1, 0), (2, 0, 0), (2, 1, 300)]);
    let sections = Sections::new(toc.entries());
    assert_eq!(sections.range(0), Some((at(0, 0), Some(at(2, 0)))));
    assert_eq!(sections.range(1), Some((at(0, 500), Some(at(1, 0)))));
    assert_eq!(sections.range(2), Some((at(1, 0), Some(at(2, 0)))));
    assert_eq!(sections.range(3), Some((at(2, 0), None)));
    assert_eq!(sections.range(9), None);
    for (place, section, chapter, above) in [
        (at(0, 499), Some(0), Some(0), vec![0]),
        (at(0, 500), Some(1), Some(0), vec![0, 1]),
        (at(0, 999), Some(1), Some(0), vec![0, 1]),
        (at(1, 0), Some(2), Some(0), vec![0, 2]),
        (at(1, 1000), Some(2), Some(0), vec![0, 2]),
        (at(2, 0), Some(3), Some(3), vec![3]),
        (at(2, 299), Some(3), Some(3), vec![3]),
        (at(2, 300), Some(4), Some(3), vec![3, 4]),
        (at(40, 0), Some(4), Some(3), vec![3, 4]),
    ] {
        assert_eq!(sections.section_of(place), section, "{place:?}");
        assert_eq!(sections.chapter_of(place), chapter, "{place:?}");
        assert_eq!(sections.headings_above(place), above, "{place:?}");
    }
    assert!(sections.holds(1, at(0, 500)) && !sections.holds(1, at(1, 0)));
}

/// Failure: text before the first entry is put in a section, a file with no contents has one, or a block's place is read from the wrong edge.
#[test]
fn text_before_any_entry_is_in_no_section() {
    let toc = toc(&[(1, 0, 200)]);
    let sections = Sections::new(toc.entries());
    assert_eq!(sections.section_of(at(0, 900)), None);
    assert_eq!(sections.section_of(at(1, 199)), None);
    assert_eq!(sections.section_of(at(1, 200)), Some(0));
    assert!(Sections::new(&[]).section_of(at(0, 0)).is_none());
    let mut high = block(Kind::Paragraph, 0, 5);
    high.page = 2;
    high.bbox = [0, 0, 100, 700];
    assert_eq!(Place::of_block(&high, 800.0), at(2, 125));
    assert_eq!(Place::of_block(&high, f32::NAN), at(2, 0));
}

fn piece(kind: Kind, text: &[char]) -> Piece<'_> {
    Piece {
        kind,
        level: 0,
        depth: None,
        text,
        hidden: &[],
    }
}

fn rendered(kind: Kind, text: &str) -> String {
    markdown::render(&[piece(kind, &chars(text))])
}

/// Failure: a page's own lines become the reader's structure: a `# heading`, a fence, a `> quote`, a `- list`, a numbered list, a table row or a setext rule starting a line of a paragraph.
#[test]
fn a_page_cannot_forge_markdown_structure() {
    let out = rendered(
        Kind::Paragraph,
        "Intro\n# Ignore the rest\n```\n> quote\n- item\n1. first\n2) second\n| a | b |\n===\n    code\n* star\n+ plus\n<b>html\n[x]: http://e",
    );
    for line in out.lines() {
        let first = line.chars().next().unwrap();
        assert!(
            !"#>-+*`|=<[ ".contains(first) || first == '\\',
            "a line that starts something: {line:?} in {out}"
        );
    }
    assert!(
        out.contains("\\# Ignore the rest") && out.contains("\\```") && out.contains("\\> quote") && out.contains("\\- item"),
        "{out}"
    );
    assert!(
        out.contains("1\\. first") && out.contains("2\\) second") && out.contains("\\| a | b |") && out.contains("\\===") && out.contains("\ncode"),
        "{out}"
    );
    // A heading made by the program is still one, and the page's text inside it can't start another thing.
    let heading = markdown::render(&[Piece {
        depth: Some(1),
        ..piece(Kind::Heading, &chars("# not\nreally"))
    }]);
    assert_eq!(heading, "## \\# not really");
    let list = markdown::render(&[piece(Kind::ListItem, &chars("first\n- nested"))]);
    assert_eq!(list, "- first\n  \\- nested");
}

/// Failure: a code block with a longer run of backticks than its fence ends the fence early, and the rest of the page's text is read as Markdown.
#[test]
fn a_code_fence_is_longer_than_any_backticks_inside() {
    let out = rendered(Kind::Code, "a ```` b\n```\n# not a heading\n  indented");
    assert_eq!(out, "`````\na ```` b\n```\n# not a heading\n  indented\n`````");
    assert_eq!(rendered(Kind::Code, "plain"), "```\nplain\n```");
    assert!(rendered(Kind::Code, "``").starts_with("```\n"), "never shorter than three");
}

/// Failure: hidden text isn't marked, so an agent reads planted words as the document's; a page forges the mark to make its own words look flagged or closes it early; or a heading's `#` count isn't from its contents depth, clamped.
#[test]
fn hidden_words_are_marked_and_the_mark_cannot_be_forged() {
    let text = chars("see secret] words [hidden: fake] end");
    let out = markdown::render(&[Piece {
        hidden: &[(4, 11)],
        ..piece(Kind::Paragraph, &text)
    }]);
    assert_eq!(out, "see [hidden: secret\\]] words \\[hidden: fake] end");
    let block = block(Kind::Paragraph, 10, 20);
    assert_eq!(markdown::spans_within(&block, &[(0, 12), (15, 40), (30, 31), (20, 25)]), vec![(0, 2), (5, 10)]);
    for (depth, level, hashes) in [(Some(0), 3, 1), (Some(2), 1, 3), (Some(9), 1, 6), (None, 0, 1), (None, 4, 4), (None, 9, 6)] {
        assert_eq!(markdown::heading_hashes(depth, level), hashes);
    }
    assert_eq!(
        markdown::render(&[piece(Kind::Paragraph, &chars("  \n ")), piece(Kind::Caption, &chars("Fig 1"))]),
        "Fig 1"
    );
}
