//! The acts run on real folders in temp roots: the survey pins what it lists (through the caps' sink), a test then changes the world the way the failure
//! it names would, and the act must stop or finish as the doc comment says. The Trash is a folder on the same volume.

use super::act::{CopyState, Done, Env, Hooks, Why, clear, copy_state};
use super::pin::{Local, PinSet, Pins, temp_pid, tree_of};
use super::trash::{FolderTrash, Trash};
use crate::removal_store::copy_fixtures::{State, copy_tree, found_of, state, surveyed, write_session};
use crate::removal_store::legacy_session::{LegacyCaps, survey_legacy_sessions_with};
use crate::removal_store::testing::{limits, version};
use crate::removal_store::update_copy::{UpdateCaps, survey_update_copy_with};
use crate::removal_store::writer_temps::{TempCaps, survey_writer_temps_with};
use recto_core::removal::{ArtifactKey, LEDGER_LIMITS, Ledger, Link, Owner, Store, Survey, TargetMode, lineage};
use recto_core::save::writer_temp_prefix;
use recto_platform::folder::{Folder, Opened};
use recto_platform::trash::Failure as TrashFailure;
use recto_testpdf::TempDir;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) const DEAD: u32 = 4_000_001;

pub(super) fn temp_name(pid: u32, attempt: u32, tail: &str) -> String {
    format!("{}{pid}-{attempt}{tail}", writer_temp_prefix(Path::new("mine.pdf")))
}

pub(super) fn tmp(prefix: &str) -> TempDir {
    TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), prefix)
}

/// A state root with the real record store's session and its copies, a documents folder with two unfinished saves (a file and a clone folder), a
/// folder standing in for the Trash and a folder outside everything.
pub(super) struct World {
    pub(super) state: State,
    pub(super) docs: TempDir,
    pub(super) trash: TempDir,
    pub(super) outside: TempDir,
}

pub(super) fn world() -> World {
    let state = state();
    write_session(&state);
    copy_tree(&state.session(), &state.root().join("session.damaged.1"));
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

impl PinSet {
    fn temp_named(&self, name: &str) -> Option<&super::pin::TempPin> {
        self.temps.iter().find(|t| t.name == OsStr::new(name))
    }
}

impl World {
    pub(super) fn root(&self) -> &Path {
        self.state.root()
    }

    fn before(&self) -> PathBuf {
        self.root().join("update").join("before")
    }

    /// What the surveys pin, with a Trash that exists and every process dead.
    fn pins(&self) -> PinSet {
        self.pins_with(Arc::new(FolderTrash::at(&self.trash)), Arc::new(|_| false))
    }

    /// What the surveys pin, with this Trash and this process table.
    fn pins_with(&self, trash: Arc<dyn Trash>, alive: Arc<dyn Fn(u32) -> bool + Send + Sync>) -> PinSet {
        self.pins_on(trash, alive, local_everywhere())
    }

    /// What the surveys pin, with this Trash, process table and volume fact.
    fn pins_on(&self, trash: Arc<dyn Trash>, alive: Arc<dyn Fn(u32) -> bool + Send + Sync>, local: Local) -> PinSet {
        let sink = Pins::new(trash, alive, local);
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

/// Where the live record is told to be in a test that has no live record to guard: two paths nothing is at.
fn no_live() -> [&'static Path; 2] {
    [Path::new("/recto-test-no-live-a"), Path::new("/recto-test-no-live-b")]
}

fn local_everywhere() -> Local {
    Arc::new(|_| true)
}

fn run(pins: &PinSet, keys: &[ArtifactKey], trash: &dyn Trash, alive: &dyn Fn(u32) -> bool, hooks: &mut dyn Hooks) -> super::act::Report {
    clear(
        pins,
        keys,
        &Env {
            trash,
            alive,
            local: &|_| true,
            live: no_live(),
        },
        hooks,
    )
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

/// Failure: the pins are not what the surveys list (a copy folder that is no artifact, its entries listed one by one so a record slot refuses every clear, a
/// temp pinned under another key or listed with an owner a plan can't act on, or pins made with no sink), so a plan counts what the act can't find, or
/// refuses what it could clear. In the Leftovers survey each copy is one `Target` artifact and nothing inside it is listed; the Removal survey is as it was.
#[test]
fn the_surveys_pin_exactly_the_artifacts_they_list() {
    let w = world();
    let pins = w.pins();
    assert_eq!(pins.temps.len(), 2);
    assert_eq!(pins.copies.len(), 3, "session.damaged.1, session.damaged and update/before");
    let sink = || Pins::new(Arc::new(FolderTrash::at(&w.trash)), Arc::new(|_| false), local_everywhere());
    let s = surveyed(&[Store::LegacySession, Store::UpdateCopy, Store::WriterTemp], |lineage, survey| {
        survey_legacy_sessions_with(
            w.root(),
            lineage,
            TargetMode::Save,
            survey,
            &LegacyCaps {
                pins: Some(sink()),
                ..LegacyCaps::default()
            },
        )?;
        survey_update_copy_with(
            w.root(),
            lineage,
            TargetMode::Save,
            survey,
            &UpdateCaps {
                pins: Some(sink()),
                ..UpdateCaps::default()
            },
        )?;
        survey_writer_temps_with(
            lineage,
            survey,
            &TempCaps {
                pins: Some(sink()),
                ..TempCaps::default()
            },
        )
    })
    .unwrap();
    let found: Vec<_> = found_of(&s).into_iter().filter(|f| f.0 != Store::WriterTemp).collect();
    let listed: Vec<ArtifactKey> = found.iter().map(|f| f.3).collect();
    assert_eq!(listed.len(), 3, "one artifact a copy and nothing inside it");
    assert!(found.iter().all(|f| f.1 == Owner::Target));
    for copy in &pins.copies {
        assert!(listed.contains(&copy.key), "{:?} is an artifact of its store", copy.name);
    }
    // Without a sink nothing is pinned and no copy folder is added: the survey is what it was.
    let plain = surveyed(&[Store::LegacySession], |lineage, survey| {
        survey_legacy_sessions_with(w.root(), lineage, TargetMode::Save, survey, &LegacyCaps::default())
    })
    .unwrap();
    assert!(!found_of(&plain).iter().any(|f| f.3 == key_of(&pins, Store::LegacySession, "session.damaged.1")));
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

/// Failure (T6): an unfinished save that the app's own save, or `recto local`, is writing at this moment is moved to the Trash from under it, or stops a
/// clear of the others. This process's own number is a live process: the survey leaves that temp out (kept in the pins as waiting) and the clear of the rest
/// goes on; and one whose process came back between the survey and the act is refused by the act.
#[test]
fn a_save_still_being_written_is_never_moved() {
    let w = world();
    let live = temp_name(std::process::id(), 7, ".tmp");
    std::fs::write(w.docs.join(&live), b"being written").unwrap();
    let pins = w.pins_with(Arc::new(FolderTrash::at(&w.trash)), Arc::new(recto_platform::local::process_exists));
    assert!(
        pins.temp_named(&live).is_none() && pins.waiting.iter().any(|t| t.name == OsStr::new(&live)),
        "left out, and said as waiting"
    );
    assert_eq!(pins.temps.len(), 2, "the two dead ones are still offered");
    let trash = FolderTrash::at(&w.trash);
    let report = run(&pins, &keys(&pins), &trash, &|_| false, &mut super::act::Unwatched);
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!(std::fs::read(w.docs.join(&live)).unwrap(), b"being written", "still in its folder");
    // The process of an offered temp comes back before the act.
    let pins = w.pins();
    let key = pins.temps[0].key;
    let before = std::fs::read_dir(&w.trash).unwrap().count();
    let report = run(&pins, &[key], &trash, &|_| true, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::BeingWritten)) if k == key), "{report:?}");
    assert_eq!(std::fs::read_dir(&w.trash).unwrap().count(), before, "nothing went to the Trash");
}

/// Failure (T12): a volume with no Trash falls back to deleting the unfinished save, or one the Trash refuses is removed anyway: the person's possibly only
/// copy of an earlier document is gone for good. A temp on such a volume isn't an artifact (so it doesn't stop a clear) and is kept in the pins as "stays".
#[test]
fn no_trash_never_means_delete() {
    let w = world();
    let none = Arc::new(FolderTrash::without_trash());
    let pins = w.pins_with(none.clone(), Arc::new(|_| false));
    assert!(pins.temps.is_empty() && pins.no_trash.len() == 2, "{pins:?}");
    // The Trash is gone between the survey and the act.
    let pins = w.pins();
    let (key, name) = (pins.temps[0].key, pins.temps[0].name.clone());
    let report = run(&pins, &[key], &*none, &|_| false, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::NoTrash)) if k == key), "{report:?}");
    assert!(w.docs.join(&name).exists());
    // A Trash that exists and refuses (its folder can't be made: a file is in its place): the item stays too.
    std::fs::write(w.trash.join("missing"), b"a file where the Trash's folder would be").unwrap();
    let refusing = FolderTrash::at(&w.trash.join("missing"));
    let report = run(&pins, &[key], &refusing, &|_| false, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::Trash(_))) if k == key), "{report:?}");
    assert!(w.docs.join(&name).exists());
    assert!(
        !report.stopped.as_ref().unwrap().1.in_doubt(),
        "a refused move is a step that failed, with the file where it was"
    );
}

/// Failure (T11): a copy an earlier version worked in after going back (`went-back`), whose work has not been brought forward, is offered and deleted,
/// and that work is lost.
#[test]
fn a_copy_with_went_back_is_never_offered() {
    let w = world();
    std::fs::write(w.root().join("session.damaged.1").join("went-back"), b"1").unwrap();
    let pins = w.pins();
    // Through the survey, which is what a plan is made from: the copy holding it is an artifact core refuses, the other one may be cleared.
    let s = surveyed(&[Store::LegacySession], |lineage, survey| {
        let caps = LegacyCaps {
            pins: Some(Pins::new(Arc::new(FolderTrash::at(&w.trash)), Arc::new(|_| false), local_everywhere())),
            ..LegacyCaps::default()
        };
        survey_legacy_sessions_with(w.root(), lineage, TargetMode::Save, survey, &caps)
    })
    .unwrap();
    let owner_of = |name: &str| {
        found_of(&s)
            .into_iter()
            .find(|f| f.3 == ArtifactKey::of(Store::LegacySession, name.as_bytes()))
            .map(|f| f.1)
    };
    assert_eq!(owner_of("session.damaged.1"), Some(Owner::Unknown), "a copy holding went-back refuses");
    assert_eq!(owner_of("session.damaged"), Some(Owner::Target));
    let wb = pins.copies.iter().find(|c| c.name == OsStr::new("session.damaged.1")).unwrap();
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
    copy_tree(&w.root().join("session.damaged.1"), &decoy);
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
    std::fs::rename(w.root().join("session.damaged.1"), w.root().join("moved-copy")).unwrap();
    std::os::unix::fs::symlink(&decoy, w.root().join("session.damaged.1")).unwrap();
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
                .any(|c| c.key == key && (c.name == OsStr::new("session.damaged.1") || c.name == OsStr::new("before")));
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
    let copy = w.root().join("session.damaged.1");
    std::os::unix::fs::symlink(w.outside.path(), copy.join("to-a-folder")).unwrap();
    std::os::unix::fs::symlink(w.outside.join("sentinel"), copy.join("to-a-file")).unwrap();
    std::os::unix::fs::symlink(w.outside.path(), copy.join("journals").join("to-a-folder-too")).unwrap();
    let pins = w.pins();
    let key = key_of(&pins, Store::LegacySession, "session.damaged.1");
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
        if after == 1 {
            // The first thing taken out is `.whole`, and nothing else is yet: a copy cut short here is one `restore_record` leaves alone.
            assert!(!w.before().join(".whole").exists() && w.before().join("journals").exists());
        }
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
    for gone in ["session.damaged.1", "session.damaged", "update/before"] {
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
    let deep = w.root().join("session.damaged.7").join("a").join("b").join("c").join("d");
    std::fs::create_dir_all(&deep).unwrap();
    let pins = w.pins();
    assert!(pins.copies.iter().all(|c| c.name != OsStr::new("session.damaged.7")));
}

/// A Trash that moves the item into its folder and then says something else than the truth, for the failures a real one can have after it moved.
struct Lying {
    inner: FolderTrash,
    says: Says,
}

enum Says {
    /// The move was made and the system gave no place.
    NoPlace,
    /// The move was made and the place given is another thing (a path that leads somewhere else).
    Elsewhere(PathBuf),
}

impl Trash for Lying {
    fn has_trash_for(&self, path: &Path) -> bool {
        self.inner.has_trash_for(path)
    }

    fn move_to_trash(&self, path: &Path) -> Result<PathBuf, TrashFailure> {
        self.inner.move_to_trash(path)?;
        match &self.says {
            Says::NoPlace => Err(TrashFailure::MovedUnknown),
            Says::Elsewhere(place) => Ok(place.clone()),
        }
    }
}

/// Failure (review 2): the system says it moved an item and gives no place, and that is read as a refusal, so the step is reported failed with the file
/// "still where it was" when it isn't. It is in doubt. And an item that is somewhere else than the Trash's answer says carries where it went.
#[test]
fn a_move_that_says_too_little_or_the_wrong_place_is_in_doubt() {
    let w = world();
    let pins = w.pins();
    let (key, name) = (pins.temps[0].key, pins.temps[0].name.clone());
    let lie = Lying {
        inner: FolderTrash::at(&w.trash),
        says: Says::NoPlace,
    };
    let report = run(&pins, &[key], &lie, &|_| false, &mut super::act::Unwatched);
    let (_, why) = report.stopped.unwrap();
    assert!(matches!(why, Why::Trash(TrashFailure::MovedUnknown)) && why.in_doubt(), "{why:?}");
    assert!(!w.docs.join(&name).exists(), "it did move");
    // The other temp: the Trash says it is at the person's document.
    let (key, name) = (pins.temps[1].key, pins.temps[1].name.clone());
    let wrong = w.docs.join("mine.pdf");
    let lie = Lying {
        inner: FolderTrash::at(&w.trash),
        says: Says::Elsewhere(wrong.clone()),
    };
    let report = run(&pins, &[key], &lie, &|_| false, &mut super::act::Unwatched);
    let (_, why) = report.stopped.unwrap();
    assert!(matches!(&why, Why::NotMoved(to) if *to == wrong) && why.in_doubt(), "{why:?}");
    assert!(w.trash.join(&name).exists());
}

/// Failure (review 3): a link named like an unfinished save is offered and moved (whether the Trash moves the link or what it leads to is not measured),
/// or stops a clear. Only a file or folder is an artifact; the link is left alone and kept as waiting.
#[test]
fn a_link_named_like_an_unfinished_save_is_left_alone() {
    let w = world();
    let link = temp_name(DEAD, 9, ".tmp");
    std::os::unix::fs::symlink(w.outside.join("sentinel"), w.docs.join(&link)).unwrap();
    let pins = w.pins();
    assert_eq!(pins.temps.len(), 2);
    assert!(pins.waiting.iter().any(|t| t.name == OsStr::new(&link)), "kept as waiting: {pins:?}");
    let report = run(&pins, &keys(&pins), &FolderTrash::at(&w.trash), &|_| false, &mut super::act::Unwatched);
    assert!(report.stopped.is_none());
    assert!(std::fs::symlink_metadata(w.docs.join(&link)).is_ok(), "the link is where it was");
    assert_eq!(std::fs::read(w.outside.join("sentinel")).unwrap(), b"not ours");
}

/// Failure (review 4): a process number in a name on a volume that isn't local (a network share, where it may be another Mac's) counts as a dead process, so
/// a save being written from another machine is moved. Not local: waiting, at the survey and again at the act.
#[test]
fn a_dead_pid_on_a_volume_that_isnt_local_proves_nothing() {
    let w = world();
    let pins = w.pins_on(Arc::new(FolderTrash::at(&w.trash)), Arc::new(|_| false), Arc::new(|_| false));
    assert!(pins.temps.is_empty() && pins.waiting.len() == 2, "{pins:?}");
    let pins = w.pins();
    let key = pins.temps[0].key;
    let trash = FolderTrash::at(&w.trash);
    let env = Env {
        trash: &trash,
        alive: &|_| false,
        local: &|_| false,
        live: no_live(),
    };
    let report = clear(&pins, &[key], &env, &mut super::act::Unwatched);
    assert!(matches!(report.stopped, Some((k, Why::BeingWritten)) if k == key), "{report:?}");
}

/// Counts how many times the volume is asked about a Trash.
struct Counting {
    inner: FolderTrash,
    asked: std::sync::atomic::AtomicUsize,
}

impl Trash for Counting {
    fn has_trash_for(&self, path: &Path) -> bool {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.has_trash_for(path)
    }

    fn move_to_trash(&self, path: &Path) -> Result<PathBuf, TrashFailure> {
        self.inner.move_to_trash(path)
    }
}

/// Failure (review 7, rule 1): the volume is asked about its Trash once for every temp, so a folder of many crashed saves costs a system call each.
/// Two temps in one folder ask once.
#[test]
fn the_volume_is_asked_once_for_a_folder_not_once_for_each_temp() {
    let w = world();
    let counting = Arc::new(Counting {
        inner: FolderTrash::at(&w.trash),
        asked: Default::default(),
    });
    let pins = w.pins_with(counting.clone(), Arc::new(|_| false));
    assert_eq!(pins.temps.len(), 2);
    assert_eq!(counting.asked.load(std::sync::atomic::Ordering::Relaxed), 1);
}

fn entries_of(tree: &recto_platform::folder::Tree) -> usize {
    tree.entries.iter().map(|e| 1 + e.below.as_ref().map_or(0, entries_of)).sum()
}

/// Failure (review 7, rule 1): the entry cap is per folder, so a copy of many folders each under the cap is held whole in memory without bound. The cap
/// is the copy's, across every level together, and at the cap plus one the copy is not pinned and is `Unknown`.
#[test]
fn a_copy_has_one_entry_budget_across_all_its_folders() {
    let w = world();
    let pins = w.pins();
    let total = entries_of(&pins.copies.iter().find(|c| c.name == OsStr::new("session.damaged")).unwrap().tree);
    let copy = pins.copies.iter().find(|c| c.name == OsStr::new("session.damaged")).unwrap();
    let journals = copy
        .tree
        .entries
        .iter()
        .find(|e| e.name == OsStr::new("journals"))
        .expect("the copy has a journals folder");
    assert!(
        !journals.below.as_ref().unwrap().entries.is_empty(),
        "journals holds something, so the budget counts a second level"
    );
    assert!(total >= 3, "the copy has a journals folder with something in it: {total}");
    let owners = |n: usize| {
        let s = surveyed(&[Store::LegacySession], |lineage, survey| {
            let caps = LegacyCaps {
                pins: Some(Pins::new(Arc::new(FolderTrash::at(&w.trash)), Arc::new(|_| false), local_everywhere())),
                folder: crate::removal_store::journals::Caps {
                    entries: n,
                    ..Default::default()
                },
                ..LegacyCaps::default()
            };
            survey_legacy_sessions_with(w.root(), lineage, TargetMode::Save, survey, &caps)
        })
        .unwrap();
        found_of(&s)
            .into_iter()
            .find(|f| f.3 == ArtifactKey::of(Store::LegacySession, b"session.damaged"))
            .map(|f| f.1)
    };
    assert_eq!(owners(total), Some(Owner::Target), "at the cap");
    assert_eq!(owners(total - 1), Some(Owner::Unknown), "at the cap plus one: not pinned, and refused");
}

/// Failure (review 8): `went-back` read as a plain file only: a link, a folder or a pipe of that name is `exists()` to the record store, which then brings the
/// copy forward, but the survey calls the copy clearable. Any entry of that name makes it `Unknown`.
#[test]
fn any_entry_named_went_back_makes_the_copy_unknown() {
    let w = world();
    std::os::unix::fs::symlink(w.outside.path(), w.root().join("session.damaged.1").join("went-back")).unwrap();
    let pins = w.pins();
    let wb = pins.copies.iter().find(|c| c.name == OsStr::new("session.damaged.1")).unwrap();
    assert!(wb.went_back);
    assert_eq!(copy_state(wb), CopyState::WentBack);
}

/// Failure (review 10): between the last entry going and the folder being taken out by its name, another folder is put in its place (the copy moved
/// aside), and the take-out by name removes that one. The folder's identity is looked at again right before.
#[test]
fn a_folder_put_in_the_copys_place_at_the_last_moment_is_not_taken() {
    struct Swap(PathBuf);
    impl Hooks for Swap {
        fn entry(&mut self, name: &OsStr) -> bool {
            if name == OsStr::new("session.damaged") {
                // The root's own gate, asked after every entry is gone.
                std::fs::rename(&self.0, self.0.with_extension("aside")).unwrap();
                std::fs::create_dir(&self.0).unwrap();
            }
            true
        }
    }
    let w = world();
    let pins = w.pins();
    let key = key_of(&pins, Store::LegacySession, "session.damaged");
    let at = w.root().join("session.damaged");
    let report = run(&pins, &[key], &FolderTrash::at(&w.trash), &|_| false, &mut Swap(at.clone()));
    assert!(matches!(report.stopped, Some((_, Why::Changed(_)))), "{report:?}");
    assert!(at.is_dir(), "the folder that was put there is still there");
}

/// Failure (review 2.1): the live record's folder is planned as a copy (beside a newer record, `session.v<CURRENT>` is the live store) and a clear deletes it
/// with its unsaved journals. A copy that is one of the two live folders is refused, by what each is when it is looked at.
#[test]
fn a_copy_that_is_a_live_folder_is_refused() {
    let w = world();
    let pins = w.pins();
    let (copy, live_session) = (w.root().join("session.damaged"), w.root().join("session"));
    let trash = FolderTrash::at(&w.trash);
    let before = w.listing();
    for live in [[live_session.as_path(), copy.as_path()], [copy.as_path(), live_session.as_path()]] {
        let env = Env {
            trash: &trash,
            alive: &|_| false,
            local: &|_| true,
            live,
        };
        let report = clear(&pins, &keys(&pins), &env, &mut super::act::Unwatched);
        assert!(matches!(report.stopped, Some((_, Why::LiveRecord))), "{report:?}");
        assert!(report.done.is_empty());
        assert_eq!(w.listing(), before, "nothing was done, not even to the others");
    }
}

/// Failure (review 2.1): until the live folders are told for real, a copy named `<flavor>.v<CURRENT_VERSION>` is deleted: beside a newer record that is the
/// live store. Both flavors' names are refused.
#[test]
fn a_copy_named_for_the_current_version_is_refused() {
    let w = world();
    for flavor in ["session", "session-windowless"] {
        copy_tree(
            &w.state.session(),
            &w.root().join(format!("{flavor}.v{}", recto_session::record::CURRENT_VERSION)),
        );
    }
    let pins = w.pins();
    assert_eq!(pins.copies.len(), 5);
    let before = w.listing();
    for flavor in ["session", "session-windowless"] {
        let name = format!("{flavor}.v{}", recto_session::record::CURRENT_VERSION);
        let key = pins.copies.iter().find(|c| c.name == OsStr::new(&name)).unwrap().key;
        let report = run(&pins, &[key], &FolderTrash::at(&w.trash), &|_| false, &mut super::act::Unwatched);
        assert!(matches!(report.stopped, Some((k, Why::LiveRecord)) if k == key), "{name}: {report:?}");
    }
    assert_eq!(w.listing(), before);
}

/// Failure (review 2.4): a volume mounted inside a copy (an entry on another device) is deleted into with the copy. The comparison is against the copy's own
/// device, at every level; the real case needs a mount, so this holds the comparison by giving the walk another device than the folder's own.
#[test]
fn an_entry_on_another_device_than_the_copy_makes_it_unpinnable() {
    let w = world();
    let Opened::Folder(copy) = Folder::open(&w.root().join("session.damaged")) else {
        panic!("the copy opens")
    };
    let dev = copy.identity().unwrap().dev;
    assert!(tree_of(&copy, 0, &mut 100, dev).is_some(), "its own device is fine at every level");
    assert!(tree_of(&copy, 0, &mut 100, dev + 1).is_none(), "another device than the copy's");
}
