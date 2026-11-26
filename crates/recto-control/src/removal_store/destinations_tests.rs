//! A Save As destination is recorded before the file is there (`ledger.rs`, `destination_entry`). It is no lineage link: what it must and must not do to
//! the surveys of the stores is held here, over a real ledger written by the save's own notes.

use super::ledger::{note_save, read, running};
use super::testing::{limits, put};
use super::{survey_backups, survey_journals, survey_writer_temps};
use recto_agent::saving::SaveNote;
use recto_core::removal::{DESIGN_LIMITS, Effect, FileVersion, Kinds, LEDGER_LIMITS, Plan, Request, Store, Survey, TargetMode, lineage, plan};
use recto_core::save::{backup_name, writer_temp_prefix};
use recto_platform::FileStamp;
use recto_session::record::{JournalHeader, journal_file_name, journal_start};
use recto_testpdf::TempDir;
use std::path::Path;
use std::time::{Duration, Instant};

fn stamp(path: &Path) -> FileStamp {
    recto_platform::file_stamp(path).unwrap()
}

fn folder() -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-destinations-")
}

fn notes(root: &Path, note: &SaveNote<'_>) {
    note_save(&root.join("backups.noindex"), note);
}

/// The first note begins the ledger, and the `recto-ledger` thread adopts the older journals after it: wait for that, so what a test writes next is not
/// read by it.
fn settled(root: &Path) {
    let until = Instant::now() + Duration::from_secs(10);
    while running(root) {
        assert!(Instant::now() < until, "the ledger's thread didn't end");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// A journal and an original backup for the file at `path`, as a session that edited it and saved over it leaves them.
fn leave_stores(root: &Path, path: &Path) {
    let at = stamp(path);
    let journals = root.join("session").join("journals");
    std::fs::create_dir_all(&journals).unwrap();
    let header = JournalHeader::new(1, path, Some(at), 3, 0);
    put(&journals, &journal_file_name(Some(at), path, None), &journal_start(&header, None));
    let backups = root.join("backups.noindex");
    std::fs::create_dir_all(&backups).unwrap();
    put(&backups, &backup_name(path), b"%PDF-1.4");
}

/// What a removal of the document at `path` would plan, over every store this test fills (the others are marked complete).
fn planned(root: &Path, path: &Path, mode: TargetMode) -> Result<Plan, recto_core::removal::Refusal> {
    let ledger = read(root);
    let version = FileVersion::new(stamp(path), recto_platform::volume::volume_identity(path));
    let lineage = lineage(&ledger, version, path, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(64));
    let ours = [
        Store::Journal,
        Store::JournalVariant,
        Store::PageIds,
        Store::SleepFile,
        Store::KeptPages,
        Store::Backup,
        Store::WriterTemp,
    ];
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && !ours.contains(s)) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    survey_journals(root, &lineage, mode, &mut survey)?;
    survey_backups(&root.join("backups.noindex"), &lineage, mode, &mut survey)?;
    survey_writer_temps(&lineage, &mut survey)?;
    let surveyed = survey.finish()?;
    plan(
        &Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false),
        &surveyed.inventory(),
        &DESIGN_LIMITS,
    )
}

fn purged(p: &Plan, store: Store) -> usize {
    p.steps()
        .iter()
        .filter(|s| s.effect() == Effect::Purge(store))
        .map(|s| p.artifacts(s).len())
        .sum()
}

/// Failure (review round 2): the destination was recorded as an `Unplaced` link, which the lineage reads on both sides for good, so after any Save As the
/// copy's own journal and backup (placed by version and path) read `Unknown` and every removal, of the copy and of an unrelated file, was refused on
/// `Store::Journal` and `Store::Backup`.
#[test]
fn after_a_save_as_the_copy_and_an_unrelated_file_still_plan() {
    let dir = folder();
    let root = dir.join("state");
    let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
    let other = put(&dir, "other.pdf", b"%PDF-1.4 other, longer");
    let at = |path: &Path| Some(stamp(path));
    notes(
        &root,
        &SaveNote::Starting {
            path: &other,
            from: at(&other),
            destination: None,
        },
    );
    settled(&root);
    let copy = dir.join("copy.pdf");
    notes(
        &root,
        &SaveNote::Starting {
            path: &mine,
            from: at(&mine),
            destination: Some(&copy),
        },
    );
    // The writer lands the copy, which is then edited: it has a journal and a backup of its own, and so has the unrelated file.
    put(&dir, "copy.pdf", b"%PDF-1.4 the copy of mine, longer still");
    notes(
        &root,
        &SaveNote::Landed {
            path: &copy,
            to: at(&copy),
            original: &mine,
            backup: None,
        },
    );
    leave_stores(&root, &copy);
    leave_stores(&root, &other);
    for (path, name) in [(&copy, "the copy"), (&other, "an unrelated file")] {
        let planned = planned(&root, path, TargetMode::Save).unwrap_or_else(|refusal| panic!("{name}: {refusal:?}"));
        assert_eq!(
            (purged(&planned, Store::Journal), purged(&planned, Store::Backup)),
            (1, 1),
            "{name}: its own journal and backup, and nothing else's"
        );
    }
}

/// The folder of every Save As destination is listed, landed or not, and a temp of exact shape in it refuses. Failure: a landed destination's folder drops out
/// of the survey, so a temp left in it by an earlier crashed save reads "none found".
#[test]
fn a_landed_destinations_folder_is_still_listed() {
    let dir = folder();
    let root = dir.join("state");
    let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let landed = elsewhere.join("copy.pdf");
    notes(
        &root,
        &SaveNote::Starting {
            path: &mine,
            from: Some(stamp(&mine)),
            destination: Some(&landed),
        },
    );
    settled(&root);
    put(&elsewhere, "copy.pdf", b"%PDF-1.4 the copy, longer");
    notes(
        &root,
        &SaveNote::Landed {
            path: &landed,
            to: Some(stamp(&landed)),
            original: &mine,
            backup: None,
        },
    );
    assert!(planned(&root, &mine, TargetMode::Save).is_ok(), "nothing is left yet");
    let temp = format!("{}9-1.tmp", writer_temp_prefix(Path::new("copy.pdf")));
    put(&elsewhere, &temp, b"left by something else");
    assert_eq!(
        planned(&root, &mine, TargetMode::Save).unwrap_err(),
        recto_core::removal::Refusal::Incomplete(Store::WriterTemp)
    );
}

/// What `recto local` and `recto session` do for a Save As: one command at Full in a throwaway session on a test state folder, through `run_here`.
fn save_as(paths: &crate::state::StatePaths, from: &Path, out: &Path) {
    use recto_agent::{Command, TabRef, commands};
    let mut session = recto_session::Session::new(recto_agent::DrawableLoader);
    let mut log = crate::log::ActivityLog::new(paths);
    let open = Command::from(commands::tabs::Open {
        path: from.to_path_buf(),
        page: None,
    });
    crate::local::run_here(&mut session, paths, &mut log, &open).unwrap();
    let save = Command::from(commands::saving::Save {
        tab: TabRef::Active,
        out: Some(out.to_path_buf()),
        replace: false,
    });
    crate::local::run_here(&mut session, paths, &mut log, &save).unwrap();
}

/// Failure: a Save As through `recto session` or `recto local` (which write the person's real files in a process of their own) is never noted, so a crash
/// leaves a temp that a later removal calls "none found"; or the route begins a ledger, which a process that ends at once leaves with no adoption of the
/// older journals for good. It notes into a ledger that has begun, and begins none.
#[test]
fn a_save_as_through_local_or_session_is_noted_in_a_ledger_that_has_begun_and_begins_none() {
    let dir = folder();
    let paths = crate::state::StatePaths::overridden(&dir.join("state")).unwrap();
    paths.prepare().unwrap();
    crate::levels::store(&paths, recto_agent::Level::Full).unwrap();
    let mine = put(
        &dir,
        "mine.pdf",
        &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["words"]))]),
    );
    save_as(&paths, &mine, &dir.join("first.pdf"));
    assert!(dir.join("first.pdf").exists(), "the save went through");
    assert!(!paths.root.join("removal").exists(), "no ledger is begun by a process that ends at once");
    // A ledger the app began: the next Save As is in it, as a destination, and then as the file that landed.
    notes(
        &paths.root,
        &SaveNote::Starting {
            path: &mine,
            from: Some(stamp(&mine)),
            destination: None,
        },
    );
    settled(&paths.root);
    let second = dir.join("second.pdf");
    save_as(&paths, &mine, &second);
    let ledger = read(&paths.root);
    // Noted at the writer's first step and landed since: the reader keeps it all the same, and the file holds it as a frame and a link.
    assert!(ledger.destinations.contains(&second), "kept: {:?}", ledger.destinations);
    let bytes = std::fs::read(paths.root.join("removal").join("ledger.v1")).unwrap();
    let name = second.as_os_str().as_encoded_bytes();
    assert_eq!(bytes.windows(name.len()).filter(|w| *w == name).count(), 2, "a destination frame and the link");
    assert!(
        ledger
            .links
            .iter()
            .any(|link| matches!(link, recto_core::removal::Link::At { path, .. } if *path == second)),
        "the copy as it landed: {:?}",
        ledger.links
    );
    assert_eq!(ledger.gap, None);
}

/// Starts a Save As of `mine` to `destination` as the writer's first step does, lets the ledger settle, and leaves what a crash leaves: the writer's temp
/// for that name in the destination's folder.
fn crash_save_as(root: &Path, mine: &Path, destination: &Path) {
    notes(
        root,
        &SaveNote::Starting {
            path: mine,
            from: Some(stamp(mine)),
            destination: Some(destination),
        },
    );
    settled(root);
    let (folder, name) = (destination.parent().unwrap(), destination.file_name().unwrap().to_str().unwrap());
    put(
        folder,
        &format!("{}9-1.tmp", writer_temp_prefix(Path::new(name))),
        b"the whole document and its edits",
    );
}

/// Failure (verification blocker): the filter read order-blind, so once a Save As to `copy.pdf` had landed, a second Save As to the same name was never a
/// destination again, and its crashed temp (the source's whole document) read `Unrelated` in the same folder or its folder was never listed in another:
/// removal of the source said none found. Order counts: the destination is recorded again after the file landed, and the removal refuses, for the copy in the
/// source's folder and in another.
#[test]
fn a_second_save_as_to_a_name_that_landed_and_crashed_is_found_in_either_folder() {
    for other_folder in [false, true] {
        let dir = folder();
        let root = dir.join("state");
        let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
        let home = if other_folder { dir.join("elsewhere") } else { dir.path().to_path_buf() };
        std::fs::create_dir_all(&home).unwrap();
        let copy = home.join("copy.pdf");
        notes(
            &root,
            &SaveNote::Starting {
                path: &mine,
                from: Some(stamp(&mine)),
                destination: Some(&copy),
            },
        );
        settled(&root);
        put(&home, "copy.pdf", b"%PDF-1.4 the copy, longer");
        notes(
            &root,
            &SaveNote::Landed {
                path: &copy,
                to: Some(stamp(&copy)),
                original: &mine,
                backup: None,
            },
        );
        assert!(planned(&root, &mine, TargetMode::Save).is_ok(), "nothing is left: {other_folder}");
        // The second Save As to the same name (which replaces the first copy) crashes in the writer, its temp left.
        std::fs::remove_file(&copy).unwrap();
        crash_save_as(&root, &mine, &copy);
        assert_eq!(
            planned(&root, &mine, TargetMode::Save).unwrap_err(),
            recto_core::removal::Refusal::Incomplete(Store::WriterTemp),
            "the second save's temp, copy in another folder: {other_folder}"
        );
    }
}

/// Failure: a Save As to a fresh name in another folder that crashes is not found (the folder is never listed), so removal says none found with the whole
/// document sitting there; through the real ledger of the save's own note.
#[test]
fn a_crashed_save_as_to_a_fresh_name_in_another_folder_refuses() {
    let dir = folder();
    let root = dir.join("state");
    let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    crash_save_as(&root, &mine, &elsewhere.join("fresh.pdf"));
    assert_eq!(
        planned(&root, &mine, TargetMode::Save).unwrap_err(),
        recto_core::removal::Refusal::Incomplete(Store::WriterTemp)
    );
}

/// Failure: a Save As the person's agreement or level refuses leaves a destination in the ledger, so every refused attempt costs a folder in every later
/// survey (64 of them is `TooLarge` for every removal). A refused save records nothing.
#[test]
fn a_refused_save_as_records_no_destination() {
    use recto_agent::{Command, TabRef, commands};
    let dir = folder();
    let paths = crate::state::StatePaths::overridden(&dir.join("state")).unwrap();
    paths.prepare().unwrap();
    crate::levels::store(&paths, recto_agent::Level::Full).unwrap();
    let mine = put(
        &dir,
        "mine.pdf",
        &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["words"]))]),
    );
    // A ledger that has begun, so a note would be recorded.
    notes(
        &paths.root,
        &SaveNote::Starting {
            path: &mine,
            from: Some(stamp(&mine)),
            destination: None,
        },
    );
    settled(&paths.root);
    let mut session = recto_session::Session::new(recto_agent::DrawableLoader);
    let mut log = crate::log::ActivityLog::new(&paths);
    crate::local::run_here(
        &mut session,
        &paths,
        &mut log,
        &Command::from(commands::tabs::Open {
            path: mine.clone(),
            page: None,
        }),
    )
    .unwrap();
    // The file changed since it was opened: the writer refuses it at its first check, before it makes anything.
    std::fs::write(&mine, b"%PDF-1.4 changed under the tab, and longer than before").unwrap();
    let out = dir.join("never.pdf");
    let save = Command::from(commands::saving::Save {
        tab: TabRef::Active,
        out: Some(out.clone()),
        replace: false,
    });
    assert!(crate::local::run_here(&mut session, &paths, &mut log, &save).is_err());
    assert!(!out.exists());
    assert!(read(&paths.root).destinations.is_empty(), "{:?}", read(&paths.root).destinations);
}

/// Failure (fifth hole): a crashed Save As of A to C leaves T1 (A's whole content, named for C); a retry lands C. The reader dropped the destination at the
/// landing, so T1 read `Unrelated` (same folder) or its folder wasn't listed (another), and A's removal said none found while lopdf would not sweep T1 for
/// an hour. A's removal refuses, for C in the same folder and in another.
#[test]
fn a_crashed_save_as_then_a_landing_of_the_retry_still_refuses() {
    for other_folder in [false, true] {
        let dir = folder();
        let root = dir.join("state");
        let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
        let home = if other_folder { dir.join("elsewhere") } else { dir.path().to_path_buf() };
        std::fs::create_dir_all(&home).unwrap();
        let copy = home.join("copy.pdf");
        crash_save_as(&root, &mine, &copy);
        // The retry: noted again, and landed.
        notes(
            &root,
            &SaveNote::Starting {
                path: &mine,
                from: Some(stamp(&mine)),
                destination: Some(&copy),
            },
        );
        put(&home, "copy.pdf", b"%PDF-1.4 the copy, longer");
        notes(
            &root,
            &SaveNote::Landed {
                path: &copy,
                to: Some(stamp(&copy)),
                original: &mine,
                backup: None,
            },
        );
        assert_eq!(
            planned(&root, &mine, TargetMode::Save).unwrap_err(),
            recto_core::removal::Refusal::Incomplete(Store::WriterTemp),
            "T1 is still there after the retry landed, copy in another folder: {other_folder}"
        );
    }
}

/// Failure (fifth hole): a Replace onto C that crashed leaves a temp named for C; a later sighting of the unchanged C (a link at C, as a later save or an
/// adoption notes it) dropped the destination and with it the folder. The removal refuses.
#[test]
fn a_crashed_replace_then_a_sighting_of_the_unchanged_file_still_refuses() {
    let dir = folder();
    let root = dir.join("state");
    let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let target = put(&elsewhere, "c.pdf", b"%PDF-1.4 c, which a replace was going to overwrite");
    crash_save_as(&root, &mine, &target);
    notes(
        &root,
        &SaveNote::Starting {
            path: &target,
            from: Some(stamp(&target)),
            destination: None,
        },
    );
    assert_eq!(
        planned(&root, &mine, TargetMode::Save).unwrap_err(),
        recto_core::removal::Refusal::Incomplete(Store::WriterTemp)
    );
}

/// Failure (verification): a Replace onto a symbolic link `C` in folder X that points to `Y/real.pdf`. lopdf resolves the whole path, so its temp is made in Y
/// and named for `real.pdf`; only X (the link's own folder) was listed, so a crashed Replace left the source's whole document in Y and removal said none found.
#[cfg(unix)]
#[test]
fn a_crashed_replace_onto_a_link_into_another_folder_refuses() {
    let dir = folder();
    let root = dir.join("state");
    let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
    let (x, y) = (dir.join("x"), dir.join("y"));
    std::fs::create_dir_all(&x).unwrap();
    std::fs::create_dir_all(&y).unwrap();
    let real = put(&y, "real.pdf", b"%PDF-1.4 the file the link names");
    let link = x.join("c.pdf");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    notes(
        &root,
        &SaveNote::Starting {
            path: &mine,
            from: Some(stamp(&mine)),
            destination: Some(&link),
        },
    );
    settled(&root);
    // What the crashed writer leaves: its temp, named for the file the link resolves to, in that file's folder.
    put(
        &y,
        &format!("{}9-1.tmp", writer_temp_prefix(Path::new("real.pdf"))),
        b"the whole document and its edits",
    );
    assert_eq!(
        planned(&root, &mine, TargetMode::Save).unwrap_err(),
        recto_core::removal::Refusal::Incomplete(Store::WriterTemp)
    );
}
