//! **Handing a development build over without the app ever opening a path an agent named** (owner, 2026-11-12: `recto update install` on a zip on an external volume hung for 14 minutes, the app's updater thread inside `open()` while macOS asked a question nobody answered).
//!
//! The `recto` process, which runs with its caller's file access, opens the zip ([`bring_in`]) and puts a copy in the app's own **hand-over folder** (`update/handover`, private, made by the app at launch): a clone where the two are on one volume, so nothing's written twice, and otherwise a stream of 64 KiB at a time, stopped and deleted at the size cap. The command then names that copy, and the app opens only it ([`open_inside`]).
//!
//! Anything running as this user can write in that folder, so the app trusts nothing in it: it opens the copy once, without following a link and without waiting, looks at that descriptor (an ordinary file, this user's, within the cap) and unpacks from that same descriptor, never opening by name again. A FIFO or a device in its place is refused at once. At most one copy waits: bringing in another replaces it, the app deletes it when the install has finished, been refused or failed, and a launch clears the folder.
//!
//! Words and limits are decided here; the `recto` binary only calls this.

use crate::fetch::MAX_ZIP_BYTES;
use crate::plan::Files;
use crate::updater::builds::version_of_zip;
use recto_platform::local;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The most a copy may be, counted as it's written, whatever the source's size said.
pub const MAX_BYTES: u64 = MAX_ZIP_BYTES;
/// The folder's name inside the update folder.
pub const FOLDER: &str = "handover";
const PART: &str = ".part";

/// How long `recto` waits on a file that won't open. The first is only for saying what it's waiting for; the second is for giving up, long enough for a person to read macOS's question and answer it.
#[derive(Clone, Copy, Debug)]
pub struct Waits {
    pub say_after: Duration,
    pub give_up: Duration,
}

/// What the command uses.
pub const WAITS: Waits = Waits {
    say_after: Duration::from_secs(5),
    give_up: Duration::from_secs(120),
};

/// Why a zip wasn't handed over or taken: the code and the words, which say what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The path isn't inside the hand-over folder: the app opens nothing else.
    NotHere,
    NotABuild,
    NotAFile,
    TooBig,
    NotYours,
    NoAccess,
    Gone,
    /// Waiting for macOS to allow access to this volume ran out.
    Slow(String),
    Cancelled,
    NoFolder,
    NoRoom,
    Unreadable,
}

impl Refusal {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotHere => "not_handed_over",
            Self::NotABuild => "not_a_build",
            Self::NotAFile => "zip_not_a_file",
            Self::TooBig => "zip_too_large",
            Self::NotYours => "zip_not_yours",
            Self::NoAccess => "zip_no_access",
            Self::Gone => "zip_gone",
            Self::Slow(_) => "zip_access_slow",
            Self::Cancelled => "cancelled",
            Self::NoFolder => "no_handover_folder",
            Self::NoRoom => "disk_full",
            Self::Unreadable => "zip_unreadable",
        }
    }

    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Self::NotHere => "RectoPDF opens a build only from its own hand-over folder. `recto update install <zip>` puts a copy there and names it, so use that".to_owned(),
            Self::NotABuild => "the zip has to be named RectoPDF-X.Y.Z.N.zip, as `tools/dev-build.sh` writes it".to_owned(),
            Self::NotAFile => "that isn't an ordinary file: it has to be the build's zip itself, not a link, a folder or a device".to_owned(),
            Self::TooBig => format!("that file is larger than any build could be (over {} MiB)", MAX_BYTES / (1024 * 1024)),
            Self::NotYours => "that file belongs to another user".to_owned(),
            Self::NoAccess => "macOS didn't let this program read that file. Allow the terminal you run `recto` in under System Settings › Privacy & Security › Files and Folders, then run the command again".to_owned(),
            Self::Gone => "that file isn't there".to_owned(),
            Self::Slow(volume) => format!(
                "gave up after {} seconds waiting for macOS to allow access to {volume}. Choose Allow when macOS asks, then run the command again, or copy the zip into your home folder first",
                WAITS.give_up.as_secs()
            ),
            Self::Cancelled => "stopped, and nothing was handed over".to_owned(),
            Self::NoFolder => "RectoPDF hasn't set up its folder for builds: quit and reopen it, then run the command again".to_owned(),
            Self::NoRoom => "the disk is full".to_owned(),
            Self::Unreadable => "that file couldn't be read".to_owned(),
        }
    }
}

/// The folder in the update folder.
#[must_use]
pub fn folder_of(files: &Files) -> PathBuf {
    files.work.join(FOLDER)
}

/// Makes the folder, private, and empties it. At launch, so nothing left by a run that ended halfway stays, and nothing a hostile process left there is trusted.
pub fn prepare(files: &Files) -> io::Result<()> {
    local::ensure_private_dir(&files.work)?;
    let folder = folder_of(files);
    local::ensure_private_dir(&folder)?;
    empty(&folder)
}

fn empty(folder: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(folder)?.flatten() {
        let path = entry.path();
        // A link or a file is removed itself, never followed.
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

/// The copy is done with: after the install, a refusal or a failure. Only a name directly in the folder is touched.
pub fn discard(path: &Path, folder: &Path) {
    if path.parent() == Some(folder) {
        let _ = std::fs::remove_file(path);
    }
}

/// What's waiting in the folder, for tests and the report.
#[must_use]
pub fn waiting_in(folder: &Path) -> usize {
    std::fs::read_dir(folder).map_or(0, |entries| entries.flatten().count())
}

/// **The app's side:** the one place it opens a zip an agent handed over. The path must be a name directly in the hand-over folder, which must be private to this user. It's opened without following a link and without waiting, then the descriptor itself is looked at: an ordinary file, this user's, within the cap. What's returned is the only handle the caller reads from.
pub fn open_inside(path: &Path, folder: &Path) -> Result<File, Refusal> {
    if path.parent() != Some(folder) || path.file_name().is_none() {
        return Err(Refusal::NotHere);
    }
    local::check_private_dir(folder).map_err(|_| Refusal::NoFolder)?;
    let file = local::open_untrusted(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => Refusal::Gone,
        io::ErrorKind::PermissionDenied => Refusal::NoAccess,
        // A link at the name, or a FIFO or device opened some other way: not the file it has to be.
        _ => Refusal::NotAFile,
    })?;
    let meta = file.metadata().map_err(|_| Refusal::Unreadable)?;
    if !meta.is_file() {
        return Err(Refusal::NotAFile);
    }
    if meta.len() > MAX_BYTES {
        return Err(Refusal::TooBig);
    }
    if local::owner_of(&file).map_err(|_| Refusal::Unreadable)? != local::current_uid() {
        return Err(Refusal::NotYours);
    }
    Ok(file)
}

/// The volume a path is on, as a person reads it: `Backup` for `/Volumes/Backup/x.zip`, or else the folder it's in.
#[must_use]
pub fn volume_name(path: &Path) -> String {
    let mut parts = path.components().filter_map(|part| part.as_os_str().to_str());
    let first = parts.next();
    if first == Some("/")
        && parts.next() == Some("Volumes")
        && let Some(name) = parts.next()
    {
        return format!("the volume “{name}”");
    }
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map_or_else(|| "that folder".to_owned(), |name| format!("the folder “{name}”"))
}

/// What's said after five seconds of an open that hasn't come back.
#[must_use]
pub fn waiting_words(path: &Path) -> String {
    format!(
        "Waiting for macOS to allow access to {}… If it is asking, choose Allow. Gives up after {} seconds.",
        volume_name(path),
        WAITS.give_up.as_secs()
    )
}

/// **The command's side**: opens `from`, and puts a copy in `folder` named as the zip is, replacing whatever waited there. Returns the copy's path.
pub fn bring_in(from: &Path, folder: &Path, waits: Waits, cancel: &AtomicBool, say: &mut dyn FnMut(&str)) -> Result<PathBuf, Refusal> {
    let path = from.to_path_buf();
    bring_in_with(from, folder, waits, cancel, say, move || local::open_untrusted(&path))
}

/// [`bring_in`] with how the file is opened, for a test of an open that never comes back.
pub fn bring_in_with(
    from: &Path,
    folder: &Path,
    waits: Waits,
    cancel: &AtomicBool,
    say: &mut dyn FnMut(&str),
    open: impl FnOnce() -> io::Result<File> + Send + 'static,
) -> Result<PathBuf, Refusal> {
    bring_in_limited(
        from,
        folder,
        waits,
        cancel,
        say,
        open,
        Limits {
            cap: MAX_BYTES,
            may_clone: true,
        },
    )
}

/// The size cap and whether a clone is tried first, so a test can reach the streaming path and its cap without a file of half a gigabyte.
#[derive(Clone, Copy)]
struct Limits {
    cap: u64,
    may_clone: bool,
}

fn bring_in_limited(
    from: &Path,
    folder: &Path,
    waits: Waits,
    cancel: &AtomicBool,
    say: &mut dyn FnMut(&str),
    open: impl FnOnce() -> io::Result<File> + Send + 'static,
    limits: Limits,
) -> Result<PathBuf, Refusal> {
    let name = from
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| version_of_zip(name).is_some())
        .ok_or(Refusal::NotABuild)?;
    let (sender, receiver) = mpsc::channel();
    // The open is the one call that can wait on the system, so it has a thread of its own, left behind if it never returns: this process ends soon after.
    std::thread::Builder::new()
        .name("recto-open".into())
        .spawn(move || {
            let _ = sender.send(open());
        })
        .map_err(|_| Refusal::Unreadable)?;
    let began = Instant::now();
    let mut said = false;
    let file = loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Refusal::Cancelled);
        }
        let waited = began.elapsed();
        if waited >= waits.give_up {
            return Err(Refusal::Slow(volume_name(from)));
        }
        if !said && waited >= waits.say_after {
            said = true;
            say(&waiting_words(from));
        }
        let next = if said { waits.give_up } else { waits.say_after }
            .saturating_sub(waited)
            .min(Duration::from_millis(100));
        match receiver.recv_timeout(next) {
            Ok(Ok(file)) => break file,
            Ok(Err(error)) => {
                return Err(match error.kind() {
                    io::ErrorKind::NotFound => Refusal::Gone,
                    io::ErrorKind::PermissionDenied => Refusal::NoAccess,
                    _ => Refusal::NotAFile,
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Refusal::Unreadable),
        }
    };
    let meta = file.metadata().map_err(|_| Refusal::Unreadable)?;
    if !meta.is_file() {
        return Err(Refusal::NotAFile);
    }
    if meta.len() > limits.cap {
        return Err(Refusal::TooBig);
    }
    local::check_private_dir(folder).map_err(|_| Refusal::NoFolder)?;
    // At most one waits: this one replaces it.
    empty(folder).map_err(|_| Refusal::NoFolder)?;
    let part = folder.join(PART);
    let target = folder.join(name);
    let made = copy_into(&file, &part, cancel, limits);
    if let Err(refusal) = made {
        let _ = std::fs::remove_file(&part);
        return Err(refusal);
    }
    std::fs::rename(&part, &target).map_err(|_| {
        let _ = std::fs::remove_file(&part);
        Refusal::NoRoom
    })?;
    Ok(target)
}

/// A clone if the volume allows, so nothing is written twice, else a stream. Either way the result is checked against the cap.
fn copy_into(file: &File, part: &Path, cancel: &AtomicBool, limits: Limits) -> Result<(), Refusal> {
    if limits.may_clone && local::clone_open_file(file, part).is_ok() {
        return if std::fs::metadata(part).is_ok_and(|meta| meta.len() <= limits.cap) {
            Ok(())
        } else {
            Err(Refusal::TooBig)
        };
    }
    let mut out = local::create_private_file(part).map_err(|_| Refusal::NoFolder)?;
    let mut input = file.take(limits.cap + 1);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Refusal::Cancelled);
        }
        let count = input.read(&mut buffer).map_err(|_| Refusal::Unreadable)?;
        if count == 0 {
            return Ok(());
        }
        written += count as u64;
        if written > limits.cap {
            return Err(Refusal::TooBig);
        }
        out.write_all(&buffer[..count]).map_err(|error| {
            if error.kind() == io::ErrorKind::StorageFull {
                Refusal::NoRoom
            } else {
                Refusal::Unreadable
            }
        })?;
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::process::Command;

    const BUILD: &str = "RectoPDF-0.1.0.2.zip";
    const FAST: Waits = Waits {
        say_after: Duration::from_millis(40),
        give_up: Duration::from_millis(400),
    };

    fn bed(name: &str) -> (recto_testpdf::TempDir, PathBuf, PathBuf) {
        let dir = crate::stage::tests::temp_dir(&format!("h-{name}"));
        let files = Files::in_state(&dir.join("state"));
        prepare(&files).unwrap();
        let folder = folder_of(&files);
        let source = dir.join("elsewhere");
        std::fs::create_dir_all(&source).unwrap();
        (dir, folder, source)
    }

    fn zip_with(path: &Path, bytes: usize) {
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    fn quiet(_: &str) {}

    fn opener(path: &Path) -> impl FnOnce() -> io::Result<File> + Send + 'static {
        let path = path.to_path_buf();
        move || File::open(path)
    }

    /// Failure: a path the agent named outside the hand-over folder is opened by the app, which is the whole hang: the app must open nothing but its own copy.
    #[test]
    fn the_app_opens_nothing_outside_its_folder() {
        let (dir, folder, source) = bed("outside");
        let zip = source.join(BUILD);
        zip_with(&zip, 10);
        assert_eq!(open_inside(&zip, &folder).unwrap_err(), Refusal::NotHere);
        assert_eq!(open_inside(&folder.join("sub").join(BUILD), &folder).unwrap_err(), Refusal::NotHere);
        assert!(Refusal::NotHere.words().contains("recto update install"), "says the next step");
        let inside = folder.join(BUILD);
        zip_with(&inside, 10);
        assert!(open_inside(&inside, &folder).is_ok());
        drop(dir);
    }

    /// Failure: a link, a FIFO with no writer, or a file over the cap in the folder is accepted, or an open of a FIFO waits for a writer that never comes and holds the updater thread, as the owner's 14-minute hang did.
    #[test]
    fn a_link_a_fifo_and_a_giant_in_the_folder_are_refused_at_once() {
        let (dir, folder, source) = bed("hostile");
        let real = source.join("real.zip");
        zip_with(&real, 10);
        let link = folder.join("RectoPDF-0.1.0.3.zip");
        symlink(&real, &link).unwrap();
        assert_eq!(open_inside(&link, &folder).unwrap_err(), Refusal::NotAFile, "a link");
        let fifo = folder.join("RectoPDF-0.1.0.4.zip");
        assert!(Command::new("/usr/bin/mkfifo").arg(&fifo).status().unwrap().success());
        let (sender, receiver) = mpsc::channel();
        let (folder_copy, fifo_copy) = (folder.clone(), fifo.clone());
        std::thread::spawn(move || {
            let _ = sender.send(open_inside(&fifo_copy, &folder_copy).map(|_| ()));
        });
        let answer = receiver.recv_timeout(Duration::from_secs(10)).expect("a FIFO with no writer held the open");
        assert_eq!(answer.unwrap_err(), Refusal::NotAFile, "a FIFO");
        let big = folder.join("RectoPDF-0.1.0.5.zip");
        File::create(&big).unwrap().set_len(MAX_BYTES + 1).unwrap();
        assert_eq!(open_inside(&big, &folder).unwrap_err(), Refusal::TooBig);
        assert!(Refusal::TooBig.words().contains("512 MiB"));
        drop(dir);
    }

    /// Failure: an open that never comes back (macOS's question about a removable volume nobody answered) holds the command for good, or says nothing of what it's waiting for, or gives up with no word of what to do.
    #[test]
    fn an_open_that_never_returns_is_answered_in_the_bound_with_what_it_waits_for() {
        let (dir, folder, _) = bed("slow");
        let from = Path::new("/Volumes/Backup/builds").join(BUILD);
        let (hold, never) = mpsc::channel::<()>();
        let mut said = Vec::new();
        let began = Instant::now();
        let answer = bring_in_with(
            &from,
            &folder,
            FAST,
            &AtomicBool::new(false),
            &mut |words| said.push(words.to_owned()),
            move || {
                let _ = never.recv();
                Err(io::ErrorKind::Other.into())
            },
        );
        assert!(began.elapsed() < Duration::from_secs(10), "bounded");
        let refusal = answer.unwrap_err();
        assert_eq!(refusal, Refusal::Slow("the volume “Backup”".into()));
        assert_eq!(said.len(), 1, "said once: {said:?}");
        assert!(said[0].starts_with("Waiting for macOS to allow access to the volume “Backup”…"), "{}", said[0]);
        assert!(
            refusal.words().contains("Choose Allow") && refusal.words().contains("run the command again"),
            "{}",
            refusal.words()
        );
        assert_eq!(waiting_in(&folder), 0, "nothing left");
        drop(hold);
        drop(dir);
    }

    /// Failure: a source that opens at once is announced as waiting, or the first seconds of a slow one aren't kept quiet.
    #[test]
    fn a_source_that_opens_at_once_says_nothing() {
        let (dir, folder, source) = bed("fast");
        let zip = source.join(BUILD);
        zip_with(&zip, 3000);
        let mut said = 0;
        let copy = bring_in(&zip, &folder, WAITS, &AtomicBool::new(false), &mut |_| said += 1).unwrap();
        assert_eq!((said, copy.clone()), (0, folder.join(BUILD)));
        assert_eq!(std::fs::read(&copy).unwrap().len(), 3000);
        drop(dir);
    }

    /// Failure: a second hand-over leaves the first waiting too, so copies of tens of megabytes pile up in the folder.
    #[test]
    fn at_most_one_copy_waits() {
        let (dir, folder, source) = bed("one");
        for name in ["RectoPDF-0.1.0.2.zip", "RectoPDF-0.1.0.3.zip"] {
            let zip = source.join(name);
            zip_with(&zip, 100);
            bring_in(&zip, &folder, FAST, &AtomicBool::new(false), &mut quiet).unwrap();
        }
        assert_eq!(waiting_in(&folder), 1);
        assert!(folder.join("RectoPDF-0.1.0.3.zip").exists());
        drop(dir);
    }

    /// Failure: a source that grows past the cap while it's streamed is trusted by its first stat and fills the disk, or its partial copy stays behind.
    #[test]
    fn streaming_stops_at_the_cap_and_deletes_what_it_wrote() {
        let (dir, folder, source) = bed("cap");
        let zip = source.join(BUILD);
        zip_with(&zip, 5000);
        let file = File::open(&zip).unwrap();
        let limits = Limits { cap: 1000, may_clone: false };
        assert_eq!(copy_into(&file, &folder.join(PART), &AtomicBool::new(false), limits), Err(Refusal::TooBig));
        // Through the whole path, where the first stat says too big already, and where it says fine: nothing stays.
        let open = opener(&zip);
        let stat_lies = Limits { cap: 5000, may_clone: false };
        let copy = bring_in_limited(&zip, &folder, FAST, &AtomicBool::new(false), &mut quiet, open, stat_lies).unwrap();
        discard(&copy, &folder);
        let tight = Limits { cap: 4999, may_clone: false };
        let answer = bring_in_limited(&zip, &folder, FAST, &AtomicBool::new(false), &mut quiet, opener(&zip), tight);
        assert_eq!(answer, Err(Refusal::TooBig));
        assert_eq!(waiting_in(&folder), 0);
        drop(dir);
    }

    /// Failure: Ctrl-C leaves a half-written copy in the app's folder, or the command goes on after it.
    #[test]
    fn an_interrupt_ends_it_with_nothing_left() {
        let (dir, folder, source) = bed("ctrl-c");
        let zip = source.join(BUILD);
        zip_with(&zip, 200_000);
        let flag = AtomicBool::new(true);
        let limits = Limits {
            cap: MAX_BYTES,
            may_clone: false,
        };
        let answer = bring_in_limited(&zip, &folder, FAST, &flag, &mut quiet, opener(&zip), limits);
        assert_eq!(answer, Err(Refusal::Cancelled));
        assert_eq!(waiting_in(&folder), 0);
        drop(dir);
    }

    /// Failure: where a clone is possible the file is written a second time anyway (SSD wear), or a clone isn't the same bytes.
    #[test]
    fn on_one_volume_the_copy_is_a_clone() {
        let (dir, folder, source) = bed("clone");
        let zip = source.join(BUILD);
        zip_with(&zip, 100_000);
        let file = File::open(&zip).unwrap();
        let part = folder.join(PART);
        if local::clone_open_file(&file, &part).is_err() {
            // Not an APFS volume: there's nothing to prove here.
            return;
        }
        assert_eq!(std::fs::read(&part).unwrap(), std::fs::read(&zip).unwrap());
        let copy = bring_in(&zip, &folder, FAST, &AtomicBool::new(false), &mut quiet).unwrap();
        assert_eq!(std::fs::read(copy).unwrap().len(), 100_000);
        drop(dir);
    }

    /// Failure: what a run left in the folder, or a hostile process put there, is trusted after a launch.
    #[test]
    fn a_launch_empties_the_folder_and_keeps_it_private() {
        let (dir, folder, source) = bed("launch");
        zip_with(&folder.join(BUILD), 10);
        symlink(source, folder.join("link")).unwrap();
        std::fs::create_dir(folder.join("dir")).unwrap();
        prepare(&Files::in_state(&dir.join("state"))).unwrap();
        assert_eq!(waiting_in(&folder), 0);
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        drop(dir);
    }

    /// Failure: a name that isn't a build's, or a missing file, gets as far as the app.
    #[test]
    fn the_command_refuses_what_is_no_build_or_not_there() {
        let (dir, folder, source) = bed("names");
        let odd = source.join("build.zip");
        zip_with(&odd, 10);
        assert_eq!(bring_in(&odd, &folder, FAST, &AtomicBool::new(false), &mut quiet), Err(Refusal::NotABuild));
        assert_eq!(
            bring_in(&source.join(BUILD), &folder, FAST, &AtomicBool::new(false), &mut quiet),
            Err(Refusal::Gone)
        );
        let link = source.join("RectoPDF-0.1.0.9.zip");
        symlink(&odd, &link).unwrap();
        assert_eq!(
            bring_in(&link, &folder, FAST, &AtomicBool::new(false), &mut quiet),
            Err(Refusal::NotAFile),
            "a link isn't followed"
        );
        drop(dir);
    }
}
