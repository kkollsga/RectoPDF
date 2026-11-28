//! **Sections from a tab's contents.** An entry's section runs from the entry to the next entry of the same or shallower depth (or the document's end).
//! A chapter is a top-level entry (depth 0). A place is a page and how far down it (thousandths of the page from its top, as a contents entry says it),
//! because that is where a contents entry is: a block's place is [`Place::of_block`], as the page is shown.
//!
//! Entries with no place (`top` 0) sit at their page's top, as `Toc::section_at_place` reads them. Entries are taken in the order the contents gives them;
//! the page of an entry that the file left out ([`recto_core::NONE`]) makes the entry unplaced, so it holds nothing.

use super::Block;
use recto_core::nav::Frame;
use recto_core::{TOP_SCALE, TocEntry};

/// Most entries read. A hostile outline is capped by the reader long before this; this keeps a query's cost bounded whatever it's given.
pub const MAX_ENTRIES: usize = 65_536;

/// A page and how far down it, in thousandths of the page from its top. Ordered by page, then place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    pub page: u32,
    pub top: u16,
}

impl Place {
    /// Where a page's blocks are put when the page has no frame to place them by, or a box can't be placed: the middle of the page.
    pub const MIDDLE: u16 = 500;

    /// Where a block is: the middle of its box (a box is whole points and a contents entry's place is a thousandth of the page, so an edge can fall half a point off an entry's own place and a heading
    /// into the section before it), **as the page is shown** (`Frame::to_shown`: the page's turn, and a crop box that doesn't start at 0, which the box's own coordinates are not in), as a thousandth of
    /// the shown height from the top. A frame with no area, or a box that can't be placed, is [`Self::MIDDLE`].
    #[must_use]
    pub fn of_block(block: &Block, frame: Frame) -> Self {
        let rect = [
            f32::from(block.bbox[0]),
            f32::from(block.bbox[1]),
            f32::from(block.bbox[2]),
            f32::from(block.bbox[3]),
        ];
        let top = match (frame.shown_size(), frame.to_shown(rect)) {
            (Some((_, height)), Some(shown)) => recto_core::thousandths(f64::from(((shown[1] + shown[3]) / 2.0) / height)),
            _ => Self::MIDDLE,
        };
        Self { page: block.page, top }
    }

    /// A block on a page with no frame.
    #[must_use]
    pub const fn middle(page: u32) -> Self {
        Self { page, top: Self::MIDDLE }
    }
}

/// The contents' entries as ranges.
#[derive(Clone, Debug, Default)]
pub struct Sections {
    /// Each entry's start, depth, and where its section ends (`None`: at the document's end). `None` for the start when the entry has no page.
    rows: Vec<Row>,
    /// The placed entries' starts and numbers, sorted by place and then number: what a page's entries are found in (one binary search), so no walk of every entry is made for a page or a block.
    by_start: Vec<(Place, u32)>,
}

#[derive(Clone, Copy, Debug)]
struct Row {
    start: Option<Place>,
    depth: u8,
    end: Option<Place>,
}

impl Sections {
    /// Ranges from the entries, in contents order. Past [`MAX_ENTRIES`] the rest are left out.
    #[must_use]
    pub fn new(entries: &[TocEntry]) -> Self {
        Self::with_tops(entries, |entry| entry.top)
    }

    /// [`Self::new`] with each entry's place down its page as `top_of` says: a page whose places mean nothing (an unsaved turn, or the view turned) has every entry at its top, as `contents` says.
    #[must_use]
    pub fn with_tops(entries: &[TocEntry], top_of: impl Fn(&TocEntry) -> u16) -> Self {
        let mut rows: Vec<Row> = entries
            .iter()
            .take(MAX_ENTRIES)
            .map(|entry| Row {
                start: (entry.page != recto_core::NONE).then_some(Place {
                    page: entry.page,
                    top: top_of(entry).min(TOP_SCALE),
                }),
                depth: entry.depth,
                end: None,
            })
            .collect();
        // The end of each section is the start of the next entry of the same or shallower depth: one pass from the back keeping, for each depth, the nearest start seen.
        let mut next_at: [Option<Place>; 256] = [None; 256];
        for row in rows.iter_mut().rev() {
            let depth = usize::from(row.depth);
            // The nearest start among the entries after this one with a depth at or under this one's.
            row.end = next_at[depth];
            if let Some(start) = row.start {
                for slot in &mut next_at[depth..] {
                    // An entry at depth d ends every section at depth d or deeper.
                    *slot = Some(slot.map_or(start, |seen| seen.min(start)));
                }
            }
        }
        let mut by_start: Vec<(Place, u32)> = rows.iter().enumerate().filter_map(|(index, row)| Some((row.start?, index as u32))).collect();
        by_start.sort_unstable();
        Self { rows, by_start }
    }

    /// The entries that start on `page`, by place and then number: `(place, entry)`.
    #[must_use]
    pub fn starting_on(&self, page: u32) -> &[(Place, u32)] {
        let first = self.by_start.partition_point(|(place, _)| place.page < page);
        let end = self.by_start.partition_point(|(place, _)| place.page <= page);
        &self.by_start[first..end]
    }

    /// The next and the previous entry to go to from `entry`: the next placed entry after it, and the nearest placed one before it, whose depth is not deeper, in the contents' order. For a chapter that is
    /// the next and the previous chapter. `None` where there is none.
    #[must_use]
    pub fn neighbours(&self, entry: usize) -> (Option<usize>, Option<usize>) {
        let Some(depth) = self.rows.get(entry).map(|row| row.depth) else {
            return (None, None);
        };
        let fits = |index: &usize| self.rows[*index].start.is_some() && self.rows[*index].depth <= depth;
        let next = (entry + 1..self.rows.len()).find(fits);
        let prev = (0..entry).rev().find(fits);
        (next, prev)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Where entry `entry`'s section starts and where it ends (`None` for the document's end).
    #[must_use]
    pub fn range(&self, entry: usize) -> Option<(Place, Option<Place>)> {
        let row = self.rows.get(entry)?;
        Some((row.start?, row.end))
    }

    /// Whether the section of `entry` holds `place`: at or after its start, before its end.
    #[must_use]
    pub fn holds(&self, entry: usize, place: Place) -> bool {
        self.range(entry)
            .is_some_and(|(start, end)| start <= place && end.is_none_or(|end| place < end))
    }

    /// The section holding `place`: the innermost, which is the one that starts last (the later entry where two start together).
    #[must_use]
    pub fn section_of(&self, place: Place) -> Option<usize> {
        let mut best: Option<(Place, usize)> = None;
        for (index, row) in self.rows.iter().enumerate() {
            #[cfg(test)]
            super::unit::count_step();
            if let Some(start) = row.start
                && self.holds(index, place)
                && best.is_none_or(|(seen, _)| start >= seen)
            {
                best = Some((start, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// The chapter holding `place`: the top-level entry (depth 0) whose section holds it.
    #[must_use]
    pub fn chapter_of(&self, place: Place) -> Option<usize> {
        self.holding(place).find(|index| self.rows[*index].depth == 0)
    }

    /// The headings above `place`: every section that holds it, the outermost first, ending with the innermost, which is [`Self::section_of`]. At most one for each depth.
    #[must_use]
    pub fn headings_above(&self, place: Place) -> Vec<usize> {
        let mut chain: Vec<usize> = Vec::new();
        for index in self.holding(place) {
            // Entries at one depth don't nest: the later one ends the earlier, so a place can't be in two unless a file's places are out of order, when the later wins.
            match chain.last().copied() {
                Some(last) if self.rows[last].depth >= self.rows[index].depth => {
                    while chain.last().is_some_and(|last| self.rows[*last].depth >= self.rows[index].depth) {
                        chain.pop();
                    }
                    chain.push(index);
                }
                _ => chain.push(index),
            }
        }
        chain
    }

    /// The depth of an entry.
    #[must_use]
    pub fn depth(&self, entry: usize) -> Option<u8> {
        self.rows.get(entry).map(|row| row.depth)
    }

    /// Every entry whose section holds `place`, in contents order.
    fn holding(&self, place: Place) -> impl Iterator<Item = usize> + '_ {
        (0..self.rows.len()).filter(move |index| self.holds(*index, place))
    }
}
