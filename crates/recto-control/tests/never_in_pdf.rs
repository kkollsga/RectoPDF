//! **A history never reaches a PDF** (owner, 2026-11-07: "you definitely don't want history to leak into the pdf file"). SPEC decision 25: "a test proves it by searching a saved file's bytes for marked text that was only ever in the history".
//!
//! On the real path: the delete goes through `recto_core::save`'s rewrite, and around it the real steps a save takes, `begin_save`, `keep_pages_of` in the hook before the move, `save_prepared` and `saved`.

use recto_control::record_store::{Flavor, RecordStore};
use recto_control::state::StatePaths;
use recto_core::save::{self, AddedFile, PageRef, SaveHooks, SaveRequest, Target, Writer};
use recto_session::{Author, DiskLoader, Session};
use recto_testpdf::{Page, pages, text_lines};
use std::path::{Path, PathBuf};

const DELETED_TEXT: &str = "HISTORYMARKDELETEDPAGE";
const UNDONE_FILE_TEXT: &str = "HISTORYMARKUNDONEFILE";
const UNDONE_FILE_NAME: &str = "historymarkaddedname.pdf";

fn found(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle.as_bytes())
}

/// What the agent's save does around the writing: keeps the pages the history can put back, from the file before it's replaced, and notes the save with them.
struct Keeping<'a> {
    session: &'a mut Session<DiskLoader>,
    store: &'a RecordStore,
    tab: recto_session::TabId,
    file: &'a Path,
    needed: Vec<u32>,
    kept: Option<recto_session::edit::AddedSource>,
}

impl SaveHooks for Keeping<'_> {
    fn prepared(&mut self, _written: &Path, stamp: recto_platform::FileStamp) -> std::io::Result<()> {
        self.kept = self.store.keep_pages_of(self.file, stamp, &self.needed);
        self.session.save_prepared(self.tab, stamp, self.kept.as_ref());
        Ok(())
    }
}

/// Failure: text from a removed page, a PDF added and undone, or the journal itself reaches a saved file. They're all in the history first, as this checks, so it's the saving that keeps them out, and undo past the save still has them.
#[test]
fn a_saved_pdf_has_nothing_of_the_history_in_it() {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-never-in-pdf-");
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
    assert!(
        std::fs::read(&file).is_ok_and(|bytes| found(&bytes, DELETED_TEXT)),
        "the mark is in the file as it was"
    );

    // The save, on the real path.
    let snapshot = session.begin_save(tab, false).unwrap().expect("there's something to save");
    let order: Vec<PageRef> = snapshot
        .slots
        .iter()
        .map(|slot| PageRef {
            source: slot.source,
            page: slot.page,
            quarter_turns: slot.quarter_turns,
        })
        .collect();
    let added_files: Vec<AddedFile<'_>> = snapshot
        .added
        .iter()
        .map(|added| AddedFile {
            path: &added.path,
            page_count: added.page_count,
            stamp: added.stamp,
        })
        .collect();
    let request = SaveRequest {
        path: &snapshot.path,
        stamp: snapshot.stamp,
        file_pages: snapshot.file_pages,
        pages: &order,
        added: &added_files,
    };
    let needed = session.own_pages_needed(tab);
    assert_eq!(needed, [1], "the history can put back the deleted page, and only that");
    let mut hooks = Keeping {
        session: &mut session,
        store: &store,
        tab,
        file: &file,
        needed,
        kept: None,
    };
    let outcome = save::save(&request, Target::Same, None, &mut hooks).unwrap();
    let kept = hooks.kept.take().expect("a small page fits");
    assert_eq!(outcome.writer, Writer::Rewrite);
    assert_eq!(session.saved(tab, outcome.stamp.unwrap(), outcome.pages, None, Some(kept.clone())), 0);
    session.end_save(tab);
    let work = session.journal_work();
    let (outcome, failed) = store.write_journals(&work, false);
    assert!(failed.is_none());
    session.journal_outcome(outcome);

    let saved = std::fs::read(&file).unwrap();
    for mark in [
        DELETED_TEXT,
        UNDONE_FILE_TEXT,
        UNDONE_FILE_NAME,
        "kept-",
        "journal-",
        "recto-journal",
        "history",
    ] {
        assert!(!found(&saved, mark), "{mark} reached the saved PDF");
    }
    assert!(found(&saved, "first page") && found(&saved, "third page"), "and it's the document");

    // And it's all still in the history, not lost: the file kept holds the page, and only that page, and undo past the save brings it back from there.
    let kept_bytes = std::fs::read(&kept.path).unwrap();
    assert!(found(&kept_bytes, DELETED_TEXT));
    assert!(
        !found(&kept_bytes, "first page") && !found(&kept_bytes, "third page"),
        "only the page that was removed is kept"
    );
    assert!(!session.is_edited(tab));
    assert!(session.undo_edit(tab), "the delete");
    assert!(session.is_edited(tab));
    let back = session.page_source(tab, 1).unwrap();
    assert_eq!(
        (back.path, back.page),
        (kept.path.to_path_buf(), 0),
        "page 1 of the kept copy, which is the page that was removed"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
