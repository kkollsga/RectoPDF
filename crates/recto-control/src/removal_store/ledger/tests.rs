//! The ledger's writer and reader, in process on temp state folders. Each test names the failure it catches.

use super::*;
use crate::host::{Host, SessionHost};
use crate::state::StatePaths;
use recto_agent::saving::SaveNote;
use recto_agent::{Actor, Command, DrawableLoader, Level, TabRef};
use recto_core::removal::{DESIGN_LIMITS, Disposition, Kinds, Limits, Refusal, Request, Store, Survey, TargetMode, lineage, plan};
use recto_testpdf::TempDir;
use std::num::NonZeroU16;
use std::sync::{Mutex, mpsc};

fn temp() -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-ledger-")
}

fn pdf(marker: &str) -> Vec<u8> {
    let page = recto_testpdf::Page::new(recto_testpdf::text_lines(&[marker]));
    let title = recto_testpdf::Object::Dict(format!("<< /Title ({marker}) >>"));
    recto_testpdf::pages_with(&[page], "", vec![title])
}

fn stamp(path: &Path) -> FileStamp {
    recto_platform::file_stamp(path).unwrap()
}

fn version(n: u64) -> FileVersion {
    FileVersion {
        volume: Some([9; 16]),
        inode: n,
        size: 10,
        mtime_ns: 1,
    }
}

fn at(n: u64) -> Entry {
    Entry::Link(Link::At {
        version: version(n),
        path: PathBuf::from(format!("/docs/{n}.pdf")),
    })
}

fn small(max_links: u32) -> Config {
    Config {
        limits: LedgerLimits { max_links, ..LEDGER_LIMITS },
        ..Config::default()
    }
}

fn ledger_path(root: &Path) -> PathBuf {
    root.join("removal").join("ledger.v1")
}

fn len(root: &Path) -> u64 {
    std::fs::metadata(ledger_path(root)).unwrap().len()
}

/// Waits, up to 10 s, for `ready` to hold of the ledger, and returns it.
fn wait_for(root: &Path, what: &str, ready: impl Fn(&Ledger) -> bool) -> Ledger {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let ledger = read(root);
        if ready(&ledger) {
            return ledger;
        }
        assert!(
            Instant::now() < until,
            "waited for {what}; the ledger has {:?}, gap {:?}",
            ledger.links,
            ledger.gap
        );
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// Failure: the ledger records the path as it was reached, so a file reached through a link is named by a path `backup_of` doesn't hash and its backup can't be
/// placed; or, with no stamp check, a link retargeted at another file records that file as the version that was seen.
#[test]
fn a_path_through_a_link_records_what_the_backup_is_named_by() {
    let dir = temp();
    let root = dir.join("state");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("a.pdf"), pdf("a")).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let seen = stamp(&real.join("a.pdf"));
    note_save(
        &root.join("backups.noindex"),
        &SaveNote::Starting {
            path: &link.join("a.pdf"),
            from: Some(seen),
        },
    );
    let ledger = read(&root);
    let [Link::At { path, version }] = ledger.links.as_slice() else {
        panic!("one At, not {:?}", ledger.links)
    };
    assert_eq!(path, &real.join("a.pdf"), "the canonical path, which is what keep_backup hashes");
    assert_eq!((version.inode, version.size), (seen.inode, seen.size));
    assert!(recto_core::save::backup_name(path).starts_with("original-"));

    // The link now leads to another folder holding another file: the stamp seen is not what is there.
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("a.pdf"), pdf("other, longer")).unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    assert!(
        canonical_version(&link.join("a.pdf"), seen).is_none(),
        "nothing is recorded for a version that isn't there"
    );
    note_save(
        &root.join("backups.noindex"),
        &SaveNote::Starting {
            path: &link.join("a.pdf"),
            from: Some(seen),
        },
    );
    let ledger = read(&root);
    assert!(
        matches!(ledger.links.last(), Some(Link::Unplaced { path }) if path == &elsewhere.join("a.pdf")),
        "a file was there that can't be proven: {:?}",
        ledger.links
    );
    assert_eq!(ledger.links.len(), 2);
}

/// Failure: a save that starts on a file whose stamp isn't known records nothing, though `keep_backup` is about to make a backup named for the path: that backup
/// then reads as nobody's, instead of as one whose path was seen but not proven.
#[test]
fn a_save_start_that_cant_be_verified_records_the_path_unplaced() {
    let dir = temp();
    let root = dir.join("state");
    std::fs::write(dir.join("a.pdf"), pdf("a")).unwrap();
    note_save(
        &root.join("backups.noindex"),
        &SaveNote::Starting {
            path: &dir.join("a.pdf"),
            from: None,
        },
    );
    assert_eq!(read(&root).links, vec![Link::Unplaced { path: dir.join("a.pdf") }]);
}

/// Failure: the same tab noted again and again (every record write, every save start) grows the ledger by a link each time until it overflows.
#[test]
fn the_same_note_twice_is_one_link() {
    let dir = temp();
    let root = dir.join("state");
    std::fs::write(dir.join("a.pdf"), pdf("a")).unwrap();
    let note = SaveNote::Starting {
        path: &dir.join("a.pdf"),
        from: Some(stamp(&dir.join("a.pdf"))),
    };
    note_save(&root.join("backups.noindex"), &note);
    let once = len(&root);
    note_save(&root.join("backups.noindex"), &note);
    assert_eq!(len(&root), once, "nothing was appended");
    assert_eq!(read(&root).links.len(), 1);
}

/// Failure: a ledger that has reached its cap goes on growing (a file of any size is read), or stops without saying so (the lineage then reads as complete
/// with links missing). At the limit: no gap; one over: exactly one `Overflowed`, then the file's length never changes.
#[test]
fn overflow_writes_one_gap_and_then_nothing() {
    let dir = temp();
    let (root, config) = (dir.join("state"), small(6));
    // Six frames in all, one kept for the gap: five links fit.
    for n in 1..=5 {
        append(&root, &config, vec![at(n)]);
    }
    let full = read_with(&root, &config.limits);
    assert_eq!((full.links.len(), full.gap), (5, None), "at the limit");
    append(&root, &config, vec![at(6)]);
    let over = read_with(&root, &config.limits);
    assert_eq!((over.links.len(), over.gap, over.overflowed, over.frames), (5, Some(Gap::Overflowed), true, 6));
    let sealed = len(&root);
    for n in 7..12 {
        append(&root, &config, vec![at(n), Entry::Gap(Gap::BackupMismatch)]);
    }
    assert_eq!(len(&root), sealed, "after the gap nothing is appended");
}

/// Failure: a path total past its cap is written whole (a ledger of long paths grows past what a read will take), or its overflow is written again by every
/// later note.
#[test]
fn the_paths_cap_overflows_too() {
    let dir = temp();
    let root = dir.join("state");
    let config = Config {
        limits: LedgerLimits {
            max_total_path_bytes: 30,
            ..LEDGER_LIMITS
        },
        ..Config::default()
    };
    append(&root, &config, vec![at(1), at(2), at(3)]); // 11 bytes each: two fit in 30
    let ledger = read_with(&root, &config.limits);
    assert_eq!((ledger.links.len(), ledger.gap), (2, Some(Gap::Overflowed)));
    let sealed = len(&root);
    append(
        &root,
        &config,
        vec![Entry::Link(Link::Saved {
            from: version(1),
            to: version(2),
        })],
    );
    assert_eq!(len(&root), sealed, "a link with no path is not let in after the overflow either");
}

/// Failure: a write cut short by a crash is read as damage for ever (the removal then refuses for good), or it is repaired without a trace (the lineage
/// reads as complete), or the repair happens on every later write. A torn tail is cut back once, with a `Torn` gap; a damaged or newer file is never written.
#[test]
fn a_torn_tail_is_repaired_once_and_a_damaged_file_never_written() {
    let dir = temp();
    let root = dir.join("state");
    let config = Config::default();
    append(&root, &config, vec![at(1), at(2)]);
    let whole = len(&root);
    let frame = encode(&at(3)).unwrap();
    let mut file = std::fs::OpenOptions::new().append(true).open(ledger_path(&root)).unwrap();
    file.write_all(&frame[..frame.len() - 3]).unwrap();
    assert_eq!(read(&root).stopped, Some(Gap::Torn));
    append(&root, &config, vec![at(4)]);
    let repaired = read(&root);
    assert_eq!(
        (repaired.links.len(), repaired.stopped, repaired.gap),
        (3, None, Some(Gap::Torn)),
        "{:?}",
        repaired.links
    );
    let after = len(&root);
    assert!(after > whole);
    append(&root, &config, vec![at(5)]);
    assert_eq!(len(&root), after + encode(&at(5)).unwrap().len() as u64, "one frame, and no second Torn");

    let before = std::fs::read(ledger_path(&root)).unwrap();
    let mut bad = before.clone();
    bad[40] ^= 0xff; // a byte inside the first frame
    std::fs::write(ledger_path(&root), &bad).unwrap();
    append(&root, &config, vec![at(6)]);
    assert_eq!(std::fs::read(ledger_path(&root)).unwrap(), bad, "damage is left as it is");
    let mut newer = before;
    newer[12] = 2;
    std::fs::write(ledger_path(&root), &newer).unwrap();
    append(&root, &config, vec![at(7)]);
    assert_eq!(std::fs::read(ledger_path(&root)).unwrap(), newer, "a newer ledger is never written to");
}

/// Failure: a header cut short (the first write died) is read as damage for ever; it is a ledger that never began, and begins again with a new time.
#[test]
fn a_torn_header_begins_again() {
    let dir = temp();
    let root = dir.join("state");
    local::ensure_private_dir(&root.join("removal")).unwrap();
    std::fs::write(ledger_path(&root), &encode_header(5)[..20]).unwrap();
    append(&root, &Config::default(), vec![at(1)]);
    let ledger = read(&root);
    assert_eq!((ledger.links.len(), ledger.stopped, ledger.gap), (1, None, None));
    assert!(ledger.began_ns > 1_767_225_600_000_000_000, "began now, not at 5");
}

/// Failure: a link, a directory or a pipe at the ledger's name is written through or opened for good: a link is followed to wherever it points, and a pipe
/// waits for a reader. Both read as damaged, and the file they point at is unchanged.
#[test]
fn a_link_at_the_ledgers_name_is_never_written_through() {
    let dir = temp();
    let root = dir.join("state");
    local::ensure_private_dir(&root.join("removal")).unwrap();
    let precious = dir.join("precious");
    std::fs::write(&precious, b"keep").unwrap();
    std::os::unix::fs::symlink(&precious, ledger_path(&root)).unwrap();
    append(&root, &Config::default(), vec![at(1)]);
    assert_eq!(std::fs::read(&precious).unwrap(), b"keep");
    assert_eq!(read(&root).gap, Some(Gap::Damaged));
}

/// Runs `work` on a thread and fails, naming `what`, if it hasn't returned in 10 s (a thread blocked for good is left behind, which only a failing run does).
fn returns_in_time(what: &str, work: impl FnOnce() + Send + 'static) {
    let (done, returned) = mpsc::channel();
    std::thread::spawn(move || {
        work();
        let _ = done.send(());
    });
    returned.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("{what} didn't return"));
}

/// A write that is lost: the lock is held by someone else for longer than the writer waits.
fn lose_a_write(root: &Path) {
    let holder = std::fs::OpenOptions::new().append(true).open(ledger_path(root)).unwrap();
    holder.lock().unwrap();
    let config = Config {
        lock_wait: Duration::from_millis(20),
        ..Config::default()
    };
    append(root, &config, vec![at(99)]);
}

/// Failure: the gap mark is opened for writing with a plain open, so a pipe planted at `ledger.gap` waits for a reader for ever and a save that lost a
/// write hangs on it.
#[test]
fn a_pipe_at_the_gap_mark_doesnt_hold_a_save_up() {
    let dir = temp();
    let root = dir.join("state");
    append(&root, &Config::default(), vec![at(1)]);
    let mark = root.join("removal").join(GAP_MARK);
    assert!(std::process::Command::new("mkfifo").arg(&mark).status().unwrap().success());
    let root = root.clone();
    returns_in_time("a save that lost a write, with a pipe at the gap mark", move || lose_a_write(&root));
}

/// Failure: when `removal/` is a link, the gap mark is made through it, in whatever folder it points at (a file made in a place the app never chose).
#[test]
fn a_linked_removal_folder_gets_no_gap_mark_through_it() {
    let dir = temp();
    let root = dir.join("state");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    local::ensure_private_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("removal")).unwrap();
    append(&root, &Config::default(), vec![at(1)]);
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing was made in the linked folder");
}

/// Failure: the reader checks only the last part of the path, so a `removal/` that is a link reads a ledger that is somewhere else (someone else's) as this one.
#[test]
fn a_linked_removal_folder_reads_as_damaged() {
    let dir = temp();
    let (theirs, mine) = (dir.join("theirs"), dir.join("mine"));
    append(&theirs, &Config::default(), vec![at(1)]);
    std::fs::create_dir_all(&mine).unwrap();
    std::os::unix::fs::symlink(theirs.join("removal"), mine.join("removal")).unwrap();
    assert_eq!(read(&theirs).links.len(), 1);
    assert_eq!(read(&mine).gap, Some(Gap::Damaged));
}

/// Failure: the ledger is opened with a plain `File::open` after a check by name, so what is swapped in after the check is read, and a file others can write
/// is trusted. A ledger that others can write is refused: it reads as damaged and is not appended to.
#[test]
fn a_ledger_others_can_write_is_neither_read_nor_appended_to() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp();
    let root = dir.join("state");
    append(&root, &Config::default(), vec![at(1)]);
    let before = len(&root);
    std::fs::set_permissions(ledger_path(&root), std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(read(&root).gap, Some(Gap::Damaged));
    append(&root, &Config::default(), vec![at(2)]);
    assert_eq!(len(&root), before, "nothing was appended");
}

/// Failure: any error opening an existing ledger is taken as "a ledger this code doesn't write to", which leaves no mark, so a transient one (no access, no
/// descriptors left) loses entries and the ledger still reads as whole. Only a link, a pipe or a folder at the name is left unmarked.
#[test]
fn an_open_that_fails_for_another_reason_leaves_a_gap_mark() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp();
    let root = dir.join("state");
    append(&root, &Config::default(), vec![at(1)]);
    std::fs::set_permissions(ledger_path(&root), std::fs::Permissions::from_mode(0o400)).unwrap();
    append(&root, &Config::default(), vec![at(2)]);
    assert_eq!(read(&root).gap, Some(Gap::WriteFailed));
}

/// Failure: two writers that read the same state both append the same link (a duplicate), or the second's header lands after the first's frames (damage). A
/// writer held between its read and its append, and another started then, must leave one link and a clean read.
#[test]
fn two_writers_leave_no_duplicate_and_no_damage() {
    let dir = temp();
    let root = dir.join("state");
    let (at_seam, seam_reached) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    let first = Config {
        before_write: Some(Arc::new({
            let (at_seam, released) = (Mutex::new(at_seam), Arc::clone(&released));
            move || {
                let _ = at_seam.lock().unwrap().send(());
                let _ = released.lock().unwrap().recv_timeout(Duration::from_secs(20));
            }
        })),
        ..Config::default()
    };
    let second = Config {
        lock_wait: Duration::from_secs(20),
        ..Config::default()
    };
    let a = std::thread::spawn({
        let root = root.clone();
        move || append(&root, &first, vec![at(1)])
    });
    seam_reached.recv_timeout(Duration::from_secs(10)).expect("the first writer reached its seam");
    let b = std::thread::spawn({
        let root = root.clone();
        move || append(&root, &second, vec![at(1)])
    });
    std::thread::sleep(Duration::from_millis(100)); // ratchet: poll (the second tries the lock; it is not yet taken)
    release.send(()).unwrap();
    a.join().unwrap();
    b.join().unwrap();
    let ledger = read(&root);
    assert_eq!((ledger.links.len(), ledger.stopped, ledger.gap), (1, None, None), "{:?}", ledger.links);
}

/// Failure: a writer that can't get the lock in time holds the save up, or fails it, or says nothing (the removal then reads a ledger with a link missing as
/// whole). It gives up at the bound and leaves `ledger.gap`, and the ledger reads `WriteFailed` from then on.
#[test]
fn a_lock_that_doesnt_come_leaves_a_gap_mark_and_returns_in_time() {
    let dir = temp();
    let root = dir.join("state");
    append(&root, &Config::default(), vec![at(1)]);
    let holder = std::fs::OpenOptions::new().append(true).open(ledger_path(&root)).unwrap();
    holder.lock().unwrap();
    let config = Config {
        lock_wait: Duration::from_millis(60),
        ..Config::default()
    };
    let began = Instant::now();
    append(&root, &config, vec![at(2)]);
    assert!(began.elapsed() < Duration::from_secs(5), "bounded by the wait, not by the holder");
    assert_eq!(read(&root).gap, Some(Gap::WriteFailed));
    assert_eq!(read(&root).links.len(), 1, "nothing was written without the lock");
}

/// A config whose adoption time can't run out on a starved machine: the cut by time has a test of its own.
fn patient() -> Config {
    Config {
        adoption_time: Duration::from_secs(600),
        ..Config::default()
    }
}

/// Failure: adoption has no time bound (a slow disk or a folder of a thousand journals holds the first save's thread for as long as it takes), or it runs out
/// of time and says nothing (the removal then reads a ledger that is missing links as the whole story). With no time, nothing is read and it says so.
#[test]
fn adoption_out_of_time_says_so() {
    let dir = temp();
    let root = dir.join("state");
    let file = dir.join("a.pdf");
    std::fs::write(&file, pdf("a")).unwrap();
    journal(&root, "session", 1, &file, Some(stamp(&file)), None);
    append(
        &root,
        &Config {
            adoption_time: Duration::ZERO,
            ..Config::default()
        },
        Vec::new(),
    );
    let ledger = read(&root);
    assert_eq!((ledger.links.len(), ledger.gap), (0, Some(Gap::AdoptionCut)));
}

/// Writes a journal whose header says `path` was `file` and a save was about to make it `saving`.
fn journal(root: &Path, session: &str, n: u32, path: &Path, file: Option<FileStamp>, saving: Option<FileStamp>) {
    let folder = root.join(session).join("journals");
    std::fs::create_dir_all(&folder).unwrap();
    let mut header = recto_session::record::JournalHeader::new(1, path, file, 1, 0);
    header.saving = saving.map(Into::into);
    std::fs::write(
        folder.join(format!("journal-{n:016x}.log")),
        recto_session::record::journal_start(&header, None),
    )
    .unwrap();
}

/// Failure: the first ledger knows nothing of a file already open or saved before it, so every backup of an older save reads as nobody's. Adoption takes a
/// header's file if it is still there, a save that landed as `At` and `Saved`, and nothing from a file that has moved on, a relative path or a header of
/// no plain file.
#[test]
fn adoption_takes_what_the_journals_prove_and_nothing_else() {
    let dir = temp();
    let root = dir.join("state");
    let still = dir.join("still.pdf");
    std::fs::write(&still, pdf("still")).unwrap();
    let landed = dir.join("landed.pdf");
    std::fs::write(&landed, pdf("before")).unwrap();
    let before = stamp(&landed);
    std::fs::write(dir.join("next.pdf"), pdf("after, longer")).unwrap();
    std::fs::rename(dir.join("next.pdf"), &landed).unwrap();
    let after = stamp(&landed);
    let moved = dir.join("moved.pdf");
    std::fs::write(&moved, pdf("moved")).unwrap();
    let was = stamp(&moved);
    std::fs::write(&moved, pdf("changed since, longer")).unwrap();
    journal(&root, "session", 1, &still, Some(stamp(&still)), None);
    journal(&root, "session-windowless", 2, &landed, Some(before), Some(after));
    journal(&root, "session", 3, &moved, Some(was), None);
    journal(&root, "session", 4, Path::new("relative/x.pdf"), Some(was), None);
    // A journal that is a link to a file elsewhere is not read.
    std::os::unix::fs::symlink(&still, root.join("session/journals/journal-0000000000000005.log")).unwrap();
    append(&root, &patient(), Vec::new());
    note_save(&root.join("backups.noindex"), &SaveNote::Starting { path: &still, from: None });
    let ledger = read(&root);
    let paths: Vec<&Path> = ledger
        .links
        .iter()
        .filter_map(|l| match l {
            Link::At { path, .. } => Some(path.as_path()),
            _ => None,
        })
        .collect();
    assert_eq!(paths, vec![still.as_path(), landed.as_path()], "{:?}", ledger.links);
    assert!(
        ledger
            .links
            .iter()
            .any(|l| matches!(l, Link::Saved { from, to } if from.size == before.size && to.size == after.size))
    );
    assert_eq!(ledger.gap, None);
}

/// Failure: adoption reads every journal there is (a folder of a million is a stall at the first save), or stops at its cap without saying so. At the cap all
/// are read; one over, the ledger says `AdoptionCut`.
#[test]
fn adoption_stops_at_its_cap_and_says_so() {
    for (journals, cut) in [(3u32, false), (4, true)] {
        let dir = temp();
        let root = dir.join("state");
        for n in 0..journals {
            let file = dir.join(format!("f{n}.pdf"));
            std::fs::write(&file, pdf(&format!("{n}"))).unwrap();
            journal(&root, "session", n + 1, &file, Some(stamp(&file)), None);
        }
        append(
            &root,
            &Config {
                adoption_headers: 3,
                ..Config::default()
            },
            Vec::new(),
        );
        let ledger = read(&root);
        assert_eq!(ledger.gap, cut.then_some(Gap::AdoptionCut), "{journals} journals");
        assert_eq!(ledger.links.len(), 3, "the three it read are in");
    }
}

fn tab(path: &Path) -> recto_session::record::TabEntry {
    let mut entry: recto_session::record::TabEntry = serde_json::from_value(serde_json::json!({
        "id": 1, "path": path.to_str().unwrap(), "page": 1, "page_count": 1
    }))
    .unwrap();
    entry.file = Some(stamp(path).into());
    entry
}

/// Failure: a record write waits for the ledger (a dead volume under the state folder stalls every record write), or starts a thread for tabs it has already
/// noted. With the ledger thread held, the call returns; a second call with something new starts no second thread; with nothing new none starts at all.
#[test]
fn a_record_write_never_waits_for_the_ledger() {
    let dir = temp();
    let root = dir.join("state");
    std::fs::write(dir.join("a.pdf"), pdf("a")).unwrap();
    std::fs::write(dir.join("b.pdf"), pdf("b, longer")).unwrap();
    let (reached, at_seam) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let (reached, released) = (Mutex::new(reached), Mutex::new(released));
    let config = Config {
        in_thread: Some(Arc::new(move || {
            let _ = reached.lock().unwrap().send(());
            let _ = released.lock().unwrap().recv_timeout(Duration::from_secs(20));
        })),
        ..Config::default()
    };
    let (done, returned) = mpsc::channel();
    let caller = std::thread::spawn({
        let (root, a, b, config) = (root.clone(), tab(&dir.join("a.pdf")), tab(&dir.join("b.pdf")), config.clone());
        move || {
            note_tabs_with(&root, std::slice::from_ref(&a), &config);
            done.send(()).unwrap();
            // The thread is held: this one has something new and starts nothing.
            note_tabs_with(&root, &[a.clone(), b], &config);
            done.send(()).unwrap();
        }
    });
    returned
        .recv_timeout(Duration::from_secs(10))
        .expect("the call returned while the ledger's thread was held");
    at_seam.recv_timeout(Duration::from_secs(10)).expect("the thread started");
    returned
        .recv_timeout(Duration::from_secs(10))
        .expect("a call with the thread running returned too");
    caller.join().unwrap();
    assert!(running(&root));
    assert!(std::fs::metadata(ledger_path(&root)).is_err(), "nothing was written while it was held");
    release.send(()).unwrap();
    wait_for(&root, "the first tab", |l| l.links.len() == 1);
    // Nothing new: no thread, so nothing running at once after the call.
    note_tabs_with(&root, &[tab(&dir.join("a.pdf"))], &Config::default());
    assert!(!running(&root), "a tab already noted starts no thread");
    // The tab the held thread skipped is noted by the next call.
    note_tabs_with(&root, &[tab(&dir.join("a.pdf")), tab(&dir.join("b.pdf"))], &Config::default());
    wait_for(&root, "the second tab", |l| l.links.len() == 2);
}

/// Failure: the record of already-seen pairs grows with every tab ever opened (it is per-tab state held for ever).
#[test]
fn the_seen_set_is_capped() {
    let dir = temp();
    let root = dir.join("state");
    std::fs::write(dir.join("a.pdf"), pdf("a")).unwrap();
    let base = tab(&dir.join("a.pdf"));
    for round in 0..70u64 {
        let tabs: Vec<_> = (0..64u64)
            .map(|n| {
                let mut t = base.clone();
                t.file.as_mut().unwrap().inode = 1 + round * 64 + n;
                t
            })
            .collect();
        assert_eq!(take_new(&root, &tabs).len(), 64, "sixty-four new pairs, all at once");
        finish(&root);
        assert!(held(&root).1 <= 4_096, "round {round}: {}", held(&root).1);
    }
    assert!(held(&root).1 < 4_480, "cleared when full");
}

fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

/// The plan for removing a backup under `folder` in `mode`, with the ledger of `root` and `current` as the file now.
fn plan_for(root: &Path, folder: &Path, current: FileVersion, at: &Path, mode: TargetMode) -> Result<(usize, Disposition), Refusal> {
    let ledger = read(root);
    let lineage = lineage(&ledger, current, at, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(16));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::Backup) {
        survey.mark_complete(store);
    }
    crate::removal_store::survey_backups(folder, &lineage, mode, &mut survey)?;
    let surveyed = survey.finish()?;
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false);
    let plan = plan(&request, &surveyed.inventory(), &DESIGN_LIMITS)?;
    let line = plan.line(Store::Backup);
    Ok((line.acted as usize, line.disposition))
}

struct Served {
    dir: TempDir,
    paths: StatePaths,
    host: SessionHost<DrawableLoader>,
}

fn served() -> Served {
    let dir = temp();
    let paths = StatePaths::overridden(&dir.join("state")).unwrap();
    paths.prepare().unwrap();
    let host = SessionHost::serving(&paths);
    Served { dir, paths, host }
}

impl Served {
    fn run(&self, command: impl Into<Command>) -> serde_json::Value {
        self.host
            .run(Actor::Agent(Level::Full), &command.into(), &mut Vec::new())
            .unwrap_or_else(|e| panic!("{}", e.message))
    }

    fn open(&self, path: &Path) {
        self.run(recto_agent::commands::tabs::Open {
            path: path.to_path_buf(),
            page: None,
        });
    }

    fn rotate_and_save(&self, out: Option<&Path>) {
        self.run(recto_agent::commands::editing::PageRotate {
            tab: TabRef::default(),
            pages: "1".into(),
            degrees: 90,
        });
        self.run(recto_agent::commands::saving::Save {
            tab: TabRef::default(),
            out: out.map(Path::to_path_buf),
            replace: false,
        });
    }

    fn backups(&self) -> PathBuf {
        crate::host::backups_folder(&self.paths)
    }
}

/// Failure: a real Save leaves a backup the ledger can't place, so a removal refuses for every file saved by this app; or it places it on the wrong file. A
/// Save, then the lineage and the survey from the ledger the save wrote: the real backup is the target's, and is purged by the plan.
#[test]
fn a_real_save_leaves_a_ledger_that_places_its_backup() {
    let s = served();
    let file = s.dir.join("doc.pdf");
    std::fs::write(&file, pdf("UNIQUEMARKER-7c1d")).unwrap();
    s.open(&file);
    s.rotate_and_save(None);
    let ledger = read(&s.paths.root);
    assert_eq!(ledger.gap, None, "{:?}", ledger.links);
    assert!(
        ledger.links.iter().any(|l| matches!(l, Link::Saved { .. })),
        "the save over itself: {:?}",
        ledger.links
    );
    let now = stamp(&file);
    let current = FileVersion::new(now, recto_platform::volume::volume_identity(&file));
    assert!(current.volume.is_some(), "the temp folder is on a volume with a real UUID (APFS or HFS+)");
    let outcome = plan_for(&s.paths.root, &s.backups(), current, &file, TargetMode::Save).unwrap();
    assert_eq!(outcome, (1, Disposition::Purged), "the real backup is the target's");
    // Neither the marker in the page text nor the one in the title is in the ledger.
    let bytes = std::fs::read(ledger_path(&s.paths.root)).unwrap();
    assert!(!bytes.windows(10).any(|w| w == b"UNIQUEMARK"), "the ledger holds no document text");
}

/// Failure: a file on a volume with no identity joins its history by path alone: the backup is placed on a guess. With no volume for the current file, the
/// backup of its own path reads Unknown and the plan refuses.
#[test]
fn a_current_file_with_no_volume_has_its_backup_read_unknown() {
    let s = served();
    let file = s.dir.join("doc.pdf");
    std::fs::write(&file, pdf("a")).unwrap();
    s.open(&file);
    s.rotate_and_save(None);
    let current = FileVersion::new(stamp(&file), None);
    assert_eq!(
        plan_for(&s.paths.root, &s.backups(), current, &file, TargetMode::Save).unwrap_err(),
        Refusal::Incomplete(Store::Backup),
        "a version with no volume joins nothing, and only the path is left, seen on both sides"
    );
}

/// Failure: a Save As records a `Saved` from the source to the copy, so a later removal on the copy deletes the source's backup. After a Save As the source's
/// backup is not the copy's.
#[test]
fn after_a_save_as_the_sources_backup_is_unrelated() {
    let s = served();
    let source = s.dir.join("source.pdf");
    std::fs::write(&source, pdf("a")).unwrap();
    s.open(&source);
    s.rotate_and_save(None);
    let copy = s.dir.join("copy.pdf");
    s.rotate_and_save(Some(&copy));
    let ledger = read(&s.paths.root);
    assert_eq!(ledger.gap, None, "{:?}", ledger.links);
    let now = FileVersion::new(stamp(&copy), recto_platform::volume::volume_identity(&copy));
    let outcome = plan_for(&s.paths.root, &s.backups(), now, &copy, TargetMode::Save).unwrap();
    assert_eq!(outcome.0, 0, "the source's backup is not the copy's to purge: {outcome:?}");
}

/// Failure: a save whose backup isn't under the name its path hashes to (so the survey would never find it) is recorded as nothing, and the removal then
/// reads "no backup" for a copy of the document that is there. It leaves `BackupMismatch`, and the lineage is not complete.
#[test]
fn a_backup_under_another_name_is_a_gap() {
    let dir = temp();
    let root = dir.join("state");
    std::fs::write(dir.join("a.pdf"), pdf("a")).unwrap();
    let right = dir.join(recto_core::save::backup_name(&dir.join("a.pdf")));
    let (file, wrong) = (dir.join("a.pdf"), dir.join("original-0000.pdf"));
    let to = Some(stamp(&file));
    let landed = |backup| SaveNote::Landed {
        path: &file,
        to,
        original: &file,
        backup: Some(backup),
    };
    note_save(&root.join("backups.noindex"), &landed(&right));
    assert_eq!(read(&root).gap, None, "the name the path hashes to is not a mismatch");
    note_save(&root.join("backups.noindex"), &landed(&wrong));
    assert_eq!(read(&root).gap, Some(Gap::BackupMismatch));
}
