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
use std::path::{Path, PathBuf};
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
    survey_writer_temps(&lineage, mode, &mut survey)?;
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

/// The folder of a Save As that never landed is listed, and a temp named for its destination reads `Unknown`; a destination that landed is a file with a
/// version, and its folder is no longer listed for it. Failure: a landed destination keeps its folder in every survey (64 Save As folders: `TooLarge`).
#[test]
fn only_a_destination_that_never_landed_has_its_folder_listed() {
    let dir = folder();
    let root = dir.join("state");
    let mine = put(&dir, "mine.pdf", b"%PDF-1.4 mine");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let (landed, crashed) = (elsewhere.join("copy.pdf"), dir.join("lost.pdf"));
    for destination in [&landed, &crashed] {
        notes(
            &root,
            &SaveNote::Starting {
                path: &mine,
                from: Some(stamp(&mine)),
                destination: Some(destination),
            },
        );
        settled(&root);
    }
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
    let temp = |name: &str| format!("{}9-1.tmp", writer_temp_prefix(Path::new(name)));
    // A temp of the copy that landed, in its folder: the folder isn't listed, so it isn't found.
    put(&elsewhere, &temp("copy.pdf"), b"left by something else");
    let survey = planned(&root, &mine, TargetMode::Save);
    assert!(survey.is_ok(), "the landed destination's folder is not listed for it: {survey:?}");
    // A temp of the one that never landed, in its folder: found, and unknown.
    let found: PathBuf = put(&dir, &temp("lost.pdf"), b"the whole document and its edits");
    assert!(found.exists());
    assert_eq!(
        planned(&root, &mine, TargetMode::Save).unwrap_err(),
        recto_core::removal::Refusal::Incomplete(Store::WriterTemp)
    );
}
