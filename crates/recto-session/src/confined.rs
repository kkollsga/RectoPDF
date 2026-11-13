//! **A session on a test state folder only uses files in its own folders** (2026-11-13): the system's temp folders ([`RunFolders`]). Set once, by
//! whatever makes a session on such a folder, and never taken off or widened. Every path a command names is checked before the command runs
//! (`recto-agent`'s `quiet_files`); this is the check for a path that becomes a tab's file or a page's source by any route, a command or not: an open, a
//! restored record's tabs, and a file that moved and would be followed. A tab outside the folders never comes to exist, so nothing reads or writes its
//! file.

use recto_platform::run_folders::RunFolders;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// The refusal's words, the one wording for every route: `recto-agent`'s refusal is made from these.
pub const OUTSIDE_WORDS: &str =
    "A quiet run only uses files in its own folders, the system's temp folders. That file isn't in them, so nothing was opened or written.";

/// The folders a session is confined to, once it is: [`Session::confinement`](super::Session::confinement).
#[derive(Debug, Default)]
pub struct Confinement(OnceLock<Arc<RunFolders>>);

impl Confinement {
    /// **Confines the session to these folders,** for good: a second call keeps the first, so nothing can widen them.
    pub fn confine(&self, folders: RunFolders) {
        let _ = self.0.set(Arc::new(folders));
    }

    /// The folders, if it's confined.
    pub fn folders(&self) -> Option<&Arc<RunFolders>> {
        self.0.get()
    }

    /// Whether a file may be the session's: anywhere, unless it's confined. Looks at the disk only when it is.
    pub fn admits(&self, path: &Path) -> bool {
        self.0.get().is_none_or(|folders| folders.admits(path))
    }
}
