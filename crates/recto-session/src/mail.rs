//! What passes between the session and the page view outside a call: the pages lately drawn, the jumps it hasn't taken yet, how new the view's word
//! taken in last on each tab is and the jump it was sent that no word since has told of (and, for a jump to a contents entry, that the view is still
//! where it put the reader), and that an edit took away the page being read.

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
    /// The jump sent the view, while no read at or past its send has been taken in; and a jump to a contents entry after that, while the view stays where it
    /// put the reader: see [`Sent`].
    jump_on_its_way: Option<Sent>,
    /// The line the contents' current entry is found by, as the read taken in last said it with its place ([`crate::view::ViewSaid`]): the page and
    /// thousandths down it, or a page of `NONE` before any read.
    line: (u32, u16),
}

// size: one for each tab a page view has told of or been sent a jump on (hot and warm, a few), none for a cold tab. 40 bytes until 2026-11-14, 48 with the
// line; the entry a jump went to is in what was the padding of the jump's (`Sent`, 24 bytes as `(Stop, u64)` was). Held there, so a field added is a
// deliberate change.
const _: () = assert!(size_of::<ViewWord>() == 48, "a page view's word on a tab grew");

/// **A jump sent a page view:** where to, the contents entry it went to (`recto goto --entry`, a click on a contents row) or `NONE`, and the place clock
/// the send came to. Once a read at or past the send is taken in, a jump with no entry is done with; one with an entry is **kept**, its send [`KEPT`],
/// and that entry stays the current one while each place taken in is still the jump's to the letter: the page view keeps its anchor as the jump set it
/// until the person scrolls, so the first place with another anchor ends it, and so does any other jump, an edit, or the tab going dormant. Not written:
/// it doesn't survive sleep or a restart, where the place does.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sent {
    to: Stop,
    entry: u32,
    sent: u64,
}

/// A [`Sent`] whose send a read has come after: a kept jump to an entry. A send's stamp is always past the read taken in before it, so never 0.
const KEPT: u64 = 0;

impl Sent {
    /// Whether a place's anchor, as a read of the page view says it, is where this jump put the reader, to the letter.
    fn holds(&self, (page, within): (u32, f64)) -> bool {
        page == self.to.page && within == f64::from(self.to.within)
    }
}

/// What passes between the session and the page view outside a call. What's public here notes a draw, or takes or says what's waiting for the view;
/// what makes a jump or forgets a tab's stays the crate's, as it goes with the tab's trail and place.
#[derive(Debug)]
pub struct PageViewMail {
    /// The latest pages drawn or tried, newest last, at most [`MAX_DRAWS_KEPT`]. Lets a waiter find out about a page finished a moment before it asked.
    draws: VecDeque<DrawNote>,
    /// Jumps the page view hasn't taken yet, with how far down the page to land and the contents entry each goes to or `NONE`: one per tab at most, the
    /// latest.
    jumps: Vec<(TabId, Stop, u32)>,
    /// **What the session knows of each page view's word on a tab,** one per tab a view has told of or been sent a jump on, gone when the tab goes dormant
    /// or closes: see [`ViewWord`].
    words: Vec<ViewWord>,
    /// The last edit removed the page a tab was on, so its place moved to the nearest one. Read once with [`PartMut::take_place_lost`](crate::PartMut::take_place_lost).
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

    /// A tab's pages are no longer what was drawn of them, and a jump it was sent, kept or not, is about the pages as they were.
    pub(crate) fn forget_draws(&mut self, tab: TabId) {
        self.draws.retain(|d| d.tab != tab);
        if let Some(word) = self.words.iter_mut().find(|word| word.tab == tab) {
            word.jump_on_its_way = None;
        }
    }

    /// A jump in a tab for the page view to take, in place of any it hasn't. It ends a kept jump to an entry: another jump has been made.
    pub(crate) fn jump(&mut self, id: TabId, to: Stop) {
        self.jumps.retain(|(tab, ..)| *tab != id);
        self.jumps.push((id, to, crate::NONE));
        if let Some(word) = self.words.iter_mut().find(|word| word.tab == id)
            && word.jump_on_its_way.is_some_and(|sent| sent.sent == KEPT)
        {
            word.jump_on_its_way = None;
        }
    }

    /// The jump waiting for a tab, if the page view hasn't taken it: where the session has sent the reader.
    pub fn waiting_jump(&self, tab: TabId) -> Option<Stop> {
        self.jumps.iter().find(|(id, ..)| *id == tab).map(|(_, stop, _)| *stop)
    }

    /// **The contents entry a jump took the reader to, while it's the current one** ([`Sent`]): the entry, and the page the jump went to, of the jump
    /// waiting for the tab, else of the one on its way, else of a kept one while `anchor`, the top of the place the tab has, is still where it put the
    /// reader.
    pub(crate) fn jumped_entry(&self, tab: TabId, anchor: Option<(u32, f64)>) -> Option<(u32, u32)> {
        if let Some((_, to, entry)) = self.jumps.iter().find(|(id, ..)| *id == tab) {
            return (*entry != crate::NONE).then_some((to.page, *entry));
        }
        let sent = self.words.iter().find(|word| word.tab == tab)?.jump_on_its_way?;
        let current = sent.sent != KEPT || anchor.is_some_and(|anchor| sent.holds(anchor));
        (sent.entry != crate::NONE && current).then_some((sent.to.page, sent.entry))
    }

    fn word(&mut self, tab: TabId) -> &mut ViewWord {
        let at = match self.words.iter().position(|word| word.tab == tab) {
            Some(at) => at,
            None => {
                self.words.push(ViewWord {
                    tab,
                    last_read: 0,
                    jump_on_its_way: None,
                    line: (crate::NONE, 0),
                });
                self.words.len() - 1
            }
        };
        &mut self.words[at]
    }

    /// The jump a tab's page view was sent that no read taken in since has told of. Not a kept one: the view has arrived, and its place is where it is.
    pub(crate) fn jump_on_its_way(&self, tab: TabId) -> Option<Stop> {
        self.words
            .iter()
            .find(|word| word.tab == tab)
            .and_then(|word| word.jump_on_its_way)
            .filter(|sent| sent.sent != KEPT)
            .map(|sent| sent.to)
    }

    /// **A read of a page view's places, stamped `stamp`, is to be taken in for a tab:** whether it's as new as what the session has. If it is, it's what
    /// the session has from here, with the `line` it said, and a jump sent at or before it is told of: done with, or for a jump to an entry kept, while
    /// the read's `anchor` is still where it put the reader.
    pub(crate) fn view_told_of(&mut self, tab: TabId, stamp: u64, line: (u32, u16), anchor: (u32, f64)) -> bool {
        let word = self.word(tab);
        if stamp < word.last_read {
            return false;
        }
        word.last_read = stamp;
        word.line = line;
        word.jump_on_its_way = word.jump_on_its_way.and_then(|sent| match sent.sent {
            KEPT => sent.holds(anchor).then_some(sent),
            at if at <= stamp => (sent.entry != crate::NONE && sent.holds(anchor)).then_some(Sent { sent: KEPT, ..sent }),
            _ => Some(sent),
        });
        true
    }

    /// The line the read taken in last said for a tab, with the place the tab has from it.
    pub(crate) fn view_line(&self, tab: TabId) -> Option<(u32, u16)> {
        self.words
            .iter()
            .find(|word| word.tab == tab)
            .map(|word| word.line)
            .filter(|(page, _)| *page != crate::NONE)
    }

    /// The tab's moved on, to another page or other pages: the jump it was sent is no longer where the reader is. A jump to an entry is kept, for the place
    /// that comes with the move to keep or end: the page view says another page on arriving where the view can't reach (near the end of the document), with
    /// the top of its view still where the jump put it, and a move by the person ends it with the place it brings.
    pub(crate) fn forget_jump_on_its_way(&mut self, tab: TabId) {
        if let Some(word) = self.words.iter_mut().find(|word| word.tab == tab) {
            word.jump_on_its_way = word
                .jump_on_its_way
                .filter(|sent| sent.entry != crate::NONE)
                .map(|sent| Sent { sent: KEPT, ..sent });
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
        self.jumps.retain(|(id, ..)| *id != tab);
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
        let at = jumps.iter().position(|(tab, ..)| *tab == id)?;
        Some(jumps.swap_remove(at).1)
    }

    /// **The jump just made in a tab goes to this contents entry** (`recto goto --entry`, a click on a contents row): it's the current entry from here
    /// until the view, having arrived, is moved from where the jump put it, or another jump is made (the mail's `Sent`). The caller has checked the entry is one of
    /// the tab's contents on the jump's page. Nothing without a jump waiting.
    pub fn jump_is_to_entry(&mut self, id: TabId, entry: u32) {
        if let Some((.., to_entry)) = self.0.jumps.iter_mut().find(|(tab, ..)| *tab == id) {
            *to_entry = entry;
        }
    }

    /// **A jump has gone to the page view,** the send having come to `stamp` on the place clock: it's where the reader is until a read at or past `stamp`
    /// is taken in, which may have happened already, between the send and this (a jump to an entry is then kept, for the next read to keep or end). It's no
    /// longer waiting, unless a newer jump has taken its place there since, which stays waiting for the next look.
    pub fn jump_sent(&mut self, tab: TabId, to: Stop, stamp: u64) {
        let entry = match self.0.jumps.iter().position(|(id, stop, _)| *id == tab && *stop == to) {
            Some(at) => self.0.jumps.swap_remove(at).2,
            None => crate::NONE,
        };
        let word = self.0.word(tab);
        let sent = if word.last_read < stamp { stamp } else { KEPT };
        word.jump_on_its_way = (sent != KEPT || entry != crate::NONE).then_some(Sent { to, entry, sent });
    }

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.0.place_lost)
    }
}
