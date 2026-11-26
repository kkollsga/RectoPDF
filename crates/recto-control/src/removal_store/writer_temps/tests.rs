//! The adapter lists real folders in a temp root and places by the writer's own names; the plan made from it is what a person would be shown.

use super::*;
use crate::removal_store::testing::{Restore, bounded, limits, mkfifo, mode, put, version};
use recto_core::removal::{DESIGN_LIMITS, Kinds, LEDGER_LIMITS, Ledger, Link, Plan, Request, Surveyed, lineage};
use recto_core::save::writer_temp_prefix;
use recto_testpdf::TempDir;
use std::sync::Arc;

const INCOMPLETE: Refusal = Refusal::Incomplete(Store::WriterTemp);

/// The folder `dir` holds this document (`mine.pdf`, inode 7) and another (`theirs.pdf`, inode 8), seen from the beginning of time.
fn ledger(dir: &Path) -> Ledger {
    let at = |inode, name: &str| Link::At {
        version: version(inode),
        path: dir.join(name),
    };
    Ledger {
        links: vec![at(7, "mine.pdf"), at(8, "theirs.pdf")],
        destinations: Vec::new(),
        path_bytes: 0,
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    }
}

/// The name of a temporary of `dir/name`. Made from the file name alone, as the writer makes it: the same name in any folder has the same prefix.
fn temp(_dir: &Path, name: &str, tail: &str) -> String {
    format!("{}{tail}", writer_temp_prefix(Path::new(name)))
}

/// A survey of the lineage of `ledger` with every store but `WriterTemp` marked complete, so the plan decides on the temps alone.
fn survey_of(ledger: &Ledger, dir: &Path, mode: TargetMode, max: u32, caps: &TempCaps) -> Result<Surveyed, Refusal> {
    let current = dir.join("mine.pdf");
    let lineage = lineage(ledger, version(7), &current, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(max));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::WriterTemp) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    survey_writer_temps_with(&lineage, mode, &mut survey, caps)?;
    survey.finish()
}

fn survey(dir: &Path, mode: TargetMode) -> Surveyed {
    survey_of(&ledger(dir), dir, mode, 64, &TempCaps::default()).unwrap()
}

fn plan_of(s: &Surveyed, mode: TargetMode) -> Result<Plan, Refusal> {
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false);
    recto_core::removal::plan(&request, &s.inventory(), &DESIGN_LIMITS)
}

fn count(s: &Surveyed, owner: Owner) -> usize {
    s.inventory()
        .artifacts()
        .iter()
        .filter(|a| a.store == Store::WriterTemp && a.owner == owner)
        .count()
}

fn total(s: &Surveyed) -> usize {
    s.inventory().artifacts().iter().filter(|a| a.store == Store::WriterTemp).count()
}

/// The folder a save over `mine.pdf` leaves when it crashes: lopdf's `.d` with a `clone` of the document in it.
fn crashed_folder(dir: &Path, name: &str) {
    std::fs::create_dir(dir.join(name)).unwrap();
    put(&dir.join(name), "clone", b"%PDF-1.4");
}

/// Failure: a link followed or a pipe waited on in the person's folder. A pipe, a link to the target PDF (which would place as a plain file with a
/// copy of the document) and a link to a folder, each named as the writer names its temporaries, are `Unknown` and never opened.
#[test]
fn a_pipe_or_a_link_named_as_a_temp_is_unknown_and_never_opened() {
    let dir = TempDir::new("recto-wtemps-");
    let pdf = put(&dir, "mine.pdf", b"%PDF-1.4");
    let other_folder = TempDir::new("recto-wtemps-out-");
    mkfifo(&dir.join(temp(&dir, "mine.pdf", "1-0.tmp")));
    std::os::unix::fs::symlink(&pdf, dir.join(temp(&dir, "mine.pdf", "2-0.tmp"))).unwrap();
    std::os::unix::fs::symlink(other_folder.path(), dir.join(temp(&dir, "mine.pdf", "1-0.d"))).unwrap();
    let path = dir.path().to_path_buf();
    let s = bounded(move || survey(&path, TargetMode::Save));
    assert_eq!((count(&s, Owner::Unknown), total(&s)), (3, 3));
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), INCOMPLETE);
}

/// Failure: the target's folder is listed by its path after the check, so a swap in between (renamed away, a link to another folder put there) lists
/// whatever the link leads to: another folder's temporaries are placed as the target's. Opened once by its descriptor, the swap changes nothing.
#[test]
fn a_folder_swapped_for_a_link_before_it_is_listed_changes_nothing() {
    let dir = TempDir::new("recto-wtemps-");
    let docs = dir.join("docs");
    std::fs::create_dir(&docs).unwrap();
    put(&docs, &temp(&docs, "mine.pdf", "1-0.tmp"), b"mine");
    let outside = TempDir::new("recto-wtemps-out-");
    for n in 2..5 {
        put(&outside, &temp(&docs, "mine.pdf", &format!("{n}-0.tmp")), b"theirs");
    }
    let moved = dir.join("moved");
    let (from, to, link) = (docs.clone(), moved.clone(), outside.path().to_path_buf());
    let hook: Hook = Arc::new(move |_| {
        std::fs::rename(&from, &to).unwrap();
        std::os::unix::fs::symlink(&link, &from).unwrap();
    });
    let caps = TempCaps {
        before_list: Some(hook),
        ..TempCaps::default()
    };
    let s = survey_of(&ledger(&docs), &docs, TargetMode::Save, 64, &caps).unwrap();
    assert!(docs.is_symlink(), "the swap happened");
    assert_eq!((count(&s, Owner::Unknown), total(&s)), (1, 1), "only the folder that was opened was listed");
    // A link already there is not opened at all.
    let s = survey_of(&ledger(&docs), &docs, TargetMode::Save, 64, &TempCaps::default()).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), INCOMPLETE);
}

/// Failure: a crash's copy of the target is left (read as none found), or one is told apart by its hash and let through: any entry of lopdf's exact shape in a
/// listed folder is `Unknown`, the target's, another document's, one of a path the ledger could not place and one of no lineage's alike (fifth hole: a hash
/// that names an unrelated lineage was read `Unrelated`, and the crashed save's leftover was let through).
#[test]
fn any_temp_of_the_exact_shape_is_unknown_whoever_it_is_named_for() {
    let dir = TempDir::new("recto-wtemps-");
    put(&dir, &temp(&dir, "mine.pdf", "1-0.tmp"), b"x");
    crashed_folder(&dir, &temp(&dir, "mine.pdf", "2-0.d"));
    put(&dir, &temp(&dir, "theirs.pdf", "3-0.tmp"), b"x");
    put(&dir, &temp(&dir, "nobody-knows.pdf", "4-0.tmp"), b"x");
    for mode in [TargetMode::Save, TargetMode::SaveAs] {
        let s = survey(&dir, mode);
        assert_eq!((count(&s, Owner::Unknown), total(&s)), (4, 4), "{mode:?}");
        assert_eq!(plan_of(&s, mode).unwrap_err(), INCOMPLETE, "{mode:?}");
    }
}

/// Failure: a folder named as a file, a file named as a folder or a hard-linked temp is taken for something a purge may delete: each is `Unknown`, whatever
/// its kind or links.
#[test]
fn a_temp_of_the_wrong_kind_or_with_other_links_is_unknown() {
    let dir = TempDir::new("recto-wtemps-");
    std::fs::create_dir(dir.join(temp(&dir, "mine.pdf", "1-0.tmp"))).unwrap();
    put(&dir, &temp(&dir, "mine.pdf", "2-0.d"), b"x");
    let s = survey(&dir, TargetMode::Save);
    assert_eq!((count(&s, Owner::Unknown), total(&s)), (2, 2));
    let dir = TempDir::new("recto-wtemps-");
    let outside = TempDir::new("recto-wtemps-out-");
    let other = put(&outside, "other", b"x");
    std::fs::hard_link(&other, dir.join(temp(&dir, "mine.pdf", "1-0.tmp"))).unwrap();
    let s = survey(&dir, TargetMode::Save);
    assert_eq!((count(&s, Owner::Unknown), total(&s)), (1, 1));
}

/// Failure: every removal refuses on the person's own files in their folder, because anything not lopdf's exact shape is named `Unknown`.
#[test]
fn the_persons_own_files_are_ignored() {
    let dir = TempDir::new("recto-wtemps-");
    put(&dir, "report.pdf", b"x");
    mkfifo(&dir.join("pipe"));
    std::fs::create_dir(dir.join("old")).unwrap();
    put(&dir, ".append-zz", b"x");
    put(&dir, &temp(&dir, "mine.pdf", "junk"), b"x");
    put(&dir, &temp(&dir, "mine.pdf", "1-0.tmp.bak"), b"x");
    let s = survey(&dir, TargetMode::Save);
    assert_eq!(total(&s), 0);
    assert!(plan_of(&s, TargetMode::Save).is_ok(), "and the store is complete");
}

/// Failure: a time rule lets a temp through: one that changed after the ledger began, or whose hash no lineage path matches, read "none found". lopdf sweeps
/// only a temp an hour old with a dead owner, so neither says whose it is; both are `Unknown` however old the ledger is.
#[test]
fn a_temp_is_unknown_whatever_its_hash_and_time() {
    let dir = TempDir::new("recto-wtemps-");
    put(&dir, ".append-0123456789abcdef-5-0.tmp", b"x");
    put(&dir, &temp(&dir, "mine.pdf", "1-0.tmp"), b"x");
    for began in [0, i64::MAX / 2] {
        let mut led = ledger(&dir);
        led.began_ns = began;
        let s = survey_of(&led, &dir, TargetMode::Save, 64, &TempCaps::default()).unwrap();
        assert_eq!((count(&s, Owner::Unknown), total(&s)), (2, 2), "ledger began at {began}");
    }
}

/// Failure: a document is opened. The target is a pipe with no writer and the temps have mode 000; every one still places without opening anything.
#[test]
fn nothing_in_the_folder_is_opened() {
    let dir = TempDir::new("recto-wtemps-");
    mkfifo(&dir.join("mine.pdf"));
    mode(&put(&dir, &temp(&dir, "mine.pdf", "1-0.tmp"), b"x"), 0o000);
    crashed_folder(&dir, &temp(&dir, "mine.pdf", "2-0.d"));
    mode(&dir.join(temp(&dir, "mine.pdf", "2-0.d")), 0o000);
    let path = dir.path().to_path_buf();
    let s = bounded(move || survey(&path, TargetMode::Save));
    mode(&dir.join(temp(&dir, "mine.pdf", "2-0.d")), 0o700);
    assert_eq!((count(&s, Owner::Unknown), total(&s)), (2, 2));
}

/// Failure: a document saved under two names has its folder listed once for each, so the same temp is found twice and every removal refuses as
/// `Inconsistent`.
#[test]
fn a_folder_is_listed_once_however_many_paths_are_in_it() {
    let dir = TempDir::new("recto-wtemps-");
    let mut renamed = ledger(&dir);
    renamed.links.push(Link::At {
        version: version(7),
        path: dir.join("mine-old-name.pdf"),
    });
    put(&dir, &temp(&dir, "mine.pdf", "1-0.tmp"), b"x");
    put(&dir, &temp(&dir, "mine-old-name.pdf", "1-0.tmp"), b"x");
    let s = survey_of(&renamed, &dir, TargetMode::Save, 64, &TempCaps::default()).unwrap();
    assert_eq!((count(&s, Owner::Unknown), total(&s)), (2, 2));
}

/// Failure: a cut survey reads complete: the entry cap or the folder cap off by one, a folder that can't be listed marked complete, or a missing
/// one refusing every removal.
#[test]
fn caps_are_exact_and_errors_leave_the_store_unmarked() {
    let dir = TempDir::new("recto-wtemps-");
    for n in 1..=3 {
        put(&dir, &format!("note-{n}"), b"x");
    }
    let caps = TempCaps {
        entries: 3,
        ..TempCaps::default()
    };
    assert!(plan_of(&survey_of(&ledger(&dir), &dir, TargetMode::Save, 64, &caps).unwrap(), TargetMode::Save).is_ok());
    put(&dir, "note-4", b"x");
    assert_eq!(survey_of(&ledger(&dir), &dir, TargetMode::Save, 64, &caps).unwrap_err(), Refusal::TooLarge);

    // Three folders hold target paths and the third is missing: it holds no file, so it counts toward no cap. A cap of 2 is complete, a cap of 1 too large.
    let elsewhere = TempDir::new("recto-wtemps-else-");
    let mut two = ledger(&dir);
    two.links.push(Link::At {
        version: version(7),
        path: elsewhere.join("mine.pdf"),
    });
    let gone = elsewhere.join("gone");
    two.links.push(Link::At {
        version: version(7),
        path: gone.join("mine.pdf"),
    });
    let at = |folders| TempCaps {
        folders,
        ..TempCaps::default()
    };
    let big = TempCaps { entries: 99, ..at(3) };
    assert!(
        plan_of(&survey_of(&two, &dir, TargetMode::Save, 64, &big).unwrap(), TargetMode::Save).is_ok(),
        "three folders, one of them missing"
    );
    assert!(
        plan_of(
            &survey_of(&two, &dir, TargetMode::Save, 64, &TempCaps { entries: 99, ..at(2) }).unwrap(),
            TargetMode::Save
        )
        .is_ok(),
        "the missing folder is not counted"
    );
    assert_eq!(
        survey_of(&two, &dir, TargetMode::Save, 64, &TempCaps { entries: 99, ..at(1) }).unwrap_err(),
        Refusal::TooLarge
    );
    // Never-landed destinations in folders that are not there cost nothing however many, and the cap's limit plus one of those that are is too large.
    let mut lost = ledger(&dir);
    lost.destinations = (0..5).map(|n| dir.join(format!("gone-{n}")).join("copy.pdf")).collect();
    assert!(
        plan_of(
            &survey_of(&lost, &dir, TargetMode::Save, 64, &TempCaps { entries: 99, ..at(1) }).unwrap(),
            TargetMode::Save
        )
        .is_ok()
    );
    for n in 0..2 {
        std::fs::create_dir(dir.join(format!("gone-{n}"))).unwrap();
    }
    assert_eq!(
        survey_of(&lost, &dir, TargetMode::Save, 64, &TempCaps { entries: 99, ..at(2) }).unwrap_err(),
        Refusal::TooLarge,
        "the target's folder and two destination folders that are there: 3 over a cap of 2"
    );
    // A folder that can't be entered: unmarked.
    let _restore = Restore(elsewhere.path().to_path_buf());
    mode(&elsewhere, 0o000);
    let s = survey_of(&two, &dir, TargetMode::Save, 64, &big).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), INCOMPLETE, "no access");
    mode(&elsewhere, 0o700);
    let s = survey_of(
        &ledger(&dir),
        &dir,
        TargetMode::Save,
        64,
        &TempCaps {
            entries: 99,
            ..TempCaps::default()
        },
    )
    .unwrap();
    assert!(plan_of(&s, TargetMode::Save).is_ok());
}

/// A real ledger fed by a save's own first note (`Starting`), as the session sends it before the writer makes anything.
fn begin_save(root: &Path, source: &Path, destination: &Path) {
    let stamp = recto_platform::file_stamp(source).unwrap();
    crate::removal_store::ledger::note_save(
        &root.join("backups.noindex"),
        &recto_agent::saving::SaveNote::Starting {
            path: source,
            from: Some(stamp),
            destination: Some(destination),
        },
    );
}

/// The survey of the lineage of the file at `source` in the ledger of `root`. Every store but the writer temps is marked complete, so the plan decides
/// on the temps alone.
fn survey_of_root(root: &Path, source: &Path) -> Surveyed {
    let ledger = crate::removal_store::ledger::read(root);
    let current = recto_platform::file_stamp(source).unwrap();
    let version = recto_core::removal::FileVersion::new(current, recto_platform::volume::volume_identity(source));
    let lineage = lineage(&ledger, version, source, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(64));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::WriterTemp) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    survey_writer_temps_with(&lineage, TargetMode::Save, &mut survey, &TempCaps::default()).unwrap();
    survey.finish().unwrap()
}

/// Failure (review round 1): a Save As of `mine.pdf` to `copy.pdf` crashes while the writer builds its temp, which is named for `copy.pdf` and holds the
/// whole target with the edits. The save's first note named only the source, so the destination's folder was never listed (another folder) or its temp
/// was ignored as "changed after the ledger began, of a save that recorded its path" (the same folder): the plan said none found. The destination is
/// recorded at `Starting`, so its folder is listed and its temp reads `Unknown`: the plan refuses.
#[test]
fn a_crashed_save_as_leaves_a_temp_the_survey_finds_in_the_destinations_folder() {
    let dir = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-wtemps-");
    for n in 0..2 {
        // A folder of its own for each, so one's temp is not in the other's survey.
        let home = dir.join(format!("home-{n}"));
        std::fs::create_dir_all(home.join("elsewhere")).unwrap();
        let source = put(&home, "mine.pdf", b"%PDF-1.4");
        let folder = if n == 0 { home.clone() } else { home.join("elsewhere") };
        let root = dir.join(format!("state-{n}"));
        let destination = folder.join("copy.pdf");
        begin_save(&root, &source, &destination);
        // The temp is made after the note, as lopdf makes it after the save began.
        put(&folder, &temp(&folder, "copy.pdf", "9-1.tmp"), b"the whole document and its edits");
        let surveyed = survey_of_root(&root, &source);
        assert_eq!(
            (count(&surveyed, Owner::Unknown), total(&surveyed)),
            (1, 1),
            "the temp of a save to {}",
            destination.display()
        );
        assert_eq!(plan_of(&surveyed, TargetMode::Save).unwrap_err(), INCOMPLETE, "{}", destination.display());
    }
}
