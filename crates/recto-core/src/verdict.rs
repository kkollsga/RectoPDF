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
        }
    }
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
