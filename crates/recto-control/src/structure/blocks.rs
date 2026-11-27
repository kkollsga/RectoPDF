//! The structure helper's `blocks` job, as the session reads it: a line a page and one end line, each believed only as far as it is checked (`docs/engineering/blocks.md`).
//!
//! **What a line may be.** A page line is `{"page","blocks"}`, read by `recto_index::blocks::map::PageLines` (no second parser): every page once, in order, and none past the index's page count. Each is mapped onto the
//! page's own text with [`recto_index::blocks::map::map_line`] against what a [`PageStore`] gives for that page, and the store is handed the page's blocks, once, in page order. The end is
//! `{"state":"ready", …}` whose `pages` is the count of lines taken and the index's own, whose `blocks` is the sum of the lines' blocks, with at most 32 entries, all numbers or bools but `state`, and none of the session's own
//! names. A `refused`, `changed` or `failed` is only ever the first thing said. Anything else is bad, and the helper is killed.
//!
//! **Nothing here writes to disk.** A store does that: it hands the text of a page and takes the mapped blocks, and may stop the job with a code of its own.

use super::{Answer, Said};
use recto_index::blocks::PageBlocks;
use recto_index::blocks::map::{Drops, LineError, MAX_LINE_BYTES, PageLines, Shape, map_line};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

/// The job line: the blocks of the whole document, at the typography scan, the helper watching its own footprint against [`super::MEMORY_LIMIT`] as a second line.
pub fn job() -> Value {
    json!({ "kind": "blocks", "memory_limit": super::MEMORY_LIMIT, "scan": "typography" })
}

/// A line it won't have.
const BAD: &str = "bad_answer";
/// Most entries of the end line.
const MAX_NUMBERS: usize = 32;
/// What an answer of a document may be before its pages are counted: a page line is at most 512 KiB, but pages are mostly far smaller. This and 64 KiB a page, up to 2 GiB.
const ANSWER_BASE: u64 = 4 * 1024 * 1024;
const ANSWER_PER_PAGE: u64 = 64 * 1024;
const ANSWER_CEILING: u64 = 2 * 1024 * 1024 * 1024;

/// Where a page's text comes from and where its blocks go. The corpus implements this; a test or a fuzz target has its own.
pub trait PageStore: Send + 'static {
    /// Puts page `page`'s characters, as the index keeps them (`recto text`'s numbering), in `chars`. False means the page has no text to map against (no text, failed, or unreadable), and it maps nothing.
    fn text(&mut self, page: u32, chars: &mut Vec<char>) -> bool;
    /// Takes a page's mapped blocks: once a page, in page order. `Err(code)` stops the job with that code.
    ///
    /// # Errors
    ///
    /// A code of the store's own.
    fn put(&mut self, page: &PageBlocks) -> Result<(), &'static str>;
}

/// What a read of blocks came to, over every page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlocksTally {
    pub pages: u32,
    /// Pages whose text the store couldn't give: they mapped nothing.
    pub pages_without_text: u32,
    /// Stored, by [`Shape::index`].
    pub stored: [u32; Shape::COUNT],
    pub dropped: [u32; Shape::COUNT],
    pub unreadable: u32,
    pub why: Drops,
    /// Milliseconds spent reading page text, mapping and storing.
    pub map_ms: u64,
}

/// What the helper has said of a document's blocks, as far as it's been believed.
pub struct BlocksAnswer<S: PageStore> {
    store: S,
    /// The document's pages as the index knows them.
    pages: u32,
    lines: PageLines,
    /// Page lines taken so far: the next page line must be this one.
    next: u32,
    /// Lines taken so far (of any kind), so a refusal is only ever the first.
    taken: u32,
    chars: Vec<char>,
    tally: BlocksTally,
    candidates: u64,
    spent: Duration,
    bytes: u64,
    stopped: Option<&'static str>,
    /// The end line's numbers, once it's been believed.
    end: Option<Map<String, Value>>,
}

impl<S: PageStore> BlocksAnswer<S> {
    /// An answer for a document of `pages` pages as the index knows them, handing each page's blocks to `store`.
    pub fn new(store: S, pages: u32) -> Self {
        Self {
            store,
            pages,
            lines: PageLines::new(pages),
            next: 0,
            taken: 0,
            chars: Vec::new(),
            tally: BlocksTally::default(),
            candidates: 0,
            spent: Duration::ZERO,
            bytes: 0,
            stopped: None,
            end: None,
        }
    }

    /// The store, and what was counted.
    pub fn into_parts(self) -> (S, BlocksTally) {
        (self.store, self.tally)
    }

    fn page_line(&mut self, line: &[u8]) -> Option<Result<Said, &'static str>> {
        let page_line = match self.lines.read(line) {
            Ok(page_line) => page_line,
            Err(LineError::NotAPageLine) => return None,
            Err(LineError::PageOutOfRange) => return Some(self.stop("pages_differ")),
            Err(LineError::TooLong | LineError::OutOfOrder) => return Some(Err(BAD)),
        };
        // Every page once, in order: a gap or a repeat is a helper that isn't saying what it read.
        if page_line.page != self.next {
            return Some(Err(BAD));
        }
        let started = Instant::now();
        self.chars.clear();
        let has_text = self.store.text(page_line.page, &mut self.chars);
        if !has_text {
            self.chars.clear();
            self.tally.pages_without_text = self.tally.pages_without_text.saturating_add(1);
        }
        let mapped = map_line(&self.chars, &page_line);
        let put = self.store.put(&mapped.blocks);
        self.spent += started.elapsed();
        if let Err(code) = put {
            return Some(self.stop(code));
        }
        self.candidates += page_line.candidates.len() as u64;
        self.tally.add(&mapped.counts);
        self.next += 1;
        Some(Ok(Said::More))
    }

    fn stop(&mut self, code: &'static str) -> Result<Said, &'static str> {
        self.stopped = Some(code);
        Err(code)
    }

    fn end_line(&mut self, line: &Map<String, Value>) -> Result<Said, &'static str> {
        let number = |name: &str| line.get(name).and_then(Value::as_u64).and_then(|value| u32::try_from(value).ok());
        let pages = number("pages").ok_or(BAD)?;
        // The helper's count of pages, the lines it sent and the index's own must be the same number.
        if pages != self.next || pages != self.pages {
            return self.stop("pages_differ");
        }
        let failed = number("pages_failed").filter(|failed| *failed <= pages).ok_or(BAD)?;
        let counted = line.get("blocks").and_then(Value::as_u64).ok_or(BAD)?;
        let sent = self.candidates + u64::from(self.tally.unreadable);
        let plain = line.len() <= MAX_NUMBERS && line.iter().all(|(name, value)| name == "state" || value.is_number() || value.is_boolean());
        let own = super::SESSION_NUMBERS.iter().any(|name| line.contains_key(*name));
        if counted != sent || !line.get("cut").is_some_and(Value::is_number) || !plain || own || failed > pages {
            return Err(BAD);
        }
        // Only numbers go on, for measuring. Nothing the helper wrote is repeated as words.
        self.end = Some(
            line.iter()
                .filter(|(_, value)| value.is_number() || value.is_boolean())
                .map(|(name, value)| (recto_core::clean_line(name).chars().take(40).collect(), value.clone()))
                .collect(),
        );
        self.tally.pages = pages;
        Ok(Said::End)
    }
}

impl<S: PageStore> std::fmt::Debug for BlocksAnswer<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlocksAnswer")
            .field("pages", &self.pages)
            .field("next", &self.next)
            .field("tally", &self.tally)
            .finish_non_exhaustive()
    }
}

impl BlocksTally {
    fn add(&mut self, counts: &recto_index::blocks::map::Counts) {
        for (to, from) in self.stored.iter_mut().zip(counts.stored) {
            *to = to.saturating_add(from);
        }
        for (to, from) in self.dropped.iter_mut().zip(counts.dropped) {
            *to = to.saturating_add(from);
        }
        self.unreadable = self.unreadable.saturating_add(counts.unreadable);
        let (why, more) = (&mut self.why, counts.why);
        for (to, from) in [
            (&mut why.not_stored_kind, more.not_stored_kind),
            (&mut why.empty, more.empty),
            (&mut why.too_long, more.too_long),
            (&mut why.not_found, more.not_found),
            (&mut why.ambiguous, more.ambiguous),
            (&mut why.over_cap, more.over_cap),
            (&mut why.page_too_long, more.page_too_long),
            (&mut why.out_of_budget, more.out_of_budget),
        ] {
            *to = to.saturating_add(from);
        }
    }
}

impl<S: PageStore> Answer for BlocksAnswer<S> {
    fn take(&mut self, line: &[u8]) -> Result<Said, &'static str> {
        self.bytes += line.len() as u64 + 1;
        let said = match self.page_line(line) {
            Some(said) => said,
            None => match serde_json::from_slice::<Value>(line).ok().as_ref().and_then(Value::as_object) {
                None => Err(BAD),
                Some(object) if object.get("state").and_then(Value::as_str) == Some("ready") => self.end_line(object),
                Some(object) => super::instead(self.taken == 0, object),
            },
        };
        self.taken += 1;
        said
    }

    fn max_line_bytes(&self) -> usize {
        MAX_LINE_BYTES
    }

    fn max_answer_bytes(&self) -> u64 {
        (ANSWER_BASE + u64::from(self.pages) * ANSWER_PER_PAGE).min(ANSWER_CEILING)
    }

    fn stopped_for(&self) -> Option<&'static str> {
        self.stopped
    }

    fn numbers(&self) -> Value {
        let Some(end) = self.end.as_ref() else { return Value::Null };
        // The helper's own numbers first, and what is counted here over them.
        let mut all = end.clone();
        let tally = &self.tally;
        let mut set = |name: &str, value: u64| {
            all.insert(name.to_string(), json!(value));
        };
        set("pages", u64::from(tally.pages));
        set("pages_without_text", u64::from(tally.pages_without_text));
        set("candidates", self.candidates);
        set("stored", tally.stored.iter().map(|n| u64::from(*n)).sum());
        set(
            "dropped",
            tally.dropped.iter().map(|n| u64::from(*n)).sum::<u64>() + u64::from(tally.unreadable),
        );
        for shape in Shape::ALL {
            set(&format!("stored_{}", shape.name()), u64::from(tally.stored[shape.index()]));
            set(&format!("dropped_{}", shape.name()), u64::from(tally.dropped[shape.index()]));
        }
        let why = tally.why;
        for (name, value) in [
            ("not_stored_kind", why.not_stored_kind),
            ("empty", why.empty),
            ("too_long", why.too_long),
            ("not_found", why.not_found),
            ("ambiguous", why.ambiguous),
            ("over_cap", why.over_cap),
            ("page_too_long", why.page_too_long),
            ("out_of_budget", why.out_of_budget),
        ] {
            set(name, u64::from(value));
        }
        set("unreadable", u64::from(tally.unreadable));
        set("map_ms", self.spent.as_millis() as u64);
        set("answer_bytes", self.bytes);
        Value::Object(all)
    }
}

/// What the whole of an answer comes to, read from bytes the way the live reader reads them (a hello, page lines, an end), for the fuzz target and the tests. `Err` is why it wasn't believed:
/// the answer's own reason, the reader's `bad_answer`, or a helper that said it refused or changed, or stopped with no end.
///
/// # Errors
///
/// A code.
pub fn read_all<S: PageStore>(said: &[u8], pages: u32, store: S) -> Result<(S, BlocksTally, Value), &'static str> {
    let (answer, end, bad) = super::read_bytes(said, BlocksAnswer::new(store, pages));
    if let Some(code) = answer.stopped_for() {
        return Err(code);
    }
    if bad {
        return Err(BAD);
    }
    match end {
        Some(Said::End) => {
            let numbers = answer.numbers();
            let (store, tally) = answer.into_parts();
            Ok((store, tally, numbers))
        }
        Some(Said::Refused(_)) => Err("refused"),
        Some(Said::Changed) => Err("changed"),
        _ => Err("helper_stopped"),
    }
}

#[cfg(test)]
mod tests;
