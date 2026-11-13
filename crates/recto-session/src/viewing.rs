//! How pages are shown for every tab, Find as the reader left it, and edit mode: what the session keeps of the view that isn't any one tab's.

use crate::record::{self, FindRecord, ViewRecord};
use crate::{Paper, ViewSettings};

/// The view's settings, Find and edit mode.
#[derive(Debug, Default)]
pub(crate) struct Viewing {
    view: ViewSettings,
    /// Find, as the reader left it.
    find: FindRecord,
    /// Edit mode: the panel opens on its tools, and Rotate turns the page being read rather than every page.
    edit_mode: bool,
}

impl Viewing {
    pub(crate) fn view(&self) -> ViewSettings {
        self.view
    }

    pub(crate) fn set_view(&mut self, view: ViewSettings) {
        self.view = view;
    }

    pub(crate) fn find(&self) -> &FindRecord {
        &self.find
    }

    /// Sets what's in Find. Longer text than [`record::MAX_FIND_BYTES`] is cut at a character.
    pub(crate) fn set_find(&mut self, open: bool, query: &str, draft: &str) {
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

    pub(crate) fn set_find_current(&mut self, current: Option<(u32, u32)>, tab: Option<u32>) {
        self.find.current = current;
        self.find.current_tab = tab;
    }

    pub(crate) fn edit_mode(&self) -> bool {
        self.edit_mode
    }

    pub(crate) fn set_edit_mode(&mut self, on: bool) {
        self.edit_mode = on;
    }

    /// What the record has of these: the view, Find and edit mode. Every field is named, so one added doesn't compile until it's carried or said not to be.
    pub(crate) fn to_record(&self) -> (ViewRecord, FindRecord, bool) {
        let Self { view, find, edit_mode } = self;
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
