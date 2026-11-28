//! Each test is a measurement on this Mac of what the module's doc says, and holds the failure it names.

use super::*;
use recto_testpdf::TempDir;
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

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

/// Failure: a link, a file, a pipe or a missing path is taken for a folder (a link followed to a folder somewhere else), a pipe with no writer is
/// waited on, or a missing one is called something else than missing.
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
    assert!(std::process::Command::new("mkfifo").arg(dir.join("pipe")).status().unwrap().success());
    let pipe = dir.join("pipe");
    assert!(
        recto_testpdf::returns_in_time("a pipe was waited on", move || matches!(Folder::open(&pipe), Opened::Other)),
        "a pipe"
    );
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
    let ok = recto_testpdf::returns_in_time("a pipe was waited on", move || {
        ["plain", "link", "open", "pipe", "absent"]
            .iter()
            .map(|n| folder.open_private_in(OsStr::new(n)).is_ok())
            .collect::<Vec<bool>>()
    });
    assert_eq!(ok, [true, false, false, false, false]);
}

/// Failure: a record's file is made over a link or another file, made readable by others, or opened in place through a link or a pipe, a file others can write,
/// or by a name that leaves the folder; or a name is replaced by a rename instead of refused.
#[test]
fn a_record_file_is_made_new_opened_in_place_and_renamed_without_replacing() {
    use std::io::Write as _;
    let dir = TempDir::new("recto-folder-");
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("plain"), dir.join("link")).unwrap();
    std::fs::write(dir.join("open"), b"x").unwrap();
    std::fs::set_permissions(dir.join("open"), std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(dir.join("pipe")).status().unwrap().success());
    let folder = opened(dir.path());

    let mut made = folder.create_private_in(OsStr::new("new")).unwrap();
    made.write_all(b"abc").unwrap();
    assert_eq!(std::fs::metadata(dir.join("new")).unwrap().mode() & 0o777, 0o600);
    for taken in ["new", "plain", "link", "pipe"] {
        assert_eq!(
            folder.create_private_in(OsStr::new(taken)).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists,
            "{taken}"
        );
    }
    assert_eq!(std::fs::read(dir.join("plain")).unwrap(), b"x", "a link was written through");
    for bad in ["", ".", "..", "a/b", "/etc/x"] {
        assert!(folder.create_private_in(OsStr::new(bad)).is_err(), "create {bad:?}");
        assert!(folder.open_private_rw_in(OsStr::new(bad)).is_err(), "rw {bad:?}");
        assert!(folder.rename_excl_in(OsStr::new("new"), OsStr::new(bad)).is_err(), "rename {bad:?}");
    }

    let folder = std::sync::Arc::new(folder);
    let opens = {
        let folder = folder.clone();
        recto_testpdf::returns_in_time("a pipe was waited on", move || {
            ["plain", "link", "open", "pipe", "absent"].map(|n| folder.open_private_rw_in(OsStr::new(n)).is_ok())
        })
    };
    assert_eq!(opens, [true, false, false, false, false]);
    let mut rw = folder.open_private_rw_in(OsStr::new("new")).unwrap();
    rw.write_all(b"Z").unwrap();
    assert_eq!(std::fs::read(dir.join("new")).unwrap(), b"Zbc", "written in place");

    assert_eq!(
        folder.rename_excl_in(OsStr::new("new"), OsStr::new("plain")).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(dir.join("plain")).unwrap(), b"x", "a name was replaced");
    folder.rename_excl_in(OsStr::new("new"), OsStr::new("named")).unwrap();
    assert!(!dir.join("new").exists() && dir.join("named").exists());
    assert_eq!(
        folder.rename_excl_in(OsStr::new("new"), OsStr::new("other")).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
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

/// Failure: a folder entry isn't told from a file (lopdf's `.d` clone folder, or a person's folder, read as a plain file), or a link to a folder or a
/// pipe is told as a folder (a link followed, so a `.d` that is a link to someone's folder is looked into as lopdf's own).
#[test]
fn entries_say_which_is_a_file_and_never_follow_a_link() {
    let dir = TempDir::new("recto-folder-");
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("sub"), dir.join("link")).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(dir.join("pipe")).status().unwrap().success());
    let folder = opened(dir.path());
    let found: std::collections::HashMap<String, Entry> = folder
        .entries()
        .unwrap()
        .map(|e| e.unwrap())
        .map(|e| (e.name.to_string_lossy().into_owned(), e))
        .collect();
    assert!(found["plain"].is_file, "a plain file");
    assert!(
        !found["sub"].is_file && !found["link"].is_file && !found["pipe"].is_file,
        "a folder, a link to one and a pipe are not files"
    );
}

/// Failure: what an entry or a folder is, is read by following a link (a link to a folder taken for a folder), or the identity of a name changes when its
/// file is replaced by another of the same name and nobody can tell (the device and inode are what a later act compares).
#[test]
fn identity_and_look_say_what_a_name_is_without_following() {
    let dir = TempDir::new("recto-folder-");
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::os::unix::fs::symlink(dir.join("sub"), dir.join("link")).unwrap();
    let folder = opened(dir.path());
    let meta = std::fs::metadata(dir.path()).unwrap();
    let id = folder.identity().unwrap();
    assert_eq!((id.dev, id.ino, id.kind), (meta.dev(), meta.ino(), Kind::Folder));
    let kinds: Vec<Kind> = ["plain", "sub", "link"].iter().map(|n| folder.look(OsStr::new(n)).unwrap().kind).collect();
    assert_eq!(kinds, [Kind::File, Kind::Folder, Kind::Link]);
    let old = folder.look(OsStr::new("plain")).unwrap().identity();
    std::fs::remove_file(dir.join("plain")).unwrap();
    std::fs::write(dir.join("plain"), b"y").unwrap();
    assert_ne!(
        folder.look(OsStr::new("plain")).unwrap().identity(),
        old,
        "another file under the same name is another identity"
    );
    assert_eq!(folder.look(OsStr::new("nothing")).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert!(folder.look(OsStr::new("../x")).is_err());
}

/// Failure: a folder is said not to be on a local volume (so a dead process number in it is taken as another Mac's and nothing in it is ever moved), or is
/// asked by its path and not by itself. A temp folder is local, asked of the opened folder. (The stand-in off the Mac answers `false` by its code, which
/// is what puts a dead process number there in `waiting`; it can't run here.)
#[test]
fn a_folder_asked_for_itself_is_local_on_this_disk() {
    let dir = TempDir::new("recto-folder-");
    assert!(opened(dir.path()).is_local());
}

/// Failure: `unlink_in` follows a link and removes what it leads to, removes a folder with something in it, takes a name that leaves the folder, or a
/// missing name is not `NotFound`.
#[test]
fn unlink_in_takes_out_one_name_and_never_follows() {
    let dir = TempDir::new("recto-folder-");
    let elsewhere = TempDir::new("recto-folder-out-");
    std::fs::write(elsewhere.join("theirs"), b"theirs").unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), dir.join("link")).unwrap();
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::fs::create_dir(dir.join("full")).unwrap();
    std::fs::write(dir.join("full").join("in"), b"x").unwrap();
    std::fs::create_dir(dir.join("empty")).unwrap();
    let folder = opened(dir.path());
    folder.unlink_in(OsStr::new("link"), false).unwrap();
    assert!(std::fs::symlink_metadata(dir.join("link")).is_err());
    assert_eq!(
        std::fs::read(elsewhere.join("theirs")).unwrap(),
        b"theirs",
        "what the link led to is still there"
    );
    folder.unlink_in(OsStr::new("plain"), false).unwrap();
    assert!(folder.unlink_in(OsStr::new("empty"), false).is_err(), "a folder is taken out as a folder only");
    folder.unlink_in(OsStr::new("empty"), true).unwrap();
    assert!(folder.unlink_in(OsStr::new("full"), true).is_err(), "a folder with something in it stays");
    assert!(dir.join("full").join("in").exists());
    assert_eq!(folder.unlink_in(OsStr::new("plain"), false).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    for bad in ["", ".", "..", "full/in", "/etc/hosts"] {
        assert!(folder.unlink_in(OsStr::new(bad), false).is_err(), "{bad:?}");
    }
    assert!(dir.join("full").join("in").exists());
}

/// Failure: `path` says where the folder was opened and not where it is, so a check that the path still leads to the folder compares the old name; or `sync`
/// of a folder fails.
#[test]
fn path_follows_the_folder_and_sync_works() {
    let dir = TempDir::new("recto-folder-");
    let real = dir.path().canonicalize().unwrap();
    std::fs::create_dir(real.join("before")).unwrap();
    let folder = opened(&real.join("before"));
    assert_eq!(folder.path().unwrap(), real.join("before"));
    std::fs::rename(real.join("before"), real.join("after")).unwrap();
    assert_eq!(folder.path().unwrap(), real.join("after"), "the descriptor's own place now");
    folder.sync().unwrap();
}

/// Failure (review 7): the walk finds a pinned entry by a linear search, which costs a scan of the folder for every entry (quadratic in a big copy). The tree
/// is sorted and searched by bisection, so a tree that isn't sorted finds nothing and the walk stops, taking nothing out.
#[test]
fn a_walk_finds_a_pinned_entry_by_bisection() {
    use crate::folder::remove::{Failure, Walk, remove_tree_in};
    let dir = TempDir::new("recto-folder-");
    std::fs::create_dir(dir.join("copy")).unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(dir.join("copy").join(name), b"x").unwrap();
    }
    let parent = opened(dir.path());
    let copy = opened(&dir.join("copy"));
    let pinned = |name: &str| Pinned {
        name: OsString::from(name),
        id: copy.look(OsStr::new(name)).unwrap().identity(),
        below: None,
    };
    let rank = |_: usize, _: &OsStr, _: Kind| 1u8;
    let mut go = |_: &OsStr| true;
    let sorted = Tree {
        id: copy.identity().unwrap(),
        entries: vec![pinned("a"), pinned("b"), pinned("c")],
    };
    let mut unsorted = sorted.clone();
    unsorted.entries.reverse();
    let mut walk = Walk {
        rank: &rank,
        gate: &mut go,
        unlinked: &mut || {},
    };
    assert!(matches!(
        remove_tree_in(&parent, OsStr::new("copy"), &unsorted, &mut walk),
        Err(Failure::Unpinned(_))
    ));
    assert!(dir.join("copy").join("a").exists(), "nothing was taken out");
    assert!(remove_tree_in(&parent, OsStr::new("copy"), &sorted, &mut walk).is_ok());
    assert!(!dir.join("copy").exists());
}

/// Every image this probe attached, detached on drop (a failed assertion included), by the mount folder it made, and only those.
struct Attached(Vec<std::path::PathBuf>);

impl Drop for Attached {
    fn drop(&mut self) {
        for mount in self.0.drain(..) {
            let plain = std::process::Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
            if !plain.is_ok_and(|s| s.success()) {
                let _ = std::process::Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&mount).status();
            }
        }
    }
}

/// The measurement behind the removal surveys of a document on an external disk (design §10, D2c-2b): does `Folder::open` take a folder on a
/// volume mounted with ownership off (`hdiutil attach -owners off`, as an external disk often is)? There every file reads as owned by the user
/// looking, or by someone else, and `Folder::open` checks the owner. Not a regression test: it prints what it found and asserts nothing about
/// ownership, because the answer is what the design needs. `--ignored`; makes one 20 MiB HFS+ image in `$RECTO_PROBE_DIR` (or the temp folder),
/// attaches it with `-nobrowse -noautoopen` at a mount folder of its own, and detaches it in a drop guard; it touches no other disk.
///
/// **Result, 2026-11-26, macOS 27.0.1 (26A434), 20 MiB HFS+ image, `-owners off`: `statfs` `f_flags` was `0x4309218`, `MNT_IGNORE_OWNERSHIP` set (asserted, so the
/// probe proves ownership was off); the folder read as uid 501, ours, and `Folder::open` returned
/// `Folder` for a folder in it and for the volume's root.** So a document on such a disk is surveyed; a volume that shows another owner (a network or
/// a foreign disk) still reads `Other` and refuses, which is meant. Not measured: an external physical disk, a network share.
#[test]
#[ignore = "attaches a disk image"]
fn folder_open_on_a_volume_with_ownership_off() {
    use std::process::Command;
    let base = std::env::var_os("RECTO_PROBE_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let dir = TempDir::new_in(&base, "recto-owners-off-");
    let image = dir.join("probe.dmg");
    let mount = dir.join("mount");
    let run = |program: &str, args: &[&std::ffi::OsStr]| {
        let out = Command::new(program).args(args).output().expect("run");
        assert!(
            out.status.success(),
            "{program} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let os = |s: &str| std::ffi::OsString::from(s);
    run(
        "hdiutil",
        &[
            &os("create"),
            &os("-quiet"),
            &os("-size"),
            &os("20m"),
            &os("-fs"),
            &os("HFS+"),
            &os("-volname"),
            &os("probe"),
            image.as_os_str(),
        ],
    );
    run("mkdir", &[mount.as_os_str()]);
    let mut attached = Attached(Vec::new());
    run(
        "hdiutil",
        &[
            &os("attach"),
            &os("-owners"),
            &os("off"),
            &os("-nobrowse"),
            &os("-noautoopen"),
            &os("-mountpoint"),
            mount.as_os_str(),
            image.as_os_str(),
        ],
    );
    attached.0.push(mount.clone());
    run("mkdir", &[mount.join("docs").as_os_str()]);
    run("touch", &[mount.join("docs").join("report.pdf").as_os_str()]);
    // The mount itself says ownership is off, so the result below is about such a volume and not one that merely looks like it.
    let flags = crate::volume::mount_of(&mount).expect("statfs of the mount").flags;
    assert_ne!(
        flags & libc::MNT_IGNORE_OWNERSHIP as u32,
        0,
        "the image is mounted with ownership off (f_flags {flags:#x})"
    );
    let meta = std::fs::metadata(mount.join("docs")).unwrap();
    let docs = match Folder::open(&mount.join("docs")) {
        Opened::Folder(_) => "Folder",
        Opened::Other => "Other",
        Opened::Missing => "Missing",
    };
    let root = match Folder::open(&mount) {
        Opened::Folder(_) => "Folder",
        Opened::Other => "Other",
        Opened::Missing => "Missing",
    };
    println!(
        "owners-off probe: f_flags={flags:#x} (MNT_IGNORE_OWNERSHIP set) folder uid={} (ours {}) Folder::open(docs)={docs} Folder::open(volume root)={root}",
        std::os::unix::fs::MetadataExt::uid(&meta),
        crate::local::current_uid()
    );
}

/// Failure: an entry's owner is not read from the listing (always 0, or left out), so "this user's" has to be asked by a second look at the path, which a swap
/// between the two looks can answer for another file.
#[test]
fn entries_say_their_owner() {
    let dir = TempDir::new("recto-folder-");
    std::fs::write(dir.join("plain"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("plain"), dir.join("link")).unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    let me = crate::local::current_uid();
    let folder = opened(dir.path());
    let owners: Vec<u32> = folder.entries().unwrap().map(|e| e.unwrap().uid).collect();
    assert_eq!(
        owners,
        vec![me; 3],
        "a file, a link (its own, not its target's) and a folder are all this user's"
    );
    assert_eq!(folder.look(OsStr::new("plain")).unwrap().uid, me, "a look says it too");
}
