//! **A refusal's code is a named [`Code`], one table of them, and nothing decides by a code's string** (step R2.5). `Code` is a word with a private field
//! rather than an enum (there are more codes than an enum's budget of variants), so what an enum would give is held here: the table is whole, every code a
//! refusal can carry is in it, and every site that routes on a refusal compares named codes, never strings or a message's words.
//!
//! Failures caught: a refusal goes out with a code no table lists, so nothing documents it or says its next step; two entries share a word, so routing on
//! one catches the other; a crate below names a new code that never makes the table; the window decides by comparing a code's string, or a message's words,
//! which a reworded message or a mistyped code silently breaks.

use recto_agent::code::Next;
use recto_agent::{Code, exit};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crates folder").to_path_buf()
}

/// Every `.rs` file under a crate's `src`, with its path.
fn sources(krate: &str) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut folders = vec![crates().join(krate).join("src")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).expect("a source folder").flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|ending| ending == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file");
                out.push((path, text));
            }
        }
    }
    out
}

/// The words in double quotes that look like codes.
fn quoted_words(text: &str) -> Vec<&str> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .filter(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_'))
        .collect()
}

/// Failure: an answer the goldens hold carries a code the table doesn't list.
#[test]
fn every_code_in_the_goldens_is_listed() {
    let mut seen = BTreeSet::new();
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden"))
        .expect("the goldens")
        .flatten()
    {
        let text = std::fs::read_to_string(entry.path()).expect("a golden");
        for at in text.match_indices("\"code\":\"").map(|(at, found)| at + found.len()) {
            let word = &text[at..at + text[at..].find('"').expect("a closing quote")];
            seen.insert(word.to_string());
        }
    }
    assert!(seen.len() > 10, "the goldens' codes were read: {seen:?}");
    let missing: Vec<&String> = seen.iter().filter(|word| Code::listed(word).is_none()).collect();
    assert!(missing.is_empty(), "codes in the goldens that no entry lists: {missing:?}");
}

/// Failure: two entries share a word, so routing on one catches the other; or an entry's next step is empty; or `ALL` misses a line of the table.
#[test]
fn the_table_is_whole_and_every_entry_says_its_next_step() {
    let words: BTreeSet<&str> = Code::ALL.iter().map(|code| code.as_str()).collect();
    assert_eq!(words.len(), Code::ALL.len(), "two entries share a word");
    for code in Code::ALL {
        if let Next::Says(words) = code.next() {
            assert!(!words.trim().is_empty(), "{code} says an empty next step");
        }
        for exit_code in [exit::FAILED, exit::USAGE, exit::NOT_FOUND, exit::PERMISSION, exit::BUSY] {
            assert!(!code.next_step(exit_code).is_empty(), "{code} at exit {exit_code}");
        }
    }
    // The table's lines, read from its source: each is in `ALL`.
    let table = std::fs::read_to_string(crates().join("recto-agent/src/code.rs")).expect("the table");
    let rows = table.split("codes! {\n").nth(1).expect("the table's rows");
    let listed: Vec<&str> = rows.lines().filter_map(|line| line.split('"').nth(1)).collect();
    assert_eq!(listed.len(), Code::ALL.len(), "a line of the table isn't in ALL");
    assert!(listed.iter().all(|word| words.contains(word)));
}

/// The code words a crate below names in what a refusal carries: in a `code()`'s body, a `code:` field, or the constructors that take one.
fn named_below() -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    let code_fns = [
        "recto-core/src/save_error.rs",
        "recto-core/src/lib.rs",
        "recto-session/src/lib.rs",
        "recto-session/src/edit.rs",
        "recto-session/src/compare.rs",
        "recto-session/src/record.rs",
        "recto-update/src/handover.rs",
        "recto-render-service/src/host.rs",
    ];
    for file in code_fns {
        let text = std::fs::read_to_string(crates().join(file)).expect("a source file");
        for (at, _) in text.match_indices("pub fn code(") {
            let body = &text[at..];
            let end = body.find("\n    }\n").expect("the function's end");
            named.extend(quoted_words(&body[..end]).into_iter().map(str::to_string));
        }
    }
    for krate in ["recto-update", "recto-session", "recto-control"] {
        for (path, text) in sources(krate) {
            if path.components().any(|part| part.as_os_str() == "tests") {
                continue;
            }
            for line in text.lines() {
                for marker in ["StageError::now(\"", "StageError::lasting(\"", "StoreError::new(\"", "code: \""] {
                    for (at, found) in line.match_indices(marker) {
                        let rest = &line[at + found.len()..];
                        if let Some(end) = rest.find('"') {
                            named.insert(rest[..end].to_string());
                        }
                    }
                }
            }
        }
    }
    // The updater's refusals, each `Self::… => ("code", …)`.
    let said = std::fs::read_to_string(crates().join("recto-update/src/updater/said.rs")).expect("the updater's words");
    for (at, found) in said.match_indices("=> (") {
        if let Some(word) = said[at + found.len()..].trim_start().strip_prefix('"').and_then(|rest| rest.split('"').next()) {
            named.insert(word.to_string());
        }
    }
    named.retain(|word| word != "code" && word.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
    named
}

/// Failure: a crate below names a new code that a refusal carries out, and the table never lists it, so it has no next step of its own and nothing
/// routes on it by name.
#[test]
fn every_code_a_crate_below_names_is_listed() {
    let named = named_below();
    assert!(named.len() > 80, "the crates below were read: {} codes", named.len());
    let missing: Vec<&String> = named.iter().filter(|word| Code::listed(word).is_none()).collect();
    assert!(missing.is_empty(), "codes named below that no entry lists: {missing:?}");
}

/// A source's lines outside test items: an item under `#[cfg(test)]` is left out to its end, and everything after it is read. Braces are counted as written,
/// which holds for this code base's sources. (As `recto-session`'s scan for notice codes reads them.)
fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut skipping: Option<(bool, i64)> = None;
    for (at, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if skipping.is_none() {
            if let Some(rest) = trimmed
                .strip_prefix("#[cfg(test)]")
                .or_else(|| trimmed.starts_with("#[cfg(all(test").then_some(""))
            {
                skipping = Some((false, 0));
                if rest.trim().is_empty() {
                    continue;
                }
            } else {
                lines.push((at, line));
                continue;
            }
        }
        let Some((opened, depth)) = skipping.as_mut() else { continue };
        if !*opened && trimmed.starts_with("#[") {
            continue;
        }
        for c in line.chars() {
            match c {
                '{' => {
                    *opened = true;
                    *depth += 1;
                }
                '}' => *depth -= 1,
                _ => {}
            }
        }
        if (*opened && *depth <= 0) || (!*opened && line.trim_end().ends_with(';')) {
            skipping = None;
        }
    }
    lines
}

/// Whether a line compares something with the quoted word `code`: `== "code"`, `"code" !=`, `Some("code")` beside one, a match arm or pattern, `matches!`,
/// `.eq(` or `contains(`. A code given out (`=> "code"`, `code: "code"`) isn't a comparison.
fn compares(line: &str, code: &str) -> bool {
    line.match_indices(code).any(|(at, _)| {
        let before = line[..at].trim_end();
        let after = line[at + code.len()..].trim_start();
        let before = before.strip_suffix("Some(").map_or(before, str::trim_end);
        let after = after.strip_prefix(')').map_or(after, str::trim_start);
        before.ends_with("==")
            || before.ends_with("!=")
            || before.ends_with("eq(")
            || before.ends_with("contains(")
            || before.ends_with('|')
            || line[..at].contains("matches!(")
            || ["==", "!=", "=>", "|"].iter().any(|op| after.starts_with(op))
    })
}

/// Where a refusal's code is compared with another word than a code's: a helper's own reasons (an index's, generated contents', the renderer's), which
/// are its words for why, and turn into a refusal's message, never its code. Each is a function that maps those reasons to words.
const REASONS_TO_WORDS: &[(&str, &str)] = &[
    ("recto-agent/src/index.rs", "fn refused_words("),
    ("recto-agent/src/generated.rs", "fn reason_words("),
    ("recto-agent/src/generated.rs", "fn failure_words("),
];

/// Lines that compare a word that happens to be a code's but isn't a refusal's code: a field's value or a helper's own vocabulary, each with why.
const NOT_A_CODE: &[(&str, &str)] = &[
    // A tab's structure, `readable` or `renderer_only`, in an answer.
    ("recto-agent/src/human.rs", "tab[\"structure\"] == \"renderer_only\""),
    ("recto-agent/src/commands/reading.rs", "data[\"structure\"] == \"renderer_only\""),
    // A level's name.
    ("recto-agent/src/lib.rs", "\"read_only\" | \"readonly\" => Some(Self::ReadOnly)"),
    // The index's and generated contents' helpers say why in their own words, which turn into a message and an exit, never a refusal's code.
    ("recto-agent/src/index.rs", "if code == \"needs_password\" { exit::PROTECTED }"),
    ("recto-agent/src/generated.rs", "if code == \"needs_password\" { exit::PROTECTED }"),
    // Their states, as their answers say them.
    (
        "recto-control/src/indexing.rs",
        "Some(state @ (\"ready\" | \"refused\" | \"failed\" | \"changed\" | \"paused\"))",
    ),
    ("recto-control/src/generating.rs", "Some(\"changed\") => Outcome::Changed"),
];

/// Failure (review of R2.3): the window routes a refusal by comparing its code's string (`error.code == "unsaved_changes"`), or its message's words
/// (`error.message == too_long.message`), which a reworded message or a mistyped code breaks with nothing failing. No production line outside the table's
/// own module compares a listed code's word or a refusal's message.
#[test]
fn nothing_decides_by_a_codes_string_or_a_messages_words() {
    let words: Vec<String> = Code::ALL.iter().map(|code| format!("\"{}\"", code.as_str())).collect();
    let mut found = Vec::new();
    let mut read = 0;
    for krate in ["recto-agent", "recto-app", "recto-control", "recto-cli", "recto-ffi"] {
        for (path, text) in sources(krate) {
            let shown = path.strip_prefix(crates()).unwrap_or(&path).display().to_string();
            if shown.ends_with("recto-agent/src/code.rs")
                || path
                    .components()
                    .any(|part| matches!(part.as_os_str().to_str(), Some("tests" | "headless" | "testkit")))
                || shown.ends_with("tests.rs")
            {
                continue;
            }
            read += 1;
            // The lines of a function that maps a helper's reasons to words, by their place: from its `fn` to its end.
            let exempt: Vec<(usize, usize)> = REASONS_TO_WORDS
                .iter()
                .filter(|(file, _)| shown.ends_with(file))
                .flat_map(|(_, marker)| {
                    text.match_indices(marker).filter_map(|(at, _)| {
                        let first = text[..at].lines().count();
                        let length = text[at..].find("\n}\n").map(|end| text[at..at + end].lines().count())?;
                        Some((first, first + length))
                    })
                })
                .collect();
            for (at, line) in production_lines(&text) {
                let line_number = at + 1;
                if exempt.iter().any(|(from, to)| (*from..=*to).contains(&line_number))
                    || NOT_A_CODE.iter().any(|(file, words)| shown.ends_with(file) && line.contains(words))
                {
                    continue;
                }
                let by_message = [".message ==", ".message !=", "== error.message", ".message.contains(", ".message.starts_with("]
                    .iter()
                    .any(|pattern| line.contains(pattern));
                if by_message || words.iter().any(|word| compares(line, word)) {
                    found.push(format!("{shown}:{line_number}: {}", line.trim()));
                }
            }
        }
    }
    assert!(read > 150, "the crates' sources were read: {read} files");
    assert!(found.is_empty(), "decided by a code's string or a message's words: {found:#?}");
}
