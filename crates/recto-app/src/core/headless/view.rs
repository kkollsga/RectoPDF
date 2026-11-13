//! **How pages are shown, as the person changes it** (step R2.3, C5a): the paper, the sidebar, the agent icon, a tab's layout and the zoom, each the `view`
//! command an agent sends, run for the person.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::Disk;

fn menu(id: &str) -> UiAction {
    UiAction::Menu { id: id.into() }
}

/// Failure (R2.3, accident A2): choosing the paper that's chosen already, or showing a sidebar that's showing, tells every watcher the view changed when
/// nothing did. Said once when it changes, and not again.
#[test]
fn a_view_change_is_announced_only_when_something_changed() {
    let app = Disk::new().start();
    app.hear_events();
    app.act(menu("paper.white"));
    app.settle();
    assert_eq!(names(&app.heard_events()), ["view_changed"], "white chosen: a change");
    app.act(menu("paper.white"));
    app.settle();
    let again = app.heard_events();
    assert!(
        !names(&again).contains(&"view_changed"),
        "white again: nothing changed, so nothing's said: {again:?}"
    );
}

/// Failure (R2.3, C5a): the window's paper, sidebar, agent icon, layout or zoom does something other than the `view` an agent sends: another change, another
/// event, or something else kept in the record. Each control, on one app, and its command at Full on another with the same documents: the same shown, said
/// and kept.
#[test]
fn the_windows_view_controls_are_the_agents_view() {
    let twins = Twins::on(&[("a.pdf", 6), ("b.pdf", 6)]);
    for (action, line) in [
        (menu("paper.dark"), r#"{"cmd":"view","paper":"dark"}"#),
        (menu("paper.warm"), r#"{"cmd":"view","paper":"warm"}"#),
        (UiAction::ToggleSidebar, r#"{"cmd":"view","sidebar":"off"}"#),
        (UiAction::ToggleSidebar, r#"{"cmd":"view","sidebar":"on"}"#),
        (menu("view.agent_icon"), r#"{"cmd":"view","agent_icon":"toggle"}"#),
        (menu("layout.two_up"), r#"{"cmd":"view","layout":"two-up"}"#),
        (menu("layout.two_up_cover"), r#"{"cmd":"view","layout":"two-up-cover"}"#),
        (menu("view.zoom_in"), r#"{"cmd":"view","zoom":"in"}"#),
        (
            UiAction::ZoomTyped {
                text: "150,5".into(),
                decimal: ',',
            },
            r#"{"cmd":"view","zoom":"150.5%"}"#,
        ),
        (menu("view.fit"), r#"{"cmd":"view","zoom":"fit"}"#),
        (menu("view.actual_size"), r#"{"cmd":"view","zoom":"actual"}"#),
    ] {
        let what = format!("{action:?} and {line}");
        let before = twins.person.reader();
        let zoomed = twins.person.core().engine.magnification();
        let (person, agent) = twins.both(action, line);
        assert!(
            twins.person.reader() != before || twins.person.core().engine.magnification() != zoomed,
            "{what}: it changed something, so the comparison is of something"
        );
        assert_eq!(person, agent, "{what}: the same said");
        twins.assert_same(&what, &[]);
    }
}
