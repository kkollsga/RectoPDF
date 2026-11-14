//! How pages are shown for every tab, Find as the reader left it, and edit mode: what the session keeps of the view that isn't any one tab's.

use crate::record::{self, FindRecord, ViewRecord};
use crate::{Paper, ViewSettings};

/// The view's settings, Find and edit mode.
#[derive(Debug)]
pub struct Viewing {
    view: ViewSettings,
    /// Find, as the reader left it.
    find: FindRecord,
    /// Edit mode: the panel opens on its tools, and Rotate turns the page being read rather than every page.
    edit_mode: bool,
    /// Find's hit gone to last was stepped to from a place withheld from agents (S2's round): it's the next hit past where following left a tab, so an
    /// agent is told nothing of it and steps from nothing of it. In memory only, and let go with the hit.
    find_current_withheld: bool,
}

impl Viewing {
    /// The view as it is with nothing set, Find empty and edit mode off. The crate's: see [`crate::parts::PartMut`].
    pub(crate) fn new() -> Self {
        Self {
            view: ViewSettings::default(),
            find: FindRecord::default(),
            edit_mode: false,
            find_current_withheld: false,
        }
    }

    pub fn view(&self) -> ViewSettings {
        self.view
    }

    /// Find, as the reader left it.
    pub fn find(&self) -> &FindRecord {
        &self.find
    }

    pub fn edit_mode(&self) -> bool {
        self.edit_mode
    }

    /// Whether Find's hit gone to last was stepped to from a place withheld from agents.
    pub fn find_current_withheld(&self) -> bool {
        self.find_current_withheld && self.find.current.is_some()
    }

    /// What the record has of these: the view, Find and edit mode. Every field is named, so one added doesn't compile until it's carried or said not to be.
    pub(crate) fn to_record(&self) -> (ViewRecord, FindRecord, bool) {
        let Self {
            view,
            find,
            edit_mode,
            find_current_withheld: _,
        } = self;
        let ViewSettings {
            paper,
            sidebar_hidden,
            agent_icon_hidden,
        } = *view;
        (
            ViewRecord {
                paper: paper.map(Paper::to_record),
                focus: false,
                sidebar_hidden,
                agent_icon_hidden,
            },
            find.clone(),
            *edit_mode,
        )
    }

    /// As a record has them.
    pub(crate) fn restore(&mut self, view: &ViewRecord, find: &FindRecord, edit_mode: bool) {
        self.find = find.clone();
        self.find_current_withheld = false;
        self.edit_mode = edit_mode;
        let sidebar_hidden = view.sidebar_comes_back_hidden();
        let ViewRecord {
            paper,
            focus: _,
            sidebar_hidden: _,
            agent_icon_hidden,
        } = *view;
        self.view = ViewSettings {
            paper: paper.map(Paper::from_record),
            sidebar_hidden,
            agent_icon_hidden,
        };
    }
}

/// What a caller outside the crate may change of the view: all of it, as nothing else holds any of it.
impl crate::parts::PartMut<'_, Viewing> {
    pub fn set_view(&mut self, view: ViewSettings) {
        self.0.view = view;
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
        let find = &mut self.0.find;
        let (current, current_tab) = if query == find.query {
            (find.current, find.current_tab)
        } else {
            (None, None)
        };
        *find = FindRecord {
            open,
            query: cut(query),
            draft: cut(draft),
            current,
            current_tab,
        };
    }

    /// The hit Find went to last, by its place, for the record.
    pub fn set_find_current(&mut self, current: Option<(u32, u32)>, tab: Option<u32>) {
        self.0.find.current = current;
        self.0.find.current_tab = tab;
        self.0.find_current_withheld = false;
    }

    /// The hit gone to last was stepped to from a place withheld from agents: [`Viewing::find_current_withheld`]. Let go when the next is set.
    pub fn withhold_find_current(&mut self) {
        self.0.find_current_withheld = true;
    }

    pub fn set_edit_mode(&mut self, on: bool) {
        self.0.edit_mode = on;
    }
}
