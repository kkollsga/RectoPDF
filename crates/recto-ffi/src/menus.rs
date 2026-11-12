//! The menu bar's and the panel's records, as the host draws them. The lists, what each item does and when it's enabled are `recto_app::menus`'s.

use recto_app::menus as app;
pub(crate) use recto_app::menus::{
    Action, Enable, RecentItem, action, context_menu, enable_rule, enabled_by_reading, menu_bar_with, panel, plain_key, zoom_from_word, zoom_label,
};

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
    /// The item whose submenu this is in, by its id, such as `file.open_recent`. Empty for an item of the menu itself.
    pub parent: String,
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
    /// Save As's sheet, with what `AppCore::save_panel` says.
    ShowSavePanel,
    /// Closing a document with unsaved edits asks first, with what `AppCore::close_prompt` says.
    ConfirmClose,
    /// The system's print panel, with what `AppCore::print_model` says.
    ShowPrintPanel,
    /// ⌘K: the palette's field in the sidebar takes the keyboard. Rust has opened it.
    FocusPalette,
    /// ⌘F: Find's field in the sidebar takes the keyboard, with what's in it selected. Rust has opened it.
    FocusFind,
    /// Go > Go to Page…: the field the page number opens, with what `AppCore::page_field` says.
    ShowGoTo,
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

impl From<app::MenuEntry> for MenuEntry {
    fn from(entry: app::MenuEntry) -> Self {
        Self {
            id: entry.id,
            title: entry.title,
            key: entry.key,
            with_control: entry.with_control,
            checked: entry.checked,
            hidden: entry.hidden,
            system_action: entry.system_action,
            parent: entry.parent,
        }
    }
}

impl From<app::MenuModel> for MenuModel {
    fn from(menu: app::MenuModel) -> Self {
        Self {
            title: menu.title,
            entries: menu.entries.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<app::HostAction> for HostAction {
    fn from(action: app::HostAction) -> Self {
        match action {
            app::HostAction::None => Self::None,
            app::HostAction::ShowOpenPanel => Self::ShowOpenPanel,
            app::HostAction::ShowAbout => Self::ShowAbout,
            app::HostAction::ShowSettings => Self::ShowSettings,
            app::HostAction::Quit => Self::Quit,
            app::HostAction::CloseWindow => Self::CloseWindow,
            app::HostAction::ShowSavePanel => Self::ShowSavePanel,
            app::HostAction::ConfirmClose => Self::ConfirmClose,
            app::HostAction::ShowPrintPanel => Self::ShowPrintPanel,
            app::HostAction::FocusPalette => Self::FocusPalette,
            app::HostAction::FocusFind => Self::FocusFind,
            app::HostAction::ShowGoTo => Self::ShowGoTo,
        }
    }
}

impl From<app::ZoomLabel> for ZoomLabel {
    fn from(label: app::ZoomLabel) -> Self {
        Self {
            text: label.text,
            editing_text: label.editing_text,
            tooltip: label.tooltip,
            reset_title: label.reset_title,
        }
    }
}

impl From<app::PanelItem> for PanelItem {
    fn from(item: app::PanelItem) -> Self {
        Self {
            id: item.id,
            title: item.title,
            selected: item.selected,
            enabled: item.enabled,
        }
    }
}

impl From<app::PanelSection> for PanelSection {
    fn from(section: app::PanelSection) -> Self {
        Self {
            title: section.title,
            items: section.items.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<app::PanelModel> for PanelModel {
    fn from(panel: app::PanelModel) -> Self {
        Self {
            open: panel.open,
            button_tooltip: panel.button_tooltip,
            sections: panel.sections.into_iter().map(Into::into).collect(),
        }
    }
}
