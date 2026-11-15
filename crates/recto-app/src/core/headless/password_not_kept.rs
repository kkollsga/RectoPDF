//! **A password the person types isn't in the state record** (PRIVACY.md: "Passwords are never kept"). It reaches the session by one call,
//! `Prompts::password_answered`, which is no command, so no command's log details can carry one: the activity log's side is held in `recto-control`'s
//! `no_words_an_agent_sends_reach_the_log`, which walks the command table. This holds the record: after a password is typed, no file the headless app has
//! written holds its bytes. The headless app writes the record and its lock and **no activity log**, and `session save` there answers `needs_server`, so
//! nothing here says anything about journals, sleep files or the log.

use super::withheld_events::{PASSWORD, person_unlocks, protected};
use crate::testkit::headless::Disk;
use recto_agent::Level;

/// Every file under a folder, with its bytes.
fn files_under(folder: &std::path::Path, found: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => files_under(&path, found),
            Ok(kind) if kind.is_file() => found.push((path.clone(), std::fs::read(&path).unwrap_or_default())),
            _ => {}
        }
    }
}

/// Failure: the password typed into the sheet is written into the state record, or a file beside it. The person types a password and an agent reads the
/// tabs; no file under the state folder holds the password's bytes (the folder has the record and its lock).
#[test]
fn a_password_the_person_types_is_in_no_file_of_the_state_folder() {
    super::print_protected::deadlines_met();
    let disk = Disk::new();
    let file = protected(&disk, 2, "Locked");
    let app = disk.start();
    person_unlocks(&app, &file);
    // The agent's commands are logged; an agent can't take the password, and what it sends is what's written.
    let _ = app.recto(Level::Full, &["tabs"]);
    let _ = app.recto(Level::Full, &["session", "save"]);
    app.settle();
    let mut found = Vec::new();
    files_under(&app.disk().paths().root, &mut found);
    assert!(!found.is_empty(), "the state folder has files");
    for (path, bytes) in &found {
        assert!(!String::from_utf8_lossy(bytes).contains(PASSWORD), "{} holds the password", path.display());
    }
}
