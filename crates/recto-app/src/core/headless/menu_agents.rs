//! **An agent's run of a menu id** (step R2.3, the review of the one table): what `recto palette --run` does with a row, whatever the palette lists. The
//! person's own is never run for an agent, and nothing an agent runs is sent as the person.

use super::menu_twins::{GRID, set_up};
use crate::testkit::headless::HeadlessApp;
use recto_agent::{Actor, Level};
use serde_json::json;

/// The ids a notice offers, which no menu lists: a refused print's and a refused snapshot's "anyway", and Stop for a print being prepared.
const NOTICE_IDS: [&str; 3] = ["print.anyway.1", "tools.copy_anyway.1", "print.stop"];

/// Every menu id there is, the notices' included.
fn every_id() -> Vec<String> {
    let mut ids: Vec<String> = super::coverage::menu_ids().into_iter().collect();
    ids.extend(NOTICE_IDS.map(str::to_string));
    ids
}

/// **The rows that are the person's own,** which an agent's run never does, at any level: a decision the person made at a notice (Copy anyway, Print
/// anyway: an agent's is `--anyway`, and it's logged), stopping the person's print, the palette's field taking the person's keyboard, quitting and closing
/// the window (the app's lifecycle is the person's), a submenu's or the system's own item, which does nothing; Copy, which puts the person's selection
/// on the system's clipboard whoever asks, and Select All, which rearranges the person's window; and a window or field for the person, which an agent's run
/// would only answer with a word (an agent has `open`, `save --out`, `print`, `goto` and `copy` of its own).
const PERSONS_OWN: &[&str] = &[
    "edit.copy_anyway",
    "print.anyway.1",
    "tools.copy_anyway.1",
    "print.stop",
    "palette.open",
    "app.quit",
    "file.close_window",
    "app.services",
    "file.open_recent",
    "view.paper",
    "edit.copy",
    "edit.select_all",
    "app.about",
    "app.settings",
    "file.settings",
    "file.open",
    "file.save_as",
    "file.print",
    "go.page",
];

/// An agent's palette run of `id` on `app` at `level`, as `recto palette --run` makes it once the row's found, settled: the code it was refused with, if it was.
fn agents_run(app: &HeadlessApp, level: Level, id: &str) -> Result<(), String> {
    let core = app.core();
    let result = core.host.run_menu_id(Actor::Agent(level), core, id, None, &mut Vec::new());
    app.settle();
    result.map(drop).map_err(|error| error.code.to_string())
}

/// Failure (review of R2.3, S2): a row that's the person's own is run for an agent at Full by the menu's way, as the person: a notice's Print anyway or Copy
/// anyway skips "Agents need `--anyway`, and it's logged", Stop stops the person's print, ⌘K sends `view --sidebar on` as the person. Nothing held it but
/// that the palette's items don't list them. Each is refused for an agent at Control and at Full, whatever the palette lists, with nothing sent as the
/// person and nothing changed.
#[test]
fn an_agent_is_refused_every_row_that_is_the_persons_own() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("a.pdf", 3);
    app.open(&file);
    app.person(r#"{"cmd":"view","sidebar":"off"}"#);
    app.settle();
    let before = (app.session_json(), format!("{:?}", app.reader()));
    app.keep_sent();
    for id in PERSONS_OWN {
        for level in [Level::Control, Level::Full] {
            assert_eq!(agents_run(&app, level, id), Err("persons_own".to_string()), "{id} at {level:?}");
        }
    }
    let sent = app.sent();
    assert!(sent.is_empty(), "nothing sent as the person: {sent:?}");
    assert_eq!((app.session_json(), format!("{:?}", app.reader())), before, "nothing changed");
}

/// Failure (review of R2.3, S2): an agent's run of a row sends a command as the person: a row the window does alone whose way sends one (⌘K's
/// `view --sidebar on`), or any row run the person's way. Every menu id, in each state of the twins grid, run for an agent at Full on one app per state:
/// nothing reaches the executor as the person's.
#[test]
fn no_rows_agent_run_sends_a_command_as_the_person() {
    let ids = every_id();
    let mut wrong = Vec::new();
    for showing in GRID {
        let app = HeadlessApp::start();
        let (a, b) = (app.disk().pdf("a.pdf", 5), app.disk().pdf("b.pdf", 5));
        app.open(&a);
        app.open(&b);
        set_up(&app, showing);
        app.keep_sent();
        // What closes or empties goes last, so the rest run in the state the grid names.
        let (last, first): (Vec<&String>, Vec<&String>) = ids.iter().partition(|id| matches!(id.as_str(), "file.close" | "recent.clear"));
        for id in first.into_iter().chain(last) {
            drop(agents_run(&app, Level::Full, id));
            let sent = app.sent();
            if !sent.is_empty() {
                wrong.push(format!("{showing:?} {id}: sent as the person {sent:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Failure (review of R2.3, F8): one menu id claimed by two features' rows, so it means what the first in the table says and the other feature's row is
/// dead code that reads as live. Every menu id, the notices' included, is claimed by exactly one feature, and the failure names both.
#[test]
fn each_menu_id_is_claimed_by_one_feature() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("a.pdf", 3);
    app.open(&file);
    let mut wrong = Vec::new();
    for id in every_id() {
        let claims = crate::core::menu_rows::claimed_by(app.core(), &id);
        if claims.len() != 1 {
            wrong.push(format!("{id}: claimed by {claims:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Edit mode's Rotate by an agent's palette, resolved on `resolved_on`'s page 3 and then, before it runs, the person's tap on `then_tab`: what the agent's
/// run came to, and which tabs it edited.
fn rotate_raced(app: &HeadlessApp, then_tab: u32) -> (Result<(), String>, Vec<u32>) {
    let core = app.core();
    let resolved = crate::core::menu_rows::for_agent(core, "rotate.right", |when_run, command| {
        let resolved = core.host.session.with_session(|session| when_run(session, Actor::Agent(Level::Full), command));
        app.person(&format!(r##"{{"cmd":"tap","tab":"#{then_tab}"}}"##));
        app.settle();
        resolved
    });
    let command = match resolved {
        Ok(crate::core::menu_rows::ForAgent::Command(command)) => command,
        Ok(_) => panic!("Rotate is a command"),
        Err(error) => return (Err(error.code.to_string()), edited_tabs(app)),
    };
    let result = core.host.run_counted(Actor::Agent(Level::Full), &command, &mut Vec::new());
    app.settle();
    (result.map(drop).map_err(|error| error.code.to_string()), edited_tabs(app))
}

/// The tabs with unsaved edits.
fn edited_tabs(app: &HeadlessApp) -> Vec<u32> {
    app.core().host.session.with_session(|session| {
        session
            .tabs()
            .iter()
            .filter(|tab| session.lists().is_edited(tab.id))
            .map(|tab| tab.id.0)
            .collect()
    })
}

/// Failure (review of R2.3, F2): edit mode's Rotate by an agent's palette is resolved under one hold of the session and run under another, its page from the
/// tab being read and its tab left as `active`, so a tap between the two rotates that page of another tab; and from a document opened with its password it
/// carries that tab's page onto one that isn't, where the agent's rotate isn't refused. The person's tap is put between the resolve and the run: the page
/// rotated is the one on the tab it was resolved on, and on a protected tab the agent is refused `protected`, with nothing rotated anywhere.
#[test]
fn edit_modes_rotate_by_an_agent_is_the_page_it_was_resolved_on() {
    let app = HeadlessApp::start();
    let file = app.disk().file("locked.pdf", &super::files::locked_with("right", 5));
    app.act(crate::core::controls::UiAction::Open {
        path: file.display().to_string(),
    });
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core().password_answered(id, Some("right".into()));
    app.settle();
    let plain = app.disk().pdf("plain.pdf", 5);
    app.open(&plain);
    let (locked, plain) = (1, 2);
    for line in [r#"{"cmd":"goto","tab":"active","page":3}"#, r#"{"cmd":"edit_mode","set":"on"}"#] {
        app.person(line);
        app.settle();
    }
    assert_eq!(
        rotate_raced(&app, locked),
        (Ok(()), vec![plain]),
        "resolved on the plain tab, run after a tap on the locked one"
    );
    app.person(r##"{"cmd":"discard","tab":"#2"}"##);
    app.person(r#"{"cmd":"goto","tab":"active","page":3}"#);
    app.settle();
    assert_eq!(app.reader().shown_tab, locked, "the locked tab in front");
    assert_eq!(
        rotate_raced(&app, plain),
        (Err("protected".to_string()), vec![]),
        "resolved on the locked tab, run after a tap on the plain one"
    );
}

/// Failure (review of R2.3, round 2, B1): an agent's palette run of Copy, with the person's text selected on a document opened with its password, copies it
/// to the system's clipboard by the page view's own copy, which doesn't know who asked: text an agent's own `copy` refuses, readable by any process, and the
/// person's clipboard overwritten. Refused, at Full, and nothing goes to the clipboard.
#[test]
fn an_agents_copy_puts_nothing_of_a_protected_document_on_the_clipboard() {
    let app = HeadlessApp::start();
    let file = app.disk().file("locked.pdf", &super::files::locked_with("right", 3));
    app.act(crate::core::controls::UiAction::Open {
        path: file.display().to_string(),
    });
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core().password_answered(id, Some("right".into()));
    app.settle();
    app.act(crate::core::controls::UiAction::Menu { id: "edit.select_all".into() });
    app.settle();
    drop(app.listener().take());
    let title = crate::core::palette::title_now(app.core(), "edit.copy").expect("Copy is listed with text selected");
    let result = app.agent(Level::Full, &serde_json::json!({ "cmd": "palette", "query": title, "run": true }).to_string());
    app.settle();
    assert_eq!(result.map(drop).map_err(|error| error.code.to_string()), Err("persons_own".to_string()));
    // A copy started is on its way to the clipboard page by page; none is, and none is waiting for the host to put on it.
    let selection = app.core().panes.reading_engine(&app.core().engine).report().0["selection"].clone();
    assert_eq!(selection["selected"], true, "the person's selection is still there: {selection}");
    assert_eq!(
        (&selection["copying"], &selection["copy_waiting_for_host"]),
        (&json!(false), &json!(false)),
        "nothing copied: {selection}"
    );
    let heard = app.listener().take();
    assert!(!heard.iter().any(|heard| matches!(heard, crate::testkit::Heard::CopyReady)), "{heard:?}");
}
