//! **⌘K, the command palette: its records and the calls.** What it lists, how it's worded and ranked, and what a row runs are `recto_app::palette`'s; this gives it the window's lists
//! and does what a row comes to.

use crate::{AppCore, CoreCall, HostAction, ReaderView, RecentFile, lock};
use recto_app::palette as app;
pub(crate) use recto_app::palette::ForWhom;
use recto_session::TabId;

/// One of the palette's rows.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaletteRow {
    /// What running it does, by Rust's own name: a menu item's id, or `tab:N`.
    pub id: String,
    pub title: String,
    /// Where it lives, as `View` or `Open tab`, in a quieter colour.
    pub detail: String,
    /// Its key, as `⇧⌘G`, or empty.
    pub shortcut: String,
    /// Where it lives and its key together, as the row shows them on its right.
    pub aside: String,
    /// The title comes from a file, as a tab's name does, so it's only ever drawn as text.
    pub from_file: bool,
    /// What VoiceOver says for the row: its title, where it lives, and its key in words, as "Find Next, Edit, Command G".
    pub spoken: String,
}

/// Everything the palette draws.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PaletteView {
    pub open: bool,
    pub text: String,
    pub placeholder: String,
    pub rows: Vec<PaletteRow>,
    pub selected: Option<u32>,
    /// What's said with no rows.
    pub empty: String,
    /// What VoiceOver calls the list of rows.
    pub list_label: String,
}

impl From<app::PaletteRow> for PaletteRow {
    fn from(row: app::PaletteRow) -> Self {
        Self {
            id: row.id,
            title: row.title,
            detail: row.detail,
            shortcut: row.shortcut,
            aside: row.aside,
            from_file: row.from_file,
            spoken: row.spoken,
        }
    }
}

impl From<app::PaletteView> for PaletteView {
    fn from(view: app::PaletteView) -> Self {
        Self {
            open: view.open,
            text: view.text,
            placeholder: view.placeholder,
            rows: view.rows.into_iter().map(Into::into).collect(),
            selected: view.selected,
            empty: view.empty,
            list_label: view.list_label,
        }
    }
}

/// The open tabs, as rows.
fn tab_rows(core: &AppCore) -> Vec<app::PaletteRow> {
    core.sidebar_tabs()
        .into_iter()
        .map(|tab| app::tab_row(tab.tab, tab.text, tab.current))
        .collect()
}

/// Files opened lately that aren't open now, newest first, as rows: `recent:<key>`, by Open Recent's key.
fn recent_rows(core: &AppCore) -> Vec<app::PaletteRow> {
    core.recent_files()
        .into_iter()
        .filter(|file| !file.open)
        .map(|file| app::recent_row(&file.key, file.name))
        .collect()
}

/// The commands that can be chosen now, menu by menu, then the panel's. A command that can't be chosen now isn't listed, as there's nothing it would do.
fn items(core: &AppCore) -> Vec<app::PaletteRow> {
    let bar = crate::menus::bar_now(core);
    let enabled = |id: &str| core.menu_item_enabled(id.to_string());
    let (edit_mode, can_rotate) = {
        let published = lock(&core.inner.published);
        (published.reader.edit_mode, published.reader.can_rotate)
    };
    app::items(bar, enabled, edit_mode, can_rotate)
}

/// The rows a query finds, best first: the tabs, the files opened lately and the commands ranked together, in that order among equals, so an empty query lists the tabs, then the recent files, then the commands. The tabs' ranking is kept for the query and the tab list's version.
pub(crate) fn found_for(core: &AppCore, query: &str, whom: &ForWhom) -> Vec<app::PaletteRow> {
    let version = lock(&core.inner.published).sidebar.tabs_version;
    app::found_for(
        &core.inner.palette,
        query,
        version,
        || tab_rows(core),
        || items(core),
        || recent_rows(core),
        whom,
    )
}

/// The agent's command a palette row comes to, for `recto palette --run`, so it's run at the agent's level with its own checks and as the agent's step, as the command itself would be. `None` for what has no such command: that needs Full, and is done as the menu item does.
pub(crate) fn as_command(id: &str, reader: &ReaderView, recent: &[RecentFile]) -> Option<recto_agent::Command> {
    let showing = app::Showing {
        sidebar_hidden: reader.sidebar_hidden,
        agent_icon_hidden: reader.agent_icon_hidden,
        edit_mode: reader.edit_mode,
    };
    let recent: Vec<(&str, &str)> = recent.iter().map(|file| (file.key.as_str(), file.path.as_str())).collect();
    app::as_command(id, showing, &recent)
}

/// What the host draws now.
pub(crate) fn view(core: &AppCore) -> PaletteView {
    core.inner.palette.view(|text| found_for(core, text, &ForWhom::person())).into()
}

/// ↑ and ↓: the selection moves, stopping at either end.
pub(crate) fn step(core: &AppCore, by: i32) {
    let count = core.inner.palette.view(|text| found_for(core, text, &ForWhom::person())).rows.len();
    core.inner.palette.step(count, by);
}

/// Runs a row, closing the palette first, and says what's left for the host.
pub(crate) fn run(core: &AppCore, id: &str) -> HostAction {
    core.inner.palette.close();
    let frozen = core.inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst);
    match app::route(id, crate::find::is_open(&core.inner), frozen) {
        app::Run::Tap(tab) => {
            core.inner.core.push(CoreCall::Tap(TabId(tab)));
            HostAction::None
        }
        app::Run::OpenRecent(key) => {
            core.open_recent(key);
            HostAction::None
        }
        app::Run::Perform { focus_find_if_nothing } => match core.perform(id.to_string()) {
            HostAction::None if focus_find_if_nothing => HostAction::FocusFind,
            action => action,
        },
    }
}

/// What the host was left to do, by name, for `recto palette --run`.
pub(crate) fn host_word(action: HostAction) -> &'static str {
    app::host_word(action.into())
}
