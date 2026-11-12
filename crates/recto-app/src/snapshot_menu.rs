//! **The snapshot tool's place in the menu bar and the panel.** What the tool does is `recto-ffi`'s `snapshot_tool` for now; its items are here, with the rest of the menus, so the
//! menu bar is whole in one place.

use crate::menus::{MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection};

/// The menu's and the panel's id for the tool.
pub const TOGGLE_ID: &str = "tools.snapshot";
/// The tool's name in the menu and the panel.
pub const SNAPSHOT_TITLE: &str = "Snapshot";

/// The tool's place in the menu bar: Tools, with Snapshot ticked while it's on. ⇧⌘A, as a key nothing else here takes, and the screenshot keys are the system's.
pub fn tools_menu(snapshot_on: bool) -> MenuModel {
    MenuModel {
        title: "Tools".into(),
        entries: vec![MenuEntry {
            id: TOGGLE_ID.into(),
            title: SNAPSHOT_TITLE.into(),
            key: "A".into(),
            with_control: false,
            checked: snapshot_on,
            hidden: false,
            system_action: String::new(),
            parent: String::new(),
        }],
    }
}

/// The panel's tools, above its view options. The panel holds tools while reading (owner, 2026-11-07: "The sidebar menu is more for tools, like edit mode etc."), and commands are in the menu bar too (decision 17).
pub fn tools_section(snapshot_on: bool) -> PanelSection {
    PanelSection {
        title: "Tools".into(),
        items: vec![PanelItem {
            id: TOGGLE_ID.into(),
            title: SNAPSHOT_TITLE.into(),
            selected: snapshot_on,
            enabled: true,
        }],
    }
}

/// The tools go first in an open panel, except in edit mode, whose Done leads the panel and the tools come after it.
pub fn add_to_panel(panel: &mut PanelModel, snapshot_on: bool, edit_mode: bool) {
    if panel.open {
        let at = usize::from(edit_mode).min(panel.sections.len());
        panel.sections.insert(at, tools_section(snapshot_on));
    }
}

/// Whether a menu item is the tool's, for QA's `menu`, which takes only what a menu has.
pub fn is_menu_id(id: &str) -> bool {
    id == TOGGLE_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the tool can't be found or doesn't say whether it's on, in the menu bar or the panel, or its key is one something else takes.
    #[test]
    fn the_snapshot_tool_is_in_the_tools_menu_and_the_panel() {
        let menu = tools_menu(true);
        assert_eq!(
            (menu.entries[0].id.as_str(), menu.entries[0].checked, menu.entries[0].key.as_str()),
            (TOGGLE_ID, true, "A")
        );
        assert!(!tools_menu(false).entries[0].checked);
        assert!(tools_section(true).items[0].selected && !tools_section(false).items[0].selected);
        let taken: Vec<(String, bool)> = crate::menus::menu_bar(recto_session::ViewSettings::default(), None, "")
            .iter()
            .flat_map(|menu| &menu.entries)
            .map(|item| (item.key.clone(), item.with_control))
            .collect();
        assert!(!taken.contains(&("A".to_string(), false)), "⇧⌘A is free");
    }

    /// Failure: the snapshot tool pushes edit mode's Done off the top of the panel, where the reader leaves edit mode from, or shows in a closed panel.
    #[test]
    fn the_tools_go_first_except_after_edit_modes_done() {
        let mut reading = crate::menus::panel(true, false, true);
        add_to_panel(&mut reading, false, false);
        assert_eq!(reading.sections[0].items[0].id, TOGGLE_ID);
        let mut editing = crate::menus::panel(true, true, true);
        add_to_panel(&mut editing, false, true);
        assert_eq!(
            (editing.sections[0].items[0].id.as_str(), editing.sections[1].items[0].id.as_str()),
            ("edit_mode.done", TOGGLE_ID)
        );
        let mut closed = crate::menus::panel(false, false, true);
        add_to_panel(&mut closed, true, false);
        assert!(closed.sections.iter().all(|section| section.items.iter().all(|item| item.id != TOGGLE_ID)));
    }
}
