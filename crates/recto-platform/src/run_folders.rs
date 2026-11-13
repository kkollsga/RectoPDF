//! **A quiet run's own folders,** the only places a session on a test state folder may open or write a file: the system's own temp folder (from
//! `confstr`, never `$TMPDIR`, which the caller sets) and `/tmp`, where its state folder is too unless it was put somewhere odd. The rule is [`RunFolders::admits`], and it goes by where a path really
//! leads: every link followed, a parent that is a link included, so a link inside a temp folder to someone's file is that file, outside. A path that
//! doesn't exist yet (a file about to be written) is judged by its nearest folder that does, and one that goes through a link to nowhere, or climbs out
//! with `..` past what exists, is refused, as it has no meaning that can be checked.
//!
//! Only the mechanism is here, so the session, the executor and the installer can all ask it. Which sessions are confined, and the refusal's words, are
//! `recto-control`'s `quiet` and `recto-agent`'s `quiet_files`.
//!
//! **What this can't see:** a folder swapped for a link between the check and the use. Only a process that can make links can do that, and such a process
//! can read the file itself; nothing a session does makes a link.

use std::path::{Component, Path, PathBuf};

/// The folders a confined session may use, each where it really is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFolders {
    roots: Vec<PathBuf>,
}

impl RunFolders {
    /// A quiet run's: the [`temp_folders`], and its state folder only where that is inside them already. A state folder elsewhere admits nothing of its
    /// own: `RECTO_STATE_DIR=~/Documents` names a private folder, which would otherwise admit everything in it (coordinator, 2026-11-13). The app's own
    /// files under a state folder are named by the app, never by a command, so they don't need it.
    #[must_use]
    pub fn for_state(_state: &Path) -> Self {
        Self { roots: temp_folders() }
    }

    /// These folders and nothing else, each resolved. For tests of the rule.
    #[must_use]
    pub fn of(folders: &[&Path]) -> Self {
        Self {
            roots: folders.iter().filter_map(|folder| resolved(folder)).collect(),
        }
    }

    /// **The rule:** whether this path, wherever its links and `..` really take it, is in one of the folders. A relative path is judged from this
    /// process's own folder, as a file call would take it.
    #[must_use]
    pub fn admits(&self, path: &Path) -> bool {
        resolved(path).is_some_and(|real| self.roots.iter().any(|root| real.starts_with(root)))
    }

    /// The folders, resolved.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// **The system's temp folders:** this user's own (`confstr`'s `_CS_DARWIN_USER_TEMP_DIR`, which `$TMPDIR` can't change) and `/tmp`, each where it
/// really is (`/private/var/folders/...` and `/private/tmp` on a Mac). One that isn't there is left out.
#[must_use]
pub fn temp_folders() -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = [crate::local::system_temp_dir(), Some(PathBuf::from("/tmp"))]
        .into_iter()
        .flatten()
        .filter_map(|folder| folder.canonicalize().ok())
        .collect();
    folders.dedup();
    folders
}

/// A path's real location, following links as far as it exists, with what doesn't exist yet put back on the end. `None` for a link to nowhere, or a
/// `..` in what doesn't exist, since neither can be checked.
#[must_use]
pub fn resolved(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for part in missing.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        // There, but it can't be followed: a link to nowhere, or one that goes round.
        if existing.symlink_metadata().is_ok() {
            return None;
        }
        let name = existing.components().next_back()?;
        if !matches!(name, Component::Normal(_)) {
            return None;
        }
        missing.push(name.as_os_str().to_os_string());
        existing = existing.parent()?;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp() -> recto_testpdf::TempDir {
        recto_testpdf::TempDir::new("recto-run-folders-")
    }

    /// Failure: a file in the run's folder, or one about to be written there, is refused, so a quiet run can't use its own files; or a file beside it is
    /// let through.
    #[test]
    fn a_file_in_the_folder_is_admitted_and_one_beside_it_isnt() {
        let dir = temp();
        std::fs::create_dir(dir.join("run")).unwrap();
        std::fs::write(dir.join("run/a.pdf"), b"x").unwrap();
        std::fs::write(dir.join("b.pdf"), b"x").unwrap();
        let folders = RunFolders::of(&[&dir.join("run")]);
        assert!(folders.admits(&dir.join("run/a.pdf")));
        assert!(
            folders.admits(&dir.join("run/new/deeper/out.png")),
            "a file about to be written, in folders not made yet"
        );
        assert!(!folders.admits(&dir.join("b.pdf")));
        assert!(!folders.admits(&dir.join("runner/a.pdf")), "a folder whose name only starts the same");
        assert!(!folders.admits(&dir.join("run/../b.pdf")), "`..` out of it");
        assert!(!folders.admits(&dir.join("run/missing/../../b.pdf")), "`..` through what doesn't exist");
    }

    /// Failure: a link inside the folder to a file outside it is taken for a file inside, so the outside file is read or written through it; or a link
    /// to nowhere is let through, to be made to point anywhere after the check.
    #[test]
    fn a_link_in_the_folder_is_where_it_leads() {
        let dir = temp();
        let run = dir.join("run");
        let outside = dir.join("outside");
        std::fs::create_dir(&run).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.pdf"), b"x").unwrap();
        symlink(outside.join("secret.pdf"), run.join("link.pdf")).unwrap();
        symlink(&outside, run.join("folder")).unwrap();
        symlink(dir.join("nowhere"), run.join("dangling.pdf")).unwrap();
        let folders = RunFolders::of(&[&run]);
        assert!(!folders.admits(&run.join("link.pdf")), "a link to a file outside");
        assert!(!folders.admits(&run.join("folder/secret.pdf")), "a parent that's a link outside");
        assert!(
            !folders.admits(&run.join("folder/new.pdf")),
            "a file to be written through a parent that's a link outside"
        );
        assert!(!folders.admits(&run.join("dangling.pdf")), "a link to nowhere");
        // And the other way: a link outside to a file inside is that file, inside.
        std::fs::write(run.join("mine.pdf"), b"x").unwrap();
        symlink(run.join("mine.pdf"), outside.join("to-mine.pdf")).unwrap();
        assert!(folders.admits(&outside.join("to-mine.pdf")));
    }

    /// Failure: `$TMPDIR` decides what counts as a temp folder, so a caller who sets it to their Documents makes those a quiet run's own; or the
    /// system's temp folder is left out, so no test file is usable.
    #[test]
    fn the_temp_folders_are_the_systems() {
        let folders = temp_folders();
        let system = crate::local::system_temp_dir().unwrap().canonicalize().unwrap();
        assert!(folders.contains(&system), "{folders:?}");
        assert!(folders.contains(&PathBuf::from("/private/tmp")), "{folders:?}");
        // Whatever this process's environment says, the list is the same.
        assert_eq!(folders.len(), 2);
        let run = RunFolders::for_state(Path::new("/private/tmp/recto-no-such-state"));
        assert!(run.admits(&system.join("x.pdf")));
        assert!(!run.admits(Path::new("/Users/Shared/x.pdf")));
        assert!(!run.admits(Path::new("/Applications/RectoPDF.app")));
    }

    /// Failure: a state folder outside the temp folders admits what's in it, so `RECTO_STATE_DIR` naming a private folder of the user's (their
    /// Documents is one) makes all its PDFs a quiet run's own. Inside a temp folder, it's admitted with it.
    #[test]
    fn a_state_folder_outside_temp_admits_nothing_of_its_own() {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).unwrap();
        let outside = crate::fresh::FreshDir::new_in(&target, "recto-state-outside-", "").unwrap();
        std::fs::write(outside.path().join("report.pdf"), b"x").unwrap();
        let folders = RunFolders::for_state(outside.path());
        assert!(
            !folders.roots().iter().any(|root| outside.path().canonicalize().unwrap().starts_with(root)),
            "the checkout's target is outside temp"
        );
        assert!(!folders.admits(&outside.path().join("report.pdf")));
        assert!(!folders.admits(&outside.path().join("new.png")));
        let dir = temp();
        assert!(RunFolders::for_state(&dir.join("state")).admits(&dir.join("state/out.pdf")));
    }
}
