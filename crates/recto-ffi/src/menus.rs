//! The menu bar's and the panel's records, as the host draws them. The lists, what each item does and when it's enabled are `recto_app::menus`'s.

use crate::lock;
use recto_app::menus as app;
pub(crate) use recto_app::menus::{
    Action, Enable, RecentItem, action, context_menu, enable_rule, enabled_by_reading, menu_bar_with, panel, plain_key, zoom_from_word, zoom_label,
};
pub use recto_app::menus::{HostAction, MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection, ZoomLabel};
use recto_session::{Paper, ViewSettings, record};
use std::sync::Arc;

/// One item of a menu. An empty `id` is a line between items.
#[uniffi::remote(Record)]
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

#[uniffi::remote(Record)]
pub struct MenuModel {
    pub title: String,
    pub entries: Vec<MenuEntry>,
}

/// What the host has to do itself, after an item is chosen. Everything else is done by then.
#[uniffi::remote(Enum)]
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
#[uniffi::remote(Record)]
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
#[uniffi::remote(Record)]
pub struct PanelItem {
    pub id: String,
    pub title: String,
    pub selected: bool,
    /// Greyed out when it can't do anything now.
    pub enabled: bool,
}

#[uniffi::remote(Record)]
pub struct PanelSection {
    pub title: String,
    pub items: Vec<PanelItem>,
}

#[uniffi::remote(Record)]
pub struct PanelModel {
    pub open: bool,
    pub button_tooltip: String,
    pub sections: Vec<PanelSection>,
}

/// The menu bar as it stands, as recto-app's lists: what `AppCore::menu_bar` converts, and the palette lists.
pub(crate) fn bar_now(core: &crate::AppCore) -> Vec<app::MenuModel> {
    let reader = lock(&core.inner.published).reader.clone();
    let recent = Arc::clone(&lock(&core.inner.published).recent);
    let items: Vec<RecentItem> = recent
        .iter()
        .map(|file| RecentItem {
            key: &file.key,
            name: &file.name,
        })
        .collect();
    let mut bar = menu_bar_with(
        ViewSettings {
            paper: Paper::from_name(&reader.paper),
            sidebar_hidden: reader.sidebar_hidden,
            agent_icon_hidden: reader.agent_icon_hidden,
        },
        record::Spread::from_name(&reader.layout),
        &core.inner.updates.menu_item().title,
        &items,
    );
    bar.push(recto_app::snapshot_menu::tools_menu(core.inner.tools.tool.is_on()));
    bar
}
