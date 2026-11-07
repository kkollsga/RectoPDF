//! **Whether a file's own contents are missing or look bad,** which is when generated ones are offered (owner, 2026-11-07: "dont modify the bad toc, but offer a generated toc using distillpdf"). Told from what a warm tab already has, its contents and its page count, so nothing is read for it. It sorts the entries' pages once, so a caller keeps the answer with the contents rather than asking at every page.
//!
//! The rules and their evidence are in `docs/ENGINEERING.md`, "Generated contents": each was looked at across the 141 PDFs of bench100, with a sample of each verdict read by eye. What's flagged is only ever offered an alternative: the file's own stay, one tap away.

use crate::{NONE, Toc};

/// Fewer pages than this, and contents would only repeat the document: nothing's offered.
pub const MIN_PAGES: u32 = 4;
/// From this many pages, two entries can't be the contents of a document.
const FEW_FROM_PAGES: u32 = 20;
const FEW_ENTRIES: usize = 2;
/// From this many pages, contents whose entries end before this share of the way through leave most of it out.
const EARLY_FROM_PAGES: u32 = 30;
const EARLY_SHARE: f64 = 1.0 / 3.0;
/// From the same length, contents with no entry for a stretch of more than this share of the document, between two entries, skip most of it: a law's title, then nothing until its annexes.
const GAP_SHARE: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentsVerdict {
    /// Good enough, or too short a document for contents to matter.
    Fine,
    /// None at all.
    Missing,
    LooksBad(BadReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadReason {
    /// Most entries lead to no page.
    GoNowhere,
    /// Every entry leads to the same page.
    OnePage,
    /// Too few entries for a document this long.
    TooFew,
    /// Its entries stop well before the document does.
    StopsEarly,
    /// Most of the document lies between two of its entries, with none in between.
    SkipsMost,
    /// Most entries lead to one page, in a long document: a merged file whose bookmarks all point at its first page, as the EU's competitiveness report, part B, has for its whole first half.
    PiledOnOnePage,
    /// The numbering says the nesting is wrong: chapter 2 inside chapter 1, or 3.1 beside 3 rather than under it.
    Misnested,
}

impl ContentsVerdict {
    /// Whether generated contents are offered.
    pub fn offers(self) -> bool {
        self != Self::Fine
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Fine => "fine",
            Self::Missing => "missing",
            Self::LooksBad(BadReason::GoNowhere) => "entries_go_nowhere",
            Self::LooksBad(BadReason::OnePage) => "all_on_one_page",
            Self::LooksBad(BadReason::TooFew) => "too_few",
            Self::LooksBad(BadReason::StopsEarly) => "stops_early",
            Self::LooksBad(BadReason::SkipsMost) => "skips_most",
            Self::LooksBad(BadReason::PiledOnOnePage) => "piled_on_one_page",
            Self::LooksBad(BadReason::Misnested) => "misnested",
        }
    }

    /// Why, in a few plain words, for a hover and for an agent.
    pub fn words(self) -> &'static str {
        match self {
            Self::Fine => "The file's own contents look usable.",
            Self::Missing => "This file has no contents of its own.",
            Self::LooksBad(BadReason::GoNowhere) => "Most of the file's own contents lead to no page.",
            Self::LooksBad(BadReason::OnePage) => "All of the file's own contents lead to the same page.",
            Self::LooksBad(BadReason::TooFew) => "The file's own contents have too few entries for its length.",
            Self::LooksBad(BadReason::StopsEarly) => "The file's own contents stop well before the document does.",
            Self::LooksBad(BadReason::SkipsMost) => "The file's own contents skip most of the document.",
            Self::LooksBad(BadReason::PiledOnOnePage) => "Most of the file's own contents lead to the same page.",
            Self::LooksBad(BadReason::Misnested) => "The file's own contents are nested in a way their numbering says is wrong.",
        }
    }
}

/// From this many pages, contents with more than half their entries on one page, and at least [`PILE_ENTRIES`] of them, are piled there.
const PILE_FROM_PAGES: u32 = 30;
const PILE_ENTRIES: usize = 8;
/// How many entries the numbering has to say are in the wrong place.
const MISNESTED_ENTRIES: usize = 2;

/// An entry's section number, as `[3, 1]` for "3.1 Computing" or "3.1. Computing", if it starts with one. At most four parts, each under 1,000, and a number has to be followed by a space or a point, so "2024 report" and "3D printing" aren't numbers.
fn section_number(title: &str) -> Option<([u16; 4], usize)> {
    let end = title.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    if !title[end..].starts_with([' ', '\u{A0}']) {
        return None;
    }
    let lead = title[..end].strip_suffix('.').unwrap_or(&title[..end]);
    let mut parts = [0u16; 4];
    let mut count = 0;
    for part in lead.split('.') {
        if count == 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        parts[count] = part.parse().ok()?;
        count += 1;
    }
    (count > 0).then_some((parts, count))
}

/// How many entries the numbering says are nested wrongly: one numbered like its parent's sibling (chapter 2 under chapter 1), or one whose number continues the entry before it at its own level (3.1 beside 3).
fn misnested(toc: &Toc) -> usize {
    let entries = toc.entries();
    let numbers: Vec<Option<([u16; 4], usize)>> = entries.iter().map(|entry| section_number(toc.title(entry))).collect();
    let mut wrong = 0;
    // The latest entry with each length of number under each parent.
    let mut latest: std::collections::HashMap<(u32, usize), usize> = std::collections::HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some((number, parts)) = numbers[index] else { continue };
        let parent = numbers.get(entry.parent as usize).copied().flatten();
        let under_its_sibling = parent.is_some_and(|(above, above_parts)| above_parts == parts && number[..parts - 1] == above[..parts - 1] && number[parts - 1] > above[parts - 1]);
        let beside_its_parent = parts > 1 && latest.get(&(entry.parent, parts - 1)).and_then(|at| numbers[*at]).is_some_and(|(before, _)| number[..parts - 1] == before[..parts - 1]);
        if under_its_sibling || beside_its_parent {
            wrong += 1;
        }
        latest.insert((entry.parent, parts), index);
    }
    wrong
}

/// The verdict on a file's own contents, for a document of `pages`. Contents that were cut at a reading limit are judged on what was read.
pub fn judge(toc: &Toc, pages: u32) -> ContentsVerdict {
    if pages < MIN_PAGES {
        return ContentsVerdict::Fine;
    }
    let entries = toc.entries();
    if entries.is_empty() {
        return ContentsVerdict::Missing;
    }
    let (mut placed, mut first, mut last, mut spread) = (0usize, NONE, 0u32, false);
    // Entries aren't always in page order: an index or a blank page can be listed out of place. So the stretches without one go by the pages sorted.
    let mut starts: Vec<u32> = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.page == NONE || entry.page >= pages {
            continue;
        }
        starts.push(entry.page);
        placed += 1;
        if first == NONE {
            first = entry.page;
        } else if entry.page != first {
            spread = true;
        }
        last = last.max(entry.page);
    }
    if placed * 2 < entries.len() || placed == 0 {
        return ContentsVerdict::LooksBad(BadReason::GoNowhere);
    }
    if !spread && pages >= 8 && entries.len() > 1 {
        return ContentsVerdict::LooksBad(BadReason::OnePage);
    }
    if pages >= FEW_FROM_PAGES && entries.len() <= FEW_ENTRIES {
        return ContentsVerdict::LooksBad(BadReason::TooFew);
    }
    if pages >= EARLY_FROM_PAGES && f64::from(last + 1) < f64::from(pages) * EARLY_SHARE {
        return ContentsVerdict::LooksBad(BadReason::StopsEarly);
    }
    if pages >= PILE_FROM_PAGES && starts.len() >= PILE_ENTRIES {
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        let most = sorted.chunk_by(|a, b| a == b).map(<[u32]>::len).max().unwrap_or(0);
        if most >= PILE_ENTRIES && most * 2 > entries.len() {
            return ContentsVerdict::LooksBad(BadReason::PiledOnOnePage);
        }
    }
    if misnested(toc) >= MISNESTED_ENTRIES {
        return ContentsVerdict::LooksBad(BadReason::Misnested);
    }
    starts.sort_unstable();
    let widest_gap = starts.windows(2).map(|pair| pair[1] - pair[0]).max().unwrap_or(0);
    if pages >= EARLY_FROM_PAGES && f64::from(widest_gap) > f64::from(pages) * GAP_SHARE {
        return ContentsVerdict::LooksBad(BadReason::SkipsMost);
    }
    ContentsVerdict::Fine
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TocLine;

    fn toc(entries: &[(u32, u8)]) -> Toc {
        let lines: Vec<TocLine<'_>> = entries.iter().map(|&(page, depth)| TocLine { title: "Section", page, parent: NONE, depth }).collect();
        Toc::from_lines(&lines, false)
    }

    /// Failure: a document with no contents gets no offer, or a three-page note gets one it doesn't need.
    #[test]
    fn missing_contents_are_offered_only_where_they_would_help() {
        assert_eq!(judge(&Toc::default(), 40), ContentsVerdict::Missing);
        assert_eq!(judge(&Toc::default(), MIN_PAGES - 1), ContentsVerdict::Fine);
        assert!(ContentsVerdict::Missing.offers());
    }

    /// Failure: contents that can't be used to get anywhere, which real files have, are taken as good, so nothing better is offered.
    #[test]
    fn unusable_contents_look_bad() {
        assert_eq!(judge(&toc(&[(NONE, 0), (NONE, 0), (3, 0)]), 40), ContentsVerdict::LooksBad(BadReason::GoNowhere));
        assert_eq!(judge(&toc(&[(0, 0), (0, 1), (0, 1)]), 40), ContentsVerdict::LooksBad(BadReason::OnePage));
        assert_eq!(judge(&toc(&[(0, 0), (5, 0)]), 40), ContentsVerdict::LooksBad(BadReason::TooFew));
        assert_eq!(judge(&toc(&[(0, 0), (2, 0), (5, 0), (9, 0)]), 100), ContentsVerdict::LooksBad(BadReason::StopsEarly));
        // The EU's AI Act: its title on page 1, then its annexes from page 124 of 144.
        assert_eq!(judge(&toc(&[(0, 0), (123, 1), (125, 1), (126, 1), (129, 1), (143, 1)]), 144), ContentsVerdict::LooksBad(BadReason::SkipsMost));
        // A page past the end leads nowhere.
        assert_eq!(judge(&toc(&[(500, 0), (600, 0), (1, 0)]), 40), ContentsVerdict::LooksBad(BadReason::GoNowhere));
    }

    fn titled(entries: &[(&str, u32, u32, u8)]) -> Toc {
        let lines: Vec<TocLine<'_>> = entries.iter().map(|&(title, page, parent, depth)| TocLine { title, page, parent, depth }).collect();
        Toc::from_lines(&lines, false)
    }

    /// Failure: the EU's competitiveness report, part B, whose first half's 41 bookmarks all lead to page 1, reads as fine, and the reader gets page numbers that make no sense with nothing better offered. The owner, 2026-11-07: "The right side numbers isnt logical".
    #[test]
    fn contents_piled_on_one_page_look_bad() {
        let mut entries: Vec<(u32, u8)> = vec![(0, 0)];
        entries.extend((0..40).map(|_| (0, 1)));
        entries.extend([(226, 0), (227, 1), (256, 1), (279, 1), (297, 1), (306, 1), (309, 2), (314, 2), (316, 2)]);
        assert_eq!(judge(&toc(&entries), 328), ContentsVerdict::LooksBad(BadReason::PiledOnOnePage));
        // A title page with a few entries on it is ordinary.
        assert_eq!(judge(&toc(&[(0, 0), (0, 1), (0, 1), (3, 0), (9, 0), (20, 0), (31, 0), (40, 0)]), 48), ContentsVerdict::Fine);
    }

    /// Failure: contents whose own numbering says they're nested wrongly read as fine: part A of the competitiveness report has chapters 2 to 6 inside chapter 1, and part B puts 3.1 to 3.3 beside 3 rather than under it. The owner, 2026-11-07, called the indents wrong.
    #[test]
    fn contents_nested_against_their_numbering_look_bad() {
        let part_a = titled(&[("1. The starting point", 12, NONE, 0), ("Three transformations", 15, 0, 1), ("2. Closing the innovation gap", 24, 0, 1), ("3. A joint plan", 40, 0, 1), ("4. Security", 55, 0, 1)]);
        assert_eq!(judge(&part_a, 73), ContentsVerdict::LooksBad(BadReason::Misnested));
        let part_b = titled(&[("Sectoral policies", 1, NONE, 0), ("3. Digitalisation", 10, 0, 1), ("3.1 Broadband", 12, 0, 1), ("3.2 Computing and AI", 20, 0, 1), ("3.3 Semiconductors", 30, 0, 1), ("4. Energy-intensive industries", 40, 0, 1)]);
        assert_eq!(judge(&part_b, 60), ContentsVerdict::LooksBad(BadReason::Misnested));
        // Numbered as they're nested, or numbers that aren't section numbers.
        let good = titled(&[("1. Introduction", 0, NONE, 0), ("1.1 Scope", 1, 0, 1), ("1.2 Terms", 2, 0, 1), ("2. Method", 4, NONE, 0), ("2.1 Data", 5, 3, 1), ("2024 in review", 8, NONE, 0), ("3D printing", 9, NONE, 0), ("3. Results", 10, NONE, 0)]);
        assert_eq!(judge(&good, 12), ContentsVerdict::Fine);
        assert_eq!(section_number("3.1 Computing"), Some(([3, 1, 0, 0], 2)));
        assert_eq!(section_number("3.1. Computing").map(|(_, parts)| parts), Some(2));
        assert_eq!(section_number("2024 report"), None);
        assert_eq!(section_number("3D printing"), None);
        assert_eq!(section_number("12345 x"), None);
    }

    /// Failure: ordinary contents are flagged, so the sidebar nags about a file that's fine: a short paper's few sections, a title entry on page 1, or contents whose last chapter is two-thirds in, with appendices after it.
    #[test]
    fn ordinary_contents_are_fine() {
        assert_eq!(judge(&toc(&[(0, 0), (1, 1), (4, 1), (8, 1)]), 12), ContentsVerdict::Fine);
        assert_eq!(judge(&toc(&[(0, 0)]), 6), ContentsVerdict::Fine);
        assert_eq!(judge(&toc(&[(0, 0), (20, 0), (45, 0), (66, 0)]), 100), ContentsVerdict::Fine);
        // A paper whose sections end at page 19 of 51, with references and appendices after: the main text is all there.
        assert_eq!(judge(&toc(&[(0, 0), (3, 1), (9, 1), (14, 1), (18, 1)]), 51), ContentsVerdict::Fine);
        // NASA's technology report lists its index, on page 115, before its chapters, and a blank page 2 last.
        assert_eq!(judge(&toc(&[(4, 0), (6, 0), (114, 0), (10, 0), (26, 0), (52, 0), (80, 0), (110, 0), (1, 0)]), 120), ContentsVerdict::Fine);
        assert_eq!(judge(&toc(&[(0, 0), (3, 0), (6, 0)]), 19), ContentsVerdict::Fine);
    }
}
