//! **A page's edge pieces, kept and read back**, so chrome can be decided from a corpus without reading the page again: what `recto get` needs to leave running heads out of what it gives.
//!
//! **Kept.** In the page's own sealed chunk, as a third region after its text and what's beside it, marked by the page flag `FLAG_EDGES`: a `u8` count of at most [`MAX_EDGE_LINES`], then that many records of [`LINE_LEN`] bytes, little-endian. A page that was read with its frame has the region even with no pieces, which says "known, none". A page that failed, or was fed without a frame, has none, which says "unknown".
//!
//! **One rule.** Nothing here decides what is chrome: [`StoredChrome`] hands the stored pieces to [`chrome_spans`], the one rule `recto text` marks pages with. It gives nothing for a page when it or any page it is compared with has no stored pieces: a page taken as empty could set off the lone page number rule and drop a figure.
//!
//! **Hostile bytes.** The region is inside an authenticated chunk, and is still checked as if it weren't ([`check`]): the count, the exact length, finite places, and `start <= end <= ` the page's characters. A region that fails makes its page damaged.
//!
//! **Cost.** At most `1 + 70 * 16 = 1,121` B a page; [`StoredChrome`] holds at most `2 * NEIGHBOURS + 1` pages' pieces and asks for each page's once as pages are walked in order.

use super::{EdgeLine, InRow, MAX_EDGE_LINES, NEIGHBOURS, NO_NUMBER, chrome_spans};
use crate::select::neighbour_pages;

/// One piece's bytes.
pub const LINE_LEN: usize = 70;
/// The most a region takes.
pub const MAX_REGION_LEN: usize = 1 + MAX_EDGE_LINES * LINE_LEN;

const TOP: u16 = 1;
const WORDS: u16 = 1 << 1;
const DIGITS_ONLY: u16 = 1 << 2;
const ISOLATED: u16 = 1 << 3;
const DATED: u16 = 1 << 4;
const SUBSTANTIAL: u16 = 1 << 5;
const ALONE: u16 = 1 << 6;
const LAST: u16 = 1 << 7;
const NUMBERED: u16 = 1 << 8;
const KNOWN: u16 = (1 << 9) - 1;

/// Whether a region can be this long: a count, and whole pieces up to the most a page keeps. Said by a chunk's lengths, before its bytes are looked at.
pub fn length_fits(len: usize) -> bool {
    len.checked_sub(1).is_some_and(|rest| rest % LINE_LEN == 0 && rest / LINE_LEN <= MAX_EDGE_LINES)
}

/// Appends a page's pieces as a region: at most [`MAX_EDGE_LINES`], the rest dropped (a page never has more).
pub fn write(lines: &[EdgeLine], out: &mut Vec<u8>) {
    let lines = &lines[..lines.len().min(MAX_EDGE_LINES)];
    out.push(lines.len() as u8);
    for line in lines {
        let bit = |on: bool, flag: u16| if on { flag } else { 0 };
        let flags = bit(line.top, TOP)
            | bit(line.words, WORDS)
            | bit(line.digits_only, DIGITS_ONLY)
            | bit(line.isolated, ISOLATED)
            | bit(line.dated, DATED)
            | bit(line.substantial, SUBSTANTIAL)
            | bit(line.place.alone, ALONE)
            | bit(line.place.last, LAST)
            | bit(line.number.is_some(), NUMBERED);
        out.extend_from_slice(&flags.to_le_bytes());
        for float in [line.from_edge, line.size, line.relative, line.from_left, line.from_right] {
            out.extend_from_slice(&float.to_le_bytes());
        }
        out.extend_from_slice(&line.key.to_le_bytes());
        out.extend_from_slice(&line.row_key.to_le_bytes());
        for number in line.row_numbers.iter().chain(&line.numbers) {
            out.extend_from_slice(&number.to_le_bytes());
        }
        out.extend_from_slice(&line.framed_number.to_le_bytes());
        out.extend_from_slice(&line.start.to_le_bytes());
        out.extend_from_slice(&line.end.to_le_bytes());
        out.extend_from_slice(&line.number.unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&line.place.row.to_le_bytes());
    }
}

/// The records of a region, if its count and length agree.
fn records(bytes: &[u8]) -> Option<&[[u8; LINE_LEN]]> {
    let (count, rest) = bytes.split_first()?;
    let (records, tail) = rest.as_chunks::<LINE_LEN>();
    (tail.is_empty() && records.len() == usize::from(*count) && records.len() <= MAX_EDGE_LINES).then_some(records)
}

/// One piece, if it's a whole one: finite places, and a span inside a page of `chars` characters.
fn line_of(record: &[u8; LINE_LEN], chars: u32) -> Option<EdgeLine> {
    let mut at = 0;
    let mut take = |len: usize| {
        let bytes = &record[at..at + len];
        at += len;
        bytes
    };
    let word = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap_or_default());
    let float = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().unwrap_or_default());
    let flags = u16::from_le_bytes(take(2).try_into().ok()?);
    if flags & !KNOWN != 0 {
        return None;
    }
    let (from_edge, size, relative, from_left, from_right) = (float(take(4)), float(take(4)), float(take(4)), float(take(4)), float(take(4)));
    if ![from_edge, size, relative, from_left, from_right].iter().all(|float| float.is_finite()) {
        return None;
    }
    let key = u64::from_le_bytes(take(8).try_into().ok()?);
    let row_key = u64::from_le_bytes(take(8).try_into().ok()?);
    let row_numbers = [word(take(4)), word(take(4))];
    let numbers = [word(take(4)), word(take(4))];
    let framed_number = u16::from_le_bytes(take(2).try_into().ok()?);
    let (start, end, number) = (word(take(4)), word(take(4)), word(take(4)));
    let row = u16::from_le_bytes(take(2).try_into().ok()?);
    // Not NO_NUMBER, which would read as "no number" in a rule, when it was written as a number.
    if start > end || end > chars || (flags & NUMBERED != 0 && number == NO_NUMBER) {
        return None;
    }
    let has = |flag: u16| flags & flag != 0;
    Some(EdgeLine {
        top: has(TOP),
        from_edge,
        size,
        key,
        row_key,
        row_numbers,
        numbers,
        framed_number,
        start,
        end,
        number: has(NUMBERED).then_some(number),
        relative,
        words: has(WORDS),
        digits_only: has(DIGITS_ONLY),
        isolated: has(ISOLATED),
        dated: has(DATED),
        substantial: has(SUBSTANTIAL),
        from_left,
        from_right,
        place: InRow {
            row,
            alone: has(ALONE),
            last: has(LAST),
        },
    })
}

/// Whether a region is whole, for a page of `chars` characters. Reads nothing into memory.
pub fn check(bytes: &[u8], chars: u32) -> bool {
    records(bytes).is_some_and(|records| records.iter().all(|record| line_of(record, chars).is_some()))
}

/// A page's pieces, or nothing if the region isn't whole.
pub fn read(bytes: &[u8], chars: u32) -> Option<Vec<EdgeLine>> {
    records(bytes)?.iter().map(|record| line_of(record, chars)).collect()
}

/// Chrome for pages walked in order, from stored pieces. Holds the pieces of at most `2 * NEIGHBOURS + 1` pages, and asks for each page's once when they're walked in order, and again only after the walk has gone away from it.
pub struct StoredChrome<F> {
    pages: u32,
    edges_of: F,
    /// Pages held, each with what's stored for it: `None` if nothing is.
    held: Vec<(u32, Option<Vec<EdgeLine>>)>,
}

impl<F: FnMut(u32) -> Option<Vec<EdgeLine>>> StoredChrome<F> {
    /// For a document of `pages`, with `edges_of(page)` giving the page's stored pieces, or `None` if it has none.
    pub fn new(pages: u32, edges_of: F) -> Self {
        Self {
            pages,
            edges_of,
            held: Vec::with_capacity(2 * NEIGHBOURS as usize + 1),
        }
    }

    /// What a copy of `page` leaves out as chrome, as `[start, end)` in its text, or `None` if it and the pages it's compared with aren't all known.
    pub fn spans(&mut self, page: u32) -> Option<Vec<(u32, u32)>> {
        if page >= self.pages {
            return None;
        }
        let (first, last) = (page.saturating_sub(NEIGHBOURS), page.saturating_add(NEIGHBOURS).min(self.pages - 1));
        self.held.retain(|(held, _)| (first..=last).contains(held));
        for wanted in first..=last {
            if !self.held.iter().any(|(held, _)| *held == wanted) {
                let edges = (self.edges_of)(wanted);
                self.held.push((wanted, edges));
            }
        }
        let find = |wanted: u32| self.held.iter().find(|(held, _)| *held == wanted).and_then(|(_, edges)| edges.as_deref());
        let own = find(page)?;
        let neighbours = neighbour_pages(page, self.pages)
            .map(|other| Some((i64::from(other) - i64::from(page), find(other)?)))
            .collect::<Option<Vec<_>>>()?;
        Some(chrome_spans(page, own, &neighbours))
    }
}

#[cfg(test)]
mod tests;
