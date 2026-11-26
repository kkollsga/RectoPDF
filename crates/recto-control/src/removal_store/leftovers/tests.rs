//! The acts run on real folders in temp roots: the survey pins what it lists (through the caps' sink), a test then changes the world the way the failure
//! it names would, and the act must stop or finish as the doc comment says. The Trash is a folder on the same volume.

use super::act::{CopyState, Done, Env, Hooks, Why, clear, copy_state};
use super::pin::{PinSet, Pins, temp_pid};
use super::trash::{FolderTrash, Trash};
use crate::removal_store::copy_fixtures::{State, copy_tree, found_of, state, surveyed, write_session};
use crate::removal_store::legacy_session::{LegacyCaps, survey_legacy_sessions_with};
use crate::removal_store::testing::{limits, version};
use crate::removal_store::update_copy::{UpdateCaps, survey_update_copy_with};
use crate::removal_store::writer_temps::{TempCaps, survey_writer_temps_with};
use recto_core::removal::{ArtifactKey, LEDGER_LIMITS, Ledger, Link, Store, Survey, TargetMode, lineage};
use recto_core::save::writer_temp_prefix;
use recto_testpdf::TempDir;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const DEAD: u32 = 4_000_001;

fn temp_name(pid: u32, attempt: u32, tail: &str) -> String {
    format!("{}{pid}-{attempt}{tail}", writer_temp_prefix(Path::new("mine.pdf")))
}

fn tmp(prefix: &str) -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), prefix)
}

/// A state root with the real record store's session and its copies, a documents folder with two unfinished saves (a file and a clone folder), a
/// folder standing in for the Trash and a folder outside everything.
struct World {
    state: State,
    docs: TempDir,
    trash: TempDir,
    outside: TempDir,
}

fn world() -> World {
    let state = state();
    write_session(&state);
    copy_tree(&state.session(), &state.root().join("session.v1"));
    copy_tree(&state.session(), &state.root().join("session.damaged"));
    std::fs::create_dir_all(state.root().join("update")).unwrap();
    assert!(recto_update::install::copy_record(
        &state.session(),
        &state.root().join("update").join("before"),
        &mut |_| {}
    ));
    let docs = tmp("recto-docs-");
    std::fs::write(docs.join("mine.pdf"), b"%PDF the person's own").unwrap();
    std::fs::write(docs.join(temp_name(DEAD, 1, ".tmp")), b"a crashed save").unwrap();
    std::fs::create_dir(docs.join(temp_name(DEAD, 2, ".d"))).unwrap();
    std::fs::write(docs.join(temp_name(DEAD, 2, ".d")).join("inside"), b"clone").unwrap();
    let outside = tmp("recto-outside-");
    std::fs::write(outside.join("sentinel"), b"not ours").unwrap();
    World {
        state,
        docs,
        trash: tmp("recto-trash-"),
        outside,
    }
}

impl World {
    fn root(&self) -> &Path {
        self.state.root()
    }

    fn before(&self) -> PathBuf {
        self.root().join("update").join("before")
    }

    /// What the surveys pin, with the copy folders and temps as the artifacts they are.
    fn pins(&self) -> PinSet {
        let sink = Pins::new();
        let ledger = Ledger {
            links: vec![Link::At {
                version: version(7),
                path: self.docs.join("mine.pdf"),
            }],
            destinations: Vec::new(),
            path_bytes: 0,
            began_ns: 0,
            gap: None,
            stopped: None,
            good_len: 0,
            frames: 0,
            overflowed: false,
        };
        let current = self.docs.join("mine.pdf");
        let lineage = lineage(&ledger, version(7), &current, &LEDGER_LIMITS).unwrap();
        let mut survey = Survey::new(&limits(256));
        let temps = TempCaps {
            pins: Some(sink.clone()),
            ..TempCaps::default()
        };
        survey_writer_temps_with(&lineage, &mut survey, &temps).unwrap();
        let legacy = LegacyCaps {
            pins: Some(sink.clone()),
            ..LegacyCaps::default()
        };
        survey_legacy_sessions_with(self.root(), &lineage, TargetMode::Save, &mut survey, &legacy).unwrap();
        let update = UpdateCaps {
            pins: Some(sink.clone()),
            ..UpdateCaps::default()
        };
        survey_update_copy_with(self.root(), &lineage, TargetMode::Save, &mut survey, &update).unwrap();
        sink.set()
    }

    /// The names under the state root's copies and the update's, as (relative path) lines, for "nothing else changed".
    fn listing(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                out.push(format!("{} {}", path.strip_prefix(base).unwrap().display(), meta.ino()));
                if meta.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self.root(), self.root(), &mut out);
        out.sort();
        out
    }
}

fn keys(pins: &PinSet) -> Vec<ArtifactKey> {
    pins.temps.iter().map(|t| t.key).chain(pins.copies.iter().map(|c| c.key)).collect()
}

fn key_of(pins: &PinSet, store: Store, name: &str) -> ArtifactKey {
    pins.copies
        .iter()
        .find(|c| c.store == store && c.name == OsStr::new(name))
        .unwrap_or_else(|| panic!("no pinned copy {name}"))
        .key
}

fn run(pins: &PinSet, keys: &[ArtifactKey], trash: &dyn Trash, alive: &dyn Fn(u32) -> bool, hooks: &mut dyn Hooks) -> super::act::Report {
    clear(pins, keys, &Env { trash, alive }, hooks)
}

/// Stops the walk once it has been asked about this many entries.
struct Stop(usize);

impl Hooks for Stop {
    fn entry(&mut self, _: &OsStr) -> bool {
        if self.0 == 0 {
            return false;
        }
        self.0 -= 1;
        true
    }
}

/// Failure: the pins are not what the surveys list (a copy folder that is no artifact, a temp pinned under another key, or pins made with no sink), so a
/// plan counts what the act can't find, or the act acts on what the plan never showed.
#[test]
fn the_surveys_pin_exactly_the_artifacts_they_list() {
    let w = world();
    let pins = w.pins();
    assert_eq!(pins.temps.len(), 2);
    assert_eq!(pins.copies.len(), 3, "session.v1, session.damaged and update/before");
    let s = surveyed(&[Store::LegacySession, Store::UpdateCopy], |lineage, survey| {
        survey_legacy_sessions_with(
            w.root(),
            lineage,
            TargetMode::Save,
            survey,
            &LegacyCaps {
                pins: Some(Pins::new()),
                ..LegacyCaps::default()
            },
        )?;
        survey_update_copy_with(
            w.root(),
            lineage,
            TargetMode::Save,
            survey,
            &UpdateCaps {
                pins: Some(Pins::new()),
                ..UpdateCaps::default()
            },
        )
    })
    .unwrap();
    let listed: Vec<ArtifactKey> = found_of(&s).into_iter().map(|f| f.3).collect();
    for copy in &pins.copies {
        assert!(listed.contains(&copy.key), "{:?} is an artifact of its store", copy.name);
    }
    // Without a sink nothing is pinned and no copy folder is added: the survey is what it was.
    let plain = surveyed(&[Store::LegacySession], |lineage, survey| {
        survey_legacy_sessions_with(w.root(), lineage, TargetMode::Save, survey, &LegacyCaps::default())
    })
    .unwrap();
    assert!(!found_of(&plain).iter().any(|f| f.3 == key_of(&pins, Store::LegacySession, "session.v1")));
}

/// Failure: the process number is read from the wrong part of the name (the attempt for the pid), so a live save reads as dead or a dead one as live; or a
/// name of another shape gets a number.
#[test]
fn the_process_number_is_the_one_the_writer_puts_in_the_name() {
    assert_eq!(temp_pid(temp_name(321, 9, ".tmp").as_bytes()), Some(321));
    assert_eq!(temp_pid(temp_name(321, 9, ".d").as_bytes()), Some(321));
    let past = format!("{}99999999999-1.tmp", writer_temp_prefix(Path::new("mine.pdf")));
    assert_eq!(temp_pid(past.as_bytes()), None, "past a u32 is no process number");
    assert_eq!(temp_pid(b"report.pdf"), None);
    assert_eq!(temp_pid(format!("{}1-2.bak", writer_temp_prefix(Path::new("a.pdf"))).as_bytes()), None);
}

/// Failure (T6): an unfinished save that the app's own save, or `recto local`, is writing at this moment is moved to the Trash from under it. This process's
/// own number is a live process; `DEAD` is none.
#[test]
fn a_save_still_being_written_is_never_moved() {
    let w = world();
    let live = temp_name(std::process::id(), 7, ".tmp");
    std::fs::write(w.docs.join(&live), b"being written").unwrap();
    let pins = w.pins();
    let trash = FolderTrash::at(&w.trash);
    let key = pins.temps.iter().find(|t| t.name == OsStr::new(&live)).unwrap().key;
    let report = run(&pins, &[key], &trash, &recto_platform::local::process_exists, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::BeingWritten)) if k == key), "{report:?}");
    assert!(w.docs.join(&live).exists(), "still in its folder");
    assert_eq!(std::fs::read_dir(&w.trash).unwrap().count(), 0, "nothing in the Trash");
    // The same names with the process gone are moved: the refusal was the process.
    let dead = pins
        .temps
        .iter()
        .find(|t| t.name != OsStr::new(&live) && t.name.as_encoded_bytes().ends_with(b".tmp"))
        .unwrap()
        .key;
    let report = run(&pins, &[dead], &trash, &|_| false, &mut super::act::Unwatched);
    assert!(report.stopped.is_none(), "{report:?}");
}

/// Failure (T12): a volume with no Trash falls back to deleting the unfinished save, or one the Trash refuses is removed anyway: the person's possibly only
/// copy of an earlier document is gone for good.
#[test]
fn no_trash_never_means_delete() {
    let w = world();
    let pins = w.pins();
    let dead = pins.temps.iter().find(|t| t.name.as_encoded_bytes().ends_with(b".tmp")).unwrap();
    let (key, name) = (dead.key, dead.name.clone());
    let none = FolderTrash::without_trash();
    let report = run(&pins, &[key], &none, &|_| false, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::NoTrash)) if k == key), "{report:?}");
    assert_eq!(std::fs::read(w.docs.join(&name)).unwrap(), b"a crashed save");
    // A Trash that exists and refuses (its folder is gone): the item stays too.
    let refusing = FolderTrash::at(&w.trash.join("missing"));
    let report = run(&pins, &[key], &refusing, &|_| false, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::Trash(_))) if k == key), "{report:?}");
    assert_eq!(std::fs::read(w.docs.join(&name)).unwrap(), b"a crashed save");
}

/// Failure (T11): a copy an earlier version worked in after going back (`went-back`), whose work has not been brought forward, is offered and deleted,
/// and that work is lost.
#[test]
fn a_copy_with_went_back_is_never_offered() {
    let w = world();
    std::fs::write(w.root().join("session.v1").join("went-back"), b"1").unwrap();
    let pins = w.pins();
    let wb = pins.copies.iter().find(|c| c.name == OsStr::new("session.v1")).unwrap();
    assert_eq!(copy_state(wb), CopyState::WentBack);
    let other = pins.copies.iter().find(|c| c.name == OsStr::new("session.damaged")).unwrap();
    assert_eq!(copy_state(other), CopyState::Offerable);
    let before = w.listing();
    let trash = FolderTrash::at(&w.trash);
    // Asked together with a copy that may go: nothing is done, not even that one.
    let report = run(&pins, &[other.key, wb.key], &trash, &|_| false, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::WentBack)) if k == wb.key), "{report:?}");
    assert!(report.done.is_empty());
    assert_eq!(w.listing(), before, "nothing changed");
}

/// Failure (T3): a copy, the folder it is in, or a temp's folder is swapped for a link after the pins were taken and the act follows it, deleting files of
/// the same names somewhere else. Each swap puts a link to a folder with the pinned names in the place of the real one.
#[test]
fn a_folder_swapped_for_a_link_after_the_recheck_is_not_followed() {
    let w = world();
    let pins = w.pins();
    // The same names as the copy holds, in the folder the link will lead to.
    let decoy = w.outside.join("decoy");
    copy_tree(&w.root().join("session.v1"), &decoy);
    std::fs::create_dir_all(w.outside.join("update-decoy")).unwrap();
    copy_tree(&w.before(), &w.outside.join("update-decoy").join("before"));
    std::fs::create_dir_all(w.outside.join("docs-decoy")).unwrap();
    std::fs::write(w.outside.join("docs-decoy").join(&pins.temps[0].name), b"someone else's").unwrap();
    std::fs::write(w.outside.join("docs-decoy").join(&pins.temps[1].name), b"someone else's").unwrap();
    let guarded = |at: &Path| {
        let mut all = Vec::new();
        fn walk(dir: &Path, all: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                all.push(p.display().to_string());
                if p.is_dir() {
                    walk(&p, all);
                }
            }
        }
        walk(at, &mut all);
        all.sort();
        all
    };
    let outside_before = guarded(&w.outside);
    // The copy itself, then the update's folder, then the documents folder: renamed away, a link in the place.
    std::fs::rename(w.root().join("session.v1"), w.root().join("moved-copy")).unwrap();
    std::os::unix::fs::symlink(&decoy, w.root().join("session.v1")).unwrap();
    std::fs::rename(w.root().join("update"), w.root().join("moved-update")).unwrap();
    std::os::unix::fs::symlink(w.outside.join("update-decoy"), w.root().join("update")).unwrap();
    let docs_path = w.docs.path().to_path_buf();
    let moved_docs = docs_path.with_extension("moved");
    std::fs::rename(&docs_path, &moved_docs).unwrap();
    std::os::unix::fs::symlink(w.outside.join("docs-decoy"), &docs_path).unwrap();
    let trash = FolderTrash::at(&w.trash);
    for key in keys(&pins) {
        let report = run(&pins, &[key], &trash, &|_| false, &mut super::act::Unwatched);
        let swapped = pins.temps.iter().any(|t| t.key == key)
            || pins
                .copies
                .iter()
                .any(|c| c.key == key && (c.name == OsStr::new("session.v1") || c.name == OsStr::new("before")));
        if swapped {
            assert!(matches!(report.stopped, Some((_, Why::Changed(_)))), "{report:?}");
        }
    }
    assert_eq!(guarded(&w.outside), outside_before, "nothing outside was touched");
    assert!(moved_docs.join("mine.pdf").exists());
    assert!(
        w.root().join("moved-copy").join("journals").exists(),
        "the real, moved copy was not taken as the pinned one"
    );
    std::fs::remove_file(&docs_path).unwrap();
    std::fs::rename(&moved_docs, &docs_path).unwrap();
}

/// Failure (T4): a link inside a copy (to a folder or a file) is followed, so the act deletes the files it leads to, or stops on it and leaves the copy
/// half taken. The link is unlinked as itself, and what it led to is still there.
#[test]
fn a_link_inside_a_copy_is_unlinked_not_followed() {
    let w = world();
    let copy = w.root().join("session.v1");
    std::os::unix::fs::symlink(w.outside.path(), copy.join("to-a-folder")).unwrap();
    std::os::unix::fs::symlink(w.outside.join("sentinel"), copy.join("to-a-file")).unwrap();
    std::os::unix::fs::symlink(w.outside.path(), copy.join("journals").join("to-a-folder-too")).unwrap();
    let pins = w.pins();
    let key = key_of(&pins, Store::LegacySession, "session.v1");
    let report = run(&pins, &[key], &FolderTrash::at(&w.trash), &|_| false, &mut super::act::Unwatched);
    assert!(report.stopped.is_none(), "{report:?}");
    assert!(!copy.exists(), "the copy is gone");
    assert_eq!(std::fs::read(w.outside.join("sentinel")).unwrap(), b"not ours");
    assert_eq!(std::fs::read_dir(&w.outside).unwrap().count(), 1, "nothing else was added or taken");
}

/// Failure (T5): the update's copy loses `.whole` last, so a copy cut short at any point still says it is whole and `restore_record` (at the next launch of
/// a version that goes back) puts a half copy over the live record and takes away what's been added since. Stopped before each entry in turn, then the
/// installer's own `restore_record` is run: the live record must be as it was.
#[test]
fn a_crash_mid_delete_leaves_no_restorable_update_copy() {
    let mut stopped_at = 0;
    for after in 0.. {
        let w = world();
        let live = w.state.session();
        let live_before = tree_bytes(&live);
        let pins = w.pins();
        let key = key_of(&pins, Store::UpdateCopy, "before");
        let report = run(&pins, &[key], &FolderTrash::at(&w.trash), &|_| false, &mut Stop(after));
        recto_update::install::restore_record(&w.before(), &live);
        assert_eq!(
            tree_bytes(&live),
            live_before,
            "restore_record after {after} entries: the live record is as it was"
        );
        if report.stopped.is_none() {
            assert!(!w.before().exists(), "the copy is gone when nothing stopped it");
            break;
        }
        stopped_at = after;
        assert!(after < 64, "the walk ends");
    }
    assert!(stopped_at >= 3, "the copy had entries to stop between: {stopped_at}");
}

fn tree_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push((p.strip_prefix(base).unwrap().display().to_string(), std::fs::read(&p).unwrap()));
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Failure: something put into a copy after it was looked at (a record slot a running version wrote) is taken with it, or the walk takes out what it
/// can and leaves the copy half gone. It stops before anything of that folder goes, and the entry that was pinned and is gone counts as done.
#[test]
fn an_entry_added_after_the_pin_stops_before_that_folder_goes_and_a_vanished_one_is_done() {
    let w = world();
    let pins = w.pins();
    let copy = w.root().join("session.damaged");
    std::fs::write(copy.join("added-later"), b"new").unwrap();
    let before = w.listing();
    let key = key_of(&pins, Store::LegacySession, "session.damaged");
    let report = run(&pins, &[key], &FolderTrash::at(&w.trash), &|_| false, &mut super::act::Unwatched);
    assert!(
        matches!(report.stopped, Some((_, Why::Unpinned(ref n))) if n == OsStr::new("added-later")),
        "{report:?}"
    );
    assert_eq!(w.listing(), before, "no entry of that folder was taken");
    std::fs::remove_file(copy.join("added-later")).unwrap();
    // A pinned file that has vanished is done, not a failure.
    let gone = std::fs::read_dir(&copy).unwrap().map(|e| e.unwrap().path()).find(|p| p.is_file()).unwrap();
    std::fs::remove_file(&gone).unwrap();
    let report = run(&pins, &[key], &FolderTrash::at(&w.trash), &|_| false, &mut super::act::Unwatched);
    assert!(report.stopped.is_none(), "{report:?}");
    assert!(matches!(report.done[0].1, Done::Deleted { already_gone: 1.., .. }), "{report:?}");
    assert!(!copy.exists());
}

/// Failure: a clear takes more than it was asked, or less: the live record, the document, a name that is no copy (`Session.txt`) or the identity of what went
/// to the Trash changes. One whole clear: the temps are in the Trash with the inode they had, the three copies are gone, and everything else is as it was.
#[test]
fn a_whole_clear_takes_what_was_pinned_and_nothing_else() {
    let w = world();
    std::fs::write(w.root().join("Session.txt"), b"a person's note").unwrap();
    let pins = w.pins();
    let temp_inodes: Vec<(std::ffi::OsString, u64)> = pins.temps.iter().map(|t| (t.name.clone(), t.id.ino)).collect();
    let live_before = tree_bytes(&w.state.session());
    let trash = FolderTrash::at(&w.trash);
    let report = run(&pins, &keys(&pins), &trash, &|_| false, &mut super::act::Unwatched);
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!(report.done.len(), 5);
    // Temps first, then the update's copy.
    assert!(matches!(report.done[0].1, Done::Trashed(_)) && matches!(report.done[1].1, Done::Trashed(_)));
    assert_eq!(report.done[2].0, key_of(&pins, Store::UpdateCopy, "before"));
    for (name, ino) in temp_inodes {
        assert_eq!(
            std::fs::metadata(w.trash.join(&name)).unwrap().ino(),
            ino,
            "{name:?} was renamed into the Trash, not copied"
        );
        assert!(!w.docs.join(&name).exists());
    }
    for gone in ["session.v1", "session.damaged", "update/before"] {
        assert!(!w.root().join(gone).exists(), "{gone}");
    }
    assert_eq!(tree_bytes(&w.state.session()), live_before, "the live record");
    assert_eq!(std::fs::read(w.docs.join("mine.pdf")).unwrap(), b"%PDF the person's own");
    assert_eq!(std::fs::read(w.root().join("Session.txt")).unwrap(), b"a person's note");
    // Cleared again with the same pins: everything is already gone, and that is done.
    let again = run(&pins, &keys(&pins), &trash, &|_| false, &mut super::act::Unwatched);
    assert!(again.stopped.is_none(), "{again:?}");
}

/// Failure: the pins fail open: a key with no pin, or a copy deeper than the walk allows, is acted on by a path anyway.
#[test]
fn a_key_nobody_pinned_is_refused_before_anything_is_done() {
    let w = world();
    let pins = w.pins();
    let before = w.listing();
    let stranger = ArtifactKey::of(Store::WriterTemp, b"nothing");
    let all = [keys(&pins), vec![stranger]].concat();
    let report = run(&pins, &all, &FolderTrash::at(&w.trash), &|_| false, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::NotPinned)) if k == stranger), "{report:?}");
    assert!(report.done.is_empty());
    assert_eq!(w.listing(), before);
    // Four folders below the copy are more than can be pinned: the copy is not pinned, and so not cleared.
    let deep = w.root().join("session.v2").join("a").join("b").join("c").join("d");
    std::fs::create_dir_all(&deep).unwrap();
    let pins = w.pins();
    assert!(pins.copies.iter().all(|c| c.name != OsStr::new("session.v2")));
}
