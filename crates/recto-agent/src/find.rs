//! **Find**: the reader's Find in the sidebar, and `recto find`, which is the same thing for an agent. It searches the document being read (in a comparison, the pane being read) by the same reading of its pages as `search`: from its corpus if that's built, and page by page through one worker if not, asking for a corpus for next time. So what Find finds is what `search` finds.
//!
//! **What's held.** The hits of one query, in reading order, at most [`MAX_HITS`], each with a short piece of its line either side ([`BEFORE`], [`AFTER`]) and its place as characters of the page's text, which are what a selection's places are, so a hit can be selected and highlighted. Past the cap they're counted and not kept. A new query replaces the last one, and stops it if it's still running. **What's said about it,** in words, is here too: the line under the field and the page rows, so the host draws them and decides none of it.
//!
//! **What runs.** [`run`] is one pass, a page at a time, looking at `stop` between pages, so a new query ends the old one within a page. Whoever holds the session runs it off every thread that mustn't wait: `recto_control::finding`. It's bounded by [`MAX_TIME`], after which it says how far it got.

use crate::Code;
use crate::index::Indexing;
pub use crate::search::KeyLookup;
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
    /// Whether its unsaved edits fit its file: where they don't, nothing says which page of it a match is on, and Find says so ([`crate::unfit::refusal`]).
    pub fit: recto_session::EditsFit,
}

/// A tab's basis now, under the session's lock: a few numbers.
pub fn basis<L: DocLoader>(session: &Session<L>, id: TabId) -> Basis {
    Basis {
        tab: id.0,
        pages: session.pages().count(id),
        stamp: session.tabs().own_stamp(id),
        changes: session.lists().get(id).map_or(0, recto_session::edit::PageEdits::changes),
        fit: session.notices().fit(id),
    }
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
    let basis = basis(session, id);
    // The same words for the person's Find and an agent's.
    if let Some(refused) = crate::unfit::refusal(basis.fit, crate::unfit::Asked::Find) {
        return Err(refused);
    }
    search::plan_of(session, id, None, None).map(|plan| FindPlan { plan, basis })
}

impl FindPlan {
    /// **For the person's own Find:** the passwords they typed, so a document they opened with one is searched through a worker that has it, page by page, with no corpus read, built or asked for. Never for an agent's: an agent's `find` and `search` are refused for such a document before they plan anything.
    #[must_use]
    pub fn with_keys(mut self, keys: search::KeyLookup) -> Self {
        self.plan.keys = Some(keys);
        self
    }
}

/// Whether a query is too long to find, which [`too_long`] refuses: the one rule, so the window says why by the query, not by the refusal's words.
#[must_use]
pub fn too_long_query(query: &str) -> bool {
    query.len() > MAX_QUERY_BYTES
}

/// What's looked for, folded as a page is: lower case, any run of spaces and line breaks one space. `None` if there's nothing to look for.
pub fn needle(query: &str) -> Option<String> {
    if too_long_query(query) {
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
    let settings = Settings {
        limit: MAX_HITS,
        context: 0,
        deadline,
        draw_until,
        second_pass_until: deadline,
        live_budget: search::LIVE_REREAD_BUDGET,
        indexing,
        force_scan: false,
    };
    let plan = &plan.plan;
    let mut reader = Reader::new(plan, &settings);
    let mut slot = Slot::default();
    let mut folded = String::new();
    let mut progress = Progress {
        pages: u32::try_from(plan.pages.len()).unwrap_or(u32::MAX),
        ..Progress::default()
    };
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
        hits.push(Hit {
            page,
            start: start_char,
            end: end_char,
            before: piece_before(&text[..start]),
            matched: piece(&text[start..end], MATCHED),
            after: piece(&text[end..], AFTER),
            hidden: is_hidden(&(start..end)),
        });
    }
    (total, hits)
}

/// The last [`BEFORE`] characters of what's before a hit, on one line.
fn piece_before(text: &str) -> Box<str> {
    let taken: Vec<char> = text.chars().rev().take(BEFORE + 1).collect();
    let cut = taken.len() > BEFORE;
    let kept: String = taken.into_iter().take(BEFORE).collect::<Vec<_>>().into_iter().rev().collect();
    let joined = one_line(&kept, true);
    if cut && !joined.is_empty() {
        format!("{CUT}{}", joined.trim_start()).into()
    } else {
        joined.into()
    }
}

/// The first `most` characters, on one line, with a mark if there was more.
fn piece(text: &str, most: usize) -> Box<str> {
    let kept: String = text.chars().take(most + 1).collect();
    let cut = kept.chars().count() > most;
    let kept: String = kept.chars().take(most).collect();
    let joined = one_line(&kept, false);
    if cut && !joined.is_empty() {
        format!("{}{CUT}", joined.trim_end()).into()
    } else {
        joined.into()
    }
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
    /// Goes up with every change, so a host knows to pull: read by [`Self::version`], moved only by `Results`' own methods, each of which changes what's
    /// drawn of Find (held by the test that walks them all).
    version: u64,
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

    /// This query can't be found in what `basis` is about, for `words`' reason: no hits, and the line under the field says why. Kept against that basis, so
    /// it's said once and not tried again until the basis moves.
    pub fn refused(&mut self, tab: u32, query: &str, basis: Basis, words: String) {
        let generation = self.begin(tab, query, 0);
        self.basis = Some(basis);
        self.finish(generation, End::Failed(words), Progress::default());
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

    /// Goes up with every change to the hits, the line under the field, whether it runs and the hit gone to: when a host pulls them again.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
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
            (None, true) => shown
                .iter()
                .copied()
                .find(|at| (self.hits[*at].page, self.hits[*at].start) >= place)
                .or_else(|| round(first)),
            (None, false) => shown
                .iter()
                .rev()
                .copied()
                .find(|at| (self.hits[*at].page, self.hits[*at].start) < place)
                .or_else(|| round(last)),
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
        let pages = |n: u32| {
            if n == 1 {
                "1 page".to_string()
            } else {
                format!("{} pages", group(u64::from(n)))
            }
        };
        let shown = if p.total > self.hits.len() as u64 {
            format!(", first {} listed", group(self.hits.len() as u64))
        } else {
            String::new()
        };
        let found = format!("{} on {}{shown}", hits(p.total), pages(p.pages_with_hits));
        let unread = if p.failed > 0 {
            format!(". {} couldn't be read", pages(p.failed))
        } else {
            String::new()
        };
        match &self.end {
            None if p.total == 0 => format!("Searching… page {} of {}", group(u64::from(p.pages_done)), group(u64::from(p.pages))),
            None => format!("{found}, searching…"),
            Some(End::Finished) if p.total == 0 && p.pages_done == p.no_text && p.pages > 0 => {
                "No text to search. A scan has none until it's made searchable".to_string()
            }
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
    AgentError::new(Code::BAD_QUERY, "That's too long to find.", exit::FAILED).then("Find a few words.")
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
    /// Waits up to `limit` for the results to be other than at `version`, as when hits come or the run ends. Says whether they are.
    fn wait_past(&self, version: u64, limit: Duration) -> bool;
}

mod command;
pub(crate) use command::command;

/// Starts finding `query` in the document being read, in the background if this session can, and otherwise here, to the end. Puts it in the Find field and opens Find. Says which run it is. **The one way a query starts,** for `recto find` and the app's field alike.
pub fn begin<A: crate::SessionAccess>(access: &A, query: &str) -> Result<u64, AgentError> {
    begin_keyed(access, query, None)
}

/// [`begin`], for the person's own Find, with their passwords: see [`FindPlan::with_keys`].
pub fn begin_keyed<A: crate::SessionAccess>(access: &A, query: &str, keys: Option<search::KeyLookup>) -> Result<u64, AgentError> {
    let needle = needle(query).ok_or_else(|| {
        if too_long_query(query) {
            too_long()
        } else {
            AgentError::new(Code::BAD_QUERY, "That has nothing to find.", exit::FAILED).then("Find a word or a phrase.")
        }
    })?;
    let plan = access.with(|s| {
        let id = s.tabs().active().ok_or_else(|| {
            AgentError::new(Code::NO_ACTIVE_TAB, "There's no document being read to find in.", exit::NOT_FOUND).then("`recto open` or `recto tap` one first.")
        })?;
        let plan = plan(s, id)?;
        s.viewing_mut().set_find(true, query, query);
        Ok::<_, AgentError>(match keys {
            Some(keys) => plan.with_keys(keys),
            None => plan,
        })
    })?;
    match access.finding() {
        Some(finding) => Ok(finding.start(plan, query, needle)),
        None => Ok(0),
    }
}

/// What's in the Find field that its hits aren't of, as after a restart or a move to another document, if anything: a query to find again before a step.
pub fn stale_query<A: crate::SessionAccess>(access: &A, finding: &dyn Finding) -> Option<String> {
    let wanted = access.with(|s| (s.viewing().find().query.clone(), s.tabs().active().map(|id| basis(s, id))));
    let mut have = (String::new(), None);
    finding.with_results(&mut |results| have = (results.query.clone(), results.basis));
    (!wanted.0.is_empty() && have != wanted).then_some(wanted.0)
}

/// How going to a hit goes: a step, as ⌘G's and an agent's `--next`, or the person's jump, as Return in the field and a click on a row, which marks where they were for Back, as following a link does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Going {
    Step,
    /// By whom, for the trail: an agent's marks give way to the person's.
    Jump(recto_session::Author),
}

/// Goes to the next hit or the one before, from the current one or from where the reader is, and notes it in the state record. The tab's page goes there too; the app also selects the hit. `None` if there's nothing to go to.
pub fn step<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, forward: bool, going: Going) -> Option<(u32, Hit)> {
    let place = access.with(|s| s.tabs().active().and_then(|id| s.tabs().tab(id)).map_or((0, 0), |tab| (tab.page, 0)));
    let mut hit = None;
    finding.with_results(&mut |results| hit = results.step(forward, place).cloned().zip(results.tab));
    let (hit, tab) = hit?;
    went_to(access, tab, &hit, going);
    Some((tab, hit))
}

/// **A step that waits for its hit,** for an agent's `recto find --next` and `--previous`, which answer once: while the query runs, [`Results::step`] says wait when the hit wanted isn't known yet, as the app's kept step does, so it's tried again each time more is known, until there's a hit, the query ends, or `limit` passes. Only then is there nothing to go to. The same rule as the app's: what's waited on is [`step`] saying so, never a second judgement.
pub fn step_waiting<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, forward: bool, going: Going, limit: Duration) -> Option<(u32, Hit)> {
    waiting_for(finding, limit, || step(access, finding, forward, going))
}

/// Tries `went` each time more is known while the query runs, until it goes somewhere, the query ends, or `limit` passes.
pub(crate) fn waiting_for(finding: &dyn Finding, limit: Duration, mut went: impl FnMut() -> Option<(u32, Hit)>) -> Option<(u32, Hit)> {
    let deadline = Instant::now() + limit;
    loop {
        // What was known before the try: anything after it is worth another.
        let mut known = (0, false);
        finding.with_results(&mut |results| known = (results.version, results.running()));
        if let Some(went) = went() {
            return Some(went);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if !known.1 || left.is_zero() {
            return None;
        }
        finding.wait_past(known.0, left);
    }
}

/// Goes to a hit by its number in the list, as a click on its row does: the person's jump.
pub fn choose<A: crate::SessionAccess>(access: &A, finding: &dyn Finding, index: usize, by: recto_session::Author) -> Option<(u32, Hit)> {
    let mut hit = None;
    finding.with_results(&mut |results| hit = results.choose(index).cloned().zip(results.tab));
    let (hit, tab) = hit?;
    went_to(access, tab, &hit, Going::Jump(by));
    Some((tab, hit))
}

fn went_to<A: crate::SessionAccess>(access: &A, tab: u32, hit: &Hit, going: Going) {
    access.with(|s| {
        s.viewing_mut().set_find_current(Some((hit.page, hit.start)), Some(tab));
        if s.tabs().active().map(|id| id.0) == Some(tab) {
            let id = TabId(tab);
            match going {
                Going::Step => {
                    let _ = s.go_to(id, hit.page);
                }
                // Marked for Back, and the page view's told nothing more: the app brings the hit itself into view, a third down, which landing at the page's top would undo.
                Going::Jump(by) => {
                    if s.jump_as(id, hit.page, 0.0, by).is_ok() {
                        let _ = s.mail_mut().take_jump(id);
                    }
                }
            }
        }
    });
}

/// **What watchers hear when a hit is gone to,** whoever went (R2.3, C5g): the page the hit's on, and Find at that page, as the hit gone to changed. No page
/// for a document the person opened with its password.
pub fn heard_going_to<L: DocLoader>(session: &Session<L>, tab: u32, hit: &Hit, events: &mut Vec<crate::Event>) {
    let id = TabId(tab);
    events.push(crate::page_changed(session, id, hit.page + 1));
    events.push(crate::Event::FindOpened {
        page: (!session.tabs().withheld_from_agents(id)).then_some(hit.page + 1),
    });
}

/// Closes Find: what's running stops, the hits go, and what was typed stays for next time, as a Mac's Find keeps it.
pub fn close<A: crate::SessionAccess>(access: &A) {
    if let Some(finding) = access.finding() {
        finding.clear();
    }
    access.with(|s| {
        let draft = s.viewing().find().draft.clone();
        s.viewing_mut().set_find(false, "", &draft);
        s.viewing_mut().set_find_current(None, None);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Find whose run is driven by the test: its hits come only once a step is waiting for them, so the step is certainly asked while the query runs.
    #[derive(Default)]
    struct Driven {
        results: std::sync::Mutex<Results>,
        changed: std::sync::Condvar,
        waiting: std::sync::atomic::AtomicBool,
    }

    impl Finding for Driven {
        fn start(&self, _plan: FindPlan, _query: &str, _needle: String) -> u64 {
            0
        }
        fn clear(&self) {}
        fn with_results(&self, step: &mut dyn FnMut(&mut Results)) {
            step(&mut self.results.lock().unwrap());
        }
        fn wait(&self, _generation: u64, _limit: Duration) -> bool {
            true
        }
        fn wait_past(&self, version: u64, limit: Duration) -> bool {
            let mut results = self.results.lock().unwrap();
            self.waiting.store(true, std::sync::atomic::Ordering::SeqCst);
            while results.version == version {
                let (next, timeout) = self.changed.wait_timeout(results, limit).unwrap();
                results = next;
                if timeout.timed_out() {
                    return false;
                }
            }
            true
        }
    }

    #[derive(Clone)]
    struct TenPages;

    impl DocLoader for TenPages {
        fn load(&self, _path: &std::path::Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
            Ok(recto_core::DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: recto_core::Toc::default(),
                renderer_only: false,
            })
        }
    }

    fn found_on(page: u32) -> Hit {
        Hit {
            page,
            start: 0,
            end: 4,
            before: "".into(),
            matched: "word".into(),
            after: "".into(),
            hidden: false,
        }
    }

    /// Failure: an agent's `recto find --next` (and ⌘K's `--run` of it), asked while the query runs and before the hit it wants is known, answers that there's nothing to go to, where the app's own step waits for it (the palette's test, after the sandbox merge changed the timing). It waits, bounded, for a hit or the query's end, and only then says there's none.
    #[test]
    fn an_agents_step_waits_for_its_hit_while_the_query_runs() {
        use std::sync::atomic::Ordering;
        let session = std::sync::Mutex::new(Session::new(TenPages));
        let tab = session.lock().unwrap().open(std::path::Path::new("/d/ten.pdf")).unwrap().0;
        let finding = std::sync::Arc::new(Driven::default());
        let run = |hits: Vec<Hit>| {
            let generation = finding.results.lock().unwrap().begin(tab, "word", 10);
            finding.waiting.store(false, Ordering::SeqCst);
            let driver = std::sync::Arc::clone(&finding);
            std::thread::spawn(move || {
                // Bounded, so a step that never waits fails its assertion rather than hanging the test.
                let given_up = Instant::now() + Duration::from_secs(10);
                while !driver.waiting.load(Ordering::SeqCst) && Instant::now() < given_up {
                    std::thread::yield_now();
                }
                let mut results = driver.results.lock().unwrap();
                let progress = Progress {
                    pages_done: 10,
                    pages: 10,
                    pages_with_hits: u32::try_from(hits.len()).unwrap(),
                    total: hits.len() as u64,
                    ..Progress::default()
                };
                results.add(generation, hits, &progress);
                results.finish(generation, End::Finished, progress);
                driver.changed.notify_all();
            })
        };
        // Forward from page 1: the hit on page 6 comes while the step waits.
        let driver = run(vec![found_on(5)]);
        let went = step_waiting(&session, &*finding, true, Going::Step, Duration::from_secs(10));
        driver.join().unwrap();
        assert_eq!(went.map(|(_, hit)| hit.page), Some(5), "the step waited for the hit");
        // Back from page 1 goes round to the last hit, which is known only at the query's end.
        session.lock().unwrap().go_to(TabId(tab), 0).unwrap();
        let driver = run(vec![found_on(2), found_on(8)]);
        let went = step_waiting(&session, &*finding, false, Going::Step, Duration::from_secs(10));
        driver.join().unwrap();
        assert_eq!(went.map(|(_, hit)| hit.page), Some(8), "it went round once the end was known");
        // A query that ends with nothing: no hit, and the answer comes at the end, not at the limit.
        let driver = run(Vec::new());
        let began = Instant::now();
        assert!(step_waiting(&session, &*finding, true, Going::Step, Duration::from_secs(10)).is_none());
        driver.join().unwrap();
        assert!(began.elapsed() < Duration::from_secs(5), "nothing to go to is said when the query ends");
    }

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
        assert!(
            found[0].before.chars().count() <= BEFORE + 1 && found[0].after.chars().count() <= AFTER + 1,
            "{found:?}"
        );
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
        Hit {
            page,
            start,
            end: start + 1,
            before: "".into(),
            matched: "x".into(),
            after: "".into(),
            hidden: false,
        }
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
        results.add(
            generation,
            vec![hit(1, 5), hit(4, 0), hit(4, 9), hit(8, 2)],
            &Progress {
                total: 4,
                ..Progress::default()
            },
        );
        results.finish(
            generation,
            End::Finished,
            Progress {
                total: 4,
                ..Progress::default()
            },
        );
        assert_eq!(results.step(true, (4, 3)).map(|hit| (hit.page, hit.start)), Some((4, 9)));
        assert_eq!(results.step(true, (0, 0)).map(|hit| hit.page), Some(8));
        assert_eq!(results.step(true, (0, 0)).map(|hit| hit.page), Some(1), "round to the first");
        assert_eq!(results.step(false, (0, 0)).map(|hit| hit.page), Some(8), "and back round to the last");
        results.current = None;
        assert_eq!(
            results.step(false, (4, 3)).map(|hit| (hit.page, hit.start)),
            Some((4, 0)),
            "back from where they are"
        );
    }

    /// Failure: ⌘G pressed while a query is still reading goes to the first hit there is, behind the reader, as the hit at their place isn't found yet; or it never goes once the query ends.
    #[test]
    fn a_step_while_it_runs_waits_for_a_hit_past_the_reader() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        results.add(
            generation,
            vec![hit(1, 0)],
            &Progress {
                total: 1,
                ..Progress::default()
            },
        );
        assert!(results.step(true, (5, 0)).is_none(), "nothing past page 6 yet: wait");
        assert!(results.step(false, (0, 0)).is_none(), "the last hit isn't known yet either");
        results.add(
            generation,
            vec![hit(7, 3)],
            &Progress {
                total: 2,
                ..Progress::default()
            },
        );
        assert_eq!(results.step(true, (5, 0)).map(|hit| hit.page), Some(7));
        assert!(results.step(true, (0, 0)).is_none(), "going round waits for the end");
        results.finish(
            generation,
            End::Finished,
            Progress {
                total: 2,
                ..Progress::default()
            },
        );
        assert_eq!(results.step(true, (0, 0)).map(|hit| hit.page), Some(1), "then it goes round");
    }

    /// Failure: a hit whose words look hidden is gone to, selecting nothing that can be seen, as though something were there.
    #[test]
    fn hidden_hits_are_listed_and_never_gone_to() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        let hidden = Hit { hidden: true, ..hit(2, 0) };
        results.add(
            generation,
            vec![hit(1, 0), hidden, hit(3, 0)],
            &Progress {
                total: 3,
                ..Progress::default()
            },
        );
        results.finish(
            generation,
            End::Finished,
            Progress {
                total: 3,
                ..Progress::default()
            },
        );
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
        results.add(
            new,
            vec![hit(2, 0)],
            &Progress {
                total: 1,
                pages_with_hits: 1,
                pages_done: 3,
                pages: 10,
                ..Progress::default()
            },
        );
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.rows(), vec![Row::Page(2), Row::Hit(0)]);
    }

    /// Failure: the list is held past its cap, so a one-letter query on a large file holds every hit.
    #[test]
    fn the_list_is_capped_and_the_count_is_whole() {
        let mut results = Results::default();
        let generation = results.begin(1, "a", 10);
        for page in 0..3 {
            results.add(
                generation,
                (0..600).map(|at| hit(page, at)).collect(),
                &Progress {
                    total: 600 * u64::from(page + 1),
                    pages_with_hits: page + 1,
                    ..Progress::default()
                },
            );
        }
        results.finish(
            generation,
            End::Finished,
            Progress {
                total: 1800,
                pages_with_hits: 3,
                pages_done: 10,
                pages: 10,
                ..Progress::default()
            },
        );
        assert_eq!(results.hits.len(), MAX_HITS);
        assert_eq!(results.status(), "1,800 matches on 3 pages, first 1,000 listed");
    }

    /// Failure: the line under the field says nothing was found when pages couldn't be read, or before it's looked.
    #[test]
    fn the_status_never_says_not_found_of_pages_not_read() {
        let mut results = Results::default();
        let generation = results.begin(1, "x", 10);
        assert_eq!(results.status(), "Searching… page 0 of 10");
        results.finish(
            generation,
            End::Finished,
            Progress {
                pages: 10,
                pages_done: 10,
                failed: 2,
                ..Progress::default()
            },
        );
        assert_eq!(results.status(), "Not found. 2 pages couldn't be read");
        results.finish(
            generation,
            End::Finished,
            Progress {
                pages: 4,
                pages_done: 4,
                no_text: 4,
                ..Progress::default()
            },
        );
        assert!(results.status().starts_with("No text to search"));
        assert_eq!(page_label(1203), "Page 1,204");
    }

    /// The names of `Results`' methods that take `&mut self`, read from this file: every one of them is a change to what a host draws of Find, or nothing.
    fn mutating_methods() -> Vec<String> {
        let source = include_str!("find.rs");
        let block = source.split("\nimpl Results {\n").nth(1).expect("`impl Results` in find.rs");
        let block = &block[..block.find("\n}\n").expect("the end of `impl Results`")];
        block
            .lines()
            .filter_map(|line| {
                let line = line.strip_prefix("    ")?;
                let line = line.strip_prefix("pub ").or_else(|| line.strip_prefix("pub(crate) ")).unwrap_or(line);
                let (name, rest) = line.strip_prefix("fn ")?.split_once('(')?;
                rest.starts_with("&mut self").then(|| name.split('<').next().unwrap_or(name).to_string())
            })
            .collect()
    }

    /// Failure (the review of R3.7's S2): a method that changes the hits, the line under the field, whether a query runs or the hit gone to doesn't move
    /// `version`, so the window, which Find now tells only when its fingerprint of what's drawn changes (the version among it), keeps showing what was; or
    /// one moves it with nothing changed, so the window pulls for nothing. Every `&mut self` method of `Results`, read from the source so a new one can't be
    /// left out, is called once where it changes something and once where it can't.
    #[test]
    fn every_change_to_the_results_moves_their_version_and_nothing_else_does() {
        let session = std::sync::Mutex::new(Session::new(TenPages));
        let tab = session.lock().unwrap().open(std::path::Path::new("/d/ten.pdf")).unwrap().0;
        let find_plan = plan(&session.lock().unwrap(), TabId(tab)).expect("a plan");
        let basis = find_plan.basis();
        // Running, with two hits and the first gone to.
        let running = || {
            let mut results = Results::default();
            let generation = results.begin(tab, "word", 10);
            results.add(generation, vec![found_on(2), found_on(5)], &Progress::default());
            results.choose(0);
            results
        };
        // Each method: a call that changes something, and one that can't (`None` for a method that always changes what's drawn).
        type Call = Box<dyn Fn(&mut Results)>;
        let table: Vec<(&str, Call, Option<Call>)> = vec![
            ("begin_on", Box::new(move |r| drop(r.begin_on(&find_plan, "word"))), None),
            ("begin", Box::new(move |r| drop(r.begin(tab, "other", 10))), None),
            ("refused", Box::new(move |r| r.refused(tab, "word", basis, "No.".into())), None),
            ("clear", Box::new(Results::clear), None),
            (
                "add",
                Box::new(|r| r.add(r.generation, vec![found_on(7)], &Progress::default())),
                Some(Box::new(|r| r.add(r.generation + 1, vec![found_on(7)], &Progress::default()))),
            ),
            (
                "finish",
                Box::new(|r| r.finish(r.generation, End::Finished, Progress::default())),
                Some(Box::new(|r| r.finish(r.generation + 1, End::Finished, Progress::default()))),
            ),
            (
                "step",
                Box::new(|r| drop(r.step(true, (0, 0)))),
                // From the last hit while the query runs: it waits for one past it, and nothing's gone to.
                Some(Box::new(|r| {
                    r.current = Some(1);
                    drop(r.step(true, (0, 0)));
                })),
            ),
            ("choose", Box::new(|r| drop(r.choose(1))), Some(Box::new(|r| drop(r.choose(9))))),
            (
                "choose_place",
                Box::new(|r| drop(r.choose_place((5, 0)))),
                Some(Box::new(|r| drop(r.choose_place((9, 9))))),
            ),
        ];
        let mut named: Vec<&str> = table.iter().map(|(name, ..)| *name).collect();
        let mut found = mutating_methods();
        named.sort_unstable();
        found.sort_unstable();
        assert_eq!(found, named, "every `&mut self` method of `Results` has a row");
        for (name, changes, cannot) in &table {
            let mut results = running();
            let was = results.version();
            changes(&mut results);
            assert!(results.version() > was, "{name}: a change that didn't move the version");
            if let Some(cannot) = cannot {
                let mut results = running();
                let was = results.version();
                cannot(&mut results);
                assert_eq!(results.version(), was, "{name}: nothing changed, and the version moved");
            }
        }
    }
}
