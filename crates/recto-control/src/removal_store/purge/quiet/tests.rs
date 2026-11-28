//! The proof of quiet over real folders in temp state roots, in process: each test names the failure it catches.

use super::*;
use crate::record_store::{Flavor, RecordStore};
use crate::removal_store::leftovers::leases::Outside;
use crate::removal_store::purge::tests::{Rig, journal_step};
use crate::removal_store::purge::{HistoryRoots, PurgeStop, purge_history};
use crate::state::StatePaths;
use recto_core::removal::Store;
use recto_testpdf::TempDir;
use std::sync::Arc;

const READY: Outside = Outside {
    keeps_record: true,
    beside_newer: false,
    sleep_pass_done: true,
};

/// A prepared state folder with its record store held for a removal, and the clear's leases on it.
struct State {
    paths: StatePaths,
    store: Arc<RecordStore>,
}

impl State {
    fn on(dir: &Path) -> Self {
        let paths = StatePaths::overridden(dir).expect("a state folder");
        paths.prepare().expect("prepared");
        let store = Arc::new(RecordStore::open(&paths, Flavor::App));
        Self { paths, store }
    }
}

fn other_state() -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-quiet-")
}

/// Failure: a hold on one state folder and the leases on another make a proof, so a purge of a folder whose record store is still writing, or whose windowless
/// record a `recto serve` keeps, runs on a proof that covers neither. One folder's pair is a proof, and with it a pinned journal is purged.
#[test]
fn a_proof_of_quiet_is_made_only_from_one_state_folders_hold_and_leases() {
    let rig = Rig::new();
    let a = State::on(rig.dir.path());
    let hold = a.store.hold_for_removal().expect("held");
    let leases = Leases::take(&a.paths.root, READY).expect("leases");
    let quiet = HistoryQuiet::of(&hold, &leases).expect("one folder's pair is a proof");
    let (wanted, _) = journal_step(&rig);
    let roots = HistoryRoots {
        state_root: &a.paths.root,
        backups: &rig.backups,
    };
    let purged = purge_history(Store::Journal, &wanted, &roots, rig.volume(), &quiet).expect("purged");
    assert_eq!(purged.removed, 2);

    let b_dir = other_state();
    let b = State::on(&b_dir);
    let leases_of_b = Leases::take(&b.paths.root, READY).expect("leases of B");
    assert!(HistoryQuiet::of(&hold, &leases_of_b).is_none(), "A's hold with B's leases proves nothing");
}

/// Failure: a proof made for one state folder is accepted for another's roots, so the purge unlinks a pinned journal in a folder nobody proved quiet. The answer
/// is `OtherState`, and the journal is still there.
#[test]
fn a_history_purge_with_another_state_folders_proof_touches_nothing() {
    let rig = Rig::new();
    let (wanted, _) = journal_step(&rig);
    let before = rig.files();
    let a_dir = other_state();
    let a = State::on(&a_dir);
    let hold = a.store.hold_for_removal().expect("held");
    let leases = Leases::take(&a.paths.root, READY).expect("leases");
    let quiet = HistoryQuiet::of(&hold, &leases).expect("A's proof");
    let roots = HistoryRoots {
        state_root: rig.dir.path(),
        backups: &rig.backups,
    };
    assert_eq!(purge_history(Store::Journal, &wanted, &roots, rig.volume(), &quiet), Err(PurgeStop::OtherState));
    assert_eq!(rig.files(), before, "nothing was unlinked");
}

/// Failure: a crate turns on recto-control's `test-seams` outside `[dev-dependencies]`, so `HistoryQuiet::assumed_for_tests` and `CachesQuiet::assumed_for_tests`
/// (a proof of quiet from nothing) are built into the app or the CLI, where a purge could then be called on no proof.
#[test]
fn only_tests_enable_recto_controls_test_seams() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = crates.join("..").join("Cargo.toml");
    let mut manifests = vec![root];
    for entry in std::fs::read_dir(&crates).expect("the crates folder") {
        manifests.push(entry.expect("an entry").path().join("Cargo.toml"));
    }
    let mut looked = 0;
    for manifest in manifests {
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        looked += 1;
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.to_string();
            } else if line.contains("recto-control") && line.contains("test-seams") {
                assert!(
                    section.ends_with("dev-dependencies]"),
                    "{} enables recto-control's test-seams in {section}",
                    manifest.display()
                );
            }
        }
    }
    assert!(looked > 10, "read only {looked} manifests");
}
