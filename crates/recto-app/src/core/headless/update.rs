//! **Updates, on the app's own core** (step R1.4): the updater joined to the app as the window has it, its look at what's in flight, the hand-off's freeze, write and
//! read back, and the launch that comes up after it, with the release server and the installer stood in for by [`crate::testkit::update_bed`]. Each test names the
//! app-launching test in `recto-cli`'s `restore::update` it stands in for. What only the real thing has (the zip, codesign, the installer process and the swap on a
//! real volume, Launch Services, the window) stays there.

use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{Installed, NEW, OLD, UpdateBed};
use recto_agent::Level;
use serde_json::Value;

/// An installed copy of [`OLD`] on a fresh disk, with one document open at a page, and the bed it updates from.
fn running_with_a_document(page: u32) -> (HeadlessApp, UpdateBed) {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let file = disk.pdf("alpha.pdf", 20);
    let app = disk.builder().update(&bed).start();
    app.open(&file);
    app.agent(Level::Control, &format!(r#"{{"cmd":"goto","page":{page}}}"#)).expect("goto");
    app.settle();
    (app, bed)
}

/// An agent's command at Full, which an update's need, and its answer.
fn full(app: &HeadlessApp, line: &str) -> Value {
    app.agent(Level::Full, line).unwrap_or_else(|error| panic!("{line}: {error:?}"))
}

/// The page the first tab is on, counted from 0, as the record says.
fn page(app: &HeadlessApp) -> Value {
    app.session_json()["tabs"][0]["page"].clone()
}

/// Failure: an update by hand doesn't hand off and come back as it was: the plan isn't written for the installer, the app isn't told to quit, or the launch on
/// the new version doesn't find the session it left, say it updated, or keep the old version. The whole path through the app's core, on stand-ins for the server
/// and the installer. (What `an_update_in_the_middle_of_things_changes_nothing` holds on the real app, with the real swap, stays there.)
#[test]
fn an_update_by_hand_hands_off_and_comes_back_as_it_was() {
    let (app, bed) = running_with_a_document(9);
    let before = page(&app);
    bed.releases.offer(NEW);
    assert_eq!(full(&app, r#"{"cmd":"update_check"}"#)["status"], "available");
    assert_eq!(full(&app, r#"{"cmd":"update_install"}"#)["started"], true);
    let (plan, disk) = app.handed_off(&bed);
    assert_eq!((plan.from.as_str(), plan.to.as_str()), (OLD, NEW), "{plan:?}");
    assert_eq!(plan.handed_off_tabs, Some(1), "the tabs as written and read back");
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert_eq!(app.disk().kept_version().as_deref(), Some(OLD));
    assert_eq!(app.update_status()["status"], "updated", "{}", app.update_status());
    assert_eq!(app.update_status()["version"], NEW);
    assert_eq!(page(&app), before, "the session as it was");
}
