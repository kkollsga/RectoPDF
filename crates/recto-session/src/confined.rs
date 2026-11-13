//! **A session on a test state folder only uses files in its own folders** (2026-11-13): the system's temp folders ([`RunFolders`]). Set once, by
//! whatever makes a session on such a folder, and never taken off or widened. Every path a command names is checked before the command runs
//! (`recto-agent`'s `quiet_files`); this is the check for a path that becomes a tab's file or a page's source by any route, a command or not: an open, a
//! restored record's tabs, and a file that moved and would be followed. A tab outside the folders never comes to exist, so nothing reads or writes its
//! file.

use recto_platform::run_folders::RunFolders;
use std::borrow::Cow;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// The refusal's words, the one wording for every route: `recto-agent`'s refusal is made from these.
pub const OUTSIDE_WORDS: &str =
    "A quiet run only uses files in its own folders, the system's temp folders. That file isn't in them, so nothing was opened or written.";

/// The folders a session is confined to, once it is: [`Session::confinement`](super::Session::confinement). One cell, which whatever holds the session
/// can share ([`Self::shared`]), so the question is answered without the session's lock (review of the branch, round 2, F-d).
#[derive(Debug, Default)]
pub struct Confinement(Arc<OnceLock<Arc<RunFolders>>>);

impl Confinement {
    /// **The same cell, for whatever holds the session** (its host, the executor's check): what it reads is what the session goes by, confined later or
    /// not, as there's only the one, and reading it takes no lock, as it's set once and never changed.
    #[must_use]
    pub fn shared(&self) -> Self {
        Self(Arc::clone(&self.0))
    }

    /// **Confines the session to these folders,** for good: a second call keeps the first, so nothing can widen them.
    pub fn confine(&self, folders: RunFolders) {
        let _ = self.0.set(Arc::new(folders));
    }

    /// The folders, if it's confined.
    pub fn folders(&self) -> Option<&Arc<RunFolders>> {
        self.0.get()
    }

    /// **The path to read a file by, once it's been put to the rule:** for a confined session, where it really leads, so the read goes where the rule
    /// looked and not wherever its name leads a moment later; for a free one, the path as given, with nothing asked of the disk. `None` when it's
    /// outside. The tab keeps the name it was given: this is only what the read opens. A free session's costs no allocation, as an open's are counted.
    pub fn to_read<'a>(&self, path: &'a Path) -> Option<Cow<'a, Path>> {
        match self.0.get() {
            Some(folders) => folders.inside(path).map(Cow::Owned),
            None => Some(Cow::Borrowed(path)),
        }
    }

    /// **Whether an edit list read back from disk may be this session's:** every PDF whose pages it adds (another file inserted, a copy kept for undo) in
    /// the folders. A record or journal in the state folder isn't the session's to trust, so one naming a file outside doesn't bring that file's pages
    /// back, by a restore, a wake or a closed PDF's history taken up (review of the branch, the replayed history).
    pub fn admits_edits(&self, edits: &crate::edit::PageEdits) -> bool {
        self.0.get().is_none() || edits.added_sources().iter().chain(edits.keeping()).all(|added| self.admits(&added.path))
    }

    /// Whether this is the same cell as `other`: one made from the other by [`Self::shared`], or by the same.
    #[must_use]
    pub fn is(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// **Whether this cell may take `current`'s place in a session without widening what it may use,** a free one first taking `current`'s folders
    /// (so the host whose cell it is becomes confined with its session, the safe direction). A confined one may only if each of its folders is in one of
    /// `current`'s. So a session confined can't come out of [`crate::parts::PartMut::keep_confinement`] free, or with wider folders, whatever cell it's
    /// handed: a `Confinement::default()` made for the purpose included.
    pub(crate) fn may_replace(&self, current: &Self) -> bool {
        let Some(folders) = current.folders() else { return true };
        self.confine(RunFolders::clone(folders));
        self.folders()
            .is_some_and(|mine| mine.roots().iter().all(|root| folders.roots().iter().any(|theirs| root.starts_with(theirs))))
    }

    /// Whether a file may be the session's: anywhere, unless it's confined. Looks at the disk only when it is.
    pub fn admits(&self, path: &Path) -> bool {
        self.0.get().is_none_or(|folders| folders.admits(path))
    }
}
