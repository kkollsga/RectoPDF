//! **The place:** where the reader is in the tab showing, the jumps sent to the host and whether it's arrived, what the reader's own scroll was, and the page the session's told of.

use super::{DocView, HOST_ROUNDING, MAX_ATTACH_PER_TURN, Op, Reader, SavedPlace, Viewport};
use crate::layout::{PageLayout, Spread};
use std::sync::atomic::{AtomicU64, Ordering};

/// **The place clock:** one count for the whole process, moved on, under the page view's lock, by everything that moves where a page view says a tab is:
/// a jump or a follow sent to the host, a change of layout that sends the place again, a zoom, and every viewport the host gives, the reader's own scroll
/// among them. So a read of a page view's places taken with its lock held ([`crate::Engine::places`]) can be told older or newer than another, of the same
/// view or of another one a tab has moved to, and than a jump sent: what the session needs so that the newer read wins (`recto_app`'s `core/places.rs`).
/// One atomic add on the scroll path: no lock, no allocation. It's process-wide, not a page view's, because a tab moves between page views (a comparison's
/// panes, which are made afresh) and a count of each view's own couldn't order one view's read against another's. **Only the order of two stamps taken in
/// one process means anything, never a stamp's value:** two headless apps in one test share the clock, so a test never asserts a stamp's number.
static PLACE_CLOCK: AtomicU64 = AtomicU64::new(0);

/// A band with no page: none is set. A band's `hi` is at most one past [`recto_core::TOP_SCALE`], so the top bit of the word it's kept in says whether the view has woken the core for it.
pub(super) const NONE_PAGE: u32 = u32::MAX;
pub(super) const BAND_WOKE: u16 = 0x8000;

/// **Sets the shown tab's contents band** (`Docs::set_band`) and, under the same lock, looks at once whether the top of the view is already outside it,
/// as a scroll step taken between the core's read of the places and this would have been: such a step was checked against the band before, with its
/// wake already spent, and the core would not hear of it until the next. Outside, it wakes the core once, as leaving it by a scroll does.
pub(crate) fn set_band(reader: &mut Reader, band: Option<(u32, u16, u16)>) {
    reader.docs.set_band(band);
    let viewport = reader.viewport;
    let place = &mut reader.place;
    // Not while a jump is on its way: where the view is isn't where it's going (as `Place::track` has it), and the core has the jump.
    if let (None, Some(page), Some(doc)) = (
        place.jump,
        place.last_view_page,
        reader.docs.hot.and_then(|tab| reader.docs.views.get_mut(&tab)),
    ) {
        place.leaves_the_band(doc, viewport, page);
    }
}

/// **The line the contents' current entry is found by,** for a view whose top is `y` and that is `height` high, with the reader on `page`: the page and how
/// far down it (0 to 1). The one rule for the page view's band (`Place::leaves_the_band`) and for the session, which has it from [`Reader::places`].
///
/// It is the top of the view, until the view is within one view-height of the end of its scroll (the document's foot less the view, and the room the
/// host leaves below the last row: [`PageLayout::end_room`]). Over that last view-height of scrolling it travels, evenly, from the top of the view to the
/// foot of the last page, which it reaches as the view reaches the end: `y + along × (foot − end)`, `along` going from 0 to 1. So it moves continuously
/// and never back as the view goes down, is never above the top of the view, and passes every place down to the last page's foot: every entry becomes
/// current by scrolling, in order, and the last one at the bottom. Without this, the top of the view can't come within a view-height of the end, and the
/// entries there could only be reached by a click. A document the view holds whole doesn't scroll, and its line is the top of the view.
pub(super) fn reading_line(layout: &PageLayout, y: f64, height: f64, page: u32) -> (u32, f64) {
    let count = layout.count();
    let document = layout.document_height();
    if count == 0 || !y.is_finite() || !height.is_finite() || height <= 0.0 || height > document {
        return layout.anchor_for(y, page);
    }
    let end = document + layout.end_room().1 - height;
    let start = (end - height).max(0.0);
    let along = if end > start { ((y - start) / (end - start)).clamp(0.0, 1.0) } else { 1.0 };
    if along >= 1.0 {
        return (count - 1, 1.0);
    }
    let last = layout.rect(count - 1);
    layout.anchor_for(y + along * (last.y + last.height - end).max(0.0), page)
}

/// Moves the place clock on, saying where it's come to.
pub(super) fn tick() -> u64 {
    PLACE_CLOCK.fetch_add(1, Ordering::SeqCst) + 1
}

/// Where the place clock is: a read of a page view's places taken with its lock held, after a send that came to `s`, carries a stamp at or past `s`, and
/// one taken before it a stamp under `s`.
#[must_use]
pub fn place_stamp() -> u64 {
    PLACE_CLOCK.load(Ordering::SeqCst)
}

/// Built only in the tiles module ([`Place::new`]), as each of [`Reader`]'s parts is (see there): the engine reaches it through `Reader`'s field for the three takes below.
#[derive(Debug)]
pub(crate) struct Place {
    pub(super) next_seq: u64,
    pub(super) jump: Option<u64>,
    pub(super) scrolled_to: u64,
    pub(super) last_view_page: Option<u32>,
    pub(super) moved_page: Option<u32>,
    /// The last viewport was the reader scrolling, and not the host arriving where it was sent, a zoom, a resize or a pinch: what synced scroll goes by.
    pub(super) user_scrolled: bool,
    /// The last viewport was the host arriving at a scroll it was sent, as a change of layout sends one to keep the place. Where it arrived is the place, or as near as the view reaches, so the place isn't taken again from it.
    pub(super) arrived: bool,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Place>() <= 56, "a page view's place grew");

impl Place {
    pub(super) fn new() -> Self {
        Self {
            next_seq: 1,
            jump: None,
            scrolled_to: 0,
            last_view_page: None,
            moved_page: None,
            user_scrolled: false,
            arrived: false,
        }
    }

    /// A scroll to send the host: its sequence number, which the host says back once it's arrived, with the place clock moved on.
    pub(super) fn send(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        tick();
        seq
    }

    /// What the new viewport says of the reader: whether it was their own scroll, and whether the host arrived where it was sent.
    pub(super) fn note_viewport(&mut self, was: Viewport, viewport: Viewport) {
        self.user_scrolled = !viewport.live
            && self.jump.is_none()
            && viewport.y != was.y
            && viewport.scrolled_to == was.scrolled_to
            && viewport.magnification == was.magnification
            && viewport.frame_width == was.frame_width
            && viewport.height == was.height;
        self.arrived = viewport.scrolled_to > was.scrolled_to && viewport.scrolled_to != u64::MAX;
    }

    /// Follows the reader's place. Only a change the reader made is the session's business: arriving where a jump sent them isn't.
    pub(super) fn track(&mut self, doc: &mut DocView, viewport: Viewport) {
        let bottom = doc.layout.document_height() - viewport.height;
        let page = if viewport.y >= bottom - 1.0 && viewport.y > 0.0 {
            doc.layout.count().saturating_sub(1)
        } else {
            doc.layout.page_at(viewport.y + viewport.height / 3.0)
        };
        // Side by side, the reader's page stays whichever of its row they were on: a jump to a right-hand page, or reading on in a row, isn't a move to the left one.
        let page = match self.last_view_page {
            Some(was) if doc.layout.same_row(was, page) => was,
            // The host arrived where a change of layout sent it, back to the place: the page the place was kept on, if it's in the row. A round trip through a view too small to reach the place, where the last page was taken for the reader's, otherwise lands on the row's left page (side-by-side review, finding 4).
            _ if self.arrived && doc.layout.same_row(doc.place.anchor.0, page) => doc.place.anchor.0,
            _ => page,
        };
        if let Some(jump) = self.jump {
            // Still on its way: where the view is now isn't where it's going, and taken for the place, it would be where it ends up on the next layout. On arrival the place is left as it was sent, to the pixel.
            if self.scrolled_to >= jump {
                self.jump = None;
                // Where it arrived can be another page than was asked, as when a pane's narrower than the view the place was taken in, or a place deep in a page puts the next one where the reader's eye is. The session's page goes with it, or the two disagree, and the place isn't brought back after a restart.
                if self.last_view_page != Some(page) {
                    self.moved_page = Some(page);
                }
                self.last_view_page = Some(page);
            }
            return;
        }
        // Arriving where a change of layout sent it, the place stays as it was kept, even where the view couldn't reach it, as at the foot of a document smaller than the view.
        let moved = !self.arrived && doc.place.exact_y.is_none_or(|(y, _)| y != viewport.y);
        if moved {
            // The host puts the view on its own pixels, so a view this near where the place is is on it, said the host's way. Taken again from the rounded height, the top of a page would be the foot of the one above, from the gap between them.
            if (doc.layout.anchor_y(doc.place.anchor) - viewport.y).abs() >= HOST_ROUNDING {
                doc.place.anchor = doc.layout.anchor_for(viewport.y, page);
            }
            doc.place.exact_y = None;
        }
        doc.place.scroll_x = viewport.x;
        if self.last_view_page != Some(page) {
            self.last_view_page = Some(page);
            self.moved_page = Some(page);
        }
        self.leaves_the_band(doc, viewport, page);
    }

    /// **Wakes the core when the top of the view leaves the stretch of a page in which the same contents entry is current** (`DocView::band`), by the wake a
    /// page change has, and only once until the core sends another. The line is [`reading_line`]'s, as the session has it: the view's top, and in the
    /// last view-height of the scroll on its way to the last page's foot; a few lookups of the layout and integer compares.
    fn leaves_the_band(&mut self, doc: &mut DocView, viewport: Viewport, page: u32) {
        let (band_page, lo, hi_woke) = doc.band;
        if band_page == NONE_PAGE || hi_woke & BAND_WOKE != 0 {
            return;
        }
        let (at_page, within) = reading_line(&doc.layout, viewport.y, viewport.height, page);
        let top = recto_core::thousandths(within);
        if at_page != band_page || top < lo || top >= hi_woke {
            doc.band.2 |= BAND_WOKE;
            self.moved_page = Some(page);
        }
    }

    /// Whether the last viewport was the reader scrolling, which another pane follows with synced scroll. Asking clears it.
    pub(crate) fn take_user_scrolled(&mut self) -> bool {
        std::mem::take(&mut self.user_scrolled)
    }

    pub(crate) fn has_moved_page(&self) -> bool {
        self.moved_page.is_some()
    }

    /// **Tells where the view is again,** as a scroll of the reader's own does: the page it's on as the page the reader moved to, with the place clock
    /// moved on. For a jump the session sent that a newer ask overtook before it got to the view ([`crate::Engine::jump_within`]): the session has the
    /// jump's place, and takes the view's from its next read. Says whether there's a page to tell.
    pub(crate) fn tell_again(&mut self) -> bool {
        let Some(page) = self.last_view_page else {
            return false;
        };
        self.moved_page = Some(page);
        tick();
        true
    }

    /// A page the reader scrolled to, for the session to take as its place. Asking clears it.
    pub(crate) fn take_moved_page(&mut self) -> Option<u32> {
        self.moved_page.take()
    }
}

impl Reader {
    /// The least room either side of the pages, as a comparison's pane has it. The place is kept.
    pub fn set_margins(&mut self, left: f64, right: f64) {
        self.margins = (left, right);
        let Some(doc) = self.docs.hot.and_then(|tab| self.docs.views.get_mut(&tab)) else {
            return;
        };
        if doc.layout.set_margins(left, right) {
            doc.place.exact_y = None;
            self.lay_out();
            self.refresh(MAX_ATTACH_PER_TURN);
        }
    }

    /// Sends the view to a place in a page, as another pane's reader is in theirs, for synced scroll. A jump, so arriving there isn't the reader moving, and neither is it this pane leading the others. Held to the document. Says which page it went to.
    pub fn follow(&mut self, page: u32, within: f64) -> Option<u32> {
        self.wake.finish_wake();
        let doc = self.docs.hot.and_then(|tab| self.docs.views.get_mut(&tab))?;
        let page = page.min(doc.layout.count().saturating_sub(1));
        let within = if within.is_finite() { within.clamp(0.0, 1.0) } else { 0.0 };
        doc.place.anchor = (page, within);
        let y = doc.layout.anchor_y((page, within));
        doc.place.exact_y = Some((y, doc.layout.document_height()));
        let seq = self.place.send();
        self.place.jump = Some(seq);
        let moved = self.place.last_view_page != Some(page);
        self.place.last_view_page = Some(page);
        if moved {
            self.place.moved_page = Some(page);
        }
        self.host.ops.push_back(Op::ScrollTo { y, seq });
        Some(page)
    }

    /// Exactly where each tab it holds is, for the session to keep. Hot and warm tabs: a few at most, and a cold tab's is already kept.
    pub fn places(&self) -> Vec<(u32, SavedPlace)> {
        self.docs
            .views
            .values()
            .filter_map(|doc| {
                let page_size = doc.layout.usual_size();
                if self.docs.hot == Some(doc.tab) {
                    let page = self.place.last_view_page?;
                    // Not yet arrived where it's been sent: what the view says is about somewhere else, and the line is where it's going.
                    let (x, y, line) = if self.place.jump.is_some() {
                        (
                            doc.place.scroll_x,
                            doc.place.exact_y.map_or_else(|| doc.layout.anchor_y(doc.place.anchor), |(y, _)| y),
                            doc.place.anchor,
                        )
                    } else {
                        (
                            self.viewport.x,
                            self.viewport.y,
                            reading_line(&doc.layout, self.viewport.y, self.viewport.height, page),
                        )
                    };
                    Some((
                        doc.tab,
                        SavedPlace {
                            page,
                            anchor: doc.place.anchor,
                            line,
                            x,
                            y,
                            document_height: doc.layout.document_height(),
                            magnification: doc.place.magnification,
                            page_size,
                        },
                    ))
                } else {
                    let (page, anchor) = doc.place.left_at?;
                    let y = doc.place.exact_y.map_or_else(|| doc.layout.anchor_y(anchor), |(y, _)| y);
                    Some((
                        doc.tab,
                        SavedPlace {
                            page,
                            anchor,
                            line: anchor,
                            x: doc.place.scroll_x,
                            y,
                            document_height: doc.layout.document_height(),
                            magnification: doc.place.magnification,
                            page_size,
                        },
                    ))
                }
            })
            .collect()
    }

    /// Whether what's on the layers is the place on the hot tab, with nothing still on its way there: what makes it right to show a window.
    pub fn is_on_place(&self) -> bool {
        self.docs.hot.is_some() && self.place.jump.is_none() && self.is_settled()
    }

    /// Lays the tab showing out another way: a page to a row, or two side by side. The reader stays on the page they were on, as far down it, and the zoom stays.
    pub fn set_spread(&mut self, spread: Spread) {
        let Some(doc) = self.docs.hot.and_then(|tab| self.docs.views.get_mut(&tab)) else {
            return;
        };
        if !doc.layout.set_spread(spread) {
            return;
        }
        // The height the view was at is about the old layout. Where it is in its page is what holds.
        doc.place.exact_y = None;
        self.lay_out();
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// **A jump the session sends,** as an agent's `goto`, a contents row, a link or Back does: to the page's top, the margin above the first page
    /// included, or `within` the way down it as [`Self::follow`] goes. Says whether it went to the host: not with no tab showing.
    pub fn jump_to(&mut self, page: u32, within: f64) -> bool {
        self.pointer.forget_reveal_unless(page);
        if within.is_finite() && within > 0.0 {
            return self.follow(page, within).is_some();
        }
        // A wake's over once someone's gone somewhere else: what's bare from here is pages being reached, not the wake.
        self.wake.finish_wake();
        let Some(doc) = self.docs.hot.and_then(|tab| self.docs.views.get_mut(&tab)) else {
            return false;
        };
        let page = page.min(doc.layout.count().saturating_sub(1));
        doc.place.anchor = (page, 0.0);
        let y = doc.layout.jump_y(page);
        // To the pixel while the layout holds: the first page's jump goes above it, to the margin, which its anchor can't say.
        doc.place.exact_y = Some((y, doc.layout.document_height()));
        let seq = self.place.send();
        self.place.jump = Some(seq);
        self.place.last_view_page = Some(page);
        // The session sent this, so it has this page: a page a move before it went to (a follow, say) is about a place it's left, and taken from the view
        // after, it would send the session where the view isn't.
        self.place.moved_page = None;
        self.host.ops.push_back(Op::ScrollTo { y, seq });
        true
    }

    /// Goes where the tab's anchor says, which is on this page, as a jump.
    pub(super) fn jump_to_anchor(&mut self, page: u32) {
        self.pointer.forget_reveal_unless(page);
        let Some(doc) = self.docs.hot.and_then(|tab| self.docs.views.get(&tab)) else {
            return;
        };
        let y = doc.place_y();
        let seq = self.place.send();
        self.place.jump = Some(seq);
        self.place.last_view_page = Some(page.min(doc.layout.count().saturating_sub(1)));
        self.host.ops.push_back(Op::ScrollTo { y, seq });
    }

    /// The document's size, then the place in it. Called when the fit or a page's size has changed.
    pub(super) fn lay_out(&mut self) {
        let Some(doc) = self.docs.hot.and_then(|tab| self.docs.views.get_mut(&tab)) else {
            return;
        };
        doc.layout.set_margins(self.margins.0, self.margins.1);
        doc.layout.set_frame(self.viewport.frame_width);
        let (top_room, bottom_room) = doc.layout.end_room();
        let y = doc.place_y();
        doc.place.width_shown = doc.layout.document_width_for(y, y + self.viewport.height);
        self.host.ops.push_back(Op::Document {
            width: doc.place.width_shown,
            height: doc.layout.document_height(),
            top_room,
            bottom_room,
        });
        let seq = self.place.send();
        if self.place.jump.is_some() {
            self.place.jump = Some(seq);
        }
        self.host.ops.push_back(Op::ScrollTo { y, seq });
    }

    /// The document as wide as the rows in view need, which is the window's unless a fold-out's row is in view: told to the host when that changes, as the view reaches a fold-out or leaves it.
    pub(super) fn widen_for_view(&mut self) {
        let viewport = self.viewport;
        let Some(doc) = self.docs.hot.and_then(|tab| self.docs.views.get_mut(&tab)) else {
            return;
        };
        let width = doc.layout.document_width_for(viewport.y, viewport.y + viewport.height);
        if (width - doc.place.width_shown).abs() < 0.5 {
            return;
        }
        doc.place.width_shown = width;
        let (top_room, bottom_room) = doc.layout.end_room();
        self.host.ops.push_back(Op::Document {
            width,
            height: doc.layout.document_height(),
            top_room,
            bottom_room,
        });
    }
}
