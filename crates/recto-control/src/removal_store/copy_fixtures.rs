//! What the two copy surveys' tests share: a lineage with two documents, a session written by the real record store, and a survey that has every
//! other store marked, so what's asserted is the copy store alone. Only compiled for tests.

use super::testing::{limits, version};
use crate::record_store::{Flavor, RecordStore};
use crate::state::StatePaths;
use recto_core::removal::{Artifact, ArtifactKey, Format, LEDGER_LIMITS, Ledger, Lineage, Link, Owner, Refusal, Store, Survey, Surveyed, lineage};
use recto_platform::FileStamp;
use recto_session::record::{
    AppRecord, AuthorName, FileRecord, FindRecord, JournalHeader, JournalOp, PathField, SessionRecord, TabEntry, ViewRecord, WithheldPlaces, journal_file_name,
    journal_start,
};
use recto_session::{JournalAppend, JournalWork, TabId};
use recto_testpdf::TempDir;
use std::path::{Path, PathBuf};

pub(super) const MINE: &str = "/docs/mine.pdf";
pub(super) const THEIRS: &str = "/docs/theirs.pdf";

pub(super) fn stamp(inode: u64) -> FileStamp {
    FileStamp {
        size: 10,
        mtime_ns: 1,
        device: 0,
        inode,
    }
}

/// This document at `MINE` (inode 7), and another at `THEIRS` (inode 8), from the beginning of time.
pub(super) fn ledger() -> Ledger {
    let at = |inode, path: &str| Link::At {
        version: version(inode),
        path: PathBuf::from(path),
    };
    Ledger {
        links: vec![at(7, MINE), at(8, THEIRS)],
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

/// A survey with every enumerated store but `under_test` marked complete and the lineage noted, handed to `work`, then finished.
pub(super) fn surveyed(under_test: &[Store], work: impl FnOnce(&Lineage<'_>, &mut Survey) -> Result<(), Refusal>) -> Result<Surveyed, Refusal> {
    let ledger = ledger();
    let lineage = lineage(&ledger, version(7), Path::new(MINE), &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(256));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && !under_test.contains(s)) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    work(&lineage, &mut survey)?;
    survey.finish()
}

/// What the survey found, as (store, owner, format, key of `name`) in the survey's own order, and whether each store is complete.
pub(super) fn found_of(s: &Surveyed) -> Vec<(Store, Owner, Format, ArtifactKey)> {
    s.inventory()
        .artifacts()
        .iter()
        .map(|a: &Artifact| {
            let (store, key) = s.key(a.id).unwrap();
            (store, a.owner, a.format, key)
        })
        .collect()
}

pub(super) fn complete(s: &Surveyed, store: Store) -> bool {
    s.inventory().complete().contains(store)
}

/// The artifact `store` has under `name`, if the survey found one.
pub(super) fn find(s: &Surveyed, store: Store, name: &str) -> Option<(Owner, Format)> {
    let key = ArtifactKey::of(store, name.as_bytes());
    found_of(s).into_iter().find(|f| f.0 == store && f.3 == key).map(|f| (f.1, f.2))
}

/// A state folder with the real record store in it.
pub(super) struct State {
    pub dir: TempDir,
    pub paths: StatePaths,
}

pub(super) fn state() -> State {
    let dir = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-copies-");
    let paths = StatePaths::overridden(&dir).unwrap();
    paths.prepare().unwrap();
    State { dir, paths }
}

impl State {
    pub fn root(&self) -> &Path {
        &self.dir
    }

    pub fn session(&self) -> PathBuf {
        self.dir.join("session")
    }

    pub fn store(&self) -> RecordStore {
        RecordStore::open(&self.paths, Flavor::App)
    }
}

/// A record with one tab on `MINE` (inode 7, as the lineage has it) and one on `THEIRS`.
pub(super) fn session_record() -> SessionRecord {
    let tab = |id: u32, path: &str, inode: u64| TabEntry {
        id,
        path: PathField::Text(path.to_string()),
        file: Some(FileRecord {
            size: 10,
            mtime_ns: 1,
            device: 0,
            inode,
        }),
        page: 0,
        page_count: 5,
        place: None,
        view_rotation: 0,
        toc_open: true,
        layout: Default::default(),
        renderer_only: false,
        selection: None,
        notice: None,
        journal: None,
        edits: None,
        generated_contents: false,
        contents_folds: Vec::new(),
        back: Vec::new(),
        forward: Vec::new(),
        password: false,
    };
    SessionRecord {
        app: AppRecord::default(),
        view: ViewRecord::default(),
        find: FindRecord::default(),
        edit_mode: false,
        active: Some(1),
        next_id: 3,
        tabs: vec![tab(1, MINE, 7), tab(2, THEIRS, 8)],
        comparison: None,
        recent: Vec::new(),
        withheld_places: WithheldPlaces::default(),
    }
}

/// The journal name for the file `inode` at `MINE`.
pub(super) fn journal_name(inode: u64) -> String {
    journal_file_name(Some(stamp(inode)), Path::new(MINE), None)
}

/// Writes the session record and one journal (of this document, version 7) through the real store, as the app does.
pub(super) fn write_session(state: &State) {
    let store = state.store();
    let _ = store.load();
    store.write_record(&session_record(), &Default::default(), false).unwrap();
    let work = JournalWork {
        retired: Vec::new(),
        pages: Vec::new(),
        appends: vec![JournalAppend {
            tab: TabId(1),
            name: journal_name(7),
            header: JournalHeader::new(1, Path::new(MINE), Some(stamp(7)), 3, 0),
            through: 1,
            ops: vec![JournalOp::Rotate(0, 1, 1, AuthorName::Owner)],
            closing: false,
            base: None,
            replaces: None,
        }],
    };
    assert!(store.write_journals(&work, false).1.is_none());
}

/// What `journal_start` makes, for a file a test puts by hand.
pub(super) fn journal_bytes(path: &str, inode: u64) -> Vec<u8> {
    journal_start(&JournalHeader::new(1, Path::new(path), Some(stamp(inode)), 3, 0), None)
}

/// A copy of a folder with everything in it, as `copy_folder` and an update's copy both leave one (plain files and folders only).
pub(super) fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
