//! **What the page view's types let other code reach,** listed and held (R3.6). `Reader` and its parts, `Engine` and its facades: every method visible
//! past its own module (`pub` or `pub(crate)`) with its signature, every trait implemented (in any file of the crate: an `impl Reader` in `paper.rs` is listed too), and every field of every struct named here that is visible past its module.
//! A part that can be built, replaced or reached around from outside where it belongs shows up here as a changed line, so a widening is a deliberate edit
//! of [`SURFACE`] that a reviewer sees, not a side effect of a move (the lesson of `Session`'s R3.5: a public `Default` and a `_mut` accessor let outside
//! code replace a whole part, and nothing listed it).
//!
//! A text scan of the sources, as the ratchet's is: an item under `#[cfg(test)]` isn't listed, a signature is joined onto one line, an inline `mod` is read as if its items were at the top, and an `impl` header over several lines (a `where` line) is joined. It sees an `unsafe impl` (`Send`, `Sync`), `pub(in …)` as visible, a derive inside `cfg_attr` or over several lines, the files in any folder under `tiles/` and `engine/` (but `tests`), and passes over only a file whose own `#![cfg(test)]` line says it's for tests. A `pub(super)` field isn't listed: it reaches only the `tiles` module the parts live in.
//!
//! **What it doesn't see,** being a scan of lines and not the compiler: an `impl` inside a function body or a `const _: () = { … }` block (indented, so
//! read as neither an item nor a method), anything a macro generates, the fields of a struct whose `where` clause stands at column 0 before its brace
//! (the `where` line ends the item), the fields of a tuple struct written over several lines (only a one-line tuple struct's are read), and a `cfg` other
//! than `test` and the `qa` feature: an item under `#[cfg(feature = "qa")]` (or `any(test, feature = "qa")`) is listed marked `[qa feature only]`, and
//! any other `cfg` as though it were always there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The files of the page view's state and its engine, in `tiles/` and `engine/` and any folder under them but a `tests` one.
fn sources() -> Vec<PathBuf> {
    sources_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
}

/// [`sources`], in a crate's `src` at `root`.
fn sources_in(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.join("tiles.rs"), root.join("engine.rs")];
    for dir in ["tiles", "engine"] {
        walk(&root.join(dir), &mut files);
    }
    files.retain(|file| !file.components().any(|part| part.as_os_str() == "tests"));
    files.sort();
    files
}

/// Every `.rs` file under `dir`, in any folder.
fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Every `.rs` file of the crate outside the page view's own: where an `impl` of one of its types could hide.
fn other_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let own = sources();
    files.retain(|file| !own.contains(file) && !file.ends_with("surface.rs") && !file.components().any(|part| part.as_os_str() == "tests"));
    files.sort();
    files
}

/// Whether a file is built only for tests: its own `#![cfg(test)]` line, not one a comment mentions.
fn only_for_tests(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "#![cfg(test)]")
}

/// The names of the structs and enums a page view file defines, of any visibility and wherever they stand: the types an `impl` elsewhere is held against.
fn defined_types(text: &str, into: &mut BTreeSet<String>) {
    for line in text.lines() {
        if let Some((_, _, rest)) = type_item(line.trim()) {
            into.insert(strip_generics(rest.split([' ', '{', '(', ';']).next().unwrap_or("")));
        }
    }
}

/// Whether a line, trimmed, says its item is reached past its module: `pub`, `pub(crate)` or `pub(in …)`, as written.
fn visibility(line: &str) -> Option<&str> {
    if line.starts_with("pub(crate) ") {
        Some("pub(crate)")
    } else if line.starts_with("pub(in ") {
        line.find(')').map(|end| &line[..=end])
    } else if line.starts_with("pub ") {
        Some("pub")
    } else {
        None
    }
}

/// The traits a derive names, wherever it's written: `#[derive(A, B)]`, or inside `#[cfg_attr(…, derive(A, B))]`.
fn derived_in(attribute: &str) -> Vec<String> {
    let Some((_, rest)) = attribute.split_once("derive(") else {
        return Vec::new();
    };
    let list = rest.split(')').next().unwrap_or("");
    list.split(',').map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).collect()
}

/// The type an `impl` header is for, and the trait if it's a trait's: `impl<'a> Selection<'a> {` is `Selection`, `impl Default for Reader {` is `Reader: Default`.
fn impl_of(header: &str) -> String {
    let header = header.trim_end_matches(['{', '}']).trim();
    let header = header.strip_prefix("unsafe ").unwrap_or(header);
    let header = header.strip_prefix("impl").unwrap_or(header);
    let header = strip_generics(header.trim_start());
    let name = |text: &str| strip_generics(text.trim()).split("::").last().unwrap_or("").trim().to_string();
    match header.split_once(" for ") {
        Some((tr, ty)) => format!("{}: {}", name(ty), name(tr)),
        None => name(&header),
    }
}

/// Takes out every `<...>`, nested or not.
fn strip_generics(text: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// What a file lets past its module, as lines of the listing.
fn listing_of(text: &str, into: &mut BTreeSet<String>) {
    listing_in(text, into, None);
}

/// [`listing_of`], and with `only` the `impl` blocks for those types alone (a file outside the page view's own).
fn listing_in(text: &str, into: &mut BTreeSet<String>, only: Option<&BTreeSet<String>>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    // The `impl` or `struct` being read: its name, and whether the struct's fields are listed.
    let mut within: Option<String> = None;
    let mut fields_of: Option<String> = None;
    let mut test_item = false;
    // The next item is built only with the `qa` feature, and the `impl` being read is.
    let (mut qa_item, mut within_qa) = (false, false);
    // What a `#[derive(..)]` just above the next item gives it: `Default` or `Clone` on a part is a way to build or replace one.
    let mut derives: Vec<String> = Vec::new();
    while at < lines.len() {
        let line = lines[at];
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]") {
            test_item = true;
            at += 1;
            continue;
        }
        if trimmed.starts_with("#[cfg(") && trimmed.contains("feature = \"qa\"") && !trimmed.contains("not(") {
            qa_item = true;
            at += 1;
            continue;
        }
        if trimmed.starts_with("#[derive(") || (trimmed.starts_with("#[cfg_attr(") && trimmed.contains("derive(")) {
            // Over several lines, joined to its `)]`.
            let mut attribute = String::from(trimmed);
            while !attribute.ends_with(")]") && at + 1 < lines.len() {
                at += 1;
                attribute.push_str(lines[at].trim());
            }
            derives.extend(derived_in(&attribute));
            at += 1;
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("//") || trimmed.is_empty() {
            at += 1;
            continue;
        }
        // An item at the top of the file: a column-0 line.
        if !line.starts_with(' ') && !line.starts_with('}') {
            let skip = std::mem::take(&mut test_item);
            let derived = std::mem::take(&mut derives);
            let qa = if std::mem::take(&mut qa_item) { QA_ONLY } else { "" };
            within = None;
            within_qa = false;
            fields_of = None;
            if skip {
                // The item and everything in it, to its closing brace at column 0, or to its `;`.
                if !trimmed.ends_with(';') {
                    while at < lines.len() && !lines[at].starts_with('}') {
                        at += 1;
                    }
                }
                at += 1;
                continue;
            }
            if trimmed.starts_with("mod ")
                || trimmed.starts_with("pub mod ")
                || trimmed.starts_with("pub(crate) mod ")
                || trimmed.starts_with("pub(super) mod ")
            {
                // An inline module: its items are read as if they stood at the top (four spaces taken off), to its closing brace.
                if trimmed.ends_with('{') {
                    let from = at + 1;
                    while at + 1 < lines.len() && !lines[at + 1].starts_with('}') {
                        at += 1;
                    }
                    let inner = lines[from..(at + 1).max(from)]
                        .iter()
                        .map(|line| line.strip_prefix("    ").unwrap_or(line))
                        .collect::<Vec<_>>()
                        .join("\n");
                    listing_in(&inner, into, only);
                    at += 1;
                }
            } else if (trimmed.starts_with("impl") || trimmed.starts_with("unsafe impl")) && !trimmed.ends_with(';') {
                // The header may run over lines (a `where` clause): joined to its opening brace, or to its empty body (`impl Send for Docs {}`).
                let mut header = String::from(trimmed);
                while !header.ends_with('{') && !header.ends_with("{}") && at + 1 < lines.len() {
                    at += 1;
                    header.push(' ');
                    header.push_str(lines[at].trim());
                }
                let header = header.split(" where ").next().unwrap_or("").trim_end_matches(['{', '}']).trim().to_string();
                let of = impl_of(&header);
                let named = of.split(':').next().unwrap_or("");
                if only.is_none_or(|types| types.contains(named)) {
                    if of.contains(": ") {
                        into.insert(format!("{of} (trait impl){qa}"));
                    }
                    within = Some(of);
                    within_qa = !qa.is_empty();
                }
            } else if only.is_some() {
                // Outside the page view's own files only an `impl` of its types is listed.
            } else if let Some((shown, kind, rest)) = type_item(trimmed) {
                let name = strip_generics(rest.split([' ', '{', '(', ';']).next().unwrap_or(""));
                if kind == "struct" {
                    fields_of = Some(name.clone());
                    if let Some(tuple) = rest.split_once('(').filter(|(_, fields)| fields.contains("pub")) {
                        into.insert(format!("{name}.({}", one_line(tuple.1)));
                    }
                }
                let derived = if derived.is_empty() {
                    String::new()
                } else {
                    format!(", derives {}", derived.join(", "))
                };
                into.insert(format!("{name} ({shown} {kind}{derived}){qa}"));
            } else if let Some(shown) = visibility(trimmed)
                && trimmed.contains(" fn ")
            {
                let mut signature = String::from(trimmed);
                while !ends_signature(&signature) && at + 1 < lines.len() {
                    at += 1;
                    signature.push(' ');
                    signature.push_str(lines[at].trim());
                }
                into.insert(format!("fn ({shown}) {}{qa}", one_line(&signature)));
            }
            at += 1;
            continue;
        }
        // A method, four spaces in, of the `impl` being read.
        if let Some(of) = &within
            && line.starts_with("    ")
            && !line.starts_with("     ")
        {
            let skip = std::mem::take(&mut test_item);
            let qa = if std::mem::take(&mut qa_item) || within_qa { QA_ONLY } else { "" };
            let shown = visibility(trimmed);
            if trimmed.contains("fn ") && (shown.is_some() || of.contains(": ")) && !skip {
                let mut signature = String::from(trimmed);
                while !ends_signature(&signature) && at + 1 < lines.len() {
                    at += 1;
                    signature.push(' ');
                    signature.push_str(lines[at].trim());
                }
                into.insert(format!("{of}::{}{qa}", one_line(&signature)));
            }
            at += 1;
            continue;
        }
        // A field of a struct named here, visible past its module.
        if let Some(of) = &fields_of
            && line.starts_with("    ")
            && !line.starts_with("     ")
            && visibility(trimmed).is_some()
            && trimmed.contains(':')
        {
            into.insert(format!("{of}.{}", trimmed.trim_end_matches(',')));
        }
        test_item = false;
        qa_item = false;
        at += 1;
    }
}

/// What a line of the listing ends with when only the `qa` feature (or a test build) has the item: a release app doesn't.
const QA_ONLY: &str = " [qa feature only]";

/// A struct or an enum, of any visibility: how visible, which, and the rest of its line.
fn type_item(line: &str) -> Option<(&str, &'static str, &str)> {
    if let Some(shown) = visibility(line).filter(|shown| shown.starts_with("pub(in "))
        && let Some(rest) = line[shown.len()..]
            .trim_start()
            .strip_prefix("struct ")
            .map(|rest| (rest, "struct"))
            .or_else(|| line[shown.len()..].trim_start().strip_prefix("enum ").map(|rest| (rest, "enum")))
    {
        return Some((shown, rest.1, rest.0));
    }
    const SHOWN: [(&str, &str); 4] = [("pub(crate) ", "pub(crate)"), ("pub(super) ", "pub(super)"), ("pub ", "pub"), ("", "private")];
    for (prefix, shown) in SHOWN {
        for (keyword, kind) in [("struct ", "struct"), ("enum ", "enum")] {
            if let Some(rest) = line.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(keyword)) {
                return Some((shown, kind, rest));
            }
        }
    }
    None
}

/// Whether a joined signature is whole: it reached its body's opening brace, its `;`, or a body written on the same line (`fn x(&self) {}`).
fn ends_signature(signature: &str) -> bool {
    signature.ends_with('{') || signature.ends_with(';') || signature.ends_with('}')
}

/// A signature as one line: its body gone (an opening brace, or a whole body on the line), its whitespace one space.
fn one_line(signature: &str) -> String {
    let signature = signature.split(" {").next().unwrap_or(signature);
    let signature = signature.trim_end_matches('{').trim_end_matches(';').trim();
    signature
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
}

fn listing() -> String {
    let mut lines = BTreeSet::new();
    let read = |file: &PathBuf| std::fs::read_to_string(file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
    let mut types = BTreeSet::new();
    for file in sources() {
        let text = read(&file);
        if only_for_tests(&text) {
            continue;
        }
        defined_types(&text, &mut types);
        listing_of(&text, &mut lines);
    }
    for file in other_sources() {
        let text = read(&file);
        if only_for_tests(&text) {
            continue;
        }
        listing_in(&text, &mut lines, Some(&types));
    }
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

/// Failure: a move or an addition widens what other code can reach of the page view: a part's method made `pub(crate)`, a part given a trait other code
/// could build or replace it by (`Default`, `Clone`), a field of `Reader` or `Engine` made visible, a facade's method that takes a lock another way, or
/// one taken away that a caller needs. The list is updated by hand, in the commit that means it.
#[test]
fn what_the_page_view_lets_other_code_reach_is_listed() {
    let now = listing();
    if now != SURFACE.trim() {
        let was: BTreeSet<&str> = SURFACE.trim().lines().collect();
        let is: BTreeSet<&str> = now.lines().collect();
        let gone: Vec<&&str> = was.difference(&is).collect();
        let new: Vec<&&str> = is.difference(&was).collect();
        panic!("what the page view lets other code reach changed.\ngone: {gone:#?}\nnew: {new:#?}\nthe whole list now:\n{now}\n");
    }
}

/// Holds the scan itself: an item under `#[cfg(test)]` isn't listed, a signature across lines is joined, and a private method isn't listed. Failure it
/// catches: a part's visible field, an item inside an inline `mod`, or an `impl` whose header ends on a `where` line going unlisted (the three ways the listing
/// was blind before R3.6a).
#[test]
fn the_listing_reads_what_rust_says() {
    let mut lines = BTreeSet::new();
    listing_of(
        "pub struct Reader {\n    a: u32,\n    pub(crate) part: Part,\n}\n\nimpl Default for Part {\n    fn default() -> Self {\n        todo!()\n    }\n}\n\nimpl<'a> Facade<'a> {\n    pub fn press(\n        &self,\n        x: f64,\n    ) -> Vec<Op> {\n        todo!()\n    }\n\n    fn helper(&self) {}\n\n    #[cfg(test)]\n    pub fn only_in_tests(&self) {}\n}\n\n#[cfg(test)]\nimpl Part {\n    pub fn hidden(&self) {}\n}\n",
        &mut lines,
    );
    let got: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_eq!(
        got,
        [
            "Facade::pub fn press(&self, x: f64) -> Vec<Op>",
            "Part: Default (trait impl)",
            "Part: Default::fn default() -> Self",
            "Reader (pub struct)",
            "Reader.pub(crate) part: Part",
        ]
    );
}

/// Failure it catches: the listing not seeing a part's field widened (`Docs.views` from `pub(super)` to `pub(crate)`), a trait impl on a part written under
/// an inline `mod` or with its header on a `where` line, or an `impl` of a page view type in a file the listing doesn't read (the second pass, with `only`).
/// And (review round 1 of the page view follow-ups): an `unsafe impl` (`Send` or `Sync` for a part, which lets it cross threads), a method or field made
/// `pub(in …)`, or a derive written inside `cfg_attr` or over several lines, each of which the listing passed over; and a one-line `impl … {}` swallowing
/// the items after it.
#[test]
fn the_listing_sees_a_part_widened_in_every_place_it_can_be() {
    let mut lines = BTreeSet::new();
    listing_of(
        "pub(crate) struct Docs {\n    pub(super) kept: u32,\n    pub(crate) views: Vec<u8>,\n}\n\nmod inner {\n    impl Default for Docs {\n        fn default() -> Self {\n            todo!()\n        }\n    }\n}\n\nimpl<T> Clone for Docs\nwhere\n    T: Copy,\n{\n    fn clone(&self) -> Self {\n        todo!()\n    }\n}\n\nunsafe impl Send for Docs {}\n\n#[cfg_attr(feature = \"qa\", derive(Default))]\npub(crate) struct Work {\n    pub(in crate::tiles) jobs: u32,\n}\n\n#[derive(\n    Clone,\n    Debug,\n)]\npub(super) struct Pointer;\n\nimpl Work {\n    pub(in crate::tiles) fn widened(&self) {}\n}\n",
        &mut lines,
    );
    let got: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_eq!(
        got,
        [
            "Docs (pub(crate) struct)",
            "Docs.pub(crate) views: Vec<u8>",
            "Docs: Clone (trait impl)",
            "Docs: Clone::fn clone(&self) -> Self",
            "Docs: Default (trait impl)",
            "Docs: Default::fn default() -> Self",
            "Docs: Send (trait impl)",
            "Pointer (pub(super) struct, derives Clone, Debug)",
            "Work (pub(crate) struct, derives Default)",
            "Work.pub(in crate::tiles) jobs: u32",
            "Work::pub(in crate::tiles) fn widened(&self)",
        ]
    );
    let mut outside = BTreeSet::new();
    let types = BTreeSet::from(["Reader".to_string()]);
    listing_in(
        "impl Reader {\n    pub(crate) fn sneak(&mut self) {}\n}\n\nimpl Other {\n    pub(crate) fn fine(&self) {}\n}\n",
        &mut outside,
        Some(&types),
    );
    let got: Vec<&str> = outside.iter().map(String::as_str).collect();
    assert_eq!(got, ["Reader::pub(crate) fn sneak(&mut self)"]);
}

/// Failure it catches (review round 2 of the page view follow-ups): an item only the `qa` feature builds (`Engine::qa` and the seams it hands out) listed
/// as though a release app had it, so its losing the `cfg`, which puts the seams in the shipping app, changed nothing in the listing. Marked, on a type,
/// a free function, a method, and every method of an `impl` under the `cfg`; an item only a build without the feature has isn't marked as the feature's.
#[test]
fn the_listing_marks_what_only_the_qa_feature_builds() {
    let mut lines = BTreeSet::new();
    listing_of(
        "#[cfg(any(test, feature = \"qa\"))]\n#[derive(Clone, Copy)]\npub struct Seams;\n\n#[cfg(feature = \"qa\")]\nimpl Seams {\n    pub fn hold(&self) {}\n}\n\nimpl Engine {\n    #[cfg(any(test, feature = \"qa\"))]\n    #[inline]\n    pub fn qa(&self) -> Seams {\n        todo!()\n    }\n\n    pub fn shipped(&self) {}\n}\n\n#[cfg(not(feature = \"qa\"))]\npub fn without() {}\n",
        &mut lines,
    );
    let got: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_eq!(
        got,
        [
            "Engine::pub fn qa(&self) -> Seams [qa feature only]",
            "Engine::pub fn shipped(&self)",
            "Seams (pub struct, derives Clone, Copy) [qa feature only]",
            "Seams::pub fn hold(&self) [qa feature only]",
            "fn (pub) pub fn without()",
        ]
    );
}

/// Failure it catches (review round 1 of the page view follow-ups): a file of the page view in a folder under `tiles/` or `engine/` going unread (only their
/// own files were), and a file passed over as built only for tests because a comment of it mentions `#![cfg(test)]`.
#[test]
fn the_listing_reads_every_file_it_should() {
    assert!(only_for_tests("//! A test file.\n#![cfg(test)]\n\nuse super::*;\n"));
    assert!(!only_for_tests("//! Unlike `tests.rs`, this isn't `#![cfg(test)]`.\npub fn shipped() {}\n"));
    let root = std::env::temp_dir().join(format!("recto-surface-walk-{}", std::process::id()));
    for dir in ["tiles/part", "engine", "tiles/tests"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in ["tiles.rs", "engine.rs", "tiles/part/hidden.rs", "engine/run.rs", "tiles/tests/mod.rs"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let found = sources_in(&root);
    std::fs::remove_dir_all(&root).unwrap();
    let found: Vec<_> = found.iter().map(|file| file.strip_prefix(&root).unwrap().to_path_buf()).collect();
    let want: Vec<PathBuf> = ["engine/run.rs", "engine.rs", "tiles/part/hidden.rs", "tiles.rs"]
        .iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(found, want, "a file in a folder under tiles/ is read, and a tests folder isn't");
}

/// The lines of `text` that build a `Reader` or put another part in one of its fields: `Reader::new()`, or `Self::new()` inside an `impl` of `Reader` (a
/// `Default` written for it, say); a part assigned (`reader.docs = …`); or swapped, replaced or taken through `std::mem`, as `mem::swap(&mut a.docs, …)`
/// or, in a file that has a `use std::mem::…`, as a bare `swap(`, `replace(` or `take(`. `parts` are the names of `Reader`'s visible fields.
fn builds_or_replaces_a_part<'a>(text: &'a str, parts: &[&str]) -> Vec<&'a str> {
    let mem_by_name = text.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("use std::mem::") || line.starts_with("use core::mem::")
    });
    // Whether the item being read, from its column-0 line, is an `impl` of `Reader`.
    let mut in_reader = false;
    let mut found = Vec::new();
    for line in text.lines() {
        let code = line.split("//").next().unwrap_or("");
        let trimmed = code.trim();
        if !line.starts_with([' ', '\t', '}', '#']) && !trimmed.is_empty() {
            let header = trimmed.split(" where ").next().unwrap_or("");
            in_reader = (header.starts_with("impl") || header.starts_with("unsafe impl")) && impl_of(header).split(':').next() == Some("Reader");
        }
        let through_mem = code.contains("mem::") || (mem_by_name && calls_by_name(code, &["swap(", "replace(", "take("]));
        let replaces = parts.iter().any(|part| {
            let field = format!(".{part}");
            code.match_indices(&field).any(|(at, _)| {
                let after = code[at + field.len()..].trim_start();
                let assigned = after.starts_with('=') && !after.starts_with("==");
                assigned || (through_mem && code[..at].contains("&mut") && !after.starts_with('.') && !after.starts_with('('))
            })
        });
        if code.contains("Reader::new()") || (in_reader && code.contains("Self::new()")) || replaces {
            found.push(line);
        }
    }
    found
}

/// Whether `code` calls one of `names` as a bare function: not a method (`.take(`) and not through a path (`mem::take(`, which is seen as that).
fn calls_by_name(code: &str, names: &[&str]) -> bool {
    names.iter().any(|name| {
        code.match_indices(name).any(|(at, _)| {
            code[..at]
                .chars()
                .last()
                .is_none_or(|before| !(before.is_alphanumeric() || matches!(before, '_' | '.' | ':')))
        })
    })
}

/// Failure it catches: code outside `tiles` builds a `Reader` of its own and takes a part from it (`reader.docs = Reader::new().docs`), or swaps or replaces
/// a part, which no visibility stops (the fields are `pub(crate)`) and the listing above doesn't show, as it lists items and not statements. The one
/// `Reader` there is, the engine's, is built in one place.
#[test]
fn nothing_outside_tiles_builds_a_reader_or_replaces_a_part() {
    let listed = listing();
    let parts: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.strip_prefix("Reader.pub(crate) ")?.split(':').next())
        .collect();
    assert!(parts.contains(&"docs"), "the parts are read from the listing: {parts:?}");
    let seeded = "    let docs = reader.docs.hot;\n    reader.docs = Reader::new().docs;\n    if reader.place == other {}\n    std::mem::swap(&mut reader.work, &mut spare.work);\n    reader.place.jump = None;\n";
    assert_eq!(
        builds_or_replaces_a_part(seeded, &parts),
        [
            "    reader.docs = Reader::new().docs;",
            "    std::mem::swap(&mut reader.work, &mut spare.work);"
        ],
        "the scan sees a part replaced and nothing else"
    );
    // (Review round 2.) A `Reader` built as `Self::new()` inside an `impl` of it, as a `Default` written for it would, and a part swapped or replaced by
    // `std::mem`'s functions brought in by name.
    let seeded = "use std::mem::{replace, swap};\n\nimpl Default for Reader {\n    fn default() -> Self {\n        Self::new()\n    }\n}\n\nimpl Other {\n    fn fresh() -> Self {\n        Self::new()\n    }\n}\n\nfn trade(reader: &mut Reader, spare: &mut Reader) {\n    swap(&mut reader.docs, &mut spare.docs);\n    let _ = replace(&mut reader.work, Work::new());\n    let _ = reader.place.take_moved_page();\n    let _ = pages.iter().take(3);\n}\n";
    assert_eq!(
        builds_or_replaces_a_part(seeded, &parts),
        [
            "        Self::new()",
            "    swap(&mut reader.docs, &mut spare.docs);",
            "    let _ = replace(&mut reader.work, Work::new());"
        ],
        "the scan sees a Reader built in its own impl and a part swapped by name, and nothing else"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for file in other_sources().into_iter().chain(sources()) {
        if file.starts_with(root.join("tiles")) || file == root.join("tiles.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for line in builds_or_replaces_a_part(&text, &parts) {
            found.push(format!("{}: {}", file.strip_prefix(&root).unwrap_or(&file).display(), line.trim()));
        }
    }
    assert_eq!(
        found,
        ["engine.rs: reader: Mutex::new(Reader::new()),"],
        "outside tiles, only the engine builds its one Reader, and nothing replaces a part"
    );
}

/// What the page view lets other code reach, sorted. Each change to it is deliberate: see [`what_the_page_view_lets_other_code_reach_is_listed`].
const SURFACE: &str = r#"
Applied (private struct, derives Default)
Ask (pub enum, derives Clone, Copy, Debug, PartialEq, Eq)
Asked (pub struct, derives Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)
Asked::pub fn now() -> Self
Asked::pub fn zoom(self, engine: &Engine, zoom: Zoom) -> Option<f64>
Asked::pub fn zoom_soon(self, engine: &Engine, value: f64) -> bool
Changed (private struct)
DocDrawing (pub(super) struct, derives Debug)
DocPlace (pub(super) struct, derives Debug)
DocRenderer: LookSource (trait impl)
DocRenderer: LookSource::fn image_boxes(&mut self, page: u32, generation: u64) -> Result<Option<Vec<[u16; 4]>>, RenderFailure>
DocRenderer: LookSource::fn is_stale(&self, generation: u64) -> bool
DocRenderer: LookSource::fn page_size(&mut self, page: u32) -> Result<(f32, f32), RenderFailure>
DocRenderer: LookSource::fn render_tile(&mut self, request: &TileRequest, surface: &PixelSurface) -> Result<TileResult, RenderFailure>
DocRenderer: LookSource::fn take_image_places_cleared(&mut self) -> bool
DocView (pub(crate) struct, derives Debug)
DocView::pub(crate) fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)>
DocView::pub(crate) fn fit(&self) -> f64
DocView::pub(crate) fn has_document(&self) -> bool
DocView::pub(crate) fn is_on_its_page(&self, point: PagePoint) -> bool
DocView::pub(crate) fn magnification(&self) -> f64
DocView::pub(crate) fn own_file_page(&self, position: u32) -> Option<u32>
DocView::pub(crate) fn page_count(&self) -> u32
DocView::pub(crate) fn page_size_shown(&self, page: u32) -> Option<(f32, f32)>
DocView::pub(crate) fn page_turns(&self, page: u32) -> u8
DocView::pub(crate) fn place_y(&self) -> f64
DocView::pub(crate) fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<PagePoint>
DocView::pub(crate) fn point_on_page(&self, x: f64, y: f64) -> Option<PagePoint>
DocView::pub(crate) fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32>
DocView::pub(crate) fn reading_anchor(&self) -> (u32, f64)
Docs (pub(crate) struct, derives Debug)
Docs::pub(crate) fn expected_stamp(&self, tab: u32, file: &std::path::Path) -> Option<FileStamp>
Docs::pub(crate) fn files_of(&self, tab: u32) -> Vec<PathBuf>
Docs::pub(crate) fn holds(&self, tab: u32) -> bool
Docs::pub(crate) fn hot_tab(&self) -> Option<u32>
Docs::pub(crate) fn is_own_file(&self, tab: u32, file: &std::path::Path) -> bool
Docs::pub(crate) fn shown(&self) -> Option<&DocView>
DrawKind (pub enum, derives Clone, Copy, Debug, PartialEq, Eq)
Ended (pub(super) enum, derives Clone, Copy, Debug, PartialEq, Eq)
Engine (pub struct, derives Clone)
Engine::pub fn activate(&self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: crate::layout::Spread)
Engine::pub fn applied(&self)
Engine::pub fn deactivate(&self)
Engine::pub fn drain(&self) -> Vec<Op>
Engine::pub fn empty_discardable(&self) -> usize
Engine::pub fn fit(&self) -> Option<f64>
Engine::pub fn follow_soon(&self, page: u32, within: f64) -> bool
Engine::pub fn has_thread(&self) -> bool
Engine::pub fn hold_drawing(&self, on: bool)
Engine::pub fn hot_tab(&self) -> Option<u32>
Engine::pub fn jump_to(&self, page: u32)
Engine::pub fn jump_within(&self, page: u32, within: f64) -> Option<u64>
Engine::pub fn key_given(&self, path: &Path)
Engine::pub fn links(&self) -> Links<'_>
Engine::pub fn magnification(&self) -> Option<f64>
Engine::pub fn memory_pressure(&self, critical: bool)
Engine::pub fn new(config: EngineConfig, events: Arc<dyn EngineEvents>) -> Self
Engine::pub fn note(&self) -> Option<String>
Engine::pub fn places(&self) -> (u64, Vec<(u32, SavedPlace)>)
Engine::pub fn points(&self) -> Points<'_>
Engine::pub fn qa(&self) -> Qa<'_> [qa feature only]
Engine::pub fn reading_anchor(&self) -> Option<(u32, f64)>
Engine::pub fn report(&self) -> ReaderReport
Engine::pub fn same_as(&self, other: &Self) -> bool
Engine::pub fn selection(&self) -> Selection<'_>
Engine::pub fn set_margins(&self, left: f64, right: f64)
Engine::pub fn set_paper(&self, paper: PaperKind)
Engine::pub fn set_share(&self, views: usize)
Engine::pub fn set_spread(&self, spread: crate::layout::Spread)
Engine::pub fn set_viewport(&self, viewport: Viewport) -> Vec<Op>
Engine::pub fn set_viewport_scrolled(&self, viewport: Viewport) -> ViewportOutcome
Engine::pub fn set_warm(&self, warm: &[u32])
Engine::pub fn shut_down(&self)
Engine::pub fn take_keyless(&self) -> Vec<PathBuf>
Engine::pub fn take_moved_page(&self) -> Option<u32>
Engine::pub fn wait_settled(&self, limit: Duration) -> bool
Engine::pub fn waking(&self) -> Wake<'_>
Engine::pub fn window_closed(&self)
Engine::pub fn window_opened(&self)
Engine::pub fn workers(&self) -> WorkerFigures<'_>
Engine::pub fn zoom(&self, zoom: Zoom) -> Option<f64>
Engine::pub fn zoom_soon(&self, value: f64) -> bool
EngineConfig (pub struct, derives Clone, Debug, Default)
EngineConfig.pub keys: Option<Arc<Keyring>>
EngineConfig.pub policy: Option<Policy>
EngineConfig.pub service: Option<ServiceConfig>
Entry (pub(super) struct, derives Debug)
HostQueue (pub(crate) struct, derives Debug)
HostQueue::pub(crate) fn has_ops(&self) -> bool
Issue (pub(super) struct, derives Clone, Debug)
Job (pub struct, derives Debug)
Job.pub file: PathBuf
Job.pub generation: u64
Job.pub kind: JobKind
Job.pub position: u32
Job.pub source_page: u32
Job.pub tab: u32
JobKind (pub enum, derives Debug)
JobPlan (pub struct, derives Debug)
JobPlan.pub file: PathBuf
JobPlan.pub generation: u64
JobPlan.pub position: u32
JobPlan.pub source_page: u32
JobPlan.pub tab: u32
JobPlan::pub fn ready(self) -> Option<Job>
Keyring (pub struct, derives Default)
Keyring: Debug (trait impl)
Keyring: Debug::fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
Keyring::pub fn for_path(&self, path: &Path) -> Option<Arc<recto_core::Password>>
Keyring::pub fn get(&self, identity: (u64, u64)) -> Option<Arc<recto_core::Password>>
Keyring::pub fn is_empty(&self) -> bool
Keyring::pub fn keep_only(&self, open: &[(u64, u64)])
Keyring::pub fn len(&self) -> usize
Keyring::pub fn set(&self, identity: (u64, u64), password: Arc<recto_core::Password>)
Links (pub struct, derives Clone, Copy)
Links::pub fn at(&self, x: f64, y: f64) -> Option<crate::links::LinkHit>
Links::pub fn own_file_page(&self, position: u32) -> Option<u32>
Links::pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32>
Links::pub fn take_clicked(&self) -> Option<crate::links::LinkHit>
Looks (pub(super) struct, derives Debug, Default)
LowResNote (pub(super) struct, derives Debug)
Next (pub(super) enum, derives Clone, Copy, Debug, PartialEq, Eq)
NoLook (pub(super) enum, derives Debug, PartialEq, Eq)
Op (pub enum, derives Clone, Debug, PartialEq)
OpenDoc (private struct)
Outcome (pub enum, derives Debug)
Owed (pub(super) struct, derives Debug)
PageMap (pub struct, derives Clone, Debug, Default, PartialEq, Eq)
PageMap.pub expected: Vec<Option<FileStamp>>
PageMap.pub files: Vec<PathBuf>
PageMap.pub follows: Option<FileStamp>
PageMap.pub own: Option<FileStamp>
PageMap.pub slots: Option<Vec<(u16, u32)>>
PageMap.pub turns: Option<Arc<[u8]>>
PageNow (pub(super) struct, derives Clone, Copy, Debug, Default, PartialEq, Eq)
Pending (private struct, derives Default)
Picture (private enum)
Place (pub(crate) struct, derives Debug)
Place::pub(crate) fn has_moved_page(&self) -> bool
Place::pub(crate) fn take_moved_page(&mut self) -> Option<u32>
Place::pub(crate) fn take_user_scrolled(&mut self) -> bool
Place::pub(crate) fn tell_again(&mut self) -> bool
Placement (pub(super) struct, derives Clone, Copy, Debug, PartialEq)
PlanKind (pub(super) enum, derives Debug)
Pointer (pub(crate) struct, derives Debug)
Pointer::pub(crate) fn dismiss_selection_notice(&mut self)
Pointer::pub(crate) fn forget_reveal(&mut self)
Pointer::pub(crate) fn forget_reveal_with(&mut self, page: u32)
Pointer::pub(crate) fn has_selection(&self) -> bool
Pointer::pub(crate) fn selection_can_be_sent(&self, pages: u32) -> bool
Pointer::pub(crate) fn selection_notice(&self) -> Option<Notice>
Pointer::pub(crate) fn selection_range(&self) -> Option<recto_index::select::Range>
Pointer::pub(crate) fn selection_release(&mut self)
Pointer::pub(crate) fn selection_text_now(&self, pages: u32) -> Option<String>
Pointer::pub(crate) fn take_clicked_link(&mut self) -> Option<crate::links::LinkHit>
Pointer::pub(crate) fn take_copied(&mut self) -> Option<recto_index::select::CopiedText>
Pointer::pub(crate) fn take_copy_signal(&mut self) -> bool
Points (pub struct, derives Clone, Copy)
Points::pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)>
Points::pub fn has_document(&self) -> bool
Points::pub fn on_given_page(&self, page: u32, x: f64, y: f64) -> Option<recto_index::select::PagePoint>
Points::pub fn on_page(&self, x: f64, y: f64) -> Option<(recto_index::select::PagePoint, (f32, f32))>
Points::pub fn page_count(&self) -> u32
Points::pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)>
Points::pub fn page_turns(&self, page: u32) -> u8
Qa (pub struct, derives Clone, Copy) [qa feature only]
Qa::pub fn follow_is_pending(&self) -> bool [qa feature only]
Qa::pub fn hold_asks(&self, ask: Ask, on: bool) [qa feature only]
Qa::pub fn hold_busy(&self, on: bool) [qa feature only]
Qa::pub fn wait_for_held_asks(&self, count: u32) -> bool [qa feature only]
QaHolds (pub(super) struct, derives Default) [qa feature only]
RawPicture (pub struct, derives Clone, Debug, PartialEq)
RawPicture.pub bgra: Vec<u8>
RawPicture.pub height: u32
RawPicture.pub page: u32
RawPicture.pub paper: PaperKind
RawPicture.pub scale: f32
RawPicture.pub width: u32
Reader (pub(crate) struct)
Reader.pub(crate) cache: TileCache
Reader.pub(crate) docs: Docs
Reader.pub(crate) host: HostQueue
Reader.pub(crate) place: Place
Reader.pub(crate) pointer: Pointer
Reader.pub(crate) wake: Waking
Reader.pub(crate) work: Work
Reader::pub fn activate(&mut self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: Spread)
Reader::pub fn applied(&mut self)
Reader::pub fn clear_selection(&mut self)
Reader::pub fn deactivate(&mut self)
Reader::pub fn drain(&mut self, budget: usize) -> Vec<Op>
Reader::pub fn file_changed(&mut self, tab: u32)
Reader::pub fn finish(&mut self, job: Job, outcome: Outcome)
Reader::pub fn follow(&mut self, page: u32, within: f64) -> Option<u32>
Reader::pub fn is_on_place(&self) -> bool
Reader::pub fn is_settled(&self) -> bool
Reader::pub fn is_showable(&self) -> bool
Reader::pub fn jump_to(&mut self, page: u32, within: f64) -> bool
Reader::pub fn key_given(&mut self, path: &std::path::Path)
Reader::pub fn memory_pressure(&mut self, critical: bool)
Reader::pub fn new() -> Self
Reader::pub fn next_plan(&mut self) -> Option<JobPlan>
Reader::pub fn note(&self) -> Option<&str>
Reader::pub fn note_version(&mut self, file: &std::path::Path, version: u64)
Reader::pub fn note_wake(&mut self, tab: u32, began: std::time::Instant)
Reader::pub fn places(&self) -> Vec<(u32, SavedPlace)>
Reader::pub fn pointer_is_over_text(&mut self, x: f64, y: f64) -> bool
Reader::pub fn pointer_link(&mut self, x: f64, y: f64) -> Option<crate::links::LinkHit>
Reader::pub fn report(&self) -> ReaderReport
Reader::pub fn restore_selection(&mut self, range: recto_index::select::Range)
Reader::pub fn reveal_range(&mut self, range: recto_index::select::Range)
Reader::pub fn running_is_wanted(&self) -> bool
Reader::pub fn select_all(&mut self)
Reader::pub fn selection_drag(&mut self, x: f64, y: f64)
Reader::pub fn selection_press(&mut self, x: f64, y: f64, clicks: u32, shift: bool)
Reader::pub fn set_margins(&mut self, left: f64, right: f64)
Reader::pub fn set_paper(&mut self, paper: PaperKind)
Reader::pub fn set_spread(&mut self, spread: Spread)
Reader::pub fn set_viewport(&mut self, viewport: Viewport) -> Vec<Op>
Reader::pub fn set_warm(&mut self, warm: &[u32])
Reader::pub fn sleep_material(&mut self, tab: u32, now: &PageMap) -> Option<SleepMaterial>
Reader::pub fn start_copy(&mut self, anyway: bool)
Reader::pub fn thread_restarted(&mut self, give_up: bool)
Reader::pub fn window_closed(&mut self)
Reader::pub fn window_opened(&mut self)
Reader::pub fn zoom(&mut self, zoom: Zoom) -> Option<f64>
ReaderCounts (pub struct, derives Clone, Debug, Default)
ReaderCounts.pub blank_seen_steps: u64
ReaderCounts.pub blank_steps: u64
ReaderCounts.pub cancelled: u64
ReaderCounts.pub cancels_asked: u64
ReaderCounts.pub discards_lost: u64
ReaderCounts.pub drawn: u64
ReaderCounts.pub jobs: u64
ReaderCounts.pub viewport_steps: u64
ReaderReport (pub struct)
ReaderReport.(pub Value)
Running (pub(super) struct, derives Debug)
SavedPlace (pub struct, derives Clone, Copy, Debug, PartialEq)
SavedPlace.pub anchor: (u32, f64)
SavedPlace.pub document_height: f64
SavedPlace.pub magnification: f64
SavedPlace.pub page: u32
SavedPlace.pub page_size: Option<PageBox>
SavedPlace.pub x: f64
SavedPlace.pub y: f64
Scratch (pub(super) struct, derives Debug, Default)
SelectingNews (private struct)
Selection (pub struct, derives Clone, Copy)
Selection::pub fn can_be_sent(&self) -> bool
Selection::pub fn clear_soon(&self) -> bool
Selection::pub fn copy(&self, anyway: bool)
Selection::pub fn dismiss_notice(&self)
Selection::pub fn drag(&self, x: f64, y: f64) -> Vec<Op>
Selection::pub fn has_any(&self) -> bool
Selection::pub fn hot(&self) -> Option<(u32, Option<recto_index::select::Range>)>
Selection::pub fn is_over_text(&self, x: f64, y: f64) -> bool
Selection::pub fn notice(&self) -> Option<Notice>
Selection::pub fn press(&self, x: f64, y: f64, clicks: u32, shift: bool) -> Vec<Op>
Selection::pub fn release(&self) -> Vec<Op>
Selection::pub fn restore(&self, from: (u32, u32), to: (u32, u32))
Selection::pub fn reveal(&self, page: u32, start: u32, end: u32)
Selection::pub fn select_all(&self)
Selection::pub fn take_copied(&self) -> Option<recto_index::select::CopiedText>
Selection::pub fn text_now(&self) -> Option<String>
Shared (private struct)
SleepDrawCounts (pub struct, derives Clone, Copy, Debug, Default, PartialEq, Eq)
SleepDrawCounts.pub heavy_passed: u64
SleepDrawCounts.pub jobs: u64
SleepDrawCounts.pub leaves_owing: u64
SleepDrawCounts.pub leaves_pictured: u64
SleepDrawCounts.pub pages_owed: u64
SleepDrawCounts.pub picture_bytes: u64
SleepDrawCounts.pub pictures: u64
SleepDraws (pub struct, derives Debug, Default)
SleepDraws::pub(crate) fn is_for(reader: &Reader, tab: u32) -> bool
SleepDraws::pub(crate) fn plan(reader: &mut Reader) -> Option<JobPlan>
SleepDraws::pub(crate) fn under_way(reader: &Reader) -> bool
SleepMaterial (pub struct, derives Clone, Debug, Default, PartialEq)
SleepMaterial.pub in_view: Vec<u32>
SleepMaterial.pub pictures: Vec<RawPicture>
SleepMaterial.pub sizes: Vec<(u32, PageBox)>
SleepMaterial.pub usual: Option<PageBox>
SleepMaterial.pub version: Option<u64>
SleepShot (pub struct, derives Debug)
SleepShot.pub page: u32
SleepShot.pub paper: PaperKind
SleepShot.pub scale: f32
SleepShot.pub surface: PixelSurface
SleepView (pub struct, derives Debug, Default)
SleepView.pub pictures: Vec<SleepShot>
SleepView.pub sizes: Vec<(u32, PageBox)>
SleepView.pub usual: Option<PageBox>
SleepView.pub version: Option<u64>
TabNow (pub(super) struct, derives Clone, Copy, Debug, Default, PartialEq, Eq)
TileCache (pub(crate) struct, derives Debug)
TileCache::pub(crate) fn empty_discardable(&mut self) -> usize
TileCache::pub(crate) fn set_share(&mut self, views: usize, host: &mut HostQueue)
TileKey (pub(super) struct, derives Clone, Copy, Debug, PartialEq, Eq, Hash)
Tries (private struct, derives Clone, Copy, Debug)
Unopened (private enum)
Viewport (pub struct, derives Clone, Copy, Debug, Default, PartialEq)
Viewport.pub backing: f64
Viewport.pub frame_width: f64
Viewport.pub height: f64
Viewport.pub live: bool
Viewport.pub magnification: f64
Viewport.pub scrolled_to: u64
Viewport.pub width: f64
Viewport.pub x: f64
Viewport.pub y: f64
ViewportOutcome (pub struct)
ViewportOutcome.pub anchor: Option<(u32, f64)>
ViewportOutcome.pub magnification: Option<f64>
ViewportOutcome.pub ops: Vec<Op>
ViewportOutcome.pub scrolled: bool
Wake (pub struct, derives Clone, Copy)
Wake::pub fn give_sleep(&self, tab: u32, view: crate::tiles::SleepView)
Wake::pub fn holds(&self, tab: u32) -> bool
Wake::pub fn is_on_place(&self) -> bool
Wake::pub fn is_showable(&self) -> bool
Wake::pub fn note_wake(&self, tab: u32, began: Instant)
Wake::pub fn sleep_material(&self, tab: u32, now: &PageMap) -> Option<crate::tiles::SleepMaterial>
Wake::pub fn window_up(&self)
WakeReport (pub struct, derives Clone, Copy, Debug, Default)
WakeReport.pub bare_with_picture: u16
WakeReport.pub blank_frames: u32
WakeReport.pub first_frame_us: Option<u64>
WakeReport.pub from_sleep: bool
WakeReport.pub had: WakeStock
WakeReport.pub sharp_us: Option<u64>
WakeReport.pub sleep_in_view: u16
WakeReport.pub sleep_pictures: u16
WakeReport.pub tab: u32
WakeStock (pub enum, derives Clone, Copy, Debug, Default, PartialEq, Eq)
WakeStock::pub fn covered_the_view(self) -> bool
WakeStock::pub fn word(self) -> &'static str
Waking (pub(crate) struct, derives Debug)
Waking::pub(crate) fn give_sleep(&mut self, tab: u32, view: SleepView)
Waking::pub(crate) fn last_wake(&self) -> Option<WakeReport>
Waking::pub(crate) fn window_up(&mut self)
Work (pub(crate) struct, derives Debug)
Work::pub(crate) fn hold_drawing(&mut self, on: bool)
Work::pub(crate) fn note_started(&mut self, job: &Job, cancel: Option<CancelHandle>)
Work::pub(crate) fn set_cancel(&mut self, cancel: CancelHandle)
Work::pub(crate) fn take_keyless(&mut self) -> Vec<PathBuf>
Work::pub(crate) fn take_reopen_files(&mut self) -> Vec<PathBuf>
Work::pub(crate) fn wanted_files(&self) -> Arc<Vec<PathBuf>>
WorkerFigures (pub struct, derives Clone, Copy)
WorkerFigures::pub fn bytes(&self, tab: u32) -> u64
WorkerFigures::pub fn footprint(&self) -> (u64, u64)
WorkerFigures::pub fn pids(&self) -> Vec<u32>
WorkerFigures::pub fn walled(&self) -> u32
Zoom (pub enum, derives Clone, Copy, Debug, PartialEq)
fn (pub) pub fn place_stamp() -> u64
fn (pub) pub fn typed_zoom(text: &str, decimal: char) -> Option<Zoom>
fn (pub) pub fn zoom_after_scroll(current: f64, points: f64) -> f64
fn (pub) pub fn zoom_percent(magnification: f64, fit: f64) -> f64
"#;
