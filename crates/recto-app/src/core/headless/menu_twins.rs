//! **A menu id is one action whoever runs it** (step R2.3; owner, 2026-11-12: "all features should be implemented once"): every menu id ⌘K's palette can
//! run, chosen by the person at the window on one app, and run by an agent at Full with `recto palette --run` on another with the same documents, in a small
//! grid of what's showing. Each must publish, announce and keep the same, but for who it's by and for what the actor rules decide.

use super::twins::Twins;
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// What's showing when the id is run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Showing {
    /// Two tabs, the second in front on its page 3, the sidebar shown, edit mode off.
    Reading,
    SidebarHidden,
    /// Edit mode on, on page 3.
    EditMode,
    /// The two tabs side by side.
    Comparing,
    /// Find open with hits.
    FindOpen,
    /// Find closed after something was found, which ⌘G finds again.
    FindClosed,
    /// The page's text selected, so Copy can be chosen.
    Selected,
    /// The owner's unsaved edits, one of them undone, and a step back made, so Undo, Redo, Forward, Save and Clear History can be chosen.
    Edited,
}

pub(super) const GRID: [Showing; 8] = [
    Showing::Reading,
    Showing::SidebarHidden,
    Showing::EditMode,
    Showing::Comparing,
    Showing::FindOpen,
    Showing::FindClosed,
    Showing::Selected,
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
pub(super) fn set_up(app: &HeadlessApp, showing: Showing) {
    let mut lines = vec![r#"{"cmd":"goto","tab":"active","page":3}"#];
    lines.extend(match showing {
        Showing::Reading => vec![],
        Showing::SidebarHidden => vec![r#"{"cmd":"view","sidebar":"off"}"#],
        Showing::EditMode => vec![r#"{"cmd":"edit_mode","set":"on"}"#],
        Showing::Comparing => vec![r##"{"cmd":"compare","toggle":"#1"}"##],
        Showing::FindOpen => vec![r#"{"cmd":"find","open":true}"#, r#"{"cmd":"find","query":"page"}"#],
        Showing::Selected => vec![],
        Showing::FindClosed => vec![
            r#"{"cmd":"find","open":true}"#,
            r#"{"cmd":"find","query":"page"}"#,
            r#"{"cmd":"find","close":true}"#,
        ],
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
    if showing == Showing::Selected {
        app.act(UiAction::Menu { id: "edit.select_all".into() });
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

/// **Where the actor rules decide that the two differ,** each stated once in its command and named here, so the test asserts the exception itself and it
/// can't grow quietly (coordinator's rulings of 2026-11-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActorRule {
    /// Find's at-the-window rule (`at_the_window` in the `find` command): the person's ⌘G or ⇧⌘G with Find closed opens it with what was typed last; an
    /// agent's is refused `nothing_found`, as there's no hit to go to.
    FindStepWithFindClosed,
    /// The same rule: the person's ⌘F shows the sidebar Find is in (A6); an agent's opens Find and leaves the sidebar as it is.
    FindOpenWithTheSidebarHidden,
    /// The close rule (`recto_agent::close_rule`): with unsaved edits a person is asked, and an agent is refused `unsaved_changes`.
    CloseThatLosesEdits,
    /// A row that's the window's alone and the person's own (`menu_rows::Agents::Never`): the person's run does it, and an agent's is refused
    /// `persons_own` with nothing changed.
    PersonsOwn,
}

/// The actor rule that decides `id` on `app` as it is now, if one does.
fn actor_rule(app: &HeadlessApp, id: &str) -> Option<ActorRule> {
    let reader = app.reader();
    let find_open = app.session_json()["find"]["open"] == true;
    match id {
        "find.next" | "find.previous" if !find_open => Some(ActorRule::FindStepWithFindClosed),
        "find.open" if reader.sidebar_hidden => Some(ActorRule::FindOpenWithTheSidebarHidden),
        "file.close" if reader.edited => Some(ActorRule::CloseThatLosesEdits),
        _ if matches!(
            crate::core::menu_rows::row(app.core(), id),
            Some(crate::core::menu_rows::Row::Window {
                agents: crate::core::menu_rows::Agents::Never,
                ..
            })
        ) =>
        {
            Some(ActorRule::PersonsOwn)
        }
        _ => None,
    }
}

/// How the id run by each actor differed with `showing`, said, or nothing.
fn differences(showing: Showing, id: &str, title: &str) -> Vec<String> {
    let twins = twins(showing);
    let rule = actor_rule(&twins.person, id);
    let (find_before, kept_before) = (twins.person.session_json()["find"].clone(), twins.agent.session_json());
    let acted = twins.person.act(UiAction::Menu { id: id.into() });
    twins.person.settle();
    let mut person_said = twins.person.heard_events();
    let (result, mut agent_said) = twins.agent.agent_heard(Level::Full, &palette_run(title));
    twins.agent.settle();
    agent_said.extend(twins.agent.heard_events());
    let mut found = Vec::new();
    let refused = result.as_ref().err().map(|error| error.code.to_string());
    let mut except = WHOSE.to_vec();
    match rule {
        Some(ActorRule::FindStepWithFindClosed) => {
            let find = &twins.person.session_json()["find"];
            let typed_last = if find_before["query"] == "" {
                &find_before["draft"]
            } else {
                &find_before["query"]
            };
            if find["open"] != true || find["query"] != *typed_last {
                found.push(format!("the person's step with Find closed didn't open it with what was typed last: {find}"));
            }
            if refused.as_deref() != Some("nothing_found") || twins.agent.session_json() != kept_before {
                found.push(format!(
                    "an agent's step with Find closed wasn't refused `nothing_found` with nothing changed: {result:?}"
                ));
            }
            return found;
        }
        Some(ActorRule::FindOpenWithTheSidebarHidden) => {
            if twins.person.reader().sidebar_hidden || !twins.agent.reader().sidebar_hidden {
                found.push("the person's ⌘F didn't show the sidebar, or an agent's did".into());
            }
            person_said.retain(|event| event.name() != "view_changed");
            except.push("sidebar_hidden");
        }
        Some(ActorRule::CloseThatLosesEdits) => {
            if acted.host != crate::menus::HostAction::ConfirmClose || refused.as_deref() != Some("unsaved_changes") {
                found.push(format!(
                    "the person wasn't asked ({:?}), or an agent wasn't refused `unsaved_changes` ({result:?})",
                    acted.host
                ));
            }
            found.extend(twins.differences(&except));
            return found;
        }
        Some(ActorRule::PersonsOwn) => {
            if refused.as_deref() != Some("persons_own") || twins.agent.session_json() != kept_before {
                found.push(format!(
                    "an agent's run of the person's own wasn't refused `persons_own` with nothing changed: {result:?}"
                ));
            }
            return found;
        }
        None => {}
    }
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
    found.extend(twins.differences(&except));
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
    let mut ids = std::collections::BTreeSet::new();
    for showing in GRID {
        for (id, title) in rows(showing) {
            ran += 1;
            ids.insert(id.clone());
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
    // The ids with a way of their own that an agent's palette can run are each in the grid: the zoom, Save, the update, Copy and Select All.
    for id in ["view.zoom_in", "file.save", "app.update", "edit.copy", "edit.select_all"] {
        assert!(ids.contains(id), "{id} is run in some state of the grid");
    }
    assert!(wrong.is_empty(), "{} differences:\n{}", wrong.len(), wrong.join("\n"));
}

/// The code an agent's line at `level` is refused with, or `Ok` if it isn't.
fn refusal(app: &HeadlessApp, level: Level, line: &str) -> Result<(), String> {
    let result = app.agent(level, line).map(drop).map_err(|error| error.code.to_string());
    app.settle();
    result
}

/// The palette's title for `id` on `app` now.
fn title_of(app: &HeadlessApp, id: &str) -> String {
    rows_now(app)
        .into_iter()
        .find(|(row, _)| row == id)
        .map(|(_, title)| title)
        .unwrap_or_else(|| panic!("{id} is listed"))
}

/// Failure (R2.3, the coordinator's rulings of 2026-11-13): a row the palette now runs as the agent's command (File > Close, Find, Back, Forward, Leave Edit
/// Mode, a layout) gets past a check that command has, or is refused another way: an agent goes Back on a document opened with its password, or closes over
/// unsaved edits, through the palette. For each, where the command's own check refuses the agent, the palette's run is refused the same way, with the same
/// code; and the close rule's three answers for an agent hold by the palette: unsaved edits refused at every level, the owner's undone steps refused below
/// Full, and at Full the close goes ahead.
#[test]
fn a_rows_refusal_is_its_commands() {
    let app = HeadlessApp::start();
    let file = app.disk().file("locked.pdf", &super::files::locked_with("right", 6));
    app.core().open_file(file);
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core().password_answered(id, Some("right".into()));
    app.settle();
    let tab = app.reader().shown_tab;
    for line in [
        r#"{"cmd":"goto","tab":"active","page":3}"#,
        r#"{"cmd":"goto","tab":"active","page":4}"#,
        r#"{"cmd":"back","tab":"active"}"#,
        r#"{"cmd":"edit_mode","set":"on"}"#,
    ] {
        app.person(line);
        app.settle();
    }
    let protected = Err("protected".to_string());
    let layout = format!(r##"{{"cmd":"view","layout":"two-up","tab":"#{tab}"}}"##);
    for (id, line) in [
        ("find.open", r#"{"cmd":"find","open":true}"#),
        ("go.back", r#"{"cmd":"back"}"#),
        ("go.forward", r#"{"cmd":"forward"}"#),
        ("layout.two_up", layout.as_str()),
    ] {
        let title = title_of(&app, id);
        assert_eq!(refusal(&app, Level::Full, line), protected, "{id}: `{line}` on a protected tab");
        assert_eq!(
            refusal(&app, Level::Full, &palette_run(&title)),
            protected,
            "{id} by the palette, as its command"
        );
    }
    let title = title_of(&app, "edit_mode.done");
    let below = Err("permission_denied".to_string());
    assert_eq!(
        refusal(&app, Level::ReadOnly, r#"{"cmd":"edit_mode","set":"off"}"#),
        below,
        "edit mode off below Control"
    );
    assert_eq!(
        refusal(&app, Level::ReadOnly, &palette_run(&title)),
        below,
        "Leave Edit Mode by the palette below Control"
    );
    // The person's layout on the protected tab applies; the agent's was refused above.
    app.act(UiAction::Menu { id: "layout.two_up".into() });
    app.settle();
    assert_eq!(app.reader().layout, "two-up", "the person's layout");

    let app = HeadlessApp::start();
    let file = app.disk().pdf("plain.pdf", 3);
    app.open(&file);
    app.person(r#"{"cmd":"page_rotate","tab":"active","pages":"1","degrees":90}"#);
    app.settle();
    let close = title_of(&app, "file.close");
    for level in [Level::Control, Level::Full] {
        let unsaved = Err("unsaved_changes".to_string());
        assert_eq!(refusal(&app, level, r#"{"cmd":"close"}"#), unsaved, "close over unsaved edits at {level:?}");
        assert_eq!(refusal(&app, level, &palette_run(&close)), unsaved, "File > Close by the palette at {level:?}");
    }
    app.person(r#"{"cmd":"undo","tab":"active"}"#);
    app.settle();
    assert!(!app.reader().edited && app.sidebar().undone_owner_steps, "only the owner's undone step left");
    let undone = Err("would_clear_redo".to_string());
    assert_eq!(
        refusal(&app, Level::Control, r#"{"cmd":"close"}"#),
        undone,
        "below Full, the owner's undone steps"
    );
    assert_eq!(refusal(&app, Level::Control, &palette_run(&close)), undone, "the same by the palette");
    assert_eq!(refusal(&app, Level::Full, &palette_run(&close)), Ok(()), "at Full it goes ahead");
    assert!(app.tabs().is_empty(), "closed");
}
