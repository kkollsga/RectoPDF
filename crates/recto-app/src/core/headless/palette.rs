//! ⌘K's palette, as an agent runs it: `recto palette --run` keeps to each command's level. Twin of `recto-cli`'s app test of the same name.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// Failure: `recto palette --run` runs a row at the palette's own level rather than the command's, so an agent at Control installs an update through it, or one
/// at Read-only changes something; or the row's step is the owner's, not the agent's. (The app test `the_palettes_run_keeps_to_each_commands_level`, which this
/// replaces.)
#[test]
fn the_palettes_run_keeps_to_each_commands_level() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("numbered.pdf", 4);
    app.open(&file);
    let zoomed = app
        .agent(Level::Control, r#"{"cmd":"palette","query":"zoom in","run":true}"#)
        .expect("zoom in at Control");
    assert_eq!(zoomed["ran"]["id"], "view.zoom_in", "{zoomed}");
    let update = app
        .agent(Level::Control, r#"{"cmd":"palette","query":"check for updates","run":true}"#)
        .expect_err("an update's install needs Full");
    assert_eq!(update.code, "permission_denied", "{update:?}");
    let rotated = app
        .agent(Level::Control, r#"{"cmd":"palette","query":"rotate all pages right","run":true}"#)
        .expect("rotate at Control");
    assert_eq!(rotated["ran"]["id"], "rotate.right", "{rotated}");
    app.settle();
    let edits = app.agent(Level::Control, r#"{"cmd":"edits"}"#).expect("edits");
    assert_eq!(edits["edited"], true, "{edits}");
    // Below Full, `discard` throws away only agents' steps: it going through says the step is the agent's, as `page rotate` makes it, and not the owner's.
    app.agent(Level::Control, r#"{"cmd":"discard"}"#).expect("the step is the agent's");
    let listed = app.agent(Level::ReadOnly, r#"{"cmd":"palette","query":"zoom"}"#).expect("listing is reading");
    assert!(listed["items"].as_array().is_some_and(|items| !items.is_empty()), "{listed}");
    app.agent(Level::ReadOnly, r#"{"cmd":"palette","query":"zoom in","run":true}"#)
        .expect_err("running is doing");
}
