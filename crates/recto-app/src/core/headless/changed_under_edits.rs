//! **A tab's file changed on disk by another program while it has unsaved edits,** on the app's own core with real files. Until the page matcher exists
//! (owner, 2026-11-13), an edit names its page by its number in the version it was made on, so what holds here is that nothing is written, the edits are kept,
//! and the words say what's true: they can't be saved, here or as a copy, while the other version is in place.

use crate::queue::CoreCall;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use std::path::{Path, PathBuf};

/// A PDF of `pages` pages whose text says `who` and the page.
fn pdf_bytes(who: &str, pages: usize) -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (1..=pages)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

/// Another program's save: a new file renamed over the old one.
fn replace(path: &Path, bytes: &[u8]) {
    let new = path.with_extension("incoming");
    std::fs::write(&new, bytes).expect("the new version");
    std::fs::rename(&new, path).expect("put in place");
}

/// The earlier version put back as it was, bytes and modification time, as a backup tool restores it: a new file, under a new number.
fn put_back(path: &Path, bytes: &[u8], modified: std::time::SystemTime) {
    let new = path.with_extension("restored");
    std::fs::write(&new, bytes).expect("the old version");
    std::fs::File::options()
        .write(true)
        .open(&new)
        .and_then(|file| file.set_modified(modified))
        .expect("its time");
    std::fs::rename(&new, path).expect("put back");
}

/// The app comes back to the front, so each file showing is looked at.
fn look(app: &HeadlessApp) {
    app.send(CoreCall::LookAtShown);
    app.settle();
}

fn notice(app: &HeadlessApp) -> Option<String> {
    app.tabs().into_iter().next().and_then(|row| row.notice)
}

/// `doc.pdf`, ten pages, open, with page 9 turned by the person.
fn edited() -> (HeadlessApp, PathBuf, Vec<u8>, std::time::SystemTime) {
    let app = HeadlessApp::start();
    let original = pdf_bytes("Mine", 10);
    let file = app.disk().file("doc.pdf", &original);
    let modified = std::fs::metadata(&file).and_then(|meta| meta.modified()).expect("its time");
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("rotated");
    app.settle();
    (app, file, original, modified)
}

/// Failure (B2): the edits made on a file another program then replaced could be saved nowhere, while every word said otherwise: Save's refusal said
/// "Save As keeps both", the agent was told "`recto save-as` keeps both versions", and the person's sheet offered Save As, all of which were refused too. Save
/// As stays refused (the edits would land on the new file's pages, in a new file), and the refusal, the agent's next step, the sheet and the tab's notice say
/// what is so: the edits were made on a version that isn't there, they're kept, nothing's written. Putting that version back, bytes and time, makes them fit
/// again, and then they save.
#[test]
fn edits_on_a_replaced_file_save_nowhere_say_so_and_fit_again_when_its_put_back() {
    let (app, file, original, modified) = edited();
    let theirs = pdf_bytes("Theirs", 5);
    replace(&file, &theirs);
    look(&app);
    let said = notice(&app).expect("the tab says what happened");
    assert!(said.contains("changed on disk") && said.contains("kept"), "{said}");
    assert!(!said.to_lowercase().contains("save as"), "{said}");
    let copy = file.with_file_name("copy.pdf");
    for words in [vec!["save"], vec!["save-as", "--out", copy.to_str().unwrap()]] {
        let refused = app.recto(Level::Full, &words).expect_err("refused");
        assert_eq!(refused.code, "file_changed", "{words:?}: {refused:?}");
        for text in [&refused.message, &refused.next] {
            let text = text.to_lowercase();
            assert!(
                !text.contains("keeps both") && !text.contains("save-as") && !text.contains("save as keeps"),
                "{words:?}: {text}"
            );
        }
        assert!(refused.message.contains("Nothing was written"), "{refused:?}");
    }
    assert!(!copy.exists(), "no copy was made");
    assert_eq!(std::fs::read(&file).expect("the file"), theirs, "and the other program's version is as it was");
    // The person's Save: the same refusal, with no Save As offered.
    app.core().perform("file.save".into());
    app.settle();
    let problem = app.core().tools.save_ui.problem().expect("the person's told");
    assert_eq!(problem.save_as_title, None, "Save As isn't offered, as it would be refused: {problem:?}");
    assert!(!problem.message.contains("Save As"), "{problem:?}");
    assert!(app.reader().edited, "the edits are still here");
    // The version they were made on, put back as it was: they fit it, the tab says nothing, and they save.
    put_back(&file, &original, modified);
    look(&app);
    assert_eq!(notice(&app), None);
    app.recto(Level::Full, &["save"]).expect("saved");
    app.settle();
    let saved = std::fs::read(&file).expect("the file");
    assert_ne!(saved, original, "the turn went in");
    assert!(saved.starts_with(&original), "appended to the version they were made on");
}

/// What Find shows: its rows, its status, and whether it's still looking.
fn find_shown(app: &HeadlessApp) -> (u32, String, bool) {
    let view = crate::core::find::view(app.core());
    (view.row_count, view.status, view.running)
}

/// Failure (B5): Find's hits are about a version of the file, but on an edited tab what they were about was the edits' stamp, which never moves, so hits found
/// on the earlier version stayed listed over the new one, never found again. Find can't say which page of this file a match is on until the edits fit it (the
/// page matcher's work), so it says so, plainly, for the person and an agent alike: the hits go when the change is taken in, the field says why, and an
/// agent's `find` is refused in the same words with the same next step as its save.
#[test]
fn find_on_an_edited_tab_whose_file_changed_says_it_cant_search() {
    let (app, file, original, modified) = edited();
    app.act(crate::core::controls::UiAction::FindTyped { text: "Mine page".into() });
    app.settle();
    assert!(find_shown(&app).0 > 0, "hits on the version the edits were made on: {:?}", find_shown(&app));
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    let (rows, status, running) = find_shown(&app);
    assert_eq!((rows, running), (0, false), "the earlier version's hits are gone: {status}");
    let words = "Find can't search this file: it changed on disk after its unsaved edits were made.";
    assert_eq!(status, words);
    let refused = app.recto(Level::Full, &["find", "Theirs"]).expect_err("refused");
    assert_eq!((refused.code, refused.message.as_str()), ("file_changed", words), "{refused:?}");
    assert!(refused.next.contains("earlier version back"), "{refused:?}");
    // Looking again isn't news, and Find goes on saying the same.
    look(&app);
    assert_eq!(find_shown(&app).1, words);
    // The version the edits were made on, back: Find finds in it again.
    put_back(&file, &original, modified);
    look(&app);
    app.settle();
    let (rows, status, _) = find_shown(&app);
    assert!(rows > 0, "found again: {status}");
}

/// Failure (B5): every return to the front read an edited tab's changed file again and reset its contents' folds, as each look took the change in anew. A look
/// that finds what's already known does nothing: a fold made after the change is taken in stays across looks.
#[test]
fn looking_again_at_an_edited_tab_whose_file_changed_changes_nothing() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &super::files::with_sections(6));
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "2", "--by", "90"]).expect("rotated");
    app.settle();
    replace(&file, &super::files::with_sections(8));
    look(&app);
    let tab = app.session_json()["tabs"][0]["id"].as_u64().expect("a tab");
    app.agent(Level::Control, &format!(r##"{{"cmd":"contents_fold","entry":0,"tab":"#{tab}"}}"##))
        .expect("folded");
    app.settle();
    let folded = app.session_json()["tabs"][0].clone();
    for _ in 0..3 {
        look(&app);
    }
    assert_eq!(app.session_json()["tabs"][0], folded, "nothing about the tab changed");
}
