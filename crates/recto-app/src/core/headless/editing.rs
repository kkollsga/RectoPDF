//! **The window's edits, as the person makes them** (step R2.3, C5d): the panel's Rotate, Edit > Undo and Redo are each the command an agent sends, run for
//! the person, whose edits are the owner's.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;

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
    ];
    for (what, action, line) in steps {
        let before = twins.person.session_json();
        let (said, agent_said) = twins.both(action, &line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert_eq!(names(&said), ["edits_changed"], "{what}");
        assert_ne!(twins.person.session_json(), before, "{what}: something changed");
        twins.assert_same(
            &format!("{what} and {line}"),
            &["by", "holds_owner_edits", "owner_steps_undone", "undone_owner_steps"],
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
        app.core().host.session.with_session(|session| session.set_edit_mode(true));
        drop(app.heard_events());
    }
    let (said, agent_said) = twins.both(menu("rotate.right"), &rotate("3", 90));
    assert_eq!(said, agent_said, "edit mode's Rotate: the same said");
    twins.assert_same(
        "edit mode's Rotate and page rotate 3",
        &["by", "holds_owner_edits", "owner_steps_undone", "undone_owner_steps"],
    );
    let runs = |app: &HeadlessApp| app.session_json()["edits_full"]["1"]["pages"].as_array().map_or(0, Vec::len);
    assert_eq!(
        runs(&twins.person),
        3,
        "one page turned, between two runs that aren't: {}",
        twins.person.session_json()
    );
}
