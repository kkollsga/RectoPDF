//! The adapter lists a temp folder and decides by name and metadata only; the plan made from it is what a person would be shown.

use super::*;
use recto_core::removal::{
    DESIGN_LIMITS, Disposition, Effect, FileVersion, Gap, Kinds, LEDGER_LIMITS, Ledger, Limits, Link, Plan, Request, Surveyed, lineage, plan,
};
use recto_testpdf::TempDir;
use std::num::NonZeroU16;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const MINE: &str = "/docs/mine.pdf";
const THEIRS: &str = "/docs/theirs.pdf";

fn stamp(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some([3; 16]),
        inode,
        size: 10,
        mtime_ns: 1,
    }
}

/// This document at `MINE` (inode 7), and another one at `THEIRS` (inode 8), from the beginning of time.
fn ledger() -> Ledger {
    Ledger {
        links: vec![
            Link::At {
                version: stamp(7),
                path: PathBuf::from(MINE),
            },
            Link::At {
                version: stamp(8),
                path: PathBuf::from(THEIRS),
            },
        ],
        destinations: Vec::new(),
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    }
}

fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

/// The survey of `folder` with every other store marked complete, so the plan decides on the backups alone.
fn survey(folder: &Path, mode: TargetMode, max: u32) -> Result<Surveyed, Refusal> {
    survey_of(&ledger(), folder, mode, max)
}

fn survey_of(links: &Ledger, folder: &Path, mode: TargetMode, max: u32) -> Result<Surveyed, Refusal> {
    let current = Path::new(MINE);
    let lineage = lineage(links, stamp(7), current, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(max));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::Backup) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
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

/// With a gap or a fork the plan now stops at the history, before it reaches coverage; the adapter's own gate (the store left unmarked) is what this
/// still holds, by reading the mark itself.
fn history_refuses_and_backups_unmarked(s: &Surveyed) {
    assert_eq!(plan_of(s, TargetMode::Save).unwrap_err(), Refusal::HistoryIncomplete);
    assert!(!s.inventory().complete().contains(Store::Backup));
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

/// Puts a folder's mode back on drop, so a failed check can't leave one `TempDir` can't remove.
struct Restore(PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// Failure: a folder that can't be read, or whose parent can't be searched, is marked complete, so a plan claims "none found" about backups it
/// never saw.
#[test]
fn an_unreadable_folder_is_not_complete() {
    let base = TempDir::new("recto-backups-");
    let folder = base.join("backups");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(backup(&folder, MINE), b"%PDF-1.4").unwrap();
    let _restore = [Restore(folder.clone()), Restore(base.to_path_buf())];
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

/// Failure: a link, a directory or a hard link named `.DS_Store` is skipped as Finder's file, so whatever it leads to is never looked at and the plan
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
    // A hard link under that name is a second way to reach another file, so it is shared, not skipped.
    let hard = TempDir::new("recto-backups-");
    std::fs::write(target.join("other"), b"x").unwrap();
    std::fs::hard_link(target.join("other"), hard.join(".DS_Store")).unwrap();
    assert_eq!(refusal(&hard, TargetMode::Save), Refusal::Shared(Store::Backup));
}

/// Failure: the adapter starts reading, renaming or removing a backup. It decides by name and metadata alone (rule 5 and 6: a backup is a PDF
/// of the document, hostile input and not ours to touch here), and the survey is read-only. A tripwire on the source, not proof: the behavioural
/// guard is `the_targets_backup_is_found_and_anothers_left`, whose mode-000 files an opener would trip on.
#[test]
fn the_adapter_only_lists_and_never_touches_a_file() {
    let source = include_str!("../backups.rs");
    let production = source.split("\n#[cfg(test)]\nmod tests;").next().unwrap();
    for word in [
        "File::",
        "fs::copy",
        "hard_link",
        "libc::open",
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

/// Failure: the ledger lost something (a gap, a fork) and the backup folder is still marked fully listed, so the plan says "none found" or
/// purges on a history it only partly knows.
#[test]
fn an_incomplete_ledger_leaves_the_folder_unmarked() {
    let dir = TempDir::new("recto-backups-");
    std::fs::write(backup(&dir, MINE), b"%PDF-1.4").unwrap();
    let mut gapped = ledger();
    gapped.gap = Some(Gap::Torn);
    let s = survey_of(&gapped, &dir, TargetMode::Save, 16).unwrap();
    history_refuses_and_backups_unmarked(&s);
    // The same folder with a whole ledger plans.
    assert!(plan_of(&survey(&dir, TargetMode::Save, 16).unwrap(), TargetMode::Save).is_ok());
    // A fork in the target's own history is a gap too.
    let mut forked = ledger();
    forked
        .links
        .extend([Link::Saved { from: stamp(7), to: stamp(70) }, Link::Saved { from: stamp(7), to: stamp(71) }]);
    let s = survey_of(&forked, &dir, TargetMode::Save, 16).unwrap();
    history_refuses_and_backups_unmarked(&s);
}

/// Failure: a backup made before the ledger existed is placed by a path the ledger only saw later, and the original of another document is
/// purged with this one's.
#[test]
fn a_backup_older_than_the_ledger_is_unknown() {
    let dir = TempDir::new("recto-backups-");
    std::fs::write(backup(&dir, MINE), b"%PDF-1.4").unwrap();
    let mut late = ledger();
    late.began_ns = i64::MAX;
    let s = survey_of(&late, &dir, TargetMode::Save, 16).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), Refusal::Incomplete(Store::Backup));
}

/// Failure: a missing backup folder is marked complete although the ledger lost something (a gap), so the plan says "no backups" about a
/// history that may have had one, in a folder that only seems empty.
#[test]
fn a_missing_folder_is_not_complete_while_the_ledger_has_a_gap() {
    let dir = TempDir::new("recto-backups-");
    let missing = dir.join("none");
    assert!(plan_of(&survey(&missing, TargetMode::Save, 16).unwrap(), TargetMode::Save).is_ok());
    let mut gapped = ledger();
    gapped.gap = Some(Gap::Overflowed);
    let s = survey_of(&gapped, &missing, TargetMode::Save, 16).unwrap();
    history_refuses_and_backups_unmarked(&s);
}

/// Failure: the ledger's beginning is compared in the wrong unit (seconds against nanoseconds) or off by one, so a backup made just before the
/// ledger is placed by it, or one made at its first moment is not. A backup at `began_ns` is the ledger's; one nanosecond older is not.
#[test]
fn a_backup_is_older_than_the_ledger_only_by_a_nanosecond() {
    use std::io::Write;
    let began: i64 = 1_800_000_000_123_456_789;
    let at = |ns: i64| std::time::UNIX_EPOCH + std::time::Duration::from_nanos(u64::try_from(ns).unwrap());
    let made = |ns: i64| {
        let dir = TempDir::new("recto-backups-");
        let mut file = std::fs::File::create(backup(&dir, MINE)).unwrap();
        file.write_all(b"%PDF-1.4").unwrap();
        file.set_modified(at(ns)).unwrap();
        drop(file);
        let mut from = ledger();
        from.began_ns = began;
        let s = survey_of(&from, &dir, TargetMode::Save, 16).unwrap();
        plan_of(&s, TargetMode::Save)
    };
    assert_eq!(purges(&made(began).unwrap()), 1, "made at the first moment: the ledger's");
    assert_eq!(made(began - 1).unwrap_err(), Refusal::Incomplete(Store::Backup), "a nanosecond before: unknown");
    assert_eq!(
        made(began - 2_000_000_000).unwrap_err(),
        Refusal::Incomplete(Store::Backup),
        "seconds before: unknown"
    );
    assert_eq!(purges(&made(began + 1).unwrap()), 1);
}

/// Failure: the backups folder is checked as a real folder and then listed by its path, so one swapped for a link in between (the real one moved
/// away) is followed: the other place's files are listed as the backups, and the real folder's unplaceable file is never seen.
#[test]
fn a_backups_folder_swapped_for_a_link_after_the_check_is_not_followed() {
    let root = TempDir::new("recto-backups-");
    let folder = root.join("backups");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("notes.txt"), b"hi").unwrap();
    let outside = TempDir::new("recto-backups-out-");
    let links = ledger();
    let lineage = lineage(&links, stamp(7), Path::new(MINE), &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(16));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::Backup) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    let swap = |path: &Path| {
        std::fs::rename(path, root.join("moved")).unwrap();
        std::os::unix::fs::symlink(outside.path(), path).unwrap();
    };
    survey_backups_with(&folder, &lineage, TargetMode::Save, &mut survey, &swap).unwrap();
    let s = survey.finish().unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), Refusal::Incomplete(Store::Backup));
}
