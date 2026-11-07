//! A page's chrome: running heads, footers and page numbers, which a copy of body text leaves out (owner, 2026-11-07: "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text.").
//!
//! **What counts.** A line is a candidate only if it's at the page's edge: in the top or bottom [`BAND`] of the page, in a block of at most [`MAX_BLOCK_ROWS`] rows from the edge, with a gap of at least [`MIN_GAP`] times its size between the block and the rest of the page, and with something on the page past it. That's position. Then one of two things about its text: it's a page number alone ("12", "xii", "Page 3 of 40", "– 7 –"), or the same line, its digits masked, is at about the same distance from the same edge on one of the [`NEIGHBOURS`] pages either side, and no bigger than the page's usual line, so a chapter title at the top of two pages isn't one. Both are needed, as in distillPDF's `chrome.rs`: text alone would take a heading repeated mid-page, and position alone the first line of every page. Unlike distillPDF it doesn't read the whole document first: a copy reads its pages one at a time, and a few on either side is what it can afford.
//!
//! **Cost.** At most [`MAX_EDGE_LINES`] lines a page, 32 B each, kept with its text, and a look at [`NEIGHBOURS`] pages' of them each side. Nothing is read for it that the page's text didn't already hold, past the neighbours' own text.
//!
//! **Its limits.** A header or footer that changes on every page and has no page number in it, such as a running head that names each page's first entry, isn't found. A page whose neighbours couldn't be read is judged with what's known, so only its page number goes.

/// How deep the bands at the top and bottom of a page are, as a part of its height. distillPDF's, measured on reports and papers.
pub const BAND: f32 = 0.12;
/// The gap between an edge block and the rest of the page, in sizes of the block's text. Lines are 1.15 to 1.4 apart, and chrome 2 and more.
pub const MIN_GAP: f32 = 1.8;
/// Rows from the edge that can be chrome: a running head, a rule's caption, a page number.
pub const MAX_BLOCK_ROWS: usize = 3;
/// Pages either side a line is looked for on. Two, so a book's running heads, which alternate between left and right pages, are found.
pub const NEIGHBOURS: u32 = 2;
/// Lines kept for one page.
pub const MAX_EDGE_LINES: usize = 12;
/// How far a line may be from where its neighbour's is, in points, or in sizes of it if that's more.
const SAME_PLACE_POINTS: f32 = 3.0;
const SAME_PLACE_SIZES: f32 = 0.5;
/// A repeated line bigger than the page's usual one by more than this is a heading, not a running head.
pub const MOST_SIZE: f32 = 1.15;
/// A page number is no smaller than this against the page's usual line: smaller is an exponent at the top of a page, or a footnote's mark. On bench100 (`examples/chrome_corpus.rs`), of 2,161 numbers 21 were under it, all GDPR's footnote marks and an exponent, and 4 real ones were within 0.03 of it. Bigger has no limit: on pages of tables and figures arXiv's page numbers are up to 3 times the usual line.
pub const NUMBER_LEAST_SIZE: f32 = 0.55;

/// One line of a page's edge block, as a copy needs to know it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeLine {
    /// At the top of the page, or the bottom.
    pub top: bool,
    /// From that edge to the line, in points.
    pub from_edge: f32,
    /// How tall its letters are, in points.
    pub size: f32,
    /// Its text with digits masked, and its row's, which is it and any other line beside it.
    pub key: u64,
    pub row_key: u64,
    /// Where it is in the page's text, `[start, end)`.
    pub start: u32,
    pub end: u32,
    /// It's a page number and nothing else.
    pub number: bool,
    /// Its size against the page's usual line's.
    pub relative: f32,
    /// It has a letter or a digit in it. A bracket by itself at the foot of two pages is a footnote's, broken off.
    pub words: bool,
}

/// Which of a page's edge lines are chrome, given its neighbours' edge lines: the ones a copy of body text leaves out, as `[start, end)` in the page's text.
pub fn chrome_spans(page: &[EdgeLine], neighbours: &[&[EdgeLine]]) -> Vec<(u32, u32)> {
    let near = |line: &EdgeLine, seen: &EdgeLine| seen.top == line.top && (seen.from_edge - line.from_edge).abs() <= SAME_PLACE_POINTS.max(SAME_PLACE_SIZES * line.size);
    page.iter()
        .filter(|line| {
            // A page number has page numbers at its place on the pages around it, so a cover's big year isn't one. A page with none around it to compare goes by its own.
            let number = line.number && line.relative >= NUMBER_LEAST_SIZE && (neighbours.iter().all(|other| other.is_empty()) || neighbours.iter().any(|other| other.iter().any(|seen| seen.number && near(line, seen))));
            let repeated = line.relative <= MOST_SIZE && neighbours.iter().any(|other| other.iter().any(|seen| near(line, seen) && (seen.key == line.key || seen.row_key == line.row_key)));
            line.words && (number || repeated)
        })
        .map(|line| (line.start, line.end))
        .collect()
}

/// A line's text as it's compared: lower case, spaces collapsed, and every run of digits one `#`, so "Page 12 of 340" and "Page 9 of 340" are the same. FNV-1a, so only 8 bytes are kept.
pub fn key(text: impl Iterator<Item = char>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut push = |byte: u8| hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    let (mut in_digits, mut in_space, mut any) = (false, false, false);
    for ch in text {
        if ch.is_ascii_digit() {
            if !in_digits {
                if in_space && any {
                    push(b' ');
                }
                push(b'#');
                any = true;
            }
            in_digits = true;
            in_space = false;
        } else if ch.is_whitespace() {
            in_digits = false;
            in_space = true;
        } else {
            if in_space && any {
                push(b' ');
            }
            in_space = false;
            in_digits = false;
            for lower in ch.to_lowercase() {
                let mut bytes = [0u8; 4];
                lower.encode_utf8(&mut bytes).bytes().for_each(&mut push);
            }
            any = true;
        }
    }
    hash
}

/// Whether a line is a page number and nothing else: `12`, `xii`, `Page 3`, `p. 3`, `3 of 40`, `3/40`, `- 3 -`, `| 3 |`. Roman numerals only alone or after a word for page, as a word like "mix" is one.
pub fn is_page_number(text: &str) -> bool {
    let decoration = |ch: char| matches!(ch, '-' | '–' | '—' | '|' | '·' | '•' | '[' | ']' | '(' | ')' | '<' | '>' | '«' | '»' | '/' | '.' | ':') || ch.is_whitespace();
    let lower = text.to_lowercase();
    // What's round it must be round it: `- 3 -` and `(3)` are page numbers, and `3)`, `3.` and `2011-` are a list's or a table's.
    let core = lower.trim_matches(decoration);
    let (before, after) = (&lower[..lower.len() - lower.trim_start_matches(decoration).len()], &lower[lower.trim_end_matches(decoration).len()..]);
    let has = |side: &str, set: &[char]| side.chars().any(|ch| set.contains(&ch));
    let dashes = ['-', '–', '—'];
    let balanced = has(before, &dashes) == has(after, &dashes) && has(before, &['(', '[']) == has(after, &[')', ']']) && !(before.trim().is_empty() && has(after, &['.', ':']));
    if !balanced {
        return false;
    }
    let words: Vec<&str> = core.split(|ch: char| ch.is_whitespace() || ch == '/').map(|word| word.trim_matches(decoration)).filter(|word| !word.is_empty()).collect();
    let digits = |word: &str| (1..=5).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_digit());
    let roman = |word: &str| (1..=7).contains(&word.len()) && word.bytes().all(|byte| matches!(byte, b'i' | b'v' | b'x' | b'l' | b'c'));
    let page_word = |word: &str| matches!(word, "page" | "p" | "pg" | "pp" | "side" | "seite" | "página" | "pagina" | "sida");
    let of_word = |word: &str| matches!(word, "of" | "av" | "de" | "von" | "sur" | "di");
    match words.as_slice() {
        [one] => digits(one) || roman(one),
        [word, one] => (page_word(word) && (digits(one) || roman(one))) || (digits(word) && digits(one)),
        [first, second, third] => (digits(first) && of_word(second) && digits(third)) || (page_word(first) && digits(second) && digits(third)),
        [word, one, of, two] => page_word(word) && digits(one) && of_word(of) && digits(two),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a page number in one of the usual forms is copied with the body text, or a word or a figure that isn't one is taken for it.
    #[test]
    fn page_numbers_are_told_from_other_lines() {
        for number in ["12", "xii", "Page 3", "p. 3", "3 of 40", "3/40", "Page 3/40", "(3)", "-2-", "-- 2 --", "- 3 -", "– 7 –", "| 54 |", "Page 3 of 40", "PAGE 12", "08"] {
            assert!(is_page_number(number), "{number}");
        }
        for not in ["mix", "2024 Annual Report", "Figure 3", "3.14 percent", "Table 1 of the survey", "", "Chapter 2", "123456", "1)", "i)", "3.", "2011-", "(", "- 3"] {
            assert!(!is_page_number(not), "{not}");
        }
    }

    /// Failure: a running head with a page number in it reads as a different line on every page, so it's never found, or two different heads read as one.
    #[test]
    fn a_key_masks_numbers_and_case() {
        let key_of = |text: &str| key(text.chars());
        assert_eq!(key_of("Page 12 of 340"), key_of("page 9  of 340"));
        assert_eq!(key_of("THE FUTURE — PART B"), key_of("the future — part b"));
        assert_ne!(key_of("Part A"), key_of("Part B"));
    }

    fn line(top: bool, from_edge: f32, text: &str) -> EdgeLine {
        EdgeLine { top, from_edge, size: 9.0, key: key(text.chars()), row_key: key(text.chars()), start: 0, end: text.len() as u32, number: is_page_number(text), relative: 1.0, words: true }
    }

    /// Failure: a running head repeated on the next page isn't left out, or one at the other edge, or far from where it was, or a heading bigger than body text, is taken for one.
    #[test]
    fn a_line_is_chrome_where_it_repeats() {
        let head = [line(true, 40.0, "Annual report 2024")];
        assert_eq!(chrome_spans(&head, &[&[line(true, 41.0, "Annual report 2024")]]).len(), 1, "the same place on the next page");
        assert!(chrome_spans(&head, &[&[line(false, 40.0, "Annual report 2024")]]).is_empty(), "the other edge");
        assert!(chrome_spans(&head, &[&[line(true, 80.0, "Annual report 2024")]]).is_empty(), "far from it");
        assert!(chrome_spans(&head, &[]).is_empty(), "nothing to compare with");
        let big = [EdgeLine { relative: 1.5, ..head[0] }];
        assert!(chrome_spans(&big, &[&head]).is_empty(), "a heading");
        assert_eq!(chrome_spans(&[line(false, 30.0, "17")], &[]).len(), 1, "a page number, with no page around to compare");
        assert_eq!(chrome_spans(&[line(false, 30.0, "17")], &[&[line(false, 31.0, "18")]]).len(), 1, "a page number, with the next at its place");
        assert!(chrome_spans(&[EdgeLine { relative: 2.5, ..line(true, 30.0, "2023") }], &[&[line(false, 30.0, "2")]]).is_empty(), "a cover's year, with the next page's number at its foot");
        assert!(chrome_spans(&[EdgeLine { relative: 0.4, ..line(true, 30.0, "2") }], &[]).is_empty(), "an exponent at the top of a page");
    }
}
