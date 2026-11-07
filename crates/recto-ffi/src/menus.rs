//! The menu bar and the sidebar's panel, as Rust's lists. The host only draws them and says which was chosen. Later phases add to these lists, and what's on them is what exists: no placeholders for what doesn't.

use recto_reader::tiles::Zoom;
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
    Quit,
    CloseWindow,
}

/// What an item does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    Host(HostAction),
    Zoom(Zoom),
    ToggleFocus,
    ToggleAgentIcon,
    SetPaper(Paper),
    TogglePanel,
}

fn entry(id: &str, title: &str, key: &str) -> MenuEntry {
    MenuEntry { id: id.into(), title: title.into(), key: key.into(), with_control: false, checked: false, hidden: false }
}

fn line() -> MenuEntry {
    entry("", "", "")
}

/// The whole menu bar, as it stands.
pub(crate) fn menu_bar(view: ViewSettings) -> Vec<MenuModel> {
    vec![
        MenuModel { title: "RectoPDF".into(), entries: vec![entry("app.about", "About RectoPDF", ""), line(), entry("app.quit", "Quit RectoPDF", "q")] },
        MenuModel { title: "File".into(), entries: vec![entry("file.open", "Open…", "o"), entry("file.close_window", "Close Window", "w")] },
        MenuModel {
            title: "View".into(),
            entries: vec![
                entry("view.zoom_in", "Zoom In", "+"),
                MenuEntry { hidden: true, ..entry("view.zoom_in", "Zoom In", "=") },
                entry("view.zoom_out", "Zoom Out", "-"),
                entry("view.actual_size", "Actual Size", "0"),
                entry("view.fit", "Fit", "9"),
                line(),
                MenuEntry { with_control: true, checked: view.focus, ..entry("view.focus", "Focus View", "f") },
                MenuEntry { checked: !view.agent_icon_hidden, ..entry("view.agent_icon", "Show Agent Icon", "") },
            ],
        },
    ]
}

/// What's in the sidebar's panel, for now: one tab's worth.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PanelItem {
    pub id: String,
    pub title: String,
    pub selected: bool,
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

/// The panel, with the paper showing now picked out.
pub(crate) fn panel(open: bool, shown: Paper, focus: bool) -> PanelModel {
    let item = |id: &str, title: &str, selected: bool| PanelItem { id: id.into(), title: title.into(), selected };
    let sections = if open {
        vec![
            PanelSection { title: "Paper".into(), items: Paper::ALL.iter().map(|paper| item(&format!("paper.{}", paper.name()), paper.label(), *paper == shown)).collect() },
            PanelSection { title: "Zoom".into(), items: vec![item("view.zoom_out", "Zoom out", false), item("view.zoom_in", "Zoom in", false), item("view.fit", "Fit", false)] },
            PanelSection { title: String::new(), items: vec![item("view.focus", "Focus view", focus)] },
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
        "app.quit" => Action::Host(HostAction::Quit),
        "file.open" => Action::Host(HostAction::ShowOpenPanel),
        "file.close_window" => Action::Host(HostAction::CloseWindow),
        "view.zoom_in" => Action::Zoom(Zoom::In),
        "view.zoom_out" => Action::Zoom(Zoom::Out),
        "view.actual_size" => Action::Zoom(Zoom::Actual),
        "view.fit" => Action::Zoom(Zoom::Fit),
        "view.focus" => Action::ToggleFocus,
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
        number => number.parse::<f64>().ok().filter(|value| value.is_finite()).map(Zoom::To),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: something in the menu bar or the panel does nothing when chosen, as an item with an id nobody handles would, or a list shows what doesn't exist.
    #[test]
    fn everything_listed_does_something() {
        for view in [ViewSettings::default(), ViewSettings { focus: true, agent_icon_hidden: true, ..ViewSettings::default() }] {
            for menu in menu_bar(view) {
                assert!(!menu.title.is_empty());
                for item in menu.entries {
                    assert!(item.id.is_empty() || action(&item.id).is_some(), "{}", item.id);
                }
            }
        }
        for section in panel(true, Paper::White, false).sections {
            for item in section.items {
                assert!(action(&item.id).is_some(), "{}", item.id);
            }
        }
        assert_eq!(action("anything else"), None);
    }

    /// Failure: the toggle doesn't say which way it is, or the paper showing isn't picked out, or the panel has anything in it at rest.
    #[test]
    fn the_lists_say_how_things_stand() {
        let checked = |view, id: &str| menu_bar(view).into_iter().flat_map(|menu| menu.entries).find(|item| item.id == id).unwrap().checked;
        assert!(checked(ViewSettings::default(), "view.agent_icon"));
        assert!(!checked(ViewSettings { agent_icon_hidden: true, ..ViewSettings::default() }, "view.agent_icon"));
        assert!(panel(false, Paper::White, false).sections.is_empty(), "only the button at rest");
        let open = panel(true, Paper::Warm, false);
        let selected: Vec<String> = open.sections.iter().flat_map(|section| &section.items).filter(|item| item.selected).map(|item| item.id.clone()).collect();
        assert_eq!(selected, vec!["paper.warm".to_string()]);
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
        assert_eq!(zoom_from_word("NaN"), None);
        assert_eq!(zoom_from_word("zzz"), None);
    }
}
