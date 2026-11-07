//! `search`: finds words across a document by reading its pages' text through one worker per file, in order, with no index yet. The later corpus and index go behind this same command.
//!
//! What it compares: the normaliser's text, lower-cased, with any run of space or line break as one space, so a phrase across a line break still matches. Bounded by a limit on hits and a limit on
//! time. Stopping early says where to go on from, so "nothing found" is only ever said of pages really searched.

use crate::{AgentError, SessionAccess, exit, parse_pages, select};
use recto_index::{hidden_report, normalize};
use recto_platform::FileStamp;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 200;
const MAX_QUERY_BYTES: usize = 1024;
/// How long one search may run before it stops and says where to go on from.
const MAX_TIME: Duration = Duration::from_secs(30);
/// Characters of context on each side of a hit.
const SNIPPET_CONTEXT: usize = 70;
/// Pages listed as having no text. The count is always whole.
const MAX_LISTED_EMPTY: usize = 200;

/// Which pages to look at, in order: position in the document as edited, which file, and which page there.
struct Plan {
    files: Vec<PlanFile>,
    pages: Vec<(u32, u16, u32)>,
    tab: u32,
}

struct PlanFile {
    path: PathBuf,
    /// What it must still be, for page numbers recorded from it to mean anything. `None` if nothing was recorded, as with a tab with no edits.
    expected: Option<FileStamp>,
    added: bool,
}

impl PlanFile {
    fn is_current(&self) -> bool {
        self.expected.is_none_or(|stamp| recto_platform::file_stamp(&self.path).ok() == Some(stamp))
    }

    fn label(&self) -> String {
        self.path.file_name().map_or_else(String::new, |name| recto_core::clean_line(&name.to_string_lossy()))
    }
}

pub(crate) fn search<A: SessionAccess>(access: &A, tab: &str, query: &str, pages: Option<&str>, limit: Option<u32>) -> Result<Value, AgentError> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(AgentError::new("bad_query", "That search is too long.", exit::FAILED).then("Search for a few words."));
    }
    let needle = fold(query).0;
    if needle.trim().is_empty() {
        return Err(AgentError::new("bad_query", "That search has nothing to look for.", exit::FAILED).then("Search for a word or a phrase."));
    }
    let plan = plan(access, tab, pages)?;
    let limit = crate::positive_limit(limit, DEFAULT_LIMIT, MAX_LIMIT)?;
    run(&plan, &needle, limit, MAX_TIME, Instant::now)
}

fn plan<A: SessionAccess>(access: &A, tab: &str, pages: Option<&str>) -> Result<Plan, AgentError> {
    let range = pages.map(|pages| parse_pages(pages, true)).transpose()?;
    access.with(|s| {
        let id = select(s, tab)?;
        let total = s.pages_in(id);
        let (start, len) = range.unwrap_or((0, total));
        // `5-` runs to the end, which is what a search's `next_page` is for.
        let open_ended = pages.is_some_and(|pages| pages.ends_with('-'));
        let len = if open_ended { len.min(total.saturating_sub(start)) } else { len };
        if start >= total.max(1) || u64::from(start) + u64::from(len) > u64::from(total) {
            return Err(AgentError::new("no_such_page", "Those pages aren't in this document.", exit::NOT_FOUND));
        }
        let path = s.path_of(id).ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))?;
        let mut files = vec![PlanFile { path, expected: s.page_edits(id).and_then(|_| s.own_stamp(id)), added: false }];
        let list = match s.page_edits(id) {
            None => (start..start.saturating_add(len)).map(|page| (page, 0, page)).collect(),
            Some(edits) => {
                files.extend(edits.added_sources().iter().map(|added| PlanFile { path: added.path.to_path_buf(), expected: added.stamp, added: true }));
                edits.slots().iter().enumerate().skip(start as usize).take(len as usize).map(|(position, slot)| (u32::try_from(position).unwrap_or(u32::MAX), slot.source, slot.page)).collect()
            }
        };
        Ok(Plan { files, pages: list, tab: id.0 })
    })
}

fn run(plan: &Plan, needle: &str, limit: usize, max_time: Duration, now: impl Fn() -> Instant) -> Result<Value, AgentError> {
    let started = now();
    // One worker at a time, whichever file it's for. A document with a thousand added files must not mean a thousand processes.
    let mut current: Option<(u16, DocRenderer)> = None;
    let mut hits = Vec::new();
    let mut searched = 0u32;
    let mut empty = Vec::new();
    let mut empty_count = 0u32;
    let mut failed = Vec::new();
    let mut next_page = None;
    let mut not_shown = Vec::new();
    let mut tag_pages = Vec::new();
    // A file that isn't what its page numbers were recorded from: its pages can't be trusted, and the rest of the document still can.
    let mut changed: Vec<u16> = Vec::new();
    for &(position, file, page) in &plan.pages {
        // Stopping only between pages, so asking again from `next_page` always moves on, however many hits a page has.
        if hits.len() >= limit || now().duration_since(started) >= max_time {
            next_page = Some(position + 1);
            break;
        }
        if changed.contains(&file) {
            searched += 1;
            failed.push(position + 1);
            continue;
        }
        if current.as_ref().is_none_or(|(open_file, _)| *open_file != file) {
            // The old one goes first, so there's never more than one.
            current = None;
            let source = &plan.files[usize::from(file)];
            let opened = if source.is_current() { DocRenderer::new(&source.path, None, ServiceConfig::discover()?, Policy::default()).map(Some) } else { Ok(None) };
            match opened {
                Ok(Some(renderer)) => current = Some((file, renderer)),
                Ok(None) | Err(RenderFailure::NotFound) if source.added || source.expected.is_some() => {
                    changed.push(file);
                    searched += 1;
                    failed.push(position + 1);
                    continue;
                }
                Ok(None) => continue,
                Err(failure) => return Err(failure.into()),
            }
        }
        let Some((_, renderer)) = current.as_mut() else { continue };
        let characters = match renderer.page_characters(page, false, false) {
            Ok(characters) => characters,
            // A file that won't be read at all is a failure of the whole search. One bad page isn't: it's listed, so "not found" doesn't cover it.
            Err(failure @ (RenderFailure::NeedsPassword | RenderFailure::NotFound | RenderFailure::NotPdf | RenderFailure::Security | RenderFailure::OpenFailed { .. } | RenderFailure::WorkerMissing | RenderFailure::LibraryMissing | RenderFailure::DocumentStopped)) => return Err(failure.into()),
            Err(_) => {
                searched += 1;
                failed.push(position + 1);
                continue;
            }
        };
        searched += 1;
        if characters.chars.is_empty() {
            empty_count += 1;
            if empty.len() < MAX_LISTED_EMPTY {
                empty.push(position + 1);
            }
            continue;
        }
        let normalized = normalize(&characters.chars, false);
        let has_tag_chars = normalized.tag_chars_removed > 0;
        let room = limit - hits.len();
        let found = find_hits(&normalized.text, needle, room);
        if found.total > found.shown.len() {
            not_shown.push(json!({ "page": position + 1, "hits": found.total - found.shown.len() }));
        }
        if has_tag_chars && found.total > 0 && tag_pages.len() < MAX_LISTED_EMPTY {
            tag_pages.push(position + 1);
        }
        let spelled = if found.total > 0 { None } else { normalized.also_spelled.iter().find(|spelled| fold(spelled).0.contains(needle)).filter(|_| hits.len() < limit) };
        if found.shown.is_empty() && spelled.is_none() {
            continue;
        }
        // Only a page with something to show is drawn to look for hidden text: doing it for every page costs three to five times as much. `docs/ENGINEERING.md` has the numbers. So a hit always says, and a page never looked at has nothing to say.
        let hidden = hidden_on_page(renderer, page);
        for (line, snippet) in found.shown {
            hits.push(json!({ "page": position + 1, "line": line, "page_has_tag_chars": has_tag_chars, "hidden_text": hidden, "untrusted": { "snippet": snippet } }));
        }
        if let Some(spelled) = spelled {
            hits.push(json!({ "page": position + 1, "line": null, "also_spelled": true, "page_has_tag_chars": has_tag_chars, "hidden_text": hidden, "untrusted": { "snippet": spelled } }));
        }
    }
    Ok(json!({
        "tab": plan.tab,
        "hits": hits,
        "hidden_text_checked": "pages_with_hits",
        "coverage": {
            "pages_searched": searched,
            "pages_in_scope": plan.pages.len(),
            "pages_with_no_text": empty_count,
            "no_text_pages": empty,
            "failed_pages": failed,
            "hits_not_shown": not_shown,
            "pages_with_tag_chars": tag_pages,
            "changed_files": changed.iter().map(|file| json!({ "untrusted": { "file": plan.files[usize::from(*file)].label() } })).collect::<Vec<_>>(),
            "next_page": next_page,
        },
    }))
}

/// Draws a page to see whether it holds text no eye can see. If drawing it fails, `checked` says so, which never reads as clean.
fn hidden_on_page(renderer: &mut DocRenderer, page: u32) -> Value {
    match renderer.page_characters(page, false, true) {
        Ok(characters) => {
            let report = hidden_report(&characters.chars, characters.hidden_checked);
            json!({ "checked": report.checked, "flagged": report.flagged })
        }
        Err(_) => json!({ "checked": false, "flagged": null }),
    }
}

/// Lower-cases and joins whitespace, and notes where each byte of the result came from. Returns the folded text and that map.
fn fold(text: &str) -> (String, Vec<u32>) {
    let mut folded = String::with_capacity(text.len());
    let mut sources = Vec::with_capacity(text.len());
    let mut pending_space = false;
    for (at, c) in text.char_indices() {
        if c.is_whitespace() {
            pending_space = !folded.is_empty();
            continue;
        }
        let at = u32::try_from(at).unwrap_or(u32::MAX);
        if pending_space {
            folded.push(' ');
            sources.push(at);
            pending_space = false;
        }
        for lower in c.to_lowercase() {
            folded.push(lower);
            sources.resize(folded.len(), at);
        }
    }
    (folded, sources)
}

/// What's on one page: how many hits in all, and the first few, each with its one-based line and the words around it on one line.
struct PageHits {
    total: usize,
    shown: Vec<(u32, String)>,
}

/// Finds the folded needle in the text. Snippets are made only for the first `room`: a hostile page of two million characters searched for one letter would otherwise make two million strings before the limit was looked at.
fn find_hits(text: &str, needle: &str, room: usize) -> PageHits {
    let (folded, sources) = fold(text);
    let mut hits = PageHits { total: 0, shown: Vec::with_capacity(room.min(MAX_LIMIT as usize)) };
    let (mut line, mut counted_to) = (1u32, 0usize);
    for (at, found) in folded.match_indices(needle) {
        hits.total += 1;
        if hits.shown.len() >= room {
            continue;
        }
        let start = sources.get(at).map_or(text.len(), |source| *source as usize);
        let end = sources.get(at + found.len()).map_or(text.len(), |source| *source as usize);
        let bytes_between = text.get(counted_to..start).unwrap_or("");
        line += u32::try_from(bytes_between.bytes().filter(|byte| *byte == b'\n').count()).unwrap_or(0);
        counted_to = start;
        hits.shown.push((line, snippet(text, start, end)));
    }
    hits
}

fn snippet(text: &str, start: usize, end: usize) -> String {
    let before: Vec<char> = text.get(..start).unwrap_or("").chars().rev().take(SNIPPET_CONTEXT).collect();
    let after: String = text.get(end..).unwrap_or("").chars().take(SNIPPET_CONTEXT).collect();
    let middle = text.get(start..end).unwrap_or("");
    let joined: String = before.iter().rev().collect::<String>() + middle + &after;
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_and_snippets(text: &str, needle: &str) -> Vec<(u32, String)> {
        find_hits(text, needle, usize::MAX).shown
    }

    /// Failure: a phrase broken across a line isn't found, or a hit's line number is off, so a citation points at the wrong place.
    #[test]
    fn a_phrase_across_a_line_break_is_found_on_its_line() {
        let hits = lines_and_snippets("first line\nThe Force\nMajeure clause\nand Force Majeure again", "force majeure");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, 2);
        assert_eq!(hits[1].0, 4);
        assert!(hits[0].1.contains("Force Majeure clause"), "{}", hits[0].1);
    }

    /// Failure: a letter whose lower case has a different length moves every position after it, cuts a character in half and panics on hostile text.
    #[test]
    fn folding_keeps_positions_through_letters_that_change_length() {
        let (folded, sources) = fold("İİİ  Æbc");
        assert_eq!(folded.len(), sources.len());
        let hits = lines_and_snippets("İİİ Æbc\nİ̇\u{130} æbc", &fold("æbc").0);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].1.contains("Æbc"));
        assert!(lines_and_snippets("", "x").is_empty());
    }

    /// Failure: a hostile page with one long line makes a snippet of its whole length.
    #[test]
    fn a_snippet_is_bounded() {
        let long = "x ".repeat(500_000) + "needle" + &" y".repeat(500_000);
        let hits = lines_and_snippets(&long, "needle");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.len() < 500, "{}", hits[0].1.len());
    }

    /// Failure: a page of two million characters searched for one letter builds a string for every hit before the limit is looked at, hundreds of megabytes, and the time cap only applies between pages. The count stays whole, and only `room` are built.
    #[test]
    fn hits_past_the_room_are_counted_and_not_built() {
        let page = "a".repeat(2_000_000);
        let started = Instant::now();
        let found = find_hits(&page, "a", 3);
        assert_eq!(found.total, 2_000_000);
        assert_eq!(found.shown.len(), 3, "only what's asked for is built");
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
        let none = find_hits(&page, "a", 0);
        assert_eq!((none.total, none.shown.len()), (2_000_000, 0));
    }

    /// Failure: many hits on many lines lose track of their lines once some are past the limit, so a citation is wrong.
    #[test]
    fn every_shown_hit_has_the_right_line() {
        let page: String = (0..50).map(|line| format!("word {line}\n")).collect();
        let found = find_hits(&page, "word", 10);
        assert_eq!(found.total, 50);
        let lines: Vec<u32> = found.shown.iter().map(|hit| hit.0).collect();
        assert_eq!(lines, (1..=10).collect::<Vec<u32>>());
        assert!(found.shown[9].1.contains("word 9"), "{}", found.shown[9].1);
    }
}
