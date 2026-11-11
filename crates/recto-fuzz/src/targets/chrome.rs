//! Chrome detection, which a copy and a snapshot's box use to leave out running heads, footers and page numbers: on positions and text from hostile pages. It runs in the host, on what a worker sent, for every page a copy reads, so a NaN, a size of 1e30, a page number past `u32`, a page of thousands of rows at its edge or a neighbour that's nothing like its page has to do no harm and take no time.
//!
//! Three ways in: text alone, through the rules that read it (`numbers`, `key`, `framed_number`, `page_number`); edge pieces made straight from the bytes, through `chrome_spans`, with neighbours any number of pages away; and whole pages of rows, five of them, each built from its own characters and boxes, compared with each other as a copy compares them. `select` only ever compares a page with itself.
//!
//! What must hold: no panic, bounded time and memory; what's left out is one of the page's own pieces, in order, inside its text; a page keeps no more than its cap of edge pieces; the rules on text give what they say they give.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_index::chrome::{self, EdgeLine, InRow, MAX_EDGE_LINES, NO_FRAMED, NO_NUMBER};
use recto_index::select::{CopiedText, MAX_SELECTABLE_CHARS, PageText, Range, Rect, chrome_for};
use recto_proto::{CharBox, CharRecord, PageFrame};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The most characters a page built here has: what a page may have to be selected on, held down by a smoke run on a debug build, where five such pages take seconds.
pub static MOST_CHARS: AtomicUsize = AtomicUsize::new(MAX_SELECTABLE_CHARS);

const BUDGET: Budget = Budget {
    time: Duration::from_secs(60),
    memory: 512 * 1024 * 1024,
};

/// Pieces of text the rules look for, and ones that have tripped text code: page numbers in their forms, running heads, dates, headings that carry numbers, case that changes length, and digits that aren't ASCII.
const PIECES: &[&str] = &[
    "12",
    "xii",
    "Page 3 of 40",
    "– 7 –",
    "- 3 -",
    "(4)",
    "[iv]",
    "p. 5",
    "L 119/6",
    "Report · 5",
    "5 | Report",
    "Chapter 5",
    "Article 12",
    "3/14/2019",
    "4.5.2016",
    "Official Journal of the European Union",
    "Annual report 2024",
    "Table 3",
    "1 Introduction",
    "99999999999999999999",
    "65535",
    "0",
    "İSTANBUL",
    "ẞ",
    "Ⅻ",
    "١٢",
    "\u{FFFE}",
    "\u{202E}",
    "\u{200B}",
    " ",
    "",
    "ccccccc",
    "mmxxiv",
    "Seite 4 von 9",
    "{n}",
    "Page {n}",
    "{n} | Notes",
];
const EDGES: &[f32] = &[
    0.0,
    10.0,
    36.0,
    700.0,
    -3.0,
    1e-30,
    1e30,
    f32::MAX,
    f32::MIN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
];
const SIZES: &[f32] = &[8.0, 8.0, 9.0, 10.0, 12.0, 24.0, 0.0, -8.0, 1e-30, 1e30, f32::MAX, f32::INFINITY, f32::NAN];
const NUMBERS: &[u32] = &[0, 1, 2, 3, 4, 12, 13, 14, 999, 1000, 65_534, NO_NUMBER - 1, NO_NUMBER];
const FRAMED: &[u16] = &[NO_FRAMED, 0, 1, 2, 3, 12, 13, 65_534];
const AWAY: &[i64] = &[-2, -1, 1, 2, 0, 3, -3, i64::MIN, i64::MAX];
const INDEXES: &[u32] = &[0, 1, 2, 11, 12, u32::MAX - 1, u32::MAX];

/// Catches: a panic, a span left out that isn't one of the page's own or is outside its text, an edge piece past its cap, a rule on text that gives what it says it doesn't, and anything worse than linear on a hostile page.
pub fn check(data: &[u8]) {
    let mut take = Take(data);
    let route = take.u8() % 3;
    within("chrome on a hostile page", BUDGET, || match route {
        0 => text_rules(take.rest()),
        1 => edge_pieces(&mut take),
        _ => whole_pages(&mut take),
    });
}

/// A text from the pieces, each chosen by a byte, with `{n}` a number near this page's.
fn text_from(take: &mut Take<'_>, page: u32) -> String {
    let mut text = String::new();
    for _ in 0..1 + take.u8() % 4 {
        let piece = take.pick(PIECES);
        // Its own number most of the time, and now and then one either side.
        let near = page
            .saturating_add(1)
            .saturating_add(u32::from(take.pick(&[1u8, 1, 1, 0, 2])))
            .saturating_sub(1);
        text.push_str(&piece.replace("{n}", &near.to_string()));
        // Two spaces set the next piece apart.
        text.push_str(take.pick(&[" ", " ", "  "]));
    }
    text
}

fn check_text(text: &str) {
    let [first, second] = chrome::numbers(text.chars());
    assert!(first != NO_NUMBER || second == NO_NUMBER, "a second number with no first in {text:?}");
    // Digits masked: any run of ASCII digits is the same as any other.
    let masked: String = {
        let mut out = String::new();
        let mut in_digits = false;
        for ch in text.chars() {
            if ch.is_ascii_digit() {
                if !in_digits {
                    out.push('7');
                }
                in_digits = true;
            } else {
                out.push(ch);
                in_digits = false;
            }
        }
        out
    };
    assert_eq!(chrome::key(text.chars()), chrome::key(masked.chars()), "digits changed the key of {text:?}");
    let framed = chrome::framed_number(text);
    if framed != NO_FRAMED {
        assert!(framed >= 1 || text.contains('0'), "{text:?} framed {framed}");
        assert!(text.contains(&framed.to_string()), "{text:?} framed {framed}, which it doesn't hold");
    }
    if let Some(number) = chrome::page_number(text) {
        assert!(number <= 99_999, "{text:?} is page {number}");
    }
}

fn text_rules(bytes: &[u8]) {
    check_text(&String::from_utf8_lossy(bytes));
    let mut take = Take(bytes);
    while !take.is_empty() {
        let page = take.pick(INDEXES);
        check_text(&text_from(&mut take, page));
    }
}

fn edge_line(take: &mut Take<'_>) -> EdgeLine {
    let start = take.pick(&[0u32, 1, 5, 40, 100, u32::MAX - 1, u32::MAX]);
    let flags = take.u8();
    EdgeLine {
        top: flags & 1 == 1,
        from_edge: take.pick(EDGES),
        size: take.pick(SIZES),
        key: u64::from(take.u8() % 4),
        row_key: u64::from(take.u8() % 4),
        row_numbers: [take.pick(NUMBERS), take.pick(NUMBERS)],
        numbers: [take.pick(NUMBERS), take.pick(NUMBERS)],
        framed_number: take.pick(FRAMED),
        start,
        end: start.saturating_add(u32::from(take.u8())),
        number: (flags & 2 == 2).then(|| take.pick(NUMBERS)),
        relative: take.pick(&[1.0, 1.0, 0.5, 0.4, 1.5, 3.0, 0.0, -1.0, f32::INFINITY, f32::NAN]),
        words: flags & 4 == 4,
        digits_only: flags & 8 == 8,
        isolated: flags & 16 == 16,
        dated: flags & 32 == 32,
        substantial: flags & 64 == 64,
        from_left: take.pick(EDGES),
        from_right: take.pick(EDGES),
        place: InRow {
            row: u16::from(take.u8() % 5),
            alone: flags & 128 == 128,
            last: take.u8() & 1 == 1,
        },
    }
}

fn lines_of(take: &mut Take<'_>) -> Vec<EdgeLine> {
    // Twice a page's cap, as a slice from anywhere else could be longer than one `PageText` keeps.
    (0..usize::from(take.u8()) % (MAX_EDGE_LINES * 2 + 1)).map(|_| edge_line(take)).collect()
}

fn edge_pieces(take: &mut Take<'_>) {
    let index = take.pick(INDEXES);
    let page = lines_of(take);
    let neighbours: Vec<(i64, Vec<EdgeLine>)> = (0..take.u8() % 9).map(|_| (take.pick(AWAY), lines_of(take))).collect();
    let borrowed: Vec<(i64, &[EdgeLine])> = neighbours.iter().map(|(away, lines)| (*away, lines.as_slice())).collect();
    for neighbours in [&borrowed[..], &[]] {
        let spans = chrome::chrome_spans(index, &page, neighbours);
        // What's left out is the page's own pieces, in their order, each at most once.
        let mut own = page.iter().map(|line| (line.start, line.end));
        for span in &spans {
            assert!(own.any(|piece| piece == *span), "{span:?} isn't one of the page's pieces, in order");
        }
    }
}

/// A row of a page: how far down its letters' tops are, how tall they are, where it starts across, and its text.
type Row = (f32, f32, f32, String);

/// What a document's running heads and footers say, chosen once and set on every page with its own number: pieces of [`PIECES`] each, with where across they go.
struct Template {
    heads: Vec<(Vec<&'static str>, f32)>,
    feet: Vec<(Vec<&'static str>, f32)>,
}

fn template(take: &mut Take<'_>) -> Template {
    let across = |take: &mut Take<'_>| take.pick(&[72.0f32, 72.0, 290.0, 520.0]);
    let rows = |take: &mut Take<'_>| {
        (0..take.u8() % 3)
            .map(|_| ((0..1 + take.u8() % 3).map(|_| take.pick(PIECES)).collect(), across(take)))
            .collect()
    };
    Template {
        heads: rows(take),
        feet: rows(take),
    }
}

/// A row of the template on a page: its pieces with the page's number, or now and then one either side, and two spaces between pieces.
fn set(pieces: &[&str], page: u32, take: &mut Take<'_>) -> String {
    let number = page.saturating_add(u32::from(take.pick(&[1u8, 1, 1, 1, 0, 2])));
    pieces
        .iter()
        .map(|piece| piece.replace("{n}", &number.to_string()))
        .collect::<Vec<_>>()
        .join(take.pick(&[" ", "  "]))
}

/// A page laid out as most are: the document's running heads, lines of body text and its footers with the page's number, at places and sizes the bytes nudge. Now and then thousands of rows at the top edge, so the block and gap rules see a page of nothing but edge.
fn laid_out(take: &mut Take<'_>, form: &Template, page: u32, height: f32) -> Vec<Row> {
    let nudge = |take: &mut Take<'_>| take.pick(&[0.0f32, 0.0, 0.0, 0.0, 0.5, -0.5, 3.0, -3.0, 14.0]);
    let mut rows = Vec::new();
    for (row, (pieces, across)) in form.heads.iter().enumerate() {
        let y = 30.0 + 12.0 * row as f32 + nudge(take);
        rows.push((y, take.pick(&[9.0f32, 9.0, 9.0, 14.0]), *across, set(pieces, page, take)));
    }
    if take.u8() == 0xFF {
        let many = 2000 + usize::from(take.u16() % 4000);
        for row in 0..many {
            rows.push((60.0 + row as f32 * 0.02, 1.0, 72.0, "12".to_string()));
        }
    }
    let body = 3 + take.u8() % 40;
    for row in 0..body {
        let words = take.pick(&[
            "The body of the page goes on here",
            "and on, line after line",
            "1 Introduction",
            "Table 3 shows the results",
            "12",
        ]);
        rows.push((100.0 + 14.0 * f32::from(row), 10.0, 72.0, words.to_string()));
    }
    for (row, (pieces, across)) in form.feet.iter().enumerate() {
        let y = height - 40.0 + 12.0 * row as f32 + nudge(take);
        rows.push((y, take.pick(&[8.0f32, 8.0, 8.0, 10.0]), *across, set(pieces, page, take)));
    }
    rows
}

/// Rows as the bytes say, with sizes, steps and places that are anything, NaN and huge among them.
fn wild_rows(take: &mut Take<'_>, page: u32) -> Vec<Row> {
    let mode = take.u8();
    let count = if mode >= 0xF0 {
        2000 + usize::from(take.u16() % 4000)
    } else {
        1 + usize::from(mode % 24)
    };
    let mut y = take.pick(&[20.0f32, 36.0, 0.0, -5.0, f32::NAN]);
    let pattern: Vec<(f32, f32, String)> = (0..1 + take.u8() % 8)
        .map(|_| {
            (
                take.pick(&[12.0f32, 2.0, 0.1, 0.0, 60.0, 400.0, -12.0, f32::NAN, 1e30]),
                take.pick(SIZES),
                text_from(take, page),
            )
        })
        .collect();
    (0..count)
        .map(|row| {
            let (step, size, text) = &pattern[row % pattern.len()];
            y += step;
            (y, *size, take.pick(EDGES), text.clone())
        })
        .collect()
}

/// Rows as characters and boxes, the way a worker gives them: a piece of a row is set apart from the next by a wide gap where its text has two spaces.
fn characters(rows: &[Row], height: f32) -> (Vec<CharRecord>, Vec<CharBox>) {
    let (mut chars, mut boxes) = (Vec::new(), Vec::new());
    let most = MOST_CHARS.load(Ordering::Relaxed);
    for (y, size, start, text) in rows {
        let mut x = *start;
        let mut last = ' ';
        for ch in text.chars() {
            if chars.len() >= most {
                return (chars, boxes);
            }
            if ch == ' ' && last == ' ' {
                x += 6.0 * size;
            }
            last = ch;
            chars.push(CharRecord::new(ch as u32, 0));
            let bottom = height - y - size;
            boxes.push([x, bottom, x + size * 0.5, bottom + size]);
            x += size * 0.55;
        }
    }
    (chars, boxes)
}

fn whole_pages(take: &mut Take<'_>) {
    let wild = take.u8().is_multiple_of(4);
    let (width, height) = if wild {
        (
            take.pick(&[612.0f32, 595.0, 100.0, 1e-3, 1e30]),
            take.pick(&[792.0f32, 842.0, 100.0, 0.0, f32::NAN]),
        )
    } else {
        take.pick(&[(612.0f32, 792.0f32), (595.0, 842.0), (792.0, 612.0)])
    };
    let frame = PageFrame {
        turns: if wild { take.u8() & 3 } else { 0 },
        left: 0.0,
        bottom: 0.0,
        right: width,
        top: height,
    };
    let count = 1 + u32::from(take.u8() % 5);
    // The number a page says it is: its own, or in a wild run one far off.
    let first = if wild {
        take.pick(&[0u32, 1, 10, u32::MAX - 5])
    } else {
        take.pick(&[0u32, 0, 0, 4])
    };
    let form = template(take);
    let texts: Vec<PageText> = (0..count)
        .map(|at| {
            let page = first.saturating_add(at);
            let rows = if wild { wild_rows(take, page) } else { laid_out(take, &form, page, height) };
            let (chars, boxes) = characters(&rows, height);
            PageText::build(&chars, &boxes, frame, true, false)
        })
        .collect();
    for (at, text) in texts.iter().enumerate() {
        let edges = text.edge_lines();
        assert!(edges.len() <= MAX_EDGE_LINES, "{} edge pieces", edges.len());
        assert!(
            edges.iter().all(|line| line.start <= line.end && line.end <= text.len()),
            "an edge piece outside its page"
        );
        let page = at as u32;
        let spans = chrome_for(page, text, count, |other| texts.get(other as usize).map(PageText::edge_lines));
        assert!(
            spans.iter().all(|(start, end)| start <= end && *end <= text.len()),
            "{spans:?} of {}",
            text.len()
        );
        // What a copy and a snapshot's box do with them: within their room, whatever's left out.
        let mut copied = CopiedText::default();
        copied.add_page(page, text, &Range::pages(page, page), 4096, &spans);
        copied.finish();
        assert!(copied.text.len() <= 4096, "{}", copied.text.len());
        let (shown_width, shown_height) = text.size();
        let boxed = text.text_in_box(
            Rect {
                x: 0.0,
                y: 0.0,
                width: shown_width,
                height: shown_height,
            },
            &spans,
            4096,
        );
        assert!(boxed.text.len() <= 4096);
    }
}
