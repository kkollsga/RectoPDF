//! **⌘K's palette, as the window and `recto palette` reach it:** the rows it finds from what's published, and what running one does. The ranking, the words and
//! the routes are `crate::palette`'s, the records the bridge's.

use super::{HostAction, Inner, RecentFile, lock};
use crate::palette as app;
use crate::palette::{ForWhom, PaletteView};
use recto_session::TabId;
use std::sync::Arc;

/// The open tabs, as rows.
fn tab_rows(inner: &Arc<Inner>) -> Vec<app::PaletteRow> {
    inner
        .sidebar_tabs()
        .into_iter()
        .map(|tab| app::tab_row(tab.tab, tab.text, tab.current))
        .collect()
}

/// Files opened lately that aren't open now, newest first, as rows: `recent:<key>`, by Open Recent's key.
fn recent_rows(inner: &Arc<Inner>) -> Vec<app::PaletteRow> {
    inner
        .recent_files()
        .into_iter()
        .filter(|file| !file.open)
        .map(|file| app::recent_row(&file.key, file.name))
        .collect()
}

/// The commands that can be chosen now, menu by menu, then the panel's. A command that can't be chosen now isn't listed, as there's nothing it would do.
fn items(inner: &Arc<Inner>) -> Vec<app::PaletteRow> {
    let bar = inner.bar_now();
    let enabled = |id: &str| inner.menu_item_enabled(id.to_string());
    let (edit_mode, can_rotate) = {
        let published = lock(&inner.published);
        (published.reader.edit_mode, published.reader.can_rotate)
    };
    app::items(bar, enabled, edit_mode, can_rotate)
}

/// The rows a query finds, best first: the tabs, the files opened lately and the commands ranked together, in that order among equals, so an empty query lists the tabs, then the recent files, then the commands. The tabs' ranking is kept for the query and the tab list's version.
pub fn found_for(inner: &Arc<Inner>, query: &str, whom: &ForWhom) -> Vec<app::PaletteRow> {
    let version = lock(&inner.published).sidebar.tabs_version;
    app::found_for(
        &inner.tools.palette,
        query,
        version,
        || tab_rows(inner),
        || items(inner),
        || recent_rows(inner),
        whom,
    )
}

/// The agent's command a tab's or a recent file's row comes to, for `recto palette --run`. A menu id's is [`super::menu_rows`]'s.
pub fn as_command(id: &str, recent: &[RecentFile]) -> Option<recto_agent::Command> {
    let recent: Vec<(&str, &str)> = recent.iter().map(|file| (file.key.as_str(), file.path.as_str())).collect();
    app::as_command(id, &recent)
}

/// **The palette's menu id** ([`super::menu_rows`]): ⌘K opens it, in the sidebar, so the sidebar's shown first, and its field takes the keyboard.
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    if asked.action != Some(crate::menus::Action::PaletteOpen) {
        return None;
    }
    Some(super::menu_rows::Row::window(
        "the palette's field takes the person's keyboard",
        super::menu_rows::Agents::Never,
        move || {
            if lock(&inner.published).reader.sidebar_hidden {
                inner.show_sidebar();
            }
            inner.tools.palette.open();
            inner.listener.model_changed();
            HostAction::FocusPalette
        },
    ))
}

/// What the host draws now.
pub fn view(inner: &Arc<Inner>) -> PaletteView {
    inner.tools.palette.view(|text| found_for(inner, text, &ForWhom::person()))
}

/// ↑ and ↓: the selection moves, stopping at either end.
pub fn step(inner: &Arc<Inner>, by: i32) {
    let count = inner.tools.palette.view(|text| found_for(inner, text, &ForWhom::person())).rows.len();
    inner.tools.palette.step(count, by);
}

/// Runs a row, closing the palette first, and says what's left for the host.
pub fn run(inner: &Arc<Inner>, id: &str) -> HostAction {
    inner.tools.palette.close();
    let frozen = inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst);
    match app::route(id, super::find::is_open(inner), frozen) {
        app::Run::Tap(tab) => {
            inner.tap_tab(TabId(tab));
            HostAction::None
        }
        app::Run::OpenRecent(key) => {
            inner.open_recent(key);
            HostAction::None
        }
        app::Run::Perform { focus_find_if_nothing } => match inner.perform(id.to_string()) {
            HostAction::None if focus_find_if_nothing => HostAction::FocusFind,
            action => action,
        },
    }
}

/// What the host was left to do, by name, for `recto palette --run`.
pub fn host_word(action: HostAction) -> &'static str {
    app::host_word(action)
}

/// What's in the palette's field now, as it's typed.
pub fn typed(inner: &Inner, text: &str) {
    inner.tools.palette.typed(text);
}

/// Escape, or the keyboard going elsewhere.
pub fn close(inner: &Inner) {
    inner.tools.palette.close();
    inner.listener.model_changed();
}
