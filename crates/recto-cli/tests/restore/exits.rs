//! **A kill in the middle of a real save, in the real app** (PLAN phase 4). The rest of the ways of leaving come with the exit matrix.

use super::{Run, State, tabs_of, text_document, wait_until};
use recto_agent::Level;

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
