//! The normaliser, line-order repair and hidden-text report, on hostile characters and boxes. They run on the host's side, on what a compromised worker sent, so a NaN, an infinity or a million lines on one baseline has to do no harm.
//!
//! What must hold: repair adds or loses nothing but whitespace, every offset stays in bounds, and it takes bounded time however the page is shaped.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_index::order::{GENERATED, repair_line_order};
use recto_index::{MAX_ALSO_SPELLED, MAX_HIDDEN_RUN_CHARS, MAX_HIDDEN_RUNS, MAX_HIDDEN_SPANS, hidden_report, normalize, read_page};
use recto_proto::{CharBox, CharRecord, MAX_PAGE_CHARS};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A page at the wire's cap is two million characters. Linear work on that is well under this, on a sanitizer build too, and quadratic work is hours.
const BUDGET: Budget = Budget {
    time: Duration::from_secs(60),
    memory: 512 * 1024 * 1024,
};

/// The longest page a check builds. A smoke run on a debug build holds this down, because a page at the wire's cap takes seconds there, and every other run wants the cap.
pub static LONGEST_PAGE: AtomicUsize = AtomicUsize::new(MAX_PAGE_CHARS as usize);

/// The longest page that isn't asking to be at the cap. About the busiest real page.
const MAX_ORDINARY_PAGE: usize = 32_768;

/// What fuzz bytes choose characters from: the ones each rule looks at, and the ones that have tripped text code before.
const ALPHABET: &[char] = &[
    'a',
    'b',
    'e',
    'i',
    'v',
    'x',
    'A',
    'I',
    'V',
    'Æ',
    'ß',
    'İ',
    'ı',
    '数',
    '1',
    '7',
    ' ',
    ' ',
    ' ',
    '\r',
    '\n',
    '\n',
    '\u{B}',
    '\u{C}',
    '\u{85}',
    '\u{2028}',
    '\u{2029}',
    '-',
    '-',
    '\u{2010}',
    '\u{2011}',
    '\u{2013}',
    '\u{2014}',
    '\u{FFFE}',
    '\u{FFFD}',
    '\u{AD}',
    '\u{A0}',
    '\u{200B}',
    '\u{202E}',
    '\u{E0041}',
    '\u{E000}',
    '\u{0}',
    '\t',
    '.',
    ',',
    ')',
    '😀',
    'ﬁ',
];

const STEPS: &[f32] = &[
    5.0,
    5.0,
    5.0,
    5.0,
    2.5,
    0.0,
    3.0,
    8.0,
    -5.0,
    -20.0,
    50.0,
    -500.0,
    0.25,
    1e30,
    -1e30,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::NAN,
];
const SIZES: &[f32] = &[7.0, 7.0, 7.0, 7.0, 7.0, 3.0, 12.0, 0.0, -7.0, 1e-30, 1e30, f32::MAX, f32::INFINITY, f32::NAN];
const LINE_STEPS: &[f32] = &[
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    -14.0,
    -14.0,
    14.0,
    2.0,
    -2.0,
    -700.0,
    1e30,
    f32::MAX,
    f32::INFINITY,
    f32::NAN,
];

/// A page out of the bytes. Most of it reads like text on lines, since that's where the rules are, with hostile numbers among it. A page repeats its pattern as far as the wire allows, because what's slow is usually what's many.
pub fn page(data: &[u8]) -> (Vec<CharRecord>, Vec<CharBox>) {
    let mut take = Take(data);
    let mode = take.u8();
    let raw = mode & 1 == 1;
    // A page at the wire's cap takes a second or so, so it's rare, and everything else stays short enough to run thousands of times.
    let at_the_cap = mode >= 0xFE;
    let repeat = if at_the_cap {
        MAX_PAGE_CHARS as usize
    } else {
        [1usize, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 8, 64, 512][usize::from(mode >> 4)]
    };
    let mut chars = Vec::new();
    let mut boxes = Vec::new();
    let (mut x, mut y) = (72.0f32, 700.0f32);
    while !take.is_empty() {
        let ch = take.pick(ALPHABET);
        let flags = take.u8();
        let flags = (u32::from(flags & 1) * CharRecord::HYPHEN)
            | (u32::from((flags >> 1) & 1) * CharRecord::GENERATED)
            | (u32::from((flags >> 2) & 1) * CharRecord::NO_INK);
        chars.push(CharRecord::new(ch as u32, flags));
        if raw {
            boxes.push([take.f32(), take.f32(), take.f32(), take.f32()]);
        } else {
            let (step, width, height, line) = (take.pick(STEPS), take.pick(SIZES), take.pick(SIZES), take.pick(LINE_STEPS));
            if line != 0.0 {
                x = 72.0;
            }
            y += line;
            boxes.push([x, y, x + width, y + height]);
            x += step;
        }
    }
    let longest = LONGEST_PAGE
        .load(Ordering::Relaxed)
        .min(if at_the_cap { usize::MAX } else { MAX_ORDINARY_PAGE });
    let wanted = chars.len().saturating_mul(repeat).min(longest);
    chars.truncate(longest);
    boxes.truncate(longest);
    let (chars_len, mut at) = (chars.len(), 0);
    if chars_len > 0 {
        while chars.len() < wanted {
            chars.push(chars[at % chars_len]);
            boxes.push(boxes[at % chars_len]);
            at += 1;
        }
    }
    (chars, boxes)
}

/// Catches: characters lost or invented by repair, an origin or span out of bounds, a cap that doesn't hold, and anything worse than linear on a hostile page.
pub fn check(data: &[u8]) {
    let (chars, boxes) = page(data);
    within("reading a hostile page", BUDGET, || {
        check_repair(&chars, &boxes);
        check_reading(&chars, &boxes, true);
        check_reading(&chars, &boxes, false);
        check_reading(&chars, &[], true);
        check_normalize(&chars);
        check_hidden(&chars);
    });
}

fn check_repair(chars: &[CharRecord], boxes: &[CharBox]) {
    // Boxes that don't match mean nothing can be trusted, and nothing's moved.
    if !chars.is_empty() {
        assert!(repair_line_order(chars, &boxes[..boxes.len() - 1]).is_none());
    }
    let Some(repaired) = repair_line_order(chars, boxes) else { return };
    assert!(repaired.runs_moved > 0, "`None` is what nothing moved looks like");
    assert_eq!(repaired.chars.len(), repaired.origin.len());
    let mut used = vec![false; chars.len()];
    for (record, &origin) in repaired.chars.iter().zip(&repaired.origin) {
        if origin == GENERATED {
            assert_eq!(record.ch(), ' ', "only a space is ever made up");
            continue;
        }
        let from = origin as usize;
        assert_eq!(*record, chars[from], "origin {from} isn't this character");
        assert!(!std::mem::replace(&mut used[from], true), "character {from} appears twice");
    }
    for (index, was_used) in used.iter().enumerate() {
        assert!(
            *was_used || chars[index].ch().is_whitespace(),
            "character {index}, {:?}, was lost",
            chars[index].ch()
        );
    }
}

fn check_reading(chars: &[CharRecord], boxes: &[CharBox], spans: bool) {
    let reading = read_page(chars, boxes, spans);
    check_normalized(&reading.normalized, chars.len(), false);
    let text_chars = reading.normalized.text.chars().count() as u32;
    assert!(reading.hidden_spans.len() <= MAX_HIDDEN_SPANS);
    assert!(spans || (reading.hidden_spans.is_empty() && !reading.hidden_flagged));
    let mut last_end = 0;
    for &(start, end) in &reading.hidden_spans {
        assert!(start < end && end <= text_chars, "span {start}..{end} of {text_chars}");
        assert!(start >= last_end, "spans overlap");
        last_end = end;
    }
    assert!(reading.hidden_spans_dropped == 0 || reading.hidden_spans.len() == MAX_HIDDEN_SPANS);
    // What's flagged has somewhere to show, unless all of it was taken out as invisible.
    assert!(reading.hidden_spans.is_empty() || reading.hidden_flagged);
}

fn check_normalized(normalized: &recto_index::Normalized, chars: usize, want_source: bool) {
    assert!(normalized.also_spelled.len() <= MAX_ALSO_SPELLED);
    assert!(normalized.source.iter().all(|&index| (index as usize) < chars.max(1)));
    if want_source {
        assert_eq!(
            normalized.source.len(),
            normalized.text.chars().count(),
            "a source for each character of the text"
        );
    }
    assert!(!normalized.text.contains('\r'), "lines end one way");
    assert!(
        !normalized.text.chars().any(recto_core::is_invisible_format),
        "something invisible survived: {:?}",
        normalized.text
    );
}

fn check_normalize(chars: &[CharRecord]) {
    let plain = normalize(chars, false);
    let mapped = normalize(chars, true);
    assert_eq!(plain.text, mapped.text, "asking for a map changes the text");
    check_normalized(&mapped, chars.len(), true);
}

fn check_hidden(chars: &[CharRecord]) {
    let report = hidden_report(chars, true);
    assert_eq!(report.chars as usize, chars.iter().filter(|c| c.has(CharRecord::NO_INK)).count());
    assert!(report.runs.len() <= MAX_HIDDEN_RUNS);
    assert!(report.runs.iter().all(|run| run.chars().count() <= MAX_HIDDEN_RUN_CHARS));
    assert!(!report.flagged || report.chars >= recto_index::MIN_FLAGGED_RUN as u32);
    assert!(hidden_report(chars, false).checked != report.checked);
}
