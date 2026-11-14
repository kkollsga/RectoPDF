//! `highlight` through the app core, with no window (`docs/FEATURES.md`, step 10). Made by `tools/new-feature.sh highlight`: each test fails saying
//! what goes in its place. Replace each with tests that name the failure they catch; Find's are `find.rs` and `finding.rs` beside this file.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// FILL IN (headless tier): what the person's control and an agent's command do, through the one executor, read from what the app publishes.
#[test]
fn fill_me_in_highlight_through_the_app_core() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("highlight.pdf", 2);
    app.open(&file);
    let tabs = app.agent(Level::Control, r#"{"cmd":"tabs"}"#).expect("tabs");
    assert_eq!(tabs["count"], 1, "the kit is up: {tabs}");
    panic!("fill me in: `highlight` sent by an agent and by the person's control (`app.act`), and what the window is told");
}

/// FILL IN (window tier): what only the real app shows of it goes into a launch that is already there; the ratchet holds the number of launches.
#[test]
fn fill_me_in_highlight_window_checks_join_a_launch() {
    panic!(
        "fill me in: what only the real app shows of `highlight` (drawing, AppKit), added to a test in crates/recto-cli/tests/app.rs that already launches it; then delete this test"
    );
}

/// FILL IN (shots): the feature once, light and dark, on the contact sheet.
#[test]
fn fill_me_in_highlight_shots_light_and_dark() {
    panic!("fill me in: one step for `highlight` in app/qa/contact-sheet.sh, run, and its sheets looked at; then delete this test");
}
