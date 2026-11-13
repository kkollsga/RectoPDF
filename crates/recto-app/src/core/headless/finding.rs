//! **Find, as the person does it and as an agent does it** (step R2.3, C5g): what each announces, and the person's Find shown equal to the agent's `find`.

use super::twins::names;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;

/// An app with a 30-page file open, `pdf page 1` on page 1 and pages 10 to 19, hearing what's announced from here on.
fn findable() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    app.hear_events();
    app
}

/// Failure (R2.3, C5g, difference 1: an announcement is made when something changed, and only then): ⌘F with Find open already announces `find_changed`,
/// though it only gives the field the keyboard; or an agent's look at Find announces something. The first ⌘F opens Find and is announced, the second isn't,
/// and an agent's `find` with nothing announces nothing.
#[test]
fn opening_find_that_is_open_announces_nothing() {
    let app = findable();
    app.act(UiAction::FindOpen);
    app.settle();
    assert_eq!(names(&app.heard_events()), ["find_changed"], "Find opened");
    app.act(UiAction::FindOpen);
    app.settle();
    assert_eq!(names(&app.heard_events()), Vec::<&str>::new(), "⌘F with Find open");
    let (looked, said) = app.agent_heard(Level::Control, r#"{"cmd":"find"}"#);
    assert!(looked.is_ok(), "{looked:?}");
    app.settle();
    assert!(said.is_empty() && app.heard_events().is_empty(), "an agent's look: {said:?}");
}
