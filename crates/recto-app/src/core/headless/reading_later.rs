//! A person's command on the session thread, waiting for a file to be read off it (R2.3, `core/person.rs`).

use crate::testkit::headless::HeadlessApp;
use std::sync::atomic::Ordering;

/// Whether a case's command has done what it does.
type Done<'a> = &'a dyn Fn(&HeadlessApp) -> bool;

/// Failure: a person's command that needs a file read it on the session thread, so a file on a dead volume (held here at the read) stops the session thread
/// answering anything: the next tap, a look, the window's pulls. With the read held, the session thread takes a later call and publishes it, nothing of the
/// command waiting has happened yet, and once the read's let go it has done what it does: a tap of a cold tab, and an open of a new file. (A flush waits for
/// the reads before it, by its promise, so what shows the thread isn't held is a later call taken.)
#[test]
fn the_session_thread_answers_while_a_persons_command_waits_for_its_read() {
    let app = HeadlessApp::start();
    let files: Vec<_> = (0..6).map(|at| app.disk().pdf(&format!("f{at}.pdf"), 3)).collect();
    for file in &files {
        app.open(file);
    }
    app.settle();
    // Without sleep files, a cold tab's waking reads its PDF.
    app.core().host.session.with_session(|session| session.dormancy_mut().set_sleep_files(false));
    let new = app.disk().pdf("new.pdf", 3);
    let inner = app.core();
    let open = format!(r#"{{"cmd":"open","path":{:?}}}"#, new.display().to_string());
    let cases: [(&str, &str, Done<'_>); 2] = [
        ("a tap of a cold tab", r##"{"cmd":"tap","tab":"#1"}"##, &|app| app.reader().shown_tab == 1),
        ("an open of a new file", &open, &|app| app.tabs().len() == 7),
    ];
    for (what, line, done) in cases {
        assert!(!done(&app), "{what}: not done before");
        inner.life.hold_reads(true);
        app.person(line);
        app.wait_until("the read to be under way", || inner.life.reads_running() > 0);
        let paper = if app.reader().paper == "dark" { "white" } else { "dark" };
        app.person(&format!(r#"{{"cmd":"view","paper":"{paper}"}}"#));
        app.wait_until("a later call taken while the read is held", || app.reader().paper == paper);
        assert!(inner.life.reads_running() > 0, "{what}: with the read still held");
        assert!(!done(&app), "{what}: nothing done before the read's in");
        inner.life.hold_reads(false);
        app.settle();
        assert!(done(&app), "{what}: done once it's in");
    }
}

/// Failure (review C8, for the person's commands): past a hand-off's last look, a person's command is taken, and lost, as what's on disk is what comes back:
/// one the session thread runs (the paper) or one that runs on a thread of its own (a save, which would write the file). Neither is taken while it's frozen,
/// and both are once it isn't.
#[test]
fn a_persons_command_isnt_taken_during_a_hand_off_on_either_thread() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("one.pdf", 3);
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"all","degrees":90}"##);
    app.settle();
    let before = std::fs::read(&file).expect("the file");
    let paper = app.reader().paper;
    app.core().core.frozen.store(true, Ordering::SeqCst);
    app.person(r#"{"cmd":"view","paper":"dark"}"#);
    app.person(r##"{"cmd":"save","tab":"#1"}"##);
    app.settle();
    assert_eq!(app.reader().paper, paper, "the paper's as it was");
    assert_eq!(std::fs::read(&file).expect("the file"), before, "and nothing was saved");
    assert!(app.reader().edited);
    app.core().core.frozen.store(false, Ordering::SeqCst);
    app.person(r#"{"cmd":"view","paper":"dark"}"#);
    app.person(r##"{"cmd":"save","tab":"#1"}"##);
    app.wait_until("the save", || !app.reader().edited);
    assert_eq!(app.reader().paper, "dark");
    assert_ne!(std::fs::read(&file).expect("the file"), before, "saved");
}
