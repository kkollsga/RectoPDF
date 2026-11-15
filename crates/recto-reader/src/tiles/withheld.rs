//! **A tab's own pages withheld while its unsaved edits are held** (the coordinator's decision of 2026-11-15, from the QA pass before 0.1.1). A tab whose
//! edits don't fit the file on disk (it changed under them, or they name a version or a length it hasn't) keeps the edited list's count and place, and
//! nothing of its own file is drawn, sized, read for the selection or given links: not the earlier version, whose tiles go at once, and not the file
//! there now, whose pages the list's numbers aren't about. The session decides whether a tab's pages are withheld (`recto-session`'s
//! `EditsFit::shows_own_pages`); this holds it in the page view. Pages from another PDF added to the tab keep their own rule (a changed one isn't
//! drawn either, `engine/run.rs`).
//!
//! **One check, where every job starts:** the drawing thread refuses each job for a withheld tab's own file ([`Docs::withholds`]), a draw, a size, a
//! text read, a weight and the links alike, so no planner can reach around it. A refused page keeps a problem in these words, so it counts as settled
//! and not blank, and isn't asked for again until the hold ends. Nothing is held for it beyond one flag of the tab's view.

use super::doc::Docs;
use super::{MAX_ATTACH_PER_TURN, Reader};

/// What a page withheld says, as a page that can't be drawn says why. The tab's own notice says the rest (what happened, and Discard).
pub(crate) const WORDS: &str = "This page isn't shown while this tab's unsaved edits don't fit its file as it is on disk.";

/// **Withholds a tab's own pages, or shows them again** (`on` false), if its view is held here: every tile of it goes from the screen and the cache,
/// what a job under way for it finds is thrown away, the selection and links on its pages go, and its pages are asked for again under the new rule.
/// Its layout and place stay, so the earlier version put back comes back where the reader was. Says whether anything changed: a tab not held here,
/// or already so, changes nothing and costs one lookup.
pub(crate) fn set(reader: &mut Reader, tab: u32, on: bool) -> bool {
    let floor = reader.work.next_generation;
    let Some(doc) = reader.docs.views.get_mut(&tab) else { return false };
    if doc.drawing.withheld == on {
        return false;
    }
    doc.drawing.withheld = on;
    doc.drawing.floor = floor;
    doc.drawing.issues.clear();
    doc.drawing.heavy.clear();
    doc.drawing.seen.clear();
    if let Some(running) = &reader.work.running
        && running.tab == tab
        && let Some(cancel) = &running.cancel
    {
        cancel.cancel_before(running.generation + 1);
    }
    let parts = &mut *reader;
    parts.cache.drop_tab(tab, &mut parts.host);
    if reader.docs.hot == Some(tab) {
        reader.clear_screen();
        reader.pointer.selecting.reset();
        reader.pointer.linking.clear();
    }
    reader.refresh(MAX_ATTACH_PER_TURN);
    true
}

impl Docs {
    /// **Whether a job for this file of this tab is refused:** it's the tab's own file, and the tab's pages are withheld. Asked by the drawing thread
    /// for every job, under the page view's lock it takes for the job anyway.
    pub(crate) fn withholds(&self, tab: u32, file: &std::path::Path) -> bool {
        self.views
            .get(&tab)
            .is_some_and(|doc| doc.drawing.withheld && doc.map.files.first().is_some_and(|own| own == file))
    }
}
