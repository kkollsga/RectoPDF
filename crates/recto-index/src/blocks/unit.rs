//! **The unit an agent asks for: a paragraph, a section or a chapter of a file's own text** (`docs/design/blocks.md` §3, §4 and "S5, settled"). Pure: no I/O, no session. What reads pages is
//! behind [`Pages`], one page at a time, so this is tested without a file, a helper or the app.
//!
//! - **Characters come from the page's text, in its order.** A unit is a run of `recto text`'s own characters, page by page, never in the order distillPDF read them in. Blocks only say where pieces
//!   start and end and what kind each is, so "the rest after a cut" is exact: no character is given twice or lost.
//! - **Nothing between blocks is dropped.** Text that no stored block covers (a dropped paragraph, a table's words, a running head) is given as a plain paragraph piece and counted.
//! - **A section runs from its entry to the next entry of the same or a shallower depth,** cut on a page at the first stored block, in character order, that is at or after the entry's place. A boundary
//!   page with no stored blocks at all is given whole (more text, never lost text) and counted.
//! - **One sweep a page.** A page's entries are found by one search ([`Sections::starting_on`]) and their cut blocks by one merge with the page's blocks: no walk of every entry for a block.
//! - **The cut** is at the end of the last whole piece that fits; a first piece over the cap alone is cut at its last line end within the cap, else at the cap.
//! - **Bounded:** at most [`MAX_PAGES_READ`] pages are read for an answer (the ones looked at for its neighbours too), and one page more only to name where it goes on (65 reads at most; pages the index says are blank are passed over without a read, at most 8), live reads stop when [`Pages::over`] says their time is spent, and one page's text
//!   is held at a time with what's given (at most the caller's cap, 65,536 characters).

use super::markdown::Piece;
use super::sections::{Place, Sections};
use super::{Block, Kind};
use recto_core::locator::{CharacterRange, text_check};
use recto_core::nav::Frame;

/// Most pages read for one answer, the ones looked at for its neighbours included.
pub const MAX_PAGES_READ: u32 = 64;
/// How many characters a reference to a unit's start, or to where it goes on, names.
pub const REF_CHARS: u32 = 32;
/// Most headings above a unit that are named.
pub const MAX_HEADINGS_ABOVE: usize = 32;
/// Pages read only to name where an answer goes on, past [`MAX_PAGES_READ`]: exactly one at most, never more. Its text is not given; the reference to it needs its characters' check.
const NAMING_READS: u32 = 1;
/// Blank pages (the index says no text) passed over, at most, while naming where an answer goes on past the page cap: 8. They cost no read.
const BLANK_PAGES_PASSED: u32 = 8;
/// A page's buffers are given back past this many characters.
const BIG_PAGE: usize = 1 << 20;

/// What the unit is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grain {
    Paragraph,
    Section,
    Chapter,
}

/// A stored page's blocks that don't open as this page's: the set is stale or damaged and is thrown away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault;

/// What one page of the file gives, one at a time. The caller implements it over the index, the stored blocks and the tab.
pub trait Pages {
    /// How many pages the file has.
    fn pages(&self) -> u32;
    /// The page's characters and hidden stretches (in characters), into the buffers, which are cleared first. False when the page's text can't be read.
    fn read(&mut self, page: u32, chars: &mut Vec<char>, hidden: &mut Vec<(u32, u32)>) -> bool;
    /// The page's stored blocks, for a page whose text is `chars` characters long now.
    ///
    /// # Errors
    ///
    /// [`Fault`] when they don't open as this page's.
    fn blocks(&mut self, page: u32, chars: u32) -> Result<Vec<Block>, Fault>;
    /// How the page is shown, or `None` when it can't be told.
    fn frame(&mut self, page: u32) -> Option<Frame>;
    /// Whether the tab shows this page (unsaved edits may have removed it).
    fn shown(&self, page: u32) -> bool;
    /// Whether the time given to reading pages live is spent.
    fn over(&mut self) -> bool;
    /// Whether the page is known to have text, without reading it (the index's own record of it). Used to pass over blank pages when naming where an answer goes on. True when it can't be told.
    fn has_text(&mut self, _page: u32) -> bool {
        true
    }
}

/// What is asked.
#[derive(Clone, Copy)]
pub struct Ask<'a> {
    pub grain: Grain,
    /// The reference's page, from 0.
    pub page: u32,
    /// The reference's first character, or `None` for a reference to a page.
    pub start: Option<u32>,
    /// Start at the reference's first character, not the unit's.
    pub from_here: bool,
    pub max_chars: u32,
    /// The entry the `more` being followed came from (`--from-here` at section grain): used when it still holds the reference's page, so going on continues the unit it came from and not the innermost
    /// section the character is in. A number from outside: it must be an entry of these contents whose range reaches the reference's page, or it's ignored.
    pub entry: Option<u32>,
    /// The contents the sections come from. A section or a chapter needs them; a paragraph names the headings above it by them if it has them.
    pub sections: Option<&'a Sections>,
}

/// Why the page is answered, not the unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// No stored block holds the reference's first character (or the page has none).
    NoBlockHere,
    /// No entry of the contents holds it, or the entry's range runs backwards.
    NoSection,
}

/// One piece given: a block, or the text between blocks as a paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Given {
    pub page: u32,
    pub kind: Kind,
    pub level: u8,
    /// The depth of the contents entry this heading is the start of.
    pub depth: Option<u8>,
    pub text: Vec<char>,
    /// Hidden stretches, in characters of `text`.
    pub hidden: Vec<(u32, u32)>,
    /// This piece is the rest of the paragraph the piece before it began, on a later page ([`Block::continues`]): one paragraph in the answer, not two.
    pub joins: bool,
}

/// A place to go to: a run of one page's characters, with their check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub page: u32,
    pub range: CharacterRange,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    pub this: Option<Mark>,
    pub next: Option<Mark>,
    pub prev: Option<Mark>,
    /// Where it goes on, when it was cut.
    pub more: Option<Mark>,
    /// What to ask for at the next coarser grain.
    pub up: Option<Mark>,
}

/// What an answer says of how it was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Blocks among the pieces given.
    pub blocks: u32,
    /// Characters given that no stored block covers.
    pub chars_outside_blocks: u32,
    pub pages_read: u32,
    /// Pages of the unit left out: removed from the tab by edits, or their text couldn't be read.
    pub pages_left_out: u32,
    /// Boundary pages with no stored blocks, given whole (0 to 2).
    pub whole_edge_pages: u32,
    /// Pages whose blocks were placed in the middle for want of a frame.
    pub pages_without_frame: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Unit {
    pub given: Vec<Given>,
    pub chars_given: u32,
    /// The characters of the unit on the pages read.
    pub chars: u32,
    /// How many pages the unit spans by its entries.
    pub pages: u32,
    pub truncated: bool,
    /// A paragraph that goes on at the top of the next page.
    pub continues: bool,
    /// The contents entry of a section or a chapter.
    pub entry: Option<usize>,
    /// The entries whose sections hold the reference, outermost first.
    pub headings: Vec<usize>,
    pub refs: Refs,
    pub counts: Counts,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Unit(Box<Unit>),
    /// The page is answered, for this reason.
    Page(Why),
}

#[cfg(test)]
thread_local! {
    static STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One step of work in the sweep or in a walk of the entries, counted for a test that holds the cost to the page's entries and blocks.
#[cfg(test)]
pub(crate) fn count_step() {
    STEPS.with(|steps| steps.set(steps.get() + 1));
}

/// The unit asked for.
///
/// # Errors
///
/// [`Fault`] when a page's blocks don't open as it: they are stale or damaged.
pub fn build<P: Pages>(pages: &mut P, ask: &Ask<'_>) -> Result<Outcome, Fault> {
    let mut walk = Walk::new(pages);
    match ask.grain {
        Grain::Paragraph => walk.paragraph(ask),
        Grain::Section | Grain::Chapter => walk.section(ask),
    }
}

/// The headings above `place`: the entries whose sections hold it, outermost first, at most [`MAX_HEADINGS_ABOVE`] (the innermost are kept).
#[must_use]
pub fn headings_above(sections: &Sections, place: Place) -> Vec<usize> {
    let mut chain = sections.headings_above(place);
    if chain.len() > MAX_HEADINGS_ABOVE {
        chain.drain(..chain.len() - MAX_HEADINGS_ABOVE);
    }
    chain
}

/// The blocks' first characters, one for each block that is first in character order at or after each of `starts` (ascending): the index of the first block whose place is at or after it, or
/// `places.len()` for none. One pass over both, so the cost is theirs added and not multiplied. The second number is the steps it took.
#[must_use]
pub fn sweep(places: &[Place], starts: &[Place]) -> (Vec<usize>, u64) {
    let (mut at, mut steps) = (0usize, 0u64);
    let mut highest: Option<Place> = None;
    let mut out = Vec::with_capacity(starts.len());
    for start in starts {
        // The first block whose place is at or after `start` is where the running highest place first reaches it.
        while let Some(place) = places.get(at) {
            let high = highest.map_or(*place, |seen| seen.max(*place));
            if high >= *start {
                break;
            }
            highest = Some(high);
            at += 1;
            steps += 1;
        }
        out.push(at);
        steps += 1;
        #[cfg(test)]
        count_step();
    }
    #[cfg(test)]
    STEPS.with(|total| total.set(total.get() + places.len() as u64));
    (out, steps)
}

/// The steps counted on this thread since the last call.
#[cfg(test)]
pub(crate) fn take_steps() -> u64 {
    STEPS.with(|steps| steps.replace(0))
}

/// Where the text is cut when one piece is over the cap: at the last line end inside the cap, and at the cap if there is none.
#[must_use]
pub fn cut_at(chars: &[char], max: usize) -> usize {
    if chars.len() <= max {
        return chars.len();
    }
    chars[..max].iter().rposition(|c| *c == '\n').filter(|at| *at > 0).unwrap_or(max)
}

/// The hidden stretches that fall inside `[from, to)`, counted from `from`.
#[must_use]
pub fn clip_spans(spans: &[(u32, u32)], from: u32, to: u32) -> Vec<(u32, u32)> {
    spans
        .iter()
        .filter_map(|&(start, end)| {
            let (start, end) = (start.max(from), end.min(to));
            (start < end).then(|| (start - from, end - from))
        })
        .collect()
}

/// The pieces as Markdown. A piece that [`Given::joins`] the one before it is the rest of that paragraph (or list item), so it goes on in the same block, on the next line, and isn't a block of its own.
#[must_use]
pub fn as_markdown(given: &[Given]) -> String {
    let mut merged: Vec<(Kind, u8, Option<u8>, Vec<char>, Vec<(u32, u32)>)> = Vec::new();
    for one in given {
        match merged.last_mut() {
            Some(last) if one.joins && last.0 == one.kind => {
                last.3.push('\n');
                let at = last.3.len() as u32;
                last.4.extend(one.hidden.iter().map(|&(start, end)| (start + at, end + at)));
                last.3.extend(one.text.iter());
            }
            _ => merged.push((one.kind, one.level, one.depth, one.text.clone(), one.hidden.clone())),
        }
    }
    let pieces: Vec<Piece<'_>> = merged
        .iter()
        .map(|(kind, level, depth, text, hidden)| Piece {
            kind: *kind,
            level: *level,
            depth: *depth,
            text,
            hidden,
        })
        .collect();
    super::markdown::render(&pieces)
}

/// The pieces as plain text: the pieces end to end, with one `\n` where a page ends and the next begins. The hidden stretches are in characters of that text.
#[must_use]
pub fn as_text(given: &[Given]) -> (String, Vec<(u32, u32)>) {
    let (mut text, mut hidden, mut at) = (String::new(), Vec::new(), 0u32);
    let mut page = None;
    for one in given {
        if page.is_some_and(|before| before != one.page) {
            text.push('\n');
            at += 1;
        }
        page = Some(one.page);
        hidden.extend(one.hidden.iter().map(|&(start, end)| (start + at, end + at)));
        text.extend(one.text.iter());
        at += one.text.len() as u32;
    }
    (text, hidden)
}

/// A piece of a page to give: block `block` of the page, or the text between blocks (`None`).
struct Spec {
    kind: Kind,
    level: u8,
    start: u32,
    end: u32,
    block: Option<usize>,
}

/// What an answer has taken in so far.
#[derive(Default)]
struct Taken {
    given: Vec<Given>,
    chars_given: u32,
    /// Where it was cut: a page and a character on it.
    cut: Option<(u32, u32)>,
    /// The first character given: the unit's own start.
    first: Option<Mark>,
}

/// Where a paragraph's walk has got to: the last page it reached, and the block after that block on that page (made while the page was loaded).
struct Tail {
    page: u32,
    next: Option<Mark>,
    has_next: bool,
}

/// What a paragraph's walk has taken in so far.
struct Walked {
    chars: u32,
    pages: u32,
    continues: bool,
    truncated: bool,
}

/// The next page with blocks, as [`Walk::page_after_with_blocks`] finds it.
enum Found {
    At(u32),
    /// No page after it has blocks.
    End,
    /// Reading stopped (the page cap, the live time, or too many pages with no text) at this page.
    Stopped(u32),
}

/// Whether a block of this kind, going on at the top of a page, is one paragraph with its rest.
const fn joins_across_pages(kind: Kind) -> bool {
    matches!(kind, Kind::Paragraph | Kind::ListItem | Kind::Caption | Kind::Footnote)
}

struct Walk<'a, P: Pages> {
    pages: &'a mut P,
    /// The page whose text and blocks are in the buffers.
    loaded: Option<u32>,
    chars: Vec<char>,
    hidden: Vec<(u32, u32)>,
    /// The loaded page's blocks in character order, and where each is.
    blocks: Vec<Block>,
    places: Vec<Place>,
    counts: Counts,
    naming_reads: u32,
}

impl<'a, P: Pages> Walk<'a, P> {
    fn new(pages: &'a mut P) -> Self {
        Self {
            pages,
            loaded: None,
            chars: Vec::new(),
            hidden: Vec::new(),
            blocks: Vec::new(),
            places: Vec::new(),
            counts: Counts::default(),
            naming_reads: 0,
        }
    }

    /// Reads a page into the buffers, unless it's the one there. False when its text can't be read (the page is counted left out).
    fn load(&mut self, page: u32) -> Result<bool, Fault> {
        if self.loaded == Some(page) {
            return Ok(true);
        }
        self.loaded = None;
        self.blocks.clear();
        self.places.clear();
        if self.chars.capacity() > BIG_PAGE {
            self.chars = Vec::new();
        }
        self.counts.pages_read += 1;
        if !self.pages.read(page, &mut self.chars, &mut self.hidden) {
            self.counts.pages_left_out += 1;
            return Ok(false);
        }
        let length = u32::try_from(self.chars.len()).unwrap_or(u32::MAX);
        let mut blocks = self.pages.blocks(page, length)?;
        blocks.retain(|block| block.start < block.end && block.end <= length);
        blocks.sort_unstable_by_key(|block| (block.start, block.end));
        let frame = self.pages.frame(page);
        if frame.is_none() && !blocks.is_empty() {
            self.counts.pages_without_frame += 1;
        }
        self.places = blocks
            .iter()
            .map(|block| frame.map_or(Place::middle(page), |frame| Place::of_block(block, frame)))
            .collect();
        self.blocks = blocks;
        self.loaded = Some(page);
        Ok(true)
    }

    /// Whether another page may be read now: under the cap, and with live time left.
    fn can_read(&mut self) -> bool {
        self.counts.pages_read < MAX_PAGES_READ && !self.pages.over()
    }

    /// A page read only to say where to go: past the cap, a few at most.
    fn load_to_name(&mut self, page: u32) -> Result<bool, Fault> {
        if self.loaded == Some(page) {
            return Ok(true);
        }
        if self.naming_reads >= NAMING_READS {
            return Ok(false);
        }
        self.naming_reads += 1;
        self.load(page)
    }

    fn len(&self) -> u32 {
        u32::try_from(self.chars.len()).unwrap_or(u32::MAX)
    }

    /// A reference to characters `[start, end)` of the loaded page.
    fn mark(&self, start: u32, end: u32) -> Option<Mark> {
        let page = self.loaded?;
        let held: String = self.chars.get(start as usize..end as usize)?.iter().collect();
        let range = CharacterRange::new(start, end, text_check(&held)).ok()?;
        Some(Mark { page, range })
    }

    /// A reference to up to [`REF_CHARS`] characters from `start` on the loaded page.
    fn mark_from_here(&self, start: u32) -> Option<Mark> {
        self.mark(start, start.saturating_add(REF_CHARS).min(self.len()))
    }

    /// A reference to where text begins at or after character `start` of `page`: on that page, or at the first page after it (as far as `last`) that has text.
    fn mark_from(&mut self, page: u32, start: u32, last: u32, naming: bool) -> Result<Option<Mark>, Fault> {
        let mut page = page;
        let mut start = start;
        let mut blank = 0;
        while page <= last && page < self.pages.pages() {
            if self.pages.shown(page) {
                // Naming where to go on: a page the index says has no text isn't read to find that out. At most [`BLANK_PAGES_PASSED`] are passed.
                if naming && self.loaded != Some(page) && !self.pages.has_text(page) {
                    blank += 1;
                    if blank > BLANK_PAGES_PASSED {
                        return Ok(None);
                    }
                    page += 1;
                    start = 0;
                    continue;
                }
                let ready = if naming { self.load_to_name(page)? } else { self.can_read_then_load(page)? };
                if ready && start < self.len() {
                    return Ok(self.mark_from_here(start));
                }
                if !ready && self.loaded != Some(page) && !self.page_is_left_out(page) {
                    return Ok(None);
                }
            }
            page += 1;
            start = 0;
        }
        Ok(None)
    }

    fn page_is_left_out(&self, page: u32) -> bool {
        !self.pages.shown(page)
    }

    fn can_read_then_load(&mut self, page: u32) -> Result<bool, Fault> {
        if self.loaded == Some(page) {
            return Ok(true);
        }
        if !self.can_read() {
            return Ok(false);
        }
        self.load(page)
    }

    /// The first block, in character order, whose place is at or after `place`: its index, or the number of blocks for none.
    fn first_at_or_after(&self, place: Place) -> usize {
        self.places.iter().position(|seen| *seen >= place).unwrap_or(self.places.len())
    }

    /// Where on the loaded page a cut for `place` is: the first character of that block, or the page's end.
    fn cut_for(&self, place: Place) -> u32 {
        self.blocks.get(self.first_at_or_after(place)).map_or_else(|| self.len(), |block| block.start)
    }

    /// The place the reference's first character is in, for choosing its section. **Sections are cut by character order**, so on the reference's page it is the start of the last entry whose cut is at
    /// or before the character (a block can sit higher on the page than its entry, in another column, and its own place would pick the section before). With none, the top of the page.
    fn anchor_place(&self, sections: &Sections, page: u32, start: Option<u32>) -> Place {
        let top = Place { page, top: 0 };
        let Some(first) = start else { return top };
        let entries = sections.starting_on(page);
        let starts: Vec<Place> = entries.iter().map(|(place, _)| *place).collect();
        let (cuts, _) = sweep(&self.places, &starts);
        let at = |cut: usize| self.blocks.get(cut).map_or_else(|| self.len(), |block| block.start);
        let held = entries
            .iter()
            .zip(cuts)
            .take_while(|(_, cut)| at(*cut) <= first)
            .last()
            .map(|((place, _), _)| *place);
        // A character before the page's first cut is in the section just before that entry, which may have started on an earlier page.
        held.or_else(|| {
            let (first_entry, _) = entries.first()?;
            match (first_entry.top, page) {
                (0, 0) => None,
                (0, _) => Some(Place {
                    page: page - 1,
                    top: recto_core::TOP_SCALE,
                }),
                (up, _) => Some(Place { page, top: up - 1 }),
            }
        })
        .unwrap_or(top)
    }

    /// The loaded page's pieces over characters `[from, to)`: blocks, and the text between them as paragraphs.
    fn specs(&self, from: u32, to: u32) -> Vec<Spec> {
        let mut out = Vec::new();
        let mut cursor = from;
        let gap = |start: u32, end: u32| Spec {
            kind: Kind::Paragraph,
            level: 0,
            start,
            end,
            block: None,
        };
        for (index, block) in self.blocks.iter().enumerate() {
            if cursor >= to || block.start >= to {
                break;
            }
            if block.end <= cursor {
                continue;
            }
            let start = block.start.max(cursor);
            if start > cursor {
                out.push(gap(cursor, start));
            }
            let end = block.end.min(to);
            out.push(Spec {
                kind: block.kind,
                level: block.level,
                start,
                end,
                block: Some(index),
            });
            cursor = end;
        }
        if cursor < to {
            out.push(gap(cursor, to));
        }
        out
    }

    /// The depth of the entry each heading block of the loaded page is the cut block of: one search for the page's entries and one merge with its blocks.
    fn heading_depths(&self, sections: Option<&Sections>, page: u32) -> Vec<Option<u8>> {
        let mut depths = vec![None; self.blocks.len()];
        let Some(sections) = sections else { return depths };
        let entries = sections.starting_on(page);
        if entries.is_empty() {
            return depths;
        }
        let starts: Vec<Place> = entries.iter().map(|(place, _)| *place).collect();
        let (cuts, _) = sweep(&self.places, &starts);
        for ((_, entry), cut) in entries.iter().zip(cuts) {
            if let Some(block) = self.blocks.get(cut)
                && block.kind == Kind::Heading
            {
                depths[cut] = sections.depth(*entry as usize);
            }
        }
        depths
    }

    /// Takes in a page's pieces until the cap is reached. Records where it was cut.
    fn give(&mut self, taken: &mut Taken, specs: &[Spec], depths: &[Option<u8>], max: usize) {
        let page = self.loaded.unwrap_or(0);
        for spec in specs {
            let len = (spec.end - spec.start) as usize;
            let room = max - taken.chars_given as usize;
            let first = taken.given.is_empty();
            let take = if len <= room {
                len
            } else if first {
                cut_at(&self.chars[spec.start as usize..spec.end as usize], max)
            } else {
                taken.cut = Some((page, spec.start));
                return;
            };
            let end = spec.start + take as u32;
            if taken.first.is_none() {
                taken.first = self.mark_from_here(spec.start);
            }
            let kind_outside = spec.block.is_none();
            let depth = spec.block.and_then(|index| depths[index]);
            taken.given.push(Given {
                page,
                kind: spec.kind,
                level: spec.level,
                depth,
                text: self.chars[spec.start as usize..end as usize].to_vec(),
                hidden: clip_spans(&self.hidden, spec.start, end),
                joins: false,
            });
            taken.chars_given = taken.chars_given.saturating_add(take as u32);
            if kind_outside {
                self.counts.chars_outside_blocks = self.counts.chars_outside_blocks.saturating_add(take as u32);
            } else {
                self.counts.blocks += 1;
            }
            if take < len {
                taken.cut = Some((page, end));
                return;
            }
        }
    }

    fn headings_above(sections: Option<&Sections>, place: Place) -> Vec<usize> {
        sections.map_or_else(Vec::new, |sections| headings_above(sections, place))
    }

    fn paragraph(&mut self, ask: &Ask<'_>) -> Result<Outcome, Fault> {
        if !self.load(ask.page)? {
            return Ok(Outcome::Page(Why::NoBlockHere));
        }
        let held = match ask.start {
            None => (!self.blocks.is_empty()).then_some(0),
            Some(first) => self.blocks.iter().position(|block| block.start <= first && first < block.end),
        };
        let Some(index) = held else { return Ok(Outcome::Page(Why::NoBlockHere)) };
        let block = self.blocks[index];
        let from = match ask.start {
            Some(first) if ask.from_here => first,
            _ => block.start,
        };
        let depths = self.heading_depths(ask.sections, ask.page);
        let specs = [Spec {
            kind: block.kind,
            level: block.level,
            start: from,
            end: block.end,
            block: Some(index),
        }];
        let mut taken = Taken::default();
        self.give(&mut taken, &specs, &depths, ask.max_chars as usize);
        let place = self.places[index];
        let mut refs = Refs {
            this: self.mark(block.start, block.end),
            more: taken.cut.and_then(|(_, at)| self.mark_from_here(at)),
            ..Refs::default()
        };
        refs.up = refs.this;
        let before = index.checked_sub(1).map(|at| self.blocks[at]);
        let after = self.blocks.get(index + 1).copied();
        refs.prev = before.and_then(|prev| self.mark(prev.start, prev.end));
        let mut tail = Tail {
            page: ask.page,
            next: after.and_then(|next| self.mark(next.start, next.end)),
            has_next: after.is_some(),
        };
        let mut walked = Walked {
            chars: block.end - from,
            pages: 1,
            continues: block.continues,
            truncated: taken.cut.is_some(),
        };
        if block.continues && taken.cut.is_none() && joins_across_pages(block.kind) {
            self.join_on(ask, &block, &mut taken, &mut tail, &mut walked, &mut refs)?;
        }
        if before.is_none() {
            refs.prev = self.neighbour_block(ask.page, false)?;
        }
        // After the last piece reached: the block after it on its page, else the first block of a later page.
        refs.next = if tail.has_next { tail.next } else { self.neighbour_block(tail.page, true)? };
        Ok(Outcome::Unit(Box::new(Unit {
            chars: walked.chars,
            chars_given: taken.chars_given,
            given: taken.given,
            pages: walked.pages,
            truncated: walked.truncated,
            continues: walked.continues,
            entry: None,
            headings: Self::headings_above(ask.sections, place),
            refs,
            counts: self.counts,
        })))
    }

    /// Follows a paragraph that goes on at the top of a later page, piece by piece, until its last piece, the end of what's read (the cap on pages read, the live time, the caller's characters) or a page that
    /// doesn't go on with the same kind of block. **Bounded by the reads, not by what a store says:** a store that sets `continues` on every page is followed only as far as the page cap lets pages be read.
    fn join_on(&mut self, ask: &Ask<'_>, block: &Block, taken: &mut Taken, tail: &mut Tail, walked: &mut Walked, refs: &mut Refs) -> Result<(), Fault> {
        loop {
            let at = match self.page_after_with_blocks(tail.page)? {
                Found::At(at) => at,
                Found::End => {
                    walked.continues = false;
                    return Ok(());
                }
                Found::Stopped(page) => {
                    // The next piece isn't read: its place is named, with the one read past the cap that naming is allowed.
                    walked.truncated = true;
                    refs.more = if self.load_to_name(page)? {
                        self.blocks.first().copied().and_then(|first| self.mark(first.start, first.end))
                    } else {
                        None
                    };
                    return Ok(());
                }
            };
            let Some(first) = self.blocks.first().copied().filter(|first| first.kind == block.kind) else {
                // The page begins with something else: this paragraph ended on the page before.
                walked.continues = false;
                return Ok(());
            };
            *tail = Tail {
                page: at,
                next: self.blocks.get(1).and_then(|next| self.mark(next.start, next.end)),
                has_next: self.blocks.len() > 1,
            };
            walked.pages = at - ask.page + 1;
            walked.chars = walked.chars.saturating_add(first.end - first.start);
            walked.continues = first.continues;
            let before = taken.given.len();
            let depths = vec![None; self.blocks.len()];
            let specs = [Spec {
                kind: first.kind,
                level: first.level,
                start: first.start,
                end: first.end,
                block: Some(0),
            }];
            self.give(taken, &specs, &depths, ask.max_chars as usize);
            if let Some(joined) = taken.given.get_mut(before) {
                joined.joins = true;
            }
            if let Some((_, cut)) = taken.cut {
                walked.truncated = true;
                refs.more = self.mark_from_here(cut);
                return Ok(());
            }
            if !first.continues {
                return Ok(());
            }
        }
    }

    /// The next page after `page` that has stored blocks, loaded: pages the tab doesn't show and pages with no text are passed over, and the cap on pages read and the live time stop it.
    fn page_after_with_blocks(&mut self, page: u32) -> Result<Found, Fault> {
        let count = self.pages.pages();
        let (mut at, mut blank) = (page, 0);
        loop {
            at += 1;
            if at >= count {
                return Ok(Found::End);
            }
            if !self.pages.shown(at) {
                continue;
            }
            if self.loaded != Some(at) {
                if !self.pages.has_text(at) {
                    blank += 1;
                    if blank > BLANK_PAGES_PASSED {
                        return Ok(Found::Stopped(at));
                    }
                    continue;
                }
                if !self.can_read() {
                    return Ok(Found::Stopped(at));
                }
            }
            if self.load(at)? && !self.blocks.is_empty() {
                return Ok(Found::At(at));
            }
        }
    }

    /// The first block of the next page, or the last of the one before, that has any: skipping pages with none and pages the tab doesn't show, as far as pages may be read.
    fn neighbour_block(&mut self, page: u32, forward: bool) -> Result<Option<Mark>, Fault> {
        let count = self.pages.pages();
        let mut at = page;
        loop {
            at = if forward {
                at + 1
            } else {
                match at.checked_sub(1) {
                    Some(before) => before,
                    None => return Ok(None),
                }
            };
            if at >= count {
                return Ok(None);
            }
            if !self.pages.shown(at) {
                continue;
            }
            if !self.can_read() {
                return Ok(None);
            }
            if !self.load(at)? {
                continue;
            }
            let block = if forward { self.blocks.first() } else { self.blocks.last() }.copied();
            if let Some(block) = block {
                return Ok(self.mark(block.start, block.end));
            }
        }
    }

    fn section(&mut self, ask: &Ask<'_>) -> Result<Outcome, Fault> {
        let Some(sections) = ask.sections else {
            return Ok(Outcome::Page(Why::NoSection));
        };
        if !self.load(ask.page)? {
            return Ok(Outcome::Page(Why::NoBlockHere));
        }
        let named = ask
            .entry
            .filter(|_| ask.from_here && ask.grain == Grain::Section)
            .map(|entry| entry as usize)
            .filter(|entry| {
                sections
                    .range(*entry)
                    .is_some_and(|(start, end)| start.page <= ask.page && end.is_none_or(|end| ask.page <= end.page))
            });
        let place = match named.and_then(|entry| sections.range(entry)) {
            Some((start, _)) => start,
            None => self.anchor_place(sections, ask.page, ask.start),
        };
        let entry = match (named, ask.grain) {
            (Some(entry), _) => Some(entry),
            (None, Grain::Chapter) => sections.chapter_of(place),
            (None, _) => sections.section_of(place),
        };
        let Some(entry) = entry else { return Ok(Outcome::Page(Why::NoSection)) };
        let Some((start, end)) = sections.range(entry) else {
            return Ok(Outcome::Page(Why::NoSection));
        };
        let count = self.pages.pages();
        let last = end.map_or(count.saturating_sub(1), |end| end.page.min(count.saturating_sub(1)));
        if end.is_some_and(|end| end < start) || start.page >= count {
            return Ok(Outcome::Page(Why::NoSection));
        }
        let headings = Self::headings_above(Some(sections), place);
        let first_page = match ask.start {
            Some(_) if ask.from_here => ask.page.max(start.page),
            _ => start.page,
        };
        let mut taken = Taken::default();
        let mut unit_chars = 0u32;
        let mut stopped_at = None;
        let mut page = first_page;
        while page <= last && taken.cut.is_none() {
            if !self.pages.shown(page) {
                self.counts.pages_left_out += 1;
                page += 1;
                continue;
            }
            if self.loaded != Some(page) && !self.can_read() {
                stopped_at = Some(page);
                break;
            }
            if !self.load(page)? {
                page += 1;
                continue;
            }
            let len = self.len();
            let (mut from, mut to) = (0, len);
            let (edge_start, edge_end) = (page == start.page, end.is_some() && page == last);
            if (edge_start || edge_end) && self.blocks.is_empty() {
                self.counts.whole_edge_pages += 1;
            } else {
                if edge_start {
                    from = self.cut_for(start);
                }
                if let (true, Some(end)) = (edge_end, end) {
                    to = self.cut_for(end);
                }
            }
            if let (true, Some(first), true) = (ask.from_here, ask.start, page == ask.page) {
                from = first.min(len);
            }
            to = to.max(from);
            unit_chars = unit_chars.saturating_add(to - from);
            let depths = self.heading_depths(Some(sections), page);
            let specs = self.specs(from, to);
            self.give(&mut taken, &specs, &depths, ask.max_chars as usize);
            page += 1;
        }
        let mut refs = Refs {
            this: taken.first,
            ..Refs::default()
        };
        let mut truncated = taken.cut.is_some();
        if let Some((cut_page, at)) = taken.cut {
            refs.more = self.mark_from(cut_page, at, cut_page, false)?;
        } else if let Some(stopped) = stopped_at {
            truncated = true;
            refs.more = self.mark_from(stopped, 0, last, true)?;
        }
        refs.up = (ask.grain == Grain::Section).then_some(refs.this).flatten();
        let (next, prev) = sections.neighbours(entry);
        refs.next = self.entry_mark(sections, next, count)?;
        refs.prev = self.entry_mark(sections, prev, count)?;
        let spans = end.map_or(count.saturating_sub(1), |end| {
            if end.top == 0 && end.page > start.page { end.page - 1 } else { end.page }
        });
        Ok(Outcome::Unit(Box::new(Unit {
            chars_given: taken.chars_given,
            given: taken.given,
            chars: unit_chars,
            pages: spans.saturating_sub(start.page).saturating_add(1),
            truncated,
            continues: false,
            entry: Some(entry),
            headings,
            refs,
            counts: self.counts,
        })))
    }

    /// A reference to the start of another entry's section: its page's cut for the entry's place, going on to the next page with text if the cut is at the page's end.
    fn entry_mark(&mut self, sections: &Sections, entry: Option<usize>, count: u32) -> Result<Option<Mark>, Fault> {
        let Some((place, _)) = entry.and_then(|entry| sections.range(entry)) else {
            return Ok(None);
        };
        if place.page >= count || !self.pages.shown(place.page) {
            return Ok(None);
        }
        if self.loaded != Some(place.page) && !self.can_read() {
            return Ok(None);
        }
        if !self.load(place.page)? {
            return Ok(None);
        }
        // With no stored block on the page the whole page is the section's.
        let from = if self.blocks.is_empty() { 0 } else { self.cut_for(place) };
        self.mark_from(place.page, from, count.saturating_sub(1), false)
    }
}

#[cfg(test)]
mod tests;
