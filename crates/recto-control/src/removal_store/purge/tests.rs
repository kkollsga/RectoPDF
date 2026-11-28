//! The pins and the purge over real folders in a temp state root, with a real survey and a real plan; each test names the failure it catches.

use super::*;
use crate::removal_store::backups::survey_backups_pinned;
use crate::removal_store::journals::survey_journals_pinned;
use crate::removal_store::journals::tests::{MINE, THEIRS, folder, journal, k, ledger, root, sidecar_of, windowless};
use crate::removal_store::testing::Hook;
use crate::removal_store::testing::{put, version};
use recto_core::removal::{ArtifactId, Effect, Kinds, LEDGER_LIMITS, Limits, Owner, Plan, Request, Survey, Surveyed, TargetMode, lineage, plan};
use recto_core::save::backup_name;
use recto_testpdf::TempDir;
use std::collections::BTreeSet;
use std::num::NonZeroU16;
use std::os::unix::fs::MetadataExt as _;
use std::sync::Arc;

const SIX: [Store; 6] = [
    Store::Backup,
    Store::Journal,
    Store::JournalVariant,
    Store::KeptPages,
    Store::PageIds,
    Store::SleepFile,
];

/// A state root with both sessions' journals folders and a backups folder, holding the six stores' files of two documents: the target (`MINE`, version 7) and
/// another (`THEIRS`, version 8).
pub(super) struct Rig {
    pub(super) dir: TempDir,
    pub(super) backups: PathBuf,
}

impl Rig {
    pub(super) fn new() -> Self {
        let dir = root();
        let backups = dir.join("backups.noindex");
        std::fs::create_dir(&backups).unwrap();
        for (path, inode) in [(MINE, 7), (THEIRS, 8)] {
            let name = format!("journal-{}.log", k(inode));
            put(&folder(&dir), &name, &journal(path, inode));
            put(&folder(&dir), &format!("{name}.unreadable"), &journal(path, inode));
            put(&folder(&dir), &format!("pages-{}.ids", k(inode)), &sidecar_of(&name, inode));
            put(&folder(&dir), &format!("sleep-{}.bin", k(inode)), b"sleep");
            put(&folder(&dir), &format!("kept-{}.keptpages", k(inode)), b"%PDF-1.4");
            put(&backups, &backup_name(Path::new(path)), b"%PDF-1.4 before");
        }
        put(&windowless(&dir), &format!("journal-{}.log", k(7)), &journal(MINE, 7));
        Self { dir, backups }
    }

    pub(super) fn roots(&self) -> HistoryRoots<'_> {
        HistoryRoots {
            state_root: self.dir.path(),
            backups: &self.backups,
        }
    }

    pub(super) fn volume(&self) -> VolumeNow {
        VolumeNow {
            root: 0,
            dev: std::fs::metadata(self.dir.path()).unwrap().dev(),
        }
    }

    /// What is left of the six stores, as `<folder>/<name>`.
    pub(super) fn files(&self) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        for (label, path) in [
            ("session", folder(&self.dir)),
            ("windowless", windowless(&self.dir)),
            ("backups", self.backups.clone()),
        ] {
            for entry in std::fs::read_dir(path).unwrap().flatten() {
                found.insert(format!("{label}/{}", entry.file_name().to_string_lossy()));
            }
        }
        found
    }

    /// The target's files: those of version 7 and its backup.
    fn targets(&self) -> BTreeSet<String> {
        let own = |name: &String| name.contains(&k(7)) || name.contains(&backup_name(Path::new(MINE)));
        self.files().into_iter().filter(own).collect()
    }
}

fn limits() -> Limits {
    Limits {
        max_artifacts: 8_192,
        max_batch: NonZeroU16::new(128).unwrap(),
    }
}

/// A real survey of the rig by the pinned adapters (every other store marked complete), the plan of a removal over it, and the pins.
fn surveyed(rig: &Rig, mode: TargetMode) -> (Surveyed, Pins) {
    let links = ledger();
    let lineage = lineage(&links, version(7), Path::new(MINE), &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits());
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && !SIX.contains(s)) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    let mut pins = Pins::default();
    survey_journals_pinned(rig.dir.path(), &lineage, mode, &mut survey, &mut pins).unwrap();
    survey_backups_pinned(&rig.backups, &lineage, mode, &mut survey, &mut pins).unwrap();
    (survey.finish().unwrap(), pins)
}

fn plan_of(s: &Surveyed) -> Plan {
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), TargetMode::Save, false);
    plan(&request, &s.inventory(), &DESIGN_LIMITS).expect("a plan")
}

/// The wanted lists of every `Purge(store)` step of the plan, with the pins the barrier would record for them (this volume is root 0).
fn steps(s: &Surveyed, p: &Plan, pins: &Pins, store: Store) -> Vec<Vec<(ArtifactKey, Pin)>> {
    p.steps()
        .iter()
        .filter(|step| step.effect() == Effect::Purge(store))
        .map(|step| wanted(s, pins, p.artifacts(step)))
        .collect()
}

fn wanted(s: &Surveyed, pins: &Pins, ids: &[ArtifactId]) -> Vec<(ArtifactKey, Pin)> {
    ids.iter()
        .map(|id| {
            let (_, key) = s.key(*id).unwrap();
            let pinned = pins.get(&key).expect("a target's artifact is pinned");
            (
                key,
                Pin {
                    root: 0,
                    inode: pinned.ino,
                    kind: PinKind::File,
                },
            )
        })
        .collect()
}

/// Every step of the plan's six stores, in the plan's order, through the purge.
fn purge_all(rig: &Rig, s: &Surveyed, p: &Plan, pins: &Pins) -> Vec<Result<Purged, PurgeStop>> {
    let mut results = Vec::new();
    for store in SIX {
        for step in steps(s, p, pins, store) {
            results.push(purge_history(store, &step, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()));
        }
    }
    results
}

/// The target's journal step, which holds both sessions' journals.
pub(super) fn journal_step(rig: &Rig) -> (Vec<(ArtifactKey, Pin)>, Surveyed) {
    let (s, pins) = surveyed(rig, TargetMode::Save);
    let p = plan_of(&s);
    let mut found = steps(&s, &p, &pins, Store::Journal);
    assert_eq!(found.len(), 1);
    (found.remove(0), s)
}

/// Failure: an `Unrelated`, `Shared`, `Unknown` or `SaveAsSource` file is pinned, so a later walk could act on it (a pin is what lets the barrier name an artifact
/// the purge may unlink). Only an artifact added as the target's is pinned, and by the file's own device and inode.
#[test]
fn pins_are_exactly_the_targets_artifacts() {
    let rig = Rig::new();
    // A second name for a file that places as the target's (Shared), and a name nobody can place (Unknown).
    std::fs::hard_link(
        folder(&rig.dir).join(format!("kept-{}.keptpages", k(7))),
        folder(&rig.dir).join(format!("kept-{}-1.keptpages", k(7))),
    )
    .unwrap();
    put(&folder(&rig.dir), "notes.txt", b"hello");
    for (mode, target) in [(TargetMode::Save, true), (TargetMode::SaveAs, false)] {
        let links = ledger();
        let lineage = lineage(&links, version(7), Path::new(MINE), &LEDGER_LIMITS).unwrap();
        let mut survey = Survey::new(&limits());
        let mut pins = Pins::default();
        survey_journals_pinned(rig.dir.path(), &lineage, mode, &mut survey, &mut pins).unwrap();
        survey_backups_pinned(&rig.backups, &lineage, mode, &mut survey, &mut pins).unwrap();
        let s = survey.finish().unwrap();
        let mut owners = BTreeSet::new();
        for artifact in s.inventory().artifacts() {
            let (_, key) = s.key(artifact.id).unwrap();
            owners.insert(format!("{:?}", artifact.owner));
            assert_eq!(
                pins.get(&key).is_some(),
                artifact.owner == Owner::Target,
                "{:?} {:?}",
                artifact.store,
                artifact.owner
            );
        }
        for owner in ["Unrelated", "Shared", "Unknown"] {
            assert!(owners.contains(owner), "the rig has {owner} artifacts: {owners:?}");
        }
        assert_eq!(owners.contains("Target"), target, "{mode:?}");
        assert_eq!(owners.contains("SaveAsSource"), !target, "{mode:?}");
    }
    // The pin is the file's own identity.
    let (s, pins) = surveyed(&rig, TargetMode::Save);
    let key = journals::journal_key(Store::Journal, b"session/journals/", std::ffi::OsStr::new(&format!("journal-{}.log", k(7))));
    let meta = std::fs::metadata(folder(&rig.dir).join(format!("journal-{}.log", k(7)))).unwrap();
    let pinned = pins.get(&key).expect("pinned");
    assert_eq!((pinned.dev, pinned.ino, pinned.kind), (meta.dev(), meta.ino(), Kind::File));
    let _ = s;
}

/// Failure: another document's history is deleted with the target's (a purge that matches by store, not by key), or the target's is left behind. Every step of
/// the plan's six stores is run: what is left is exactly what was not the target's.
#[test]
fn purge_removes_exactly_the_planned_files() {
    let rig = Rig::new();
    let others: BTreeSet<String> = rig.files().difference(&rig.targets()).cloned().collect();
    assert!(rig.targets().len() >= 7 && !others.is_empty());
    let (s, pins) = surveyed(&rig, TargetMode::Save);
    let p = plan_of(&s);
    for result in purge_all(&rig, &s, &p, &pins) {
        assert!(result.is_ok(), "{result:?}");
    }
    assert_eq!(rig.files(), others, "the target's history is gone, and another document's is untouched");
}

/// Failure: a resume after a kill between a step's act and its slot fails for ever, because the files it wants are gone. The second run succeeds, removes
/// nothing, and counts them as already gone.
#[test]
fn a_repeated_purge_succeeds_and_counts_what_is_gone() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let first = purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()).unwrap();
    assert_eq!((first.removed, first.already_gone, first.durable), (2, 0, true));
    let again = purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()).unwrap();
    assert_eq!((again.removed, again.already_gone, again.durable), (0, 2, true));
}

/// Failure: a repeat after a kill is counted durable though nothing was synced: the killed run may have unlinked and died before its sync, so the names a
/// repeat finds gone are not on the drive. With names gone, every opened folder is synced, and a failure to is not durable.
#[test]
fn a_run_that_finds_names_gone_syncs_every_folder() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()).unwrap();
    let seams = Seams {
        fail_sync: true,
        ..Seams::default()
    };
    let again = purge_with(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests(), &seams).unwrap();
    assert_eq!((again.removed, again.already_gone, again.durable), (0, 2, false));
    assert_eq!(seams.syncs.load(std::sync::atomic::Ordering::SeqCst), 2, "both sessions' folders");
}

/// Failure: a step that stops in pass 1 (a wanted file that differs) leaves its folders unsynced though a name it wanted was already gone: the killed run that
/// took it may have died before its own sync, as pass 2's exits assume. One wanted file is gone and the other has a second name: the step stops `Linked`,
/// nothing is unlinked, and both sessions' folders are synced.
#[test]
fn a_stop_in_pass_one_still_syncs_when_a_wanted_name_was_already_gone() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let name = format!("journal-{}.log", k(7));
    std::fs::remove_file(windowless(&rig.dir).join(&name)).unwrap();
    std::fs::hard_link(folder(&rig.dir).join(&name), rig.dir.join("another-name")).unwrap();
    let seams = Seams::default();
    let result = purge_with(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests(), &seams);
    assert_eq!(result, Err(PurgeStop::Linked));
    assert!(folder(&rig.dir).join(&name).exists(), "nothing was unlinked");
    assert_eq!(
        seams.syncs.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "both sessions' folders were synced on the way out"
    );
}

/// Failure: a step that stops in pass 2 leaves the names it did take out unsynced, so a power cut brings back history a walk was told was gone. The
/// windowless journal is swapped between the two passes; the session's, already unlinked, is synced on the way out.
#[test]
fn a_stop_in_pass_two_still_syncs_the_folder_that_lost_a_name() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let name = format!("journal-{}.log", k(7));
    let swap: Hook = {
        let at = windowless(&rig.dir).join(&name);
        Arc::new(move |path| {
            if path == at {
                std::fs::remove_file(path).unwrap();
                put(path.parent().unwrap(), path.file_name().unwrap().to_str().unwrap(), &journal(MINE, 7));
            }
        })
    };
    let seams = Seams {
        before_unlink: Some(swap),
        ..Seams::default()
    };
    let result = purge_with(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests(), &seams);
    assert_eq!(result, Err(PurgeStop::NotThePinned));
    assert!(!folder(&rig.dir).join(&name).exists(), "the first was taken out");
    assert_eq!(seams.syncs.load(std::sync::atomic::Ordering::SeqCst), 1, "and its folder was synced");
}

/// Failure: a key given twice (two pins that may disagree) is taken with the last one winning, by an accident of order.
#[test]
fn a_key_twice_in_a_step_is_a_bad_pin() {
    let rig = Rig::new();
    let (mut wanted, _) = journal_step(&rig);
    wanted.push(wanted[0]);
    assert_eq!(
        purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::BadPin)
    );
    assert!(folder(&rig.dir).join(format!("journal-{}.log", k(7))).exists());
}

/// Failure: a file placed by its name alone (sleep, kept, backup) is pinned though it is another user's, so a walk could unlink what this user doesn't own.
/// Only the file's owner being this user pins it.
#[test]
fn a_name_placed_file_is_pinned_only_if_it_is_this_users() {
    let key = ArtifactKey::of(Store::SleepFile, b"x");
    let identity = Identity {
        dev: 1,
        ino: 2,
        kind: Kind::File,
    };
    let mut pins = Pins::default();
    let me = recto_platform::local::current_uid();
    pins.note_owned_by(key, identity, me.wrapping_add(1)).unwrap();
    assert!(pins.get(&key).is_none(), "another user's");
    pins.note_owned_by(key, identity, me).unwrap();
    assert!(pins.get(&key).is_some(), "this user's");
    // And the real survey pins the rig's name-placed files, which are this user's.
    let rig = Rig::new();
    let (s, pins) = surveyed(&rig, TargetMode::Save);
    let targets: Vec<_> = s.inventory().artifacts().iter().filter(|artifact| artifact.owner == Owner::Target).collect();
    assert!(targets.iter().any(|artifact| artifact.store == Store::SleepFile));
    assert!(targets.iter().all(|artifact| pins.get(&s.key(artifact.id).unwrap().1).is_some()));
}

/// Failure: a step half done on a swapped file: the first of two wanted files is unlinked before the second is found to be another file. A file replaced after
/// the survey (the windowless journal, the step's second) stops the step before any unlink, so the session's journal is still there.
#[test]
fn a_file_replaced_after_the_survey_stops_the_step_before_any_unlink() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let name = format!("journal-{}.log", k(7));
    std::fs::remove_file(windowless(&rig.dir).join(&name)).unwrap();
    put(&windowless(&rig.dir), &name, &journal(MINE, 7));
    let result = purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests());
    assert_eq!(result, Err(PurgeStop::NotThePinned));
    assert_eq!(ack(&result), Ack::Failed);
    assert!(folder(&rig.dir).join(&name).exists(), "nothing was unlinked");
    assert!(windowless(&rig.dir).join(&name).exists());
}

/// Failure: another name's file is lost through a hard link made after the survey (the file is another path's too). The step stops, and the file is there.
#[test]
fn a_second_link_made_after_the_survey_stops_the_step() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let name = format!("journal-{}.log", k(7));
    std::fs::hard_link(folder(&rig.dir).join(&name), rig.dir.join("another-name")).unwrap();
    assert_eq!(
        purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::Linked)
    );
    assert!(folder(&rig.dir).join(&name).exists() && windowless(&rig.dir).join(&name).exists());
}

/// Failure: a link's target, or a folder, is unlinked where a wanted file was: a symlink or a folder put at the name after the survey stops the step, and
/// what the link points at is untouched.
#[test]
fn a_link_or_folder_at_a_wanted_name_stops_the_step() {
    for as_folder in [false, true] {
        let rig = Rig::new();
        let (wanted, _) = journal_step(&rig);
        let name = format!("journal-{}.log", k(7));
        let at = folder(&rig.dir).join(&name);
        std::fs::remove_file(&at).unwrap();
        let outside = TempDir::new("recto-purge-out-");
        let precious = put(&outside, "precious", b"keep");
        if as_folder {
            std::fs::create_dir(&at).unwrap();
        } else {
            std::os::unix::fs::symlink(&precious, &at).unwrap();
        }
        let result = purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests());
        assert_eq!(result, Err(PurgeStop::NotAFile), "folder: {as_folder}");
        assert!(precious.exists() && std::fs::symlink_metadata(&at).is_ok());
        assert!(windowless(&rig.dir).join(&name).exists(), "nothing was unlinked");
    }
}

/// Failure: the purge follows a link out of the state folder. A `journals` that is a link before the purge is not ours; and one swapped for a link after
/// the folder was opened changes nothing, as the folder is acted on by its descriptor, not its path.
#[test]
fn a_journals_folder_swapped_for_a_link_is_not_ours() {
    let name = format!("journal-{}.log", k(7));
    // Before: a link at the place.
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let outside = TempDir::new("recto-purge-out-");
    let precious = put(&outside, &name, &journal(MINE, 7));
    std::fs::rename(folder(&rig.dir), rig.dir.join("moved")).unwrap();
    std::os::unix::fs::symlink(outside.path(), folder(&rig.dir)).unwrap();
    assert_eq!(
        purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::FolderNotOurs)
    );
    assert!(precious.exists());
    // After the check, before the listing: the descriptor still names the real folder.
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let outside = TempDir::new("recto-purge-out-");
    let precious = put(&outside, &name, &journal(MINE, 7));
    let swap: Hook = {
        let (real, outside) = (folder(&rig.dir), outside.path().to_path_buf());
        let moved = rig.dir.join("moved");
        Arc::new(move |path| {
            if path == real && !moved.exists() {
                std::fs::rename(&real, &moved).unwrap();
                std::os::unix::fs::symlink(&outside, &real).unwrap();
            }
        })
    };
    let seams = Seams {
        before_list: Some(swap),
        ..Seams::default()
    };
    let result = purge_with(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests(), &seams);
    assert!(result.is_ok(), "{result:?}");
    assert!(precious.exists(), "what the link led to is untouched");
    assert!(!rig.dir.join("moved").join(&name).exists(), "the folder that was opened lost its file");
}

/// Failure: a pin from another disk is matched by inode alone, or a folder's pin is taken for a file's. Each does nothing, and the file is there.
#[test]
fn a_pin_for_another_volume_or_a_folder_does_nothing() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let name = format!("journal-{}.log", k(7));
    let other_root: Vec<_> = wanted.iter().map(|(key, pin)| (*key, Pin { root: 1, ..*pin })).collect();
    assert_eq!(
        purge_history(Store::Journal, &other_root, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::OtherVolume)
    );
    let folders: Vec<_> = wanted.iter().map(|(key, pin)| (*key, Pin { kind: PinKind::Folder, ..*pin })).collect();
    assert_eq!(
        purge_history(Store::Journal, &folders, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::BadPin)
    );
    let other_disk = VolumeNow {
        dev: rig.volume().dev.wrapping_add(1),
        ..rig.volume()
    };
    assert_eq!(
        purge_history(Store::Journal, &wanted, &rig.roots(), other_disk, &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::NotThePinned)
    );
    assert!(folder(&rig.dir).join(&name).exists() && windowless(&rig.dir).join(&name).exists());
}

/// Failure: a cache or the live record is purged by this adapter, or a step is unbounded. A store that isn't one of the six does nothing; 128 wanted are
/// taken and 129 are not.
#[test]
fn a_store_that_isnt_history_or_a_batch_over_the_cap_does_nothing() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let before = rig.files();
    for store in [
        Store::CorpusText,
        Store::GeneratedContents,
        Store::SessionRecord,
        Store::SourcePdf,
        Store::LegacySession,
    ] {
        assert_eq!(
            purge_history(store, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
            Err(PurgeStop::NotHistory),
            "{store:?}"
        );
    }
    assert_eq!(rig.files(), before);
    let many = |count: u32| -> Vec<(ArtifactKey, Pin)> {
        (0..count)
            .map(|at| {
                let name = format!("absent-{at}");
                (
                    ArtifactKey::of(Store::Journal, name.as_bytes()),
                    Pin {
                        root: 0,
                        inode: u64::from(at) + 1,
                        kind: PinKind::File,
                    },
                )
            })
            .collect()
    };
    let at_cap = purge_history(Store::Journal, &many(128), &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()).unwrap();
    assert_eq!((at_cap.removed, at_cap.already_gone), (0, 128));
    assert_eq!(
        purge_history(Store::Journal, &many(129), &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()),
        Err(PurgeStop::TooLarge)
    );
    assert_eq!(rig.files(), before);
}

/// Failure: a purge is counted durable when the drive never had it. A folder that fails to sync gives `Ok` with `durable` false, which the walk
/// acknowledges as done-not-durable; a good one is done.
#[test]
fn an_unsynced_folder_is_done_not_durable() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let seams = Seams {
        fail_sync: true,
        ..Seams::default()
    };
    let result = purge_with(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests(), &seams);
    assert_eq!(result.as_ref().map(|purged| (purged.removed, purged.durable)), Ok((2, false)));
    assert_eq!(ack(&result), Ack::DoneNotDurable);
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let good = purge_history(Store::Journal, &wanted, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests());
    assert_eq!(ack(&good), Ack::Done);
}

/// Failure: a store, a session flavor or a kind of name the purge misses, so removal leaves the target's history. A real survey and a real plan of a removal;
/// every `Purge` step of the six stores through the purge; a second survey finds nothing that is the target's.
#[test]
fn a_plans_history_steps_leave_no_target_history() {
    let rig = Rig::new();
    let (s, pins) = surveyed(&rig, TargetMode::Save);
    let p = plan_of(&s);
    let covered: Vec<_> = SIX.iter().filter(|store| !steps(&s, &p, &pins, **store).is_empty()).collect();
    assert_eq!(covered.len(), 6, "the rig has the target's file in each store: {covered:?}");
    for result in purge_all(&rig, &s, &p, &pins) {
        assert_eq!(result.map(|purged| purged.durable), Ok(true));
    }
    let (after, _) = surveyed(&rig, TargetMode::Save);
    assert!(
        after.inventory().artifacts().iter().all(|artifact| artifact.owner != Owner::Target),
        "nothing of the target's is left"
    );
    assert!(rig.files().len() >= 6, "and another document's history is");
}

/// What a purge of the most a plan can hold costs: 8,192 journals in 64 steps of 128, five times. An estimate, measured here under whatever load there
/// is: run `cargo test --release -p recto-control -- --ignored --nocapture purge_of_the_most_a_plan_holds`, through the build queue.
#[test]
#[ignore = "a cost, not a rule"]
fn purge_of_the_most_a_plan_holds_costs() {
    let mut times = Vec::new();
    for _ in 0..5 {
        let rig = Rig::new();
        for at in [folder(&rig.dir), windowless(&rig.dir), rig.backups.clone()] {
            for entry in std::fs::read_dir(at).unwrap().flatten() {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
        for at in 0..8_192u32 {
            let name = if at == 0 {
                format!("journal-{}.log", k(7))
            } else {
                format!("journal-{}-{at}.log", k(7))
            };
            put(&folder(&rig.dir), &name, &journal(MINE, 7));
        }
        let (s, pins) = surveyed(&rig, TargetMode::Save);
        let p = plan_of(&s);
        let batches = steps(&s, &p, &pins, Store::Journal);
        assert_eq!(batches.len(), 64);
        let began = std::time::Instant::now();
        for batch in &batches {
            assert!(purge_history(Store::Journal, batch, &rig.roots(), rig.volume(), &HistoryQuiet::assumed_for_tests()).is_ok());
        }
        times.push(began.elapsed());
    }
    times.sort();
    eprintln!("purge of 8,192 journals in 64 steps of 128: {times:?}, median {:?}", times[2]);
}
