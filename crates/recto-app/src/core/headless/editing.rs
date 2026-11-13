//! **The window's edits, as the person makes them** (step R2.3, C5d): the panel's Rotate, Edit > Undo and Redo are each the command an agent sends, run for
//! the person, whose edits are the owner's.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

fn menu(id: &str) -> UiAction {
    UiAction::Menu { id: id.into() }
}

/// One edit: its name, what the person does, and the agent's line.
type Step = (&'static str, UiAction, String);

/// Who the steps of the tab's edits are by, as the record keeps them: `o` for the owner's, `a` for an agent's.
fn by(app: &HeadlessApp) -> Vec<String> {
    let record = app.session_json();
    let edits = &record["edits_full"]["1"];
    ["undo", "redo"]
        .iter()
        .flat_map(|list| edits[*list].as_array().cloned().unwrap_or_default())
        .filter_map(|step| step["by"].as_str().map(String::from))
        .collect()
}

/// Failure (R2.3, C5d): the panel's Rotate, Undo or Redo does something other than the command an agent sends: other pages turned, another event, the
/// edit-mode Rotate turning every page or another page, or something else kept; or the person's edit isn't the owner's. Each on one app, and its line at
/// Full on another with the same file: the same shown, said and kept, but for whose edits they are.
#[test]
fn the_windows_edits_are_the_agents_commands() {
    let twins = Twins::on(&[("a.pdf", 5)]);
    let rotate = |pages: &str, degrees: i32| format!(r#"{{"cmd":"page_rotate","tab":"active","pages":"{pages}","degrees":{degrees}}}"#);
    let steps: Vec<Step> = vec![
        ("Rotate right", menu("rotate.right"), rotate("all", 90)),
        ("Rotate left", menu("rotate.left"), rotate("all", -90)),
        ("Undo", menu("edit.undo"), r#"{"cmd":"undo"}"#.into()),
        ("Redo", menu("edit.redo"), r#"{"cmd":"redo"}"#.into()),
        ("Undo again", menu("edit.undo"), r#"{"cmd":"undo"}"#.into()),
        ("Clear History", menu("edit.clear_history"), r#"{"cmd":"clear_history"}"#.into()),
        ("Rotate after the clear", menu("rotate.right"), rotate("all", 90)),
    ];
    for (what, action, line) in steps {
        let before = twins.person.session_json();
        let (said, agent_said) = twins.both(action, &line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert_eq!(names(&said), ["edits_changed"], "{what}");
        assert_ne!(twins.person.session_json(), before, "{what}: something changed");
        // Whose the steps are, and so whether a step of the owner's was dropped, is the actor's; a journal's name is its disk's.
        twins.assert_same(
            &format!("{what} and {line}"),
            &[
                "by",
                "holds_owner_edits",
                "owner_steps_undone",
                "undone_owner_steps",
                "owner_step_dropped",
                "journal",
            ],
        );
    }
    assert!(
        by(&twins.person).iter().all(|by| by == "o"),
        "the person's edits are the owner's: {:?}",
        by(&twins.person)
    );
    assert!(by(&twins.agent).iter().all(|by| by == "a"), "an agent's are an agent's: {:?}", by(&twins.agent));
    // In edit mode, the panel's Rotate turns the page being read, by the edit mode the session has when it runs.
    for app in [&twins.person, &twins.agent] {
        app.agent(recto_agent::Level::Full, r##"{"cmd":"goto","tab":"#1","page":3}"##)
            .expect("to page 3");
        app.settle();
        app.recto(recto_agent::Level::Control, &["edit", "mode", "on"])
            .expect("an agent at Control turns it on");
        app.settle();
        drop(app.heard_events());
    }
    let (said, agent_said) = twins.both(menu("rotate.right"), &rotate("3", 90));
    assert_eq!(said, agent_said, "edit mode's Rotate: the same said");
    twins.assert_same(
        "edit mode's Rotate and page rotate 3",
        &[
            "by",
            "holds_owner_edits",
            "owner_steps_undone",
            "undone_owner_steps",
            "owner_step_dropped",
            "journal",
        ],
    );
    let runs = |app: &HeadlessApp| app.session_json()["edits_full"]["1"]["pages"].as_array().map_or(0, Vec::len);
    assert_eq!(
        runs(&twins.person),
        3,
        "one page turned, between two runs that aren't: {}",
        twins.person.session_json()
    );
}

/// Failure (R2.3, C5d): the panel's Done does something other than `edit mode off`, or `recto edit mode on` at Control doesn't turn on the window's edit mode
/// (its tools in the sidebar, its Rotate of the page being read).
#[test]
fn the_panels_done_is_edit_mode_off() {
    let twins = Twins::on(&[("a.pdf", 5)]);
    for app in [&twins.person, &twins.agent] {
        let on = app.recto(recto_agent::Level::Control, &["edit", "mode", "on"]).expect("turned on at Control");
        assert_eq!(on["changed"], true);
        app.settle();
        assert!(app.reader().edit_mode, "the window's in edit mode");
        drop(app.heard_events());
    }
    let (said, agent_said) = twins.both(menu("edit_mode.done"), r#"{"cmd":"edit_mode","set":"off"}"#);
    assert_eq!(said, agent_said, "the same said");
    assert!(!twins.person.reader().edit_mode, "out of edit mode");
    twins.assert_same("Done and edit mode off", &[]);
}

/// Failure (R2.3, by A2's rule): Clear History is announced other than by whether there was history to clear: the window's said nothing when it cleared some,
/// and an agent's `clear-history` said `edits_changed` when there was none. For the person and an agent, a clear of some history is announced, and one of
/// none isn't.
#[test]
fn clear_history_is_announced_only_when_there_was_history() {
    let app = crate::testkit::headless::Disk::new().start();
    let file = app.disk().pdf("a.pdf", 3);
    app.open(&file);
    app.hear_events();
    let edit = || {
        app.person(r#"{"cmd":"page_rotate","tab":"active","pages":"all","degrees":90}"#);
        app.settle();
        drop(app.heard_events());
    };
    let person = || {
        app.act(menu("edit.clear_history"));
        app.settle();
        names(&app.heard_events())
    };
    let agent = || {
        let (result, mut said) = app.agent_heard(recto_agent::Level::Full, r#"{"cmd":"clear_history"}"#);
        result.expect("cleared");
        app.settle();
        said.extend(app.heard_events());
        names(&said)
    };
    edit();
    assert_eq!(person(), ["edits_changed"], "the person's clear of a step");
    assert!(person().is_empty(), "and of none");
    edit();
    assert_eq!(agent(), ["edits_changed"], "an agent's clear of a step");
    assert!(agent().is_empty(), "and of none");
}

/// An app whose tab `#1` has the owner's rotation on page 1, its list asleep and its journal written over, so it can't be read back; `#2` is showing.
fn edits_that_cant_be_read() -> crate::testkit::headless::HeadlessApp {
    let disk = crate::testkit::headless::Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3));
    let app = disk.start();
    app.open(&a);
    app.open(&b);
    for line in [
        r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##,
        r##"{"cmd":"tap","tab":"#2"}"##,
    ] {
        app.person(line);
        app.settle();
    }
    let slept = app.agent(Level::Full, r##"{"cmd":"session_sleep","tab":"#1"}"##).expect("put to sleep");
    assert_eq!(slept["list_asleep"], true, "{slept}");
    super::closing::journals_written_over(&app);
    app
}

/// **Failure (R3.5a): `edits` on a tab whose list can't be read back answers that nothing is reordered or turned** (`runs: []`, from the list not being
/// in memory), when the tab is edited and what's on its pages can't be known. It refuses as anything reading the pages does, `edits_unreadable`, with
/// what to do.
#[test]
fn edits_of_a_list_that_cant_be_read_back_is_refused_not_empty() {
    let app = edits_that_cant_be_read();
    let refused = app
        .agent(Level::Full, r##"{"cmd":"edits","tab":"#1"}"##)
        .map_err(|error| (error.code, error.next.clone()));
    let Err((code, next)) = refused else { panic!("answered: {refused:?}") };
    assert_eq!(code, "edits_unreadable");
    assert!(next.contains("recto discard"), "it says what to do: {next}");
}

/// Failure (review of R2.3, round 2, B2): an agent's `discard` or `clear-history` of a tab whose edit list is asleep and can't be read back throws away the
/// owner's edits: the guards read the owner's edits from a list in memory, which an asleep one isn't, so an agent at Control got past them, the list was
/// given up, and `discard` said there had been no edits. Where what it would lose can't all be read, an agent is refused `edits_unreadable` at Control and
/// at Full, and nothing is given up; the person's own discard still throws them away, and says so.
#[test]
fn an_agent_neither_discards_nor_clears_edits_it_cant_read() {
    let app = edits_that_cant_be_read();
    let asleep = || {
        app.core()
            .host
            .session
            .with_session(|session| session.lists().is_asleep(recto_session::TabId(1)))
    };
    for line in [r##"{"cmd":"discard","tab":"#1"}"##, r##"{"cmd":"clear_history","tab":"#1"}"##] {
        for level in [Level::Control, Level::Full] {
            let refused = app.agent(level, line).map(drop).map_err(|error| error.code);
            app.settle();
            assert_eq!(refused, Err("edits_unreadable"), "{line} at {level:?}");
            let edited = app
                .core()
                .host
                .session
                .with_session(|session| session.lists().is_edited(recto_session::TabId(1)));
            assert!(asleep() && edited, "{line} at {level:?}: nothing given up");
        }
    }
    drop(app.heard_events());
    app.hear_events();
    app.person(r##"{"cmd":"discard","tab":"#1"}"##);
    app.settle();
    assert!(!asleep(), "the person's discard gives the list up");
    let said = app.heard_events();
    assert!(said.iter().any(|event| event.name() == "edits_changed"), "and says so: {said:?}");
}
