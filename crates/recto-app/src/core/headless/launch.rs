//! Launching and quitting, with nothing open.

use crate::testkit::Heard;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_platform::slow::Seam;

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

/// Every file under a folder, with its length and when it last changed.
fn files_under(folder: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, (u64, std::time::SystemTime)> {
    let mut found = std::collections::BTreeMap::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                folders.push(entry.path());
            } else {
                found.insert(entry.path(), (meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
            }
        }
    }
    found
}

/// Failure: the kit's kill writes what a real kill can't, so a test of what survives a crash passes on writes that never happen: sleep files waiting to be
/// written, or the record, after the moment of the kill. Two tabs back from a quit, more opened with every sleep file's write held at its seam, then a kill:
/// once what was held has gone on and every thread has let go, the state folder is as it was at the kill, byte for byte in length and time.
#[test]
fn a_kill_writes_nothing_after_it() {
    let disk = Disk::new();
    let docs: Vec<_> = (0..6).map(|at| disk.pdf(&format!("k{at}.pdf"), 2)).collect();
    let app = disk.start();
    app.open(&docs[0]);
    app.open(&docs[1]);
    let (_, disk) = app.quit();
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    for doc in &docs[2..] {
        app.open(doc);
    }
    app.wait_until("sleep files waiting to be written", || app.waiting(Seam::SleepWrite) > 0);
    let root = app.disk().paths().root.clone();
    let at_the_kill = files_under(&root);
    let disk = app.kill();
    assert_eq!(files_under(&root), at_the_kill, "nothing written after the kill");
    drop(disk);
}

/// Failure: a state read late isn't taken in as the window has it, or the kit can't put the late seams in the order a slow machine does: the window up without
/// the state, the state in after it, the window made again from it after that. With the state's read and the window's remake both held: the host's told once
/// and shows the home screen; let in, the state's read but nothing's remade yet; the remake let through, the window's made again from the state and the tab's
/// back.
#[test]
fn a_late_state_and_a_late_remake_come_in_the_order_theyre_let_through() {
    let disk = Disk::new();
    let doc = disk.pdf("late.pdf", 3);
    let app = disk.start();
    app.open(&doc);
    let (_, disk) = app.quit();
    let app = disk.builder().hold(&[Seam::State, Seam::Remake]).start();
    assert_eq!(app.listener().count(&Heard::RestoreReady), 1, "up without the state");
    assert!(!app.core().state_read() && app.tabs().is_empty() && app.reader().home);
    app.let_state_in();
    assert!(app.core().state_read(), "the state's in");
    assert_eq!(app.listener().count(&Heard::RestoreReady), 1, "and the remake is held");
    assert!(!app.core().life.shown_with_state());
    app.let_through(Seam::Remake, 1);
    assert_eq!(app.listener().count(&Heard::RestoreReady), 2, "told again, late");
    assert!(app.core().life.shown_with_state(), "the window made again from the state");
    assert_eq!(app.record_json()["remade"], true, "and said to have been: {}", app.record_json());
    assert_eq!(app.tabs().iter().map(|row| row.text.as_str()).collect::<Vec<_>>(), ["late"]);
    drop(app.quit());
}

/// Failure (CI, main at faa9180: "⌘F gives the field the keyboard"; the slow stage's `what_is_done_before_a_late_state_comes_in_isnt_lost`): ⌘F and typing
/// in the moment a window was up without the state, read late, were taken by that window and lost when it was made again from the state. What a person does
/// waits for the window made from the state: the state's read and the remake held, the state let in, and ⌘F is neither enabled nor taken, and Find stays as the
/// state has it; the remake let through, it's enabled and taken, Find opens with what's typed, and the place is the state's. (That
/// the field then has the keyboard is the window's, held by the app test's normal-tier run.)
#[test]
fn what_is_done_before_the_window_is_made_from_a_late_state_waits_for_it() {
    use crate::queue::CoreCall;
    let disk = Disk::new();
    let doc = disk.pdf("alpha.pdf", 20);
    let app = disk.start();
    app.open(&doc);
    app.agent(recto_agent::Level::Control, r#"{"cmd":"goto","page":9}"#).expect("goto");
    app.settle();
    let (_, disk) = app.quit();
    let app = disk.builder().hold(&[Seam::State, Seam::Remake]).start();
    app.let_state_in();
    assert!(app.core().state_read() && !app.core().life.shown_with_state(), "the state's in, and the window not yet made from it");
    assert!(!app.core().menu_item_enabled("find.open".into()), "⌘F isn't enabled on the window that's to go");
    app.core().perform("find.open".into());
    app.settle();
    assert_eq!(app.session_json()["find"]["open"], false, "nor taken there: {}", app.session_json()["find"]);
    app.let_through(Seam::Remake, 1);
    app.settle();
    assert!(app.core().menu_item_enabled("find.open".into()), "on the window made from the state, it is");
    app.core().perform("find.open".into());
    app.send(CoreCall::Find(crate::find::FindCall::Typed("words".into())));
    app.settle();
    let session = app.session_json();
    assert_eq!(session["find"]["open"], true, "{}", session["find"]);
    assert_eq!(session["find"]["draft"], "words", "what was typed: {}", session["find"]);
    assert_eq!(session["tabs"][0]["page"], 8, "and the place from the state");
}
