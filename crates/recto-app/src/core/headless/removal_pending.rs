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
