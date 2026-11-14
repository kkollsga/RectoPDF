//! **Highlights on the page view** (R4.3): the tab's highlights made as edits, drawn by the host as a layer of each page's, as the selection is, so they
//! scroll and zoom with it and no tile knows. Rust decides the rectangles and the colour; the host keeps the set it was last given and draws it on the
//! pages it has ([`Op::Marks`]).
//!
//! **What it costs.** Nothing is held here: the page view has no state for highlights, so a tab without any costs nothing and no op goes for it. The
//! app gives the whole set when it changes, when the tab is shown anew and when the paper changes (`recto-app`'s `core/highlight.rs`), one op each time.
//!
//! Highlights already in the file are part of the page as PDFium draws it, and aren't here.

use super::doc::Docs;
use super::{HostQueue, Op};
use crate::paper::PaperKind;
use recto_index::select::Rect;

/// A tab's highlights by page position: each page's lines in its own frame (points from its top left, before any turn the reader adds), as the
/// selection's are. Made by the app from the session's edits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabMarks {
    pub tab: u32,
    pub pages: Vec<(u32, Vec<Rect>)>,
}

/// The colour a highlight is drawn in over this paper, `0xRRGGBB`, and whether it lightens rather than multiplies. On dark paper a page is turned over,
/// so the yellow is dimmed and lightens what's under it, as the selection does there.
pub fn look(paper: PaperKind) -> (u32, bool) {
    let rgb = recto_core::save::HIGHLIGHT_RGB;
    if paper.highlight().lighten {
        let dim = |shift: u32| (((rgb >> shift) & 0xFF) * 110 / 255) << shift;
        (dim(16) | dim(8) | dim(0), true)
    } else {
        (rgb, false)
    }
}

/// Most pages one set gives the host, and rectangles in all: past what an edited list can hold (10,000 highlights), so only a set made wrongly is cut.
pub const MAX_PAGES_TOLD: usize = 10_000;
pub const MAX_RECTS_TOLD: usize = 65_536;

/// **The host is given the tab's whole set,** replacing what it had: none clears every page's. `TabMarks` can be made by any code, so what's given is
/// held here to what can be drawn (coordinator, 2026-11-14): pages the tab showing has, rectangles finite, of some area and inside their page, and at
/// most [`MAX_PAGES_TOLD`] pages and [`MAX_RECTS_TOLD`] rectangles. A set for another tab than the one showing gives nothing.
pub(crate) fn tell(host: &mut HostQueue, docs: &Docs, marks: &TabMarks, paper: PaperKind) {
    tell_within(host, docs, marks, paper, (MAX_PAGES_TOLD, MAX_RECTS_TOLD));
}

pub(super) fn tell_within(host: &mut HostQueue, docs: &Docs, marks: &TabMarks, paper: PaperKind, (most_pages, most_rects): (usize, usize)) {
    let Some(doc) = docs.shown().filter(|doc| doc.tab == marks.tab) else { return };
    let mut rects_left = most_rects;
    let mut pages = Vec::new();
    for (page, rects) in marks.pages.iter().take(most_pages) {
        if *page >= doc.layout.count() || rects_left == 0 {
            continue;
        }
        let size = doc.layout.raw_size(*page);
        let (width, height) = (size.width + 0.5, size.height + 0.5);
        let kept: Vec<Rect> = rects
            .iter()
            .filter(|rect| {
                [rect.x, rect.y, rect.width, rect.height].iter().all(|edge| edge.is_finite())
                    && rect.width > 0.0
                    && rect.height > 0.0
                    && rect.x >= 0.0
                    && rect.y >= 0.0
                    && rect.x + rect.width <= width
                    && rect.y + rect.height <= height
            })
            .take(rects_left)
            .copied()
            .collect();
        rects_left -= kept.len();
        if !kept.is_empty() {
            pages.push((*page, kept));
        }
    }
    let (rgb, lighten) = look(paper);
    host.ops.push_back(Op::Marks { pages, rgb, lighten });
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Failure: a highlight is drawn in another colour than the file gets, on white or warm paper; or on dark paper it multiplies, which leaves it
    /// invisible on a page turned over, or isn't dimmed and glares.
    #[test]
    fn a_highlight_is_the_files_colour_on_white_and_dimmed_to_lighten_on_dark() {
        assert_eq!(look(PaperKind::White), (recto_core::save::HIGHLIGHT_RGB, false));
        let (dark, lighten) = look(PaperKind::Dark);
        assert!(lighten);
        assert!(
            [16, 8, 0]
                .iter()
                .all(|shift| (dark >> shift) & 0xFF < (recto_core::save::HIGHLIGHT_RGB >> shift) & 0xFF || (dark >> shift) & 0xFF == 0)
        );
    }
}
