//! **A launch that finds an unfinished removal's record** (`docs/design/removal.md` §6, D3b; owner, 2026-11-27: "Empty window, session kept"): the window starts
//! empty and says why, the saved session stays exactly as it is, and nothing the person does in the window (an open, an edit, a save, a quit) changes the
//! state folder until the removal is finished.

use super::late_lock::a_saved_session;
use recto_agent::Level;
use recto_control::removal_store::barrier::fixture::{pending_at, tree};
use recto_session::record::notices;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The state folder as the crash left it, without what a running app writes as it goes whatever it does (its lock, its logs and its level file).
fn state_folder(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut found = tree(root);
    found.retain(|path, _| !path.starts_with("run") && !path.to_string_lossy().contains("log") && path != &PathBuf::from("level"));
    found
}

/// Failure (the owner's choice, 2026-11-27, and the point of the gate): a launch that restores the saved session while a removal is unfinished (the tabs'
/// journals are what it is purging), or that starts empty and then writes over the saved session as the person works (an open's history, a save, a quit's
/// flush), or that shows the person the "couldn't save" notice for it, or that stops them quitting. It starts with no tabs and says in words (no file names)
/// that the session was left as it is; `session save` is refused as `removal_pending` and `session restore` brings nothing back; an open, an edit and a quit
/// leave the state folder as it was; the quit goes; and `qa removal-state` says what is there.
#[test]
fn a_launch_with_an_unfinished_removal_starts_empty_and_leaves_the_session_as_it_is() {
    let disk = a_saved_session();
    pending_at(&disk.paths().root, 1);
    let app = disk.start();
    app.settle();
    let words = notices::words(notices::REMOVAL_PENDING);
    assert_eq!(app.reader().note.as_deref(), words, "the person is told");
    assert!(
        !words.unwrap().contains(".pdf") && !words.unwrap().contains("pending.v1"),
        "no file name in the words"
    );
    assert!(app.tabs().is_empty(), "no tab came back: {:?}", app.tabs());
    let root = app.disk().paths().root.clone();
    let before = state_folder(&root);

    let c = app.disk().pdf("c.pdf", 2);
    app.open(&c);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##);
    app.settle();
    let saved = app
        .agent(Level::Full, r#"{"cmd":"session_save"}"#)
        .expect_err("a save while a removal is pending");
    assert_eq!(saved.code, "removal_pending", "{saved:?}");
    let restored = app.agent(Level::Full, r#"{"cmd":"session_restore"}"#).expect("a restore that restored nothing");
    assert_eq!(restored["restored"], false, "{restored}");
    assert_eq!(restored["notice"].as_str(), words, "{restored}");
    assert_ne!(
        app.reader().note.as_deref(),
        Some(notices::SAVE_FAILED),
        "the person isn't told that saving failed: nothing was to be saved"
    );
    let state = app
        .core()
        .qa_session_verb("removal_state", "")
        .expect("a QA verb")
        .expect("its answer")
        .expect("a state");
    assert_eq!(state["record"]["state"], "pending", "{state}");
    assert_eq!(state["withheld"], "removal_pending", "{state}");
    assert!(!state.to_string().contains("pending.v1"), "no file name in {state}");
    assert_eq!(state_folder(&root), before, "an open, an edit and a save changed the state folder");

    // The edit made in this window can't be kept, which is said as for any refused write: the quit asks, and goes on Quit Anyway.
    let answer = app.ask_to_quit();
    assert!(!answer.go && answer.quit_button == "Quit Anyway", "{answer:?}");
    let disk = app.quit_anyway();
    assert_eq!(state_folder(&root), before, "a quit changed the state folder");
    let app = disk.start();
    app.settle();
    assert!(app.tabs().is_empty(), "and the next launch finds it as it was");
    assert_eq!(app.reader().note.as_deref(), words);
    let (answer, _) = app.quit();
    assert!(answer.go, "with nothing edited the quit isn't asked about: {answer:?}");
}

/// Every file under `dir`, at any depth: what the backups folder holds.
fn files_under(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .map(|entry| if entry.path().is_dir() { files_under(&entry.path()) } else { 1 })
            .sum()
    })
}

/// Opens a PDF, turns a page and saves it as an agent at Full does, in an app started on `disk`; says what the save answered and whether the file changed.
fn turn_and_save(disk: crate::testkit::headless::Disk) -> (Result<serde_json::Value, recto_agent::AgentError>, bool) {
    let (saved, changed, _) = turn_and_save_counting(disk);
    (saved, changed)
}

/// As [`turn_and_save`], and says how many files the backups folder holds after it.
fn turn_and_save_counting(disk: crate::testkit::headless::Disk) -> (Result<serde_json::Value, recto_agent::AgentError>, bool, usize) {
    let backups = recto_control::host::backups_folder(disk.paths());
    let c = disk.pdf("c.pdf", 2);
    let app = disk.start();
    app.settle();
    let was = std::fs::read(&c).unwrap();
    app.open(&c);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##);
    app.settle();
    let saved = app.agent(Level::Full, r##"{"cmd":"save","tab":"#1"}"##);
    app.settle();
    let changed = std::fs::read(&c).unwrap() != was;
    let kept = files_under(&backups);
    drop(app.quit_anyway());
    (saved, changed, kept)
}

/// Failure (D3b's gates, `removal.md` §10): a save made while a removal is unfinished keeps a new backup of the original, a copy of the file the removal's
/// plan didn't pin, which a restore or a going back would then bring back after the purge. The save still goes through and the file is written, and no
/// backup is made. The same save with no record there does make one (so the test can see what it stops).
#[test]
fn a_save_while_a_record_is_there_keeps_no_backup() {
    let (saved, changed, kept) = turn_and_save_counting(crate::testkit::headless::Disk::new());
    assert!(
        saved.is_ok() && changed && kept > 0,
        "with no record: {saved:?}, changed {changed}, backup files {kept}"
    );
    let disk = crate::testkit::headless::Disk::new();
    pending_at(&disk.paths().root, 1);
    let (saved, changed, kept) = turn_and_save_counting(disk);
    assert!(saved.is_ok() && changed, "{saved:?}, file changed: {changed}");
    assert_eq!(kept, 0, "a backup was made while a record was there");
}

/// Failure (D3b review, a blocker): in a window whose session is withheld the store keeps no journal, but the session still names one in memory for the
/// tab, and a save looks for its note on disk, doesn't find it and fails ("the save's note didn't reach the journal"): a person can't save a PDF they edit
/// in that window. The save goes through and the file is written; no journal, record or backup is written (`a_save_while_a_record_is_there_keeps_no_backup`).
#[test]
fn a_pdf_edited_in_a_withheld_window_can_be_saved() {
    let disk = crate::testkit::headless::Disk::new();
    pending_at(&disk.paths().root, 1);
    let (saved, changed) = turn_and_save(disk);
    assert!(saved.is_ok() && changed, "{saved:?}, file changed: {changed}");
}

/// Failure (found on the way, the same bug in the mode that was there before D3b): a record that can't be read leaves the store refusing writes, and a save
/// of an edited PDF in that window fails for the same reason.
#[test]
fn a_pdf_edited_in_an_unreadable_record_window_can_be_saved() {
    use std::os::unix::fs::PermissionsExt as _;
    let disk = a_saved_session();
    let slot = disk.paths().root.join("session").join("record.a.json");
    std::fs::set_permissions(&slot, std::fs::Permissions::from_mode(0o666)).unwrap();
    let (saved, changed) = turn_and_save(disk);
    assert!(saved.is_ok() && changed, "{saved:?}, file changed: {changed}");
}

/// Failure (D3b review): `removal check` while an unfinished removal's record is there answers as a look that found nothing to offer (and an agent or a
/// person reads "nothing to clear"), where `removal clear` is refused as busy. Both are refused as `removal_busy`, in the same words, and nothing is offered.
#[test]
fn a_removal_check_while_a_removal_is_pending_is_refused_as_busy() {
    let bed = super::removal_clear::bed();
    pending_at(&bed.root(), 1);
    for level in [Level::ReadOnly, Level::Full] {
        let refused = bed
            .app
            .agent(level, r#"{"cmd":"removal_check"}"#)
            .expect_err("a check while a removal is pending");
        assert_eq!(refused.code, "removal_busy", "{level:?}: {refused:?}");
        assert!(refused.message.contains("left unfinished"), "{refused:?}");
    }
}

/// Failure (D3b's gates, `removal.md` §10): a launch while a removal is unfinished prunes the old backups, which the plan pinned, outside the plan. The same
/// launch with no record does prune (so the test sees what it stops).
#[test]
fn a_launch_with_a_record_keeps_old_backups() {
    for pending in [false, true] {
        let disk = crate::testkit::headless::Disk::new();
        let root = disk.paths().root.clone();
        if pending {
            pending_at(&root, 1);
        }
        let folder = root.join("backups.noindex");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("original-aaaaaaaa.pdf");
        std::fs::write(&file, b"%PDF-1.4 an original").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(400 * 24 * 60 * 60);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        let app = disk.start();
        app.settle();
        assert_eq!(file.exists(), pending, "pending {pending}: an old backup after a launch");
        drop(app.quit_anyway());
    }
}

/// Seeds an unfinished removal's record in `disk`'s state folder, for a test in another file.
pub(super) fn pending_at_root(disk: &crate::testkit::headless::Disk) {
    pending_at(&disk.paths().root, 1);
}
