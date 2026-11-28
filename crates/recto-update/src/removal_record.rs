//! **Whether an unfinished removal's record is in the state folder, from the updater's side** (`docs/design/removal.md` §6, "Older binaries and updates"). While
//! one is, an update neither installs nor brings an older build's session copy back, because each would put back data the removal is purging: the install
//! copies the whole record (history included) into `update/before`, and a going back puts that copy over `session/`.
//!
//! This looks at the one name, `<state root>/removal/pending.v1`, and never reads it: a record that can't be read is as much there as one that can, and
//! **only a name that is not there is "nothing pending"**. `recto-control`'s `removal_store::gate` answers the same for the app's own reads; a test in that
//! crate holds the two to the same answer for every shape of folder. This crate cannot depend on `recto-control`, so the name is spelled here too.
//!
//! One descriptor for as long as the call, no thread, nothing kept.

use recto_platform::folder::{Folder, Opened};
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::Path;

/// The folder under the state root, and the record's name in it: `recto_control::removal_store::barrier`'s, held by a test.
const DIR: &str = "removal";
const NAME: &str = "pending.v1";

/// The `Info.plist` key a build carries when it reads the record before it restores anything. A build from before it ignores the record and restores
/// everything, so going back to one while a record is there would bring the removed data back.
pub const GATE_KEY: &str = "RectoRemovalGate";

/// Whether anything is at `<root>/removal/pending.v1`, readable or not. A folder that isn't a folder of ours, a link or a file at `removal`, is a yes: it
/// isn't known to be empty. A leftover temporary (`pending.v1.new-*`) is no record. Never makes anything. `root` is the state folder, never a path a command
/// or a document gave. May wait on the disk: not for a UI thread.
#[must_use]
pub fn anything_there(root: &Path) -> bool {
    match Folder::open(&root.join(DIR)) {
        Opened::Missing => false,
        Opened::Other => true,
        Opened::Folder(folder) => match folder.look(OsStr::new(NAME)) {
            Ok(_) => true,
            Err(error) => error.kind() != ErrorKind::NotFound,
        },
    }
}

/// Whether the build at `app` reads the record before it restores anything (it carries [`GATE_KEY`] as `1`).
#[must_use]
pub fn reads_the_record(app: &Path) -> bool {
    crate::stage::plist_value(app, GATE_KEY).ok().flatten().is_some_and(|value| value == "1")
}

/// What going back does when it finds the session's files and the removal's record side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoingBack {
    /// Go back and put the kept copy of the session back, as before (`restore_record`).
    Restore,
    /// Go back and leave the session's files exactly as they are: the plan keeps the record anyway, or a removal's record is there and the kept build reads it.
    LeaveRecord,
    /// Don't go back. The kept build wouldn't read the record, and would restore everything at its start, or put the kept copy over what the removal is purging.
    Refuse,
}

/// The one rule. No record: as before, the plan's `keep_record` deciding whether the kept copy is put back. A record: only to a build that reads it, and
/// never with the kept copy put back (it may hold the very history being purged). A build that doesn't read the record is never gone back to.
pub(crate) const fn going_back(removal_there: bool, kept_reads_it: bool, keep_record: bool) -> GoingBack {
    match (removal_there, kept_reads_it, keep_record) {
        (false, _, false) => GoingBack::Restore,
        (false, _, true) | (true, true, _) => GoingBack::LeaveRecord,
        (true, false, _) => GoingBack::Refuse,
    }
}

#[cfg(test)]
mod tests;
