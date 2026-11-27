//! **Mapping distillPDF's blocks onto a page's text.** A candidate is a kind, a level, a box and its letters, as plain text already. It's stored only when
//! those letters are one unbroken run in `recto text`'s characters of the page, found by this rule (settled in `docs/design/blocks.md`, "After S0's
//! measurements"):
//!
//! 1. Look **after the previous mapped block's end**, and take the first run there that overlaps no block already mapped.
//! 2. If there's none, take the block's **one occurrence anywhere on the page** that overlaps no block already mapped.
//! 3. If the letters occur more than once in the unmapped text, or not at all, **the block isn't stored.** A repeated running head or a short phrase would map to the wrong place.
//!
//! Reading order stays distillPDF's. Letters are compared as `recto-headings` compares a heading's: letters and digits, lowercased, ligatures spelled out.
//! Tables and figures have no text to map and aren't stored. Every cap says what it dropped, never a failure: a page that maps nothing is a page with no blocks.

use super::{Block, Kind, MAX_BLOCKS, MAX_LEVEL, MAX_PAGE_CHARS, PageBlocks};
use recto_core::match_letters as letters;
use serde_json::Value;

/// Most letters in one block. A block with more isn't stored. At the limit it is.
pub const MAX_LETTERS: usize = 64 * 1024;
/// Most character comparisons looking for the blocks of one page, as `recto-headings` bounds one heading's: 2,000,000 `a`s and a block of 240 of them would otherwise be half a billion. Past this the blocks still unplaced aren't stored.
pub const MAX_COMPARISONS: usize = 20_000_000;
/// Most bytes of one page's line: 512 KiB.
pub const MAX_LINE_BYTES: usize = 512 * 1024;

/// What distillPDF calls a block, tables and figures included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Paragraph,
    Heading,
    ListItem,
    Caption,
    Code,
    Footnote,
    Table,
    Figure,
}

impl Shape {
    pub const COUNT: usize = 8;
    pub const ALL: [Shape; Self::COUNT] = [
        Self::Paragraph,
        Self::Heading,
        Self::ListItem,
        Self::Caption,
        Self::Code,
        Self::Footnote,
        Self::Table,
        Self::Figure,
    ];

    /// The kind it's stored as, if it's stored.
    #[must_use]
    pub const fn stored(self) -> Option<Kind> {
        Some(match self {
            Self::Paragraph => Kind::Paragraph,
            Self::Heading => Kind::Heading,
            Self::ListItem => Kind::ListItem,
            Self::Caption => Kind::Caption,
            Self::Code => Kind::Code,
            Self::Footnote => Kind::Footnote,
            Self::Table | Self::Figure => return None,
        })
    }

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Paragraph => 0,
            Self::Heading => 1,
            Self::ListItem => 2,
            Self::Caption => 3,
            Self::Code => 4,
            Self::Footnote => 5,
            Self::Table => 6,
            Self::Figure => 7,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Figure => "figure",
            Self::Paragraph => Kind::Paragraph.name(),
            Self::Heading => Kind::Heading.name(),
            Self::ListItem => Kind::ListItem.name(),
            Self::Caption => Kind::Caption.name(),
            Self::Code => Kind::Code.name(),
            Self::Footnote => Kind::Footnote.name(),
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "para" | "paragraph" => Self::Paragraph,
            "heading" => Self::Heading,
            "list_item" => Self::ListItem,
            "caption" => Self::Caption,
            "code" => Self::Code,
            "footnote" => Self::Footnote,
            "table" => Self::Table,
            "figure" => Self::Figure,
            _ => return None,
        })
    }
}

/// A block as distillPDF found it, before it's placed in the page's text.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub shape: Shape,
    pub level: u8,
    /// Left, bottom, right, top, as the helper says them. Not believed: [`map_page`] makes them whole points inside what a record holds.
    pub bbox: [f32; 4],
    /// The block's own text. Document content, and only its letters are used.
    pub text: String,
    pub continues: bool,
}

impl Candidate {
    /// One block of a page line. `None` for what isn't an object with a kind this knows. A level that is missing or isn't a whole number is 0; a box that isn't
    /// four numbers is empty; hostile text never panics here, it is a string and nothing of it is run.
    fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let shape = Shape::from_name(object.get("kind")?.as_str()?)?;
        let level = object
            .get("level")
            .and_then(Value::as_u64)
            .map_or(0, |level| level.min(u64::from(MAX_LEVEL)) as u8);
        let mut bbox = [0.0f32; 4];
        if let Some(sides) = object.get("box").and_then(Value::as_array).filter(|sides| sides.len() == 4) {
            for (to, side) in bbox.iter_mut().zip(sides) {
                *to = side.as_f64().unwrap_or(0.0) as f32;
            }
        }
        Some(Self {
            shape,
            level,
            bbox,
            text: object.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
            continues: object.get("continues").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Most blocks read from one page's line. Past it they are counted as unreadable, so a line of a hundred thousand empty objects costs no more than this.
pub const MAX_LINE_BLOCKS: usize = 4096;

/// Why a page's line wasn't taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineError {
    /// Over [`MAX_LINE_BYTES`].
    TooLong,
    /// Not a JSON object with a whole `page` and a `blocks` list.
    NotAPageLine,
    /// A page the document doesn't have.
    PageOutOfRange,
    /// A page that doesn't come after the last one read: pages arrive once each, in order.
    OutOfOrder,
}

/// One page's blocks as the helper says them.
#[derive(Clone, Debug, PartialEq)]
pub struct PageLine {
    /// Zero-based.
    pub page: u32,
    pub candidates: Vec<Candidate>,
    /// Blocks in the line that couldn't be read as one: not an object, or a kind this doesn't know, or past [`MAX_LINE_BLOCKS`].
    pub unreadable: u32,
}

/// The lines of one helper run, one per page: `{"page":N,"blocks":[{"kind","level","box":[4],"text","continues"}]}`. `kind` is `para` (or `paragraph`), `heading`, `list_item`,
/// `caption`, `code`, `footnote`, `table` or `figure`; `text` is plain text of which only the letters are used.
#[derive(Clone, Debug)]
pub struct PageLines {
    pages: u32,
    next: u32,
}

impl PageLines {
    /// For a document of `pages` pages (the page map's length).
    #[must_use]
    pub fn new(pages: u32) -> Self {
        Self { pages, next: 0 }
    }

    /// The next line. A refused line changes nothing: the caller drops it, or the whole run.
    ///
    /// # Errors
    ///
    /// [`LineError`].
    pub fn read(&mut self, line: &[u8]) -> Result<PageLine, LineError> {
        if line.len() > MAX_LINE_BYTES {
            return Err(LineError::TooLong);
        }
        let value: Value = serde_json::from_slice(line).map_err(|_| LineError::NotAPageLine)?;
        let object = value.as_object().ok_or(LineError::NotAPageLine)?;
        let page = object.get("page").and_then(Value::as_u64).ok_or(LineError::NotAPageLine)?;
        let blocks = object.get("blocks").and_then(Value::as_array).ok_or(LineError::NotAPageLine)?;
        let page = u32::try_from(page).ok().filter(|page| *page < self.pages).ok_or(LineError::PageOutOfRange)?;
        if page < self.next {
            return Err(LineError::OutOfOrder);
        }
        self.next = page + 1;
        let candidates: Vec<Candidate> = blocks.iter().take(MAX_LINE_BLOCKS).filter_map(Candidate::from_value).collect();
        let unreadable = (blocks.len() - candidates.len()) as u32;
        Ok(PageLine { page, candidates, unreadable })
    }
}

/// Why a candidate wasn't stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drops {
    /// Tables and figures.
    pub not_stored_kind: u32,
    /// No letters to look for.
    pub empty: u32,
    /// More than [`MAX_LETTERS`].
    pub too_long: u32,
    /// Its letters aren't an unbroken run in the unmapped text.
    pub not_found: u32,
    /// Its letters are in the unmapped text more than once, and not after the previous block.
    pub ambiguous: u32,
    /// Past [`MAX_BLOCKS`] for the page.
    pub over_cap: u32,
    /// The page's text is past [`MAX_PAGE_CHARS`], so nothing on it is looked for.
    pub page_too_long: u32,
    /// The page's comparisons ran out ([`MAX_COMPARISONS`]).
    pub out_of_budget: u32,
}

/// What a page's mapping stored and dropped, for each of distillPDF's kinds (indexed by [`Shape::index`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub stored: [u32; Shape::COUNT],
    pub dropped: [u32; Shape::COUNT],
    /// Blocks in the page's line that couldn't be read as one (see [`PageLine::unreadable`]). They are dropped too, with no kind to count them under.
    pub unreadable: u32,
    pub why: Drops,
}

impl Counts {
    #[must_use]
    pub fn stored_total(&self) -> u32 {
        self.stored.iter().sum()
    }

    #[must_use]
    pub fn dropped_total(&self) -> u32 {
        self.dropped.iter().sum::<u32>() + self.unreadable
    }
}

/// A page's mapping.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapped {
    pub blocks: PageBlocks,
    pub counts: Counts,
    /// Steps the search took: characters compared and mapped blocks looked at. Bounded by [`MAX_COMPARISONS`] whatever the page and the candidates.
    pub work: u64,
    /// For each stored block, in the same order: which candidate it came from and which step of the rule placed it. For a test or a fuzz target to check the rule by.
    pub placed: Vec<Placed>,
}

/// Which step of the rule placed a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The first run after the previous mapped block's end.
    AfterPrevious,
    /// The one run anywhere on the page that overlaps no mapped block.
    OnlyOccurrence,
}

/// A stored block's source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Index in the candidates given.
    pub candidate: u32,
    pub step: Step,
}

/// What a page's search may still spend, and what it has.
struct Meter {
    left: usize,
    work: u64,
}

impl Meter {
    /// Spends `steps`; false when there isn't that much left (nothing more is spent after).
    fn spend(&mut self, steps: usize) -> bool {
        self.work += steps as u64;
        if self.left < steps {
            self.left = 0;
            return false;
        }
        self.left -= steps;
        true
    }
}

/// Where every start of `needle` in `key` at or after `from` is, in order, until `each` says stop (false) or the budget runs out. Each character looked at costs 1, and `each` charges what it looks at itself.
/// Returns false only when the budget ran out first.
fn runs(key: &[char], needle: &[char], from: usize, meter: &mut Meter, mut each: impl FnMut(usize, &mut Meter) -> bool) -> bool {
    if needle.is_empty() || key.len() < needle.len() {
        return true;
    }
    for start in from..=key.len() - needle.len() {
        let mut matched = 0;
        while matched < needle.len() && key[start + matched] == needle[matched] {
            matched += 1;
        }
        if !meter.spend(matched + usize::from(matched < needle.len())) {
            return false;
        }
        if matched == needle.len() && (!each(start, meter) || meter.left == 0) {
            return meter.left > 0;
        }
    }
    true
}

/// Whether `[from, to)` shares a character with a mapped block. `mapped` is sorted and its blocks overlap none another, so the first that ends after `from` is the only one to look at.
fn overlaps(mapped: &[(u32, u32)], from: u32, to: u32) -> bool {
    let first = mapped.partition_point(|(_, b)| *b <= from);
    mapped.get(first).is_some_and(|(a, _)| *a < to)
}

/// Most characters a block's range reaches over on each side for the marks next to its letters.
const MAX_WIDEN: u32 = 8;

/// A block's characters are its letters and what is stuck to them: the full stop that ends a paragraph, an opening quote, a `$`, the `)` after a number. So the range is
/// widened over adjacent characters that aren't space and aren't letters or digits (those are other words), at most [`MAX_WIDEN`] a side, and never into a block already mapped.
fn widen(page: &[char], (start, end): (u32, u32), mapped: &[(u32, u32)]) -> (u32, u32) {
    let sticks = |at: u32| page.get(at as usize).is_some_and(|c| !c.is_whitespace() && !c.is_alphanumeric()) && !overlaps(mapped, at, at + 1);
    let (mut from, mut to) = (start, end);
    while start - from < MAX_WIDEN && from > 0 && sticks(from - 1) {
        from -= 1;
    }
    while to - end < MAX_WIDEN && sticks(to) {
        to += 1;
    }
    (from, to)
}

/// A box as whole points a record holds, in the order left, bottom, right, top. A side that isn't a number is 0.
fn whole_points(bbox: [f32; 4]) -> [i16; 4] {
    let whole = |side: f32| {
        if side.is_finite() {
            side.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
        } else {
            0
        }
    };
    let [left, bottom, right, top] = bbox.map(whole);
    [left.min(right), bottom.min(top), left.max(right), bottom.max(top)]
}

/// [`map_page`] for a page's line: its unreadable blocks are counted as dropped.
#[must_use]
pub fn map_line(page: &[char], line: &PageLine) -> Mapped {
    let mut mapped = map_page(line.page, page, &line.candidates);
    mapped.counts.unreadable = line.unreadable;
    mapped
}

/// Maps a page's candidates onto its text, as `recto text` gives it (`page` is the characters of `text`, as `read_page` yields them), by the rule above.
/// Blocks come back in the candidates' order, none overlapping another.
#[must_use]
pub fn map_page(page_no: u32, page: &[char], candidates: &[Candidate]) -> Mapped {
    let mut out = Mapped {
        blocks: PageBlocks {
            page: page_no,
            blocks: Vec::new(),
        },
        counts: Counts::default(),
        work: 0,
        placed: Vec::new(),
    };
    // A page a record can't point into is not read at all: nothing is copied, nothing is found, and every candidate says why.
    if page.len() > MAX_PAGE_CHARS as usize {
        for candidate in candidates {
            out.counts.dropped[candidate.shape.index()] += 1;
            out.counts.why.page_too_long += 1;
        }
        return out;
    }
    // Counted first so the two buffers are the size they need and never grow by doubling.
    let mut letter_count = 0;
    letters(page.iter().copied(), |_, _| letter_count += 1);
    let mut key: Vec<char> = Vec::with_capacity(letter_count);
    let mut origin: Vec<u32> = Vec::with_capacity(letter_count);
    letters(page.iter().copied(), |letter, at| {
        key.push(letter);
        origin.push(at as u32);
    });
    let mut meter = Meter {
        left: MAX_COMPARISONS,
        work: 0,
    };
    let mut previous_end = 0u32;
    // Sorted by where they start, and overlapping none another.
    let mut mapped: Vec<(u32, u32)> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let shape = candidate.shape.index();
        let drop = |counts: &mut Counts, why: fn(&mut Drops) -> &mut u32| {
            counts.dropped[shape] += 1;
            *why(&mut counts.why) += 1;
        };
        if candidate.shape.stored().is_none() {
            drop(&mut out.counts, |why| &mut why.not_stored_kind);
            continue;
        }
        let mut needle: Vec<char> = Vec::new();
        let mut too_long = false;
        letters(candidate.text.chars(), |letter, _| {
            if needle.len() < MAX_LETTERS {
                needle.push(letter);
            } else {
                too_long = true;
            }
        });
        if too_long {
            drop(&mut out.counts, |why| &mut why.too_long);
            continue;
        }
        if needle.is_empty() {
            drop(&mut out.counts, |why| &mut why.empty);
            continue;
        }
        if out.blocks.blocks.len() >= MAX_BLOCKS {
            drop(&mut out.counts, |why| &mut why.over_cap);
            continue;
        }
        // The characters a run at letter `start` covers, if no block already mapped has any of them.
        let free = |start: usize, mapped: &[(u32, u32)], meter: &mut Meter| {
            let (from, to) = (origin[start], origin[start + needle.len() - 1] + 1);
            // Looking at the mapped blocks is work too: a binary search of them, charged by its steps.
            meter.spend(1 + (usize::BITS - mapped.len().leading_zeros()) as usize);
            (!overlaps(mapped, from, to)).then_some((from, to))
        };
        let mut found = None;
        let after = origin.partition_point(|at| *at < previous_end);
        let mut complete = runs(&key, &needle, after, &mut meter, |start, meter| {
            found = free(start, &mapped, meter);
            found.is_none()
        });
        let mut step = Step::AfterPrevious;
        let mut ambiguous = false;
        if found.is_none() && complete {
            let mut seen = 0;
            let mut only = None;
            complete = runs(&key, &needle, 0, &mut meter, |start, meter| {
                if let Some(span) = free(start, &mapped, meter) {
                    seen += 1;
                    only = Some(span);
                }
                seen < 2
            });
            ambiguous = seen > 1;
            if seen == 1 {
                found = only;
                step = Step::OnlyOccurrence;
            }
        }
        let Some((start, end)) = found else {
            if !complete {
                drop(&mut out.counts, |why| &mut why.out_of_budget);
            } else if ambiguous {
                drop(&mut out.counts, |why| &mut why.ambiguous);
            } else {
                drop(&mut out.counts, |why| &mut why.not_found);
            }
            continue;
        };
        let (start, end) = widen(page, (start, end), &mapped);
        let at = mapped.partition_point(|(a, _)| *a < start);
        mapped.insert(at, (start, end));
        previous_end = end;
        out.counts.stored[shape] += 1;
        out.placed.push(Placed { candidate: index as u32, step });
        out.blocks.blocks.push(Block {
            kind: candidate.shape.stored().unwrap_or(Kind::Paragraph),
            level: candidate.level.min(MAX_LEVEL),
            page: page_no,
            start,
            end,
            bbox: whole_points(candidate.bbox),
            continues: candidate.continues,
        });
    }
    out.work = meter.work;
    out
}
