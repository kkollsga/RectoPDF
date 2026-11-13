//! **Every control the window has is a command, or the host's alone, said why** (step R2.3, C6; owner, 2026-11-12: "all features should be implemented
//! once"). Every menu item of the menu bar in every state it can be drawn in, the snapshot tool's menu, the context menu, the panel with the snapshot tool's
//! part, the palette's rows with a tab's and a recent file's, the keys that work with no ⌘, and every kind of [`UiAction`]: each, done on an app of its own,
//! sends the person's command to the one executor, or is the window's alone with the reason its row in the one table gives
//! (`core::menu_rows`), or [`HOST_ONLY`]'s for a control that's no menu id. A command a person sends is a line an agent could send: it reads
//! back from its own line as itself.

use crate::RowKind;
use crate::core::controls::UiAction;
use crate::menus::{self, RecentItem};
use crate::saving::CloseChoice;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_session::{Paper, ViewSettings, record::Spread};
use std::collections::BTreeSet;

/// **What the window does itself with no command, and why, for the controls that are no menu id:** a menu id's reason is its row's in the one table
/// (`core::menu_rows`). What's chosen in a panel or sheet comes back as a control of its own, which is a command.
const HOST_ONLY: &[(&str, &str)] = &[
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
pub(super) fn menu_ids() -> BTreeSet<String> {
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
            row: crate::SidebarRow {
                kind: RowKind::Contents,
                page: Some(3),
                ..row.clone()
            },
            adding: true,
        },
        UiAction::Row {
            row: rows[1].clone(),
            adding: true,
        },
        // The note over a tab's contents, b's, which has none of its own: its action is what a click does.
        UiAction::Row {
            row: crate::SidebarRow {
                kind: RowKind::Note,
                action: Some(recto_session::ContentsAction::Generate.id().into()),
                ..rows[1].clone()
            },
            adding: false,
        },
        UiAction::PaneRead { tab },
        UiAction::Chevron { first: 0 },
        UiAction::Open {
            path: app.disk().files().join("c.pdf").display().to_string(),
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
            decimal: ".".into(),
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
        UiAction::Row { row, adding } => match (row.kind, adding) {
            (RowKind::Tab, false) => "Row(tab)",
            (RowKind::Tab, true) => "Row(tab, adding)",
            (RowKind::Contents, false) => "Row(contents)",
            (RowKind::Contents, true) => "Row(contents, adding)",
            (RowKind::Note, _) => "Row(note)",
        },
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

/// What one control did on an app of its own, settled: the person's commands it sent, each checked to read back from its own line as itself; why it's the
/// window's alone, if it's said to be (a menu id's by its row in the one table, any other control's by [`HOST_ONLY`]); what it gave back to the host; and
/// the Listener's calls it made, by kind, in order.
struct Did {
    sent: Vec<String>,
    why: Option<&'static str>,
    acted: crate::core::controls::Acted,
    heard: Vec<&'static str>,
}

fn sent_by(name: &str, make: impl FnOnce(&HeadlessApp) -> UiAction) -> Did {
    let app = app();
    prepare(&app, name);
    let action = make(&app);
    let why = match &action {
        UiAction::Menu { id } | UiAction::PaletteRun { id } => crate::core::menu_rows::row(app.core(), id).and_then(|row| row.windows_alone()),
        _ => HOST_ONLY.iter().find(|(host, _)| *host == name).map(|(_, why)| *why),
    };
    app.settle();
    app.keep_sent();
    drop(app.listener().take());
    let acted = app.act(action);
    app.settle();
    let heard = app.listener().take().iter().map(heard_kind).collect();
    let sent = app
        .sent()
        .into_iter()
        .map(|command| {
            let wire = command.wire();
            let again = recto_agent::parse(&wire).unwrap_or_else(|error| panic!("{name}: {wire} isn't a line an agent can send: {error:?}"));
            assert_eq!(format!("{again:?}"), format!("{command:?}"), "{name}: {wire} reads back as another command");
            wire.replace(&app.disk().files().display().to_string(), "<files>")
        })
        .collect();
    Did { sent, why, acted, heard }
}

/// A Listener call's kind, with nothing it carried: what the golden holds of the host being told.
pub(super) fn heard_kind(heard: &crate::testkit::Heard) -> &'static str {
    use crate::testkit::Heard;
    match heard {
        Heard::ModelChanged => "model_changed",
        Heard::OpsReady => "ops_ready",
        Heard::QaRequest { .. } => "qa_request",
        Heard::SessionEnded => "session_ended",
        Heard::CopyReady => "copy_ready",
        Heard::RestoreReady => "restore_ready",
        Heard::QuitAnswer(_) => "quit_answer",
        Heard::HandOffQuit => "hand_off_quit",
    }
}

/// Every control's name and what it did, and the key of the recent file the names have.
fn every_control() -> (Vec<(String, Did)>, String) {
    let mut controls: BTreeSet<Control> = menu_ids().into_iter().map(Control::Menu).collect();
    let probe = app();
    let rows = crate::palette::items(probe.core().bar_now(), |_| true, false, true)
        .into_iter()
        .chain(crate::palette::items(probe.core().bar_now(), |_| true, true, true))
        .map(|row| row.id)
        .chain([format!("tab:{}", probe.tabs()[0].tab), format!("recent:{}", recent_key(&probe))]);
    controls.extend(rows.map(Control::Palette));
    let mut out: Vec<(String, Did)> = controls
        .into_iter()
        .map(|control| match control {
            Control::Menu(id) => {
                let name = id.clone();
                let did = sent_by(&name, |app| UiAction::Menu {
                    id: id.replace(&format!("recent.{ANY_RECENT}"), &format!("recent.{}", recent_key(app))),
                });
                (name, did)
            }
            Control::Palette(id) => {
                let name = format!("palette {id}");
                let did = sent_by(&name, |app| UiAction::PaletteRun {
                    id: id.replace(&recent_key(&probe), &recent_key(app)),
                });
                (name, did)
            }
        })
        .collect();
    let kinds = every_kind(&probe);
    for (at, (name, _)) in kinds.iter().enumerate() {
        let did = sent_by(name, |app| every_kind(app).swap_remove(at).1);
        out.push((name.clone(), did));
    }
    (out, recent_key(&probe))
}

/// **What each control did, a line each, as the golden has it:** its name, the commands it sent, what it gave back to the host and whether it was
/// understood, and the Listener's calls it made, in order. A recent file's key and a temporary folder are made the same on every app, so the line is the
/// same on every run; no wording a person sees and no time is in it. What the host is told is [`heard_line`]'s.
fn golden_lines(controls: &[(String, Did)], recent: &str) -> Vec<String> {
    controls
        .iter()
        .map(|(name, did)| {
            let name = name.replace(recent, "<recent>");
            let sent = did.sent.iter().map(|wire| wire.replace(recent, "<recent>")).collect::<Vec<_>>().join(" ; ");
            let saves = did.sent.iter().any(|wire| wire.starts_with(r#"{"cmd":"save","#));
            format!(
                "{name} | sent [{sent}] | host {:?} understood {} | heard {}",
                did.acted.host,
                did.acted.understood,
                heard_line(&did.heard, saves)
            )
        })
        .collect()
}

/// **The Listener's calls as the golden holds them:** every call in order, a run of one kind as `kind*n`, so an announcement made with nothing changed is a
/// count that differs, and the host told in another order is a line that differs. A control that saves has each run of a kind as `kind+` instead: a save's
/// steps (checking every page, writing, putting the file in place) are each a change the host is told of, as Save's tooltip says each, and two steps the
/// session thread's look finds together are one call; so how many go by the save's thread (2026-11-13: a save told 4 times in 19 runs of 20 under
/// `--like-ci`, and 3 in one, each call a step's change), while the order of the kinds held in every run. Every other control's calls were the same in all 20.
fn heard_line(heard: &[&str], saves: bool) -> String {
    if heard.is_empty() {
        return "nothing".to_string();
    }
    let mut runs: Vec<(&str, usize)> = Vec::new();
    for kind in heard {
        match runs.last_mut() {
            Some((last, count)) if last == kind => *count += 1,
            _ => runs.push((kind, 1)),
        }
    }
    let run = |(kind, count): &(&str, usize)| match (saves, count) {
        (true, _) => format!("{kind}+"),
        (false, 1) => (*kind).to_string(),
        (false, count) => format!("{kind}*{count}"),
    };
    runs.iter().map(run).collect::<Vec<_>>().join(" ")
}

/// What every control did on 2026-11-13, before `Inner`'s person's methods moved into their features (R3.7): one line a control, as [`golden_lines`] makes them.
const GOLDEN: &str = include_str!("golden/controls.txt");

/// **The first control whose line differs from the golden,** named, with both lines; and the whole of what it did now, written to a file to compare.
fn against_golden(now: &[String]) -> Result<(), String> {
    let golden: Vec<&str> = GOLDEN.lines().collect();
    if golden.iter().copied().eq(now.iter().map(String::as_str)) {
        return Ok(());
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recto-golden");
    let _ = std::fs::create_dir_all(&out);
    let written = out.join("controls.txt");
    let _ = std::fs::write(&written, now.join("\n") + "\n");
    // Each line by its control's name and which of that name it is: two controls of one kind (a tab's row and a contents row) have one name.
    let keyed = |lines: &mut dyn Iterator<Item = &str>| -> Vec<(String, String)> {
        let mut seen = std::collections::BTreeMap::<String, usize>::new();
        lines
            .map(|line| {
                let name = line.split(" | ").next().unwrap_or_default().to_string();
                let nth = seen.entry(name.clone()).or_default();
                *nth += 1;
                (format!("{name} (#{nth})"), line.to_string())
            })
            .collect()
    };
    let golden = keyed(&mut golden.into_iter());
    let now = keyed(&mut now.iter().map(String::as_str));
    let was: std::collections::BTreeMap<&str, &str> = golden.iter().map(|(key, line)| (key.as_str(), line.as_str())).collect();
    let is: std::collections::BTreeMap<&str, &str> = now.iter().map(|(key, line)| (key.as_str(), line.as_str())).collect();
    let mut said = Vec::new();
    for (key, _) in &golden {
        if !is.contains_key(key.as_str()) {
            said.push(format!("{key}: in the golden, and no control is it now"));
        }
    }
    for (key, line) in &now {
        match was.get(key.as_str()) {
            None => said.push(format!("{key}: a control the golden doesn't have:\n  now:    {line}")),
            Some(golden) if golden != line => said.push(format!("{key} did otherwise:\n  golden: {golden}\n  now:    {line}")),
            Some(_) => {}
        }
    }
    said.truncate(8);
    Err(format!("{}\n(what every control did now: {})", said.join("\n"), written.display()))
}

/// Failure (R2.3, C6): a control the window has does something no command does, with no reason given for it being the host's, so a person can do what an
/// agent can't (a second implementation, against the owner's rule); or a control said to be the host's sends a command after all, or is in no menu, panel,
/// palette or key any more, so the list says what isn't so; or a command the window sends isn't a line an agent could send. Every control, each on an app of
/// its own, as `every_control` lists them.
///
/// And (R3.7, a guard put in before `Inner`'s methods moved): a control does something other than it did, after code moved: it sends another command, sends
/// one twice or none, gives the host another action, or the host is told otherwise. Each control's line is held against the golden, naming the first that
/// differs. On the same apps, so it costs no launch of its own.
#[test]
fn every_control_is_a_command_or_the_hosts_with_a_reason() {
    let (controls, recent) = every_control();
    let lines = golden_lines(&controls, &recent);
    let mut wrong = Vec::new();
    let mut seen = BTreeSet::new();
    for (name, Did { sent, why, .. }) in &controls {
        if why.is_some() {
            seen.insert(name.as_str());
        }
        match (sent.is_empty(), why) {
            (true, None) => wrong.push(format!("{name} sends no command and isn't the host's")),
            (false, Some(why)) => wrong.push(format!("{name} is said to be the host's ({why}) and sends {sent:?}")),
            (_, Some("")) => wrong.push(format!("{name} needs its reason")),
            _ => {}
        }
    }
    for (id, _) in HOST_ONLY {
        if !seen.contains(id) {
            wrong.push(format!("{id} is in HOST_ONLY and no control is it"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(controls.len() > 60, "every control: {}", controls.len());
    if let Err(differs) = against_golden(&lines) {
        panic!("{differs}");
    }
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
