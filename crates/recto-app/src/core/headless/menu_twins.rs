//! **A menu id is one action whoever runs it** (step R2.3; owner, 2026-11-12: "all features should be implemented once"): every menu id ⌘K's palette can
//! run, chosen by the person at the window on one app, and run by an agent at Full with `recto palette --run` on another with the same documents, in a small
//! grid of what's showing. Each must publish, announce and keep the same, but for who it's by and for what the actor rules decide.

use super::twins::Twins;
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// What's showing when the id is run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Showing {
    /// Two tabs, the second in front on its page 3, the sidebar shown, edit mode off.
    Reading,
    SidebarHidden,
    /// Edit mode on, on page 3.
    EditMode,
    /// The two tabs side by side.
    Comparing,
    /// Find open with hits.
    FindOpen,
    /// The owner's unsaved edits, one of them undone, and a step back made, so Undo, Redo, Forward, Save and Clear History can be chosen.
    Edited,
}

const GRID: [Showing; 6] = [
    Showing::Reading,
    Showing::SidebarHidden,
    Showing::EditMode,
    Showing::Comparing,
    Showing::FindOpen,
    Showing::Edited,
];

/// What the record and the window say of who something was by, which the actor decides: an agent's steps and jumps are an agent's, the person's the owner's.
const WHOSE: &[&str] = &[
    "by",
    "by_agent",
    "holds_owner_edits",
    "owner_steps_undone",
    "undone_owner_steps",
    "owner_step_dropped",
    "journal",
];

/// The person's own commands that make `showing`, on one of the twins, settled, with what they announced heard and forgotten.
fn set_up(app: &HeadlessApp, showing: Showing) {
    let mut lines = vec![r#"{"cmd":"goto","tab":"active","page":3}"#];
    lines.extend(match showing {
        Showing::Reading => vec![],
        Showing::SidebarHidden => vec![r#"{"cmd":"view","sidebar":"off"}"#],
        Showing::EditMode => vec![r#"{"cmd":"edit_mode","set":"on"}"#],
        Showing::Comparing => vec![r##"{"cmd":"compare","toggle":"#1"}"##],
        Showing::FindOpen => vec![r#"{"cmd":"find","open":true}"#, r#"{"cmd":"find","query":"page"}"#],
        Showing::Edited => vec![
            r#"{"cmd":"page_rotate","tab":"active","pages":"1","degrees":90}"#,
            r#"{"cmd":"page_rotate","tab":"active","pages":"2","degrees":90}"#,
            r#"{"cmd":"undo","tab":"active"}"#,
            r#"{"cmd":"back","tab":"active"}"#,
        ],
    });
    for line in lines {
        app.person(line);
        app.settle();
    }
    drop(app.heard_events());
}

fn twins(showing: Showing) -> Twins {
    let twins = Twins::on(&[("a.pdf", 5), ("b.pdf", 5)]);
    set_up(&twins.person, showing);
    set_up(&twins.agent, showing);
    twins
}

/// The palette's rows of menu ids with `showing`, as the window lists them: (id, title).
fn rows(showing: Showing) -> Vec<(String, String)> {
    rows_now(&twins(showing).agent)
}

/// An agent's `palette --run` of `title`'s row.
fn palette_run(title: &str) -> String {
    serde_json::json!({ "cmd": "palette", "query": title, "run": true }).to_string()
}

/// How the id run by each actor differed with `showing`, said, or nothing.
fn differences(showing: Showing, id: &str, title: &str) -> Vec<String> {
    let twins = twins(showing);
    twins.person.act(UiAction::Menu { id: id.into() });
    twins.person.settle();
    let person_said = twins.person.heard_events();
    let (result, mut agent_said) = twins.agent.agent_heard(Level::Full, &palette_run(title));
    twins.agent.settle();
    agent_said.extend(twins.agent.heard_events());
    let mut found = Vec::new();
    match &result {
        Err(error) => found.push(format!("the agent's run was refused ({}: {})", error.code, error.message)),
        Ok(answer) if answer["ran"]["id"] != id => found.push(format!("the agent's palette ran {} for `{title}`", answer["ran"]["id"])),
        Ok(_) => {}
    }
    if person_said != agent_said {
        found.push(format!(
            "announced otherwise: person {:?}, agent {:?}",
            super::twins::names(&person_said),
            super::twins::names(&agent_said)
        ));
    }
    found.extend(twins.differences(WHOSE));
    found
}

/// The person's two quick presses of a toggle, and an agent's two quick runs of its row, each pair made before the window has caught up with the first.
fn twice_quickly(id: &str) -> Vec<String> {
    let twins = twins(Showing::Reading);
    let title = |app: &HeadlessApp| rows_now(app).into_iter().find(|(row, _)| row == id).map(|(_, title)| title);
    let title = title(&twins.agent).unwrap_or_else(|| panic!("{id} is listed"));
    {
        let _held = twins.person.hold_session_thread();
        twins.person.act(UiAction::Menu { id: id.into() });
        twins.person.act(UiAction::Menu { id: id.into() });
    }
    twins.person.settle();
    let mut found = Vec::new();
    {
        let _held = twins.agent.hold_session_thread();
        for _ in 0..2 {
            if let Err(error) = twins.agent.agent(Level::Full, &palette_run(&title)) {
                found.push(format!("the agent's run was refused ({}: {})", error.code, error.message));
            }
        }
    }
    twins.agent.settle();
    found.extend(twins.differences(WHOSE));
    found
}

/// The palette's rows of menu ids as `app` lists them now: (id, title).
fn rows_now(app: &HeadlessApp) -> Vec<(String, String)> {
    let inner = app.core();
    let reader = app.reader();
    crate::palette::items(
        inner.bar_now(),
        |id| inner.menu_item_enabled(id.to_string()),
        reader.edit_mode,
        reader.can_rotate,
    )
    .into_iter()
    .map(|row| (row.id, row.title))
    .collect()
}

/// Failure (R2.3, the review's "Rule 4, in hand"): a menu id does one thing chosen by the person and another run by an agent through the palette, so the
/// action is implemented twice and the two have drifted: a toggle worked out from what the window last showed for the agent and toggled when it runs for the
/// person (two quick runs leave it otherwise), an edit mode Rotate the person can make and the agent's palette refuses, a row the agent's palette refuses that
/// the person's menu runs as a command. Every menu id the palette lists in each state of a small grid, each on twins of its own: the same published,
/// announced and kept, but for whose it is.
#[test]
fn every_menu_id_is_one_action_for_the_person_and_an_agent() {
    let mut wrong = Vec::new();
    let mut ran = 0;
    for showing in GRID {
        for (id, title) in rows(showing) {
            ran += 1;
            for difference in differences(showing, &id, &title) {
                wrong.push(format!("{showing:?} {id}: {difference}"));
            }
        }
    }
    for id in ["view.sidebar", "view.agent_icon"] {
        for difference in twice_quickly(id) {
            wrong.push(format!("twice quickly, {id}: {difference}"));
        }
    }
    assert!(ran > 100, "every row in every state: {ran}");
    assert!(wrong.is_empty(), "{} differences:\n{}", wrong.len(), wrong.join("\n"));
}
