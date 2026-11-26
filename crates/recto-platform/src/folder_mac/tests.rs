//! Each test is a measurement on this Mac of what the module's doc says, and holds the failure it names.

use super::*;
use recto_testpdf::TempDir;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;
use std::time::Duration;

fn names(folder: &Folder) -> Vec<String> {
    let mut names: Vec<String> = folder.entries().unwrap().map(|e| e.unwrap().name.to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn opened(path: &Path) -> Folder {
    match Folder::open(path) {
        Opened::Folder(folder) => folder,
        other => panic!("{other:?}"),
    }
}

/// Failure: a link, a file or a missing path is taken for a folder (a link followed to a folder somewhere else), or a missing one is called
/// something else than missing.
#[test]
fn only_a_real_folder_of_ours_opens() {
    let dir = TempDir::new("recto-folder-");
    let real = dir.join("real");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("link")).unwrap();
    std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("dangling")).unwrap();
    std::fs::write(dir.join("file"), b"x").unwrap();
    assert!(matches!(Folder::open(&real), Opened::Folder(_)));
    assert!(matches!(Folder::open(&dir.join("link")), Opened::Other), "a link to a folder");
    assert!(matches!(Folder::open(&dir.join("dangling")), Opened::Other), "a link to nothing");
    assert!(matches!(Folder::open(&dir.join("file")), Opened::Other), "a file");
    assert!(matches!(Folder::open(&dir.join("nowhere")), Opened::Missing));
    let parent = opened(dir.path());
    assert!(matches!(parent.open_in(OsStr::new("real")), Opened::Folder(_)));
    assert!(matches!(parent.open_in(OsStr::new("link")), Opened::Other), "a child that is a link");
    assert!(matches!(parent.open_in(OsStr::new("file")), Opened::Other));
    assert!(matches!(parent.open_in(OsStr::new("nowhere")), Opened::Missing));
}

/// Failure: the folder is listed by its path after the check, so a swap in between (renamed away, a link in its place) lists, and opens files
/// from, whatever the link leads to.
#[test]
fn a_swapped_path_changes_nothing_for_an_open_folder() {
    let dir = TempDir::new("recto-folder-");
    let real = dir.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("mine"), b"mine").unwrap();
    let elsewhere = TempDir::new("recto-folder-out-");
    std::fs::write(elsewhere.join("theirs"), b"theirs").unwrap();
    let folder = opened(&real);
    std::fs::rename(&real, dir.join("moved")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), &real).unwrap();
    assert_eq!(names(&folder), ["mine"]);
    assert_eq!(names(&folder), ["mine"], "a second listing starts again from the top");
    let mut text = String::new();
    folder.open_private_in(OsStr::new("mine")).unwrap().read_to_string(&mut text).unwrap();
    assert_eq!(text, "mine");
    assert!(folder.open_private_in(OsStr::new("theirs")).is_err());
}

/// Failure: a link, a pipe, a folder or a hard link is taken for a plain file with one name; `.` and `..` are listed.
#[test]
fn entries_say_what_each_is_without_following() {
    let dir = TempDir::new("recto-folder-");
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::fs::hard_link(dir.join("plain"), dir.join("twin")).unwrap();
    std::os::unix::fs::symlink(dir.join("plain"), dir.join("link")).unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(dir.join("pipe")).status().unwrap().success());
    let folder = opened(dir.path());
    let found: std::collections::HashMap<String, Entry> = folder
        .entries()
        .unwrap()
        .map(|e| e.unwrap())
        .map(|e| (e.name.to_string_lossy().into_owned(), e))
        .collect();
    assert_eq!(found.len(), 5);
    let shown = |name: &str| (found[name].is_file, found[name].links);
    assert_eq!(shown("plain"), (true, 2));
    assert_eq!(shown("twin"), (true, 2));
    assert!(!found["link"].is_file && !found["pipe"].is_file && !found["sub"].is_file);
    assert!(
        found["plain"].modified_ns.is_some_and(|ns| ns > 1_600_000_000_000_000_000),
        "a time in nanoseconds since 1970"
    );
}

/// Failure: a link is followed, a pipe with no writer is waited on, or a file others can write is read.
#[test]
fn open_private_in_refuses_what_it_should_and_never_waits() {
    let dir = TempDir::new("recto-folder-");
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("plain"), dir.join("link")).unwrap();
    std::fs::write(dir.join("open"), b"x").unwrap();
    std::fs::set_permissions(dir.join("open"), std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(dir.join("pipe")).status().unwrap().success());
    let folder = opened(dir.path());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let kinds: Vec<bool> = ["plain", "link", "open", "pipe", "absent"]
            .iter()
            .map(|n| folder.open_private_in(OsStr::new(n)).is_ok())
            .collect();
        let _ = sender.send(kinds);
    });
    let ok = receiver.recv_timeout(Duration::from_secs(10)).expect("a pipe was waited on");
    assert_eq!(ok, [true, false, false, false, false]);
}

/// Failure: a name that is not one plain name of this folder reaches `openat`. An absolute name makes `openat` ignore the folder's descriptor
/// (`/dev/fd/N` reopens descriptor N, such as a person's PDF), and `.`, `..`, an empty name or a `/` leave the folder.
#[test]
fn a_name_that_is_not_one_plain_name_is_refused() {
    let dir = TempDir::new("recto-folder-");
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("inner"), b"x").unwrap();
    std::fs::write(dir.join("plain"), b"x").unwrap();
    let folder = opened(dir.path());
    let held = std::fs::File::open(dir.join("plain")).unwrap();
    let fd_path = format!("/dev/fd/{}", std::os::fd::AsRawFd::as_raw_fd(&held));
    for (case, bad) in [
        ("empty", ""),
        ("dot", "."),
        ("dot dot", ".."),
        ("a slash inside", "sub/inner"),
        ("a leading slash", "/etc"),
        ("another descriptor", fd_path.as_str()),
    ] {
        assert!(matches!(folder.open_in(OsStr::new(bad)), Opened::Other), "open_in: {case}");
        assert!(folder.open_private_in(OsStr::new(bad)).is_err(), "open_private_in: {case}");
    }
    assert!(folder.open_private_in(OsStr::new("plain")).is_ok());
}

/// Failure: two listings of one folder alive together share one directory offset (a duplicated descriptor), so one's progress or rewind
/// changes what the other sees: entries skipped or listed twice.
#[test]
fn two_listings_at_once_do_not_disturb_each_other() {
    let dir = TempDir::new("recto-folder-");
    let count = 1500; // more than one read of the directory's buffer holds
    for i in 0..count {
        std::fs::write(dir.join(format!("file-with-a-longish-name-{i:05}")), b"").unwrap();
    }
    let folder = opened(dir.path());
    let mut first = folder.entries().unwrap();
    let mut seen_first: Vec<OsString> = (0..10).map(|_| first.next().unwrap().unwrap().name).collect();
    let second: Vec<OsString> = folder.entries().unwrap().map(|e| e.unwrap().name).collect();
    seen_first.extend(first.map(|e| e.unwrap().name));
    for (case, mut names) in [("the first, interrupted", seen_first), ("the second", second)] {
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "{case}: every entry once");
    }
}
