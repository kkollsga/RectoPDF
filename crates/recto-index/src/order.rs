//! Puts words back on the lines they're printed on. A PDF can draw a line's styled words after the rest of the page, and PDFium hands characters back in drawing order, so
//! `A recipient MUST ignore` reads `A recipient ignore`, with `MUST` lost at the bottom of the page. A dropped `MUST NOT` turns a sentence round.
//!
//! **What it does, and nothing else.** A later run goes into an earlier line if its baseline matches that line and it fits a horizontal gap in it, or sits a word's space beyond either end of
//! it. Everything else stays in the order PDFium gave. A full layout sort isn't wanted: the renderer spike (§10.2) found drawing order right for two-column body text on 79 of 84 pages, and a
//! column can't land in another by these rules, because it neither fits a gap between characters nor starts a word's space away from the end of a line.
//!
//! **A move is all or nothing, and passes nothing.** All of a line moves, or none of it. And a run doesn't move past something drawn between it and its line that sits on or against that line:
//! a superscript or a fraction's parts stay in their place in the equation, in drawing order, rather than what follows them jumping ahead and leaving them behind. So mathematics, which PDFium
//! breaks into many lines at each raised symbol, mostly stays as broken up as it was, in the right order.
//!
//! **What it never does.** Add or lose a character that isn't a space. A run with a line-end hyphen isn't moved, and nothing goes after one. Text running right to left, up the page, or on a
//! slope is left alone, as are lines broken up by a reversal, because a reversed word looks exactly like a run of words drawn late. Letters or digits touching across the end of a line stay
//! apart, since nothing says they're one word: a map's `AREA` and the `PRL` beside it aren't `AREAPRL`.
//!
//! **Cost.** Linear in the characters, plus sorting the lines. A page past [`MAX_LINES`] lines stays as it is. Looking for a line to go into is capped per line and per page, so a hostile page
//! gets its order left alone, and no more time. It holds two integers per character at most, and only while it runs.

use crate::is_break;
use recto_core::is_invisible_format;
use recto_proto::{CharBox, CharRecord};

/// Lines on a page this will look at. The longest real page in bench100 has far fewer. Past this, the page is either hostile or not one a reader follows line by line.
pub const MAX_LINES: usize = 16_384;
/// Earlier lines looked at for one line's words, nearest baseline first. Plenty for real pages, where a baseline holds a few lines at most, and it's what keeps a page of one baseline linear.
const MAX_HOSTS_LOOKED: usize = 16;
/// Runs that may go into one line.
const MAX_INSERTS_PER_LINE: usize = 32;
/// Fits tried on one page. A real page uses hundreds.
const MAX_FITS_TRIED: usize = 1 << 20;
/// Characters per line used to find its baseline and height.
const SAMPLE: usize = 32;

/// How far a baseline may differ and still be the same line, in line heights.
const SAME_BASELINE: f32 = 0.25;
/// How many lines back to look for something a run mustn't pass.
const BLOCKER_LOOK_BACK: usize = 16;
/// How close above or below a line something must be to count as against it, in line heights. Superscripts, subscripts and a fraction's parts are this close, and the next line of ordinary text on an ordinary page isn't: RFC 9110's lines have 0.36 of a line height between them.
const AGAINST_LINE: f32 = 0.25;
/// How far a line may drift up or down from first character to last, in line heights, and still be level. Past this it's a sloped label.
const SLOPE: f32 = 0.5;
/// How far the next character must step back, in line heights, to be a new run rather than kerning or an accent.
const STEP_BACK: f32 = 0.5;
/// How wide a gap splits a run, in line heights. Words a little apart can go into different gaps of a line.
const SPLIT_GAP: f32 = 0.8;
/// How far beyond a line's end a run may sit, in line heights. A wide justified word space reaches about this. The narrowest gutter between columns doesn't.
const END_GAP: f32 = 0.6;
/// How much a run may overlap its neighbours, in line heights. Glyph boxes overlap a little when kerned.
const OVERLAP: f32 = 0.15;
/// A gap this wide, in line heights, is a space between words rather than a change of style inside one.
const WORD_GAP: f32 = 0.1;
/// One in this many steps going back or jumping lines, and a line isn't plain left-to-right text.
const ODD_STEPS: u32 = 4;

/// A page's characters in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairedOrder {
    pub chars: Vec<CharRecord>,
    /// For each character in `chars`, its index in the characters given. [`GENERATED`] for a space made here.
    pub origin: Vec<u32>,
    pub runs_moved: u32,
}

/// In [`RepairedOrder::origin`]: a space this made, which came from nowhere.
pub const GENERATED: u32 = u32::MAX;

/// A monotonic stretch of a stream line. At most [`MAX_LINES`] of these, for as long as a page is being read, and asserted at 72 B or less in a test.
struct Line {
    /// Its range in the list of characters with geometry.
    geo: (u32, u32),
    stream_line: u32,
    baseline: f32,
    height: f32,
    left: f32,
    right: f32,
    low: f32,
    high: f32,
    /// Level text, in a stream line of plain left-to-right text.
    usable: bool,
    /// It went elsewhere.
    lost: bool,
    /// What's gone into it, as left and right edges.
    inserted: Vec<(f32, f32)>,
}

/// What lies between two line breaks.
struct StreamLine {
    start: u32,
    /// Past its line breaks.
    end: u32,
    steps: u32,
    back: u32,
    jumps: u32,
    lost: bool,
}

impl StreamLine {
    fn plain(&self) -> bool {
        self.back.saturating_mul(ODD_STEPS) < self.steps.max(1) && self.jumps.saturating_mul(ODD_STEPS) < self.steps.max(1)
    }
}

/// A run on its way to a line.
#[derive(Clone, Copy)]
struct Move {
    /// Stream index it goes before.
    at: u32,
    left: f32,
    start: u32,
    end: u32,
    space_before: bool,
    space_after: bool,
}

/// Where a run would go in one line.
#[derive(Clone, Copy)]
struct Fit {
    in_a_gap: bool,
    end_gap: f32,
    at: u32,
    /// Where the run's stretch of the line starts: the edge of what it follows.
    reach_from: f32,
    space_before: bool,
    space_after: bool,
}

fn has_geometry(record: CharRecord, char_box: &CharBox) -> bool {
    let ch = record.ch();
    // A line-end hyphen counts, even though it's a format character, because it's what says where a line stops.
    let printed = is_hyphen_marker(record) || (!ch.is_whitespace() && !ch.is_control() && !record.has(CharRecord::GENERATED) && !is_invisible_format(ch));
    printed && char_box[2] >= char_box[0] && char_box[3] > char_box[1]
}

fn closes(record: CharRecord) -> bool {
    matches!(record.ch(), '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}')
}

fn is_hyphen_marker(record: CharRecord) -> bool {
    record.ch() == '\u{FFFE}' || record.has(CharRecord::HYPHEN)
}

/// Puts late runs into the lines they belong to. `None` if nothing moved, which is nearly every page, so nearly every page costs nothing more than this look.
pub fn repair_line_order(chars: &[CharRecord], boxes: &[CharBox]) -> Option<RepairedOrder> {
    if chars.len() != boxes.len() || chars.is_empty() || u32::try_from(chars.len()).is_err() {
        return None;
    }
    let (geo, mut lines, mut stream_lines) = read_lines(chars, boxes)?;
    let moves = find_moves(chars, boxes, &geo, &mut lines, &mut stream_lines);
    if moves.is_empty() {
        return None;
    }
    Some(rebuild(chars, moves, &stream_lines))
}

/// Splits the page into lines, or gives up past [`MAX_LINES`].
fn read_lines(chars: &[CharRecord], boxes: &[CharBox]) -> Option<(Vec<u32>, Vec<Line>, Vec<StreamLine>)> {
    let mut geo: Vec<u32> = Vec::with_capacity(chars.len());
    let mut lines: Vec<Line> = Vec::new();
    let mut stream_lines: Vec<StreamLine> = Vec::new();
    let mut line_start = 0u32;
    let mut current = StreamLine { start: 0, end: 0, steps: 0, back: 0, jumps: 0, lost: false };
    let mut previous: Option<usize> = None;
    let mut in_breaks = false;
    let close_line = |geo: &Vec<u32>, lines: &mut Vec<Line>, line_start: &mut u32, stream_line: usize| {
        let end = geo.len() as u32;
        if end > *line_start {
            lines.push(Line { geo: (*line_start, end), stream_line: stream_line as u32, baseline: 0.0, height: 0.0, left: 0.0, right: 0.0, low: 0.0, high: 0.0, usable: false, lost: false, inserted: Vec::new() });
        }
        *line_start = end;
    };
    for (index, record) in chars.iter().enumerate() {
        if is_break(record.ch()) {
            close_line(&geo, &mut lines, &mut line_start, stream_lines.len());
            in_breaks = true;
            previous = None;
            continue;
        }
        if in_breaks {
            current.end = index as u32;
            let next = StreamLine { start: index as u32, end: 0, steps: 0, back: 0, jumps: 0, lost: false };
            stream_lines.push(std::mem::replace(&mut current, next));
            in_breaks = false;
            // Lines with nothing printed on them count too, or a page of nothing but line breaks costs what a real page never does.
            if stream_lines.len() > MAX_LINES {
                return None;
            }
        }
        if !has_geometry(*record, &boxes[index]) {
            continue;
        }
        let here = &boxes[index];
        if let Some(last) = previous {
            let before = &boxes[last];
            let height = (here[3] - here[1]).max(before[3] - before[1]);
            current.steps += 1;
            if here[0] < before[0] - STEP_BACK * height {
                current.back += 1;
                close_line(&geo, &mut lines, &mut line_start, stream_lines.len());
            }
            if (here[1] - before[1]).abs() > height {
                current.jumps += 1;
            }
        }
        geo.push(index as u32);
        previous = Some(index);
        if lines.len() > MAX_LINES {
            return None;
        }
    }
    close_line(&geo, &mut lines, &mut line_start, stream_lines.len());
    current.end = chars.len() as u32;
    stream_lines.push(current);
    if lines.len() > MAX_LINES {
        return None;
    }
    for line in &mut lines {
        measure(line, &geo, boxes);
        line.usable = line.usable && stream_lines[line.stream_line as usize].plain();
    }
    Some((geo, lines, stream_lines))
}

/// A line's baseline is the middle of its characters' bottoms, since most letters sit on it and only a few hang below. Its height runs from the lowest to the highest of them.
fn measure(line: &mut Line, geo: &[u32], boxes: &[CharBox]) {
    let members = &geo[line.geo.0 as usize..line.geo.1 as usize];
    let mut bottoms = [0f32; SAMPLE];
    let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut count = 0;
    for &index in members.iter().take(SAMPLE) {
        let char_box = &boxes[index as usize];
        bottoms[count] = char_box[1];
        low = low.min(char_box[1]);
        high = high.max(char_box[3]);
        count += 1;
    }
    let (first, last) = (&boxes[members[0] as usize], &boxes[members[members.len() - 1] as usize]);
    let bottoms = &mut bottoms[..count];
    bottoms.sort_unstable_by(f32::total_cmp);
    line.baseline = bottoms[count / 2];
    line.height = (high - low).max(0.0);
    line.left = first[0];
    line.right = last[2];
    line.low = low;
    line.high = high;
    line.usable = (last[1] - first[1]).abs() <= SLOPE * line.height;
}

/// One run of a line: its range of characters, and its left and right edges.
#[derive(Clone, Copy)]
struct Run {
    start: u32,
    end: u32,
    left: f32,
    right: f32,
}

fn run_of(members: &[u32], boxes: &[CharBox]) -> Run {
    let (first, last) = (members[0], members[members.len() - 1]);
    Run { start: first, end: last + 1, left: boxes[first as usize][0], right: boxes[last as usize][2] }
}

fn find_moves(chars: &[CharRecord], boxes: &[CharBox], geo: &[u32], lines: &mut [Line], stream_lines: &mut [StreamLine]) -> Vec<Move> {
    let mut by_baseline: Vec<(f32, u32)> = lines.iter().enumerate().filter(|(_, line)| line.usable).map(|(index, line)| (line.baseline, index as u32)).collect();
    by_baseline.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let tallest = lines.iter().map(|line| line.height).fold(0.0, f32::max);
    let mut moves = Vec::new();
    let mut tried = 0usize;
    let mut hosts = [0u32; MAX_HOSTS_LOOKED];
    let mut runs: Vec<Run> = Vec::new();
    for k in 0..lines.len() {
        if tried > MAX_FITS_TRIED {
            break;
        }
        if !lines[k].usable {
            continue;
        }
        let found = nearby_lines(&by_baseline, lines, k, tallest, &mut hosts);
        if found == 0 {
            continue;
        }
        let members = &geo[lines[k].geo.0 as usize..lines[k].geo.1 as usize];
        // A hyphen ending a line belongs at the end of a line.
        if chars[members[0] as usize..=members[members.len() - 1] as usize].iter().any(|record| is_hyphen_marker(*record)) {
            continue;
        }
        // The whole line first. Failing that, pieces of it, if every one of them has somewhere to go.
        runs.clear();
        runs.push(run_of(members, boxes));
        tried += found;
        let mut placed = place_runs(chars, boxes, geo, lines, k, &hosts[..found], &runs);
        if placed.is_none() {
            let split_at = SPLIT_GAP * lines[k].height;
            runs.clear();
            let mut from = 0;
            for at in 0..members.len() {
                if at + 1 == members.len() || boxes[members[at + 1] as usize][0] - boxes[members[at] as usize][2] > split_at {
                    runs.push(run_of(&members[from..=at], boxes));
                    from = at + 1;
                }
            }
            if runs.len() > 1 {
                tried += found.saturating_mul(runs.len());
                placed = place_runs(chars, boxes, geo, lines, k, &hosts[..found], &runs);
            }
        }
        if let Some(placed) = placed {
            let stream_line = lines[k].stream_line as usize;
            lines[k].lost = true;
            stream_lines[stream_line].lost = true;
            moves.extend(placed);
        }
    }
    moves
}

/// Finds every run a place, or leaves everything as it was and says so.
fn place_runs(chars: &[CharRecord], boxes: &[CharBox], geo: &[u32], lines: &mut [Line], k: usize, hosts: &[u32], runs: &[Run]) -> Option<Vec<Move>> {
    let mut placed = Vec::with_capacity(runs.len());
    let mut used: Vec<u32> = Vec::with_capacity(runs.len());
    for run in runs {
        let mut best: Option<(u32, Fit)> = None;
        for &host in hosts {
            let (host_line, line) = (&lines[host as usize], &lines[k]);
            let height = if host_line.geo.1 - host_line.geo.0 >= line.geo.1 - line.geo.0 { host_line.height } else { line.height };
            if (host_line.baseline - line.baseline).abs() > SAME_BASELINE * height {
                continue;
            }
            let Some(fit) = fit_into(chars, boxes, geo, host_line, run, height) else { continue };
            if passes_something(lines, host as usize, k, fit.reach_from.min(run.left), run.right) {
                continue;
            }
            if best.is_none_or(|(_, so_far)| (fit.in_a_gap && !so_far.in_a_gap) || (fit.in_a_gap == so_far.in_a_gap && fit.end_gap < so_far.end_gap)) {
                best = Some((host, fit));
            }
        }
        let Some((host, fit)) = best else {
            for host in used {
                lines[host as usize].inserted.pop();
            }
            return None;
        };
        lines[host as usize].inserted.push((run.left, run.right));
        used.push(host);
        placed.push(Move { at: fit.at, left: run.left, start: run.start, end: run.end, space_before: fit.space_before, space_after: fit.space_after });
    }
    Some(placed)
}

/// Whether something drawn since the host, and still where it was drawn, sits on the host's line in the stretch this run would cross.
fn passes_something(lines: &[Line], host: usize, k: usize, from_x: f32, to_x: f32) -> bool {
    let host_line = &lines[host];
    lines[host + 1..k].iter().rev().take(BLOCKER_LOOK_BACK).any(|between| {
        if between.lost || between.right < from_x || between.left > to_x {
            return false;
        }
        let near = AGAINST_LINE * (host_line.high - host_line.low);
        between.low < host_line.high + near && between.high > host_line.low - near
    })
}

/// Earlier lines whose baselines are nearest this one's. Writes them to `hosts`, and returns how many.
fn nearby_lines(by_baseline: &[(f32, u32)], lines: &[Line], k: usize, tallest: f32, hosts: &mut [u32; MAX_HOSTS_LOOKED]) -> usize {
    let baseline = lines[k].baseline;
    let reach = SAME_BASELINE * tallest;
    let position = by_baseline.partition_point(|&(other, index)| other.total_cmp(&baseline).then(index.cmp(&(k as u32))).is_lt());
    let mut count = 0;
    let mut usable = |index: u32, count: &mut usize| {
        let line = &lines[index as usize];
        if (index as usize) < k && !line.lost && line.inserted.len() < MAX_INSERTS_PER_LINE && *count < MAX_HOSTS_LOOKED {
            hosts[*count] = index;
            *count += 1;
        }
    };
    // Half the looks each way, so a crowd of lines just under this baseline can't hide the line just over it.
    for &(other, index) in by_baseline[..position].iter().rev().take(MAX_HOSTS_LOOKED / 2) {
        if baseline - other > reach {
            break;
        }
        usable(index, &mut count);
    }
    for &(other, index) in by_baseline[position..].iter().take(MAX_HOSTS_LOOKED / 2 + 1) {
        if other - baseline > reach {
            break;
        }
        usable(index, &mut count);
    }
    count
}

/// Whether a run fits in this line, and where.
fn fit_into(chars: &[CharRecord], boxes: &[CharBox], geo: &[u32], line: &Line, run: &Run, height: f32) -> Option<Fit> {
    let middle = f32::midpoint(run.left, run.right);
    let slack = OVERLAP * height;
    let members = &geo[line.geo.0 as usize..line.geo.1 as usize];
    let split = members.partition_point(|&index| boxes[index as usize][0] < middle);
    let before = split.checked_sub(1).map(|at| members[at]);
    let after = members.get(split).copied();
    let mut left_edge = before.map(|index| boxes[index as usize][2]);
    let mut right_edge = after.map(|index| boxes[index as usize][0]);
    for &(other_left, other_right) in &line.inserted {
        if f32::midpoint(other_left, other_right) < middle {
            left_edge = Some(left_edge.map_or(other_right, |edge| edge.max(other_right)));
        } else {
            right_edge = Some(right_edge.map_or(other_left, |edge| edge.min(other_left)));
        }
    }
    if left_edge.is_some_and(|edge| edge > run.left + slack) || right_edge.is_some_and(|edge| edge < run.right - slack) {
        return None;
    }
    let gap_before = left_edge.map(|edge| run.left - edge);
    let gap_after = right_edge.map(|edge| edge - run.right);
    // Nothing printed goes after a hyphen that ends a line.
    if before.is_some_and(|index| is_hyphen_marker(chars[index as usize])) {
        return None;
    }
    let word = WORD_GAP * height;
    let in_a_gap = before.is_some() && after.is_some();
    let end_gap = match (gap_before, gap_after) {
        (Some(_), Some(_)) => 0.0,
        (Some(gap), None) | (None, Some(gap)) if gap <= END_GAP * height => gap,
        _ => return None,
    };
    if !in_a_gap && end_gap < word {
        // Touching at the end of a line, which says nothing about being one word. Punctuation can touch; letters and digits have to show they belong together.
        let (touching_left, touching_right) = match (before, after) {
            (Some(index), None) => (chars[index as usize], chars[run.start as usize]),
            (None, Some(index)) => (chars[(run.end - 1) as usize], chars[index as usize]),
            _ => return None,
        };
        if touching_left.ch().is_alphanumeric() && touching_right.ch().is_alphanumeric() {
            return None;
        }
    }
    let at = match before {
        Some(index) => {
            let next = index as usize + 1;
            // The space that followed the last word stays with it.
            if chars.get(next).is_some_and(|record| record.ch().is_whitespace() && !is_break(record.ch())) { next as u32 + 1 } else { next as u32 }
        }
        None => members[0],
    };
    // Nothing puts a space before a full stop or a closing bracket, however wide the gap is drawn.
    let space_before = gap_before.is_some_and(|gap| gap >= word) && !closes(chars[run.start as usize]);
    let space_after = gap_after.is_some_and(|gap| gap >= word) && !after.is_some_and(|index| closes(chars[index as usize]));
    Some(Fit { in_a_gap, end_gap, at, reach_from: left_edge.unwrap_or(run.left), space_before, space_after })
}

/// Lays the characters out again. Only whitespace is ever left out or made up.
fn rebuild(chars: &[CharRecord], mut moves: Vec<Move>, stream_lines: &[StreamLine]) -> RepairedOrder {
    let runs_moved = moves.len() as u32;
    // Moves were found in stream order, so the moved ranges are already sorted.
    let mut skipped: Vec<(u32, u32)> = moves.iter().map(|one| (one.start, one.end)).collect();
    // A line that's lost all it had leaves nothing behind, not even a blank line.
    let mut emptied: Vec<(u32, u32)> = Vec::new();
    let mut in_skipped = 0;
    for line in stream_lines {
        while skipped.get(in_skipped).is_some_and(|range| range.1 <= line.start) {
            in_skipped += 1;
        }
        if !line.lost {
            continue;
        }
        let mut cursor = line.start;
        let mut scan = in_skipped;
        let mut empty = true;
        while cursor < line.end {
            if let Some(range) = skipped.get(scan)
                && range.0 <= cursor
            {
                cursor = range.1.max(cursor + 1);
                scan += 1;
                continue;
            }
            if !chars[cursor as usize].ch().is_whitespace() {
                empty = false;
                break;
            }
            cursor += 1;
        }
        if empty {
            emptied.push((line.start, line.end));
        }
    }
    if !emptied.is_empty() {
        skipped.extend(emptied);
        skipped.sort_unstable();
    }
    moves.sort_unstable_by(|a, b| a.at.cmp(&b.at).then(a.left.total_cmp(&b.left)));

    let mut out = Vec::with_capacity(chars.len() + moves.len() * 2);
    let mut origin = Vec::with_capacity(chars.len() + moves.len() * 2);
    let mut pending_space = false;
    let push = |out: &mut Vec<CharRecord>, origin: &mut Vec<u32>, pending: &mut bool, index: u32| {
        let record = chars[index as usize];
        if std::mem::take(pending) && !record.ch().is_whitespace() {
            out.push(CharRecord::new(' ' as u32, CharRecord::GENERATED));
            origin.push(GENERATED);
        }
        out.push(record);
        origin.push(index);
    };
    let place = |out: &mut Vec<CharRecord>, origin: &mut Vec<u32>, pending: &mut bool, one: &Move| {
        if one.space_before && out.last().is_some_and(|last: &CharRecord| !last.ch().is_whitespace()) {
            *pending = true;
        }
        for index in one.start..one.end {
            push(out, origin, pending, index);
        }
        *pending = one.space_after;
    };
    let mut next_move = 0;
    let mut next_skip = 0;
    let mut index = 0u32;
    while (index as usize) < chars.len() {
        while let Some(one) = moves.get(next_move).filter(|one| one.at <= index) {
            place(&mut out, &mut origin, &mut pending_space, one);
            next_move += 1;
        }
        while skipped.get(next_skip).is_some_and(|range| range.1 <= index) {
            next_skip += 1;
        }
        if let Some(range) = skipped.get(next_skip)
            && range.0 <= index
        {
            index = range.1;
            continue;
        }
        push(&mut out, &mut origin, &mut pending_space, index);
        index += 1;
    }
    for one in &moves[next_move..] {
        place(&mut out, &mut origin, &mut pending_space, one);
    }
    RepairedOrder { chars: out, origin, runs_moved }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const WIDTH: f32 = 5.0;
    const SPACE: f32 = 2.5;
    const TOP: f32 = 7.0;

    /// Draws text on a page: each string starts at its `x` on its baseline `y`, with a real space between words and a line break after.
    struct Page {
        chars: Vec<CharRecord>,
        boxes: Vec<CharBox>,
    }

    impl Page {
        fn new() -> Self {
            Self { chars: Vec::new(), boxes: Vec::new() }
        }

        fn line(mut self, x: f32, y: f32, text: &str) -> Self {
            if !self.chars.is_empty() {
                self.chars.push(CharRecord::new('\r' as u32, CharRecord::GENERATED));
                self.boxes.push([0.0; 4]);
                self.chars.push(CharRecord::new('\n' as u32, CharRecord::GENERATED));
                self.boxes.push([0.0; 4]);
            }
            self.draw(x, y, text)
        }

        /// As `line`, with no break before it, the way PDFium runs together what shares a baseline.
        fn draw(mut self, mut x: f32, y: f32, text: &str) -> Self {
            for ch in text.chars() {
                self.chars.push(CharRecord::new(ch as u32, 0));
                if ch == ' ' {
                    self.boxes.push([x, y, x + SPACE, y + TOP]);
                    x += SPACE;
                } else {
                    self.boxes.push([x, y, x + WIDTH, y + TOP]);
                    x += WIDTH;
                }
            }
            self
        }

        fn repaired(&self) -> String {
            let order = repair_line_order(&self.chars, &self.boxes);
            let chars = order.as_ref().map_or(&self.chars, |order| &order.chars);
            read(chars)
        }
    }

    fn read(chars: &[CharRecord]) -> String {
        crate::normalize(chars, false).text
    }

    fn x_of_word(words_before: &str) -> f32 {
        words_before.chars().map(|ch| if ch == ' ' { SPACE } else { WIDTH }).sum()
    }

    fn non_space(chars: &[CharRecord]) -> HashMap<u32, u32> {
        let mut counts = HashMap::new();
        for record in chars.iter().filter(|record| !record.ch().is_whitespace()) {
            *counts.entry(record.bits()).or_insert(0) += 1;
        }
        counts
    }

    /// Failure: a word drawn late stays at the bottom of the page, so `A recipient ignore` reads the opposite of `A recipient MUST ignore` and a search for the phrase finds nothing. RFC 9110 p. 110.
    #[test]
    fn a_word_drawn_late_goes_into_the_gap_on_its_line() {
        let start = x_of_word("A recipient MUST ");
        let page = Page::new().draw(0.0, 700.0, "A recipient ").draw(start, 700.0, "ignore this").line(0.0, 686.0, "the next line").line(x_of_word("A recipient "), 700.0, "MUST");
        assert_eq!(page.repaired(), "A recipient MUST ignore this\nthe next line\n");
    }

    /// Failure: only the word in the middle of a line is put back, so the word that starts or ends a line stays lost. RFC 9110's `2.` and `Section`.
    #[test]
    fn a_word_at_either_end_of_a_line_comes_back_to_it() {
        let page = Page::new().line(20.0, 700.0, "field value").line(0.0, 686.0, "are defined in").line(x_of_word("are defined in "), 686.0, "Section").line(20.0 - WIDTH * 2.0 - SPACE, 700.0, "2.");
        assert_eq!(page.repaired(), "2. field value\nare defined in Section\n");
    }

    /// Failure: two late words on one line go in the wrong order, or the second is refused because the first took the gap. PDFium runs them together with one gap between them, so a whole-run rule would miss both.
    #[test]
    fn several_late_words_on_a_line_each_find_their_place() {
        let start_may = x_of_word("he MUST go ");
        let start_end = x_of_word("he MUST go MAY ");
        let page = Page::new().draw(0.0, 700.0, "he ").draw(x_of_word("he MUST "), 700.0, "go ").draw(start_end, 700.0, "stay").line(x_of_word("he "), 700.0, "MUST").draw(start_may, 700.0, "MAY");
        assert_eq!(page.repaired(), "he MUST go MAY stay\n");
    }

    /// Failure: a run's second column lands inside the first, so text from two columns is read as one sentence. Owner: two columns never interleave, whichever order they're drawn in.
    #[test]
    fn two_columns_never_interleave() {
        let (right, lines) = (300.0, ["left one here", "left two here", "left three now"]);
        let mut in_order = Page::new();
        for (row, line) in lines.iter().enumerate() {
            in_order = in_order.line(0.0, 700.0 - 14.0 * row as f32, line);
        }
        let mut reversed = Page::new();
        for row in 0..3 {
            in_order = in_order.line(right, 700.0 - 14.0 * row as f32, "right column words");
            reversed = reversed.line(right, 700.0 - 14.0 * row as f32, "right column words");
        }
        for (row, line) in lines.iter().enumerate() {
            reversed = reversed.line(0.0, 700.0 - 14.0 * row as f32, line);
        }
        assert!(repair_line_order(&in_order.chars, &in_order.boxes).is_none(), "{}", in_order.repaired());
        assert!(repair_line_order(&reversed.chars, &reversed.boxes).is_none(), "{}", reversed.repaired());
    }

    /// Failure: a late word in the left column goes to the right of the wrong line, or its neighbour in the right column is taken for its line.
    #[test]
    fn a_late_word_stays_in_its_own_column() {
        let page = Page::new().draw(0.0, 700.0, "left ").draw(x_of_word("left MUST "), 700.0, "side").line(300.0, 700.0, "right ").draw(300.0 + x_of_word("right MUST "), 700.0, "side").line(x_of_word("left "), 700.0, "MUST").line(300.0 + x_of_word("right "), 700.0, "MUST");
        assert_eq!(page.repaired(), "left MUST side\nright MUST side\n");
    }

    /// Failure: Hebrew or Arabic, whose characters run backwards along the page, is taken for words drawn late, and reversed.
    #[test]
    fn text_running_right_to_left_is_left_alone() {
        let mut page = Page::new();
        for (at, ch) in "backwards".chars().enumerate() {
            page = page.draw(100.0 - WIDTH * at as f32, 700.0, &ch.to_string());
        }
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
    }

    /// Failure: a line running up the page, as on a sideways figure label, is read as many one-letter lines and shuffled into whatever's beside it.
    #[test]
    fn text_running_up_the_page_is_left_alone() {
        let mut page = Page::new().draw(0.0, 700.0, "a line here");
        for (at, ch) in "vertical".chars().enumerate() {
            page = page.line(WIDTH + SPACE, 500.0 + 8.0 * at as f32, &ch.to_string());
        }
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
    }

    /// Failure: a word at another height on the page lands in a line it isn't on, or something raised does.
    #[test]
    fn a_different_baseline_is_a_different_line() {
        let page = Page::new().draw(0.0, 700.0, "see ").draw(x_of_word("see xxxx "), 700.0, "more").line(x_of_word("see "), 700.0 + TOP * 1.2, "note");
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
        let wide = x_of_word("see ") + x_of_word("a long run of words here") + 10.0;
        let long = Page::new().draw(0.0, 700.0, "see ").draw(wide, 700.0, "more").line(x_of_word("see "), 700.0 + TOP * 0.5, "a long run of words here");
        assert!(repair_line_order(&long.chars, &long.boxes).is_none(), "{}", long.repaired());
        let raised = Page::new().draw(0.0, 700.0, "see ").draw(x_of_word("see xxxx "), 700.0, "more").line(x_of_word("see "), 700.0 + TOP * 0.5, "note");
        assert!(repair_line_order(&raised.chars, &raised.boxes).is_none(), "a superscript isn't on the line, and stays where it was drawn: {}", raised.repaired());
    }

    /// Failure: the rest of a line moves up and leaves its superscript behind, so `F -adapted` reads as though `F` had no prime, which is worse than the three short lines PDFium gave, in the right order. A real paper, math_AG p. 34.
    #[test]
    fn a_line_doesnt_leave_its_superscript_behind() {
        let page = Page::new().line(0.0, 700.0, "let F").line(x_of_word("let F"), 700.0 + 3.5, "'").line(x_of_word("let F'"), 700.0, " be the pullback");
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
    }

    /// Failure: what follows a fraction jumps ahead of its numerator and denominator, leaving them after the equation. In drawing order they were in the right place, so moving made it worse.
    #[test]
    fn a_run_doesnt_jump_over_what_was_drawn_between() {
        let page = Page::new().line(0.0, 700.0, "x =").line(20.0, 700.0 + 6.0, "numerator").line(20.0, 700.0 - 6.0, "denominator").line(x_of_word("x = denominator "), 700.0, "+ y");
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
    }

    /// Failure: half a line moves and the other half doesn't, so one sentence ends up on two lines with others between. `Since A is klt` after a wide gap, in a real paper.
    #[test]
    fn all_of_a_line_moves_or_none_of_it() {
        let whole = Page::new().line(0.0, 700.0, "where B").line(x_of_word("where B "), 700.0, "are effective.").draw(x_of_word("where B are effective.") + 8.0, 700.0, "Since A");
        assert_eq!(whole.repaired(), "where B are effective.Since A\n", "the page's own characters, with no space between them here");
        // The first run has a gap to go to, and the second has nowhere.
        let part = Page::new().draw(0.0, 700.0, "a ").draw(x_of_word("a MUST "), 700.0, "b").line(x_of_word("a "), 700.0, "MUST").draw(x_of_word("a MUST b") + 200.0, 700.0, "far");
        assert!(repair_line_order(&part.chars, &part.boxes).is_none(), "{}", part.repaired());
    }

    /// Failure: two labels that happen to touch on a map are joined into a word that doesn't exist, `AREAPRL`, which no search will find.
    #[test]
    fn letters_touching_at_a_lines_end_stay_apart() {
        let page = Page::new().line(0.0, 700.0, "AREA").line(x_of_word("AREA"), 700.0, "PRL 051");
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
        let full_stop = Page::new().line(x_of_word("end"), 700.0, ".").line(0.0, 700.0, "end");
        assert_eq!(full_stop.repaired(), "end.\n", "punctuation can touch");
        let apart = Page::new().line(x_of_word("end") + 3.0, 700.0, ".").line(0.0, 700.0, "end");
        assert_eq!(apart.repaired(), "end.\n", "and gets no space before it, as RFC 9110's `[CACHING].` drawn a little apart");
    }

    /// Failure: labels on a slope, as on a chart's axis, have matching baselines by accident, and are run together: `qwen3-vl-8bqwen3-vl-235b`.
    #[test]
    fn labels_on_a_slope_are_left_alone() {
        let mut page = Page::new();
        for label in 0..3 {
            let x = label as f32 * 30.0;
            page = page.line(x, 700.0, "s");
            for (step, ch) in "loped label".chars().enumerate() {
                page = page.draw(x + 5.0 * (step + 1) as f32, 700.0 + 3.0 * (step + 1) as f32, &ch.to_string());
            }
        }
        assert!(repair_line_order(&page.chars, &page.boxes).is_none(), "{}", page.repaired());
    }

    /// Failure: text drawn twice, as a bold effect, or a run overprinting another, is moved on top of what it overlaps.
    #[test]
    fn a_run_that_overlaps_a_line_isnt_moved() {
        let page = Page::new().line(0.0, 700.0, "printed twice").line(0.0, 700.0, "printed twice");
        assert!(repair_line_order(&page.chars, &page.boxes).is_none());
    }

    /// Failure: a word broken at a line's end moves away from the rest of itself, or a late word lands after the hyphen, so the word can't join up.
    #[test]
    fn a_line_end_hyphen_stays_put() {
        let mut page = Page::new().draw(0.0, 700.0, "valu");
        page.chars.push(CharRecord::new(0xFFFE, CharRecord::HYPHEN));
        page.boxes.push([20.0, 700.0, 23.0, 700.0 + TOP]);
        let page = page.line(24.0, 700.0, "word").line(0.0, 686.0, "able");
        let order = repair_line_order(&page.chars, &page.boxes);
        assert!(order.is_none(), "{:?}", order.map(|order| read(&order.chars)));
    }

    /// Failure: a style change inside a word gets a space, so `re-do` with its middle drawn late reads `re do` rather than `redo`.
    #[test]
    fn a_change_of_style_inside_a_word_adds_no_space() {
        let page = Page::new().draw(0.0, 700.0, "un").draw(WIDTH * 4.0, 700.0, "done").line(WIDTH * 2.0, 700.0, "do");
        assert_eq!(page.repaired(), "undodone\n");
    }

    /// Failure: a moved run leaves its own line behind as a blank, or a line with something still on it is thrown away.
    #[test]
    fn only_a_line_with_nothing_left_goes() {
        let page = Page::new().draw(0.0, 700.0, "a ").draw(x_of_word("a MUST "), 700.0, "b").line(0.0, 686.0, "next").line(x_of_word("a "), 700.0, "MUST ").line(0.0, 672.0, "after");
        assert_eq!(page.repaired(), "a MUST b\nnext\nafter");
        // A line with a word that can't go anywhere keeps its other word too, and the line stays as it was.
        let keeps = Page::new().draw(0.0, 700.0, "a ").draw(x_of_word("a MUST "), 700.0, "b").line(0.0, 500.0, "stays").draw(100.0, 500.0, "here").line(x_of_word("a "), 700.0, "MUST");
        assert_eq!(keeps.repaired(), "a MUST b\nstayshere\n");
    }

    /// Failure: `origin` doesn't say where a character came from, so hidden text can't be marked once a run has moved.
    #[test]
    fn origin_says_where_each_character_came_from() {
        let page = Page::new().draw(0.0, 700.0, "a ").draw(x_of_word("a MUST "), 700.0, "b").line(x_of_word("a "), 700.0, "MUST");
        let order = repair_line_order(&page.chars, &page.boxes).unwrap();
        assert_eq!(order.runs_moved, 1);
        assert_eq!(order.chars.len(), order.origin.len());
        for (record, &from) in order.chars.iter().zip(&order.origin) {
            if from == GENERATED {
                assert!(record.ch().is_whitespace() && record.has(CharRecord::GENERATED));
            } else {
                assert_eq!(*record, page.chars[from as usize]);
            }
        }
    }

    /// Failure: a line's record grows without anyone deciding it should, and a hostile page of many lines costs more than was reckoned on.
    #[test]
    fn a_line_stays_small() {
        assert!(size_of::<Line>() <= 72, "{}", size_of::<Line>());
        assert!(size_of::<StreamLine>() <= 32, "{}", size_of::<StreamLine>());
    }

    /// Failure: a page of nothing but line breaks, which has no lines to count, still grows a record for each.
    #[test]
    fn a_page_of_nothing_but_breaks_is_turned_away() {
        let chars: Vec<CharRecord> = (0..MAX_LINES * 3).map(|at| CharRecord::new(if at % 2 == 0 { ' ' } else { '\n' } as u32, 0)).collect();
        let boxes = vec![[0.0; 4]; chars.len()];
        assert!(read_lines(&chars, &boxes).is_none());
    }

    /// A small deterministic generator, so a failure here happens every time.
    struct Dice(u64);

    impl Dice {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }

        fn below(&mut self, limit: u32) -> u32 {
            self.next() % limit
        }
    }

    /// Failure: on some arrangement of characters, one is lost, doubled or made up, or the code panics. That's the first thing a hostile page would try. Runs thousands of arrangements with broken boxes among them.
    #[test]
    fn no_arrangement_changes_the_characters_or_panics() {
        let mut dice = Dice(7);
        let alphabet: Vec<char> = "abcMUST .,-\u{FFFE}\u{200B}\r\n\t😀".chars().collect();
        let odd_values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, 0.0, -0.0, 1e-30];
        let mut moved_some = 0;
        for _ in 0..4000 {
            let count = dice.below(120) as usize;
            let mut chars = Vec::with_capacity(count);
            let mut boxes = Vec::with_capacity(count);
            let (mut x, mut y) = (0.0f32, 0.0f32);
            for _ in 0..count {
                let ch = alphabet[dice.below(alphabet.len() as u32) as usize];
                // Lines at a few heights, in pieces with gaps, drawn out of order, so runs often do fit somewhere. Sometimes a step back with no line break.
                if matches!(ch, '\r' | '\n') || dice.below(30) == 0 {
                    (x, y) = (dice.below(40) as f32 * 2.5, dice.below(3) as f32 * 14.0);
                } else if dice.below(6) == 0 {
                    x += dice.below(8) as f32 * 2.5;
                }
                x += 5.0;
                let flags = match dice.below(10) {
                    0 => CharRecord::GENERATED,
                    1 => CharRecord::HYPHEN,
                    2 => CharRecord::NO_INK,
                    _ => 0,
                };
                chars.push(CharRecord::new(ch as u32, flags));
                boxes.push(if dice.below(40) == 0 { [odd_values[dice.below(8) as usize], y, odd_values[dice.below(8) as usize], y + TOP] } else { [x, y, x + 4.0, y + TOP] });
            }
            if let Some(order) = repair_line_order(&chars, &boxes) {
                moved_some += 1;
                assert_eq!(non_space(&order.chars), non_space(&chars), "{chars:?} {boxes:?}");
                assert_eq!(order.chars.len(), order.origin.len());
                let mut seen = vec![false; chars.len()];
                for &from in order.origin.iter().filter(|from| **from != GENERATED) {
                    assert!(!seen[from as usize], "a character came out twice");
                    seen[from as usize] = true;
                }
            }
        }
        assert!(moved_some > 100, "only {moved_some} arrangements moved anything, so this tested little");
    }

    /// Failure: a hostile page makes the look for a line quadratic, stalling a search for minutes, or the work done is out of proportion to the characters. Four shapes of the largest page the wire allows.
    #[test]
    fn the_largest_hostile_pages_stay_cheap() {
        let count = recto_proto::MAX_PAGE_CHARS as usize;
        let record = CharRecord::new('x' as u32, 0);
        let line_break = CharRecord::new('\n' as u32, CharRecord::GENERATED);
        // As many lines as it will look at, of as many characters as fit, all on one baseline, with every character far enough from the next to count as a run of its own.
        let per_line = count / MAX_LINES + 1;
        let crowd = move |at: usize| {
            let (row, column) = (at / per_line, at % per_line);
            if column == per_line - 1 {
                (line_break, [0.0; 4])
            } else {
                let x = column as f32 * 40.0 + (row % 3) as f32 * 13.0;
                (record, [x, 700.0, x + 4.0, 707.0])
            }
        };
        // As many as it will look at again, each with gaps that later lines really do go into, so a lot moves.
        let many_moves = move |at: usize| {
            let row = at / 200;
            let column = at % 200;
            let y = (row % 4096) as f32 * 20.0;
            if column == 199 {
                (line_break, [0.0; 4])
            } else if row < 4096 {
                (record, [column as f32 * 40.0, y, column as f32 * 40.0 + 4.0, y + 7.0])
            } else if column < 30 {
                (record, [column as f32 * 40.0 + 20.0, y, column as f32 * 40.0 + 24.0, y + 7.0])
            } else {
                (CharRecord::new(' ' as u32, 0), [0.0; 4])
            }
        };
        type Shape = Box<dyn Fn(usize) -> (CharRecord, CharBox)>;
        let shapes: [(&str, Shape); 6] = [
            ("the most lines it will look at, on one baseline, every character its own run", Box::new(crowd)),
            ("many later lines with runs that fit", Box::new(many_moves)),
            ("one baseline, many short lines, each in a gap of the last", Box::new(move |at| if at % 2 == 1 { (line_break, [0.0; 4]) } else { (record, [at as f32 * 0.25, 700.0, at as f32 * 0.25 + 0.2, 707.0]) })),
            ("every character a step back", Box::new(move |at| (record, [2_000_000.0 - at as f32 * 8.0, 700.0, 2_000_000.0 - at as f32 * 8.0 + 4.0, 707.0]))),
            ("many gaps in one line", Box::new(move |at| (record, [at as f32 * 20.0, 700.0, at as f32 * 20.0 + 4.0, 707.0]))),
            ("lines of ten at the same place", Box::new(move |at| if at % 11 == 10 { (line_break, [0.0; 4]) } else { (record, [(at % 11) as f32 * 40.0, 700.0, (at % 11) as f32 * 40.0 + 4.0, 707.0]) })),
        ];
        for (name, shape) in &shapes {
            let (chars, boxes): (Vec<_>, Vec<_>) = (0..count).map(shape).unzip();
            let started = std::time::Instant::now();
            let order = repair_line_order(&chars, &boxes);
            // Far above a linear run on any machine, far below a quadratic one.
            let took = started.elapsed();
            assert!(took < std::time::Duration::from_secs(60), "{name} took {took:?}");
            eprintln!("{name}: {took:?}, {} runs moved", order.as_ref().map_or(0, |order| order.runs_moved));
            if name.starts_with("many later lines") {
                assert!(order.as_ref().is_some_and(|order| order.runs_moved > 10_000), "this shape is meant to move a lot, or it tests nothing");
            }
            if let Some(order) = order {
                assert_eq!(non_space(&order.chars), non_space(&chars), "{name}");
            }
        }
    }
}
