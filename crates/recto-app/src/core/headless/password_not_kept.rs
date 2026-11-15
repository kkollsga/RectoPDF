//! **A password the person types is kept nowhere** (PRIVACY.md: "Passwords are never kept"; the activity log "never document text"). It reaches the session by
//! one call, `Prompts::password_answered`, which is no command, so no command's `log_details` can carry one; this holds it at that seam, against every file
//! the state folder comes to.

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

/// Failure: the password typed into the sheet is written somewhere in the state folder: the activity log (which records an agent's commands, and here the
/// ones the test sends while the document is open), the state record, a journal, a sleep file. The person types a password, an agent works on the
/// session, the record is saved; no file of the state folder holds the password's bytes, and the log has lines.
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
