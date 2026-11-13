//! What passes between the session and the page view outside a call: the pages lately drawn, the jumps it hasn't taken yet, how new the view's word
//! taken in last on each tab is and the jump it was sent that no word since has told of, and that an edit took away the page being read.

use crate::trail::Stop;
use crate::{DrawNote, MAX_DRAWS_KEPT, TabId};
use std::collections::VecDeque;

/// **How new what the session has of a page view's word on a tab is, by the place clock** (`recto_reader::tiles::place_stamp`): the stamp of the read
/// it took in last, and the jump it sent the view that no read taken in since has told of. A read is stamped under the page view's lock, so a read's
/// stamp at or past a jump's says the read came after the send and is about where the jump took the reader or later, and a read stamped under the last
/// one taken in is older than what the session has.
#[derive(Debug, Clone, Copy)]
struct ViewWord {
    tab: TabId,
    /// The stamp of the last read of the view's places taken in for this tab: an older one is skipped.
    last_read: u64,
    /// The jump sent the view and the place clock the send came to, while no read at or past it has been taken in: where the session has sent the reader.
    jump_on_its_way: Option<(Stop, u64)>,
}

/// What passes between the session and the page view outside a call. What's public here notes a draw, or takes or says what's waiting for the view;
/// what makes a jump or forgets a tab's stays the crate's, as it goes with the tab's trail and place.
#[derive(Debug)]
pub struct PageViewMail {
    /// The latest pages drawn or tried, newest last, at most [`MAX_DRAWS_KEPT`]. Lets a waiter find out about a page finished a moment before it asked.
    draws: VecDeque<DrawNote>,
    /// Jumps the page view hasn't taken yet, with how far down the page to land: one per tab at most, the latest.
    jumps: Vec<(TabId, Stop)>,
    /// **What the session knows of each page view's word on a tab,** one per tab a view has told of or been sent a jump on, gone when the tab goes dormant
    /// or closes: see [`ViewWord`].
    words: Vec<ViewWord>,
    /// The last edit removed the page a tab was on, so its place moved to the nearest one. Read once with [`crate::Session::take_place_lost`].
    place_lost: bool,
}

impl PageViewMail {
    /// Nothing drawn, nothing waiting. The crate's: see [`crate::parts::PartMut`].
    pub(crate) fn new() -> Self {
        Self {
            draws: VecDeque::new(),
            jumps: Vec::new(),
            words: Vec::new(),
            place_lost: false,
        }
    }

    /// What's known about a page's latest draw, if it's still remembered.
    pub fn draw_note(&self, tab: TabId, page: u32) -> Option<DrawNote> {
        self.draws.iter().rev().find(|d| (d.tab, d.page) == (tab, page)).copied()
    }

    /// A tab's pages are no longer what was drawn of them, and a jump it was sent is about the pages as they were.
    pub(crate) fn forget_draws(&mut self, tab: TabId) {
        self.draws.retain(|d| d.tab != tab);
        self.forget_jump_on_its_way(tab);
    }

    /// A jump in a tab for the page view to take, in place of any it hasn't.
    pub(crate) fn jump(&mut self, id: TabId, to: Stop) {
        self.jumps.retain(|(tab, _)| *tab != id);
        self.jumps.push((id, to));
    }

    /// The jump waiting for a tab, if the page view hasn't taken it: where the session has sent the reader.
    pub fn waiting_jump(&self, tab: TabId) -> Option<Stop> {
        self.jumps.iter().find(|(id, _)| *id == tab).map(|(_, stop)| *stop)
    }

    fn word(&mut self, tab: TabId) -> &mut ViewWord {
        let at = match self.words.iter().position(|word| word.tab == tab) {
            Some(at) => at,
            None => {
                self.words.push(ViewWord {
                    tab,
                    last_read: 0,
                    jump_on_its_way: None,
                });
                self.words.len() - 1
            }
        };
        &mut self.words[at]
    }

    /// The jump a tab's page view was sent that no read taken in since has told of.
    pub(crate) fn jump_on_its_way(&self, tab: TabId) -> Option<Stop> {
        self.words
            .iter()
            .find(|word| word.tab == tab)
            .and_then(|word| word.jump_on_its_way)
            .map(|(to, _)| to)
    }

    /// **A read of a page view's places, stamped `stamp`, is to be taken in for a tab:** whether it's as new as what the session has. If it is, it's what
    /// the session has from here, and a jump sent at or before it is told of.
    pub(crate) fn view_told_of(&mut self, tab: TabId, stamp: u64) -> bool {
        let word = self.word(tab);
        if stamp < word.last_read {
            return false;
        }
        word.last_read = stamp;
        if word.jump_on_its_way.is_some_and(|(_, sent)| sent <= stamp) {
            word.jump_on_its_way = None;
        }
        true
    }

    /// The tab's moved on, to another page or other pages: the jump it was sent is no longer where the reader is.
    pub(crate) fn forget_jump_on_its_way(&mut self, tab: TabId) {
        if let Some(word) = self.words.iter_mut().find(|word| word.tab == tab) {
            word.jump_on_its_way = None;
        }
    }

    /// The tab's gone dormant: no page view holds it, and it costs nothing here.
    pub(crate) fn forget_view_word(&mut self, tab: TabId) {
        self.words.retain(|word| word.tab != tab);
    }

    pub(crate) fn set_place_lost(&mut self, lost: bool) {
        self.place_lost = lost;
    }

    /// A tab's gone: nothing of it is for the page view.
    pub(crate) fn forget(&mut self, tab: TabId) {
        self.forget_draws(tab);
        self.forget_view_word(tab);
        self.jumps.retain(|(id, _)| *id != tab);
    }

    #[cfg(test)]
    pub(crate) fn holds_draw_of(&self, tab: TabId) -> bool {
        self.draws.iter().any(|draw| draw.tab == tab)
    }

    #[cfg(test)]
    pub(crate) fn holds_view_word_of(&self, tab: TabId) -> bool {
        self.words.iter().any(|word| word.tab == tab)
    }

    #[cfg(test)]
    pub(crate) fn draws_kept(&self) -> usize {
        self.draws.len()
    }

    #[cfg(test)]
    pub(crate) fn no_jumps(&self) -> bool {
        self.jumps.is_empty() && self.words.is_empty()
    }
}

/// What a caller outside the crate may change of the mail: that a page was drawn, and what it takes of what's waiting for the page view. Making a jump or
/// forgetting a tab's stays the session's, as it goes with the tab's trail and place.
impl crate::parts::PartMut<'_, PageViewMail> {
    /// Notes that a page finished drawing, or couldn't be.
    pub fn note_drawn(&mut self, note: DrawNote) {
        let draws = &mut self.0.draws;
        draws.retain(|d| (d.tab, d.page) != (note.tab, note.page));
        draws.push_back(note);
        while draws.len() > MAX_DRAWS_KEPT {
            draws.pop_front();
        }
    }

    /// The last jump in a tab the page view hasn't taken, with how far down to land. Taken once.
    pub fn take_jump(&mut self, id: TabId) -> Option<Stop> {
        let jumps = &mut self.0.jumps;
        let at = jumps.iter().position(|(tab, _)| *tab == id)?;
        Some(jumps.swap_remove(at).1)
    }

    /// **A jump has gone to the page view,** the send having come to `stamp` on the place clock: it's where the reader is until a read at or past `stamp`
    /// is taken in, which may have happened already, between the send and this. It's no longer waiting, unless a newer jump has taken its place there since,
    /// which stays waiting for the next look.
    pub fn jump_sent(&mut self, tab: TabId, to: Stop, stamp: u64) {
        if self.0.waiting_jump(tab) == Some(to) {
            self.take_jump(tab);
        }
        let word = self.0.word(tab);
        word.jump_on_its_way = (word.last_read < stamp).then_some((to, stamp));
    }

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.0.place_lost)
    }
}
