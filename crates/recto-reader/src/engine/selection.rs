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

// size: the engine's shared state, one for each page view (a pane), with the hidden highlights' map (64 bytes, empty and unallocated for a view
// with none): 2760 bytes before it on 2026-11-14 and 2824 after, measured with the QA holds it has in tests.
#[cfg(all(target_os = "macos", any(test, feature = "qa")))]
const _: () = assert!(size_of::<super::Shared>() <= 2824, "the engine's shared state grew");

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
        self.engine.unpark_thread();
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

    /// **The tab's highlights made as edits,** the whole set, for the host to draw on the pages it shows, in the colour for `paper`. Rectangles the app
    /// made from the session's list, so nothing of a file's is here. Nothing is kept: the app gives the set again when it changes, the tab is shown anew
    /// or the paper changes. Called on the session thread by the app's pass, outside the session's lock; it takes the page view's lock once, as this
    /// facade's other methods do.
    pub fn show_marks(&self, marks: &crate::tiles::TabMarks, paper: crate::paper::PaperKind) {
        let mut reader = lock(&self.engine.shared.reader);
        if reader.docs.hot_tab() != Some(marks.tab) {
            return;
        }
        let parts = &mut *reader;
        crate::tiles::tell_marks(&mut parts.host, &parts.docs, marks, paper);
        self.engine.after(reader);
    }

    /// **The file's highlights a tab has taken out and not saved** (review of R4.3, S2), the whole set for its own `file`, or `None` for none: the
    /// jobs that draw the tab leave them out (`recto_render_service::DocRenderer::hidden`), and its pages that either set hides anything on are
    /// drawn again. The set's lock is taken alone and let go before the page view's is taken, and never with the session's.
    pub fn show_hidden(&self, tab: u32, file: &std::path::Path, set: Option<std::sync::Arc<recto_proto::hidden::HiddenAnnots>>) {
        let pages = crate::tiles::put_hidden(&mut lock(&self.engine.shared.hidden), tab, file.to_path_buf(), set);
        if pages.is_empty() {
            return;
        }
        let mut reader = lock(&self.engine.shared.reader);
        crate::tiles::redraw_own_pages(&mut reader, tab, &pages);
        self.engine.after(reader);
    }

    /// How many tabs this page view holds a set of hidden highlights for, for tests and QA.
    pub fn hidden_sets(&self) -> usize {
        lock(&self.engine.shared.hidden).len()
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

/// The set of the file's highlights `tab` has taken out, for a job that draws it from `file`: one short lock of the set's own, taken alone, and no
/// copy of the path while no tab here has one.
pub(super) fn hidden_for(shared: &super::Shared, tab: u32, file: &std::path::Path) -> Option<std::sync::Arc<recto_proto::hidden::HiddenAnnots>> {
    let sets = lock(&shared.hidden);
    if sets.is_empty() {
        return None;
    }
    sets.get(&(tab, file.to_path_buf())).cloned()
}

#[cfg(test)]
mod tests {
    /// Failure (review of R4.3, S2): a job is drawn by a renderer found some other way than `renderer_for`, which hands it the tab's set of hidden
    /// file highlights, so that job draws a highlight the person has taken out. A source scan: jobs are done in one place, with what
    /// `renderer_for` gave, and it gives the job's tab's set.
    #[test]
    fn every_job_reaches_its_renderer_through_renderer_for() {
        let run = include_str!("run.rs");
        let body = |name: &str| {
            let start = run.find(&format!("fn {name}")).expect("the function");
            &run[start..run[start..].find("\n}\n").map_or(run.len(), |end| start + end)]
        };
        assert_eq!(run.matches("do_job(").count(), 2, "do_job is defined once and called once");
        let job_loop = body("run(shared");
        let (found, done) = (
            job_loop.find("renderer_for(").expect("run finds a renderer"),
            job_loop.find("do_job(").expect("and does the job"),
        );
        assert!(found < done, "with what renderer_for found");
        assert!(body("renderer_for<").contains("hidden.draw_for("), "and renderer_for gives the job's tab's set");
    }
}
