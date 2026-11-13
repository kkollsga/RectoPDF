//! What passes between the session and the page view outside a call: the pages lately drawn, the jumps it hasn't taken yet, and that an edit took away the page being read.

use crate::trail::Stop;
use crate::{DrawNote, MAX_DRAWS_KEPT, TabId};
use std::collections::VecDeque;

/// Notes to and from the page view, each kept until it's taken or capped.
#[derive(Debug, Default)]
pub(crate) struct PageViewMail {
    /// The latest pages drawn or tried, newest last, at most [`MAX_DRAWS_KEPT`]. Lets a waiter find out about a page finished a moment before it asked.
    draws: VecDeque<DrawNote>,
    /// Jumps the page view hasn't taken yet, with how far down the page to land: one per tab at most, the latest.
    jumps: Vec<(TabId, Stop)>,
    /// The last edit removed the page a tab was on, so its place moved to the nearest one. Read once with [`crate::Session::take_place_lost`].
    place_lost: bool,
}

impl PageViewMail {
    /// Notes that a page finished drawing, or couldn't be.
    pub(crate) fn note_drawn(&mut self, note: DrawNote) {
        self.draws.retain(|d| (d.tab, d.page) != (note.tab, note.page));
        self.draws.push_back(note);
        while self.draws.len() > MAX_DRAWS_KEPT {
            self.draws.pop_front();
        }
    }

    /// What's known about a page's latest draw, if it's still remembered.
    pub(crate) fn draw_note(&self, tab: TabId, page: u32) -> Option<DrawNote> {
        self.draws.iter().rev().find(|d| (d.tab, d.page) == (tab, page)).copied()
    }

    /// A tab's pages are no longer what was drawn of them.
    pub(crate) fn forget_draws(&mut self, tab: TabId) {
        self.draws.retain(|d| d.tab != tab);
    }

    /// A jump in a tab for the page view to take, in place of any it hasn't.
    pub(crate) fn jump(&mut self, id: TabId, to: Stop) {
        self.jumps.retain(|(tab, _)| *tab != id);
        self.jumps.push((id, to));
    }

    /// The jump waiting for a tab, if the page view hasn't taken it: where the session has sent the reader.
    pub(crate) fn waiting_jump(&self, tab: TabId) -> Option<Stop> {
        self.jumps.iter().find(|(id, _)| *id == tab).map(|(_, stop)| *stop)
    }

    pub(crate) fn take_jump(&mut self, id: TabId) -> Option<Stop> {
        let at = self.jumps.iter().position(|(tab, _)| *tab == id)?;
        Some(self.jumps.swap_remove(at).1)
    }

    pub(crate) fn set_place_lost(&mut self, lost: bool) {
        self.place_lost = lost;
    }

    pub(crate) fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.place_lost)
    }

    /// A tab's gone: nothing of it is for the page view.
    pub(crate) fn forget(&mut self, tab: TabId) {
        self.forget_draws(tab);
        self.jumps.retain(|(id, _)| *id != tab);
    }

    #[cfg(test)]
    pub(crate) fn holds_draw_of(&self, tab: TabId) -> bool {
        self.draws.iter().any(|draw| draw.tab == tab)
    }

    #[cfg(test)]
    pub(crate) fn draws_kept(&self) -> usize {
        self.draws.len()
    }

    #[cfg(test)]
    pub(crate) fn no_jumps(&self) -> bool {
        self.jumps.is_empty()
    }
}
