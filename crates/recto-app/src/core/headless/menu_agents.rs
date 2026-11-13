//! **An agent's run of a menu id** (step R2.3, the review of the one table): what `recto palette --run` does with a row, whatever the palette lists. The
//! person's own is never run for an agent, and nothing an agent runs is sent as the person.

use super::menu_twins::{GRID, set_up};
use crate::testkit::headless::HeadlessApp;
use recto_agent::{Actor, Level};

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
/// the window (the app's lifecycle is the person's), and a submenu's or the system's own item, which does nothing.
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
];

/// An agent's palette run of `id` on `app` at `level`, as `recto palette --run` makes it once the row's found, settled: the code it was refused with, if it was.
fn agents_run(app: &HeadlessApp, level: Level, id: &str) -> Result<(), String> {
    let core = app.core();
    let result = core.host.run_menu_id(Actor::Agent(level), core, id, &mut Vec::new());
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
