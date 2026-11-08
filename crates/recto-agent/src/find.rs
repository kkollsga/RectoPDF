//! **Find**: the reader's Find in the sidebar, and `recto find`, which is the same thing for an agent. It searches the document being read (in a comparison, the pane being read) by the same reading of its pages as `search`: from its corpus if that's built, and page by page through one worker if not, asking for a corpus for next time. So what Find finds is what `search` finds.
//!
//! **What's held.** The hits of one query, in reading order, at most [`MAX_HITS`], each with a short piece of its line either side ([`BEFORE`], [`AFTER`]) and its place as characters of the page's text, which are what a selection's places are, so a hit can be selected and highlighted. Past the cap they're counted and not kept. A new query replaces the last one, and stops it if it's still running. **What's said about it,** in words, is here too: the line under the field and the page rows, so the host draws them and decides none of it.
//!
//! **What runs.** [`run`] is one pass, a page at a time, looking at `stop` between pages, so a new query ends the old one within a page. Whoever holds the session runs it off every thread that mustn't wait: `recto_control::finding`. It's bounded by [`MAX_TIME`], after which it says how far it got.

use crate::index::Indexing;
use crate::search::{self, Got, Plan, Reader, Settings, Slot};
use crate::{AgentError, exit};
use recto_session::{DocLoader, Session, TabId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Hits kept for one query. More than anyone reads in a list; past it they're counted. About 200 B each at most, so 200 KiB.
pub const MAX_HITS: usize = 1000;
/// How long one query may run. NASA's 383 pages take about 1.2 s page by page and tens of milliseconds from the index; this is for a file of thousands of pages with no index yet.
pub const MAX_TIME: Duration = Duration::from_secs(60);
/// The longest query, in bytes. Longer is refused, as `search` refuses it.
pub const MAX_QUERY_BYTES: usize = 1024;
/// Characters of a hit's line shown before it, and after it. Short before, so the match sits near the start of a row and is seen in a narrow sidebar.
pub const BEFORE: usize = 24;
pub const AFTER: usize = 64;
/// Most characters shown of what matched: a phrase can be long.
const MATCHED: usize = 64;
/// How long a query may spend drawing pages read live to check their hits for hidden text.
const HIDDEN_CHECK_TIME: Duration = Duration::from_secs(10);
/// What's put where a piece was cut.
const CUT: &str = "…";

/// One hit, as a page's characters: `start..end` of the text `recto text` gives, which is what a selection's places count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The page's position in the document as it's edited, from 0.
    pub page: u32,
    pub start: u32,
    pub end: u32,
    /// From the document, so they're only ever drawn as text. Whitespace is one space, and a cut end says so.
    pub before: Box<str>,
    pub matched: Box<str>,
    pub after: Box<str>,
    /// Its words include text that looks hidden, by the check `search` makes, so there's nothing to see where it is. Listed, and never gone to.
    pub hidden: bool,
}

/// What a query's hits are about: the tab, its pages as they were arranged, and which version of its file. Any of these changing makes the hits about something else, so they're found again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Basis {
    pub tab: u32,
    pub pages: u32,
    pub stamp: Option<recto_platform::FileStamp>,
    pub changes: u64,
}

/// A tab's basis now, under the session's lock: a few numbers.
pub fn basis<L: DocLoader>(session: &Session<L>, id: TabId) -> Basis {
    Basis { tab: id.0, pages: session.pages_in(id), stamp: session.own_stamp(id), changes: session.page_edits(id).map_or(0, recto_session::edit::PageEdits::changes) }
}

/// What one query's pages are: an opaque list of which file and page each position is.
pub struct FindPlan {
    plan: Plan,
    basis: Basis,
}

impl FindPlan {
    pub fn tab(&self) -> u32 {
        self.plan.tab
    }

    pub fn pages(&self) -> u32 {
        u32::try_from(self.plan.pages.len()).unwrap_or(u32::MAX)
    }

    pub fn basis(&self) -> Basis {
        self.basis
    }
}

/// A tab's pages to look through, as its edits have them, under the session's lock. A few bytes a page.
pub fn plan<L: DocLoader>(session: &Session<L>, id: TabId) -> Result<FindPlan, AgentError> {
    search::plan_of(session, id, None, None).map(|plan| FindPlan { plan, basis: basis(session, id) })
}

/// What's looked for, folded as a page is: lower case, any run of spaces and line breaks one space. `None` if there's nothing to look for.
pub fn needle(query: &str) -> Option<String> {
    if query.len() > MAX_QUERY_BYTES {
        return None;
    }
    let folded = search::fold(query).0;
    (!folded.trim().is_empty()).then_some(folded)
}

/// How far a query has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub pages_done: u32,
    pub pages: u32,
    /// Pages that couldn't be read, so finding nothing says nothing about them.
    pub failed: u32,
    pub no_text: u32,
    /// Every hit, kept or not.
    pub total: u64,
    /// Pages with a hit.
    pub pages_with_hits: u32,
}

/// How a query ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    Finished,
    /// A new query, or Find closing, stopped it.
    Stopped,
    /// [`MAX_TIME`] ran out, at this page position.
    OutOfTime(u32),
    /// The file couldn't be read at all, in Rust's words.
    Failed(String),
}

/// Runs one query over a plan, a page at a time, giving each page's hits to `on_page` as they're found, with how far it's got. No more than [`MAX_HITS`] are made in all; past that they're counted. Looks at `stop` between pages.
pub fn run(plan: &FindPlan, needle: &str, indexing: Option<&dyn Indexing>, stop: &AtomicBool, on_page: &mut dyn FnMut(Vec<Hit>, &Progress)) -> (End, Progress) {
    let deadline = Instant::now() + MAX_TIME;
    // Drawing a page read live, to check a hit's words aren't hidden, stops after this, as `search`'s does: past it, hits are listed as not hidden, unchecked.
    let draw_until = Instant::now() + HIDDEN_CHECK_TIME;
    let settings = Settings { limit: MAX_HITS, context: 0, deadline, draw_until, second_pass_until: deadline, live_budget: search::LIVE_REREAD_BUDGET, indexing, force_scan: false };
    let plan = &plan.plan;
    let mut reader = Reader::new(plan, &settings);
    let mut slot = Slot::default();
    let mut folded = String::new();
    let mut progress = Progress { pages: u32::try_from(plan.pages.len()).unwrap_or(u32::MAX), ..Progress::default() };
    let mut kept = 0usize;
    for &(position, file, page) in &plan.pages {
        if stop.load(Ordering::Relaxed) {
            return (End::Stopped, progress);
        }
        if Instant::now() >= deadline {
            return (End::OutOfTime(position), progress);
        }
        let got = match reader.read(file, page, &mut slot) {
            Ok(got) => got,
            Err(error) => return (End::Failed(error.message), progress),
        };
        progress.pages_done += 1;
        let mut hits = Vec::new();
        match got {
            Got::Failed => progress.failed += 1,
            Got::NoText => progress.no_text += 1,
            Got::Nothing => {}
            Got::Page(view) => {
                search::fold_into(view.text, &mut folded);
                if folded.contains(needle) {
                    // Only a page with hits kept is checked, from its corpus if it has one and otherwise by drawing it, as `search` checks the pages it shows.
                    let hidden = (kept < MAX_HITS).then(|| reader.hidden(file, page, &view));
                    let is_hidden = |range: &std::ops::Range<usize>| hidden.as_ref().is_some_and(|hidden| hidden.overlaps(range) == Some(true));
                    let (total, made) = page_hits(view.text, needle, position, MAX_HITS - kept, &is_hidden);
                    progress.total += total as u64;
                    progress.pages_with_hits += u32::from(total > 0);
                    kept += made.len();
                    hits = made;
                }
            }
        }
        // Every page, hits or none, so how far it's got can be said. Whoever holds the results says it no more often than they like.
        on_page(hits, &progress);
    }
    (End::Finished, progress)
}

/// One page's hits: how many in all, and the first `room` made into [`Hit`]s.
pub(crate) fn page_hits(text: &str, needle: &str, page: u32, room: usize, is_hidden: &dyn Fn(&std::ops::Range<usize>) -> bool) -> (usize, Vec<Hit>) {
    let (folded, sources) = search::fold(text);
    let mut hits = Vec::new();
    let mut total = 0;
    // Characters before a byte, counted once along the page.
    let (mut counted_bytes, mut counted_chars) = (0usize, 0u32);
    let mut chars_to = |byte: usize| -> u32 {
        let byte = byte.min(text.len());
        if byte >= counted_bytes {
            counted_chars += u32::try_from(text.get(counted_bytes..byte).map_or(0, |piece| piece.chars().count())).unwrap_or(u32::MAX);
            counted_bytes = byte;
        }
        counted_chars
    };
    for (at, found) in folded.match_indices(needle) {
        total += 1;
        if hits.len() >= room {
            continue;
        }
        let start = sources.get(at).map_or(text.len(), |source| *source as usize);
        let end = sources.get(at + found.len()).map_or(text.len(), |source| *source as usize);
        // What follows a match is where the next character starts, past any spaces: those aren't the hit's.
        let end = start + text.get(start..end).map_or(0, |piece| piece.trim_end().len());
        if end <= start || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        let (start_char, end_char) = (chars_to(start), chars_to(end));
        hits.push(Hit { page, start: start_char, end: end_char, before: piece_before(&text[..start]), matched: piece(&text[start..end], MATCHED), after: piece(&text[end..], AFTER), hidden: is_hidden(&(start..end)) });
    }
    (total, hits)
}

/// The last [`BEFORE`] characters of what's before a hit, on one line.
fn piece_before(text: &str) -> Box<str> {
    let taken: Vec<char> = text.chars().rev().take(BEFORE + 1).collect();
    let cut = taken.len() > BEFORE;
    let kept: String = taken.into_iter().take(BEFORE).collect::<Vec<_>>().into_iter().rev().collect();
    let joined = one_line(&kept, true);
    if cut && !joined.is_empty() { format!("{CUT}{}", joined.trim_start()).into() } else { joined.into() }
}

/// The first `most` characters, on one line, with a mark if there was more.
fn piece(text: &str, most: usize) -> Box<str> {
    let kept: String = text.chars().take(most + 1).collect();
    let cut = kept.chars().count() > most;
    let kept: String = kept.chars().take(most).collect();
    let joined = one_line(&kept, false);
    if cut && !joined.is_empty() { format!("{}{CUT}", joined.trim_end()).into() } else { joined.into() }
}

/// Any run of whitespace as one space, keeping one at the edge that touches the hit, so words stay apart. Control characters go: a document's text is only drawn, and never as a command.
fn one_line(text: &str, keep_end: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if c.is_control() {
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    if space && keep_end && !out.is_empty() {
        out.push(' ');
    }
    if !keep_end && text.starts_with(char::is_whitespace) && !out.is_empty() {
        out.insert(0, ' ');
    }
    out
}

/// The results of one query, as whoever holds the session keeps them. Plain data, with the rules for stepping through them and for what's said about them.
#[derive(Debug, Default)]
pub struct Results {
    pub tab: Option<u32>,
    pub query: String,
    pub hits: Vec<Hit>,
    pub progress: Progress,
    /// `None` while it runs.
    pub end: Option<End>,
    /// The hit gone to last, if any.
    pub current: Option<usize>,
    /// Goes up with every change, so a host knows to pull.
    pub version: u64,
    /// Which run this is. Answers from an older one are dropped.
    pub generation: u64,
    /// What these hits are about, for knowing when they're about something else.
    pub basis: Option<Basis>,
    /// When it began, and how long until its first hit and its end, for measuring.
    pub began: Option<Instant>,
    pub first_hit: Option<Duration>,
    pub took: Option<Duration>,
}

impl Results {
    /// Starts again for another query.
    /// Starts again for another query over a plan.
    pub fn begin_on(&mut self, plan: &FindPlan, query: &str) -> u64 {
        let generation = self.begin(plan.tab(), query, plan.pages());
        self.basis = Some(plan.basis());
        generation
    }

    pub fn begin(&mut self, tab: u32, query: &str, pages: u32) -> u64 {
        self.basis = None;
        self.generation += 1;
        self.version += 1;
        self.tab = Some(tab);
        query.clone_into(&mut self.query);
        self.hits.clear();
        self.hits.shrink_to(64);
        self.progress = Progress { pages, ..Progress::default() };
        self.end = None;
        self.current = None;
        self.began = Some(Instant::now());
        self.first_hit = None;
        self.took = None;
        self.generation
    }

    /// Nothing's being looked for.
    pub fn clear(&mut self) {
        self.generation += 1;
        self.version += 1;
        self.tab = None;
        self.query.clear();
        self.hits = Vec::new();
        self.progress = Progress::default();
        self.end = None;
        self.current = None;
    }

    /// A page's hits arrived, for this run.
    pub fn add(&mut self, generation: u64, hits: Vec<Hit>, progress: &Progress) {
        if generation != self.generation {
            return;
        }
        let room = MAX_HITS.saturating_sub(self.hits.len());
        if self.first_hit.is_none() && !hits.is_empty() {
            self.first_hit = self.began.map(|began| began.elapsed());
        }
        self.hits.extend(hits.into_iter().take(room));
        self.progress = *progress;
        self.version += 1;
    }

    pub fn finish(&mut self, generation: u64, end: End, progress: Progress) {
        if generation != self.generation {
            return;
        }
        self.end = Some(end);
        self.progress = progress;
        self.took = self.began.map(|began| began.elapsed());
        self.version += 1;
    }

    pub fn running(&self) -> bool {
        self.tab.is_some() && self.end.is_none()
    }

    /// The next hit from where the reader is, or the one before it: from the current hit if there is one, and otherwise from `place`, a page and a character on it, so the first ⌘G goes to the first hit at or past where they're reading. It goes round at either end, as Find does on a Mac, once the query's finished: **while it runs, `None` means wait**, as the hit wanted may be on a page not read yet. A hit on hidden text is never gone to.
    pub fn step(&mut self, forward: bool, place: (u32, u32)) -> Option<&Hit> {
        let shown: Vec<usize> = self.hits.iter().enumerate().filter(|(_, hit)| !hit.hidden).map(|(at, _)| at).collect();
        let (first, last) = (*shown.first()?, *shown.last()?);
        let round = |end: usize| (!self.running()).then_some(end);
        let next = match (self.current, forward) {
            (Some(at), true) => shown.iter().copied().find(|other| *other > at).or_else(|| round(first)),
            (Some(at), false) => shown.iter().rev().copied().find(|other| *other < at).or_else(|| round(last)),
            (None, true) => shown.iter().copied().find(|at| (self.hits[*at].page, self.hits[*at].start) >= place).or_else(|| round(first)),
            (None, false) => shown.iter().rev().copied().find(|at| (self.hits[*at].page, self.hits[*at].start) < place).or_else(|| round(last)),
        }?;
        self.current = Some(next);
        self.version += 1;
        self.hits.get(next)
    }

    /// Goes to a hit by its number.
    pub fn choose(&mut self, index: usize) -> Option<&Hit> {
        if self.hits.get(index).is_none_or(|hit| hit.hidden) {
            return None;
        }
        self.current = Some(index);
        self.version += 1;
        self.hits.get(index)
    }

    /// The hit at a place, which is what the state record keeps of the current one.
    pub fn choose_place(&mut self, place: (u32, u32)) -> bool {
        match self.hits.iter().position(|hit| (hit.page, hit.start) == place) {
            Some(index) => {
                self.current = Some(index);
                self.version += 1;
                true
            }
            None => false,
        }
    }

    /// The line under the field, in words. Empty with nothing typed.
    pub fn status(&self) -> String {
        if self.tab.is_none() {
            return String::new();
        }
        let p = &self.progress;
        let hits = |n: u64| if n == 1 { "1 match".to_string() } else { format!("{} matches", group(n)) };
        let pages = |n: u32| if n == 1 { "1 page".to_string() } else { format!("{} pages", group(u64::from(n))) };
        let shown = if p.total > self.hits.len() as u64 { format!(", first {} listed", group(self.hits.len() as u64)) } else { String::new() };
        let found = format!("{} on {}{shown}", hits(p.total), pages(p.pages_with_hits));
        let unread = if p.failed > 0 { format!(". {} couldn't be read", pages(p.failed)) } else { String::new() };
        match &self.end {
            None if p.total == 0 => format!("Searching… page {} of {}", group(u64::from(p.pages_done)), group(u64::from(p.pages))),
            None => format!("{found}, searching…"),
            Some(End::Finished) if p.total == 0 && p.pages_done == p.no_text && p.pages > 0 => "No text to search. A scan has none until it's made searchable".to_string(),
            Some(End::Finished) if p.total == 0 => format!("Not found{unread}"),
            Some(End::Finished) => format!("{found}{unread}"),
            Some(End::Stopped) => format!("Stopped at page {}", group(u64::from(p.pages_done))),
            Some(End::OutOfTime(at)) if p.total == 0 => format!("Not found in pages 1 to {}, where it stopped for time", group(u64::from(*at))),
            Some(End::OutOfTime(at)) => format!("{found} in pages 1 to {}, where it stopped for time", group(u64::from(*at))),
            Some(End::Failed(words)) => words.clone(),
        }
    }

    /// The list as rows: each page with hits, then its hits, in reading order. `(page row, hit index)`: a page row has no hit.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.hits.len() * 2);
        let mut last = None;
        for (index, hit) in self.hits.iter().enumerate() {
            if last != Some(hit.page) {
                rows.push(Row::Page(hit.page));
                last = Some(hit.page);
            }
            rows.push(Row::Hit(index));
        }
        rows
    }
}

/// A row of the hit list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A page, from 0, heading its hits.
    Page(u32),
    Hit(usize),
}

/// What a page row says.
pub fn page_label(page: u32) -> String {
    format!("Page {}", group(u64::from(page) + 1))
}

/// A number with thin groups, as `1,204`.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, c) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The refusal for a query that's too long.
pub fn too_long() -> AgentError {
    AgentError::new("bad_query", "That's too long to find.", exit::FAILED).then("Find a few words.")
}

/// Whoever holds a session and can find in the background: `recto_control::finding::Finder`. One query at a time.
pub trait Finding: Send + Sync {
    /// Starts finding `needle` (from [`needle`]) for `query`, stopping and replacing whatever ran. Says which run it is.
    fn start(&self, plan: FindPlan, query: &str, needle: String) -> u64;
    /// Stops what's running and forgets the hits.
    fn clear(&self);
    fn with_results(&self, step: &mut dyn FnMut(&mut Results));
    /// Waits up to `limit` for that run to end. Says whether it did.
    fn wait(&self, generation: u64, limit: Duration) -> bool;
}

/// How long `recto find` waits for a query to finish before answering with what's found so far.
const COMMAND_WAIT: Duration = Duration::from_secs(30);
/// Hits in an answer, if not told otherwise, and at most.
const DEFAULT_LISTED: u32 = 20;
const MAX_LISTED: u32 = 200;

/// What `recto find` was asked.
pub(crate) struct Asked<'a> {
    pub query: Option<&'a str>,
    pub next: bool,
    pub previous: bool,
    pub close: bool,
    pub limit: Option<u32>,
}

/// Results kept for a session that can't find in the background, such as `recto local`: one command's worth.
struct Here(std::sync::Mutex<Results>);

impl Finding for Here {
    fn start(&self, _plan: FindPlan, _query: &str, _needle: String) -> u64 {
        0
    }
    fn clear(&self) {}
    fn with_results(&self, step: &mut dyn FnMut(&mut Results)) {
        step(&mut self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    }
    fn wait(&self, _generation: u64, _limit: Duration) -> bool {
        true
    }
}

/// Starts finding `query` in the document being read, in the background if this session can, and otherwise here, to the end. Puts it in the Find field and opens Find. Says which run it is. **The one way a query starts,** for `recto find` and the app's field alike.
pub fn begin<A: crate::SessionAccess>(access: &A, query: &str) -> Result<u64, AgentError> {
    let needle = needle(query).ok_or_else(|| if query.len() > MAX_QUERY_BYTES { too_long() } else { AgentError::new("bad_query", "That has nothing to find.", exit::FAILED).then("Find a word or a phrase.") })?;
    let plan = access.with(|s| {
        let id = s.active().ok_or_else(|| AgentError::new("no_active_tab", "There's no document being read to find in.", exit::NOT_FOUND).then("`recto open` or `recto tap` one first."))?;
        let plan = plan(s, id)?;
        s.set_find(true, query, query);
        Ok::<_, AgentError>(plan)
    })?;
    match access.finding() {
        Some(finding) => Ok(finding.start(plan, query, needle)),
        None => Ok(0),
    }
}

/// What's in the Find field that its hits aren't of, as after a restart or a move to another document, if anything: a query to find again before a step.
pub fn stale_query<A: crate::SessionAccess>(access: &A, finding: &dyn Finding) -> Option<String> {
    let wanted = access.with(|s| (s.find().query.clone(), s.active().map(|id| basis(s, id))));
    let mut have = (String::new(), None);
    finding.with_results(&mut |results| have = (results.query.clone(), results.basis));
    (!wanted.0.is_empty() && have != wanted).then_some(wanted.0)
}

/// How going to a hit goes: a step, as ⌘G's and an agent's `--next`, or the person's jump, as Return in the field and a click on a row, which marks where they were for Back, as following a link does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Going {
    Step,
    Jump,
}

/// Goes to the next hit or the one before, from the current one or from where the reader is, and notes it in the state record. The tab's page goes there too; the app also selects the hit. `None` if there's nothing to go to.
pub fn step<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, forward: bool, going: Going) -> Option<(u32, Hit)> {
    let place = access.with(|s| s.active().and_then(|id| s.tab(id)).map_or((0, 0), |tab| (tab.page, 0)));
    let mut hit = None;
    finding.with_results(&mut |results| hit = results.step(forward, place).cloned().zip(results.tab));
    let (hit, tab) = hit?;
    went_to(access, tab, &hit, going);
    Some((tab, hit))
}

/// Goes to a hit by its number in the list, as a click on its row does: the person's jump.
pub fn choose<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, index: usize) -> Option<(u32, Hit)> {
    let mut hit = None;
    finding.with_results(&mut |results| hit = results.choose(index).cloned().zip(results.tab));
    let (hit, tab) = hit?;
    went_to(access, tab, &hit, Going::Jump);
    Some((tab, hit))
}

fn went_to<A: crate::SessionAccess>(access: &A, tab: u32, hit: &Hit, going: Going) {
    access.with(|s| {
        s.set_find_current(Some((hit.page, hit.start)), Some(tab));
        if s.active().map(|id| id.0) == Some(tab) {
            let id = TabId(tab);
            match going {
                Going::Step => {
                    let _ = s.go_to(id, hit.page);
                }
                // Marked for Back, and the page view's told nothing more: the app brings the hit itself into view, a third down, which landing at the page's top would undo.
                Going::Jump => {
                    if s.jump_as(id, hit.page, 0.0, recto_session::Author::Owner).is_ok() {
                        let _ = s.take_jump(id);
                    }
                }
            }
        }
    });
}

/// Closes Find: what's running stops, the hits go, and what was typed stays for next time, as a Mac's Find keeps it.
pub fn close<A: crate::SessionAccess>(access: &A) {
    if let Some(finding) = access.finding() {
        finding.clear();
    }
    access.with(|s| {
        let draft = s.find().draft.clone();
        s.set_find(false, "", &draft);
        s.set_find_current(None, None);
    });
}

/// `recto find`: the app's Find, for an agent. With a query, finds it in the document being read and waits for it; `--next` and `--previous` go to a hit, from where the reader is; `--close` closes Find. Always says where Find stands.
pub(crate) fn command<A: crate::SessionAccess>(access: &A, asked: &Asked<'_>, events: &mut Vec<crate::Event>) -> Result<serde_json::Value, AgentError> {
    let listed = crate::positive_limit(asked.limit, DEFAULT_LISTED, MAX_LISTED)?;
    if [asked.query.is_some(), asked.next || asked.previous, asked.close].iter().filter(|said| **said).count() > 1 || (asked.next && asked.previous) {
        return Err(AgentError::new("usage", "That's more than one thing at once: a query, a step, or closing.", exit::USAGE).then("Find first, then `recto find --next`."));
    }
    let here = Here(std::sync::Mutex::new(Results::default()));
    let finding: &dyn Finding = access.finding().unwrap_or(&here);
    if asked.close {
        close(access);
        events.push(crate::Event::new("find_changed", serde_json::json!({ "open": false })));
        return Ok(answer(access, finding, listed, None));
    }
    // A step needs the hits of what's in the field, for the document being read: found again if they're of something else, as after a restart.
    let query = match asked.query {
        Some(query) => Some(query.to_string()),
        None if asked.next || asked.previous => stale_query(access, finding),
        None => None,
    };
    if let Some(query) = &query {
        let generation = begin(access, query)?;
        if access.finding().is_some() {
            finding.wait(generation, COMMAND_WAIT);
        } else {
            let (plan, needle) = access.with(|s| Ok::<_, AgentError>((plan(s, s.active().ok_or_else(|| AgentError::new("no_active_tab", "There's no document being read to find in.", exit::NOT_FOUND))?)?, needle(query).unwrap_or_default())))?;
            let mut results = Results::default();
            let generation = results.begin_on(&plan, query);
            let stop = AtomicBool::new(false);
            let (end, progress) = run(&plan, &needle, access.indexing(), &stop, &mut |hits, progress| results.add(generation, hits, progress));
            results.finish(generation, end, progress);
            here.with_results(&mut |kept| *kept = std::mem::take(&mut results));
        }
        events.push(crate::Event::new("find_changed", serde_json::json!({ "open": true })));
    }
    let mut went = None;
    if asked.next || asked.previous {
        let Some((tab, hit)) = step(access, finding, asked.next, Going::Step) else {
            return Err(AgentError::new("nothing_found", "There's no hit to go to.", exit::NOT_FOUND).then("Find something first: `recto find \"words\"`."));
        };
        events.push(crate::Event::new("page_changed", serde_json::json!({ "tab": tab, "page": hit.page + 1 })));
        events.push(crate::Event::new("find_changed", serde_json::json!({ "open": true, "page": hit.page + 1 })));
        went = Some((tab, hit));
    }
    Ok(answer(access, finding, listed, went))
}

/// Where Find stands, for an answer.
fn answer<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, listed: usize, went: Option<(u32, Hit)>) -> serde_json::Value {
    use serde_json::json;
    let record = access.with(|s| s.find().clone());
    let mut value = json!(null);
    finding.with_results(&mut |results| {
        let state = match &results.end {
            _ if results.tab.is_none() => "idle",
            None => "running",
            Some(End::Finished) => "done",
            Some(End::Stopped) => "stopped",
            Some(End::OutOfTime(_)) => "out_of_time",
            Some(End::Failed(_)) => "failed",
        };
        let hit_json = |index: usize, hit: &Hit| json!({ "index": index, "page": hit.page + 1, "start": hit.start, "end": hit.end, "hidden": hit.hidden, "untrusted": { "before": hit.before, "matched": hit.matched, "after": hit.after } });
        let p = results.progress;
        value = json!({
            "open": record.open,
            "query": record.query,
            "tab": results.tab,
            "state": state,
            "status": results.status(),
            "hits_total": p.total,
            "hits_listed": results.hits.len(),
            "pages_with_hits": p.pages_with_hits,
            "pages_searched": p.pages_done,
            "pages_in_scope": p.pages,
            "failed_pages": p.failed,
            "no_text_pages": p.no_text,
            "first_hit_ms": results.first_hit.map(|took| took.as_secs_f64() * 1000.0),
            "took_ms": results.took.map(|took| took.as_secs_f64() * 1000.0),
            "current": results.current.and_then(|at| results.hits.get(at).map(|hit| hit_json(at, hit))),
            "hits": results.hits.iter().take(listed).enumerate().map(|(at, hit)| hit_json(at, hit)).collect::<Vec<_>>(),
        });
    });
    if let Some((tab, hit)) = went {
        value["went_to"] = json!({ "tab": tab, "page": hit.page + 1, "start": hit.start, "end": hit.end });
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(text: &str, query: &str) -> Vec<Hit> {
        page_hits(text, &needle(query).unwrap(), 3, MAX_HITS, &|_| false).1
    }

    /// Failure: a hit's place is counted in bytes, so past an accented letter the highlight lands on the wrong words; or it takes in the spaces after it, highlighting the gap.
    #[test]
    fn a_hit_is_placed_in_characters_and_ends_at_its_last_letter() {
        let text = "caffè latte and the Latte\nbreak";
        let found = hits(text, "latte");
        assert_eq!(found.len(), 2);
        let chars: Vec<char> = text.chars().collect();
        for hit in &found {
            let words: String = chars[hit.start as usize..hit.end as usize].iter().collect();
            assert_eq!(words.to_lowercase(), "latte");
            assert_eq!(hit.page, 3);
        }
        assert_eq!(&*found[0].before, "caffè ");
        assert_eq!(&*found[1].after, " break", "a line break is one space");
    }

    /// Failure: a phrase broken over two lines isn't found, so Find misses what `search` finds.
    #[test]
    fn a_phrase_across_a_line_is_found_and_its_words_kept() {
        let found = hits("The Force\nMajeure clause", "force majeure");
        assert_eq!(found.len(), 1);
        assert_eq!(&*found[0].matched, "Force Majeure");
    }

    /// Failure: a hostile page of one long line makes a row of its whole length, or a page of one letter makes millions of hits.
    #[test]
    fn pieces_and_hits_are_bounded() {
        let long = "x".repeat(100_000) + "needle" + &"y".repeat(100_000);
        let found = hits(&long, "needle");
        assert!(found[0].before.chars().count() <= BEFORE + 1 && found[0].after.chars().count() <= AFTER + 1, "{found:?}");
        assert!(found[0].before.starts_with(CUT) && found[0].after.ends_with(CUT));
        let (total, kept) = page_hits(&"a".repeat(200_000), "a", 0, 5, &|_| false);
        assert_eq!((total, kept.len()), (200_000, 5));
    }

    /// Failure: a control character from the document reaches a row as it is.
    #[test]
    fn control_characters_never_reach_a_row() {
        let found = hits("bell\u{7}word\u{1b}[31m after", "word");
        assert!(!found[0].before.chars().chain(found[0].after.chars()).any(char::is_control), "{found:?}");
    }

    fn hit(page: u32, start: u32) -> Hit {
        Hit { page, start, end: start + 1, before: "".into(), matched: "x".into(), after: "".into(), hidden: false }
    }

    /// Failure: a hit held in the list grows, so 1,000 of them cost more than ENGINEERING's table says.
    #[test]
    fn a_hit_is_small() {
        assert!(std::mem::size_of::<Hit>() <= 64, "{}", std::mem::size_of::<Hit>());
    }

    /// Failure: ⌘G starts from the top whatever's being read, or doesn't go round at the end, as Find does on a Mac.
    #[test]
    fn stepping_starts_where_the_reader_is_and_goes_round() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        results.add(generation, vec![hit(1, 5), hit(4, 0), hit(4, 9), hit(8, 2)], &Progress { total: 4, ..Progress::default() });
        results.finish(generation, End::Finished, Progress { total: 4, ..Progress::default() });
        assert_eq!(results.step(true, (4, 3)).map(|hit| (hit.page, hit.start)), Some((4, 9)));
        assert_eq!(results.step(true, (0, 0)).map(|hit| hit.page), Some(8));
        assert_eq!(results.step(true, (0, 0)).map(|hit| hit.page), Some(1), "round to the first");
        assert_eq!(results.step(false, (0, 0)).map(|hit| hit.page), Some(8), "and back round to the last");
        results.current = None;
        assert_eq!(results.step(false, (4, 3)).map(|hit| (hit.page, hit.start)), Some((4, 0)), "back from where they are");
    }

    /// Failure: ⌘G pressed while a query is still reading goes to the first hit there is, behind the reader, as the hit at their place isn't found yet; or it never goes once the query ends.
    #[test]
    fn a_step_while_it_runs_waits_for_a_hit_past_the_reader() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        results.add(generation, vec![hit(1, 0)], &Progress { total: 1, ..Progress::default() });
        assert!(results.step(true, (5, 0)).is_none(), "nothing past page 6 yet: wait");
        assert!(results.step(false, (0, 0)).is_none(), "the last hit isn't known yet either");
        results.add(generation, vec![hit(7, 3)], &Progress { total: 2, ..Progress::default() });
        assert_eq!(results.step(true, (5, 0)).map(|hit| hit.page), Some(7));
        assert!(results.step(true, (0, 0)).is_none(), "going round waits for the end");
        results.finish(generation, End::Finished, Progress { total: 2, ..Progress::default() });
        assert_eq!(results.step(true, (0, 0)).map(|hit| hit.page), Some(1), "then it goes round");
    }

    /// Failure: a hit whose words look hidden is gone to, selecting nothing that can be seen, as though something were there.
    #[test]
    fn hidden_hits_are_listed_and_never_gone_to() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        let hidden = Hit { hidden: true, ..hit(2, 0) };
        results.add(generation, vec![hit(1, 0), hidden, hit(3, 0)], &Progress { total: 3, ..Progress::default() });
        results.finish(generation, End::Finished, Progress { total: 3, ..Progress::default() });
        assert_eq!(results.step(true, (2, 0)).map(|hit| hit.page), Some(3), "past the hidden one");
        assert_eq!(results.step(false, (0, 0)).map(|hit| hit.page), Some(1));
        assert!(results.choose(1).is_none(), "a click on it goes nowhere");
        let found = page_hits("seen and ignore this", "ignore", 0, MAX_HITS, &|range| range.start >= 9).1;
        assert!(found[0].hidden, "the check's answer is on the hit");
    }

    /// Failure: hits from a query that's been replaced land in the new one's list.
    #[test]
    fn an_old_runs_hits_are_dropped() {
        let mut results = Results::default();
        let old = results.begin(1, "a", 10);
        let new = results.begin(1, "ab", 10);
        results.add(old, vec![hit(0, 0)], &Progress::default());
        results.finish(old, End::Finished, Progress::default());
        assert!(results.hits.is_empty() && results.running());
        results.add(new, vec![hit(2, 0)], &Progress { total: 1, pages_with_hits: 1, pages_done: 3, pages: 10, ..Progress::default() });
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.rows(), vec![Row::Page(2), Row::Hit(0)]);
    }

    /// Failure: the list is held past its cap, so a one-letter query on a large file holds every hit.
    #[test]
    fn the_list_is_capped_and_the_count_is_whole() {
        let mut results = Results::default();
        let generation = results.begin(1, "a", 10);
        for page in 0..3 {
            results.add(generation, (0..600).map(|at| hit(page, at)).collect(), &Progress { total: 600 * u64::from(page + 1), pages_with_hits: page + 1, ..Progress::default() });
        }
        results.finish(generation, End::Finished, Progress { total: 1800, pages_with_hits: 3, pages_done: 10, pages: 10, ..Progress::default() });
        assert_eq!(results.hits.len(), MAX_HITS);
        assert_eq!(results.status(), "1,800 matches on 3 pages, first 1,000 listed");
    }

    /// Failure: the line under the field says nothing was found when pages couldn't be read, or before it's looked.
    #[test]
    fn the_status_never_says_not_found_of_pages_not_read() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        assert_eq!(results.status(), "Searching… page 0 of 10");
        results.finish(generation, End::Finished, Progress { pages: 10, pages_done: 10, failed: 2, ..Progress::default() });
        assert_eq!(results.status(), "Not found. 2 pages couldn't be read");
        results.finish(generation, End::Finished, Progress { pages: 4, pages_done: 4, no_text: 4, ..Progress::default() });
        assert!(results.status().starts_with("No text to search"));
        assert_eq!(page_label(1203), "Page 1,204");
    }
}
