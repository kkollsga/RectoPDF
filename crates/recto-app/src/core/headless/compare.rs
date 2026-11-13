//! **Side by side, as the person does it** (step R2.3, C5e): a ⌘-click on a tab's row, moving into a pane, and the context menu's separate or synced
//! scroll, each the `compare` an agent sends, run for the person.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;

/// One control: its name, what the person does, and the agent's line.
type Step = (&'static str, Box<dyn Fn(&HeadlessApp)>, String);

/// A ⌘-click on a tab's row, as the window reports it.
fn command_click(app: &HeadlessApp, tab: u32) -> UiAction {
    let row = app.tabs().into_iter().find(|row| row.tab == tab).expect("its row");
    UiAction::Row { row, adding: true }
}

/// Without sleep files, and with this tab put to sleep, so waking it reads its PDF.
fn cold(app: &HeadlessApp, tab: u32) {
    app.core().host.session.with_session(|session| session.set_sleep_files(false));
    app.agent(Level::Full, &format!(r##"{{"cmd":"session_sleep","tab":"#{tab}"}}"##))
        .expect("put to sleep");
    app.settle();
}

/// Failure (R2.3, C5e): a ⌘-click on a tab's row, moving into a pane, or the context menu's scroll does something other than the `compare` an agent sends:
/// other panes, another one read, another event, or something else kept in the record. Each on one app, and its line at Full on another with the same
/// files: a warm tab joining, the reader moving into the other pane, synced and separate scroll, the tab read leaving, and a cold tab joining, read first.
#[test]
fn the_windows_comparison_controls_are_the_agents_compare() {
    let twins = Twins::on(&[("a.pdf", 4), ("b.pdf", 6), ("c.pdf", 5)]);
    let toggle = |tab: u32| format!(r##"{{"cmd":"compare","toggle":"#{tab}"}}"##);
    let menu = |id: &'static str| {
        move |app: &HeadlessApp| {
            app.act(UiAction::Menu { id: id.into() });
        }
    };
    let click = |tab: u32| {
        move |app: &HeadlessApp| {
            app.act(command_click(app, tab));
        }
    };
    let steps: Vec<Step> = vec![
        ("a warm tab joins", Box::new(click(2)), toggle(2)),
        (
            "the reader moves into the other pane",
            Box::new(|app: &HeadlessApp| {
                app.act(UiAction::PaneRead { tab: 3 });
            }),
            r##"{"cmd":"compare","reading":"#3"}"##.into(),
        ),
        ("synced scroll", Box::new(menu("compare.synced")), r#"{"cmd":"compare","sync":"on"}"#.into()),
        (
            "separate scroll",
            Box::new(menu("compare.separate")),
            r#"{"cmd":"compare","sync":"off"}"#.into(),
        ),
        ("the tab read leaves", Box::new(click(3)), toggle(3)),
        ("a cold tab joins", Box::new(click(1)), toggle(1)),
    ];
    for (what, person, line) in steps {
        if what == "a cold tab joins" {
            cold(&twins.person, 1);
            cold(&twins.agent, 1);
            drop((twins.person.heard_events(), twins.agent.heard_events()));
        }
        let before = (twins.person.reader(), twins.person.session_json());
        let (said, agent_said) = twins.both_with(person, &line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert_ne!((twins.person.reader(), twins.person.session_json()), before, "{what}: something changed");
        twins.assert_same(&format!("{what} and {line}"), &[]);
    }
    let comparison = twins.person.session_json()["comparison"].clone();
    assert_eq!(
        comparison["tabs"],
        serde_json::json!([2, 1]),
        "the cold tab's beside the one left: {comparison}"
    );
}

/// Failure (R2.3, C5e): a ⌘-click that would make a fourth pane says why in an agent's words, or says nothing, or a move into a pane already read, or synced
/// scroll with nothing side by side, says something the window never said. Only the first says why, in the session's words, and nothing changes.
#[test]
fn only_a_fourth_pane_says_why_it_wasnt_added() {
    let disk = Disk::new();
    let files: Vec<_> = (0..4).map(|at| disk.pdf(&format!("f{at}.pdf"), 3)).collect();
    let app = disk.start();
    for file in &files {
        app.open(file);
    }
    let notice = |app: &HeadlessApp| crate::lock(&app.core().window.action_notice).clone();
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.settle();
    assert_eq!(notice(&app), None, "no comparison to sync: nothing said");
    for tab in [1, 2] {
        app.act(command_click(&app, tab));
        app.settle();
    }
    app.hear_events();
    app.act(UiAction::PaneRead { tab: 2 });
    app.settle();
    assert_eq!(notice(&app), None, "the pane read already: nothing said");
    assert!(app.heard_events().is_empty(), "and nothing announced");
    let before = app.session_json()["comparison"].clone();
    app.act(command_click(&app, 3));
    app.settle();
    assert_eq!(notice(&app).as_deref(), Some(recto_session::compare::CompareError::TooMany.message()));
    assert_eq!(app.session_json()["comparison"], before, "the comparison stands");
    assert!(names(&app.heard_events()).is_empty(), "and nothing's announced");
}
