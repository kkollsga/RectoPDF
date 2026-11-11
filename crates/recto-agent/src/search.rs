//! `search`: finds words across a document. A file with a finished corpus is read from that, a pass over text already on disk. Any other is read through one worker per file, in order, and asked to be indexed for next time. Which way a page was read changes nothing about what's said of it, and `route` says which it was.
//!
//! Three kinds of search, over the same text `text` gives. The plain one compares the normaliser's text, lower-cased, with any run of space or line break as one space, so a phrase across a line break still matches. `--regex` runs a pattern over that text as it is, by ripgrep's own engine in this process. `--rank` counts words and puts pages in order: see `recto_index::rank`. Bounded by a limit on hits and a limit on
//! time. Stopping early says where to go on from, so "nothing found" is only ever said of pages really searched.

use crate::index::{FileIndex, Indexing};
use crate::{AgentError, SessionAccess, exit, parse_pages, select};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use recto_index::corpus::{CorpusKey, Damaged, PageKind, ReadyCorpus, StoredPage};
use recto_index::rank::{Collection, Terms, count_terms, first_term, lower, score, terms_of};
use recto_index::{hidden_report, read_page};
use recto_platform::FileStamp;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig};
use serde_json::{Value, json};
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_LIMIT: u32 = 20;
/// Pages in a ranked answer if not told otherwise. Fewer than hits, because each is a page to look at.
const DEFAULT_RANKED: u32 = 10;
const MAX_LIMIT: u32 = 200;
const MAX_QUERY_BYTES: usize = 1024;
/// How long one search may run before it stops and says where to go on from.
const MAX_TIME: Duration = Duration::from_secs(30);
/// How long one search may spend reading again pages that failed when their corpus was built, since each can cost a worker and a page's whole time. Past that they're said to have failed, as they did, and the clean pages behind them are still reached.
pub(crate) const LIVE_REREAD_BUDGET: Duration = Duration::from_secs(5);
/// How long past that a ranked search may go on reading its top pages for their snippets, a page at a time when there's no corpus. Past that, the rest are left out and it says how many.
const SECOND_PASS_TIME: Duration = Duration::from_secs(5);
/// Characters of context on each side of a hit.
const SNIPPET_CONTEXT: usize = 70;
/// Most lines of context on each side of a hit, and most characters shown of each: a page can be one line.
const MAX_CONTEXT: u32 = 20;
const MAX_CONTEXT_LINE: usize = 400;
/// Most characters shown of what a hit matched. A pattern can match a whole page.
const MAX_MATCH_SHOWN: usize = 400;
/// Pages listed as having no text. The count is always whole.
const MAX_LISTED_EMPTY: usize = 200;

/// Which pages to look at, in order: position in the document as edited, which file, and which page there.
pub(crate) struct Plan {
    files: Vec<PlanFile>,
    pub(crate) pages: Vec<(u32, u16, u32)>,
    pub(crate) tab: u32,
    /// The passwords the person typed, for the person's own Find only: never set for an agent's search or find. See [`KeyLookup`].
    pub(crate) keys: Option<KeyLookup>,
}

/// **The person's passwords, for their own Find in a document they opened with one.** Looked up by file, on the thread that reads the pages, as a look can wait on a disk. A file with a password is read page by page through a worker opened with it, and never from or into a corpus: decrypted text is never written anywhere.
pub type KeyLookup = std::sync::Arc<dyn Fn(&std::path::Path) -> Option<std::sync::Arc<recto_core::Password>> + Send + Sync>;

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
        self.path
            .file_name()
            .map_or_else(String::new, |name| recto_core::clean_line(&name.to_string_lossy()))
    }
}

/// What a search was asked for.
pub(crate) struct Asked<'a> {
    pub tab: &'a str,
    pub query: &'a str,
    pub pages: Option<&'a str>,
    pub limit: Option<u32>,
    pub scan: bool,
    pub regex: bool,
    pub rank: bool,
    pub context: Option<u32>,
}

pub(crate) fn search<A: SessionAccess>(access: &A, asked: &Asked<'_>) -> Result<Value, AgentError> {
    if asked.query.len() > MAX_QUERY_BYTES {
        return Err(AgentError::new("bad_query", "That search is too long.", exit::FAILED).then("Search for a few words."));
    }
    if asked.regex && asked.rank {
        return Err(AgentError::new(
            "bad_query",
            "`--regex` and `--rank` are two different searches, so it's one or the other.",
            exit::FAILED,
        )
        .then("`recto help search` says when to use which."));
    }
    let context = asked.context.unwrap_or(0).min(MAX_CONTEXT) as usize;
    let default_limit = if asked.rank { DEFAULT_RANKED } else { DEFAULT_LIMIT };
    let settings = |limit| {
        let deadline = Instant::now() + MAX_TIME;
        Settings {
            limit,
            context,
            deadline,
            draw_until: deadline,
            second_pass_until: deadline + SECOND_PASS_TIME,
            live_budget: LIVE_REREAD_BUDGET,
            indexing: access.indexing(),
            force_scan: asked.scan,
        }
    };
    if asked.rank {
        let terms = terms_of(asked.query);
        if terms.terms.is_empty() {
            return Err(AgentError::new("bad_query", "That search has no words to count.", exit::FAILED)
                .then("Give some words. Ranking goes by words, so punctuation by itself counts for nothing."));
        }
        let plan = plan(access, asked.tab, asked.pages)?;
        let limit = crate::positive_limit(asked.limit, default_limit, MAX_LIMIT)?;
        return run_ranked(&plan, &terms, &settings(limit));
    }
    let (query, mode) = if asked.regex {
        if asked.query.is_empty() {
            return Err(AgentError::new("bad_query", "That search has nothing to look for.", exit::FAILED).then("Give a pattern."));
        }
        (Query::regex(asked.query)?, "regex")
    } else {
        let needle = fold(asked.query).0;
        if needle.trim().is_empty() {
            return Err(AgentError::new("bad_query", "That search has nothing to look for.", exit::FAILED).then("Search for a word or a phrase."));
        }
        (Query::Phrase(needle), "phrase")
    };
    let plan = plan(access, asked.tab, asked.pages)?;
    let limit = crate::positive_limit(asked.limit, default_limit, MAX_LIMIT)?;
    run(&plan, &query, mode, &settings(limit))
}

fn plan<A: SessionAccess>(access: &A, tab: &str, pages: Option<&str>) -> Result<Plan, AgentError> {
    let range = pages.map(|pages| parse_pages(pages, true)).transpose()?;
    access.with(|s| {
        let id = select(s, tab)?;
        plan_of(s, id, pages, range)
    })
}

/// The pages of one tab to look at, as its edits have them, under the session's lock: a few bytes a page.
pub(crate) fn plan_of<L: recto_session::DocLoader>(
    s: &recto_session::Session<L>,
    id: recto_session::TabId,
    pages: Option<&str>,
    range: Option<(u32, u32)>,
) -> Result<Plan, AgentError> {
    {
        let total = s.pages_in(id);
        let (start, len) = range.unwrap_or((0, total));
        // `5-` runs to the end, which is what a search's `next_page` is for.
        let open_ended = pages.is_some_and(|pages| pages.ends_with('-'));
        let len = if open_ended { len.min(total.saturating_sub(start)) } else { len };
        if start >= total.max(1) || u64::from(start) + u64::from(len) > u64::from(total) {
            return Err(AgentError::new("no_such_page", "Those pages aren't in this document.", exit::NOT_FOUND));
        }
        let path = s
            .path_of(id)
            .ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))?;
        let mut files = vec![PlanFile {
            path,
            expected: s.page_edits(id).and_then(|_| s.own_stamp(id)),
            added: false,
        }];
        let list = match s.page_edits(id) {
            None => (start..start.saturating_add(len)).map(|page| (page, 0, page)).collect(),
            Some(edits) => {
                files.extend(edits.added_sources().iter().map(|added| PlanFile {
                    path: added.path.to_path_buf(),
                    expected: added.stamp,
                    added: true,
                }));
                edits
                    .slots()
                    .iter()
                    .enumerate()
                    .skip(start as usize)
                    .take(len as usize)
                    .map(|(position, slot)| (u32::try_from(position).unwrap_or(u32::MAX), slot.source, slot.page))
                    .collect()
            }
        };
        Ok(Plan {
            files,
            pages: list,
            tab: id.0,
            keys: None,
        })
    }
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

/// What's being looked for: a phrase, or a pattern.
enum Query {
    /// Folded as a page is.
    Phrase(String),
    Regex(RegexMatcher),
}

/// How large a pattern may compile to, and how much memory it may use to run. Enough for anything a person or an agent would write, and small enough that a pattern's cost on a page stays in proportion. ripgrep's own defaults are 10 times and 1,000 times these.
const REGEX_SIZE_LIMIT: usize = 1024 * 1024;
const REGEX_DFA_LIMIT: usize = 2 * 1024 * 1024;
const REGEX_NEST_LIMIT: u32 = 64;
/// The longest page a pattern runs over. A deadline gets a look between matches, and the engine's one pass can't be stopped, so this and the size limit are what bound how far past it a search can go: 3.5 s for the worst pattern found. `examples/regex_worst.rs` has the numbers. The longest of 5,872 real pages is 11 KB. Longer ones are listed as not read, and a plain search still reads them.
const MAX_PATTERN_PAGE_BYTES: usize = 128 * 1024;

impl Query {
    /// A pattern, through ripgrep's own engine, run here: no program is started, so nothing like `rg --pre` can run anything. Ignores case like any other search, which `(?-i)` turns off. `^` and `$` go by lines, `.` stops at a line's end and `\s` doesn't, so `force\s+majeure` finds the phrase broken across two lines.
    fn regex(pattern: &str) -> Result<Self, AgentError> {
        let mut builder = RegexMatcherBuilder::new();
        builder
            .case_insensitive(true)
            .multi_line(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_LIMIT)
            .nest_limit(REGEX_NEST_LIMIT);
        builder.build(pattern).map(Self::Regex).map_err(|error| {
            let said = recto_core::clean_line(&error.to_string());
            let said: String = said.chars().take(400).collect();
            if said.contains("size limit") {
                AgentError::new("pattern_too_large", "That pattern compiles to more than a search may use.", exit::FAILED)
                    .then("Try a simpler one: fewer and smaller repeats.")
            } else {
                AgentError::new("bad_pattern", format!("That pattern can't be read: {said}"), exit::FAILED)
                    .then("It's a Rust regex. There's no look-around or back-reference. For plain words, leave out `--regex`.")
            }
        })
    }

    /// Whether this page is past what a pattern may run over.
    fn too_long_for(&self, text: &str) -> bool {
        matches!(self, Self::Regex(_)) && text.len() > MAX_PATTERN_PAGE_BYTES
    }

    /// Whether a page has it, with nothing built. Runs on every page.
    fn on_page(&self, text: &str, folded: &mut String, deadline: Instant) -> OnPage {
        match self {
            Self::Phrase(needle) => {
                fold_into(text, folded);
                if folded.contains(needle.as_str()) { OnPage::Yes } else { OnPage::No }
            }
            Self::Regex(matcher) => match Self::first_match(matcher, text, deadline) {
                (Some(_), _) => OnPage::Yes,
                (None, true) => OnPage::OutOfTime,
                (None, false) => OnPage::No,
            },
        }
    }

    /// A page's hits, for the total, with nothing built. `None` if the time ran out on it.
    fn count(&self, text: &str, folded: &mut String, deadline: Instant) -> Option<u64> {
        match self {
            Self::Phrase(needle) => {
                fold_into(text, folded);
                Some(folded.matches(needle.as_str()).count() as u64)
            }
            Self::Regex(matcher) => {
                let (mut total, mut out_of_time) = (0, false);
                let _ = matcher.find_iter(text.as_bytes(), |found| {
                    total += u64::from(!found.is_empty());
                    out_of_time = Instant::now() >= deadline;
                    !out_of_time
                });
                (!out_of_time).then_some(total)
            }
        }
    }

    /// The first real match, and whether the time ran out looking. It's looked at between matches, empty ones included, which is all a pass allows.
    fn first_match(matcher: &RegexMatcher, text: &str, deadline: Instant) -> (Option<Range<usize>>, bool) {
        let (mut first, mut out_of_time) = (None, false);
        let _ = matcher.find_iter(text.as_bytes(), |found| {
            if !found.is_empty() {
                first = Some(found.start()..found.end());
                return false;
            }
            out_of_time = Instant::now() >= deadline;
            !out_of_time
        });
        (first, out_of_time)
    }

    /// Whether another spelling of a word on the page is this. It's one word, so the time doesn't come into it.
    fn spelled(&self, word: &str) -> bool {
        match self {
            Self::Phrase(needle) => fold(word).0.contains(needle.as_str()),
            Self::Regex(matcher) => Self::first_match(matcher, word, far_future()).0.is_some(),
        }
    }

    fn find(&self, text: &str, room: usize, deadline: Instant) -> PageHits {
        match self {
            Self::Phrase(needle) => find_hits(text, needle, room),
            Self::Regex(matcher) => find_regex_hits(text, matcher, room, deadline),
        }
    }
}

/// Whether a page has what's being looked for.
enum OnPage {
    Yes,
    No,
    OutOfTime,
}

/// A deadline nothing reaches.
fn far_future() -> Instant {
    Instant::now() + Duration::from_secs(365 * 24 * 60 * 60)
}

/// What the pattern finds on a page, in the same shape as [`find_hits`]. A match of nothing at all isn't a hit: `x*` on a page of two million characters would otherwise be that many, each about nothing. Bytes it gives lie on character boundaries unless a pattern asks otherwise with `(?-u)`, and a snippet of those comes out empty rather than wrong.
fn find_regex_hits(text: &str, matcher: &RegexMatcher, room: usize, deadline: Instant) -> PageHits {
    let mut hits = PageHits {
        total: 0,
        shown: Vec::with_capacity(room.min(MAX_LIMIT as usize)),
        out_of_time: false,
    };
    let (mut line, mut counted_to) = (1u32, 0usize);
    let _ = matcher.find_iter(text.as_bytes(), |found| {
        // A pattern can restart a pass for every match, so this is where a page of them gets stopped.
        if Instant::now() >= deadline {
            hits.out_of_time = true;
            return false;
        }
        if found.is_empty() {
            return true;
        }
        hits.total += 1;
        if hits.shown.len() < room {
            let between = text.as_bytes().get(counted_to..found.start()).unwrap_or(&[]);
            line += u32::try_from(between.iter().filter(|byte| **byte == b'\n').count()).unwrap_or(0);
            counted_to = found.start();
            hits.shown.push((line, snippet(text, found.start(), found.end()), found.start()..found.end()));
        }
        true
    });
    hits
}

/// The settings of one search.
#[derive(Clone, Copy)]
pub(crate) struct Settings<'a> {
    pub(crate) limit: usize,
    pub(crate) context: usize,
    /// When it stops reading, and stops counting.
    pub(crate) deadline: Instant,
    /// Past this no page is drawn to look for hidden text, and a page that needed it says it wasn't checked.
    pub(crate) draw_until: Instant,
    /// Past this a ranked search reads no more of its top pages.
    pub(crate) second_pass_until: Instant,
    /// How long it may spend reading again pages that failed when they were built.
    pub(crate) live_budget: Duration,
    pub(crate) indexing: Option<&'a dyn Indexing>,
    pub(crate) force_scan: bool,
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
    /// Every hit, including those past the limit. The other spelling of a word counts as one, as it does in `hits`.
    total: u64,
    /// `total` may be short, because of pages past `next_page` that couldn't be counted.
    total_cut_short: bool,
    /// Pages past `next_page` that couldn't be read, so they aren't in `total`.
    uncounted: u32,
    /// Of `failed`, those too long for a pattern to be run over.
    too_long: u32,
    /// Of `failed`, those a pattern ran out of time on.
    out_of_time: u32,
}

impl Found {
    fn no_text(&mut self, position: u32) {
        self.searched += 1;
        self.empty_count += 1;
        if self.empty.len() < MAX_LISTED_EMPTY {
            self.empty.push(position + 1);
        }
    }

    fn note_tag_chars(&mut self, position: u32) {
        if self.tag_pages.len() < MAX_LISTED_EMPTY {
            self.tag_pages.push(position + 1);
        }
    }

    /// Takes what's on a page. `hidden` is only asked for if the page has something to show, as finding out can mean drawing it.
    fn take(
        &mut self,
        (position, page): (u32, u32),
        view: &PageView<'_>,
        query: &Query,
        settings: &Settings<'_>,
        source: &PlanFile,
        hidden: impl FnOnce() -> HiddenOnPage,
    ) {
        let room = settings.limit - self.hits.len();
        let found = query.find(view.text, room, settings.deadline);
        // What it found stands, and the page isn't said to have been searched.
        if found.out_of_time {
            self.failed.push(position + 1);
            self.out_of_time += 1;
        } else {
            self.searched += 1;
        }
        self.total += found.total as u64;
        if found.total > found.shown.len() {
            self.not_shown.push(json!({ "page": position + 1, "hits": found.total - found.shown.len() }));
        }
        if view.has_tag_chars && found.total > 0 {
            self.note_tag_chars(position);
        }
        let spelled = if found.total > 0 {
            None
        } else {
            view.also_spelled
                .iter()
                .find(|spelled| query.spelled(spelled))
                .filter(|_| self.hits.len() < settings.limit)
        };
        if found.shown.is_empty() && spelled.is_none() {
            return;
        }
        // Only a page with something to show is checked for hidden text: doing it for every page by drawing it costs three to five times as much. `docs/ENGINEERING.md` has the numbers. So a hit always says, and a page never looked at has nothing to say.
        let hidden = hidden();
        for (line, snippet, range) in found.shown {
            let mut hit = json!({ "page": position + 1, "line": line, "page_has_tag_chars": view.has_tag_chars, "hidden_text": hidden.summary, "overlaps_hidden": hidden.overlaps(&range), "untrusted": { "snippet": snippet } });
            add_context(&mut hit, view.text, &range, settings.context);
            say_where_from(&mut hit, page, source);
            self.hits.push(hit);
        }
        if let Some(spelled) = spelled {
            self.total += 1;
            let mut hit = json!({ "page": position + 1, "line": null, "also_spelled": true, "page_has_tag_chars": view.has_tag_chars, "hidden_text": hidden.summary, "untrusted": { "snippet": spelled } });
            say_where_from(&mut hit, page, source);
            self.hits.push(hit);
        }
    }

    /// What's the same in every answer: which pages were looked at, and what couldn't be.
    fn coverage(&self, plan: &Plan, reader: &Reader<'_>) -> Value {
        json!({
            "pages_searched": self.searched,
            "pages_in_scope": plan.pages.len(),
            "pages_with_no_text": self.empty_count,
            "no_text_pages": self.empty,
            "failed_pages": self.failed,
            "hits_not_shown": self.not_shown,
            "pages_with_tag_chars": self.tag_pages,
            "changed_files": reader.changed.iter().map(|file| json!({ "untrusted": { "file": plan.files[usize::from(*file)].label() } })).collect::<Vec<_>>(),
            "next_page": self.next_page,
            "hits_total": (!self.total_cut_short && self.uncounted == 0 && self.failed.is_empty()).then_some(self.total),
            "hits_at_least": self.total,
            "uncounted_pages": self.uncounted,
            "pages_too_long_for_a_pattern": self.too_long,
            "pages_a_pattern_ran_out_of_time_on": self.out_of_time,
            "failed_pages_not_read_again": reader.not_read_again,
        })
    }
}

/// The lines around a hit, if asked for: the document's own, so under `untrusted`, and each cut to a length, since a page can be one line.
fn add_context(hit: &mut Value, text: &str, range: &Range<usize>, lines: usize) {
    if lines == 0 {
        return;
    }
    let cut = |line: &str| line.chars().take(MAX_CONTEXT_LINE).collect::<String>();
    let start_of_line = text.get(..range.start).and_then(|before| before.rfind('\n')).map_or(0, |at| at + 1);
    let end_of_line = text.get(range.end..).and_then(|after| after.find('\n')).map_or(text.len(), |at| range.end + at);
    let mut before: Vec<String> = text.get(..start_of_line).unwrap_or("").lines().rev().take(lines).map(cut).collect();
    before.reverse();
    let after: Vec<String> = text.get(end_of_line..).unwrap_or("").lines().skip(1).take(lines).map(cut).collect();
    hit["untrusted"]["context"] = json!({ "before": before, "after": after });
}

/// One page's text, whichever way it was read.
pub(crate) struct PageView<'a> {
    pub(crate) text: &'a str,
    also_spelled: Vec<&'a str>,
    has_tag_chars: bool,
    /// What a corpus has to say about it, if it came from one.
    stored: Option<StoredPage<'a>>,
}

/// What came of reading a page.
pub(crate) enum Got<'a> {
    Page(PageView<'a>),
    NoText,
    Failed,
    /// There's nothing to be said about it.
    Nothing,
}

/// Where a page's text sits while it's in hand, so a search takes no allocation per page.
#[derive(Default)]
pub(crate) struct Slot {
    buffer: Vec<u8>,
    live: Option<recto_index::Normalized>,
}

/// Reads pages by whichever way each file's can be read, the one way for every kind of search.
pub(crate) struct Reader<'a> {
    plan: &'a Plan,
    indexing: Option<&'a dyn Indexing>,
    force_scan: bool,
    current: Option<(u16, DocRenderer)>,
    corpus_open: Option<(u16, ReadyCorpus)>,
    routes: Vec<FileRoute>,
    not_ready: Vec<FileIndex>,
    /// A file that isn't what its page numbers were recorded from: its pages can't be trusted, and the rest of the document still can.
    changed: Vec<u16>,
    used_index: bool,
    used_scan: bool,
    draw_until: Instant,
    live_budget: Duration,
    /// How much of that's gone.
    live_spent: Duration,
    not_read_again: u32,
    deadline: Instant,
    /// Each file's password, once looked up: `None` not yet looked.
    file_keys: Vec<Option<Option<std::sync::Arc<recto_core::Password>>>>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(plan: &'a Plan, settings: &Settings<'a>) -> Self {
        Self {
            plan,
            indexing: settings.indexing,
            force_scan: settings.force_scan,
            current: None,
            corpus_open: None,
            routes: vec![FileRoute::NotLookedAt; plan.files.len()],
            not_ready: Vec::new(),
            changed: Vec::new(),
            used_index: false,
            used_scan: false,
            draw_until: settings.draw_until,
            live_budget: settings.live_budget,
            live_spent: Duration::ZERO,
            not_read_again: 0,
            deadline: settings.deadline,
            file_keys: vec![None; plan.files.len()],
        }
    }

    /// Works out how a file's pages get read: from its corpus if there's a finished one for the file as it is now, or else page by page, asking for a corpus to be built for next time. A file reached only for the count doesn't change how the answer's hits were read, so it isn't said to have.
    /// The password the person typed for this file, if this is their own Find and they did.
    fn key_of(&mut self, file: u16) -> Option<std::sync::Arc<recto_core::Password>> {
        let keys = self.plan.keys.as_ref()?;
        let known = &mut self.file_keys[usize::from(file)];
        if known.is_none() {
            *known = Some(keys(&self.plan.files[usize::from(file)].path));
        }
        known.clone().flatten()
    }

    fn route_of(&mut self, file: u16, only_counting: bool) -> FileRoute {
        let route = self.routes[usize::from(file)];
        if !matches!(route, FileRoute::NotLookedAt) {
            return route;
        }
        // Opened with a password: read through a worker with it, and no corpus is read or asked for, so its text is never written.
        if self.key_of(file).is_some() {
            self.routes[usize::from(file)] = FileRoute::Scan;
            return FileRoute::Scan;
        }
        let source = &self.plan.files[usize::from(file)];
        let mut said = None;
        let route = match self.indexing.filter(|_| !self.force_scan) {
            None => FileRoute::Scan,
            Some(_) if !source.is_current() => {
                said = Some(FileIndex::Unavailable);
                FileRoute::Scan
            }
            Some(indexing) => match crate::index::file_index(indexing, &source.path, true) {
                (Some(key), FileIndex::Ready(_)) => FileRoute::Index(key),
                (_, state) => {
                    said = Some(state);
                    FileRoute::Scan
                }
            },
        };
        if let Some(said) = said.filter(|_| !only_counting) {
            self.not_ready.push(said);
        }
        self.routes[usize::from(file)] = route;
        route
    }

    /// One page's text. A file that won't be read at all is an error, of the whole search. One bad page isn't: it's `Failed`, so "not found" doesn't cover it.
    pub(crate) fn read<'s>(&mut self, file: u16, page: u32, slot: &'s mut Slot) -> Result<Got<'s>, AgentError> {
        if self.changed.contains(&file) {
            return Ok(Got::Failed);
        }
        let source = &self.plan.files[usize::from(file)];
        let Slot { buffer, live } = slot;
        let mut read_again_live = false;
        if let FileRoute::Index(key) = self.route_of(file, false) {
            let read = match corpus_for(&mut self.corpus_open, file, key, self.indexing) {
                // Pages it hasn't got are pages the file hasn't got: the same as a worker saying so.
                Ok(corpus) if page >= corpus.pages() => {
                    self.used_index = true;
                    return Ok(Got::Failed);
                }
                Ok(corpus) => corpus.page(page, buffer),
                Err(Damaged) => Err(Damaged),
            };
            match read {
                Ok(stored) if stored.kind == PageKind::Text => {
                    self.used_index = true;
                    return Ok(Got::Page(PageView {
                        text: stored.text,
                        also_spelled: stored.also_spelled(),
                        has_tag_chars: stored.has_tag_chars,
                        stored: Some(stored),
                    }));
                }
                Ok(stored) if stored.kind == PageKind::NoText => {
                    self.used_index = true;
                    return Ok(Got::NoText);
                }
                // Time's up for reading these again: each can cost a worker and a page's whole time, and there may be clean pages behind them.
                Ok(_) if self.live_spent >= self.live_budget => {
                    self.used_index = true;
                    self.not_read_again += 1;
                    return Ok(Got::Failed);
                }
                // That's only what happened when it was built, which may have been the moment rather than the page, so it's read now. It can't then end the search: past this, the rest of the corpus is still good.
                Ok(_) => read_again_live = true,
                // This page and the rest of the file are read one by one.
                Err(Damaged) => {
                    self.corpus_open = None;
                    throw_away_corpus(self.indexing, source, key);
                    self.not_ready.push(FileIndex::Failed("damaged".to_string()));
                    self.routes[usize::from(file)] = FileRoute::Scan;
                }
            }
        }
        if !read_again_live {
            return self.read_live(file, page, live, false);
        }
        let began = Instant::now();
        let got = self.read_live(file, page, live, true);
        self.live_spent += began.elapsed();
        got
    }

    /// One page's text, by a worker. `read_again_live` is that its corpus has it as failed, so this can only say it still is.
    fn read_live<'s>(&mut self, file: u16, page: u32, live: &'s mut Option<recto_index::Normalized>, read_again_live: bool) -> Result<Got<'s>, AgentError> {
        let key = self.key_of(file);
        let source = &self.plan.files[usize::from(file)];
        self.used_scan = true;
        let renderer = match worker_for(&mut self.current, file, source, key) {
            Ok(Some(renderer)) => renderer,
            Err(_) if read_again_live => return Ok(Got::Failed),
            Ok(None) | Err(RenderFailure::NotFound) if source.added || source.expected.is_some() => {
                self.changed.push(file);
                return Ok(Got::Failed);
            }
            Ok(None) => return Ok(Got::Nothing),
            Err(failure) => return Err(failure.into()),
        };
        let characters = match renderer.page_characters(page, true, false) {
            Ok(characters) => characters,
            Err(_) if read_again_live => return Ok(Got::Failed),
            Err(
                failure @ (RenderFailure::NeedsPassword
                | RenderFailure::NotFound
                | RenderFailure::NotPdf
                | RenderFailure::Security
                | RenderFailure::OpenFailed { .. }
                | RenderFailure::WorkerMissing
                | RenderFailure::LibraryMissing
                | RenderFailure::WallsDown(_)
                | RenderFailure::StartRefused
                | RenderFailure::DocumentStopped),
            ) => return Err(failure.into()),
            Err(_) => return Ok(Got::Failed),
        };
        if characters.chars.is_empty() {
            return Ok(Got::NoText);
        }
        let normalized: &'s recto_index::Normalized = live.insert(read_page(&characters.chars, &characters.boxes, false).normalized);
        Ok(Got::Page(PageView {
            text: &normalized.text,
            also_spelled: normalized.also_spelled.iter().map(String::as_str).collect(),
            has_tag_chars: normalized.tag_chars_removed > 0,
            stored: None,
        }))
    }

    /// What's known of hidden text on this page: what the build found, or if it couldn't look, what drawing it now says.
    pub(crate) fn hidden(&mut self, file: u16, page: u32, view: &PageView<'_>) -> HiddenOnPage {
        if let Some(stored) = view.stored.as_ref().filter(|stored| stored.hidden_checked) {
            return HiddenOnPage {
                summary: json!({ "checked": true, "flagged": stored.hidden_flagged }),
                spans: byte_spans(view.text, &stored.hidden_spans()),
                partial: stored.spans_dropped,
            };
        }
        // Drawing can cost a page's whole time, for each of up to 200 hits. Past the time it isn't done, which says so.
        if Instant::now() >= self.draw_until {
            return HiddenOnPage {
                summary: json!({ "checked": false, "flagged": null }),
                spans: Vec::new(),
                partial: true,
            };
        }
        let key = self.key_of(file);
        match worker_for(&mut self.current, file, &self.plan.files[usize::from(file)], key) {
            Ok(Some(renderer)) => hidden_on_page(renderer, page, view.text),
            Ok(None) | Err(_) => HiddenOnPage {
                summary: json!({ "checked": false, "flagged": null }),
                spans: Vec::new(),
                partial: true,
            },
        }
    }

    /// Counts a page past the limit, from a corpus, which is the only way that's cheap. `None` if it can't be, which ends the count.
    fn count(&mut self, file: u16, page: u32, query: &Query, slot: &mut Slot, folded: &mut String, found: &mut Found) -> Option<()> {
        let FileRoute::Index(key) = self.route_of(file, true) else { return None };
        let source = &self.plan.files[usize::from(file)];
        let read = match corpus_for(&mut self.corpus_open, file, key, self.indexing) {
            Ok(corpus) if page >= corpus.pages() => {
                found.uncounted += 1;
                return Some(());
            }
            Ok(corpus) => corpus.page(page, &mut slot.buffer),
            Err(Damaged) => Err(Damaged),
        };
        match read {
            Ok(stored) if stored.kind == PageKind::Text && query.too_long_for(stored.text) => found.uncounted += 1,
            Ok(stored) if stored.kind == PageKind::Text => {
                let on_page = query.count(stored.text, folded, self.deadline)?;
                found.total += if on_page > 0 {
                    on_page
                } else {
                    u64::from(stored.also_spelled().iter().any(|spelled| query.spelled(spelled)))
                };
            }
            Ok(stored) if stored.kind == PageKind::Failed => found.uncounted += 1,
            Ok(_) => {}
            Err(Damaged) => {
                throw_away_corpus(self.indexing, source, key);
                return None;
            }
        }
        Some(())
    }

    /// How this search's pages were read, and why, for its answer.
    pub(crate) fn say_how(&self, value: &mut Value) {
        let route = match (self.used_index, self.used_scan) {
            (true, true) => "mixed",
            (true, false) => "index",
            _ => "scan",
        };
        let (state, note) = if self.force_scan {
            ("not_used", "Read the pages one by one, as asked.".to_string())
        } else if self.plan.pages.is_empty() {
            ("not_used", "There were no pages to read.".to_string())
        } else if let Some(indexing) = self.indexing {
            let note = match self.not_ready.first() {
                Some(state) => state.scan_note(indexing.starts_builds()),
                None if self.used_scan && self.used_index => {
                    "Answered from the index, apart from pages it couldn't read when it was built, which were read again just now.".to_string()
                }
                None if self.used_scan => "None of these pages could be read when the index was built, so they were read again just now.".to_string(),
                None => "Answered from the index.".to_string(),
            };
            let note = if self.not_read_again > 0 {
                format!(
                    "{note} {} that couldn't be read when it was built weren't read again, for time, and are listed as failed.",
                    self.not_read_again
                )
            } else {
                note
            };
            (self.not_ready.first().map_or("ready", FileIndex::word), note)
        } else {
            ("off", "This session has no index, so this read the pages one by one.".to_string())
        };
        value["route"] = json!(route);
        value["index"] = json!({ "state": state, "note": note });
    }
}

/// Opens this page's worker if it isn't the one already open. One at a time, whichever file it's for: a document with a thousand added files must not mean a thousand processes. `None` if the file isn't what its page numbers were recorded from, or can't be found.
fn worker_for<'a>(
    current: &'a mut Option<(u16, DocRenderer)>,
    file: u16,
    source: &PlanFile,
    key: Option<std::sync::Arc<recto_core::Password>>,
) -> Result<Option<&'a mut DocRenderer>, RenderFailure> {
    if current.as_ref().is_none_or(|(open_file, _)| *open_file != file) {
        // The old one goes first, so there's never more than one.
        *current = None;
        if !source.is_current() {
            return Ok(None);
        }
        *current = Some((file, DocRenderer::new(&source.path, key, ServiceConfig::discover()?, Policy::default())?));
    }
    Ok(current.as_mut().map(|(_, renderer)| renderer))
}

fn run(plan: &Plan, query: &Query, mode: &str, settings: &Settings<'_>) -> Result<Value, AgentError> {
    let mut reader = Reader::new(plan, settings);
    let mut found = Found::default();
    let mut slot = Slot::default();
    let mut folded = String::new();
    let mut counting_only = false;
    for &(position, file, page) in &plan.pages {
        let out_of_time = Instant::now() >= settings.deadline;
        // Stopping only between pages, so asking again from `next_page` always moves on, however many hits a page has.
        if !counting_only && (found.hits.len() >= settings.limit || out_of_time) {
            found.next_page = Some(position + 1);
            counting_only = true;
        }
        if counting_only {
            // Past the limit, only a corpus makes it cheap to go on, and only for the count.
            if out_of_time || reader.count(file, page, query, &mut slot, &mut folded, &mut found).is_none() {
                found.total_cut_short = true;
                break;
            }
            continue;
        }
        match reader.read(file, page, &mut slot)? {
            Got::Failed => found.failed.push(position + 1),
            Got::NoText => found.no_text(position),
            Got::Nothing => {}
            Got::Page(view) if query.too_long_for(view.text) => {
                found.failed.push(position + 1);
                found.too_long += 1;
            }
            Got::Page(view) => {
                match query.on_page(view.text, &mut folded, settings.deadline) {
                    OnPage::OutOfTime => {
                        found.failed.push(position + 1);
                        found.out_of_time += 1;
                        continue;
                    }
                    OnPage::No if !view.also_spelled.iter().any(|spelled| query.spelled(spelled)) => {
                        found.searched += 1;
                        continue;
                    }
                    OnPage::Yes | OnPage::No => {}
                }
                let source = &plan.files[usize::from(file)];
                found.take((position, page), &view, query, settings, source, || reader.hidden(file, page, &view));
            }
        }
    }
    let mut answer = json!({
        "tab": plan.tab,
        "mode": mode,
        "hits": found.hits,
        "hidden_text_checked": "pages_with_hits",
        "coverage": found.coverage(plan, &reader),
    });
    reader.say_how(&mut answer);
    Ok(answer)
}

/// Best-matching pages first, by BM25 over their words: see `recto_index::rank`. One pass over the pages for the counts, holding numbers for pages with any of the words and no text at all, then the top pages are read once more for their snippets.
fn run_ranked(plan: &Plan, terms: &Terms, settings: &Settings<'_>) -> Result<Value, AgentError> {
    let mut reader = Reader::new(plan, settings);
    let mut found = Found::default();
    let mut slot = Slot::default();
    let words = terms.terms.len();
    let mut counts = vec![0u32; words];
    // Numbers for the pages with any of the words: which page, how long, then one count a word.
    let mut matched: Vec<(u32, u32)> = Vec::new();
    let mut matched_counts: Vec<u32> = Vec::new();
    let mut pages_with = vec![0u32; words];
    let mut collection = Collection { pages: 0, words: 0 };
    for (index, &(position, file, page)) in plan.pages.iter().enumerate() {
        if Instant::now() >= settings.deadline {
            found.next_page = Some(position + 1);
            break;
        }
        match reader.read(file, page, &mut slot)? {
            Got::Failed => found.failed.push(position + 1),
            Got::NoText => found.no_text(position),
            Got::Nothing => {}
            Got::Page(view) => {
                found.searched += 1;
                counts.fill(0);
                let length = count_terms(view.text, &terms.terms, &mut counts);
                for spelled in &view.also_spelled {
                    count_terms(spelled, &terms.terms, &mut counts);
                }
                collection.pages += 1;
                collection.words += u64::from(length);
                if counts.iter().any(|count| *count > 0) {
                    for (with, count) in pages_with.iter_mut().zip(&counts) {
                        *with += u32::from(*count > 0);
                    }
                    matched.push((u32::try_from(index).unwrap_or(u32::MAX), length));
                    matched_counts.extend_from_slice(&counts);
                    if view.has_tag_chars {
                        found.note_tag_chars(position);
                    }
                }
            }
        }
    }
    let mut scored: Vec<(f64, usize)> = matched
        .iter()
        .enumerate()
        .map(|(at, (_, length))| (score(collection, *length, &matched_counts[at * words..(at + 1) * words], &pages_with), at))
        .collect();
    // Highest first, and by page among equals, so the same search gives the same order every time.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left_out = 0;
    for (shown, (value, at)) in scored.iter().take(settings.limit).enumerate() {
        // With no corpus, each of these is a page to read again.
        if Instant::now() >= settings.second_pass_until {
            left_out = scored.len().min(settings.limit) - shown;
            break;
        }
        let (index, _) = matched[*at];
        let (position, file, page) = plan.pages[index as usize];
        let page_counts = &matched_counts[*at * words..(*at + 1) * words];
        let matched_terms: Vec<&str> = terms
            .terms
            .iter()
            .zip(page_counts)
            .filter(|(_, count)| **count > 0)
            .map(|(term, _)| term.as_str())
            .collect();
        let term_counts: Vec<u32> = page_counts.iter().copied().filter(|count| *count > 0).collect();
        let Got::Page(view) = reader.read(file, page, &mut slot)? else {
            // It read a moment ago, and doesn't now, so there's nothing to show of it.
            found.failed.push(position + 1);
            continue;
        };
        let hidden = reader.hidden(file, page, &view);
        let place = first_term(view.text, &terms.terms);
        let mut hit = json!({ "page": position + 1, "score": (value * 10_000.0).round() / 10_000.0, "terms": matched_terms, "term_counts": term_counts, "page_has_tag_chars": view.has_tag_chars, "hidden_text": hidden.summary });
        match &place {
            Some(range) => {
                hit["line"] = json!(1 + view.text.as_bytes()[..range.start].iter().filter(|byte| **byte == b'\n').count());
                hit["overlaps_hidden"] = json!(hidden.overlaps(range));
                hit["untrusted"] = json!({ "snippet": snippet(view.text, range.start, range.end) });
                add_context(&mut hit, view.text, range, settings.context);
            }
            // Only another spelling of a word has it.
            None => {
                hit["line"] = Value::Null;
                hit["also_spelled"] = json!(true);
                hit["untrusted"] = json!({ "snippet": view.also_spelled.iter().find(|word| first_term(word, &terms.terms).is_some()) });
            }
        }
        say_where_from(&mut hit, page, &plan.files[usize::from(file)]);
        found.hits.push(hit);
    }
    found.total_cut_short = true;
    let mut coverage = found.coverage(plan, &reader);
    coverage["pages_with_these_words"] = json!(matched.len());
    coverage["top_pages_left_out_for_time"] = json!(left_out);
    // There's no count of hits here to be at least anything.
    if let Some(coverage) = coverage.as_object_mut() {
        coverage.remove("hits_at_least");
    }
    let mut answer = json!({
        "tab": plan.tab,
        "mode": "ranked",
        "ranked_note": RANKED_NOTE,
        "terms": terms.terms,
        "terms_dropped": terms.dropped,
        "hits": found.hits,
        "hidden_text_checked": "pages_with_hits",
        "coverage": coverage,
    });
    reader.say_how(&mut answer);
    Ok(answer)
}

/// What a ranked answer is, and isn't.
const RANKED_NOTE: &str = "Pages in order of a count of these words, rarest words and shorter pages counting most (BM25). One line from each. It isn't every hit, and a score says nothing about whether a page is right, or even any use: a document can repeat a word to come first.";

/// Says where a hit's page really comes from, on every hit, so an agent reading these needn't know what it means for a field to be missing.
fn say_where_from(hit: &mut Value, page: u32, source: &PlanFile) {
    hit["source_page"] = json!(page + 1);
    hit["from_added_file"] = json!(source.added);
    if source.added {
        hit["untrusted"]["added_file"] = json!(source.label());
    }
}

/// What drawing a page says about hidden text.
pub(crate) struct HiddenOnPage {
    summary: Value,
    /// Where it is in the page's text, in bytes, in order. Empty if there's none, or if they couldn't be placed.
    spans: Vec<Range<usize>>,
    /// Some of it isn't in `spans`, so a hit that overlaps none of them may still overlap some.
    partial: bool,
}

impl HiddenOnPage {
    /// Whether a hit's words include hidden text: `null` if that can't be said, which isn't the same as no.
    pub(crate) fn overlaps(&self, hit: &Range<usize>) -> Option<bool> {
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
            HiddenOnPage {
                summary,
                spans,
                partial: !report.checked || !same_text || reading.hidden_spans_dropped > 0,
            }
        }
        Err(_) => HiddenOnPage {
            summary: json!({ "checked": false, "flagged": null }),
            spans: Vec::new(),
            partial: true,
        },
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
pub(crate) fn fold_into(text: &str, folded: &mut String) {
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
        folded.extend(lower(c));
    }
}

/// Lower-cases and joins whitespace, and notes where each byte of the result came from. Returns the folded text and that map.
pub(crate) fn fold(text: &str) -> (String, Vec<u32>) {
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
        for lower in lower(c) {
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
    /// The time ran out partway down the page, so there may be more.
    out_of_time: bool,
}

/// Finds the folded needle in the text. Snippets are made only for the first `room`: a hostile page of two million characters searched for one letter would otherwise make two million strings before the limit was looked at.
fn find_hits(text: &str, needle: &str, room: usize) -> PageHits {
    let (folded, sources) = fold(text);
    let mut hits = PageHits {
        total: 0,
        shown: Vec::with_capacity(room.min(MAX_LIMIT as usize)),
        out_of_time: false,
    };
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
    // A pattern can match a whole page.
    let middle: String = middle.chars().take(MAX_MATCH_SHOWN).collect();
    let joined: String = before.iter().rev().collect::<String>() + &middle + &after;
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
        let hidden = HiddenOnPage {
            summary: Value::Null,
            spans,
            partial: false,
        };
        let hit = |word: &str| find_hits(text, word, 1).shown[0].2.clone();
        assert_eq!(hidden.overlaps(&hit("ignore")), Some(true));
        assert_eq!(hidden.overlaps(&hit("this but")), Some(true), "a phrase that starts in it");
        assert_eq!(hidden.overlaps(&hit("caffè ignore")), Some(true), "a phrase that ends in it");
        assert_eq!(hidden.overlaps(&hit("keep")), Some(false));
        assert_eq!(hidden.overlaps(&hit("caffè")), Some(false));
        let not_known = HiddenOnPage {
            summary: Value::Null,
            spans: Vec::new(),
            partial: true,
        };
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
        for page in [
            "",
            " ",
            "\n\n",
            "plain text\nON two LINES",
            "  leading\ttabs\x0Bvertical\x0Cfeed\r\nend  ",
            "\x1Cnot space\x1Fin unicode",
            "İİİ  Æbc\u{A0}non\u{2003}break\u{85}nel",
            "caffè ﬁnal ß Σίσυφος",
            "a\u{2028}b\u{2029}c",
        ] {
            fold_into(page, &mut quick);
            assert_eq!(quick, fold(page).0, "{page:?}");
        }
    }

    fn pattern(text: &str) -> RegexMatcher {
        let Ok(Query::Regex(matcher)) = Query::regex(text) else { panic!("{text}") };
        matcher
    }

    /// With all the time there is.
    fn find_regex(text: &str, matcher: &RegexMatcher, room: usize) -> PageHits {
        find_regex_hits(text, matcher, room, far_future())
    }

    /// Failure: a pattern's hits are placed on the wrong line, a phrase across a line break isn't found, or a match of nothing counts as a hit, so `x*` is two million hits about nothing on a hostile page.
    #[test]
    fn a_pattern_finds_what_it_says_on_the_lines_it_says() {
        let page = "first line\nThe Force\nMajeure clause\nFORCE majeure again";
        let found = find_regex(page, &pattern(r"force\s+majeure"), 10);
        assert_eq!(found.total, 2, "across a line break, in any case");
        assert_eq!((found.shown[0].0, found.shown[1].0), (2, 4));
        assert_eq!(
            &page[found.shown[0].2.clone()],
            "Force\nMajeure",
            "and where it is, as hidden-text marks need it"
        );
        assert_eq!(find_regex(page, &pattern(r"(?-i)force"), 10).total, 0, "case is one flag away");
        assert_eq!(find_regex(page, &pattern(r"^the"), 10).total, 1, "`^` goes by lines");
        assert_eq!(find_regex(page, &pattern(r"line.The"), 10).total, 0, "`.` stops at a line's end");
        let nothing = find_regex(page, &pattern(r"x*"), 10);
        assert_eq!((nothing.total, nothing.shown.len()), (0, 0), "matching nothing isn't a hit");
        let some = find_regex(page, &pattern(r"[a-z]+"), 2);
        assert!(some.total > 2 && some.shown.len() == 2, "past the room they're counted and not built");
    }

    /// Failure: the two ways of counting a pattern's hits disagree, so a total is wrong, or the check that a page has any says no when it does.
    #[test]
    fn counting_a_pattern_agrees_with_finding_it() {
        for (page, text) in [("caffè İ line\nand more\n", r"\w+"), ("", "x"), ("x", "x"), ("xxxx", "x*"), ("a\nb", "(?s).")] {
            let query = Query::regex(text).unwrap();
            let mut folded = String::new();
            let found = query.find(page, usize::MAX, far_future());
            assert_eq!(query.count(page, &mut folded, far_future()), Some(found.total as u64), "{page:?} {text}");
            assert_eq!(
                matches!(query.on_page(page, &mut folded, far_future()), OnPage::Yes),
                found.total > 0,
                "{page:?} {text}"
            );
        }
    }

    /// Failure: a hostile pattern takes the session's memory or time to compile, or panics, or a pattern that's just wrong reads as "nothing found". Each is refused, with its own code, and with no state left behind.
    #[test]
    fn a_hostile_pattern_is_refused_plainly() {
        let code_of = |text: &str| Query::regex(text).err().map(|error| (error.code, error.exit));
        assert_eq!(code_of("(unclosed"), Some(("bad_pattern", exit::FAILED)));
        assert_eq!(code_of(r"(?<=look)behind"), Some(("bad_pattern", exit::FAILED)), "no look-around");
        assert_eq!(code_of(r"(a)\1"), Some(("bad_pattern", exit::FAILED)), "no back-references");
        assert_eq!(
            code_of(&format!("{}x{}", "(".repeat(100), ")".repeat(100))),
            Some(("bad_pattern", exit::FAILED)),
            "nested past the limit"
        );
        assert_eq!(code_of(r"(?:\w{1000}){1000}"), Some(("pattern_too_large", exit::FAILED)));
        assert_eq!(code_of(r"\w{1,100000}"), Some(("pattern_too_large", exit::FAILED)));
        let started = Instant::now();
        assert_eq!(code_of(&"(?:a{1000}){1000}".repeat(30)).map(|code| code.0), Some("pattern_too_large"));
        assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
    }

    /// Failure: a pattern that matches a whole page makes a snippet as large as the page, in every hit.
    #[test]
    fn a_pattern_matching_a_whole_page_is_shown_in_part() {
        let page = "word ".repeat(100_000);
        let found = find_regex(&page, &pattern(r"(?s).+"), 1);
        assert_eq!(found.total, 1);
        assert!(found.shown[0].1.len() < 2 * MAX_MATCH_SHOWN, "{}", found.shown[0].1.len());
        assert!(snippet(&page, 0, page.len()).len() < 2 * MAX_MATCH_SHOWN);
    }

    /// Failure: context lines are off by one, leave out the hit's own line's neighbours, take a page's single long line whole, or run past either end of the page.
    #[test]
    fn context_gives_the_lines_around_a_hit() {
        let page = "one\ntwo\nthree\nfour\nfive";
        let at = page.find("three").unwrap();
        let context = |lines: usize, range: Range<usize>| {
            let mut hit = json!({ "untrusted": {} });
            add_context(&mut hit, page, &range, lines);
            hit["untrusted"]["context"].clone()
        };
        assert_eq!(context(1, at..at + 5), json!({ "before": ["two"], "after": ["four"] }));
        assert_eq!(
            context(9, at..at + 5),
            json!({ "before": ["one", "two"], "after": ["four", "five"] }),
            "past either end"
        );
        assert_eq!(context(1, 0..3), json!({ "before": [], "after": ["two"] }));
        assert_eq!(context(1, page.len() - 4..page.len()), json!({ "before": ["four"], "after": [] }));
        assert_eq!(
            context(1, at..page.find("four").unwrap() + 4),
            json!({ "before": ["two"], "after": ["five"] }),
            "a hit over two lines has its context outside both"
        );
        let mut none = json!({ "untrusted": {} });
        add_context(&mut none, page, &(0..1), 0);
        assert_eq!(none["untrusted"].get("context"), None, "it costs nothing unless asked for");
        let long = format!("start\n{}\nend", "y".repeat(10_000));
        let mut hit = json!({ "untrusted": {} });
        add_context(&mut hit, &long, &(0..5), 1);
        assert_eq!(hit["untrusted"]["context"]["after"][0].as_str().unwrap().chars().count(), MAX_CONTEXT_LINE);
    }

    /// Failure: a change here starts a program to search with. `rg --pre` runs text that comes from a PDF, which is why ripgrep's engine is run in this process and nothing is ever started. The only process a search starts is the render worker, in `recto-render-service`.
    #[test]
    fn a_search_starts_no_program() {
        let source = include_str!("search.rs");
        for started in [concat!("process", "::Command"), concat!("Command", "::new"), concat!("\"r", "g\"")] {
            assert!(!source.contains(started), "{started}");
        }
    }

    /// Failure: a pattern that matches over and over restarts a pass for every match, so one page of `a` holds its job thread for minutes or hours by the review's arithmetic, 37 s measured for the second of these at 512 KiB, with the time cap only applying between pages. These stop at the deadline: a look comes between matches, empty ones included.
    #[test]
    fn a_pattern_that_matches_over_and_over_stops_at_the_deadline() {
        let page = "a".repeat(MAX_PATTERN_PAGE_BYTES);
        for text in [r"(?:a|b|c|d|e|f|g|h|i|j){1,3000}\d|.", r"\w+\d|\w", r"(a|aa|aaa|aaaa){1,100}\d|x*"] {
            let query = Query::regex(text).unwrap();
            let started = Instant::now();
            let deadline = started + Duration::from_millis(50);
            let found = query.find(&page, 5, deadline);
            assert!(found.out_of_time, "{text}");
            assert!(found.total < page.len(), "{text} went to the end of the page");
            assert_eq!(query.count(&page, &mut String::new(), Instant::now()), None, "{text}");
            // Far above one pass on any machine, debug build included, and far below a pass per match.
            assert!(started.elapsed() < Duration::from_secs(30), "{text}: {:?}", started.elapsed());
        }
        // Empty matches are all that comes back from this one, so finding whether it's there at all has to stop too.
        let query = Query::regex(r"(a|aa|aaa|aaaa){1,100}\d|x*").unwrap();
        assert!(matches!(query.on_page(&page, &mut String::new(), Instant::now()), OnPage::OutOfTime));
        assert!(!Query::Phrase("a".to_string()).too_long_for(&page), "and nothing changes for plain words");
        assert!(query.too_long_for(&"a".repeat(MAX_PATTERN_PAGE_BYTES + 1)));
        assert!(!query.too_long_for(&page), "up to the limit it runs");
    }

    /// A corpus of these pages, and a plan to read all of them. A page with no text failed when it was built. The file is whatever's given.
    struct Fixture {
        _folder: recto_testpdf::TempDir,
        plan: Plan,
        indexing: crate::index::ReadOnlyCorpus,
    }

    struct Pages(Vec<Option<(String, bool)>>);

    impl recto_index::corpus::PageFeed for Pages {
        fn page_count(&mut self) -> Result<u32, recto_index::corpus::FeedError> {
            Ok(self.0.len() as u32)
        }
        fn read(&mut self, page: u32) -> Result<recto_index::corpus::FedPage, recto_index::corpus::FeedError> {
            match &self.0[page as usize] {
                Some((text, checked)) => Ok(recto_index::corpus::FedPage {
                    chars: text.chars().map(|c| recto_proto::CharRecord::new(c as u32, 0)).collect(),
                    boxes: Vec::new(),
                    hidden_checked: *checked,
                }),
                None => Err(recto_index::corpus::FeedError::Page(recto_index::corpus::FailReason::Stopped)),
            }
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }

    fn fixture(name: &str, file: &[u8], pages: Vec<Option<(String, bool)>>) -> Fixture {
        let folder = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), &format!("recto-search-{name}-"));
        recto_platform::local::ensure_private_dir(&folder).unwrap();
        let path = folder.join("doc.pdf");
        std::fs::write(&path, file).unwrap();
        let root = folder.join("corpus.noindex");
        let key = CorpusKey::of_file(&path).unwrap();
        let count = pages.len() as u32;
        recto_index::corpus::build(
            &root,
            key,
            &mut Pages(pages),
            &recto_index::corpus::DocMeta::default(),
            &recto_index::corpus::BuildLimits::default(),
        )
        .unwrap();
        Fixture {
            _folder: folder,
            plan: Plan {
                files: vec![PlanFile {
                    path,
                    expected: None,
                    added: false,
                }],
                pages: (0..count).map(|page| (page, 0, page)).collect(),
                tab: 1,
                keys: None,
            },
            indexing: crate::index::ReadOnlyCorpus(root),
        }
    }

    fn settings(fixture: &Fixture) -> Settings<'_> {
        let far = far_future();
        Settings {
            limit: 20,
            context: 0,
            deadline: far,
            draw_until: far,
            second_pass_until: far,
            live_budget: Duration::from_secs(5),
            indexing: Some(&fixture.indexing),
            force_scan: false,
        }
    }

    /// Failure: a hostile pattern on one page uses up the search, and the answer reads as "nothing found" for it, or says a total, or never says where to go on from. The page is listed, and that's said to be why.
    #[test]
    fn a_page_a_pattern_ran_out_of_time_on_is_listed() {
        let page = "a".repeat(MAX_PATTERN_PAGE_BYTES);
        let fixture = fixture(
            "pattern-time",
            b"a file",
            vec![
                Some((page, true)),
                Some(("a clean page".to_string(), true)),
                Some(("another".to_string(), true)),
            ],
        );
        let started = Instant::now();
        // Long enough to reach the page on a Mac under any load, and far short of the many seconds this page takes.
        let settings = Settings {
            deadline: started + Duration::from_secs(3),
            ..settings(&fixture)
        };
        let answer = run(&fixture.plan, &Query::regex(r"\w+\d|\w").unwrap(), "regex", &settings).unwrap();
        assert!(started.elapsed() < Duration::from_secs(30), "{:?}", started.elapsed());
        let coverage = &answer["coverage"];
        assert_eq!(coverage["failed_pages"], json!([1]), "{answer}");
        assert_eq!(coverage["pages_a_pattern_ran_out_of_time_on"], 1, "{answer}");
        assert_eq!(coverage["hits_total"], Value::Null, "{answer}");
        assert_eq!(coverage["next_page"], 2, "{answer}");
        assert_eq!(coverage["pages_searched"], 0, "{answer}");
    }

    /// Failure: every failed page in a corpus is read again by a worker, each costing up to a page's whole time, so a file of many bad pages uses up every search on the first few, with clean pages behind them never reached. Past a budget they're said to have failed, as they did, with no worker, and the clean ones are read.
    #[test]
    fn failed_pages_past_the_budget_arent_read_again() {
        let mut pages: Vec<Option<(String, bool)>> = vec![None; 300];
        pages.push(Some(("the clean page at the end says needleword".to_string(), true)));
        let fixture = fixture("budget", b"not a pdf at all", pages);
        let settings = Settings {
            live_budget: Duration::ZERO,
            ..settings(&fixture)
        };
        let started = Instant::now();
        let answer = run(&fixture.plan, &Query::Phrase("needleword".to_string()), "phrase", &settings).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert_eq!(answer["route"], "index", "no worker was to be started: {answer}");
        assert_eq!(answer["coverage"]["failed_pages_not_read_again"], 300, "{answer}");
        assert_eq!(answer["coverage"]["failed_pages"].as_array().unwrap().len(), 300, "{answer}");
        assert_eq!(answer["hits"][0]["page"], 301, "the clean page behind them was reached: {answer}");
        assert!(answer["index"]["note"].as_str().unwrap().contains("weren't read again"), "{answer}");
        // With the budget it gets, the first goes live, and it runs out.
        let settings = Settings {
            live_budget: Duration::from_nanos(1),
            ..settings
        };
        let answer = run(&fixture.plan, &Query::Phrase("needleword".to_string()), "phrase", &settings).unwrap();
        assert_eq!(answer["route"], "mixed", "{answer}");
        assert_eq!(answer["coverage"]["failed_pages_not_read_again"], 299, "{answer}");
        assert_eq!(answer["hits"][0]["page"], 301, "{answer}");
        assert!(
            answer["index"]["note"].as_str().unwrap().contains("read again just now"),
            "the note says what happened, on any route that read one: {answer}"
        );
    }

    /// Failure: a ranked search's second pass, or any search with many hits, draws each page the build couldn't check, 5 s a page, up to 1,000 s past the 30 s cap with `--limit 200`. Past the time it says `checked: false`, and past a little more, a ranked search leaves out the pages it hasn't read and says how many.
    #[test]
    fn past_the_time_nothing_more_is_drawn_or_read() {
        let pdf = recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["a word on a page"]))]);
        let fixture = fixture("draw", &pdf, vec![Some(("a word on a page".to_string(), false))]);
        let terms = terms_of("word");
        let drawn = run_ranked(&fixture.plan, &terms, &settings(&fixture)).unwrap();
        assert_eq!(drawn["hits"][0]["hidden_text"]["checked"], true, "with time to spare, it's drawn: {drawn}");
        let past = Instant::now() - Duration::from_millis(1);
        let not = Settings {
            draw_until: past,
            ..settings(&fixture)
        };
        let answer = run_ranked(&fixture.plan, &terms, &not).unwrap();
        assert_eq!(answer["hits"][0]["hidden_text"], json!({ "checked": false, "flagged": null }), "{answer}");
        let phrase = run(&fixture.plan, &Query::Phrase("word".to_string()), "phrase", &not).unwrap();
        assert_eq!(phrase["hits"][0]["hidden_text"]["checked"], false, "{phrase}");
        let none = Settings {
            second_pass_until: past,
            ..settings(&fixture)
        };
        let answer = run_ranked(&fixture.plan, &terms, &none).unwrap();
        assert_eq!(
            (
                answer["hits"].as_array().unwrap().len(),
                answer["coverage"]["top_pages_left_out_for_time"].as_u64()
            ),
            (0, Some(1)),
            "{answer}"
        );
    }

    /// Failure: Greek's two lower-case sigmas and Turkish's dotted capital keep a plain search or a pattern from finding the word as printed in another case. A pattern goes by the engine's folding, which has both sigmas and not the dotted `İ`, and that's said in `docs/ENGINEERING.md`.
    #[test]
    fn every_search_finds_greek_and_turkish_in_either_case() {
        for (query, page) in [
            ("ΟΔΟΣ", "στην ΟΔΟΣ εδώ"),
            ("ΟΔΟΣ", "στην οδος εδώ"),
            ("οδος", "στην ΟΔΟΣ εδώ"),
            ("İstanbul", "in İSTANBUL here"),
            ("istanbul", "in İstanbul here"),
            ("İSTANBUL", "in istanbul here"),
        ] {
            assert_eq!(find_hits(page, &fold(query).0, 5).total, 1, "{query} on {page}");
            let mut folded = String::new();
            fold_into(page, &mut folded);
            assert!(folded.contains(&fold(query).0), "{query} on {page}");
        }
        for (text, page) in [
            ("ΟΔΟΣ", "στην οδος εδώ"),
            ("οδος", "στην ΟΔΟΣ εδώ"),
            ("οδοσ", "στην οδος εδώ"),
            ("İSTANBUL", "in İstanbul"),
        ] {
            assert_eq!(find_regex(page, &pattern(text), 5).total, 1, "{text} on {page}");
        }
    }
}
