//! A page's chrome: running heads, footers and page numbers, which a copy of body text leaves out (owner, 2026-11-07: "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text.").
//!
//! **What counts.** A line is cut into pieces where its letters leave a wide gap, so a page number set at the end of a footnote's line is a piece of its own. A piece is a candidate if it's at the page's edge: in the top or bottom [`BAND`] of the page, in a block of at most [`MAX_BLOCK_ROWS`] rows from the edge, with a gap of at least [`MIN_GAP`] times its size between the block and the rest of the page, and with something on the page past it. Then one of two things about its text:
//!
//! - **A repeated row,** set apart, with a letter in it, no bigger than the page's usual line by more than [`MOST_SIZE`], whose text, digits masked, is at about the same distance from the same edge on one of the [`NEIGHBOURS`] pages either side. If its numbers differ from that page's, they have to step with the pages, as a page number does, and two pages have to say so: "Article 12" and "Article 13" at the top of consecutive pages are headings, and "Page 12" and "Page 13" a running head. One page is enough only where the row holds the page's own number as a page's number is set: a piece that's a page number, or a number after a separator or a word for page at a piece's end, or before a separator at its start (`L 119/6`, `Page 5`, `5 | Report`). Fewer pages around, as in a two-page file or with a page that couldn't be read, is never a reason to need fewer. Owner, 2026-11-11, through the coordinator: a heading missing from a copy is worse than a header that stays, so when in doubt the line stays. The whole row goes, its figures too: pieces exist only so a page number can be found inside a row. A piece alone counts if it's long enough to say so by its own text, or is a date (`3/14/2019`, `4.5.2016`), by the same rule for its own numbers; figures alone otherwise never repeat, as footnote marks, axis labels and table cells do.
//! - **A page number** ("12", "xii", "Page 3 of 40", "– 7 –"), alone in its row, at its end or centred on the page, between [`NUMBER_LEAST_SIZE`] and [`NUMBER_MOST_SIZE`] of the page's usual line or under [`NUMBER_MOST_POINTS`], whose neighbours have page numbers at its place, down and across or mirrored across, that step with the pages: 13 on the next page, 10 two before. With that, it needn't be set apart, as a number at the end of a page's last line often isn't. A number that starts a row, as "1 Introduction" or a question's does, isn't one. With no page around to compare, as in a one-page file, one set apart and under 1000 is enough, so a lone year stays.
//!
//! Both position and text are needed, as in distillPDF's `chrome.rs`: text alone would take a heading repeated mid-page, and position alone the first line of every page. Unlike distillPDF it doesn't read the whole document first: a copy reads its pages one at a time, and a few on either side is what it can afford.
//!
//! **Cost.** At most [`MAX_EDGE_LINES`] pieces a page, at most 80 B each (a test holds it), kept with its text, and a look at [`NEIGHBOURS`] pages' of them each side. Nothing is read for it that the page's text didn't already hold, past the neighbours' own text.
//!
//! **Its limits.** A header or footer that changes on every page and has no page number in it isn't found. A page whose neighbours couldn't be read is judged with what's known, so only a page number set apart goes. Real content that repeats at a page's edge does go: a table's header set off from its rows on every page, and numbered headings at the top of three pages in a row whose numbers count with them, as one-page articles' would. And the other way, running heads whose numbers move by other than the pages do stay: "Articles 12-14" then "Articles 14-15", or a two-up footer's "12 | 13" then "14 | 15". `docs/PLAN.md` tracks them.

/// How deep the bands at the top and bottom of a page are, as a part of its height. distillPDF's, measured on reports and papers.
pub const BAND: f32 = 0.12;
/// The gap between an edge block and the rest of the page, in sizes of the block's text. Lines are 1.15 to 1.4 apart, and chrome 2 and more.
pub const MIN_GAP: f32 = 1.8;
/// A gap along a line this many sizes wide cuts it into pieces. Words are a third of a size apart, and a page number at the end of a line far more.
pub const PIECE_GAP: f32 = 1.5;
/// Rows from the edge that can be chrome: a running head, a rule's caption, a page number.
pub const MAX_BLOCK_ROWS: usize = 3;
/// Pages either side a line is looked for on. Two, so a book's running heads, which alternate between left and right pages, are found.
pub const NEIGHBOURS: u32 = 2;
/// Lines kept for one page.
pub const MAX_EDGE_LINES: usize = 16;
/// How far a line may be from where its neighbour's is, in points, or in sizes of it if that's more.
const SAME_PLACE_POINTS: f32 = 3.0;
const SAME_PLACE_SIZES: f32 = 0.5;
/// How far across a page number may be from its neighbour's, or from where its neighbour's would be mirrored, in points: a few characters.
const SAME_ACROSS_POINTS: f32 = 24.0;
/// A repeated row bigger than the page's usual line by more than this is a heading, not a running head. Sizes are the letters' boxes, line by line: the AI Act's footer is 1.29 of its body's, and a heading is 1.5 and up.
pub const MOST_SIZE: f32 = 1.4;
/// A page number is no smaller than this against the page's usual line: smaller is an exponent at the top of a page, or a footnote's mark. On bench100 (`examples/chrome_corpus.rs`) GDPR's footnote marks are 0.37 to 0.43, and the smallest real page numbers 0.52 and 0.54, Roman ones in a NASA report and one on an arXiv page of figures, so it sits between. Bigger has no limit: on pages of tables and figures arXiv's page numbers are up to 3 times the usual line.
pub const NUMBER_LEAST_SIZE: f32 = 0.48;
/// A page number's size: up to this many times the page's usual line, or this many points tall, which is a 14 pt figure. A slide's 24 pt section number or a timeline's 20 pt year is bigger than either. On pages of tables and figures arXiv's page numbers are up to 3 times the usual line, and under 9 points.
pub const NUMBER_MOST_SIZE: f32 = 1.6;
pub const NUMBER_MOST_POINTS: f32 = 10.0;
/// A piece this close to the page's middle, as a part of its width, is centred, as many page numbers are with a section's name beside them.
const CENTRED: f32 = 0.05;
/// On a page with none around it to compare, a number set apart is its page number only under this: a year at the foot of a one-page letter isn't.
pub const LONE_PAGE_NUMBER_BELOW: u32 = 1000;
/// Pages that have to agree that a repeated row's or piece's numbers step with them, when it isn't the same on any of them and doesn't hold the page's own number as a page's number is set: one could be two numbered headings that happen to follow each other. Never fewer for fewer pages around.
pub const STEPPED_ROW_PAGES: usize = 2;
/// In [`EdgeLine::row_numbers`] and [`EdgeLine::numbers`], no number there.
pub const NO_NUMBER: u32 = u32::MAX;
/// In [`EdgeLine::framed_number`], none.
pub const NO_FRAMED: u16 = u16::MAX;

/// One piece of a line at a page's edge, as a copy needs to know it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeLine {
    /// At the top of the page, or the bottom.
    pub top: bool,
    /// From that edge to the piece, in points.
    pub from_edge: f32,
    /// How tall its letters are, in points.
    pub size: f32,
    /// Its text with digits masked, and its row's, which is it and any other piece beside it.
    pub key: u64,
    pub row_key: u64,
    /// The first two numbers in its row, [`NO_NUMBER`] past the ones it has: rows that are the same but for these are the same line only if they step with the pages. See [`numbers`].
    pub row_numbers: [u32; 2],
    /// The same, of its own text, for a piece that repeats by itself.
    pub numbers: [u32; 2],
    /// A number one of its row's pieces is framed by as a page's number is, [`NO_FRAMED`] if none: see [`framed_number`]. Only that, or a piece that's a page number, lets one page around say a row whose numbers step is a running head.
    pub framed_number: u16,
    /// Where it is in the page's text, `[start, end)`.
    pub start: u32,
    pub end: u32,
    /// The page number it is and nothing else, if it is one.
    pub number: Option<u32>,
    /// Its size against the page's usual line's.
    pub relative: f32,
    /// It has a letter or a digit in it. A bracket by itself at the foot of two pages is a footnote's, broken off.
    pub words: bool,
    /// Figures and nothing else that's a letter: a footnote's mark, axis labels, a table's cells.
    pub digits_only: bool,
    /// It's in a block set apart at the edge. Only a page number confirmed by its neighbours can be chrome without that.
    pub isolated: bool,
    /// Figures joined by `/` or `:`, or by two `.` or `-`, with no space: a date or a time, which a footer repeats, where marks, labels, decimals and section numbers are figures alone, spaced or with one point.
    pub dated: bool,
    /// Two words or more, and ten letters or more: enough to be the same line by its own text, where a short piece like "Table 3" isn't.
    pub substantial: bool,
    /// Its middle from the page's left edge and from its right, in points. A page number is at the same place across, or mirrored on facing pages; a figure in a ruler or a table isn't.
    pub from_left: f32,
    pub from_right: f32,
    /// Its row on the page, and where in it it is.
    pub place: InRow,
}

/// Where a piece is in its row: rows are decided whole, and a page number counted by its neighbours is alone in its row or at its end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InRow {
    pub row: u16,
    pub alone: bool,
    pub last: bool,
}

/// Which of a page's edge pieces are chrome, given its neighbours' edge pieces and how many pages away each neighbour is, `-2` to `2`: the ones a copy of body text leaves out, as `[start, end)` in the page's text. `index` is the page's place in its file, counted from 0.
pub fn chrome_spans(index: u32, page: &[EdgeLine], neighbours: &[(i64, &[EdgeLine])]) -> Vec<(u32, u32)> {
    let near = |line: &EdgeLine, seen: &EdgeLine| {
        seen.top == line.top && (seen.from_edge - line.from_edge).abs() <= SAME_PLACE_POINTS.max(SAME_PLACE_SIZES * line.size)
    };
    let across = |line: &EdgeLine, seen: &EdgeLine| {
        (seen.from_left - line.from_left).abs() <= SAME_ACROSS_POINTS || (seen.from_right - line.from_left).abs() <= SAME_ACROSS_POINTS
    };
    let alone = neighbours.iter().all(|(_, other)| other.is_empty());
    // The row holds the page's own number as a page's number is set: one page around saying its numbers step is enough then, as a book's running heads alternate, so at a chapter's ends only one page around has the same. A heading that happens to carry its page's number, "Chapter 5" on page 5, doesn't.
    let own_number = |line: &EdgeLine| {
        u32::from(line.framed_number) == index.saturating_add(1) || page.iter().any(|other| other.place.row == line.place.row && other.number.is_some())
    };
    // Its twin on the pages around, by `same_text`: the same, or the same but for numbers that step with the pages, which takes two pages, or one with the page's own number in it. Fewer pages around never needs fewer.
    let repeats = |line: &EdgeLine, same_text: &dyn Fn(&EdgeLine) -> bool, numbers: fn(&EdgeLine) -> [u32; 2]| {
        let (mut stepped, mut same) = (0, false);
        for (away, other) in neighbours {
            let (mut here_stepped, mut here_same) = (false, false);
            for seen in other.iter().filter(|seen| near(line, seen) && same_text(seen)) {
                match step_match(numbers(line), numbers(seen), *away) {
                    Some(true) => here_same = true,
                    Some(false) => here_stepped = true,
                    None => {}
                }
            }
            same |= here_same;
            stepped += usize::from(here_stepped && !here_same);
        }
        same || stepped >= STEPPED_ROW_PAGES || (stepped >= 1 && own_number(line))
    };
    let row_repeats = |line: &EdgeLine| repeats(line, &|seen| seen.row_key == line.row_key, |line| line.row_numbers);
    // A row set apart, with a letter in it and no bigger than the usual line, whose text, figures masked, is at its place on a page around, its figures the same or counting with the pages: all of it goes, its figures too.
    let rows_repeated: Vec<u16> = page
        .iter()
        .filter(|line| {
            line.isolated
                && line.relative <= MOST_SIZE
                && page.iter().any(|other| other.place.row == line.place.row && other.words && !other.digits_only)
                && row_repeats(line)
        })
        .map(|line| line.place.row)
        .collect();
    page.iter()
        .filter(|line| {
            if rows_repeated.contains(&line.place.row) {
                return true;
            }
            // A page number is alone in its row, ends it or is centred on the page, about the size page numbers are, and counted with its neighbours' numbers at its place. One that starts a row from the margin is a question's, a heading's or a footnote's.
            let number = line.number.is_some_and(|value| {
                let centred = (line.from_left - line.from_right).abs() <= CENTRED * (line.from_left + line.from_right);
                let shaped = (line.place.alone || line.place.last || centred)
                    && line.relative >= NUMBER_LEAST_SIZE
                    && (line.relative <= NUMBER_MOST_SIZE || line.size <= NUMBER_MOST_POINTS);
                let stepped = neighbours.iter().any(|(away, other)| {
                    other
                        .iter()
                        .any(|seen| near(line, seen) && across(line, seen) && seen.number.is_some_and(|theirs| i64::from(theirs) - i64::from(value) == *away))
                });
                shaped && (stepped || (alone && line.isolated && value < LONE_PAGE_NUMBER_BELOW))
            });
            // A piece long enough to say so by its own text, or a date or a version, at its place on a page around.
            let piece_repeated = line.isolated
                && line.relative <= MOST_SIZE
                && (line.substantial || (line.dated && line.relative >= NUMBER_LEAST_SIZE))
                && repeats(line, &|seen| seen.key == line.key, |line| line.numbers);
            line.words && (number || piece_repeated)
        })
        .map(|line| (line.start, line.end))
        .collect()
}

/// Whether numbers `away` pages from these are the same text again: `None` if not, and whether they're the same if so. Numbers that differ have to step with the pages, each by `away`, as "Page 12" does on the next page and "Article 12" two pages on doesn't.
fn step_match(mine: [u32; 2], theirs: [u32; 2], away: i64) -> Option<bool> {
    let mut exact = true;
    for (&mine, &theirs) in mine.iter().zip(&theirs) {
        if mine == theirs {
            continue;
        }
        if mine == NO_NUMBER || theirs == NO_NUMBER || i64::from(theirs) - i64::from(mine) != away {
            return None;
        }
        exact = false;
    }
    Some(exact)
}

/// The first two runs of digits in a text, as numbers, [`NO_NUMBER`] past the ones it has. A run too long for one is the largest below it.
pub fn numbers(text: impl Iterator<Item = char>) -> [u32; 2] {
    let mut found = [NO_NUMBER; 2];
    let (mut count, mut current): (usize, Option<u32>) = (0, None);
    for ch in text.chain(std::iter::once(' ')) {
        if let Some(digit) = ch.to_digit(10).filter(|_| ch.is_ascii_digit()) {
            current = Some(current.unwrap_or(0).saturating_mul(10).saturating_add(digit).min(NO_NUMBER - 1));
        } else if let Some(number) = current.take() {
            if count < found.len() {
                found[count] = number;
            }
            count += 1;
            if count == found.len() {
                break;
            }
        }
    }
    found
}

/// The number a piece of text is framed by as a page's own number is set: after a separator or a word for page at its end (`L 119/6`, `XX:2`, `Report · 5`, `Page 5`, `p. 5`), or before a separator at its start (`5 | Report`, `5 — Report`). [`NO_FRAMED`] if none, or past what a page number is. A heading's number, "Chapter 5" or "Lesson 1", isn't framed so.
pub fn framed_number(text: &str) -> u16 {
    let text = text.trim();
    let digits_at_end = text.len() - text.trim_end_matches(|ch: char| ch.is_ascii_digit()).len();
    let digits_at_start = text.len() - text.trim_start_matches(|ch: char| ch.is_ascii_digit()).len();
    let value = |digits: &str| {
        digits
            .parse::<u16>()
            .ok()
            .filter(|value| *value != NO_FRAMED && (1..=5).contains(&digits.len()))
    };
    if digits_at_end > 0 && digits_at_end < text.len() {
        let (before, digits) = text.split_at(text.len() - digits_at_end);
        let before = before.trim_end();
        let separated = before.ends_with(['/', '|', '·', '•', '–', '—', ':']);
        let word = before
            .rsplit(char::is_whitespace)
            .next()
            .map(|word| word.trim_end_matches('.').to_lowercase())
            .is_some_and(|word| matches!(word.as_str(), "page" | "p" | "pg" | "side" | "seite" | "página" | "pagina" | "sida"));
        if (separated || word)
            && let Some(value) = value(digits)
        {
            return value;
        }
    }
    if digits_at_start > 0 && digits_at_start < text.len() {
        let (digits, after) = text.split_at(digits_at_start);
        if after.trim_start().starts_with(['|', '·', '•', '–', '—'])
            && let Some(value) = value(digits)
        {
            return value;
        }
    }
    NO_FRAMED
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

/// The page number a piece is, if it's one and nothing else: `12`, `xii`, `Page 3`, `p. 3`, `3 of 40`, `3/40`, `- 3 -`, `| 3 |`. Roman numerals only alone or after a word for page, as a word like "mix" is one.
pub fn page_number(text: &str) -> Option<u32> {
    let decoration = |ch: char| {
        matches!(
            ch,
            '-' | '–' | '—' | '|' | '·' | '•' | '[' | ']' | '(' | ')' | '<' | '>' | '«' | '»' | '/' | '.' | ':'
        ) || ch.is_whitespace()
    };
    let lower = text.to_lowercase();
    // What's round it must be round it: `- 3 -` and `(3)` are page numbers, and `3)`, `3.` and `2011-` are a list's or a table's.
    let core = lower.trim_matches(decoration);
    let (before, after) = (
        &lower[..lower.len() - lower.trim_start_matches(decoration).len()],
        &lower[lower.trim_end_matches(decoration).len()..],
    );
    let has = |side: &str, set: &[char]| side.chars().any(|ch| set.contains(&ch));
    let dashes = ['-', '–', '—'];
    let balanced = has(before, &dashes) == has(after, &dashes)
        && has(before, &['(', '[']) == has(after, &[')', ']'])
        && !(before.trim().is_empty() && has(after, &['.', ':']));
    if !balanced {
        return None;
    }
    let words: Vec<&str> = core
        .split(|ch: char| ch.is_whitespace() || ch == '/')
        .map(|word| word.trim_matches(decoration))
        .filter(|word| !word.is_empty())
        .collect();
    let digits = |word: &str| {
        ((1..=5).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| word.parse::<u32>().ok())
            .flatten()
    };
    let roman = |word: &str| {
        ((1..=7).contains(&word.len()) && word.bytes().all(|byte| matches!(byte, b'i' | b'v' | b'x' | b'l' | b'c')))
            .then(|| roman_value(word))
            .flatten()
    };
    let page_word = |word: &str| matches!(word, "page" | "p" | "pg" | "pp" | "side" | "seite" | "página" | "pagina" | "sida");
    let of_word = |word: &str| matches!(word, "of" | "av" | "de" | "von" | "sur" | "di");
    match words.as_slice() {
        [one] => digits(one).or_else(|| roman(one)),
        [word, one] if page_word(word) => digits(one).or_else(|| roman(one)),
        [one, two] => digits(two).and(digits(one)),
        [first, second, third] if of_word(second) => digits(third).and(digits(first)),
        [first, second, third] if page_word(first) => digits(third).and(digits(second)),
        [word, one, of, two] if page_word(word) && of_word(of) => digits(two).and(digits(one)),
        _ => None,
    }
}

/// A Roman numeral's value, if it's a well-formed one: `xii` is 12, and `iiv` is nothing.
fn roman_value(word: &str) -> Option<u32> {
    let value = |byte: u8| match byte {
        b'i' => 1,
        b'v' => 5,
        b'x' => 10,
        b'l' => 50,
        _ => 100,
    };
    let bytes = word.as_bytes();
    let mut total = 0i64;
    for (at, &byte) in bytes.iter().enumerate() {
        let here = value(byte);
        if bytes.get(at + 1).is_some_and(|&next| value(next) > here) {
            total -= here
        } else {
            total += here
        }
    }
    let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
    // Only the canonical spelling counts, so random letters that happen to be numerals don't.
    (to_roman(total) == word).then_some(total)
}

fn to_roman(mut value: u32) -> String {
    let mut out = String::new();
    for (step, letters) in [
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ] {
        while value >= step {
            out.push_str(letters);
            value -= step;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: an edge piece grows, so a page's text and the app's 64 pages of edge pieces cost more than the docs say.
    #[test]
    fn an_edge_piece_stays_small() {
        assert!(size_of::<EdgeLine>() <= 80, "{}", size_of::<EdgeLine>());
        eprintln!("EdgeLine is {} B", size_of::<EdgeLine>());
    }

    /// Failure: a page number in one of the usual forms is copied with the body text, or read as another number, or a word, a list's number or a figure that isn't one is taken for it.
    #[test]
    fn page_numbers_are_told_from_other_lines() {
        for (number, value) in [
            ("12", 12),
            ("xii", 12),
            ("Page 3", 3),
            ("p. 3", 3),
            ("3 of 40", 3),
            ("3/40", 3),
            ("Page 3/40", 3),
            ("(3)", 3),
            ("-2-", 2),
            ("-- 2 --", 2),
            ("- 3 -", 3),
            ("– 7 –", 7),
            ("| 54 |", 54),
            ("Page 3 of 40", 3),
            ("PAGE 12", 12),
            ("08", 8),
            ("iv", 4),
        ] {
            assert_eq!(page_number(number), Some(value), "{number}");
        }
        for not in [
            "mix",
            "2024 Annual Report",
            "Figure 3",
            "3.14 percent",
            "Table 1 of the survey",
            "",
            "Chapter 2",
            "123456",
            "1)",
            "i)",
            "3.",
            "2011-",
            "(",
            "- 3",
            "iiv",
            "civil",
        ] {
            assert_eq!(page_number(not), None, "{not}");
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
        let digits_only = text.chars().any(|ch| ch.is_ascii_digit()) && !text.chars().any(char::is_alphabetic);
        EdgeLine {
            top,
            from_edge,
            size: 9.0,
            key: key(text.chars()),
            row_key: key(text.chars()),
            row_numbers: numbers(text.chars()),
            numbers: numbers(text.chars()),
            framed_number: framed_number(text),
            start: 0,
            end: text.len() as u32,
            number: page_number(text),
            relative: 1.0,
            words: true,
            digits_only,
            isolated: true,
            dated: false,
            substantial: text.split_whitespace().count() >= 2 && text.chars().filter(|ch| ch.is_alphabetic()).count() >= 10,
            from_left: 300.0,
            from_right: 312.0,
            place: InRow {
                row: 0,
                alone: true,
                last: true,
            },
        }
    }

    /// Failure: a running head repeated on the next page isn't left out, or one at the other edge, or far from where it was, or a heading bigger than body text, is taken for one.
    #[test]
    fn a_line_is_chrome_where_it_repeats() {
        let head = [line(true, 40.0, "Annual report 2024")];
        assert_eq!(
            chrome_spans(5, &head, &[(1, &[line(true, 41.0, "Annual report 2024")])]).len(),
            1,
            "the same place on the next page"
        );
        assert!(
            chrome_spans(5, &head, &[(1, &[line(false, 40.0, "Annual report 2024")])]).is_empty(),
            "the other edge"
        );
        assert!(
            chrome_spans(5, &head, &[(1, &[line(true, 80.0, "Annual report 2024")])]).is_empty(),
            "far from it"
        );
        assert!(chrome_spans(5, &head, &[]).is_empty(), "nothing to compare with");
        let big = [EdgeLine { relative: 1.5, ..head[0] }];
        assert!(chrome_spans(5, &big, &[(1, &head)]).is_empty(), "a heading");
    }

    /// Failure: a page number on the last line of a page's text stays in a copy, as on 24 pages of the owner's part B, because it isn't set apart; or numbers that don't count pages, axis labels on two figure pages or a cover's year, are taken for page numbers; or a footnote's mark is.
    #[test]
    fn a_page_number_is_one_its_neighbours_count_with() {
        let bottom = |text: &str, isolated: bool| EdgeLine {
            isolated,
            ..line(false, 30.0, text)
        };
        assert_eq!(
            chrome_spans(5, &[bottom("82", false)], &[(-1, &[bottom("81", true)]), (2, &[bottom("84", false)])]).len(),
            1,
            "counted with its neighbours, set apart or not"
        );
        assert!(
            chrome_spans(5, &[bottom("1012", true)], &[(1, &[bottom("1010", true)])]).is_empty(),
            "axis labels on two figure pages"
        );
        assert!(
            chrome_spans(5, &[bottom("1000", true)], &[(1, &[bottom("1000", true)])]).is_empty(),
            "the same figure twice"
        );
        assert!(
            chrome_spans(
                5,
                &[EdgeLine {
                    relative: 2.5,
                    ..line(true, 30.0, "2023")
                }],
                &[(1, &[bottom("2", true)])]
            )
            .is_empty(),
            "a cover's year"
        );
        assert_eq!(
            chrome_spans(5, &[bottom("17", true)], &[]).len(),
            1,
            "a page number, with no page around to compare"
        );
        assert!(
            chrome_spans(5, &[bottom("2024", true)], &[]).is_empty(),
            "a year at the foot of a one-page letter"
        );
        assert!(
            chrome_spans(5, &[bottom("17", false)], &[]).is_empty(),
            "not set apart, and nothing to count with"
        );
        let mark = |page: u32| EdgeLine {
            relative: 0.43,
            ..bottom(&page.to_string(), true)
        };
        assert!(
            chrome_spans(5, &[mark(1)], &[(1, &[mark(2)])]).is_empty(),
            "footnote marks, though they count up"
        );
        assert!(
            chrome_spans(5, &[bottom("0 25 50 75 100", true)], &[(1, &[bottom("0 25 50 75 100", true)])]).is_empty(),
            "a slide's axis labels repeated"
        );
        let at = |text: &str, from_left: f32| EdgeLine {
            from_left,
            from_right: 612.0 - from_left,
            ..bottom(text, true)
        };
        assert!(
            chrome_spans(5, &[at("1", 100.0)], &[(1, &[at("2", 140.0)])]).is_empty(),
            "a ruler's figures, one along from the other"
        );
        assert_eq!(
            chrome_spans(5, &[at("4", 80.0)], &[(1, &[at("5", 532.0)])]).len(),
            1,
            "facing pages, the number mirrored"
        );
        let date = EdgeLine {
            dated: true,
            ..bottom("3/14/2019", true)
        };
        assert_eq!(chrome_spans(5, &[date], &[(1, &[date])]).len(), 1, "a footer's date");
        let caption = EdgeLine {
            row_key: 1,
            ..line(true, 30.0, "Table 3")
        };
        assert!(
            chrome_spans(
                5,
                &[caption],
                &[(
                    1,
                    &[EdgeLine {
                        row_key: 2,
                        ..line(true, 30.0, "Table 4")
                    }]
                )]
            )
            .is_empty(),
            "a short piece of rows that differ"
        );
    }

    /// Failure: numbered headings at the top of pages, "Article 12" and "Article 13", read as one running head once their digits are masked, so real headings leave a copy (a made `articles.pdf` in `recto-cli`'s tests too); or a running head whose number counts with the pages, "Page 12", stays, or one of a book's alternating heads at a chapter's end, with only one page around to agree.
    #[test]
    fn a_rows_numbers_step_with_the_pages_or_it_is_not_repeated() {
        let top = |text: &str| line(true, 40.0, text);
        let heading = [top("Article 12")];
        assert!(
            chrome_spans(5, &heading, &[(2, &[top("Article 13")])]).is_empty(),
            "two pages on, one article on"
        );
        assert!(
            chrome_spans(5, &heading, &[(1, &[top("Article 13")]), (2, &[])]).is_empty(),
            "the next page's heading only: one page isn't enough"
        );
        assert!(
            chrome_spans(5, &heading, &[(1, &[top("Article 15")])]).is_empty(),
            "numbers that don't count with the pages"
        );
        assert!(
            chrome_spans(5, &heading, &[(1, &[top("Article 13")])]).is_empty(),
            "a two-page file's only other page agreeing isn't enough"
        );
        let head = [top("Page 12 of 40")];
        assert_eq!(
            chrome_spans(5, &head, &[(-1, &[top("Page 11 of 40")]), (1, &[top("Page 13 of 40")]), (2, &[])]).len(),
            1,
            "a running head that counts with the pages"
        );
        assert_eq!(
            chrome_spans(5, &[top("Chapter 9 report")], &[(1, &[top("Chapter 9 report")]), (2, &[])]).len(),
            1,
            "the same numbers, once is enough"
        );
        assert_eq!(
            chrome_spans(5, &[top("L 119/6")], &[(-1, &[]), (2, &[top("L 119/8")])]).len(),
            1,
            "one page is enough when the row has the page's own number: a book's alternating head at a chapter's end"
        );
        let author = |number: &str, row_key: u64, from_left: f32| EdgeLine {
            row_key,
            row_numbers: numbers(number.chars()),
            from_left,
            from_right: 612.0 - from_left,
            place: InRow {
                row: 0,
                alone: false,
                last: from_left > 300.0,
            },
            ..top(number)
        };
        let row = |name: &str, number: &str| {
            let row_key = key(format!("{name} {number}").chars());
            [author(name, row_key, 100.0), author(number, row_key, 500.0)]
        };
        assert_eq!(
            chrome_spans(5, &row("Cox et al.", "20"), &[(-1, &[]), (2, &row("Cox et al.", "22"))]).len(),
            2,
            "or a page number as a piece of its own"
        );
        assert_eq!(numbers("L 119/12, and 7".chars()), [119, 12]);
        assert_eq!(framed_number("L 119/12"), 12);
        assert_eq!(numbers("none".chars()), [NO_NUMBER; 2]);
        assert_eq!(numbers("99999999999".chars()), [NO_NUMBER - 1, NO_NUMBER]);
    }

    /// Failure: a heading of a few words whose number counts with the pages, "Lesson 12 Worksheet" then "Lesson 13 Worksheet", leaves a copy because it's long enough to repeat as a piece by itself and that path never looked at its numbers; or one whose numbers don't step at all does (review of `build/release-fixes`).
    #[test]
    fn a_long_heading_steps_by_the_same_rule_as_a_row() {
        let top = |text: &str| line(true, 40.0, text);
        let other = top("Overview of the course");
        assert!(chrome_spans(5, &[top("Lesson 12 Worksheet")], &[(1, &[top("Lesson 14 Worksheet")]), (2, &[])]).is_empty());
        assert!(
            chrome_spans(5, &[top("Lesson 12 Worksheet")], &[(1, &[top("Lesson 13 Worksheet")]), (2, &[other])]).is_empty(),
            "one page agreeing isn't enough"
        );
        let date = EdgeLine {
            dated: true,
            ..line(false, 30.0, "3/14/2019")
        };
        let other_date = EdgeLine {
            dated: true,
            ..line(false, 30.0, "4/15/2020")
        };
        assert!(chrome_spans(5, &[date], &[(1, &[other_date])]).is_empty(), "a date that isn't the same");
        assert_eq!(chrome_spans(5, &[date], &[(1, &[date])]).len(), 1, "a footer's date, the same");
    }

    /// Failure: a heading that happens to carry its page's number, "Chapter 5" on page 5 or "Lesson 1" on page 1, counts as the page's own number, so one page around agreeing takes it out of a copy (review of `build/release-fixes`).
    #[test]
    fn a_heading_with_its_pages_number_stays() {
        let top = |text: &str| line(true, 40.0, text);
        let other = top("Overview of the course");
        assert!(chrome_spans(4, &[top("Chapter 5")], &[(-2, &[]), (-1, &[]), (1, &[top("Chapter 6")]), (2, &[])]).is_empty());
        assert!(chrome_spans(0, &[top("Lesson 1")], &[(1, &[top("Lesson 2")]), (2, &[other])]).is_empty());
        assert!(chrome_spans(1, &[top("Lesson 2")], &[(-1, &[top("Lesson 1")]), (1, &[other]), (2, &[])]).is_empty());
        // Set as a page's number is, it's the page's own.
        assert_eq!(
            chrome_spans(4, &[top("Report · 5")], &[(-1, &[]), (1, &[top("Report · 6")]), (2, &[])]).len(),
            1
        );
        for (text, value) in [("L 119/6", 6), ("XX:2", 2), ("Page 5", 5), ("p. 5", 5), ("5 | Report", 5), ("Report — 12", 12)] {
            assert_eq!(framed_number(text), value, "{text}");
        }
        for text in ["Chapter 5", "Lesson 1", "2024", "Section 3.2", "L 119/123456", "5 Results", ""] {
            assert_eq!(framed_number(text), NO_FRAMED, "{text}");
        }
    }

    /// Failure: a two-page file's numbered headings, "Exercise 7" then "Exercise 8", both leave a copy as there's only one page around to agree; or a page that couldn't be read counts differently as an empty neighbour and as none, so `recto text`'s chrome and `recto copy`'s disagree when a page fails (review of `build/release-fixes`).
    #[test]
    fn fewer_pages_around_never_lowers_the_bar() {
        let top = |text: &str| line(true, 40.0, text);
        assert!(chrome_spans(0, &[top("Exercise 7")], &[(1, &[top("Exercise 8")])]).is_empty());
        assert!(chrome_spans(1, &[top("Exercise 8")], &[(-1, &[top("Exercise 7")])]).is_empty());
        let cases: Vec<(u32, Vec<EdgeLine>, Vec<(i64, Vec<EdgeLine>)>)> = vec![
            (
                5,
                vec![top("Page 12 of 40")],
                vec![(-1, vec![top("Page 11 of 40")]), (1, vec![top("Page 13 of 40")])],
            ),
            (5, vec![top("L 119/6")], vec![(2, vec![top("L 119/8")])]),
            (5, vec![top("Article 12")], vec![(1, vec![top("Article 13")])]),
            (5, vec![line(false, 30.0, "17")], vec![]),
            (5, vec![top("Annual report 2024")], vec![(1, vec![top("Annual report 2024")])]),
        ];
        for (index, page, neighbours) in &cases {
            let read: Vec<(i64, &[EdgeLine])> = neighbours.iter().map(|(away, lines)| (*away, lines.as_slice())).collect();
            // The pages around that weren't read, kept as empty rather than left out.
            let empty: &[EdgeLine] = &[];
            let mut with_failed = read.clone();
            with_failed.extend(
                [-2i64, -1, 1, 2]
                    .into_iter()
                    .filter(|away| !read.iter().any(|(seen, _)| seen == away))
                    .map(|away| (away, empty)),
            );
            assert_eq!(chrome_spans(*index, page, &read), chrome_spans(*index, page, &with_failed), "{page:?}");
        }
    }

    /// Failure: hostile edge pieces, NaN places, extreme distances and thousands of them on a neighbour, panic or take long.
    #[test]
    fn hostile_pieces_do_no_harm() {
        let mut odd = line(true, f32::NAN, "Page 1");
        odd.size = f32::NAN;
        let many: Vec<EdgeLine> = (0..5000)
            .map(|at| EdgeLine {
                row_numbers: [at, NO_NUMBER - 1],
                place: InRow {
                    row: (at % 65536) as u16,
                    alone: true,
                    last: true,
                },
                ..line(true, 40.0, "Page 1")
            })
            .collect();
        let _ = chrome_spans(u32::MAX, &[odd], &[(i64::MIN, &many), (i64::MAX, &many)]);
        let _ = chrome_spans(u32::MAX, &many[..64], &[(i64::MIN, &many[..64]), (i64::MAX, &many[..64])]);
        assert_eq!(numbers("1".repeat(100_000).chars())[0], NO_NUMBER - 1);
        assert_eq!(framed_number(&"9".repeat(100_000)), NO_FRAMED);
    }
}
