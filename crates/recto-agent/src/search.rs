//! `search`: finds words across a document. A file with a finished corpus is read from that, a pass over text already on disk. Any other is read through one worker per file, in order, and asked to be indexed for next time. Which way a page was read changes nothing about what's said of it, and `route` says which it was.
//!
//! What it compares: the normaliser's text, lower-cased, with any run of space or line break as one space, so a phrase across a line break still matches. Bounded by a limit on hits and a limit on
//! time. Stopping early says where to go on from, so "nothing found" is only ever said of pages really searched.

use crate::index::{FileIndex, Indexing};
use crate::{AgentError, SessionAccess, exit, parse_pages, select};
use recto_index::corpus::{CorpusKey, Damaged, PageKind, ReadyCorpus, StoredPage};
use recto_index::{hidden_report, read_page};
use recto_platform::FileStamp;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig};
use serde_json::{Value, json};
use std::ops::Range;
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

pub(crate) fn search<A: SessionAccess>(access: &A, tab: &str, query: &str, pages: Option<&str>, limit: Option<u32>, scan: bool) -> Result<Value, AgentError> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(AgentError::new("bad_query", "That search is too long.", exit::FAILED).then("Search for a few words."));
    }
    let needle = fold(query).0;
    if needle.trim().is_empty() {
        return Err(AgentError::new("bad_query", "That search has nothing to look for.", exit::FAILED).then("Search for a word or a phrase."));
    }
    let plan = plan(access, tab, pages)?;
    let limit = crate::positive_limit(limit, DEFAULT_LIMIT, MAX_LIMIT)?;
    run(&plan, &needle, limit, MAX_TIME, Instant::now, access.indexing(), scan)
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

/// How one file's pages get read in this search. Worked out when its first page comes up, and never again: a corpus doesn't change.
#[derive(Clone, Copy)]
enum FileRoute {
    NotLookedAt,
    Index(CorpusKey),
    Scan,
}

/// The corpus for this page, opened if it isn't the one that's open. One at a time, as with workers: a document of a thousand added files mustn't hold a thousand descriptors.
fn corpus_for<'a>(open: &'a mut Option<(u16, ReadyCorpus)>, file: u16, key: CorpusKey, indexing: Option<&dyn Indexing>) -> Result<&'a ReadyCorpus, Damaged> {
    if open.as_ref().is_none_or(|(open_file, _)| *open_file != file) {
        *open = None;
        let indexing = indexing.ok_or(Damaged)?;
        *open = Some((file, ReadyCorpus::open(indexing.root(), key)?.ok_or(Damaged)?));
    }
    open.as_ref().map(|(_, corpus)| corpus).ok_or(Damaged)
}

/// A corpus that doesn't add up is thrown away, and made again. Page by page is always right.
fn throw_away_corpus(indexing: Option<&dyn Indexing>, source: &PlanFile, key: CorpusKey) {
    if let Some(indexing) = indexing {
        let _ = recto_index::corpus::clear(indexing.root(), key);
        indexing.want(&source.path, key);
    }
}

/// What's been found so far. The same whichever way a page was read.
#[derive(Default)]
struct Found {
    hits: Vec<Value>,
    searched: u32,
    empty: Vec<u32>,
    empty_count: u32,
    failed: Vec<u32>,
    next_page: Option<u32>,
    not_shown: Vec<Value>,
    tag_pages: Vec<u32>,
    /// A file that isn't what its page numbers were recorded from: its pages can't be trusted, and the rest of the document still can.
    changed: Vec<u16>,
    /// Every hit, including those past the limit. The other spelling of a word counts as one, as it does in `hits`.
    total: u64,
    /// `total` may be short, because of pages past `next_page` that couldn't be counted.
    total_cut_short: bool,
    /// Pages past `next_page` that couldn't be read, so they aren't in `total`.
    uncounted: u32,
    used_index: bool,
    used_scan: bool,
}

/// One page's text, whichever way it was read.
struct PageText<'a> {
    position: u32,
    /// Its page in its own file.
    page: u32,
    text: &'a str,
    also_spelled: &'a [&'a str],
    has_tag_chars: bool,
}

impl Found {
    fn no_text(&mut self, position: u32) {
        self.searched += 1;
        self.empty_count += 1;
        if self.empty.len() < MAX_LISTED_EMPTY {
            self.empty.push(position + 1);
        }
    }

    /// Takes what's on a page. `hidden` is only asked for if the page has something to show, as finding out can mean drawing it.
    fn take(&mut self, page: &PageText<'_>, needle: &str, limit: usize, source: &PlanFile, hidden: impl FnOnce() -> HiddenOnPage) {
        let position = page.position;
        let room = limit - self.hits.len();
        let found = find_hits(page.text, needle, room);
        self.total += found.total as u64;
        if found.total > found.shown.len() {
            self.not_shown.push(json!({ "page": position + 1, "hits": found.total - found.shown.len() }));
        }
        if page.has_tag_chars && found.total > 0 && self.tag_pages.len() < MAX_LISTED_EMPTY {
            self.tag_pages.push(position + 1);
        }
        let spelled = if found.total > 0 { None } else { page.also_spelled.iter().find(|spelled| fold(spelled).0.contains(needle)).filter(|_| self.hits.len() < limit) };
        if found.shown.is_empty() && spelled.is_none() {
            return;
        }
        // Only a page with something to show is checked for hidden text: doing it for every page by drawing it costs three to five times as much. `docs/ENGINEERING.md` has the numbers. So a hit always says, and a page never looked at has nothing to say.
        let hidden = hidden();
        for (line, snippet, range) in found.shown {
            let mut hit = json!({ "page": position + 1, "line": line, "page_has_tag_chars": page.has_tag_chars, "hidden_text": hidden.summary, "overlaps_hidden": hidden.overlaps(&range), "untrusted": { "snippet": snippet } });
            say_where_from(&mut hit, page.page, source);
            self.hits.push(hit);
        }
        if let Some(spelled) = spelled {
            self.total += 1;
            let mut hit = json!({ "page": position + 1, "line": null, "also_spelled": true, "page_has_tag_chars": page.has_tag_chars, "hidden_text": hidden.summary, "untrusted": { "snippet": spelled } });
            say_where_from(&mut hit, page.page, source);
            self.hits.push(hit);
        }
    }
}

/// Opens this page's worker if it isn't the one already open. One at a time, whichever file it's for: a document with a thousand added files must not mean a thousand processes. `None` if the file isn't what its page numbers were recorded from, or can't be found.
fn worker_for<'a>(current: &'a mut Option<(u16, DocRenderer)>, file: u16, source: &PlanFile) -> Result<Option<&'a mut DocRenderer>, RenderFailure> {
    if current.as_ref().is_none_or(|(open_file, _)| *open_file != file) {
        // The old one goes first, so there's never more than one.
        *current = None;
        if !source.is_current() {
            return Ok(None);
        }
        *current = Some((file, DocRenderer::new(&source.path, None, ServiceConfig::discover()?, Policy::default())?));
    }
    Ok(current.as_mut().map(|(_, renderer)| renderer))
}

/// Works out how a file's pages get read: from its corpus if there's a finished one for the file as it is now, or else page by page, asking for a corpus to be built for next time.
fn look_at_file(indexing: Option<&dyn Indexing>, source: &PlanFile, force_scan: bool, not_ready: &mut Vec<FileIndex>) -> FileRoute {
    let Some(indexing) = indexing.filter(|_| !force_scan) else { return FileRoute::Scan };
    if !source.is_current() {
        not_ready.push(FileIndex::Unavailable);
        return FileRoute::Scan;
    }
    match crate::index::file_index(indexing, &source.path, true) {
        (Some(key), FileIndex::Ready(_)) => FileRoute::Index(key),
        (_, state) => {
            not_ready.push(state);
            FileRoute::Scan
        }
    }
}

fn run(plan: &Plan, needle: &str, limit: usize, max_time: Duration, now: impl Fn() -> Instant, indexing: Option<&dyn Indexing>, force_scan: bool) -> Result<Value, AgentError> {
    let started = now();
    let mut current: Option<(u16, DocRenderer)> = None;
    let mut corpus_open: Option<(u16, ReadyCorpus)> = None;
    let mut found = Found::default();
    let mut routes = vec![FileRoute::NotLookedAt; plan.files.len()];
    let mut not_ready = Vec::new();
    let (mut buffer, mut folded) = (Vec::new(), String::new());
    let mut counting_only = false;
    for &(position, file, page) in &plan.pages {
        let source = &plan.files[usize::from(file)];
        let out_of_time = now().duration_since(started) >= max_time;
        // Stopping only between pages, so asking again from `next_page` always moves on, however many hits a page has.
        if !counting_only && (found.hits.len() >= limit || out_of_time) {
            found.next_page = Some(position + 1);
            counting_only = true;
        }
        if counting_only && out_of_time {
            found.total_cut_short = true;
            break;
        }
        if !counting_only && found.changed.contains(&file) {
            found.failed.push(position + 1);
            continue;
        }
        if matches!(routes[usize::from(file)], FileRoute::NotLookedAt) {
            // A file reached only for the count doesn't change how the answer's hits were read, so it isn't said to have.
            let mut not_said = Vec::new();
            routes[usize::from(file)] = look_at_file(indexing, source, force_scan, if counting_only { &mut not_said } else { &mut not_ready });
        }
        if counting_only {
            // Past the limit, only a corpus makes it cheap to go on, and only for the count.
            let FileRoute::Index(key) = routes[usize::from(file)] else {
                found.total_cut_short = true;
                break;
            };
            let read = match corpus_for(&mut corpus_open, file, key, indexing) {
                Ok(corpus) if page >= corpus.pages() => {
                    found.uncounted += 1;
                    continue;
                }
                Ok(corpus) => corpus.page(page, &mut buffer),
                Err(Damaged) => Err(Damaged),
            };
            match read {
                Ok(stored) if stored.kind == PageKind::Text => found.total += count_on_page(&stored, needle, &mut folded),
                Ok(stored) if stored.kind == PageKind::Failed => found.uncounted += 1,
                Ok(_) => {}
                Err(Damaged) => {
                    throw_away_corpus(indexing, source, key);
                    found.total_cut_short = true;
                    break;
                }
            }
            continue;
        }
        let mut read_live_after_failure = false;
        if let FileRoute::Index(key) = routes[usize::from(file)] {
            let read = match corpus_for(&mut corpus_open, file, key, indexing) {
                // Pages it hasn't got are pages the file hasn't got: the same as a worker saying so.
                Ok(corpus) if page >= corpus.pages() => Ok(None),
                Ok(corpus) => corpus.page(page, &mut buffer).map(Some),
                Err(Damaged) => Err(Damaged),
            };
            match read {
                Ok(None) => {
                    found.used_index = true;
                    found.failed.push(position + 1);
                    continue;
                }
                Ok(Some(stored)) => {
                    if stored.kind == PageKind::Failed {
                        // That's only what happened when it was built, which may have been the moment rather than the page. It goes on down to be read now. Past this a whole file can't be read, so this search says that page failed, rather than ending.
                        read_live_after_failure = true;
                    }
                    match stored.kind {
                        PageKind::Failed => {}
                        PageKind::NoText => {
                            found.used_index = true;
                            found.no_text(position);
                        }
                        PageKind::Text => {
                            found.used_index = true;
                            found.searched += 1;
                            let also_spelled = stored.also_spelled();
                            fold_into(stored.text, &mut folded);
                            if !folded.contains(needle) && !also_spelled.iter().any(|spelled| fold(spelled).0.contains(needle)) {
                                continue;
                            }
                            let text = PageText { position, page, text: stored.text, also_spelled: &also_spelled, has_tag_chars: stored.has_tag_chars };
                            found.take(&text, needle, limit, source, || {
                                if stored.hidden_checked {
                                    HiddenOnPage { summary: json!({ "checked": true, "flagged": stored.hidden_flagged }), spans: byte_spans(stored.text, &stored.hidden_spans()), partial: stored.spans_dropped }
                                } else {
                                    // The build couldn't check this page under its own limits. A search can.
                                    match worker_for(&mut current, file, source) {
                                        Ok(Some(renderer)) => hidden_on_page(renderer, page, stored.text),
                                        Ok(None) | Err(_) => HiddenOnPage { summary: json!({ "checked": false, "flagged": null }), spans: Vec::new(), partial: true },
                                    }
                                }
                            });
                        }
                    }
                    if !read_live_after_failure {
                        continue;
                    }
                }
                // This page and the rest of the file are read one by one.
                Err(Damaged) => {
                    corpus_open = None;
                    throw_away_corpus(indexing, source, key);
                    not_ready.push(FileIndex::Failed("damaged".to_string()));
                    routes[usize::from(file)] = FileRoute::Scan;
                }
            }
        }
        found.used_scan = true;
        let renderer = match worker_for(&mut current, file, source) {
            Ok(Some(renderer)) => renderer,
            Err(_) if read_live_after_failure => {
                found.failed.push(position + 1);
                continue;
            }
            Ok(None) | Err(RenderFailure::NotFound) if source.added || source.expected.is_some() => {
                found.changed.push(file);
                found.failed.push(position + 1);
                continue;
            }
            Ok(None) => continue,
            Err(failure) => return Err(failure.into()),
        };
        let characters = match renderer.page_characters(page, true, false) {
            Ok(characters) => characters,
            Err(_) if read_live_after_failure => {
                found.failed.push(position + 1);
                continue;
            }
            // A file that won't be read at all is a failure of the whole search. One bad page isn't: it's listed, so "not found" doesn't cover it.
            Err(failure @ (RenderFailure::NeedsPassword | RenderFailure::NotFound | RenderFailure::NotPdf | RenderFailure::Security | RenderFailure::OpenFailed { .. } | RenderFailure::WorkerMissing | RenderFailure::LibraryMissing | RenderFailure::DocumentStopped)) => return Err(failure.into()),
            Err(_) => {
                found.failed.push(position + 1);
                continue;
            }
        };
        if characters.chars.is_empty() {
            found.no_text(position);
            continue;
        }
        found.searched += 1;
        let normalized = read_page(&characters.chars, &characters.boxes, false).normalized;
        let also_spelled: Vec<&str> = normalized.also_spelled.iter().map(String::as_str).collect();
        let text = PageText { position, page, text: &normalized.text, also_spelled: &also_spelled, has_tag_chars: normalized.tag_chars_removed > 0 };
        found.take(&text, needle, limit, source, || hidden_on_page(renderer, page, &normalized.text));
    }
    let route = match (found.used_index, found.used_scan) {
        (true, true) => "mixed",
        (true, false) => "index",
        _ => "scan",
    };
    let (index_state, index_note) = if force_scan {
        ("not_used", "Read the pages one by one, as asked.".to_string())
    } else if plan.pages.is_empty() {
        ("not_used", "There were no pages to read.".to_string())
    } else if let Some(indexing) = indexing {
        (not_ready.first().map_or("ready", FileIndex::word), not_ready.first().map_or_else(|| "Answered from the index.".to_string(), |state| state.scan_note(indexing.starts_builds())))
    } else {
        ("off", "This session has no index, so this read the pages one by one.".to_string())
    };
    Ok(json!({
        "tab": plan.tab,
        "hits": found.hits,
        "hidden_text_checked": "pages_with_hits",
        "route": route,
        "index": { "state": index_state, "note": index_note },
        "coverage": {
            "pages_searched": found.searched,
            "pages_in_scope": plan.pages.len(),
            "pages_with_no_text": found.empty_count,
            "no_text_pages": found.empty,
            "failed_pages": found.failed,
            "hits_not_shown": found.not_shown,
            "pages_with_tag_chars": found.tag_pages,
            "changed_files": found.changed.iter().map(|file| json!({ "untrusted": { "file": plan.files[usize::from(*file)].label() } })).collect::<Vec<_>>(),
            "next_page": found.next_page,
            "hits_total": (!found.total_cut_short && found.uncounted == 0 && found.failed.is_empty()).then_some(found.total),
            "hits_at_least": found.total,
            "uncounted_pages": found.uncounted,
        },
    }))
}

/// How many hits a page has, for the total, with nothing built. The same count [`find_hits`] gives: its own spelling, and if there's none of that, another spelling counts as one.
fn count_on_page(stored: &StoredPage<'_>, needle: &str, folded: &mut String) -> u64 {
    fold_into(stored.text, folded);
    let on_page = folded.matches(needle).count() as u64;
    if on_page > 0 {
        return on_page;
    }
    u64::from(stored.also_spelled().iter().any(|spelled| fold(spelled).0.contains(needle)))
}

/// Says where a hit's page really comes from, on every hit, so an agent reading these needn't know what it means for a field to be missing.
fn say_where_from(hit: &mut Value, page: u32, source: &PlanFile) {
    hit["source_page"] = json!(page + 1);
    hit["from_added_file"] = json!(source.added);
    if source.added {
        hit["untrusted"]["added_file"] = json!(source.label());
    }
}

/// What drawing a page says about hidden text.
struct HiddenOnPage {
    summary: Value,
    /// Where it is in the page's text, in bytes, in order. Empty if there's none, or if they couldn't be placed.
    spans: Vec<Range<usize>>,
    /// Some of it isn't in `spans`, so a hit that overlaps none of them may still overlap some.
    partial: bool,
}

impl HiddenOnPage {
    /// Whether a hit's words include hidden text: `null` if that can't be said, which isn't the same as no.
    fn overlaps(&self, hit: &Range<usize>) -> Option<bool> {
        let after = self.spans.partition_point(|span| span.end <= hit.start);
        if self.spans.get(after).is_some_and(|span| span.start < hit.end) {
            return Some(true);
        }
        (!self.partial).then_some(false)
    }
}

/// Draws a page to see whether it holds text that looks hidden, reading it the same way the search did, so moved words are judged as they read. If drawing it fails, `checked` says so, which never reads as clean. If the second reading isn't the text that was searched, nothing can be said about where hidden text falls on a hit.
fn hidden_on_page(renderer: &mut DocRenderer, page: u32, searched: &str) -> HiddenOnPage {
    match renderer.page_characters(page, true, true) {
        Ok(characters) => {
            let report = hidden_report(&characters.chars, characters.hidden_checked);
            let reading = read_page(&characters.chars, &characters.boxes, characters.hidden_checked);
            let same_text = reading.normalized.text == searched;
            let spans = if same_text { byte_spans(searched, &reading.hidden_spans) } else { Vec::new() };
            let summary = json!({ "checked": report.checked, "flagged": report.flagged || reading.hidden_flagged });
            HiddenOnPage { summary, spans, partial: !report.checked || !same_text || reading.hidden_spans_dropped > 0 }
        }
        Err(_) => HiddenOnPage { summary: json!({ "checked": false, "flagged": null }), spans: Vec::new(), partial: true },
    }
}

/// Turns spans counted in characters into byte ranges, in one pass over the text.
fn byte_spans(text: &str, spans: &[(u32, u32)]) -> Vec<Range<usize>> {
    let mut out = Vec::with_capacity(spans.len());
    if spans.is_empty() {
        return out;
    }
    let mut next = 0;
    let mut start = None;
    for (count, (at, _)) in text.char_indices().enumerate() {
        let count = count as u32;
        let Some(span) = spans.get(next) else { break };
        if count == span.1 {
            if let Some(start) = start.take() {
                out.push(start..at);
            }
            next += 1;
        }
        if spans.get(next).is_some_and(|span| span.0 == count) {
            start = Some(at);
        }
    }
    if let (Some(start), true) = (start, next < spans.len()) {
        out.push(start..text.len());
    }
    out
}

/// What [`fold`] makes, into a buffer that's used again, with no map back to the text. It runs on every page of a search, and the map only matters on a page with a hit. An all-ASCII page, most of them, takes a shorter way.
fn fold_into(text: &str, folded: &mut String) {
    folded.clear();
    folded.reserve(text.len());
    let mut pending_space = false;
    if text.is_ascii() {
        for byte in text.bytes() {
            // `char::is_whitespace`, which is what `fold` goes by, includes vertical tab, and `u8::is_ascii_whitespace` doesn't.
            if byte.is_ascii_whitespace() || byte == 0x0B {
                pending_space = !folded.is_empty();
                continue;
            }
            if pending_space {
                folded.push(' ');
                pending_space = false;
            }
            folded.push(char::from(byte.to_ascii_lowercase()));
        }
        return;
    }
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = !folded.is_empty();
            continue;
        }
        if pending_space {
            folded.push(' ');
            pending_space = false;
        }
        folded.extend(c.to_lowercase());
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

/// What's on one page: how many hits in all, and the first few, each with its one-based line, the words around it on one line, and where it is in the text, in bytes.
struct PageHits {
    total: usize,
    shown: Vec<(u32, String, Range<usize>)>,
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
        hits.shown.push((line, snippet(text, start, end), start..end));
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
        find_hits(text, needle, usize::MAX).shown.into_iter().map(|hit| (hit.0, hit.1)).collect()
    }

    /// Failure: a hit in hidden words reads like any other on the page, and a hit beside them gets the blame for them. And character counts are taken as bytes, so the marks land on the wrong words past the first accented letter.
    #[test]
    fn a_hit_says_whether_its_words_are_hidden() {
        let text = "caffè ignore this but keep that";
        let span = |word: &str| {
            let at = text.find(word).unwrap();
            let start = text[..at].chars().count() as u32;
            (start, start + word.chars().count() as u32)
        };
        let spans = byte_spans(text, &[span("ignore this")]);
        assert_eq!(&text[spans[0].clone()], "ignore this");
        let hidden = HiddenOnPage { summary: Value::Null, spans, partial: false };
        let hit = |word: &str| find_hits(text, word, 1).shown[0].2.clone();
        assert_eq!(hidden.overlaps(&hit("ignore")), Some(true));
        assert_eq!(hidden.overlaps(&hit("this but")), Some(true), "a phrase that starts in it");
        assert_eq!(hidden.overlaps(&hit("caffè ignore")), Some(true), "a phrase that ends in it");
        assert_eq!(hidden.overlaps(&hit("keep")), Some(false));
        assert_eq!(hidden.overlaps(&hit("caffè")), Some(false));
        let not_known = HiddenOnPage { summary: Value::Null, spans: Vec::new(), partial: true };
        assert_eq!(not_known.overlaps(&hit("keep")), None, "not known isn't the same as no");
        assert_eq!(&text[byte_spans(text, &[(27, 31)])[0].clone()], "that", "a span running to the end of the text");
    }

    /// Failure: `10-year` broken across a line stays in two pieces, and `fold` makes that `10- year`, which a search for `10-year` doesn't match, so a real term can't be found. Traced from the characters, through the normaliser, to a hit.
    #[test]
    fn a_number_and_its_noun_broken_across_lines_is_found() {
        let page: Vec<recto_proto::CharRecord> = "A plan\n10-\nyear horizon".chars().map(|c| recto_proto::CharRecord::new(c as u32, 0)).collect();
        let text = recto_index::normalize(&page, false).text;
        assert_eq!(find_hits(&text, &fold("10-year").0, 5).total, 1, "{text:?}");
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

    /// Failure: the shorter way of folding reads a page differently from the way a hit is placed, so a hit is missed, or found where nothing can be placed. Both must make the same text, on odd spaces and letters that change length and pages that are nearly all ASCII.
    #[test]
    fn the_quick_fold_makes_what_the_full_one_does() {
        let mut quick = String::from("left over from the last page");
        for page in ["", " ", "\n\n", "plain text\nON two LINES", "  leading\ttabs\x0Bvertical\x0Cfeed\r\nend  ", "\x1Cnot space\x1Fin unicode", "İİİ  Æbc\u{A0}non\u{2003}break\u{85}nel", "caffè ﬁnal ß Σίσυφος", "a\u{2028}b\u{2029}c"] {
            fold_into(page, &mut quick);
            assert_eq!(quick, fold(page).0, "{page:?}");
        }
    }
}
