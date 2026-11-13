//! **What a set of types lets other code reach, listed** (R3.6's listing, R3.7's one copy of it): every method visible past its module (`pub` or
//! `pub(crate)`) with its signature, every trait implemented with its methods, a type's own declaration with what it derives, and the fields named to be
//! listed. A test holds the listing against a list kept in its own file, so a part that can be built, replaced or reached around from outside where it
//! belongs shows up as a changed line in a diff a reviewer sees, not as a side effect of a move.
//!
//! A text scan of the sources, as the ratchet's is, for code formatted by rustfmt: an item under `#[cfg(test)]` isn't listed, and a signature is joined
//! onto one line. Here, rather than in each crate's tests, so there's one scan: the page view's (`recto-reader/src/surface.rs`) and the session's parts'
//! (`recto-session/src/tests/parts.rs`) listings came first, each with its own copy, and are to move onto this one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What a listing covers.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    /// The types listed, by name: their declarations, methods and trait impls. `None` lists everything in the files, free functions included.
    pub only: Option<&'a [&'a str]>,
    /// The types whose fields visible past their module are listed too.
    pub fields_of: &'a [&'a str],
}

/// Every `.rs` file under `dir`, in order, the folders under it included.
#[must_use]
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// **The listing of `files` within `scope`,** sorted, a line an item. A file that's all tests (`#![cfg(test)]`) isn't read.
///
/// # Panics
///
/// A file that can't be read.
#[must_use]
pub fn listing(files: &[PathBuf], scope: Scope<'_>) -> String {
    let mut lines = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        if text.contains("#![cfg(test)]") {
            continue;
        }
        listing_of(&text, scope, &mut lines);
    }
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

/// **`now` against the list kept,** saying what's gone and what's new, and the whole list now to copy, if they differ.
///
/// # Errors
///
/// What changed, for the test to fail with.
pub fn same_as(kept: &str, now: &str, what: &str) -> Result<(), String> {
    if now == kept.trim() {
        return Ok(());
    }
    let was: BTreeSet<&str> = kept.trim().lines().collect();
    let is: BTreeSet<&str> = now.lines().collect();
    let gone: Vec<&&str> = was.difference(&is).collect();
    let new: Vec<&&str> = is.difference(&was).collect();
    Err(format!(
        "what {what} lets other code reach changed.\ngone: {gone:#?}\nnew: {new:#?}\nthe whole list now:\n{now}\n"
    ))
}

/// Whether a line, trimmed, says its item is reached past its module.
fn visibility(line: &str) -> Option<&'static str> {
    if line.starts_with("pub(crate) ") {
        Some("pub(crate)")
    } else if line.starts_with("pub ") {
        Some("pub")
    } else {
        None
    }
}

/// The type an `impl` header is for, and the trait if it's a trait's: `impl<'a> Selection<'a> {` is `Selection`, `impl Default for Reader {` is
/// `Reader: Default`, `impl super::Inner {` is `Inner`.
fn impl_of(header: &str) -> String {
    let header = header.trim_end_matches('{').trim();
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

/// The type an `impl`'s name is of: `Reader: Default` is `Reader`'s.
fn type_of(of: &str) -> &str {
    of.split(": ").next().unwrap_or(of)
}

/// What a file lets past its module, as lines of the listing.
fn listing_of(text: &str, scope: Scope<'_>, into: &mut BTreeSet<String>) {
    let wanted = |name: &str| scope.only.is_none_or(|only| only.contains(&name));
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    // The `impl` or `struct` being read: its name, and whether the struct's fields are listed.
    let mut within: Option<String> = None;
    let mut fields_of: Option<String> = None;
    let mut test_item = false;
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
        if let Some(list) = trimmed.strip_prefix("#[derive(").and_then(|rest| rest.strip_suffix(")]")) {
            derives.extend(list.split(',').map(|name| name.trim().to_string()).filter(|name| !name.is_empty()));
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
            within = None;
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
            if trimmed.starts_with("impl") && trimmed.ends_with('{') {
                let of = impl_of(trimmed);
                if wanted(type_of(&of)) {
                    if of.contains(": ") {
                        into.insert(format!("{of} (trait impl)"));
                    }
                    within = Some(of);
                }
            } else if let Some((shown, kind, rest)) = type_item(trimmed) {
                let name = strip_generics(rest.split([' ', '{', '(', ';']).next().unwrap_or(""));
                if wanted(&name) {
                    if scope.fields_of.contains(&name.as_str()) {
                        fields_of = Some(name.clone());
                    }
                    let derived = if derived.is_empty() {
                        String::new()
                    } else {
                        format!(", derives {}", derived.join(", "))
                    };
                    into.insert(format!("{name} ({shown} {kind}{derived})"));
                }
            } else if let Some(shown) = visibility(trimmed)
                && trimmed.contains(" fn ")
                && scope.only.is_none()
            {
                into.insert(format!("fn ({shown}) {}", signature(&lines, &mut at)));
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
            let shown = visibility(trimmed);
            if trimmed.contains("fn ") && (shown.is_some() || of.contains(": ")) && !skip {
                let of = of.clone();
                into.insert(format!("{of}::{}", signature(&lines, &mut at)));
            }
            at += 1;
            continue;
        }
        // A field of a type whose fields are listed, visible past its module.
        if let Some(of) = &fields_of
            && line.starts_with("    ")
            && !line.starts_with("     ")
            && visibility(trimmed).is_some()
            && trimmed.contains(':')
            && !std::mem::take(&mut test_item)
        {
            into.insert(format!("{of}.{}", trimmed.trim_end_matches(',')));
        }
        test_item = false;
        at += 1;
    }
}

/// The signature starting at `at`, joined onto one line to its body's brace (or an empty body, `{}`) or its `;`, with `at` left on its last line.
fn signature(lines: &[&str], at: &mut usize) -> String {
    let mut signature = String::from(lines[*at].trim());
    while !signature.ends_with('{') && !signature.ends_with("{}") && !signature.ends_with(';') && *at + 1 < lines.len() {
        *at += 1;
        signature.push(' ');
        signature.push_str(lines[*at].trim());
    }
    one_line(&signature)
}

/// A struct or an enum, of any visibility: how visible, which, and the rest of its line.
fn type_item(line: &str) -> Option<(&'static str, &'static str, &str)> {
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

/// A signature as one line: its body's brace gone, its whitespace one space.
fn one_line(signature: &str) -> String {
    let signature = signature.trim_end_matches("{}").trim_end_matches('{').trim_end_matches(';').trim();
    signature
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "pub struct Reader {\n    a: u32,\n    pub(crate) part: Part,\n    #[cfg(test)]\n    pub seam: u8,\n}\n\n#[derive(Default)]\npub struct Other {\n    pub field: u8,\n}\n\nimpl Default for Part {\n    fn default() -> Self {\n        todo!()\n    }\n}\n\nimpl<'a> Facade<'a> {\n    pub fn press(\n        &self,\n        x: f64,\n    ) -> Vec<Op> {\n        todo!()\n    }\n\n    fn helper(&self) {}\n\n    #[cfg(test)]\n    pub fn only_in_tests(&self) {}\n}\n\nimpl super::Reader {\n    pub(crate) fn step(self: &Arc<Self>) {}\n}\n\n#[cfg(test)]\nimpl Part {\n    pub fn hidden(&self) {}\n}\n\npub fn free(x: u8) -> u8 {\n    x\n}\n";

    fn listed(scope: Scope<'_>) -> Vec<String> {
        let mut lines = BTreeSet::new();
        listing_of(SOURCE, scope, &mut lines);
        lines.into_iter().collect()
    }

    /// Failure: the scan lists what Rust keeps from other code, or misses what it lets through: an item under `#[cfg(test)]` listed, a signature across
    /// lines cut short, one with an empty body run on into the next item, a private method listed, a `super::` path taken for another type, a field of a type
    /// not asked for listed.
    #[test]
    fn the_listing_reads_what_rust_says() {
        assert_eq!(
            listed(Scope {
                only: None,
                fields_of: &["Reader"],
            }),
            [
                "Facade::pub fn press(&self, x: f64) -> Vec<Op>",
                "Other (pub struct, derives Default)",
                "Part: Default (trait impl)",
                "Part: Default::fn default() -> Self",
                "Reader (pub struct)",
                "Reader.pub(crate) part: Part",
                "Reader::pub(crate) fn step(self: &Arc<Self>)",
                "fn (pub) pub fn free(x: u8) -> u8",
            ]
        );
    }

    /// Failure: a listing of some types lists another's methods, its trait impls or the file's free functions, so it changes with code that isn't theirs.
    #[test]
    fn a_listing_of_some_types_lists_only_theirs() {
        assert_eq!(
            listed(Scope {
                only: Some(&["Reader", "Part"]),
                fields_of: &["Reader"],
            }),
            [
                "Part: Default (trait impl)",
                "Part: Default::fn default() -> Self",
                "Reader (pub struct)",
                "Reader.pub(crate) part: Part",
                "Reader::pub(crate) fn step(self: &Arc<Self>)",
            ]
        );
    }
}
