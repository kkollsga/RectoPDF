//! The adapter lists a temp folder and decides by name and metadata only; the plan made from it is what a person would be shown.

use super::*;
use recto_core::removal::{DESIGN_LIMITS, Disposition, Effect, Kinds, LEDGER_LIMITS, Limits, Link, Plan, Request, Surveyed, lineage, plan};
use recto_platform::FileStamp;
use recto_testpdf::TempDir;
use std::num::NonZeroU16;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const MINE: &str = "/docs/mine.pdf";
const THEIRS: &str = "/docs/theirs.pdf";

fn stamp(inode: u64) -> FileStamp {
    FileStamp {
        size: 10,
        mtime_ns: 1,
        device: 1,
        inode,
    }
}

/// This document at `MINE` (inode 7), and another one at `THEIRS` (inode 8).
fn ledger() -> Vec<Link> {
    vec![
        Link::At {
            stamp: stamp(7),
            path: PathBuf::from(MINE),
        },
        Link::At {
            stamp: stamp(8),
            path: PathBuf::from(THEIRS),
        },
    ]
}

fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

/// The survey of `folder` with every other store marked complete, so the plan decides on the backups alone.
fn survey(folder: &Path, mode: TargetMode, max: u32) -> Result<Surveyed, Refusal> {
    let links = ledger();
    let current = Path::new(MINE);
    let lineage = lineage(&links, stamp(7), current, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(max));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::Backup) {
        survey.mark_complete(store);
    }
    survey_backups(folder, &lineage, mode, &mut survey)?;
    survey.finish()
}

fn plan_of(s: &Surveyed, mode: TargetMode) -> Result<Plan, Refusal> {
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false);
    plan(&request, &s.inventory(), &DESIGN_LIMITS)
}

fn purges(p: &Plan) -> usize {
    p.steps()
        .iter()
        .filter(|s| s.effect() == Effect::Purge(Store::Backup))
        .map(|s| p.artifacts(s).len())
        .sum()
}

fn backup(folder: &Path, of: &str) -> PathBuf {
    folder.join(backup_name(Path::new(of)))
}

fn partial(folder: &Path, of: &str) -> PathBuf {
    folder.join(format!(".{}.partial", backup_name(Path::new(of))))
}

fn refusal(folder: &Path, mode: TargetMode) -> Refusal {
    plan_of(&survey(folder, mode, 16).unwrap(), mode).unwrap_err()
}

/// Failure: the wrong file goes (another document's backup is purged with this one's), or an adapter that opens each backup and treats the
/// error as a refusal fails on the unreadable (mode 000) files here, so the plan is refused instead of made (as a non-root user; root opens them).
#[test]
fn the_targets_backup_is_found_and_anothers_left() {
    let dir = TempDir::new("recto-backups-");
    for of in [MINE, THEIRS] {
        let path = backup(&dir, of);
        std::fs::write(&path, b"%PDF-1.4").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let s = survey(&dir, TargetMode::Save, 16).unwrap();
    let p = plan_of(&s, TargetMode::Save).unwrap();
    assert_eq!(purges(&p), 1);
    let line = p.line(Store::Backup);
    assert_eq!((line.disposition, line.acted, line.untouched), (Disposition::Purged, 1, 1));
    let id = p.artifacts(p.steps().iter().find(|s| s.effect() == Effect::Purge(Store::Backup)).unwrap())[0];
    let name = backup_name(Path::new(MINE));
    assert_eq!(s.key(id), Some((Store::Backup, ArtifactKey::of(Store::Backup, name.as_bytes()))));
}

/// Failure: a Save As deletes the source's original backup, so the file it left in place loses its history.
#[test]
fn save_as_keeps_the_sources_backup() {
    let dir = TempDir::new("recto-backups-");
    std::fs::write(backup(&dir, MINE), b"%PDF-1.4").unwrap();
    let s = survey(&dir, TargetMode::SaveAs, 16).unwrap();
    let p = plan_of(&s, TargetMode::SaveAs).unwrap();
    assert_eq!(purges(&p), 0);
    let line = p.line(Store::Backup);
    assert_eq!((line.disposition, line.kept), (Disposition::KeptAsSource, 1));
}

/// Failure: an interrupted copy of the original (`.original-....pdf.partial`) survives the removal, with the document's bytes in it.
#[test]
fn a_partial_backup_is_the_targets_too() {
    let dir = TempDir::new("recto-backups-");
    std::fs::write(partial(&dir, MINE), b"%PDF-1.4 half").unwrap();
    std::fs::write(partial(&dir, THEIRS), b"%PDF-1.4 half").unwrap();
    let p = plan_of(&survey(&dir, TargetMode::Save, 16).unwrap(), TargetMode::Save).unwrap();
    assert_eq!(purges(&p), 1);
    assert_eq!((p.line(Store::Backup).acted, p.line(Store::Backup).untouched), (1, 1));
}

/// Failure: an old or foreign file in the folder is skipped, and the plan claims "none found" while a copy of the document is still there.
#[test]
fn a_backup_nobody_can_place_refuses() {
    let unmapped = TempDir::new("recto-backups-");
    std::fs::write(unmapped.join("original-0123456789abcdef01234567.pdf"), b"%PDF-1.4").unwrap();
    assert_eq!(refusal(&unmapped, TargetMode::Save), Refusal::Incomplete(Store::Backup));
    let foreign = TempDir::new("recto-backups-");
    std::fs::write(foreign.join("notes.txt"), b"hello").unwrap();
    assert_eq!(refusal(&foreign, TargetMode::Save), Refusal::Incomplete(Store::Backup));
    let lone = TempDir::new("recto-backups-");
    std::fs::write(lone.join(".DS_Store"), b"x").unwrap();
    let p = plan_of(&survey(&lone, TargetMode::Save, 16).unwrap(), TargetMode::Save).unwrap();
    assert_eq!(p.line(Store::Backup).disposition, Disposition::NoneFound);
}

/// Failure: a link is followed (and what it points at is acted on), or a hard-linked copy of the original survives the purge of its name.
#[test]
fn a_link_folder_or_hard_link_at_the_targets_name_refuses() {
    let outside = TempDir::new("recto-backups-out-");
    let precious = outside.join("precious.pdf");
    std::fs::write(&precious, b"keep me").unwrap();

    let linked = TempDir::new("recto-backups-");
    std::os::unix::fs::symlink(&precious, backup(&linked, MINE)).unwrap();
    assert_eq!(refusal(&linked, TargetMode::Save), Refusal::Incomplete(Store::Backup));
    assert_eq!(std::fs::read(&precious).unwrap(), b"keep me");

    let folder = TempDir::new("recto-backups-");
    std::fs::create_dir(backup(&folder, MINE)).unwrap();
    assert_eq!(refusal(&folder, TargetMode::Save), Refusal::Incomplete(Store::Backup));

    let hard = TempDir::new("recto-backups-");
    std::fs::hard_link(&precious, backup(&hard, MINE)).unwrap();
    assert_eq!(refusal(&hard, TargetMode::Save), Refusal::Shared(Store::Backup));
    assert_eq!(std::fs::read(&precious).unwrap(), b"keep me");
}

/// Failure: a symlinked `backups.noindex` is listed (the files of wherever it leads are taken for backups), or a missing folder reads as
/// unfinished and a plan can never be made for a document that has no backup.
#[test]
fn a_missing_folder_is_complete_and_a_linked_folder_is_not() {
    let base = TempDir::new("recto-backups-");
    let p = plan_of(&survey(&base.join("absent"), TargetMode::Save, 16).unwrap(), TargetMode::Save).unwrap();
    assert_eq!(p.line(Store::Backup).disposition, Disposition::NoneFound);

    let real = base.join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(backup(&real, MINE), b"%PDF-1.4").unwrap();
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(refusal(&link, TargetMode::Save), Refusal::Incomplete(Store::Backup));
    let file = base.join("a-file");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(refusal(&file, TargetMode::Save), Refusal::Incomplete(Store::Backup));
}

/// Failure: a folder past the cap is cut off by stopping the listing at the cap and then marking the store complete (as `prune_backups` does with
/// `take`), so the files past the cap read as "none found" and a copy of the document is left behind.
#[test]
fn a_folder_over_the_cap_refuses() {
    let dir = TempDir::new("recto-backups-");
    for i in 0..4 {
        std::fs::write(dir.join(format!("f{i}")), b"x").unwrap();
    }
    assert_eq!(survey(&dir, TargetMode::Save, 3).unwrap_err(), Refusal::TooLarge);
    std::fs::remove_file(dir.join("f3")).unwrap();
    // Three files fit the cap, and none of them can be placed.
    assert!(survey(&dir, TargetMode::Save, 3).is_ok());
}

/// Failure: a folder that can't be read, or whose parent can't be searched, is marked complete, so a plan claims "none found" about backups it
/// never saw.
#[test]
fn an_unreadable_folder_is_not_complete() {
    let base = TempDir::new("recto-backups-");
    let folder = base.join("backups");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(backup(&folder, MINE), b"%PDF-1.4").unwrap();
    let mode = |path: &Path, bits| std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap();
    mode(&folder, 0o000);
    let unlistable = refusal(&folder, TargetMode::Save);
    mode(&folder, 0o700);
    mode(&base, 0o000);
    let unsearchable = refusal(&folder, TargetMode::Save);
    mode(&base, 0o700);
    assert_eq!(unlistable, Refusal::Incomplete(Store::Backup));
    assert_eq!(unsearchable, Refusal::Incomplete(Store::Backup));
}

/// Failure: a link or a directory named `.DS_Store` is skipped as Finder's file, so whatever it leads to is never looked at and the plan
/// claims "none found".
#[test]
fn only_a_plain_ds_store_file_is_skipped() {
    let target = TempDir::new("recto-backups-out-");
    let linked = TempDir::new("recto-backups-");
    std::os::unix::fs::symlink(target.path(), linked.join(".DS_Store")).unwrap();
    assert_eq!(refusal(&linked, TargetMode::Save), Refusal::Incomplete(Store::Backup));
    let folder = TempDir::new("recto-backups-");
    std::fs::create_dir(folder.join(".DS_Store")).unwrap();
    assert_eq!(refusal(&folder, TargetMode::Save), Refusal::Incomplete(Store::Backup));
}

/// Failure: the adapter starts reading, renaming or removing a backup. It decides by name and metadata alone (rule 5 and 6: a backup is a PDF
/// of the document, hostile input and not ours to touch here), and the survey is read-only.
#[test]
fn the_adapter_only_lists_and_never_touches_a_file() {
    let source = include_str!("../backups.rs");
    let production = source.split("#[cfg(test)]").next().unwrap();
    for word in [
        "File::open",
        "fs::read(",
        "fs::read_to",
        "OpenOptions",
        "rename",
        "remove",
        "fs::write",
        "set_permissions",
    ] {
        assert!(!production.contains(word), "backups.rs must not use `{word}`");
    }
}
