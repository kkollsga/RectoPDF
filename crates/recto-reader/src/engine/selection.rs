//! **The selection, from the engine's callers:** what the mouse does to it on a page, what the Services menu, Copy and the state record ask of it, and Find's hit selected and brought into view. The bridge (the page view's mouse and Services) and the app (Copy, Select All, the state record, Find) take it alone.
//!
//! A borrow of the [`Engine`], built inline by [`Engine::selection`]: each method takes the page view's lock exactly as the `Engine` method it was until R3.6
//! did, in the same order with the same locks after it, so the engine's lock map holds as it was.

use super::{Engine, lock};
use crate::selection::Notice;
use crate::tiles::Op;

/// What the selection is and what's done to it, on this page view.
#[derive(Clone, Copy)]
pub struct Selection<'a> {
    engine: &'a Engine,
}

// size: one reference, so taking it costs what calling a method of `Engine` did.
const _: () = assert!(size_of::<Selection<'static>>() <= size_of::<usize>(), "a facade of the engine grew");

impl Engine {
    /// What the selection is and what's done to it, on this page view.
    #[inline]
    pub fn selection(&self) -> Selection<'_> {
        Selection { engine: self }
    }
}

impl Selection<'_> {
    pub fn press(&self, x: f64, y: f64, clicks: u32, shift: bool) -> Vec<Op> {
        self.engine.pointer_did(|reader| reader.selection_press(x, y, clicks, shift))
    }

    pub fn drag(&self, x: f64, y: f64) -> Vec<Op> {
        self.engine.pointer_did(|reader| reader.selection_drag(x, y))
    }

    pub fn release(&self) -> Vec<Op> {
        self.engine.pointer_did(|reader| reader.pointer.selection_release())
    }

    /// Whether the selection can be sent to the Services menu as it stands, which it's asked on the UI thread. Nothing's built.
    pub fn can_be_sent(&self) -> bool {
        let reader = lock(&self.engine.shared.reader);
        reader.docs.shown().is_some_and(|doc| reader.pointer.selection_can_be_sent(doc.page_count()))
    }

    /// Whether the pointer's on text. If its page hasn't been read, it's wanted, and this says no for now.
    pub fn is_over_text(&self, x: f64, y: f64) -> bool {
        let mut reader = lock(&self.engine.shared.reader);
        let over = reader.pointer_is_over_text(x, y);
        drop(reader);
        self.engine.wake();
        over
    }

    pub fn select_all(&self) {
        let mut reader = lock(&self.engine.shared.reader);
        reader.select_all();
        self.engine.after(reader);
    }

    /// Another pane took the selection, from the UI thread, the same way.
    pub fn clear_soon(&self) -> bool {
        self.engine.now_or_soon(
            |pending| pending.clear_selection = true,
            |reader, pending| {
                pending.clear_selection = false;
                reader.clear_selection();
            },
        )
    }

    /// Which tab is showing, and what's selected on it. A short lock, for the state record.
    pub fn hot(&self) -> Option<(u32, Option<recto_index::select::Range>)> {
        let reader = lock(&self.engine.shared.reader);
        reader.docs.hot_tab().map(|tab| (tab, reader.pointer.selection_range()))
    }

    /// Puts back what was selected, from the state record.
    pub fn restore(&self, from: (u32, u32), to: (u32, u32)) {
        let mut reader = lock(&self.engine.shared.reader);
        reader.restore_selection(recto_index::select::Range {
            start: recto_index::select::Place { page: from.0, at: from.1 },
            end: recto_index::select::Place { page: to.0, at: to.1 },
        });
        self.engine.after(reader);
    }

    pub fn has_any(&self) -> bool {
        lock(&self.engine.shared.reader).pointer.has_selection()
    }

    /// Selects Find's hit and brings it into view: characters `start..end` of a page, from 0. Off the UI thread.
    pub fn reveal(&self, page: u32, start: u32, end: u32) {
        let mut reader = lock(&self.engine.shared.reader);
        reader.reveal_range(recto_index::select::Range {
            start: recto_index::select::Place { page, at: start },
            end: recto_index::select::Place { page, at: end },
        });
        self.engine.after(reader);
    }

    /// Starts the selection on its way to the clipboard. It goes page by page on the engine's thread, and `EngineEvents::copy_ready` says when.
    pub fn copy(&self, anyway: bool) {
        let mut reader = lock(&self.engine.shared.reader);
        reader.start_copy(anyway);
        self.engine.after(reader);
    }

    pub fn take_copied(&self) -> Option<recto_index::select::CopiedText> {
        let mut reader = lock(&self.engine.shared.reader);
        let copied = reader.pointer.take_copied();
        drop(reader);
        self.engine.shared.settled.notify_all();
        copied
    }

    pub fn text_now(&self) -> Option<String> {
        let reader = lock(&self.engine.shared.reader);
        reader.pointer.selection_text_now(reader.docs.shown()?.page_count())
    }

    pub fn notice(&self) -> Option<Notice> {
        lock(&self.engine.shared.reader).pointer.selection_notice()
    }

    pub fn dismiss_notice(&self) {
        let mut reader = lock(&self.engine.shared.reader);
        reader.pointer.dismiss_selection_notice();
        self.engine.after(reader);
    }
}
