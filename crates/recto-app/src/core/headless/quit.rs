//! **A quit, as the app's life steps it** (step R1.4): its freeze, its write, its answers, and what the updater is told of each, on the app's own core.

use crate::queue::CoreCall;
use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{OLD, UpdateBed};
use recto_update::saved::{Probation, Saved};
use std::sync::atomic::Ordering;

/// An installed copy of [`OLD`] whose run is being watched, as one is in its first minutes after an update.
fn watched_copy() -> (Disk, UpdateBed) {
    let disk = Disk::new();
    disk.install_app(OLD);
    Saved {
        proven: Some(OLD.to_owned()),
        probation: Some(Probation {
            version: OLD.to_owned(),
            since_ms: recto_update::wall_ms(),
            running_since_ms: None,
            early_ends: 0,
        }),
        ..Saved::default()
    }
    .write(&disk.update_files().saved);
    (disk, UpdateBed::new())
}

/// The state folder's own part that the record goes in, made so it can't be written, or writable again.
#[cfg(unix)]
fn record_folder_writable(app: &HeadlessApp, writable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let folder = app.core().saver.store.main_folder();
    std::fs::create_dir_all(&folder).expect("the record's folder");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(if writable { 0o700 } else { 0o500 })).expect("the record's folder");
}

/// Failure (63cc07b, 2026-11-11): a quit whose state couldn't be written, with edits it would lose, answers go and loses them; or Quit Anyway counts as a run that
/// ended early, so a good version that once couldn't write its state is sent back; or what a person does stays held back while the app asks; or Stay leaves the
/// run unwatched, so a crash after it isn't counted. The record's folder made unwritable under an owner's edit: the quit asks, the run's no longer watched and
/// the app takes input; Stay, and it's watched again; asked again and Quit Anyway, and the next launch counts no early end.
#[cfg(unix)]
#[test]
fn quit_anyway_isnt_an_early_end_and_stay_is_watched_again() {
    let (disk, bed) = watched_copy();
    let files = disk.update_files();
    let probation = || Saved::read(&files.saved).probation.expect("watched");
    let doc = disk.pdf("edited.pdf", 3);
    let app = disk.builder().update(&bed).start();
    app.open(&doc);
    app.send(CoreCall::Rotate(1));
    app.settle();
    assert!(probation().running_since_ms.is_some(), "this run's watched");
    record_folder_writable(&app, false);
    let asked = app.ask_to_quit();
    assert!(!asked.go && !asked.quit_button.is_empty(), "the edits couldn't be kept, so it asks: {asked:?}");
    assert_eq!(probation().running_since_ms, None, "asked for, so not an early end, whatever's answered");
    assert!(!app.core().core.frozen.load(Ordering::SeqCst), "what a person does is taken while it asks");
    app.stay();
    assert!(probation().running_since_ms.is_some(), "stayed: watched again, so a crash from here counts");
    assert!(!app.ask_to_quit().go, "asked again");
    record_folder_writable(&app, true);
    let disk = app.quit_anyway();
    let app = disk.builder().update(&bed).start();
    assert_eq!(probation().early_ends, 0, "Quit Anyway isn't an early end");
    drop(app.quit());
}
