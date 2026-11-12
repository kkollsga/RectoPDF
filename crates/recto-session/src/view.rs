//! The view: settings, find, edit mode, selection, place and draws.

use super::*;

impl<L: DocLoader> Session<L> {
    pub fn view(&self) -> ViewSettings {
        self.view
    }

    pub fn set_view(&mut self, view: ViewSettings) {
        self.view = view;
    }

    /// Find, as the reader left it.
    pub fn find(&self) -> &FindRecord {
        &self.find
    }

    /// Sets what's in Find. Longer text than [`record::MAX_FIND_BYTES`] is cut at a character.
    pub fn set_find(&mut self, open: bool, query: &str, draft: &str) {
        let cut = |text: &str| {
            let mut end = text.len().min(record::MAX_FIND_BYTES);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_string()
        };
        let (current, current_tab) = if query == self.find.query {
            (self.find.current, self.find.current_tab)
        } else {
            (None, None)
        };
        self.find = FindRecord {
            open,
            query: cut(query),
            draft: cut(draft),
            current,
            current_tab,
        };
    }

    /// The hit Find went to last, by its place, for the record.
    pub fn set_find_current(&mut self, current: Option<(u32, u32)>, tab: Option<u32>) {
        self.find.current = current;
        self.find.current_tab = tab;
    }

    pub fn edit_mode(&self) -> bool {
        self.edit_mode
    }

    pub fn set_edit_mode(&mut self, on: bool) {
        self.edit_mode = on;
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
            tab.selection = selection.map(Box::new);
        }
    }

    /// What's selected in the tab showing, as the page view says. Only that one holds a selection: one that's left has lost its.
    pub fn note_selection(&mut self, showing: TabId, selection: Option<Selection>) {
        for tab in &mut self.tabs {
            if tab.selection.is_some() && tab.id != showing {
                tab.selection = None;
            }
        }
        self.set_selection(showing, selection);
    }

    /// What's selected in a tab.
    pub fn selection_of(&self, id: TabId) -> Option<Selection> {
        self.tab(id)?.selection.as_deref().copied()
    }

    /// Exactly where a tab's reader is, as the page view says. Marked to be written only if it's moved.
    pub fn note_place(&mut self, id: TabId, place: PlaceRecord) {
        if let Some(tab) = self.tab_mut(id)
            && tab.place != Some(place)
        {
            tab.place = Some(place);
            tab.place_dirty = true;
        }
    }

    /// Where to bring a tab back to: only if it's still about the page the tab is on, as a `goto` while it was cold leaves it not.
    pub fn place_to_show(&self, id: TabId) -> Option<PlaceRecord> {
        let tab = self.tab(id)?;
        tab.place.filter(|place| place.page == tab.page)
    }

    /// What's changed that a line of the log of places can say. Marks it as said.
    pub fn take_place_delta(&mut self, seq: u64) -> PlaceDelta {
        let mut places = Vec::new();
        for tab in &mut self.tabs {
            if std::mem::take(&mut tab.place_dirty) {
                places.push((tab.id.0, tab.page, tab.place));
            }
        }
        PlaceDelta {
            seq,
            active: self.active.map(|id| id.0),
            places,
            contents_scroll: None,
            focus: None,
        }
    }

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.place_lost)
    }

    /// Notes that a page finished drawing, or couldn't be.
    pub fn note_drawn(&mut self, note: DrawNote) {
        self.draws.retain(|d| (d.tab, d.page) != (note.tab, note.page));
        self.draws.push_back(note);
        while self.draws.len() > MAX_DRAWS_KEPT {
            self.draws.pop_front();
        }
    }

    /// What's known about a page's latest draw, if it's still remembered.
    pub fn draw_note(&self, tab: TabId, page: u32) -> Option<DrawNote> {
        self.draws.iter().rev().find(|d| (d.tab, d.page) == (tab, page)).copied()
    }
}
