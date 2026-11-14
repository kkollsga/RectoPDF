//! **Highlights already in a file,** read through the lazy reader for `recto highlights` (R4.3). Geometry and colour only: an annotation's
//! `/Contents`, `/T`, `/NM` and every other string are never read, so nothing a file says in one can reach an agent or become a command.
//!
//! Everything here comes from a hostile file, so it's bounded and judged here (AGENTS.md rule 6):
//! - at most [`MAX_PAGE_ANNOTS`] annotations looked at on a page, [`MAX_PAGE_HIGHLIGHTS`] highlights kept, and [`MAX_QUADS`] quadrilaterals each;
//!   past any of them `truncated` says so;
//! - `/QuadPoints` of a length that isn't a multiple of eight keeps its whole quadrilaterals, and one with none uses `/Rect`; a number that isn't
//!   finite drops its quadrilateral;
//! - every rectangle given is finite, of some area, and inside the page as it's shown, clamped to it (coordinator, 2026-11-14).
//!
//! Rectangles are in the page's frame, as `recto select` and links give them: points from the top left, y down, past the crop and turned by the
//! page's own `/Rotate`. Removing a file's highlight isn't built yet (the coordinator's scope cut, 2026-11-14), so nothing here writes.

use crate::nav::{Frame, frame_of};
use crate::{OpenError, deref, open_bytes, open_reader_cached};
use lopdf::{Dictionary, IndexedReader, Object};
use std::path::Path;
use std::sync::Arc;

/// Most annotations looked at on one page, highlights or not.
pub const MAX_PAGE_ANNOTS: usize = 8192;
/// Most highlights kept from one page.
pub const MAX_PAGE_HIGHLIGHTS: usize = 256;
/// Most quadrilaterals kept from one highlight: a line each.
pub const MAX_QUADS: usize = 256;

/// One highlight of the file's.
#[derive(Clone, Debug, PartialEq)]
pub struct FileHighlight {
    /// Its place in the page's `/Annots`, from 0: what names it while the file is the same.
    pub index: u32,
    /// Its lines, `[x, y, width, height]` in the page's frame, each inside the page.
    pub rects: Vec<[f32; 4]>,
    /// Its colour, `0xRRGGBB`, if it says one that can be read.
    pub rgb: Option<u32>,
}

/// A page's highlights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageHighlights {
    pub frame: Frame,
    pub found: Vec<FileHighlight>,
    /// More than the limits keep, or annotations past what's looked at.
    pub truncated: bool,
}

/// The highlights on one page of a file, zero-based. A page that isn't there, or can't be read, has none.
pub fn page_highlights(path: &Path, page: u32) -> Result<PageHighlights, OpenError> {
    Ok(highlights_of(&open_reader_cached(path)?, page, Limits::default()))
}

/// [`page_highlights`] on bytes in memory, for the fuzzer.
pub fn page_highlights_bytes(bytes: Arc<[u8]>, page: u32) -> Result<PageHighlights, OpenError> {
    Ok(highlights_of(&open_bytes(bytes)?, page, Limits::default()))
}

/// The limits a read keeps to, set small by the tests so each is held at itself plus one.
#[derive(Clone, Copy)]
struct Limits {
    annots: usize,
    highlights: usize,
    quads: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            annots: MAX_PAGE_ANNOTS,
            highlights: MAX_PAGE_HIGHLIGHTS,
            quads: MAX_QUADS,
        }
    }
}

fn highlights_of(reader: &IndexedReader, page: u32, limits: Limits) -> PageHighlights {
    let Some(entry) = reader.page_map().ok().and_then(|map| map.get(page as usize).cloned()) else {
        return PageHighlights::default();
    };
    let frame = frame_of(reader, &entry);
    let mut out = PageHighlights {
        frame,
        ..PageHighlights::default()
    };
    let Some(size) = frame.shown_size() else { return out };
    let annots = match reader
        .resolve_object(entry.id())
        .ok()
        .and_then(|dict| dict.as_dict().ok()?.get(b"Annots").ok().and_then(|annots| deref(reader, annots)))
    {
        Some(Object::Array(items)) => items,
        _ => return out,
    };
    out.truncated = annots.len() > limits.annots;
    for (index, item) in annots.iter().take(limits.annots).enumerate() {
        let Some(Object::Dictionary(annot)) = deref(reader, item) else { continue };
        if !matches!(annot.get(b"Subtype"), Ok(Object::Name(kind)) if kind == b"Highlight") {
            continue;
        }
        if out.found.len() >= limits.highlights {
            out.truncated = true;
            break;
        }
        let (rects, cut) = rects_of(reader, &annot, frame, size, limits.quads);
        out.truncated |= cut;
        if rects.is_empty() {
            continue;
        }
        out.found.push(FileHighlight {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            rects,
            rgb: colour_of(reader, &annot),
        });
    }
    out
}

/// A number the file says, if it's one and finite.
fn number(reader: &IndexedReader, object: &Object) -> Option<f32> {
    match deref(reader, object)? {
        Object::Integer(value) => Some(value as f32),
        Object::Real(value) => Some(value),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

/// A highlight's lines as shown, clamped to the page: its whole quadrilaterals, or its `/Rect` if it has none. Says whether any were cut off.
fn rects_of(reader: &IndexedReader, annot: &Dictionary, frame: Frame, (width, height): (f32, f32), max: usize) -> (Vec<[f32; 4]>, bool) {
    let numbers: Vec<Option<f32>> = match annot.get(b"QuadPoints").ok().and_then(|quads| deref(reader, quads)) {
        Some(Object::Array(items)) => items.iter().take(max.saturating_add(1) * 8).map(|item| number(reader, item)).collect(),
        _ => Vec::new(),
    };
    let mut boxes: Vec<[f32; 4]> = numbers
        .as_chunks::<8>()
        .0
        .iter()
        .filter_map(|quad| {
            let quad: Vec<f32> = quad.iter().copied().collect::<Option<_>>()?;
            let xs = [quad[0], quad[2], quad[4], quad[6]];
            let ys = [quad[1], quad[3], quad[5], quad[7]];
            Some([
                xs.iter().copied().fold(f32::MAX, f32::min),
                ys.iter().copied().fold(f32::MAX, f32::min),
                xs.iter().copied().fold(f32::MIN, f32::max),
                ys.iter().copied().fold(f32::MIN, f32::max),
            ])
        })
        .collect();
    let cut = boxes.len() > max;
    boxes.truncate(max);
    if boxes.is_empty()
        && let Some(Object::Array(rect)) = annot.get(b"Rect").ok().and_then(|rect| deref(reader, rect))
        && rect.len() == 4
        && let Some(rect) = rect.iter().map(|edge| number(reader, edge)).collect::<Option<Vec<f32>>>()
    {
        boxes.push([rect[0], rect[1], rect[2], rect[3]]);
    }
    // Held to the page before anything else, so a quadrilateral of huge numbers is the page's edge, never infinity.
    let shown = boxes
        .into_iter()
        .map(|[x0, y0, x1, y1]| {
            let x = |value: f32| value.clamp(frame.left, frame.right);
            let y = |value: f32| value.clamp(frame.bottom, frame.top);
            [x(x0), y(y0), x(x1), y(y1)]
        })
        .filter_map(|user| frame.to_shown(user))
        .filter_map(|[x0, y0, x1, y1]| {
            let (x0, y0, x1, y1) = (x0.clamp(0.0, width), y0.clamp(0.0, height), x1.clamp(0.0, width), y1.clamp(0.0, height));
            (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
        })
        .collect();
    (shown, cut)
}

/// `/C` of three numbers from 0 to 1, as `0xRRGGBB`.
fn colour_of(reader: &IndexedReader, annot: &Dictionary) -> Option<u32> {
    let Some(Object::Array(items)) = annot.get(b"C").ok().and_then(|colour| deref(reader, colour)) else {
        return None;
    };
    if items.len() != 3 {
        return None;
    }
    let mut rgb = 0u32;
    for item in &items {
        let value = number(reader, item).filter(|value| (0.0..=1.0).contains(value))?;
        rgb = rgb << 8 | (value * 255.0).round() as u32;
    }
    Some(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_testpdf::{Object as Raw, build};

    /// A page of 600 by 800 with these annotations, written in place in its `/Annots`.
    fn page_with(annots: &[&str], page_extra: &str) -> IndexedReader {
        let list = annots.join(" ");
        let bytes = build(&[
            Raw::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            Raw::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            Raw::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] {page_extra} /Annots [{list}] >>")),
        ]);
        open_bytes(bytes.into()).unwrap()
    }

    const ONE: &str = "<< /Subtype /Highlight /Rect [0 0 1 1] /QuadPoints [50 720 150 720 50 708 150 708] /C [1 1 0] /Contents (run me) >>";

    /// Failure: a highlight in a file is read somewhere other than where it shows, with the wrong colour, or what it says in words comes along.
    #[test]
    fn a_files_highlight_is_read_where_it_shows() {
        let read = highlights_of(&page_with(&["<< /Subtype /Link /Rect [0 0 9 9] >>", ONE], ""), 0, Limits::default());
        assert_eq!(
            read.found,
            vec![FileHighlight {
                index: 1,
                rects: vec![[50.0, 80.0, 100.0, 12.0]],
                rgb: Some(0xFFFF00)
            }]
        );
        assert!(!read.truncated);
        let turned = highlights_of(&page_with(&[ONE], "/Rotate 90"), 0, Limits::default());
        assert_eq!(turned.found[0].rects, vec![[708.0, 50.0, 12.0, 100.0]]);
    }

    /// Failure: hostile `/QuadPoints` (a length that isn't a multiple of eight, NaN, a number that isn't one, a quadrilateral far off the page,
    /// none at all) panics, reaches the host unclamped, or drops a highlight that has a sound `/Rect`.
    #[test]
    fn hostile_quad_points_are_judged_here() {
        let cases = [
            (
                "<< /Subtype /Highlight /QuadPoints [50 720 150 720 50 708 150 708 1 2 3] >>",
                vec![[50.0, 80.0, 100.0, 12.0]],
            ),
            (
                "<< /Subtype /Highlight /QuadPoints [50 720 150 720 50 708 150 (x)] /Rect [10 10 20 20] >>",
                vec![[10.0, 780.0, 10.0, 10.0]],
            ),
            (
                "<< /Subtype /Highlight /QuadPoints [-1000000000000 -1000000000000 1000000000000 1000000000000.5 0 0 1 1] >>",
                vec![[0.0, 0.0, 600.0, 800.0]],
            ),
            ("<< /Subtype /Highlight /QuadPoints [700 900 800 900 700 850 800 850] >>", vec![]),
            ("<< /Subtype /Highlight /QuadPoints 7 >>", vec![]),
            ("<< /Subtype /Highlight /C [2 0 0] /Rect [10 10 20 20] >>", vec![[10.0, 780.0, 10.0, 10.0]]),
        ];
        for (annot, rects) in cases {
            let read = highlights_of(&page_with(&[annot], ""), 0, Limits::default());
            let got: Vec<Vec<[f32; 4]>> = read.found.iter().map(|found| found.rects.clone()).collect();
            let want: Vec<Vec<[f32; 4]>> = if rects.is_empty() { vec![] } else { vec![rects] };
            assert_eq!(got, want, "{annot}");
            assert!(read.found.iter().all(|found| found.rgb.is_none_or(|rgb| rgb <= 0xFFFFFF)));
        }
    }

    /// Failure: a page of thousands of annotations, highlights or quadrilaterals is read past the limits, or what's left out isn't said. Each limit
    /// is held at itself plus one, set small.
    #[test]
    fn every_limit_is_held_and_said() {
        let two_quads = "<< /Subtype /Highlight /QuadPoints [50 720 150 720 50 708 150 708 50 700 150 700 50 690 150 690] >>";
        let small = Limits {
            annots: 3,
            highlights: 2,
            quads: 1,
        };
        let at = highlights_of(&page_with(&[ONE, ONE], ""), 0, small);
        assert_eq!((at.found.len(), at.truncated), (2, false));
        let past = highlights_of(&page_with(&[ONE, ONE, ONE], ""), 0, small);
        assert_eq!((past.found.len(), past.truncated), (2, true), "a third highlight");
        let annots = highlights_of(&page_with(&["<< >>", "<< >>", "<< >>", ONE], ""), 0, small);
        assert_eq!((annots.found.len(), annots.truncated), (0, true), "a fourth annotation isn't looked at");
        let quads = highlights_of(&page_with(&[two_quads], ""), 0, small);
        assert_eq!((quads.found[0].rects.len(), quads.truncated), (1, true), "a second quadrilateral");
    }
}
