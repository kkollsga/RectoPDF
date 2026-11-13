//! A person's command on the session thread, waiting for a file to be read off it (R2.3, `core/person.rs`).

use crate::queue::{CoreCall, PersonCommand};
use crate::testkit::headless::HeadlessApp;
use std::sync::atomic::Ordering;

/// Whether a case's command has done what it does.
type Done<'a> = &'a dyn Fn(&HeadlessApp) -> bool;

/// A person's command, as the window will send it.
fn person(app: &HeadlessApp, line: &str) {
    app.send(CoreCall::Run(PersonCommand::input(recto_agent::parse(line).expect("a command"))));
}

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
    app.core().host.session.with_session(|session| session.set_sleep_files(false));
    let new = app.disk().pdf("new.pdf", 3);
    let inner = app.core();
    let open = format!(r#"{{"cmd":"open","path":{:?}}}"#, new.display().to_string());
    let cases: [(&str, &str, Done<'_>); 2] = [
        ("a tap of a cold tab", r##"{"cmd":"tap","tab":"#1"}"##, &|app| app.reader().shown_tab == 1),
        ("an open of a new file", &open, &|app| app.tabs().len() == 7),
    ];
    for (what, line, done) in cases {
        assert!(!done(&app), "{what}: not done before");
        inner.life.qa_hold_reads.store(true, Ordering::SeqCst);
        person(&app, line);
        app.wait_until("the read to be under way", || inner.life.reads_running.load(Ordering::SeqCst) > 0);
        let paper = if app.reader().paper == "dark" { "white" } else { "dark" };
        person(&app, &format!(r#"{{"cmd":"view","paper":"{paper}"}}"#));
        app.wait_until("a later call taken while the read is held", || app.reader().paper == paper);
        assert!(inner.life.reads_running.load(Ordering::SeqCst) > 0, "{what}: with the read still held");
        assert!(!done(&app), "{what}: nothing done before the read's in");
        inner.life.qa_hold_reads.store(false, Ordering::SeqCst);
        app.settle();
        assert!(done(&app), "{what}: done once it's in");
    }
}
