//! **⌘K, the command palette**: every command the menu bar and the panel have, and every open tab, found by a few letters (`recto_session::palette` ranks them) and run with Return. Rust lists the items, words them, ranks them and runs them; the host draws the field and the list, and says what was typed and pressed.
//!
//! **Where it goes** (owner, 2026-11-06: "We need to maximize vertical reading space", nothing above or over a page): in the sidebar, in place of its tabs and contents while it's open, and gone with Escape, a run, or a click elsewhere. Fully keyboard-driven: typing, ↑ and ↓, Return, Escape.
//!
//! Files opened lately are listed after the tabs, by the key File > Open Recent goes by, never an index, so a row opens the file it showed however the list has moved. One already open in a tab is that tab's row.
//!
//! **What an agent sees** (`recto palette`): never a tab opened with a password the person typed, whose name may have been read with it, and files opened lately only at the level `recto recent` needs.

use crate::menus::{self, HostAction, MenuEntry};
use crate::{AppCore, CoreCall, Inner, lock};
use recto_session::TabId;
use recto_session::palette;

/// What the palette is doing, on the UI thread under a short lock.
#[derive(Debug, Default)]
pub(crate) struct State {
    open: bool,
    text: String,
    /// The row Return runs.
    selected: usize,
    /// The tabs ranked for a query, at a version of the tab list, so a query's tabs are ranked once however often the palette's drawn: there can be 10,000 of them.
    tabs_ranked: Option<(String, u64, Ranked)>,
}

/// Rows with their scores, best first.
type Ranked = Vec<(PaletteRow, i32)>;

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

fn row(id: String, title: String, detail: &str, shortcut: String, from_file: bool) -> PaletteRow {
    let aside = if shortcut.is_empty() {
        detail.to_string()
    } else {
        format!("{detail}  {shortcut}")
    };
    let spoken = if shortcut.is_empty() {
        format!("{title}, {detail}")
    } else {
        format!("{title}, {detail}, {}", spoken_key(&shortcut))
    };
    PaletteRow {
        id,
        title,
        detail: detail.to_string(),
        shortcut,
        aside,
        from_file,
        spoken,
    }
}

/// A key as it's said, not drawn: `⇧⌘G` is "Shift Command G", in the order a Mac draws its marks.
fn spoken_key(shortcut: &str) -> String {
    shortcut
        .chars()
        .map(|mark| match mark {
            '⌃' => "Control".to_string(),
            '⌥' => "Option".to_string(),
            '⇧' => "Shift".to_string(),
            '⌘' => "Command".to_string(),
            '+' => "Plus".to_string(),
            '-' => "Minus".to_string(),
            '=' => "Equals".to_string(),
            '[' => "Left Bracket".to_string(),
            ']' => "Right Bracket".to_string(),
            other => other.to_uppercase().collect(),
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// A menu item's key as a Mac shows it.
pub(crate) fn shortcut(entry: &MenuEntry) -> String {
    if entry.key.is_empty() {
        return String::new();
    }
    let shift = entry.key.chars().next().is_some_and(char::is_uppercase);
    format!(
        "{}{}⌘{}",
        if entry.with_control { "⌃" } else { "" },
        if shift { "⇧" } else { "" },
        entry.key.to_uppercase()
    )
}

/// The open tabs, as rows.
fn tab_rows(core: &AppCore) -> Vec<PaletteRow> {
    core.sidebar_tabs()
        .into_iter()
        .map(|tab| {
            row(
                format!("tab:{}", tab.tab),
                tab.text,
                if tab.current { "Showing" } else { "Open tab" },
                String::new(),
                true,
            )
        })
        .collect()
}

/// Files opened lately that aren't open now, newest first, as rows: `recent:<key>`, by Open Recent's key.
fn recent_rows(core: &AppCore) -> Vec<PaletteRow> {
    core.recent_files()
        .into_iter()
        .filter(|file| !file.open)
        .map(|file| row(format!("{RECENT}{}", file.key), file.name, "Recent", String::new(), true))
        .collect()
}

/// The id a recent file's row has, before its key.
const RECENT: &str = "recent:";

/// Who the rows are for. The person sees everything; an agent sees less: see the module's notes.
pub(crate) struct ForWhom {
    /// Tabs left out, as tabs opened with a password are for an agent.
    pub hidden_tabs: Vec<u32>,
    /// Whether files opened lately are listed.
    pub recent: bool,
}

impl ForWhom {
    pub(crate) fn person() -> Self {
        Self {
            hidden_tabs: Vec::new(),
            recent: true,
        }
    }
}

/// The commands that can be chosen now, menu by menu, then the panel's. A command that can't be chosen now isn't listed, as there's nothing it would do.
pub(crate) fn items(core: &AppCore) -> Vec<PaletteRow> {
    let mut rows: Vec<PaletteRow> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for menu in core.menu_bar() {
        let titles: std::collections::HashMap<String, String> = menu.entries.iter().map(|entry| (entry.id.clone(), entry.title.clone())).collect();
        for entry in menu.entries {
            // A line, a second key for an item, what the system fills, and the palette itself.
            // One row a command: Settings is in two menus, as on any Mac, and does one thing.
            // An Open Recent file isn't a command: a file named "Close" mustn't stand for Close. Open Recent's files are rows of their own. A submenu's own item only opens it.
            if entry.id.is_empty()
                || entry.hidden
                || entry.parent == "file.open_recent"
                || matches!(entry.id.as_str(), "app.services" | "palette.open" | "file.open_recent" | "view.paper")
                || !seen.insert(entry.id.clone())
                || !core.menu_item_enabled(entry.id.clone())
            {
                continue;
            }
            // A choice in a submenu, such as View > Paper's, is named with it, to stand alone: "Warm Paper".
            let title = match titles.get(&entry.parent) {
                Some(submenu) => format!("{} {submenu}", entry.title),
                None => entry.title.trim_end_matches('…').to_string(),
            };
            if !seen.insert(format!("title:{title}")) {
                continue;
            }
            let key = shortcut(&entry);
            rows.push(row(entry.id, title, &menu.title, key, false));
        }
    }
    // The panel's tools, worded to stand alone. Rotating needs a document.
    let has_document = core.menu_item_enabled("edit.select_all".into());
    let reader = lock(&core.inner.published).reader.clone();
    for section in menus::panel(true, reader.edit_mode, reader.can_rotate).sections {
        for item in section.items {
            let title = match item.id.as_str() {
                "rotate.left" | "rotate.right" if !has_document || !item.enabled => continue,
                "rotate.left" if reader.edit_mode => "Rotate This Page Left".to_string(),
                "rotate.right" if reader.edit_mode => "Rotate This Page Right".to_string(),
                "rotate.left" => "Rotate All Pages Left".to_string(),
                "rotate.right" => "Rotate All Pages Right".to_string(),
                "edit_mode.done" => "Leave Edit Mode".to_string(),
                _ => item.title.clone(),
            };
            if seen.insert(item.id.clone()) {
                rows.push(row(item.id, title, "Panel", String::new(), false));
            }
        }
    }
    rows
}

/// The rows a query finds for the person, best first.
pub(crate) fn found(core: &AppCore, query: &str) -> Vec<PaletteRow> {
    found_for(core, query, &ForWhom::person())
}

/// The rows a query finds, best first: the tabs, the files opened lately and the commands ranked together, in that order among equals, so an empty query lists the tabs, then the recent files, then the commands. The tabs' ranking is kept for the query and the tab list's version.
pub(crate) fn found_for(core: &AppCore, query: &str, whom: &ForWhom) -> Vec<PaletteRow> {
    let version = lock(&core.inner.published).sidebar.tabs_version;
    let cached = lock(&core.inner.palette)
        .tabs_ranked
        .as_ref()
        .filter(|(was, at, _)| was == query && *at == version)
        .map(|(_, _, rows)| rows.clone());
    let tabs = cached.unwrap_or_else(|| {
        let all = tab_rows(core);
        let ranked: Vec<(PaletteRow, i32)> = palette::rank(query, all.iter().map(|row| row.title.as_str()))
            .into_iter()
            .filter_map(|(at, score)| Some((all.get(at)?.clone(), score)))
            .collect();
        lock(&core.inner.palette).tabs_ranked = Some((query.to_string(), version, ranked.clone()));
        ranked
    });
    let commands = items(core);
    let mut both: Vec<(PaletteRow, i32)> = tabs
        .into_iter()
        .filter(|(row, _)| !whom.hidden_tabs.iter().any(|tab| row.id == format!("tab:{tab}")))
        .collect();
    if whom.recent {
        let recent = recent_rows(core);
        both.extend(
            palette::rank(query, recent.iter().map(|row| row.title.as_str()))
                .into_iter()
                .filter_map(|(at, score)| Some((recent.get(at)?.clone(), score))),
        );
    }
    both.extend(
        palette::rank(query, commands.iter().map(|row| row.title.as_str()))
            .into_iter()
            .filter_map(|(at, score)| Some((commands.get(at)?.clone(), score))),
    );
    // Stable, so equals keep tabs first and each list's own order.
    both.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    both.into_iter().take(palette::MAX_RESULTS).map(|(row, _)| row).collect()
}

/// The agent's command a palette row comes to, for `recto palette --run`, so it's run at the agent's level with its own checks and as the agent's step, as the command itself would be. `None` for what has no such command: that needs Full, and is done as the menu item does.
pub(crate) fn as_command(id: &str, reader: &crate::ReaderView, recent: &[crate::RecentFile]) -> Option<recto_agent::Command> {
    use recto_agent::Command;
    let active = || "active".to_string();
    let view = |paper: Option<&str>, sidebar: Option<&str>, agent_icon: Option<&str>, zoom: Option<&str>, layout: Option<&str>| Command::View {
        paper: paper.map(str::to_string),
        sidebar: sidebar.map(str::to_string),
        agent_icon: agent_icon.map(str::to_string),
        zoom: zoom.map(str::to_string),
        layout: layout.map(str::to_string),
    };
    Some(match id {
        "view.zoom_in" => view(None, None, None, Some("in"), None),
        "view.zoom_out" => view(None, None, None, Some("out"), None),
        "view.actual_size" => view(None, None, None, Some("actual"), None),
        "view.fit" => view(None, None, None, Some("fit"), None),
        "view.sidebar" => view(None, Some(if reader.sidebar_hidden { "on" } else { "off" }), None, None, None),
        "view.agent_icon" => view(None, None, Some(if reader.agent_icon_hidden { "on" } else { "off" }), None, None),
        "layout.single" => view(None, None, None, None, Some("single")),
        "layout.two_up" => view(None, None, None, None, Some("two-up")),
        "layout.two_up_cover" => view(None, None, None, None, Some("two-up-cover")),
        "paper.white" => view(Some("white"), None, None, None, None),
        "paper.warm" => view(Some("warm"), None, None, None, None),
        "paper.dark" => view(Some("dark"), None, None, None, None),
        "compare.separate" => Command::Compare {
            tabs: None,
            sync: Some("off".into()),
            off: false,
        },
        "compare.synced" => Command::Compare {
            tabs: None,
            sync: Some("on".into()),
            off: false,
        },
        "edit.undo" => Command::Undo { tab: active() },
        "edit.redo" => Command::Redo { tab: active() },
        "edit.clear_history" => Command::ClearHistory { tab: active() },
        "file.save" => Command::Save {
            tab: active(),
            out: None,
            replace: false,
        },
        "find.next" | "find.previous" => Command::Find {
            query: None,
            next: id == "find.next",
            previous: id == "find.previous",
            close: false,
            limit: None,
        },
        "rotate.left" if !reader.edit_mode => Command::PageRotate {
            tab: active(),
            pages: "all".into(),
            degrees: -90,
        },
        "rotate.right" if !reader.edit_mode => Command::PageRotate {
            tab: active(),
            pages: "all".into(),
            degrees: 90,
        },
        file if file.starts_with(RECENT) => Command::Open {
            path: recent.iter().find(|known| known.key == file[RECENT.len()..])?.path.clone().into(),
            page: None,
        },
        tab if tab.starts_with("tab:") => Command::Tap {
            tab: format!("#{}", tab.trim_start_matches("tab:")),
        },
        _ => return None,
    })
}

/// What the host draws now.
pub(crate) fn view(core: &AppCore) -> PaletteView {
    let state = lock(&core.inner.palette);
    let (open, text, selected) = (state.open, state.text.clone(), state.selected);
    drop(state);
    let rows = if open { found(core, &text) } else { Vec::new() };
    let selected = (!rows.is_empty()).then(|| u32::try_from(selected.min(rows.len() - 1)).unwrap_or(0));
    PaletteView {
        open,
        text,
        placeholder: "Type a command or a tab".into(),
        rows,
        selected,
        empty: "Nothing matches".into(),
        list_label: "Commands and documents".into(),
    }
}

pub(crate) fn open(inner: &Inner) {
    *lock(&inner.palette) = State {
        open: true,
        ..State::default()
    };
}

impl Inner {
    pub(crate) fn palette_open(&self) -> bool {
        lock(&self.palette).open
    }
}

pub(crate) fn close(inner: &Inner) {
    *lock(&inner.palette) = State::default();
}

pub(crate) fn typed(inner: &Inner, text: &str) {
    let mut state = lock(&inner.palette);
    state.text = text.chars().take(palette::MAX_QUERY_CHARS * 4).collect();
    state.selected = 0;
}

/// ↑ and ↓: the selection moves, stopping at either end.
pub(crate) fn step(core: &AppCore, by: i32) {
    let count = view(core).rows.len();
    let mut state = lock(&core.inner.palette);
    if count == 0 {
        state.selected = 0;
        return;
    }
    let at = i64::try_from(state.selected.min(count - 1)).unwrap_or(0) + i64::from(by);
    state.selected = usize::try_from(at.clamp(0, i64::try_from(count - 1).unwrap_or(0))).unwrap_or(0);
}

/// Runs a row, closing the palette first, and says what's left for the host.
pub(crate) fn run(core: &AppCore, id: &str) -> HostAction {
    close(&core.inner);
    if let Some(tab) = id.strip_prefix("tab:").and_then(|tab| tab.parse::<u32>().ok()) {
        core.inner.core.push(CoreCall::Tap(TabId(tab)));
        return HostAction::None;
    }
    if let Some(key) = id.strip_prefix(RECENT) {
        core.open_recent(key.to_string());
        return HostAction::None;
    }
    // Find Next or Previous with Find closed opens it, with what was typed last, and its field takes the keyboard, as ⌘F's would: the palette that had it has gone.
    let find_was_closed =
        matches!(id, "find.next" | "find.previous") && !crate::find::is_open(&core.inner) && !core.inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst);
    match core.perform(id.to_string()) {
        HostAction::None if find_was_closed => HostAction::FocusFind,
        action => action,
    }
}

/// What the host was left to do, by name, for `recto palette --run`.
pub(crate) fn host_word(action: HostAction) -> &'static str {
    match action {
        HostAction::None => "none",
        HostAction::ShowOpenPanel => "show_open_panel",
        HostAction::ShowAbout => "show_about",
        HostAction::ShowSettings => "show_settings",
        HostAction::Quit => "quit",
        HostAction::CloseWindow => "close_window",
        HostAction::ShowSavePanel => "show_save_panel",
        HostAction::ConfirmClose => "confirm_close",
        HostAction::FocusPalette => "focus_palette",
        HostAction::FocusFind => "focus_find",
        HostAction::ShowGoTo => "show_go_to",
        HostAction::ShowPrintPanel => "show_print_panel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a shortcut shown in the palette isn't the one the menu has, so the reader learns the wrong key.
    #[test]
    fn shortcuts_read_as_a_macs_do() {
        let entry = |key: &str, with_control: bool| MenuEntry {
            id: "x".into(),
            title: "x".into(),
            key: key.into(),
            with_control,
            checked: false,
            hidden: false,
            system_action: String::new(),
            parent: String::new(),
        };
        assert_eq!(shortcut(&entry("f", false)), "⌘F");
        assert_eq!(shortcut(&entry("G", false)), "⇧⌘G");
        assert_eq!(shortcut(&entry("s", true)), "⌃⌘S");
        assert_eq!(shortcut(&entry("+", false)), "⌘+");
        assert_eq!(shortcut(&entry("", false)), "");
    }

    /// Failure: VoiceOver reads a palette row as its marks, or without where it lives or its key, so a row can't be told from another with the same title, or a key isn't said as a person would press it.
    #[test]
    fn a_row_is_said_in_words() {
        assert_eq!(row("find.next".into(), "Find Next".into(), "Edit", "⌘G".into(), false).spoken, "Find Next, Edit, Command G");
        assert_eq!(spoken_key("⇧⌘G"), "Shift Command G");
        assert_eq!(spoken_key("⌃⌘S"), "Control Command S");
        assert_eq!(spoken_key("⌘+"), "Command Plus");
        assert_eq!(row("tab:2".into(), "report.pdf".into(), "Open tab", String::new(), true).spoken, "report.pdf, Open tab");
    }
}
