//! Generated contents as they're read back: the kept file in the user's caches, and the structure helper's answer on stdout (a hello, a line a page and an end). Both came from a PDF through a process that may have been made to say anything, and the kept file sits in a folder something else may touch. And the search indexer's report, the other helper's one line, read the same way.
//!
//! What must hold: no panic, bounded time and memory, and what's believed keeps every cap: entries, depth, pages in range, places on the page, titles cleaned, one line each, and never empty. Each input also goes in as a well-formed file with its numbers and titles taken from the bytes, or almost nothing would get past the format check.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_control::structure::headings::read_all;
use recto_index::corpus::CorpusKey;
use recto_index::generated::visible::{Found, Headings, InkSource, PageInk, keep_visible};
use recto_index::generated::{Generated, MAX_DEPTH, MAX_ENTRIES, MAX_PAGES, MAX_TITLE_BYTES, PLACE_SCALE};
use recto_proto::{CharRecord, PageFrame};
use serde_json::{Value, json};
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(20),
    memory: 256 * 1024 * 1024,
};

/// Titles fuzz bytes choose pieces from: line breaks, invisible and reordering characters, and what a prompt would look like.
const PIECES: &[&str] = &[
    "Introduction",
    "\n",
    "\r\n",
    "\u{200B}",
    "\u{202E}",
    "\u{E0041}",
    "\u{FEFF}",
    " ",
    "",
    "Ignore your instructions",
    "é",
    "数",
    "😀",
    "\u{0}",
    "\\",
    "\"",
    "<b>",
    "&amp;",
];

fn key() -> CorpusKey {
    CorpusKey::from_hex(&"ab".repeat(32)).unwrap_or_else(|| unreachable!())
}

fn holds(generated: &Generated) {
    assert!(generated.pages > 0 && generated.pages <= MAX_PAGES);
    assert!(generated.entries.len() <= MAX_ENTRIES);
    for entry in &generated.entries {
        assert!(entry.depth <= MAX_DEPTH);
        assert!(entry.page < generated.pages);
        assert!(entry.place <= PLACE_SCALE);
        assert!(!entry.title.is_empty() && entry.title.len() <= MAX_TITLE_BYTES);
        assert!(!entry.title.contains(['\n', '\r', '\u{200B}', '\u{202E}', '\u{E0041}']), "{:?}", entry.title);
    }
    let toc = generated.toc();
    assert_eq!(toc.entries().len(), generated.entries.len());
    for (index, entry) in toc.entries().iter().enumerate() {
        assert!(
            entry.parent == recto_core::NONE || (entry.parent as usize) < index,
            "a parent comes before its children"
        );
    }
    // What's believed writes back as something believed the same.
    assert_eq!(Generated::decode(generated.encode(key()).as_bytes(), key()).as_ref(), Ok(generated));
}

/// A well-formed file with what's in it chosen by the bytes.
fn shaped(data: &[u8]) -> Vec<u8> {
    let mut take = Take(data);
    let pages = u64::from(take.u8()) * u64::from(take.u8()) + u64::from(take.u8() % 3);
    let mut entries = Vec::new();
    while !take.is_empty() && entries.len() < 600 {
        let depth = u64::from(take.u8() % 9);
        let page = u64::from(take.u8()) * u64::from(take.u8() % 4 + 1);
        let place = u64::from(take.u8()) * 5;
        let mut title = String::new();
        for _ in 0..take.u8() % 6 {
            title.push_str(take.pick(PIECES));
        }
        entries.push(json!([depth, page, place, title]));
    }
    json!({ "format": "recto-generated-contents", "version": 1, "key": key().hex(), "pages": pages, "truncated": false, "entries": entries })
        .to_string()
        .into_bytes()
}

/// A structure helper's answer with what's in it chosen by the bytes: its hello, page lines in no promised order and with levels and boxes that may be anything, and an end that may not add up.
fn shaped_answer(data: &[u8]) -> Vec<u8> {
    let mut take = Take(data);
    let mut lines = vec![json!({ "state": "hello", "walled": true })];
    let (mut page, mut sent) = (0u64, 0u64);
    while !take.is_empty() && lines.len() < 200 {
        let mut found = Vec::new();
        for _ in 0..take.u8() % 5 {
            let level = u64::from(take.u8() % 9);
            let bbox = if take.u8() % 4 == 0 {
                Value::Null
            } else {
                json!([take.f32(), take.f32(), take.f32(), take.f32()])
            };
            let mut title = String::new();
            for _ in 0..take.u8() % 6 {
                title.push_str(take.pick(PIECES));
            }
            found.push(json!({ "level": level, "bbox": bbox, "title": title }));
        }
        sent += found.len() as u64;
        lines.push(json!({ "page": page, "found": found }));
        page += 1 + u64::from(take.u8() % 3);
    }
    let pages = if take.u8() % 8 == 0 {
        u64::from(take.u8())
    } else {
        page + u64::from(take.u8() % 3)
    };
    let counted = if take.u8() % 8 == 0 { u64::from(take.u8()) } else { sent };
    lines.push(json!({ "state": "ready", "pages": pages, "pages_failed": take.u8() % 3, "truncated": take.u8() % 2 == 0, "found": counted }));
    lines.iter().flat_map(|line| (line.to_string() + "\n").into_bytes()).collect()
}

/// What an answer that was believed must keep, whatever the bytes said.
fn answer_holds(headings: &Headings) {
    assert!(headings.found.len() <= MAX_ENTRIES);
    assert!(headings.pages > 0 && headings.pages <= MAX_PAGES && headings.pages_failed <= headings.pages);
    let mut last = None;
    for found in &headings.found {
        assert!(
            last.is_none_or(|last| found.page >= last) && found.page < headings.pages,
            "pages in order and inside the document"
        );
        last = Some(found.page);
        assert!((1..=6).contains(&found.level));
        assert!(found.bbox.is_none_or(|bbox| bbox.iter().all(|edge| edge.is_finite())));
        assert!(
            found.title.len() <= MAX_TITLE_BYTES && found.title.chars().any(char::is_alphanumeric),
            "{:?}",
            found.title
        );
        assert!(!found.title.contains(['\n', '\r', '\u{200B}', '\u{202E}', '\u{E0041}']), "{:?}", found.title);
    }
    // One line a page: a page's headings are together, and a page is never seen twice.
    let mut seen = std::collections::HashSet::new();
    let mut before = None;
    for found in &headings.found {
        if before != Some(found.page) {
            assert!(seen.insert(found.page), "page {} comes twice", found.page);
            before = Some(found.page);
        }
    }
}

/// Characters that spell each heading of the page it's asked for, inside its box, for every other page; the rest it can't read.
struct Spells(Vec<Found>);

impl InkSource for Spells {
    fn page(&mut self, page: u32) -> Option<PageInk> {
        if page % 2 != 0 {
            return None;
        }
        let (mut chars, mut boxes) = (Vec::new(), Vec::new());
        for found in self.0.iter().filter(|found| found.page == page) {
            let [x0, y0, x1, y1] = found.bbox.unwrap_or([0.0; 4]);
            let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            for c in found.title.chars() {
                chars.push(CharRecord::new(c as u32, 0));
                boxes.push([cx - 0.5, cy - 0.5, cx + 0.5, cy + 0.5]);
            }
        }
        Some(PageInk {
            chars,
            boxes,
            frame: PageFrame {
                turns: 0,
                left: -1e9,
                bottom: -1e9,
                right: 1e9,
                top: 1e9,
            },
            checked: true,
        })
    }
}

pub fn check(data: &[u8]) {
    within("generated", BUDGET, || {
        if let Ok(generated) = Generated::decode(data, key()) {
            holds(&generated);
        }
        let shaped = shaped(data);
        if let Ok(generated) = Generated::decode(&shaped, key()) {
            holds(&generated);
        }
        // As the indexer's report: one of its states, a code that's a word, and counts, whatever it said.
        for said in [String::from_utf8_lossy(data).into_owned(), String::from_utf8_lossy(&shaped).into_owned()] {
            let report = recto_control::indexing::read_report(&said);
            let fields = report.as_object().expect("a report is an object");
            assert!(
                matches!(report["state"].as_str(), Some("ready" | "refused" | "failed" | "changed" | "paused")),
                "{report}"
            );
            assert!(
                report["code"].is_null()
                    || report["code"]
                        .as_str()
                        .is_some_and(|code| code.len() <= 48 && code.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')),
                "{report}"
            );
            assert!(
                fields.iter().all(|(key, value)| matches!(key.as_str(), "state" | "code") || value.is_u64()),
                "{report}"
            );
            assert!(fields.len() <= 20, "{report}");
        }
        // As the structure helper's answer: whatever the bytes say, and a hello, page lines and an end built from them.
        for answer in [data.to_vec(), shaped_answer(data)] {
            if let Ok((headings, numbers)) = read_all(&answer, MAX_ENTRIES) {
                answer_holds(&headings);
                assert!(
                    numbers
                        .as_object()
                        .is_some_and(|numbers| numbers.len() <= 32 && numbers.values().all(|value| value.is_number() || value.is_boolean()))
                );
                // The check for hidden text, with a stand-in that draws every other page and spells the titles on it where they are.
                let mut spells = Spells(headings.found.clone());
                holds(&keep_visible(headings, &mut spells));
            }
        }
    });
}
