//! **What the pointer does on a page:** the selection and its copy, Find's hit on its way into view, and the links under the pointer, each read from the page's text or its file as it's wanted.

use super::{Op, Reader};
use crate::selection::{Notice, Selecting};
use recto_index::select::Range;
use std::collections::HashMap;
use std::sync::Arc;

/// Built only in the tiles module ([`Pointer::new`]), with no `Default`: the engine reaches it through `Reader`'s field to call the methods below, and can neither build one nor put another in its place.
#[derive(Debug)]
pub(crate) struct Pointer {
    pub(super) selecting: Selecting,
    /// Links on the tab showing, read as they're pointed at.
    pub(super) linking: crate::links::Linking,
    /// What was selected on the tab showing when a renderer saw its file replaced, until the next activation says whether that was a save of what was shown.
    pub(super) selection_before_change: Option<(u32, recto_index::select::Range)>,
    /// Find's hit, selected and on its way into view once its page's text says where on the page it is: the tab, the hit, and whether its page was jumped to for it, which then places it whatever it shows.
    pub(super) revealing: Option<(u32, recto_index::select::Range, bool)>,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Pointer>() <= 664, "what the pointer does grew");

impl Pointer {
    pub(super) fn new() -> Self {
        Self {
            selecting: Selecting::default(),
            linking: crate::links::Linking::default(),
            selection_before_change: None,
            revealing: None,
        }
    }

    /// The mouse went up: a drag that was selecting or pressing a link ends.
    pub(crate) fn selection_release(&mut self) {
        self.linking.release();
        self.selecting.release();
    }

    /// The link a click has just landed on, taken once.
    pub(crate) fn take_clicked_link(&mut self) -> Option<crate::links::LinkHit> {
        self.linking.take_clicked()
    }

    /// Pages whose links are held, for a test: nothing outside asks since R3.6 found `Engine::link_pages_held` called by nobody.
    #[cfg(test)]
    pub(crate) fn link_pages_held(&self) -> usize {
        self.linking.pages_held()
    }

    /// What's selected on the tab showing, for the state record.
    pub(crate) fn selection_range(&self) -> Option<recto_index::select::Range> {
        self.selecting.range()
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.selecting.has_selection()
    }

    pub(crate) fn take_copy_signal(&mut self) -> bool {
        self.selecting.take_copy_signal()
    }

    pub(crate) fn take_copied(&mut self) -> Option<recto_index::select::CopiedText> {
        self.selecting.take_copied()
    }

    /// The selection's text if it's all here, for what can't wait for it, in a tab of `pages` pages.
    pub(crate) fn selection_text_now(&self, pages: u32) -> Option<String> {
        self.selecting.text_now(pages)
    }

    /// Whether the selection can be offered to the Services menu as it stands, in a tab of `pages` pages.
    pub(crate) fn selection_can_be_sent(&self, pages: u32) -> bool {
        self.selecting.can_offer(pages)
    }

    pub(crate) fn selection_notice(&self) -> Option<Notice> {
        self.selecting.notice().cloned()
    }

    pub(crate) fn dismiss_selection_notice(&mut self) {
        self.selecting.dismiss_notice();
    }

    /// **The last thing asked for wins:** a jump the session sends to another page than Find's hit waiting for its page's text is the newer ask, so the
    /// hit isn't brought into view when the text comes, which would take the view back from where the session has it and be told of as the reader's own
    /// move. The session's own jump to the hit's page, which comes after the reveal it went with, leaves it. Every jump the session sends says this first:
    /// `Reader::jump_to` (with or without a place down the page) and `Reader::jump_to_anchor`.
    pub(super) fn forget_reveal_unless(&mut self, page: u32) {
        if self.revealing.is_some_and(|(_, range, _)| range.start.page != page) {
            self.revealing = None;
        }
    }
}

impl Reader {
    /// Tells the host what's changed about highlights. A short look at nothing, with nothing selected.
    pub(super) fn highlight_ops(&mut self) {
        if self.pointer.selecting.nothing_to_draw() {
            return;
        }
        let pages: Vec<u32> = self.host.shown.keys().copied().collect();
        for page in pages {
            if let Some(rects) = self.pointer.selecting.layer_update(page) {
                self.host.ops.push_back(Op::Highlight { page, rects });
            }
        }
    }

    /// The mouse went down at a point in the document. Cheap: at worst it looks through a page already here.
    pub fn selection_press(&mut self, x: f64, y: f64, clicks: u32, shift: bool) {
        let point = self
            .docs
            .shown()
            .and_then(|doc| doc.point_on_page(x, y).filter(|point| doc.is_on_its_page(*point)));
        self.pointer.linking.press(if shift { None } else { point }, clicks);
        match self.docs.shown().and_then(|doc| doc.point_on_page(x, y)) {
            Some(point) => self.pointer.selecting.press(point, clicks, shift),
            None => self.pointer.selecting.clear(),
        }
        self.highlight_ops();
        self.cancel_if_unwanted();
    }

    pub fn selection_drag(&mut self, x: f64, y: f64) {
        self.pointer.linking.drag(self.docs.shown().and_then(|doc| doc.point_on_page(x, y)));
        if let Some(point) = self.docs.shown().and_then(|doc| doc.point_on_page(x, y)) {
            self.pointer.selecting.drag(point);
            self.highlight_ops();
            self.cancel_if_unwanted();
        }
    }

    /// **The link under the pointer,** if its page's links are here. A page not read yet says nothing for now, and is read next.
    pub fn pointer_link(&mut self, x: f64, y: f64) -> Option<crate::links::LinkHit> {
        let point = self
            .docs
            .shown()
            .and_then(|doc| doc.point_on_page(x, y).filter(|point| doc.is_on_its_page(*point)))?;
        self.pointer.linking.at(point)
    }

    /// Whether the pointer's on text, and so an I-beam. A page not read yet is wanted, for when there's nothing else to do.
    pub fn pointer_is_over_text(&mut self, x: f64, y: f64) -> bool {
        // Only on a page: the margin and the gap between pages are neither.
        let Some(point) = self
            .docs
            .shown()
            .and_then(|doc| doc.point_on_page(x, y).filter(|point| doc.is_on_its_page(*point)))
        else {
            return false;
        };
        self.pointer.selecting.is_over_text(point)
    }

    /// The whole document, as a Mac PDF reader takes ⌘A. It reads nothing until something's showing.
    pub fn select_all(&mut self) {
        let Some(pages) = self.docs.shown().map(|doc| doc.layout.count()).filter(|pages| *pages > 0) else {
            return;
        };
        self.pointer.selecting.set_range(Range::all(pages));
        self.highlight_ops();
    }

    pub fn clear_selection(&mut self) {
        self.pointer.selecting.clear();
        self.highlight_ops();
    }

    /// Puts back what was selected, from the state record. It's held to the document's pages, as a record can say anything.
    pub fn restore_selection(&mut self, range: recto_index::select::Range) {
        let pages = self.docs.shown().map_or(0, |doc| doc.layout.count());
        if range.start.page < pages && range.end.page < pages && (range.start.page, range.start.at) <= (range.end.page, range.end.at) {
            self.pointer.selecting.set_range(range);
            self.highlight_ops();
        }
    }

    /// **Find's hit:** selects it, as Find on a Mac does, and brings it into view. Its page is gone to at once if none of it shows, and once the page's text is here, which a selection's end asks for first, the hit is put a third of the way down the view. A hit already showing, away from the view's edges, stays where it is, so stepping through hits on one screen doesn't move it. Held to the document's pages.
    pub fn reveal_range(&mut self, range: recto_index::select::Range) {
        let Some(tab) = self.docs.hot else { return };
        let pages = self.docs.shown().map_or(0, |doc| doc.layout.count());
        if range.start.page >= pages || range.end.page >= pages || (range.start.page, range.start.at) > (range.end.page, range.end.at) {
            return;
        }
        self.pointer.selecting.set_range(range);
        self.highlight_ops();
        let shows = self.docs.shown().is_some_and(|doc| self.visible_pages(doc).contains(&range.start.page));
        self.pointer.revealing = Some((tab, range, !shows));
        if !self.try_reveal() && !shows {
            self.follow(range.start.page, 0.0);
        }
    }

    /// Moves the view to Find's hit if its place on the page is known now. Says whether it's settled, gone there or not needed.
    pub(super) fn try_reveal(&mut self) -> bool {
        let Some((tab, range, jumped)) = self.pointer.revealing else { return true };
        if self.docs.hot != Some(tab) {
            self.pointer.revealing = None;
            return true;
        }
        let end = if range.end.page == range.start.page { range.end.at } else { u32::MAX };
        let Some(bounds) = self.pointer.selecting.bounds_of(range.start.page, range.start.at, end) else {
            return false;
        };
        self.pointer.revealing = None;
        let Some(bounds) = bounds else { return true };
        let Some(doc) = self.docs.shown() else { return true };
        let rect = doc.layout.rect(range.start.page);
        let fit = doc.layout.fit();
        let (top, bottom) = (rect.y + f64::from(bounds.y) * fit, rect.y + f64::from(bounds.y + bounds.height) * fit);
        let (view_top, view_height) = (self.viewport.y, self.viewport.height);
        // Away from the edges: a tenth of the view at the top, a fifth at the bottom, where the page number and a notice sit.
        if !jumped && view_height > 0.0 && top >= view_top + view_height * 0.1 && bottom <= view_top + view_height * 0.8 {
            return true;
        }
        let want_top = (top - view_height / 3.0).max(0.0);
        let within = ((want_top - rect.y) / rect.height.max(1.0)).clamp(0.0, 1.0);
        let page = if want_top < rect.y { doc.layout.page_at(want_top) } else { range.start.page };
        let within = if page == range.start.page { within } else { doc.layout.anchor_at(want_top).1 };
        self.follow(page, within);
        true
    }

    pub(super) fn heavy_pages(&self) -> HashMap<u32, bool> {
        self.docs.shown().map(|doc| doc.drawing.heavy.clone()).unwrap_or_default()
    }

    /// Starts the selection on its way to the clipboard, a page at a time, off this thread. `anyway` is the reader having said so.
    pub fn start_copy(&mut self, anyway: bool) {
        let Some(pages) = self.docs.shown().map(|doc| doc.layout.count()) else {
            return;
        };
        let heavy = self.heavy_pages();
        self.pointer
            .selecting
            .start_copy(anyway, pages, |page| heavy.get(&page).copied().unwrap_or(false));
    }

    /// Text arrived, or couldn't be read.
    pub(super) fn take_text(&mut self, tab: u32, position: u32, outcome: Option<(Arc<recto_index::select::PageText>, bool)>) {
        if self.docs.hot != Some(tab) || !self.window_open {
            return;
        }
        let visible: Vec<u32> = self.docs.shown().map(|doc| self.visible_pages(doc).collect()).unwrap_or_default();
        match outcome {
            Some((text, checked)) => self.pointer.selecting.page_read(position, text, checked, &visible),
            None => self.pointer.selecting.page_failed(position, &visible),
        }
        if self.pointer.revealing.is_some_and(|(_, range, _)| range.start.page == position) {
            self.try_reveal();
        }
        self.advance_copy();
    }

    pub(super) fn advance_copy(&mut self) {
        let pages = self.docs.shown().map_or(0, |doc| doc.layout.count());
        let heavy = self.heavy_pages();
        self.pointer.selecting.advance_copy(pages, |page| heavy.get(&page).copied().unwrap_or(false));
    }

    /// The file behind a tab has been seen to be another one, so places on its pages mean nothing now. It comes before the job's outcome, which may be about it.
    pub fn file_changed(&mut self, tab: u32) {
        if self.docs.hot == Some(tab) {
            self.pointer.linking.clear();
            // Kept aside until the session says what the new version is: a save of what was shown keeps it (see [`Self::activate`]), anything else lets it go.
            self.pointer.selection_before_change = self.pointer.selecting.range().map(|range| (tab, range));
            self.pointer.selecting.file_changed();
            self.highlight_ops();
        }
    }
}
