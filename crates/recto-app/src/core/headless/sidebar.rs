//! The sidebar hidden by its button, and each tab's layout, across a quit and a hand-off whose state comes in late.

use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{Installed, NEW, OLD, UpdateBed};
use recto_agent::Level;
use recto_platform::slow::Seam;
use serde_json::{Value, json};

/// What's to come back: the sidebar hidden, the tab read in two-up, on page 6, and each tab's own layout, as the window and the record have them.
fn as_left(app: &HeadlessApp, said: &str) {
    let reader = app.reader();
    assert!(reader.sidebar_hidden, "{said}: the sidebar's still hidden");
    assert_eq!(reader.layout, "two-up", "{said}: the tab read has its layout");
    assert_eq!(reader.place, "6 of 9", "{said}: on its page");
    let session = app.session_json();
    let layouts: Vec<&Value> = session["tabs"].as_array().into_iter().flatten().map(|tab| &tab["layout"]).collect();
    assert_eq!(layouts, [&json!("two-up-cover"), &json!("two-up")], "{said}: each tab's own layout");
    assert_eq!(session["view"]["sidebar_hidden"], true, "{said}: and the record says it's hidden");
}

/// Failure (the slow stage's `the_hidden_sidebar_and_each_tabs_layout_come_back`): the sidebar hidden by its button comes back shown, or a tab's layout as
/// another tab's, when the state comes in late, after a quit or a hand-off. After a quit: the state's read and the window's remake held, and let through in
/// that order, and it's all back on the window made from the state. After a hand-off: what this copy is and the state both held, and let through, and it's back
/// on the window that waited for it. Shown again by its button after that.
#[test]
fn the_hidden_sidebar_and_each_tabs_layout_come_back_when_the_state_is_late() {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let (one, two) = (disk.pdf("one.pdf", 9), disk.pdf("two.pdf", 9));
    let app = disk.builder().update(&bed).start();
    app.open(&one);
    app.recto(Level::Control, &["view", "--layout", "two-up-cover"]).expect("a layout");
    app.open(&two);
    app.recto(Level::Control, &["view", "--layout", "two-up", "--sidebar", "off"])
        .expect("a layout, and the sidebar hidden");
    app.recto(Level::Control, &["goto", "6"]).expect("goto");
    app.settle();
    as_left(&app, "before");
    let (_, disk) = app.quit();
    let app = disk.builder().update(&bed).hold(&[Seam::State, Seam::Remake]).start();
    assert!(!app.reader().sidebar_hidden, "the window up without the state has the sidebar");
    app.let_state_in();
    app.let_through(Seam::Remake, 1);
    app.settle();
    assert!(app.core().life.remade(), "the window made again from the state");
    as_left(&app, "after a quit");
    recto_control::levels::store(app.disk().paths(), Level::Full).expect("the level's file");
    bed.releases.offer(NEW);
    app.agent(Level::Full, r#"{"cmd":"update_check"}"#).expect("a check");
    app.agent(Level::Full, r#"{"cmd":"update_install"}"#).expect("an install");
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).hold(&[Seam::Launch, Seam::State]).start();
    app.let_through(Seam::Launch, 1);
    app.wait_until("the state's read to wait at its seam", || app.waiting(Seam::State) > 0);
    app.let_state_in();
    as_left(&app, "after a hand-off");
    app.core().toggle_sidebar();
    app.settle();
    assert!(!app.reader().sidebar_hidden, "its button shows it again");
}
