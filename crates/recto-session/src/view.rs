//! The view: settings, find, edit mode, selection, place and draws.

use super::*;
use crate::record::FindRecord;

impl<L: DocLoader> Session<L> {
    /// Find, as the reader left it.
    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged.
    pub fn find(&self) -> &FindRecord {
        self.viewing.find()
    }

    /// Sets what's in Find.
    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged.
    pub fn set_find(&mut self, open: bool, query: &str, draft: &str) {
        self.viewing.set_find(open, query, draft);
    }

    /// Turns the view of a tab, by quarter turns.
    pub fn set_view_rotation(&mut self, id: TabId, quarter_turns: u8) {
        if let Some(tab) = self.tab_mut(id) {
            tab.view_rotation = quarter_turns % 4;
        }
    }

    /// Lays a tab's pages out another way. Says whether that changed anything.
    pub fn set_spread(&mut self, id: TabId, spread: record::Spread) -> bool {
        match self.tab_mut(id) {
            Some(tab) if tab.spread != spread => {
                tab.spread = spread;
                true
            }
            _ => false,
        }
    }

    /// How a tab's pages are laid out.
    pub fn spread_of(&self, id: TabId) -> Option<record::Spread> {
        self.tab(id).map(|tab| tab.spread)
    }

    /// What's selected in a tab. Carried across a restart, and nothing else here reads it.
    pub fn set_selection(&mut self, id: TabId, selection: Option<Selection>) {
        if let Some(tab) = self.tab_mut(id) {
            tab.reading.selection = selection.map(Box::new);
        }
    }

    /// What's selected in the tab showing, as the page view says. Only that one holds a selection: one that's left has lost its.
    pub fn note_selection(&mut self, showing: TabId, selection: Option<Selection>) {
        for tab in self.tabs.iter_mut() {
            if tab.reading.selection.is_some() && tab.id != showing {
                tab.reading.selection = None;
            }
        }
        self.set_selection(showing, selection);
    }

    /// **Exactly where a tab's reader is, as a read of the page view stamped `stamp` on the place clock says,** taken in only if it's as new as the
    /// read taken in last on this tab, whoever took that in: an older read still landing doesn't undo a newer one, the person's own scroll among it. A jump
    /// the view was sent at or before `stamp` is then told of ([`Self::here`]). Says whether it was taken. Marked to be written only if it's moved.
    pub fn view_told_of(&mut self, id: TabId, place: PlaceRecord, stamp: u64) -> bool {
        if self.tab(id).is_none() || !self.mail.view_told_of(id, stamp) {
            return false;
        }
        if let Some(tab) = self.tab_mut(id)
            && tab.reading.place != Some(place)
        {
            tab.reading.place = Some(place);
            tab.place_dirty = true;
        }
        true
    }

    /// Where to bring a tab back to: only if it's still about the page the tab is on, as a `goto` while it was cold leaves it not.
    pub fn place_to_show(&self, id: TabId) -> Option<PlaceRecord> {
        let tab = self.tab(id)?;
        tab.reading.place.filter(|place| place.page == tab.page)
    }

    /// What's changed that a line of the log of places can say. Marks it as said.
    pub fn take_place_delta(&mut self, seq: u64) -> PlaceDelta {
        let mut places = Vec::new();
        for tab in self.tabs.iter_mut() {
            if std::mem::take(&mut tab.place_dirty) {
                places.push((tab.id.0, tab.page, tab.reading.place));
            }
        }
        PlaceDelta {
            seq,
            active: self.tabs.active().map(|id| id.0),
            places,
            contents_scroll: None,
            focus: None,
        }
    }

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        self.mail.take_place_lost()
    }

    /// Notes that a page finished drawing, or couldn't be.
    pub fn note_drawn(&mut self, note: DrawNote) {
        self.mail.note_drawn(note);
    }

    /// What's known about a page's latest draw, if it's still remembered.
    pub fn draw_note(&self, tab: TabId, page: u32) -> Option<DrawNote> {
        self.mail.draw_note(tab, page)
    }
}
