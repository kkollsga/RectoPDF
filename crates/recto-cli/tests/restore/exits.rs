//! **Leaving a tab or the app, where only the real app shows it** (PLAN phase 4, from the research's rule 3: "no exit path loses work silently"). Every shipped
//! edit against every way of leaving, a quit, a kill, a hand-off, sleep and a kill while asleep, the window closing, printing and exporting, and each answer of
//! the close prompt, is held in process by `recto-app`'s `core::headless::exits`. What's here is what goes through the window's own sheet, and a kill in the
//! middle of a real save, by the save's debug-only pause.

use super::{Run, State, tabs_of, text_document, wait_until};
use crate::common::app::wait_for;
use recto_agent::Level;
use std::path::Path;

fn tab_of(run: &Run<'_>, path: &Path) -> String {
    let name = path.file_name().unwrap().to_str().unwrap();
    let tab = tabs_of(&run.report())
        .into_iter()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .unwrap_or_else(|| panic!("no tab for {name}"));
    format!("#{}", tab["id"])
}

/// Failure: closing a tab with unsaved edits closes it without the window asking, or the window's sheet shows other buttons than Rust's or in another order, or
/// a button does another's answer: Cancel closes it, or Don't Save writes something. One edited tab: File > Close shows the sheet with Save first; its second
/// button keeps the tab and its edits; its third closes it with its PDF as it was. What each answer does for every kind, Save's keeping the history out of the
/// PDF, and nothing coming back after a launch are the core's, held by `recto-app`'s `core::headless::exits::closing_a_tab_asks_and_each_answer_does_what_it_says`.
#[test]
fn closing_a_tab_asks_and_each_answer_does_what_it_says() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on(&state) else { return };
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let path = text_document(&folder, "turn.pdf", 4);
    let original = std::fs::read(&path).unwrap();
    run.ok(&["open", path.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    run.settled();
    let tab = tab_of(&run, &path);
    let before = run.ok(&["edits", "--tab", &tab]);
    let ask = || {
        run.ok(&["qa", "menu", "file.close"]);
        wait_for(&run, |state| state["app"]["sheet"]["kind"] == "close_prompt")
    };
    let gone = || wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
    let asked = ask();
    assert_eq!(
        asked["app"]["sheet"]["buttons"],
        serde_json::json!(["Save", "Cancel", "Don't Save"]),
        "{}",
        asked["app"]["sheet"]
    );
    run.ok(&["qa", "sheet-answer", "second"]);
    gone();
    assert_eq!(run.ok(&["edits", "--tab", &tab]), before, "Cancel keeps it");
    ask();
    run.ok(&["qa", "sheet-answer", "third"]);
    gone();
    wait_until("closed", || tabs_of(&run.report()).is_empty());
    assert!(std::fs::read(&path).unwrap() == original, "Don't Save wrote nothing");
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
