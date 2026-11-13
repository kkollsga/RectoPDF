//! **Every control the window has is a command, or the host's alone, said why** (step R2.3, C6; owner, 2026-11-12: "all features should be implemented
//! once"). Every menu item of the menu bar in every state it can be drawn in, the snapshot tool's menu, the context menu, the panel with the snapshot tool's
//! part, the palette's rows with a tab's and a recent file's, the keys that work with no ⌘, and every kind of [`UiAction`]: each, done on an app of its own,
//! sends the person's command to the one executor, or is in [`HOST_ONLY`] with the reason. A command a person sends is a line an agent could send: it reads
//! back from its own line as itself.

use crate::RowKind;
use crate::core::controls::UiAction;
use crate::menus::{self, RecentItem};
use crate::saving::CloseChoice;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_session::{Paper, ViewSettings, record::Spread};
use std::collections::BTreeSet;

/// **What the window does itself, with no command,** and why: drawing, a system panel or sheet, the keyboard's focus, or the window's own pointer mode.
/// What's chosen in a panel or sheet comes back as a control of its own, which is a command.
const HOST_ONLY: &[(&str, &str)] = &[
    ("app.about", "the About window is the system's"),
    (
        "app.quit",
        "quitting is the app's lifecycle: it asks about unsaved edits, then the hand-off of the state",
    ),
    ("app.services", "the system fills the Services menu"),
    ("app.settings", "opens the Settings window; what's chosen there is `SettingChosen`"),
    (
        "edit.copy",
        "the selection is the page view's, on the UI thread, and goes to the system's pasteboard",
    ),
    ("edit.copy_anyway", "the same, after the person has said to copy a protected document's text"),
    ("edit.select_all", "the selection is the page view's"),
    ("file.close_window", "closes the window, which is the system's"),
    ("file.open", "opens the system's Open panel; what's chosen there is `Open`"),
    ("file.open_recent", "a submenu's own item only opens it"),
    ("file.print", "opens the system's print panel, which is the host's"),
    ("file.save_as", "opens the Save As sheet; its answer is `SaveAs`"),
    ("file.settings", "opens the Settings window, as `app.settings`"),
    ("go.page", "opens the go-to field; what's typed there is `PageTyped`"),
    ("palette.open", "the palette's field takes the keyboard"),
    ("panel.toggle", "the tools panel open or shut is the window's own, and nothing of a document's"),
    ("tools.snapshot", "the snapshot tool is what the pointer does in the window"),
    ("view.paper", "a submenu's own item only opens it"),
    ("UiAction::CloseAnswered(Cancel)", "the close prompt goes, and nothing else happens"),
    (
        "UiAction::CloseAnswered(SaveAs)",
        "opens the Save As sheet for a document Save is off for; its answer is `SaveAs`",
    ),
];

/// A key to test: a menu id, run as the menu runs it, or a palette row, as the palette runs it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Control {
    Menu(String),
    Palette(String),
}

/// What a fake Open Recent key in the grid stands for: the app's own recent file's.
const ANY_RECENT: &str = "0123456789abcdef";

/// The ids of the menu bar in every state it's drawn in: each paper, the sidebar and the agent icon shown or hidden, each layout or none, the update item's
/// two titles, Open Recent empty and not; the snapshot tool's menu off and on; the context menu with each layout, in a comparison or not; the panel open,
/// in edit mode, rotating or not, with the snapshot tool's part; and the keys with no ⌘.
fn menu_ids() -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut add = |entries: Vec<menus::MenuEntry>| ids.extend(entries.into_iter().map(|entry| entry.id).filter(|id| !id.is_empty()));
    let recent = [RecentItem { key: ANY_RECENT, name: "c" }];
    let layouts = std::iter::once(None).chain(Spread::ALL.iter().copied().map(Some));
    for paper in Paper::ALL {
        for (sidebar_hidden, agent_icon_hidden) in [(false, false), (true, false), (false, true), (true, true)] {
            for shown in layouts.clone() {
                for title in ["Check for Updates…", "Update to 9.9.9…"] {
                    for recent in [&[][..], &recent[..]] {
                        let view = ViewSettings {
                            paper: Some(paper),
                            sidebar_hidden,
                            agent_icon_hidden,
                        };
                        for menu in menus::menu_bar_with(view, shown, title, recent) {
                            add(menu.entries);
                        }
                    }
                }
            }
        }
    }
    for on in [false, true] {
        add(crate::snapshot_menu::tools_menu(on).entries);
    }
    for shown in layouts {
        for comparing in [None, Some(false), Some(true)] {
            add(menus::context_menu(shown, comparing));
        }
    }
    for (open, edit_mode, can_rotate, snapshot_on) in (0..16).map(|bits| (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0)) {
        let mut panel = menus::panel(open, edit_mode, can_rotate);
        crate::snapshot_menu::add_to_panel(&mut panel, snapshot_on, edit_mode);
        let items = panel.sections.into_iter().flat_map(|section| section.items);
        ids.extend(items.map(|item| item.id));
    }
    for key in ["+", "=", "-", "a", "0", " "] {
        ids.extend(menus::plain_key(key).map(str::to_string));
    }
    // The window's own buttons (the tools panel's, Save's beside the window's controls, the sidebar's) and a notice's action, each naming a menu id.
    ids.extend(["panel.toggle", "save.button", "view.sidebar", "edit.copy_anyway"].map(str::to_string));
    ids
}

/// An app with two files open, a (with contents) and b, a third opened and closed so Open Recent and the palette have a file that isn't open, and a's in front.
fn app() -> HeadlessApp {
    let disk = Disk::new();
    // a has contents, so a section's chevron folds something.
    let (a, b, c) = (disk.file("a.pdf", &super::files::with_sections(4)), disk.pdf("b.pdf", 4), disk.pdf("c.pdf", 2));
    let app = disk.start();
    app.open(&c);
    app.agent(Level::Full, r#"{"cmd":"close"}"#).expect("c closed");
    app.open(&b);
    app.open(&a);
    app
}

/// The key of the file opened lately that isn't open.
fn recent_key(app: &HeadlessApp) -> String {
    app.core().recent_files().into_iter().find(|file| !file.open).expect("a recent file").key
}

/// What a control needs first to do anything: an edit to save, or hits to choose from.
fn prepare(app: &HeadlessApp, name: &str) {
    let name = name.strip_prefix("palette ").unwrap_or(name);
    if matches!(name, "file.save" | "save.button" | "UiAction::CloseAnswered(Save)" | "UiAction::SaveAs") {
        app.agent(Level::Full, r#"{"cmd":"page_rotate","pages":"1","degrees":90}"#).expect("an edit");
        app.settle();
    }
    // A hit's row, which needs hits.
    if name == "UiAction::FindChoose" {
        app.agent(Level::Full, r#"{"cmd":"find","query":"page"}"#).expect("found");
        app.settle();
    }
}

/// **Each kind of control, by name,** with no default, so a new kind doesn't compile here until it's said what it is, and one of each made on `app`.
fn every_kind(app: &HeadlessApp) -> Vec<(String, UiAction)> {
    let rows = app.tabs();
    let row = rows[0].clone();
    let tab = row.tab;
    let actions = vec![
        UiAction::Menu { id: "view.sidebar".into() },
        UiAction::Row {
            row: row.clone(),
            adding: false,
        },
        UiAction::Row {
            row: crate::SidebarRow {
                kind: RowKind::Contents,
                page: Some(2),
                ..row.clone()
            },
            adding: false,
        },
        UiAction::Row {
            row: rows[1].clone(),
            adding: true,
        },
        UiAction::PaneRead { tab },
        UiAction::Chevron { first: 0 },
        UiAction::Open {
            path: app.disk().files().join("c.pdf"),
        },
        UiAction::OpenRecent { key: recent_key(app) },
        UiAction::FindTyped { text: "page".into() },
        UiAction::FindOpen,
        UiAction::FindClose,
        UiAction::FindReturn { forward: true },
        // Row 0 is the first page's, and row 1 its hit.
        UiAction::FindChoose { row: 1 },
        UiAction::PageTyped { text: "2".into() },
        UiAction::Swiped { delta_x: 1.0 },
        UiAction::ZoomTyped {
            text: "150".into(),
            decimal: '.',
        },
        UiAction::ToggleSidebar,
        UiAction::PaletteRun { id: format!("tab:{tab}") },
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::Cancel,
        },
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::Save,
        },
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::DontSave,
        },
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::SaveAs,
        },
        UiAction::SaveAs {
            tab,
            path: app.disk().files().join("copy.pdf").display().to_string(),
        },
        UiAction::SettingChosen {
            id: crate::core::updates::AUTO_UPDATE_ROW.into(),
            on: false,
        },
    ];
    actions.into_iter().map(|action| (kind(&action), action)).collect()
}

/// A control's name: its kind, and what tells its forms apart.
fn kind(action: &UiAction) -> String {
    let name = match action {
        UiAction::Menu { .. } => "Menu",
        UiAction::Row { adding: false, .. } => "Row",
        UiAction::Row { adding: true, .. } => "Row(adding)",
        UiAction::PaneRead { .. } => "PaneRead",
        UiAction::Chevron { .. } => "Chevron",
        UiAction::Open { .. } => "Open",
        UiAction::OpenRecent { .. } => "OpenRecent",
        UiAction::FindTyped { .. } => "FindTyped",
        UiAction::FindOpen => "FindOpen",
        UiAction::FindClose => "FindClose",
        UiAction::FindReturn { .. } => "FindReturn",
        UiAction::FindChoose { .. } => "FindChoose",
        UiAction::PageTyped { .. } => "PageTyped",
        UiAction::Swiped { .. } => "Swiped",
        UiAction::ZoomTyped { .. } => "ZoomTyped",
        UiAction::ToggleSidebar => "ToggleSidebar",
        UiAction::PaletteRun { .. } => "PaletteRun",
        UiAction::CloseAnswered { choice, .. } => match choice {
            CloseChoice::Cancel => "CloseAnswered(Cancel)",
            CloseChoice::SaveAs => "CloseAnswered(SaveAs)",
            CloseChoice::Save => "CloseAnswered(Save)",
            CloseChoice::DontSave => "CloseAnswered(DontSave)",
        },
        UiAction::SaveAs { .. } => "SaveAs",
        UiAction::SettingChosen { .. } => "SettingChosen",
    };
    format!("UiAction::{name}")
}

/// What one control sent on an app of its own, settled: the person's commands, each checked to read back from its own line as itself.
fn sent_by(name: &str, make: impl FnOnce(&HeadlessApp) -> UiAction) -> Vec<String> {
    let app = app();
    prepare(&app, name);
    let action = make(&app);
    app.keep_sent();
    app.act(action);
    app.settle();
    app.sent()
        .into_iter()
        .map(|command| {
            let wire = command.wire();
            let again = recto_agent::parse(&wire).unwrap_or_else(|error| panic!("{name}: {wire} isn't a line an agent can send: {error:?}"));
            assert_eq!(format!("{again:?}"), format!("{command:?}"), "{name}: {wire} reads back as another command");
            wire
        })
        .collect()
}

/// Every control's name and what it sent.
fn every_control() -> Vec<(String, Vec<String>)> {
    let mut controls: BTreeSet<Control> = menu_ids().into_iter().map(Control::Menu).collect();
    let probe = app();
    let rows = crate::palette::items(probe.core().bar_now(), |_| true, false, true)
        .into_iter()
        .chain(crate::palette::items(probe.core().bar_now(), |_| true, true, true))
        .map(|row| row.id)
        .chain([format!("tab:{}", probe.tabs()[0].tab), format!("recent:{}", recent_key(&probe))]);
    controls.extend(rows.map(Control::Palette));
    let mut out: Vec<(String, Vec<String>)> = controls
        .into_iter()
        .map(|control| match control {
            Control::Menu(id) => {
                let name = id.clone();
                let sent = sent_by(&name, |app| UiAction::Menu {
                    id: id.replace(&format!("recent.{ANY_RECENT}"), &format!("recent.{}", recent_key(app))),
                });
                (name, sent)
            }
            Control::Palette(id) => {
                let name = format!("palette {id}");
                let sent = sent_by(&name, |app| UiAction::PaletteRun {
                    id: id.replace(&recent_key(&probe), &recent_key(app)),
                });
                (name, sent)
            }
        })
        .collect();
    let kinds = every_kind(&probe);
    for (at, (name, _)) in kinds.iter().enumerate() {
        let sent = sent_by(name, |app| every_kind(app).swap_remove(at).1);
        out.push((name.clone(), sent));
    }
    out
}

/// Failure (R2.3, C6): a control the window has does something no command does, with no reason given for it being the host's, so a person can do what an
/// agent can't (a second implementation, against the owner's rule); or a control said to be the host's sends a command after all, or is in no menu, panel,
/// palette or key any more, so the list says what isn't so; or a command the window sends isn't a line an agent could send. Every control, each on an app of
/// its own, as `every_control` lists them.
#[test]
fn every_control_is_a_command_or_the_hosts_with_a_reason() {
    let controls = every_control();
    let host_only = |name: &str| {
        let name = name.strip_prefix("palette ").unwrap_or(name);
        HOST_ONLY.iter().find(|(id, _)| *id == name).map(|(_, why)| *why)
    };
    let mut wrong = Vec::new();
    let mut seen = BTreeSet::new();
    for (name, sent) in &controls {
        let why = host_only(name);
        if why.is_some() {
            seen.insert(name.strip_prefix("palette ").unwrap_or(name).to_string());
        }
        match (sent.is_empty(), why) {
            (true, None) => wrong.push(format!("{name} sends no command and isn't the host's")),
            (false, Some(why)) => wrong.push(format!("{name} is said to be the host's ({why}) and sends {sent:?}")),
            _ => {}
        }
    }
    for (id, why) in HOST_ONLY {
        assert!(!why.is_empty(), "{id} needs its reason");
        if !seen.contains(*id) {
            wrong.push(format!("{id} is in HOST_ONLY and no control is it"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(controls.len() > 60, "every control: {}", controls.len());
}

/// Failure (R2.3, C6): the bridge reaches past the one way a person's action comes in (`Inner::act` and the methods it calls), queuing a person's call on
/// the session thread itself: a second way in, which the coverage test above can't see. The bridge's sources name no `CoreCall` but `Reconcile`, which asks
/// the window to be drawn again and does nothing a person does.
#[test]
fn the_bridge_queues_none_of_a_persons_calls() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../recto-ffi/src");
    let mut files = vec![root];
    let mut named = Vec::new();
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(
                std::fs::read_dir(&path)
                    .expect("the bridge's sources")
                    .map(|entry| entry.expect("an entry").path()),
            );
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("a source");
            for (at, _) in text.match_indices("CoreCall::") {
                let call: String = text[at + "CoreCall::".len()..].chars().take_while(|c| c.is_alphanumeric()).collect();
                if call != "Reconcile" {
                    named.push(format!("{}: CoreCall::{call}", path.display()));
                }
            }
        }
    }
    assert!(named.is_empty(), "{named:#?}");
}
