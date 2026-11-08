//! The menu bar and the sidebar's panel, as Rust's lists. The host only draws them and says which was chosen. Later phases add to these lists, and what's on them is what exists: no placeholders for what doesn't.

use recto_reader::tiles::Zoom;
use recto_session::record::Spread;
use recto_session::{Paper, ViewSettings};

/// One item of a menu. An empty `id` is a line between items.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MenuEntry {
    pub id: String,
    pub title: String,
    /// The key that goes with ⌘, or none.
    pub key: String,
    pub with_control: bool,
    pub checked: bool,
    /// Not listed, but its key works: the same action under another key.
    pub hidden: bool,
    /// The standard selector this goes by, such as `copy:`, so it reaches whatever has the focus first, as it does in any Mac app: a text field in an open panel keeps its own ⌘C. Empty for items that are this app's own.
    pub system_action: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MenuModel {
    pub title: String,
    pub entries: Vec<MenuEntry>,
}

/// What the host has to do itself, after an item is chosen. Everything else is done by then.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HostAction {
    None,
    ShowOpenPanel,
    ShowAbout,
    ShowSettings,
    Quit,
    CloseWindow,
}

/// What an item does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    /// Clears the showing tab's edit history, SPEC decision 25. Its unsaved edits stay.
    ClearEditHistory,
    Host(HostAction),
    Copy { anyway: bool },
    SelectAll,
    Zoom(Zoom),
    /// Lays the tab showing out another way.
    SetSpread(Spread),
    /// The panel's Rotate, by quarter turns: every page outside edit mode, the page being read in it.
    Rotate(i32),
    Undo,
    Redo,
    /// Shows the sidebar, or hides it: its button beside the window's controls, and the View menu.
    ToggleSidebar,
    ToggleAgentIcon,
    SetPaper(Paper),
    TogglePanel,
    /// *Check for Updates…*, or *Update to X.Y.Z…* once there's one.
    Update,
}

fn entry(id: &str, title: &str, key: &str) -> MenuEntry {
    MenuEntry { id: id.into(), title: title.into(), key: key.into(), with_control: false, checked: false, hidden: false, system_action: String::new() }
}

/// The items that go by a standard selector. In the menu bar and the context menu alike.
fn edit_entries() -> Vec<MenuEntry> {
    vec![MenuEntry { system_action: "copy:".into(), ..entry("edit.copy", "Copy", "c") }, MenuEntry { system_action: "selectAll:".into(), ..entry("edit.select_all", "Select All", "a") }]
}

/// Undo and Redo, by the standard selectors, so a text field with the focus keeps its own. `Z` is ⇧⌘Z.
fn undo_entries() -> Vec<MenuEntry> {
    vec![MenuEntry { system_action: "undo:".into(), ..entry("edit.undo", "Undo", "z") }, MenuEntry { system_action: "redo:".into(), ..entry("edit.redo", "Redo", "Z") }]
}

/// The layouts, the one showing ticked. Owner, 2026-11-07: "In the right click menu you can select side by side layout, with and without header page."
fn layout_entries(shown: Option<Spread>) -> Vec<MenuEntry> {
    Spread::ALL.iter().map(|spread| MenuEntry { checked: shown == Some(*spread), ..entry(&layout_id(*spread), spread.label(), "") }).collect()
}

fn layout_id(spread: Spread) -> String {
    format!("layout.{}", spread.name().replace('-', "_"))
}

/// What a right-click on a page offers: Copy and Select All, then the layouts. The system adds Services and the like itself, as it does to any menu that has a selection to act on.
pub(crate) fn context_menu(shown: Option<Spread>) -> Vec<MenuEntry> {
    let mut entries = edit_entries();
    for item in &mut entries {
        item.key = String::new();
    }
    entries.push(line());
    entries.extend(layout_entries(shown));
    entries
}

/// Whether an item can be chosen now. Asked when a menu opens and on its key, so it's cheap, and nothing's rebuilt as a selection comes and goes.
pub(crate) fn enabled(id: &str, has_document: bool, has_selection: bool) -> bool {
    match id {
        "edit.copy" | "edit.copy_anyway" => has_selection,
        "edit.select_all" | "edit.clear_history" => has_document,
        layout if layout.starts_with("layout.") => has_document,
        _ => true,
    }
}

fn line() -> MenuEntry {
    entry("", "", "")
}

/// The whole menu bar, as it stands.
pub(crate) fn menu_bar(view: ViewSettings, shown: Option<Spread>, update_title: &str) -> Vec<MenuModel> {
    vec![
        MenuModel { title: "RectoPDF".into(), entries: vec![entry("app.about", "About RectoPDF", ""), entry("app.update", update_title, ""), line(), entry("app.settings", "Settings…", ","), line(), entry("app.services", "Services", ""), line(), entry("app.quit", "Quit RectoPDF", "q")] },
        // Owner, 2026-11-07: "The full app settings should be accessible from file -> settings". The app menu keeps its own, as on any Mac, and both say ⌘,.
        MenuModel { title: "File".into(), entries: vec![entry("file.open", "Open…", "o"), line(), entry("file.settings", "Settings…", ","), line(), entry("file.close_window", "Close Window", "w")] },
        MenuModel { title: "Edit".into(), entries: undo_entries().into_iter().chain([line()]).chain(edit_entries()).chain([line(), entry("edit.clear_history", "Clear Edit History", "")]).collect() },
        MenuModel {
            title: "View".into(),
            entries: vec![
                entry("view.zoom_in", "Zoom In", "+"),
                MenuEntry { hidden: true, ..entry("view.zoom_in", "Zoom In", "=") },
                entry("view.zoom_out", "Zoom Out", "-"),
                entry("view.actual_size", "Actual Size", "0"),
                entry("view.fit", "Fit", "9"),
                line(),
                // ⌃⌘S, as on any Mac app with a sidebar. The title says what choosing it does.
                MenuEntry { with_control: true, ..entry("view.sidebar", if view.sidebar_hidden { "Show Sidebar" } else { "Hide Sidebar" }, "s") },
                line(),
            ]
            .into_iter()
            .chain(layout_entries(shown))
            .chain([
                line(),
                MenuEntry { checked: !view.agent_icon_hidden, ..entry("view.agent_icon", "Show Agent Icon", "") },
            ])
            .collect(),
        },
    ]
}

/// The zoom level in the top left, and what clicking it offers. Owner, 2026-11-07: "lets show the zoom level in upper left and clicking the zoom value enables you to type in zoom level or reset".
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ZoomLabel {
    /// As it's shown: `125%`.
    pub text: String,
    /// What's put in the field to type over: the number alone.
    pub editing_text: String,
    pub tooltip: String,
    /// The button beside the field, which goes back to the fitted width.
    pub reset_title: String,
}

/// The zoom level's words, from the magnification and the page's fit.
pub(crate) fn zoom_label(magnification: f64, fit: f64) -> ZoomLabel {
    let percent = recto_reader::tiles::zoom_percent(magnification, fit).round().clamp(1.0, 100_000.0);
    ZoomLabel { text: format!("{percent:.0}%"), editing_text: format!("{percent:.0}"), tooltip: "Zoom. Click to type a level, or to fit the page again".into(), reset_title: "Fit".into() }
}

/// What's in the sidebar's panel, for now: one tab's worth.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PanelItem {
    pub id: String,
    pub title: String,
    pub selected: bool,
    /// Greyed out when it can't do anything now.
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PanelSection {
    pub title: String,
    pub items: Vec<PanelItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PanelModel {
    pub open: bool,
    pub button_tooltip: String,
    pub sections: Vec<PanelSection>,
}

/// The panel, with the paper showing now picked out. Owner, 2026-11-07: "The sidebar menu is more for tools, like edit mode etc." So Rotate is here: every page together outside edit mode, and the page being read in it.
pub(crate) fn panel(open: bool, shown: Paper, edit_mode: bool, can_rotate: bool) -> PanelModel {
    let item = |id: &str, title: &str, selected: bool| PanelItem { id: id.into(), title: title.into(), selected, enabled: true };
    let rotate = |id: &str, title: &str| PanelItem { enabled: can_rotate, ..item(id, title, false) };
    let sections = if open {
        vec![
            PanelSection { title: if edit_mode { "Rotate this page" } else { "Rotate all pages" }.into(), items: vec![rotate("rotate.left", "Rotate left"), rotate("rotate.right", "Rotate right")] },
            PanelSection { title: "Paper".into(), items: Paper::ALL.iter().map(|paper| item(&format!("paper.{}", paper.name()), paper.label(), *paper == shown)).collect() },
        ]
    } else {
        Vec::new()
    };
    PanelModel { open, button_tooltip: "View options".into(), sections }
}

/// What an id does. Ids from the host, and only these do anything.
pub(crate) fn action(id: &str) -> Option<Action> {
    Some(match id {
        "app.about" => Action::Host(HostAction::ShowAbout),
        "app.settings" | "file.settings" => Action::Host(HostAction::ShowSettings),
        "app.update" => Action::Update,
        // The system fills this one.
        "app.services" => Action::Host(HostAction::None),
        "edit.copy" => Action::Copy { anyway: false },
        "edit.copy_anyway" => Action::Copy { anyway: true },
        "edit.select_all" => Action::SelectAll,
        "edit.clear_history" => Action::ClearEditHistory,
        "app.quit" => Action::Host(HostAction::Quit),
        "file.open" => Action::Host(HostAction::ShowOpenPanel),
        "file.close_window" => Action::Host(HostAction::CloseWindow),
        "view.zoom_in" => Action::Zoom(Zoom::In),
        "view.zoom_out" => Action::Zoom(Zoom::Out),
        "view.actual_size" => Action::Zoom(Zoom::Actual),
        "view.fit" => Action::Zoom(Zoom::Fit),
        "view.sidebar" => Action::ToggleSidebar,
        "layout.single" => Action::SetSpread(Spread::Single),
        "rotate.left" => Action::Rotate(-1),
        "rotate.right" => Action::Rotate(1),
        "edit.undo" => Action::Undo,
        "edit.redo" => Action::Redo,
        "layout.two_up" => Action::SetSpread(Spread::TwoUp),
        "layout.two_up_cover" => Action::SetSpread(Spread::TwoUpCover),
        "view.agent_icon" => Action::ToggleAgentIcon,
        "panel.toggle" => Action::TogglePanel,
        "paper.white" => Action::SetPaper(Paper::White),
        "paper.warm" => Action::SetPaper(Paper::Warm),
        "paper.dark" => Action::SetPaper(Paper::Dark),
        _ => return None,
    })
}

/// The id a key does with no ⌘ held, when no text field has it. Owner, 2026-11-07: "enable + - and command - scroll for zooming".
pub(crate) fn plain_key(characters: &str) -> Option<&'static str> {
    match characters {
        "+" | "=" => Some("view.zoom_in"),
        "-" => Some("view.zoom_out"),
        _ => None,
    }
}

/// What `view --zoom` takes.
pub(crate) fn zoom_from_word(word: &str) -> Option<Zoom> {
    match word {
        "in" => Some(Zoom::In),
        "out" => Some(Zoom::Out),
        "fit" => Some(Zoom::Fit),
        "actual" => Some(Zoom::Actual),
        level if level.trim_end().ends_with('%') => recto_reader::tiles::typed_zoom(level, '.'),
        number => number.parse::<f64>().ok().filter(|value| value.is_finite()).map(Zoom::To),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: something in the menu bar or the panel does nothing when chosen, as an item with an id nobody handles would, or a list shows what doesn't exist.
    #[test]
    fn everything_listed_does_something() {
        for view in [ViewSettings::default(), ViewSettings { sidebar_hidden: true, agent_icon_hidden: true, ..ViewSettings::default() }] {
            for menu in menu_bar(view, None, "Check for Updates…") {
                assert!(!menu.title.is_empty());
                for item in menu.entries {
                    assert!(item.id.is_empty() || action(&item.id).is_some(), "{}", item.id);
                }
            }
        }
        for section in panel(true, Paper::White, false, true).sections {
            for item in section.items {
                assert!(action(&item.id).is_some(), "{}", item.id);
            }
        }
        assert_eq!(action("anything else"), None);
    }

    /// Failure: Copy can be chosen with nothing selected, or Select All with nothing open, or a menu item that has nothing to do with selection goes grey with it. And Copy goes by the app's own action, so it takes ⌘C from a text field that has the focus.
    #[test]
    fn copy_and_select_all_are_enabled_as_on_any_mac_app() {
        assert!(!enabled("edit.copy", true, false));
        assert!(enabled("edit.copy", true, true));
        assert!(!enabled("edit.select_all", false, false));
        assert!(enabled("edit.select_all", true, false));
        assert!(enabled("view.zoom_in", false, false));
        let edit = menu_bar(ViewSettings::default(), None, "Check for Updates…").into_iter().find(|menu| menu.title == "Edit").unwrap();
        let copy = edit.entries.iter().find(|item| item.id == "edit.copy").unwrap();
        assert_eq!((copy.key.as_str(), copy.system_action.as_str()), ("c", "copy:"));
        let all = edit.entries.iter().find(|item| item.id == "edit.select_all").unwrap();
        assert_eq!((all.key.as_str(), all.system_action.as_str()), ("a", "selectAll:"));
        assert!(context_menu(None).iter().all(|item| item.key.is_empty() && (item.id.is_empty() || action(&item.id).is_some())), "no keys shown in a context menu");
    }

    /// Failure: the toggle doesn't say which way it is, or the paper showing isn't picked out, or the panel has anything in it at rest.
    #[test]
    fn the_lists_say_how_things_stand() {
        let checked = |view, id: &str| menu_bar(view, None, "Check for Updates…").into_iter().flat_map(|menu| menu.entries).find(|item| item.id == id).unwrap().checked;
        assert!(checked(ViewSettings::default(), "view.agent_icon"));
        assert!(!checked(ViewSettings { agent_icon_hidden: true, ..ViewSettings::default() }, "view.agent_icon"));
        assert!(panel(false, Paper::White, false, true).sections.is_empty(), "only the button at rest");
        let open = panel(true, Paper::Warm, false, true);
        let selected: Vec<String> = open.sections.iter().flat_map(|section| &section.items).filter(|item| item.selected).map(|item| item.id.clone()).collect();
        assert_eq!(selected, vec!["paper.warm".to_string()]);
    }

    /// Failure: the sidebar's menu item doesn't say what choosing it does, or hasn't the Mac's usual ⌃⌘S, or focus view is still there. Owner, 2026-11-08: "No need mor a separate mode".
    #[test]
    fn the_view_menu_shows_and_hides_the_sidebar() {
        let item = |view| menu_bar(view, None, "Check for Updates…").into_iter().flat_map(|menu| menu.entries).find(|item| item.id == "view.sidebar").unwrap();
        let shown = item(ViewSettings::default());
        assert_eq!((shown.title.as_str(), shown.key.as_str(), shown.with_control), ("Hide Sidebar", "s", true));
        assert_eq!(item(ViewSettings { sidebar_hidden: true, ..ViewSettings::default() }).title, "Show Sidebar");
        assert_eq!(action("view.sidebar"), Some(Action::ToggleSidebar));
        assert_eq!(action("view.focus"), None);
        assert!(menu_bar(ViewSettings::default(), None, "").iter().flat_map(|menu| &menu.entries).all(|item| !item.title.contains("Focus")));
    }

    /// Failure: the panel's Rotate isn't there, or says it turns every page in edit mode where it turns one, or can be pressed where it can't turn anything, or Undo and Redo aren't on the Edit menu with their keys and standard selectors.
    #[test]
    fn the_panel_rotates_and_the_edit_menu_undoes() {
        let rotate = |edit_mode| panel(true, Paper::White, edit_mode, true).sections.into_iter().find(|section| section.items.iter().any(|item| item.id == "rotate.right")).unwrap();
        assert_eq!(rotate(false).title, "Rotate all pages");
        assert_eq!(rotate(true).title, "Rotate this page");
        assert_eq!((action("rotate.left"), action("rotate.right")), (Some(Action::Rotate(-1)), Some(Action::Rotate(1))));
        let greyed = panel(true, Paper::White, false, false).sections.into_iter().flat_map(|section| section.items).filter(|item| item.id.starts_with("rotate.")).all(|item| !item.enabled);
        assert!(greyed, "Rotate is greyed out where it can't turn anything");
        let edit = menu_bar(ViewSettings::default(), None, "").into_iter().find(|menu| menu.title == "Edit").unwrap().entries;
        let keyed = |id: &str| edit.iter().find(|item| item.id == id).map(|item| (item.key.clone(), item.system_action.clone())).unwrap();
        assert_eq!(keyed("edit.undo"), ("z".to_string(), "undo:".to_string()));
        assert_eq!(keyed("edit.redo"), ("Z".to_string(), "redo:".to_string()));
    }

    /// Failure: the right-click menu doesn't offer the three layouts, or doesn't tick the one showing, or offers focus view, which is gone; or a layout can be chosen with nothing open. Owner, 2026-11-07: "In the right click menu you can select side by side layout, with and without header page."
    #[test]
    fn the_right_click_menu_offers_the_layouts() {
        let menu = context_menu(Some(Spread::TwoUpCover));
        let layouts: Vec<(&str, bool)> = menu.iter().filter(|item| item.id.starts_with("layout.")).map(|item| (item.id.as_str(), item.checked)).collect();
        assert_eq!(layouts, [("layout.single", false), ("layout.two_up", false), ("layout.two_up_cover", true)]);
        assert!(menu.iter().all(|item| !item.title.contains("Focus")));
        for spread in Spread::ALL {
            assert_eq!(action(&layout_id(spread)), Some(Action::SetSpread(spread)));
        }
        assert!(!enabled("layout.two_up", false, false) && enabled("layout.two_up", true, false));
        let view = menu_bar(ViewSettings::default(), Some(Spread::TwoUp), "").into_iter().find(|menu| menu.title == "View").unwrap();
        assert!(view.entries.iter().any(|item| item.id == "layout.two_up" && item.checked), "and in the View menu, as on a Mac");
    }

    /// Failure: the owner's *Check for Updates…* isn't where it was asked for, doesn't turn into the offer, or Settings has no ⌘, as on any Mac app, or isn't under File as the owner asked.
    #[test]
    fn the_app_menu_has_the_update_item_and_settings() {
        let items = menu_bar(ViewSettings::default(), None, "Update to 0.2.0…").into_iter().find(|menu| menu.title == "RectoPDF").unwrap().entries;
        let item = |id: &str| items.iter().find(|item| item.id == id).unwrap().clone();
        assert_eq!(item("app.update").title, "Update to 0.2.0…");
        assert_eq!((item("app.settings").title.as_str(), item("app.settings").key.as_str()), ("Settings…", ","));
        assert_eq!(action("app.update"), Some(Action::Update));
        assert_eq!(action("app.settings"), Some(Action::Host(HostAction::ShowSettings)));
        let file = menu_bar(ViewSettings::default(), None, "").into_iter().find(|menu| menu.title == "File").unwrap().entries;
        let settings = file.iter().find(|item| item.id == "file.settings").expect("Settings… in File too: owner, 2026-11-07");
        assert_eq!((settings.title.as_str(), settings.key.as_str()), ("Settings…", ","));
        assert_eq!(action("file.settings"), Some(Action::Host(HostAction::ShowSettings)));
    }

    /// Failure: the zoom level reads oddly, as `124.99999%` or `0%`, or the field to type in starts with a sign in it to delete.
    #[test]
    fn the_zoom_level_reads_as_a_whole_percentage() {
        assert_eq!(zoom_label(1.0, 1.2418).text, "124%");
        assert_eq!(zoom_label(1.0, 1.2418).editing_text, "124");
        assert_eq!(zoom_label(0.25, 0.01).text, "1%", "never 0%");
        assert_eq!(zoom_label(f64::NAN, 1.0).text, "100%");
        assert!(!zoom_label(1.0, 1.0).reset_title.is_empty());
    }

    /// Failure: a key meant for typing or for something else zooms, or `+` on a keyboard that has it as shifted `=` doesn't.
    #[test]
    fn only_plus_and_minus_are_zoom_keys() {
        assert_eq!(plain_key("+"), Some("view.zoom_in"));
        assert_eq!(plain_key("="), Some("view.zoom_in"));
        assert_eq!(plain_key("-"), Some("view.zoom_out"));
        for other in ["a", "0", "", " ", "\u{1b}"] {
            assert_eq!(plain_key(other), None);
        }
        assert_eq!(zoom_from_word("in"), Some(Zoom::In));
        assert_eq!(zoom_from_word("2.5"), Some(Zoom::To(2.5)));
        assert_eq!(zoom_from_word("150%"), Some(Zoom::Percent(150.0)), "a level, as the top left shows it");
        assert_eq!(zoom_from_word("x%"), None);
        assert_eq!(zoom_from_word("NaN"), None);
        assert_eq!(zoom_from_word("zzz"), None);
    }
}
