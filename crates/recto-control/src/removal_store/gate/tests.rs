//! What an unfinished removal's record does to the session's files, on this Mac's disk, in process: each test names the failure it catches.

use super::*;
use crate::record_store::{Flavor, Loaded, RecordStore};
use crate::removal_store::barrier::fixture::{Kind, pending_at, seed, tree};
use crate::removal_store::testing::Restore;
use crate::state::StatePaths;
use recto_session::HistorySource as _;
use recto_session::record::{self, AppRecord, FindRecord, JournalHeader, SessionRecord, ViewRecord, WithheldPlaces};
use recto_testpdf::TempDir;
use std::path::Path;

fn temp() -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-gate-")
}

fn paths(dir: &TempDir) -> StatePaths {
    StatePaths::overridden(dir.path()).unwrap()
}

/// An empty session record, as a write of it would be.
fn empty_record() -> SessionRecord {
    SessionRecord {
        app: AppRecord::default(),
        view: ViewRecord::default(),
        find: FindRecord::default(),
        edit_mode: false,
        active: None,
        next_id: 1,
        tabs: Vec::new(),
        comparison: None,
        recent: Vec::new(),
        withheld_places: WithheldPlaces::default(),
    }
}

/// Failure: one reader of the state folder says "nothing pending" where another says something is: the record store loads a session (and brings back what a
/// removal is purging) while the updater refuses, or the other way about (an update copies the history being purged). For every shape of `removal/`, the
/// gate, both flavors of the record store's load and the updater's look give the same answer, and only a folder with no record, or the ledger alone, is
/// "nothing": a record that can't be read, one of a newer version, one that isn't ours or isn't readable here is as much a record as a good one.
#[test]
fn every_reader_gives_the_same_answer_for_every_shape_of_the_removal_folder() {
    for kind in Kind::ALL {
        for flavor in [Flavor::App, Flavor::Windowless] {
            let dir = temp();
            let _put_back = (kind == Kind::ReadOnly).then(|| Restore(dir.join("removal").join("pending.v1")));
            seed(dir.path(), kind);
            let paths = paths(&dir);
            let expected = kind.answer();
            let state = state(&paths.root);
            assert_eq!(state["state"], expected, "{kind:?}: {state}");
            let want = match expected {
                "nothing" => None,
                "pending" => Some(Withheld::Pending),
                _ => Some(Withheld::Unreadable),
            };
            assert_eq!(at(&paths.root), want, "{kind:?}");
            assert_eq!(anything_there(&paths.root), want.is_some(), "{kind:?}");
            assert_eq!(
                recto_update::removal_record::anything_there(&paths.root),
                want.is_some(),
                "{kind:?}: the updater's look"
            );
            let store = RecordStore::open(&paths, flavor);
            match (store.load(), want) {
                (Loaded::Nothing, None) => {}
                (Loaded::Withheld(got), Some(want)) => assert_eq!(got, want, "{kind:?}"),
                (other, want) => panic!("{kind:?} {flavor:?}: load gave {other:?}, the gate says {want:?}"),
            }
            assert_eq!(store.withheld(), want, "{kind:?}: what the store keeps");
        }
    }
}

/// Failure: a pending record answers by what step it had reached: `state` says how far it got, and not a path or a name (a QA read is a report to a
/// stranger).
#[test]
fn the_state_says_how_far_and_names_nothing() {
    let dir = temp();
    pending_at(dir.path(), 2);
    let state = state(dir.path());
    assert_eq!(state["state"], "pending");
    assert!(
        state["acked"].as_u64().unwrap() > 0 && state["steps"].as_u64().unwrap() > state["acked"].as_u64().unwrap(),
        "{state}"
    );
    assert!(state["outcome"].is_string(), "{state}");
    let text = state.to_string();
    assert!(!text.contains(dir.path().to_str().unwrap()) && !text.contains("/d/a.pdf"), "a path in {text}");
    assert_eq!(state.as_object().unwrap().len(), 4, "{state}");
}

/// Failure (the gate's reason for being): a removal killed after its barrier, at each of its steps, and the app started again. The load brings back
/// what the plan is purging (a `session.v1` that went back comes forward over the session; damaged slots are rotated aside, the oldest deleted; temporaries
/// are swept; a history is copied for going back), or a write or a delete follows (the session, a journal, a checkpoint, a sleep file, a history taken for a
/// document being opened), or an update's copy is put over the session. Every one of them is refused or returns nothing, and the state folder is
/// byte for byte, name for name, inode for inode and time for time what the crash left.
#[test]
fn a_restart_after_a_crash_at_any_step_reads_moves_writes_and_deletes_nothing() {
    for later in 0..=3 {
        let dir = temp();
        let root = dir.path();
        pending_at(root, later);
        // What a load would act on: a session that went back (`session.v1`, marked), damaged slots with a temporary beside them, the journals' folder with
        // a journal of the document being purged, and the update's whole copy of an older session.
        std::fs::create_dir_all(root.join("session.v1")).unwrap();
        std::fs::write(root.join("session.v1").join("went-back"), b"1").unwrap();
        std::fs::write(root.join("session.v1").join("record.a.json"), b"what the older app wrote").unwrap();
        std::fs::create_dir_all(root.join("session").join("journals")).unwrap();
        std::fs::write(root.join("session").join("record.a.json"), b"damaged, not a record").unwrap();
        std::fs::write(root.join("session").join("record.b.json.tmp123"), b"a temporary").unwrap();
        let journal = record::journal_file_name(None, Path::new("/d/a.pdf"), None);
        std::fs::write(root.join("session").join("journals").join(&journal), b"a journal of the purged document").unwrap();
        std::fs::create_dir_all(root.join("update").join("before")).unwrap();
        std::fs::write(root.join("update").join("before").join(".whole"), b"").unwrap();
        std::fs::write(root.join("update").join("before").join("record.a.json"), b"the history being purged").unwrap();

        let paths = paths(&dir);
        let store = RecordStore::open_as(&paths, Flavor::App, 3);
        let before = tree(root);
        let loaded = store.load();
        assert!(matches!(loaded, Loaded::Withheld(Withheld::Pending)), "later {later}: {loaded:?}");
        assert!(!root.join("session.damaged").exists(), "later {later}: damaged slots were rotated aside");
        assert!(!root.join("session.v3").exists(), "later {later}: a copy was made going forward");

        let stamp = recto_platform::FileStamp {
            size: 1,
            mtime_ns: 2,
            device: 3,
            inode: 4,
        };
        let path = Path::new("/d/a.pdf");
        let header = JournalHeader::new(1, path, Some(stamp), 3, 0);
        let base: record::EditsRecord = serde_json::from_str(r#"{"file_pages":3,"pages":[]}"#).unwrap();
        let refused = |what: &str, result: Result<(), crate::record_store::StoreError>| {
            let error = result.expect_err(what);
            assert_eq!(error.code, "removal_pending", "later {later}: {what}");
        };
        refused("write_checkpoint", store.write_checkpoint(&journal, &header, &base));
        refused(
            "write_record",
            store.write_record(&empty_record(), &record::PlaceDelta::default(), true).map(|_| ()),
        );
        let (_, error) = store.write_journals(&recto_session::JournalWork::default(), true);
        assert_eq!(error.map(|error| error.code), Some("removal_pending"), "later {later}: write_journals");
        assert!(store.keep_pages_of(path, stamp, &[0]).is_none(), "later {later}: keep_pages_of");
        assert_eq!(
            store.write_sleep_bytes(path, stamp, b"a sleep file"),
            0,
            "later {later}: a sleep file was written"
        );
        let older = store.remove_older_sleep_files(8);
        assert_eq!(older.removed, 0, "later {later}: older sleep files were removed");
        store.remove_journal(&journal);
        store.remove_sleep(stamp, path);
        assert!(store.read_list(&journal).is_none(), "later {later}: a list was read");
        assert!(store.read_sleep(stamp, path).is_none(), "later {later}: a sleep file was read");
        assert!(store.history_of(Some(stamp), path).is_none(), "later {later}: a history was taken");
        // The updater's own put-back of its copy, as `fall_back_if_unreadable` and the installer would run it.
        recto_update::install::restore_record(&root.join("update").join("before"), &root.join("session"));

        assert_eq!(tree(root), before, "later {later}: the state folder changed");
        assert_eq!(
            std::fs::read(root.join("session").join("record.a.json")).unwrap(),
            b"damaged, not a record",
            "later {later}: the session's slot was written over"
        );
    }
}

/// Failure: the one-transaction token is given back late or never (a `Held` dropped, a failed call), so a second removal in the same process is refused
/// for good; or it isn't held at all, so two walks write one record. While a `Held` lives, `begin`, `reopen` and `abandon` are `Busy` and change nothing;
/// once it is dropped they work, and a `begin` that failed gives it back at once.
#[test]
fn one_transaction_in_a_process_is_held_by_its_held_and_given_back_when_it_goes() {
    use crate::removal_store::barrier::{AbandonError, BeginError, Looked, abandon, begin, fixture::rig, look, reopen};
    let r = rig();
    let dir = temp();
    let (held, _progress) = r.begin_held(dir.path());
    let mut again = r.progress();
    assert_eq!(
        begin(dir.path(), &r.plan, &r.surveyed, &r.limits, &mut again, &r.facts).unwrap_err(),
        BeginError::Busy
    );
    let Looked::Pending(recorded) = look(dir.path()) else { panic!("a record") };
    assert_eq!(reopen(dir.path(), &recorded).unwrap_err(), AbandonError::Busy);
    assert_eq!(abandon(dir.path(), &recorded).unwrap_err(), AbandonError::Busy);
    drop(held);
    // Given back: a reopen works and holds it itself; an abandon of a record with a slot written is refused for that, not as busy.
    drop(reopen(dir.path(), &recorded).expect("the token is given back"));
    assert_eq!(abandon(dir.path(), &recorded).unwrap_err(), AbandonError::RecoveryRequired);
    // A begin that fails (a record is there already) doesn't keep it.
    let mut again = r.progress();
    assert_eq!(
        begin(dir.path(), &r.plan, &r.surveyed, &r.limits, &mut again, &r.facts).unwrap_err(),
        BeginError::Pending
    );
    drop(reopen(dir.path(), &recorded).expect("a failed begin gave it back"));
}

/// Failure: the record is looked at from somewhere that isn't the gate (a startup path that treats `Nothing` as it likes, or a command that gives it a path a
/// document named), so what an unfinished removal does is decided twice. Outside the barrier's own tests, only `gate.rs` calls `barrier::look`; and only the
/// record store, the leftovers' leases and check, and the app's QA verb name the gate. A line that is a comment, and a file's tests (from its
/// `#[cfg(test)]` on, or in a `tests` folder or file), name them freely.
#[test]
fn only_the_gate_looks_at_the_record_and_only_a_few_ask_the_gate() {
    fn walk(folder: &Path, found: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    // The crates that can reach the record store at all: no other depends on this one.
    for name in ["recto-control", "recto-app", "recto-cli", "recto-ffi"] {
        walk(&crates.join(name).join("src"), &mut files);
    }
    let in_code = |what: &[&str]| -> Vec<String> {
        let mut found: Vec<String> = files
            .iter()
            .filter(|(path, text)| {
                let test_file = path.contains("/tests/")
                    || path.ends_with("tests.rs")
                    || path.ends_with("_tests.rs")
                    || path.contains("/headless/")
                    || path.ends_with("fixture.rs");
                let code = text.lines().take_while(|line| line.trim() != "mod tests {");
                !test_file
                    && code
                        .into_iter()
                        .any(|line| !line.trim_start().starts_with("//") && what.iter().any(|name| line.contains(name)))
            })
            .map(|(path, _)| path.rsplit("/src/").next().unwrap().to_owned())
            .collect();
        found.sort();
        found
    };
    assert_eq!(in_code(&["barrier::look("]), ["removal_store/gate.rs"], "where the record is looked at");
    assert_eq!(
        in_code(&["gate::at(", "gate::state(", "gate::anything_there(", "gate::backups_may_change(", "removal_store::gate"]),
        [
            "core/removal.rs",
            "core/saving.rs",
            "core/start.rs",
            "host.rs",
            "record_store.rs",
            "record_store/withheld.rs",
            "removal_store/ledger.rs",
            "removal_store/leftovers/leases.rs",
            "removal_store/leftovers/run.rs"
        ],
        "who asks the gate"
    );
}

/// Failure: a damaged, too-new or not-ours record lets a backup be made or pruned (only a readable pending one is looked at), or a record's ledger-only folder
/// withholds them. For every shape of `removal/`, backups may change exactly when the gate says nothing is there.
#[test]
fn backups_may_not_change_while_any_record_shape_is_there() {
    for kind in Kind::ALL {
        let dir = temp();
        let _put_back = (kind == Kind::ReadOnly).then(|| Restore(dir.join("removal").join("pending.v1")));
        seed(dir.path(), kind);
        let paths = paths(&dir);
        assert_eq!(backups_may_change(&paths.root), kind.answer() == "nothing", "{kind:?}");
    }
}

/// A backup file made old enough for the prune to take it.
fn old_backup(root: &Path) -> std::path::PathBuf {
    let folder = root.join("backups.noindex");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("original-aaaaaaaa.pdf");
    std::fs::write(&file, b"%PDF-1.4 an original").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(400 * 24 * 60 * 60);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
    file
}

/// Failure: a session that starts while a removal is unfinished prunes the old backups (a file the removal's plan pinned goes outside it), or, with no record
/// there, no longer prunes at all. A host (`recto serve`, `recto session`) makes no backup for a save while a record is there either, and makes one with none.
#[test]
fn old_backups_stay_while_a_record_is_there() {
    use crate::host::{BackupsOnly, SessionHost};
    use recto_agent::saving::SaveSupport as _;
    for pending in [false, true] {
        let dir = temp();
        let paths = paths(&dir);
        if pending {
            pending_at(&paths.root, 1);
        }
        let file = old_backup(&paths.root);
        let _host = SessionHost::new(recto_agent::DrawableLoader).with_backups(&paths);
        assert_eq!(file.exists(), pending, "pending {pending}: an old backup after the host started");
        let support = BackupsOnly {
            folder: crate::host::backups_folder(&paths),
            root: paths.root.clone(),
        };
        assert_eq!(support.backups().is_some(), !pending, "pending {pending}: a save's backup folder");
    }
}

/// Failure (the gates' review): a save's note is appended to the ledger while an unfinished removal's record is there, so something is kept from a window that
/// is to keep nothing, for a backup that was never made. With a record there nothing is written (the ledger isn't even created); with none, the note is.
#[test]
fn a_saves_note_is_not_appended_while_a_record_is_there() {
    for pending in [true, false] {
        let dir = temp();
        let root = dir.join("state");
        std::fs::create_dir_all(&root).unwrap();
        if pending {
            pending_at(&root, 1);
        }
        let file = dir.join("a.pdf");
        std::fs::write(&file, b"%PDF-1.4 a").unwrap();
        let seen = recto_platform::file_stamp(&file).unwrap();
        let before = tree(&root);
        crate::removal_store::ledger::note_save(
            &root.join("backups.noindex"),
            &recto_agent::saving::SaveNote::Starting {
                path: &file,
                from: Some(seen),
                destination: None,
            },
        );
        assert_eq!(tree(&root) == before, pending, "record there {pending}");
    }
}
