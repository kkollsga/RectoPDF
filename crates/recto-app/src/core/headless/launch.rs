//! Launching and quitting, with nothing open.

use crate::testkit::Heard;
use crate::testkit::headless::HeadlessApp;

/// Failure: the core can't be started on what a host gives it, or a quit leaves something holding it, so a second launch on the same state folder can't take the
/// record's lock, or comes back with something other than an empty app: the window shows a tab, or the record says something was written that wasn't.
#[test]
fn an_empty_app_starts_quits_and_starts_again() {
    let app = HeadlessApp::start();
    assert!(app.reader().home, "an empty app shows its home screen");
    assert!(app.tabs().is_empty());
    assert_eq!(app.listener().count(&Heard::RestoreReady), 1, "the window's told once, with the state in time");
    let (answer, disk) = app.quit();
    assert!(answer.go, "nothing to lose, so the quit goes: {answer:?}");
    let again = disk.start();
    assert!(again.reader().home);
    assert!(again.tabs().is_empty());
    assert_eq!(again.session_json()["tabs"], serde_json::json!([]), "{}", again.session_json());
    drop(again.quit());
}

/// Failure: a tab opened in the headless app isn't the window's (no tab row, the home screen still up), or a quit doesn't write it, so the next launch on the same
/// disk comes back empty.
#[test]
fn a_file_opened_comes_back_after_a_quit() {
    let app = HeadlessApp::start();
    let pdf = app.disk().pdf("one.pdf", 3);
    app.open(&pdf);
    assert_eq!(app.tabs().iter().map(|row| row.text.as_str()).collect::<Vec<_>>(), ["one"]);
    assert!(!app.reader().home);
    assert_eq!(app.reader().place, "1 of 3");
    let (answer, disk) = app.quit();
    assert!(answer.go);
    let again = disk.start();
    assert_eq!(again.tabs().iter().map(|row| row.text.as_str()).collect::<Vec<_>>(), ["one"]);
    assert_eq!(again.reader().place, "1 of 3");
    drop(again.quit());
}
