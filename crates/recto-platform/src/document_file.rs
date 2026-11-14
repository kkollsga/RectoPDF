//! **The file wall: the one way a path becomes a document's open file** (the security review of `build/print-protected`, S1, 2026-11-14). Every read of
//! a document by its path (an open, a restore, a page added from another file, a page drawn, a save's read, a key for the index) opens it here, for every
//! session and every actor; a reviewed source scan (`recto-agent`'s `file_wall_scan`) fails any other open of a path in the crates a command's path reaches.
//!
//! **Why:** the person's print of a document they opened with its password is a plain PDF with no name, which the app holds by a descriptor while it's
//! prepared and while the system prints it. `/dev/fd/N` opens a descriptor of this process again, so a path an agent names could reach a copy no name
//! leads to. Three checks, each refusing with [`Refused::NotADocument`]:
//! 1. **A path into `/dev`**, as it's spelt ([`spelt_into_devices`], with no call to the disk, before anything is looked at: a command's first check)
//!    or where its links lead ([`leads_into_devices`], on the thread that reads, a link to nowhere included): the answer for `/dev/fd/N` is the same
//!    whether N is open, and whatever it is.
//! 2. **Not a regular file** (a folder, a pipe, a device), by the descriptor once it's open, and before it's opened by [`open_regular`] for a caller
//!    that mustn't wait (a pipe would wait for a writer for ever): the structure reader's and the index's opens, as they always looked first. The page
//!    view's and a worker's open ([`open`]) runs on a thread of its own that may wait, as for a file on a dead volume, and refuses it once it's open.
//! 3. **A file with no link**, by the descriptor once it's open (`fstat`): a file no name leads to can only be reached through a descriptor someone holds,
//!    however that's spelt (`/dev/fd/N`, `/dev/stdin`, a link to either), so this is the check no spelling gets round. macOS's `/.vol/<device>/<inode>`
//!    doesn't resolve for a file with no link (seen on APFS, 2026-11-14: "No such file or directory" once it's unlinked).
//!
//! A person has no reason to open anything under `/dev`; if they try, they're told what they'd be told of any file that isn't a PDF. The wall costs a
//! `realpath` and an `fstat` on an open, and nothing for a tab that isn't opening.

use std::fs::File;
use std::io;
use std::path::Path;

/// Why a path wasn't opened.
#[derive(Debug)]
pub enum Refused {
    /// The look at the path failed (`stat`): nothing there, not allowed, a volume gone. Kept, so each caller says what it always said.
    Look(io::Error),
    /// The open itself failed after the look.
    Open(io::Error),
    /// **Not a document's file:** a path into `/dev`, not a regular file, or a file with no link. One refusal for each, so none says which.
    NotADocument,
}

impl Refused {
    /// The failure as an I/O error, for a caller that speaks in those: the wall's own refusal is [`io::ErrorKind::InvalidInput`].
    #[must_use]
    pub fn into_io(self) -> io::Error {
        match self {
            Self::Look(error) | Self::Open(error) => error,
            Self::NotADocument => io::Error::new(io::ErrorKind::InvalidInput, "not a document's file"),
        }
    }
}

/// How many links a link to nowhere is followed through, looking for `/dev` at its end, as the system's own limit on a path's links.
const MAX_LINKS: u8 = 32;

/// **Opens a document's file for reading, through the wall** (see the module): the path's check, the open, and the descriptor's. The open may wait, on a
/// pipe for a writer or on a dead volume, so it's for a caller on a thread that may.
///
/// # Errors
///
/// [`Refused`]: what the open said, or that it's not a document's file.
pub fn open(path: &Path) -> Result<File, Refused> {
    if leads_into_devices(path) {
        return Err(Refused::NotADocument);
    }
    let file = File::open(path).map_err(Refused::Open)?;
    admit(&file)?;
    Ok(file)
}

/// **[`open`], with a look first** (`stat`) that refuses what isn't a regular file before it's opened, so a pipe isn't waited on.
///
/// # Errors
///
/// [`Refused`]: what the look or the open said, or that it's not a document's file.
pub fn open_regular(path: &Path) -> Result<File, Refused> {
    if leads_into_devices(path) {
        return Err(Refused::NotADocument);
    }
    if !std::fs::metadata(path).map_err(Refused::Look)?.is_file() {
        return Err(Refused::NotADocument);
    }
    let file = File::open(path).map_err(Refused::Open)?;
    admit(&file)?;
    Ok(file)
}

/// **The descriptor's check:** a regular file with at least one link. `fstat`, nothing else.
///
/// # Errors
///
/// [`Refused::NotADocument`] for anything else, or [`Refused::Open`] if it can't be asked.
pub fn admit(file: &File) -> Result<(), Refused> {
    let meta = file.metadata().map_err(Refused::Open)?;
    if !meta.is_file() || !has_a_link(&meta) {
        return Err(Refused::NotADocument);
    }
    Ok(())
}

#[cfg(unix)]
fn has_a_link(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 0
}

// Windows has no `/dev/fd`, and no stable link count in the standard library.
#[cfg(not(unix))]
fn has_a_link(_: &std::fs::Metadata) -> bool {
    true
}

/// **Whether a path is under `/dev`, as it's spelt or where its links really lead:** every link and parent followed, as far as the path exists, and a
/// link to nowhere judged by where it points, so a link to a descriptor that isn't open is answered as one to a descriptor that is.
#[must_use]
pub fn leads_into_devices(path: &Path) -> bool {
    seams::wait_if_held(path);
    leads_within(path, MAX_LINKS)
}

/// **A test's hold on the wall's look at one path** (`realpath`, `readlink`, a parent's), as a file on a volume that doesn't answer holds it: for the
/// test that the look never runs on a thread that mustn't wait. Each hold ends when it's dropped, or at [`seams::LIMIT`].
#[cfg(feature = "test-seams")]
pub mod seams {
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
    use std::time::Duration;

    /// The longest a look waits, so a test that fails with one held doesn't hang.
    pub const LIMIT: Duration = Duration::from_secs(60);
    /// The path held, and how many looks wait at it.
    static HELD: Mutex<(Option<PathBuf>, usize)> = Mutex::new((None, 0));
    static LET_GO: Condvar = Condvar::new();

    fn held() -> MutexGuard<'static, (Option<PathBuf>, usize)> {
        HELD.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Holds every look at `path` until this is dropped.
    #[must_use]
    pub struct Hold;

    impl Drop for Hold {
        fn drop(&mut self) {
            held().0 = None;
            LET_GO.notify_all();
        }
    }

    /// Holds the looks at `path`.
    pub fn hold(path: &Path) -> Hold {
        held().0 = Some(path.to_path_buf());
        Hold
    }

    /// How many looks wait at the held path now.
    #[must_use]
    pub fn waiting() -> usize {
        held().1
    }

    pub(super) fn wait_if_held(path: &Path) {
        let mut state = held();
        if state.0.as_deref() != Some(path) {
            return;
        }
        state.1 += 1;
        let (mut state, _) = LET_GO
            .wait_timeout_while(state, LIMIT, |state| state.0.as_deref() == Some(path))
            .unwrap_or_else(PoisonError::into_inner);
        state.1 -= 1;
    }
}

#[cfg(not(feature = "test-seams"))]
mod seams {
    pub(super) fn wait_if_held(_: &std::path::Path) {}
}

fn leads_within(path: &Path, links: u8) -> bool {
    let Ok(absolute) = std::path::absolute(path) else { return false };
    if under_devices(&absolute) {
        return true;
    }
    let mut existing = absolute.as_path();
    loop {
        // The part that exists, where it really is: what follows it doesn't exist, so it can't be a link that leads anywhere else.
        if let Ok(real) = existing.canonicalize() {
            return under_devices(&real);
        }
        // A link that can't be followed to its end: where it points, judged the same way.
        if let Ok(target) = std::fs::read_link(existing) {
            let from = existing.parent().unwrap_or(Path::new("/"));
            return links > 0 && leads_within(&from.join(target), links - 1);
        }
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return false,
        }
    }
}

fn under_devices(path: &Path) -> bool {
    spelt_into_devices(path)
}

/// **Whether a whole path, as it's spelt, is under `/dev`:** `.` and `//` taken out, `..` taken back, and `dev` matched whatever its case, as a
/// case-insensitive root reaches the devices by `/DEV` too. No call to the disk, so it's for a thread that mustn't wait (the person's open on the
/// session thread); where links lead is [`leads_into_devices`]'s, on the thread that reads. A relative path is never under it: it's refused as such.
#[must_use]
pub fn spelt_into_devices(path: &Path) -> bool {
    use std::path::Component;
    if !cfg!(unix) || !path.is_absolute() {
        return false;
    }
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Prefix(_) | Component::RootDir => parts.clear(),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(name) => parts.push(name),
        }
    }
    parts.first().is_some_and(|first| first.eq_ignore_ascii_case("dev"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::symlink;

    fn temp() -> recto_testpdf::TempDir {
        recto_testpdf::TempDir::new("recto-document-file-")
    }

    /// A file with no link holding `bytes`, as the person's print is.
    fn unlinked(dir: &Path, bytes: &[u8]) -> File {
        use std::io::Write;
        let path = dir.join("unlinked");
        let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        file.write_all(bytes).unwrap();
        file
    }

    fn refused(path: &Path) -> bool {
        matches!(open_regular(path), Err(Refused::NotADocument)) && (!path.exists() || matches!(open(path), Err(Refused::NotADocument)))
    }

    /// **Failure (S1): a file with no name, held by a descriptor, opens again through `/dev/fd/N` or a link to it**, a spelling of `/dev` the path's
    /// check misses (`//dev`, `/usr/../dev`), or a link to a descriptor that isn't open is told otherwise than one that is. Each is refused as not a
    /// document's file, and a file with no link is refused by its descriptor, whatever the path.
    #[test]
    fn a_descriptor_by_any_spelling_is_refused() {
        let dir = temp();
        let held = unlinked(dir.path(), b"%PDF-1.4 the print");
        let n = held.as_raw_fd();
        let named = dir.join("named.pdf");
        std::fs::write(&named, b"%PDF-1.4 named").unwrap();
        let named_held = File::open(&named).unwrap();
        let spellings = [
            format!("/dev/fd/{n}"),
            format!("//dev/./fd/{n}"),
            // `/usr` is a folder (`/tmp` is a link, whose `..` is `/private`).
            format!("/usr/../dev/fd/{n}"),
            format!("/dev/fd/{}", named_held.as_raw_fd()),
            "/dev/fd/999".into(),
            "/dev/stdin".into(),
            "/dev/null".into(),
            "/dev".into(),
            // A case-insensitive root reaches the devices by any case.
            format!("/DEV/fd/{n}"),
            format!("/Dev/fd/{n}"),
        ];
        for (k, spelling) in spellings.iter().enumerate() {
            assert!(refused(Path::new(spelling)), "{spelling}");
            let link = dir.join(format!("link-{k}"));
            symlink(spelling, &link).unwrap();
            assert!(refused(&link), "a link to {spelling}");
            let twice = dir.join(format!("link-to-link-{k}"));
            symlink(&link, &twice).unwrap();
            assert!(refused(&twice), "a link to a link to {spelling}");
        }
        let folder = dir.join("fd");
        symlink("/dev/fd", &folder).unwrap();
        assert!(refused(&folder.join(n.to_string())), "through a linked folder");
        assert!(refused(&folder.join("999")), "through a linked folder, to one that isn't open");
        assert!(matches!(admit(&held), Err(Refused::NotADocument)), "the descriptor's own check");
        for spelling in [
            format!("/DEV/fd/{n}"),
            format!("/Dev/fd/{n}"),
            format!("//dev/./fd/{n}"),
            format!("/usr/../dev/fd/{n}"),
            "/dev".into(),
        ] {
            assert!(spelt_into_devices(Path::new(&spelling)), "spelt: {spelling}");
        }
    }

    /// **Failure (the wall refusing a person's file):** a plain file, one reached through a link, one with a second name or held open elsewhere is
    /// refused; or a file that isn't there, or a folder, says otherwise than it did (not found from the look; a folder not a document's file).
    #[test]
    fn a_persons_files_open_as_before() {
        let dir = temp();
        let plain = dir.join("plain.pdf");
        std::fs::write(&plain, b"%PDF-1.4").unwrap();
        let link = dir.join("link.pdf");
        symlink(&plain, &link).unwrap();
        let second = dir.join("second.pdf");
        std::fs::hard_link(&plain, &second).unwrap();
        let _elsewhere = File::open(&plain).unwrap();
        for path in [&plain, &link, &second] {
            assert!(open(path).is_ok() && open_regular(path).is_ok(), "{}", path.display());
        }
        assert!(matches!(open_regular(&dir.join("missing.pdf")), Err(Refused::Look(error)) if error.kind() == io::ErrorKind::NotFound));
        assert!(matches!(open(&dir.join("missing.pdf")), Err(Refused::Open(error)) if error.kind() == io::ErrorKind::NotFound));
        assert!(refused(dir.path()), "a folder");
        assert!(
            !leads_into_devices(&dir.join("missing/../dev")),
            "a name like a device's in a folder of the person's"
        );
        assert!(
            !spelt_into_devices(&dir.join("dev/fd/3")) && !spelt_into_devices(Path::new("dev/fd/3")),
            "`dev` not at the root"
        );
    }
}
