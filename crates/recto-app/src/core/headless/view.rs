//! **How pages are shown, as the person changes it** (step R2.3, C5a): the paper, the sidebar, the agent icon, a tab's layout and the zoom, each the `view`
//! command an agent sends, run for the person.

use crate::core::controls::UiAction;
use crate::testkit::headless::Disk;

/// The names of the events, in order, for a message.
fn names(events: &[recto_agent::Event]) -> Vec<&str> {
    events.iter().map(recto_agent::Event::name).collect()
}

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
