//! **Every shipped edit against every way of leaving,** on the real app (PLAN phase 4, from the research's rule 3: "no exit path loses work
//! silently"). A quit, a kill, a hand-off, sleep and a kill while asleep, the window closing, and printing and exporting are held in process by `recto-app`'s
//! `core::headless::exits` (2026-11-12); what's here is what goes through the window's own sheet and Save. The edits are the ones the first release ships: a turn, a move, pages added from another PDF, a delete, and the rotation of every page
//! at once. Each test puts one of each on a tab of its own, leaves by one way, comes back, and checks two things: every edit is there as it was, with
//! whose it was and its undo and redo, and no PDF changed by a byte. Leaving with a save is checked for what reaches the file.
//!
//! What a kill keeps is what the journal holds: edits go to it after 2 s of quiet or 5 s at most in a burst (SPEC decision 25, for the SSD), so a
//! crash in the moments after an edit can lose that edit. The kill tests wait for the journal, as the record's own tests do.

use super::{Run, State, tabs_of, text_document, wait_until};
use crate::common::app::wait_for;
use recto_agent::Level;
use std::path::{Path, PathBuf};

/// The kinds of edit the first release ships.
const KINDS: [&str; 5] = ["turn", "move", "add", "delete", "rotate-all"];

/// A document with one kind of edit on it, and its bytes as they were.
struct Edited {
    kind: &'static str,
    path: PathBuf,
    original: Vec<u8>,
}

/// Opens a document per kind, each with that edit, some the owner's and some an agent's, and one PDF the added pages come from.
fn one_of_each(run: &Run<'_>, folder: &Path) -> (Vec<Edited>, Edited) {
    std::fs::create_dir_all(folder).unwrap();
    let extra_path = text_document(folder, "extra.pdf", 2);
    let extra = Edited {
        kind: "source",
        original: std::fs::read(&extra_path).unwrap(),
        path: extra_path,
    };
    let mut docs = Vec::new();
    for kind in KINDS {
        let path = text_document(folder, &format!("{kind}.pdf"), 4);
        run.ok(&["open", path.to_str().unwrap()]);
        run.settled();
        match kind {
            "turn" => run.ok(&["qa", "owner-edit", "rotate:2"]),
            "move" => run.ok(&["page", "move", "3", "--to", "1"]),
            "add" => run.ok(&["page", "insert", extra.path.to_str().unwrap(), "--at", "2"]),
            "delete" => run.ok(&["qa", "owner-edit", "delete:2"]),
            _ => run.ok(&["page", "rotate", "all", "--by", "90"]),
        };
        docs.push(Edited {
            kind,
            original: std::fs::read(&path).unwrap(),
            path,
        });
    }
    run.settled();
    (docs, extra)
}

fn tab_of(run: &Run<'_>, path: &Path) -> String {
    let name = path.file_name().unwrap().to_str().unwrap();
    let tab = tabs_of(&run.report())
        .into_iter()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .unwrap_or_else(|| panic!("no tab for {name}"));
    format!("#{}", tab["id"])
}

/// Failure: closing a tab with unsaved edits loses them without asking, Cancel closes it anyway, Don't Save writes something, or Save brings what
/// was only ever in the history into the PDF (owner: "you definitely don't want history to leak into the pdf file"). Every kind asks; Cancel keeps
/// the tab and its edits; Don't Save closes it with its PDF as it was; Save writes the edit and nothing of a PDF added and undone, whose name and
/// text were in the history.
#[test]
fn closing_a_tab_asks_and_each_answer_does_what_it_says() {
    const MARK: &str = "HISTORYMARKUNDONEADD";
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    let (docs, _) = one_of_each(&run, &folder);
    let marked = folder.join(format!("{MARK}.pdf"));
    std::fs::write(&marked, recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&[MARK]))])).unwrap();
    for doc in &docs {
        let tab = tab_of(&run, &doc.path);
        run.ok(&["tap", &tab]);
        run.settled();
        // Cancel: the tab and its edits stay.
        let before = run.ok(&["edits", "--tab", &tab]);
        run.ok(&["qa", "menu", "file.close"]);
        let asked = wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
        assert_eq!(asked["app"]["sheet"]["buttons"][0], "Save", "{}: {}", doc.kind, asked["app"]["sheet"]);
        run.ok(&["qa", "sheet-answer", "second"]);
        wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
        assert_eq!(run.ok(&["edits", "--tab", &tab]), before, "{}: Cancel keeps it", doc.kind);
    }
    // Save, on the tab with the turn, with a PDF added and undone first so its name and text are only in the history.
    let turn = &docs[0];
    let tab = tab_of(&run, &turn.path);
    run.ok(&["tap", &tab]);
    run.ok(&["page", "insert", marked.to_str().unwrap(), "--at", "1"]);
    run.ok(&["undo"]);
    run.settled();
    run.ok(&["session", "save"]);
    let journals = state.root.join("session").join("journals");
    let in_history = std::fs::read_dir(&journals)
        .unwrap()
        .flatten()
        .any(|entry| std::fs::read(entry.path()).is_ok_and(|bytes| bytes.windows(MARK.len()).any(|window| window == MARK.as_bytes())));
    assert!(in_history, "the mark is in the history");
    run.ok(&["qa", "menu", "file.close"]);
    wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
    run.ok(&["qa", "sheet-answer", "first"]);
    wait_until("saved and closed", || tabs_of(&run.report()).len() == KINDS.len() - 1);
    let saved = std::fs::read(&turn.path).unwrap();
    assert!(saved.starts_with(&turn.original) && saved.len() > turn.original.len(), "the turn was appended");
    assert!(
        !saved.windows(MARK.len()).any(|window| window == MARK.as_bytes()),
        "nothing of the history in the PDF"
    );
    // Don't Save, on every other: closed, and its PDF as it was.
    for doc in &docs[1..] {
        run.ok(&["tap", &tab_of(&run, &doc.path)]);
        run.settled();
        run.ok(&["qa", "menu", "file.close"]);
        wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt");
        run.ok(&["qa", "sheet-answer", "third"]);
        wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
        assert!(std::fs::read(&doc.path).unwrap() == doc.original, "{}: Don't Save wrote nothing", doc.kind);
    }
    wait_until("all closed", || tabs_of(&run.report()).is_empty());
    // And what each answer did holds across a launch: nothing comes back as a tab.
    run.quit();
    let run = Run::on(&state).unwrap();
    run.settled();
    assert!(tabs_of(&run.report()).is_empty(), "{}", run.report()["session"]["tabs"]);
}

/// Failure (PLAN phase 4: "a kill after the move and before `saved`, in the real app"): a crash in the moment between the new file going into place
/// and the session hearing it was saved loses the history, or comes back reading the saved file as unsaved edits on top of it, which would turn a
/// page twice or add a page again. A real Save from the window, of a tab with a turn, a move and an added page, and of one with a delete, which
/// rewrites, killed by the save's debug-only pause just after the move. At the next launch the file is the whole new one, the tab reads as saved,
/// and undo goes back past the save. With no record yet naming the tab, it doesn't come back, as nothing in it is unsaved, and opening the PDF
/// again finds the same. Killed just before the move, the file is as it was and the edits are still there, unsaved.
#[test]
fn a_kill_between_the_move_and_saved_carries_the_save_over() {
    for (step, delete, recorded) in [
        ("committed", false, true),
        ("committed", true, true),
        ("committed", false, false),
        ("prepared", false, false),
        ("writing", false, false),
    ] {
        let state = State::at(Level::Full);
        let marker = state.root.join("paused");
        let env = [
            ("RECTO_TEST_SAVE_PAUSE", std::ffi::OsStr::new(step)),
            ("RECTO_TEST_SAVE_MARKER", marker.as_os_str()),
        ];
        let Some(run) = Run::on_with_env(&state, &env) else { return };
        let folder = state.root.join("docs");
        std::fs::create_dir_all(&folder).unwrap();
        let file = text_document(&folder, "doc.pdf", 4);
        let extra = text_document(&folder, "extra.pdf", 1);
        let original = std::fs::read(&file).unwrap();
        run.ok(&["open", file.to_str().unwrap()]);
        run.settled();
        if delete {
            run.ok(&["qa", "owner-edit", "delete:2"]);
        } else {
            run.ok(&["qa", "owner-edit", "rotate:1"]);
            run.ok(&["page", "move", "4", "--to", "1"]);
            run.ok(&["page", "insert", extra.to_str().unwrap(), "--at", "5"]);
        }
        run.settled();
        let pages = run.ok(&["edits"])["pages"].clone();
        if recorded {
            // The record has the tab, as it does a minute after it's opened.
            run.ok(&["session", "save"]);
        }
        run.ok(&["qa", "button", "save"]);
        wait_until("the save reaches its pause", || marker.exists());
        run.kill();
        let now = std::fs::read(&file).unwrap();
        let run = Run::on(&state).unwrap();
        run.settled();
        let case = format!("{step}, delete {delete}, recorded {recorded}");
        if step == "committed" && !recorded {
            // No record named the tab, and nothing in it is unsaved now: as for any tab opened in the last minute with nothing unsaved, a crash
            // doesn't bring it back. Its history waits for the PDF, on the new file.
            assert!(tabs_of(&run.report()).is_empty(), "{case}: {}", run.report()["session"]["tabs"]);
            run.ok(&["open", file.to_str().unwrap()]);
            run.settled();
        }
        let edits = run.ok(&["edits"]);
        assert_eq!(edits["pages"], pages, "{case}: the same pages either way: {edits}");
        if step != "committed" {
            assert!(now == original, "{case}: before the move, the file is as it was");
            assert_eq!(edits["edited"], true, "{case}: and the edits are there, unsaved: {edits}");
            continue;
        }
        assert!(now != original, "{case}: the move had happened");
        if !delete {
            assert!(now.starts_with(&original), "{case}: appended");
        }
        let reader = lopdf::IndexedReader::open(lopdf::FileSource::open(&file).unwrap()).unwrap();
        assert_eq!(serde_json::json!(reader.page_count().unwrap()), pages, "{case}: the whole new file");
        assert_eq!(
            edits["edited"], false,
            "{case}: it reads as saved, not as edits on top of the saved file: {edits}"
        );
        assert_eq!(edits["file_pages"], pages, "{case}: {edits}");
        // Undo goes past the save, back to the file as it was.
        run.ok(&["undo"]);
        run.settled();
        let undone = run.ok(&["edits"]);
        assert_eq!(undone["edited"], true, "{case}: past the save: {undone}");
    }
}
