//! What's selected on the page, across a quit and a launch whose state comes in late.

use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_platform::slow::Seam;

/// What's selected on the tab showing, as the page view has it: its tab and its range.
fn selected(app: &HeadlessApp) -> Option<(u32, recto_index::select::Range)> {
    let (tab, range) = app.core().panes.reading_engine(&app.core().engine).selection().hot()?;
    Some((tab, range?))
}

/// Failure (late-restore-2, and the slow stage's `a_selection_comes_back`): with the state read late, the selection came back on the window that went up
/// without it and went with that window when it was made again from the state (SPEC decision 23 has it as part of what comes back). The state's read and the
/// window's remake both held: let in, the selection's back on the window up without it; the remake let through, it's on the window made from the state, the same
/// characters, and the record says so.
#[test]
fn a_selection_comes_back_on_the_window_made_again_from_a_late_state() {
    let disk = Disk::new();
    let doc = disk.pdf("selected.pdf", 8);
    let app = disk.start();
    app.open(&doc);
    app.agent(Level::Control, r#"{"cmd":"goto","page":2}"#).expect("goto");
    app.settle();
    app.core().perform("edit.select_all".into());
    app.settle();
    let before = selected(&app).expect("select all selects the page");
    let recorded = app.session_json()["tabs"][0]["selection"].clone();
    assert!(!recorded.is_null(), "the record has it: {}", app.session_json()["tabs"][0]);
    let (_, disk) = app.quit();
    let app = disk.builder().hold(&[Seam::State, Seam::Remake]).start();
    app.let_state_in();
    assert_eq!(selected(&app), Some(before), "back on the window up without the state");
    app.let_through(Seam::Remake, 1);
    app.settle();
    assert!(app.core().life.remade(), "the window was made again from the state");
    assert_eq!(selected(&app), Some(before), "and on the window made from it");
    assert_eq!(app.session_json()["tabs"][0]["selection"], recorded, "and in the record");
}
