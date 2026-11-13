//! The view: settings, find, edit mode, selection, place and draws.

use super::*;

impl<L: DocLoader> Session<L> {
    /// What's selected in the tab showing, as the page view says. Only that one holds a selection: one that's left has lost its.
    pub fn note_selection(&mut self, showing: TabId, selection: Option<Selection>) {
        for tab in self.tabs.iter_mut() {
            if tab.reading.selection.is_some() && tab.id != showing {
                tab.reading.selection = None;
            }
        }
        self.tabs.set_selection(showing, selection);
    }

    /// **Exactly where a tab's reader is, as a read of the page view stamped `stamp` on the place clock says,** taken in only if it's as new as the
    /// read taken in last on this tab, whoever took that in: an older read still landing doesn't undo a newer one, the person's own scroll among it. A jump
    /// the view was sent at or before `stamp` is then told of ([`Self::here`]). Says whether it was taken. Marked to be written only if it's moved.
    pub fn view_told_of(&mut self, id: TabId, place: PlaceRecord, stamp: u64) -> bool {
        if self.tabs.tab(id).is_none() || !self.mail.view_told_of(id, stamp) {
            return false;
        }
        if let Some(tab) = self.tabs.tab_mut(id)
            && tab.reading.place != Some(place)
        {
            tab.reading.place = Some(place);
            tab.place_dirty = true;
        }
        true
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
