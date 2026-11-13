//! **Every shipped edit against every way of leaving** (PLAN phase 4, the research's rule 3: "no exit path loses work silently"), on the app's own core: a turn,
//! a move, pages added from another PDF, a delete and the rotation of every page, each on a tab of its own; one way of leaving; and back, with every edit as it
//! was, whose it was, its undo and redo, and no PDF changed by a byte. Twins of `recto-cli`'s `restore::exits`, which keeps what only the real app shows.

use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_session::TabId;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The kinds of edit the first release ships.
const KINDS: [&str; 5] = ["turn", "move", "add", "delete", "rotate-all"];

/// A document with one kind of edit on it, and its bytes as they were.
struct Edited {
    kind: &'static str,
    path: PathBuf,
    original: Vec<u8>,
}

fn full(app: &HeadlessApp, words: &[&str]) -> Value {
    app.recto(Level::Full, words).unwrap_or_else(|error| panic!("{words:?}: {error:?}"))
}

fn owner(app: &HeadlessApp, edit: &str) {
    app.core().qa_session_verb("owner_edit", edit).expect("a QA verb").expect("the owner's edit");
}

/// A document per kind, each with that edit, some the owner's and some an agent's, and the PDF the added pages come from.
fn one_of_each(app: &HeadlessApp) -> (Vec<Edited>, Edited) {
    let extra_path = app.disk().pdf("extra.pdf", 2);
    let extra = Edited {
        kind: "source",
        original: std::fs::read(&extra_path).expect("the extra PDF"),
        path: extra_path,
    };
    let mut docs = Vec::new();
    for kind in KINDS {
        let path = app.disk().pdf(&format!("{kind}.pdf"), 4);
        app.open(&path);
        match kind {
            "turn" => owner(app, "rotate:2"),
            "move" => drop(full(app, &["page", "move", "3", "--to", "1"])),
            "add" => drop(full(app, &["page", "insert", extra.path.to_str().expect("a path"), "--at", "2"])),
            "delete" => owner(app, "delete:2"),
            _ => drop(full(app, &["page", "rotate", "all", "--by", "90"])),
        }
        app.settle();
        docs.push(Edited {
            kind,
            original: std::fs::read(&path).expect("the PDF"),
            path,
        });
    }
    (docs, extra)
}

/// Every tab's edits, with whose each was, its undo and redo, by the file it's on: after a crash before the record's gone out, tabs come back from their
/// journals under new numbers, which loses nothing.
fn edits(app: &HeadlessApp) -> BTreeMap<String, Value> {
    let session = app.session_json();
    let by_tab = session["edits_full"].as_object().cloned().unwrap_or_default();
    let edits: BTreeMap<String, Value> = session["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tab| Some((tab["path"].as_str()?.to_owned(), by_tab.get(&tab["id"].to_string())?.clone())))
        .collect();
    assert_eq!(edits.len(), KINDS.len(), "a list per tab: {}", session["edits_full"]);
    edits
}

/// No PDF changed by a byte: nothing of an unsaved edit or its history reached one.
fn untouched(docs: &[Edited], extra: &Edited) {
    for doc in docs.iter().chain([extra]) {
        assert!(std::fs::read(&doc.path).expect("the PDF") == doc.original, "{}: the PDF changed", doc.kind);
    }
}

/// The tab a file's on, by its name.
fn tab_of(app: &HeadlessApp, doc: &Edited) -> TabId {
    let name = doc.path.file_name().and_then(|name| name.to_str()).expect("a name");
    let session = app.session_json();
    let id = session["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .and_then(|tab| tab["id"].as_u64())
        .unwrap_or_else(|| panic!("no tab for {name}"));
    TabId(u32::try_from(id).expect("an id"))
}

/// Back on the same disk, at rest.
fn again(disk: Disk) -> HeadlessApp {
    disk.start()
}

/// Failure (the research's rule 3, and the owner: "It's crucial that app state is perfectly recovered"): an edit of a kind that ships is lost, changes hands or
/// loses its undo across a quit; or a quit writes something into a PDF. (Replaces the app test `every_edit_survives_a_quit`.)
#[test]
fn every_edit_survives_a_quit() {
    let app = HeadlessApp::start();
    let (docs, extra) = one_of_each(&app);
    let before = edits(&app);
    let (answer, disk) = app.quit();
    assert!(answer.go);
    let app = again(disk);
    assert_eq!(edits(&app), before, "after a quit and a launch");
    untouched(&docs, &extra);
}

/// Failure: a crash loses an edit the journal already had, brings one back wrongly, or leaves anything in a PDF. The journals written, as they are once their
/// quiet time is over, then killed outright: each comes back, and no PDF changed. (Replaces the app test `every_edit_survives_a_kill`.)
#[test]
fn every_edit_survives_a_kill() {
    let app = HeadlessApp::start();
    let (docs, extra) = one_of_each(&app);
    journaled(&app);
    let before = edits(&app);
    let app = again(app.kill());
    assert_eq!(edits(&app), before, "after a kill and a launch");
    untouched(&docs, &extra);
}

/// Failure (SPEC decision 24: an update "while the user is using it"): an edit is lost across an update's hand-off. Handed off as `qa handoff now` does it, the
/// state written, forced and read back, the app gone, and each kind comes back in the app that takes over. (Replaces the app test
/// `every_edit_survives_a_hand_off`.) And the host is told to quit once, as a QA run asks twice (`handoff`'s own thread, then `hand_off_quit`), each answered yes.
#[test]
fn every_edit_survives_a_hand_off() {
    let app = HeadlessApp::start();
    let (docs, extra) = one_of_each(&app);
    let before = edits(&app);
    assert!(!app.core().hand_off_quit_now(), "not before it's handed off");
    app.core().hand_off(crate::testkit::headless::WAIT_LIMIT).expect("handed off");
    assert!(app.core().hand_off_quit_now(), "told to quit");
    assert!(app.core().hand_off_quit_now(), "asked again, still quitting");
    assert_eq!(app.listener().count(&crate::testkit::Heard::HandOffQuit), 1, "told once");
    let app = again(app.kill());
    assert_eq!(edits(&app), before, "after a hand-off");
    untouched(&docs, &extra);
}

/// Failure (owner: "When it's on deep sleep, we want its state to be stored on disk, not on memory"): a tab put to sleep loses its edits, they don't survive a
/// crash while it sleeps, or it wakes without them. Every tab but the one showing put to sleep, killed, and each comes back with its edits and wakes with them,
/// undo included. (Replaces the app test `every_edit_survives_sleep_and_a_kill_while_asleep`.)
#[test]
fn every_edit_survives_sleep_and_a_kill_while_asleep() {
    let app = HeadlessApp::start();
    let (docs, extra) = one_of_each(&app);
    let before = edits(&app);
    let showing = tab_of(&app, docs.last().expect("a doc"));
    for doc in &docs[..docs.len() - 1] {
        let slept = full(&app, &["session", "sleep", "--tab", &format!("#{}", tab_of(&app, doc).0)]);
        assert_eq!(slept["dormant"], true, "{}: {slept}", doc.kind);
    }
    assert_eq!(edits(&app), before, "read from their journals, what was in memory");
    let app = again(app.kill());
    assert_eq!(edits(&app), before, "after a kill while asleep");
    for doc in &docs {
        let tab = tab_of(&app, doc);
        app.person(&format!(r##"{{"cmd":"tap","tab":"#{}"}}"##, tab.0));
        app.settle();
        let now = full(&app, &["edits", "--tab", &format!("#{}", tab.0)]);
        assert_eq!(
            (now["edited"].as_bool(), now["can_undo"].as_bool()),
            (Some(true), Some(true)),
            "{}: {now}",
            doc.kind
        );
    }
    app.person(&format!(r##"{{"cmd":"tap","tab":"#{}"}}"##, showing.0));
    app.settle();
    assert_eq!(edits(&app), before, "woken, as they were");
    untouched(&docs, &extra);
}

/// Failure: closing the window, which leaves the app running, drops a tab's edits, or opening it again shows them gone, or they don't outlast the app after
/// that. Closed and opened again by the calls the window makes, then a quit. (Replaces the app test `every_edit_survives_the_window_closing`.)
#[test]
fn every_edit_survives_the_window_closing() {
    let app = HeadlessApp::start();
    let (docs, extra) = one_of_each(&app);
    let before = edits(&app);
    app.core().window_closed();
    app.settle();
    assert!(crate::lock(&app.core().window.app).window_closed);
    assert_eq!(edits(&app), before, "with the window closed");
    app.core().window_opened();
    app.settle();
    assert_eq!(edits(&app), before, "with it open again");
    let (_, disk) = app.quit();
    let app = again(disk);
    assert_eq!(edits(&app), before, "and after a quit");
    untouched(&docs, &extra);
}

/// Failure (the research's rule 3: Folio 1.10.5's printing cleared the edited flag): printing or exporting a tab with unsaved edits saves them, drops them or
/// marks them saved. A print to PDF and a picture of a page from each tab leave every edit as it was, each tab still edited and the window saying so, and no PDF
/// changed. (Replaces the app test `every_edit_survives_printing_and_exporting`.)
#[test]
fn every_edit_survives_printing_and_exporting() {
    let app = HeadlessApp::start();
    let (docs, extra) = one_of_each(&app);
    let before = edits(&app);
    for doc in &docs {
        let tab = tab_of(&app, doc);
        app.person(&format!(r##"{{"cmd":"tap","tab":"#{}"}}"##, tab.0));
        app.settle();
        let printed = app.disk().files().join(format!("{}-printed.pdf", doc.kind));
        full(&app, &["print", "--to-pdf", printed.to_str().expect("a path")]);
        let picture = app.disk().files().join(format!("{}.png", doc.kind));
        full(&app, &["render", "1", "--out", picture.to_str().expect("a path")]);
        app.settle();
        assert!(printed.is_file() && picture.is_file(), "{}: both made", doc.kind);
        assert_eq!(
            full(&app, &["edits", "--tab", &format!("#{}", tab.0)])["edited"],
            true,
            "{}: still edited",
            doc.kind
        );
        assert!(app.reader().edited, "{}: and the window says so", doc.kind);
    }
    assert_eq!(edits(&app), before, "after printing and exporting");
    untouched(&docs, &extra);
}

/// Waits until every tab's edits are in its journal, as a crash needs them to be: the journals' quiet time is over by the clock, and they're written.
fn journaled(app: &HeadlessApp) {
    let journals = app.core().saver.store.main_folder().join("journals");
    let written = || {
        std::fs::read_dir(&journals).is_ok_and(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("journal-"))
                .filter(|entry| entry.metadata().is_ok_and(|meta| meta.len() > 0))
                .count()
                >= KINDS.len()
        })
    };
    app.advance(6_000);
    app.wait_until("every tab's edits in its journal", written);
}
