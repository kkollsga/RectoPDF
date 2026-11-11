//! Generated contents as they're read back: the kept file in the user's caches, and the helper's two lines on stdout. Both came from a PDF through a process that may have been made to say anything, and the kept file sits in a folder something else may touch. And the search indexer's report, the other helper's one line, read the same way.
//!
//! What must hold: no panic, bounded time and memory, and what's believed keeps every cap: entries, depth, pages in range, places on the page, titles cleaned, one line each, and never empty. Each input also goes in as a well-formed file with its numbers and titles taken from the bytes, or almost nothing would get past the format check.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_control::generating::{Outcome, read_answer};
use recto_index::corpus::CorpusKey;
use recto_index::generated::{Generated, MAX_DEPTH, MAX_ENTRIES, MAX_PAGES, MAX_TITLE_BYTES, PLACE_SCALE};
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
        // As the helper's answer: a report line, then the contents.
        for answer in [data.to_vec(), [&b"{\"state\":\"ready\"}\n"[..], &shaped].concat()] {
            let (outcome, numbers) = read_answer(&answer, key());
            if let Outcome::Ready(generated) = &outcome {
                holds(generated);
            }
            assert!(
                numbers
                    .as_object()
                    .is_some_and(|numbers| numbers.len() <= 32 && numbers.values().all(|value| value.is_number() || value.is_boolean()))
                    || numbers == Value::Null
            );
        }
    });
}
