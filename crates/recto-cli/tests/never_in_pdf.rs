//! **A history never reaches a PDF** (owner, 2026-11-07: "you definitely don't want history to leak into the pdf file"). SPEC decision 25: "a test proves it by searching a saved file's bytes for marked text that was only ever in the history".
//!
//! Save isn't built, so this has a stand-in for it: it writes what [`Session::page_source`] says the document is, and nothing else, with lopdf, then goes through the real steps around a save, `save_prepared`, `keep_pages_of` and `saved`. **The Save worker keeps this test, on its real path:** `docs/PLAN.md` has that.

use recto_control::record_store::{Flavor, RecordStore};
use recto_control::state::StatePaths;
use recto_platform::file_stamp;
use recto_session::{Author, DiskLoader, Session};
use recto_testpdf::{Page, pages, text_lines};
use std::path::{Path, PathBuf};

const DELETED_TEXT: &str = "HISTORYMARKDELETEDPAGE";
const UNDONE_FILE_TEXT: &str = "HISTORYMARKUNDONEFILE";
const UNDONE_FILE_NAME: &str = "historymarkaddedname.pdf";

fn found(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle.as_bytes())
}

/// The stand-in for Save's writing: only the pages the document has, from where it says they are. It takes a rewrite, as removing has to.
fn write_what_the_document_is(session: &Session<DiskLoader>, tab: recto_session::TabId, source: &Path, to: &Path) -> u32 {
    let mut document = lopdf::Document::load(source).unwrap();
    let wanted: Vec<u32> = (0..session.pages_in(tab)).map(|at| session.page_source(tab, at).unwrap()).inspect(|page| assert_eq!(page.path, source, "this test's document is all from one file")).map(|page| page.page + 1).collect();
    let all = u32::try_from(document.get_pages().len()).unwrap();
    let delete: Vec<u32> = (1..=all).filter(|page| !wanted.contains(page)).collect();
    document.delete_pages(&delete);
    document.prune_objects();
    document.save(to).unwrap();
    u32::try_from(wanted.len()).unwrap()
}

/// Failure: text from a removed page, a PDF added and undone, or the journal itself reaches a saved file. They're all in the history first, as this checks, so it's the saving that keeps them out, and undo past the save still has them.
#[test]
fn a_saved_pdf_has_nothing_of_the_history_in_it() {
    let root = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-never-in-pdf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("state");
    let paths = StatePaths::overridden(&state).unwrap();
    let store = RecordStore::open(&paths, Flavor::App);
    let file: PathBuf = root.join("doc.pdf");
    let body = |text: &str| Page::new(text_lines(&[text]));
    std::fs::write(&file, pages(&[body("first page"), body(DELETED_TEXT), body("third page")])).unwrap();
    let added = root.join(UNDONE_FILE_NAME);
    std::fs::write(&added, pages(&[body(UNDONE_FILE_TEXT)])).unwrap();

    let mut session = Session::new(DiskLoader);
    session.set_journaling(true);
    let tab = session.open(&file).unwrap();
    session.edit_pages_as(tab, Author::Owner, |edits| edits.delete(1, 1)).unwrap();
    session.insert_pdf(tab, 0, &added).unwrap();
    assert!(session.undo_edit(tab), "so the added PDF is only ever in the history");

    let journal = |name: &str| std::fs::read(state.join("session").join("journals").join(name)).unwrap();
    let work = session.journal_work();
    let name = work.appends[0].name.clone();
    let (outcome, failed) = store.write_journals(&work, false);
    assert!(failed.is_none());
    session.journal_outcome(outcome);
    assert!(found(&journal(&name), UNDONE_FILE_NAME), "the mark is in the history");
    assert!(std::fs::read(&file).is_ok_and(|bytes| found(&bytes, DELETED_TEXT)), "the mark is in the file as it was");

    // The save, step by step.
    let temp = root.join("doc.saving");
    let written = write_what_the_document_is(&session, tab, &file, &temp);
    let target = file_stamp(&temp).unwrap();
    let kept = store.keep_pages_of(&file, target, 3).expect("a small file fits");
    session.save_prepared(tab, target, Some(&kept));
    std::fs::rename(&temp, &file).unwrap();
    assert_eq!(session.saved(tab, file_stamp(&file).unwrap(), written, None, Some(kept.clone())), 0);
    let work = session.journal_work();
    let (outcome, failed) = store.write_journals(&work, false);
    assert!(failed.is_none());
    session.journal_outcome(outcome);

    let saved = std::fs::read(&file).unwrap();
    for mark in [DELETED_TEXT, UNDONE_FILE_TEXT, UNDONE_FILE_NAME, "kept-", "journal-", "recto-journal", "history"] {
        assert!(!found(&saved, mark), "{mark} reached the saved PDF");
    }
    assert!(found(&saved, "first page") && found(&saved, "third page"), "and it's the document");

    // And it's all still in the history, not lost: the file kept holds the page, and undo past the save brings it back from there.
    assert!(found(&std::fs::read(&kept.path).unwrap(), DELETED_TEXT));
    assert!(!session.is_edited(tab));
    assert!(session.undo_edit(tab), "the delete");
    assert!(session.is_edited(tab));
    assert_eq!(session.page_source(tab, 1).unwrap().path, kept.path.to_path_buf());
    std::fs::remove_dir_all(&root).unwrap();
}
