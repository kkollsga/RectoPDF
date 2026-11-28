//! The hold on real folders in a temp state root, in process: each test names the failure it catches.

use super::*;
use crate::removal_store::barrier::fixture::{pending_at, tree};
use crate::state::StatePaths;
use recto_session::TabId;
use recto_session::record::{AppRecord, FindRecord, SessionRecord, ViewRecord, WithheldPlaces};
use recto_session::sleep::{SleepFile, encode_sleep};
use recto_session::{JournalAppend, JournalWork, edit::PageEdits};
use recto_testpdf::TempDir;
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Duration;

fn rotation() -> recto_session::record::JournalOp {
    recto_session::record::JournalOp::Rotate(0, 1, 1, record::AuthorName::Owner)
}

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

fn sleep_of(stamp: FileStamp, pages: u32) -> SleepFile {
    SleepFile {
        file: stamp,
        file_pages: pages,
        pages,
        list_seq: 0,
        renderer_only: false,
        version: None,
        toc: recto_core::Toc::default(),
        usual: Some((612.0, 792.0)),
        sizes: Vec::new(),
    }
}

fn append(name: &str, ops: Vec<recto_session::record::JournalOp>, through: u64, closing: bool) -> JournalWork {
    JournalWork {
        pages: Vec::new(),
        retired: Vec::new(),
        appends: vec![JournalAppend {
            tab: TabId(1),
            name: name.to_string(),
            header: JournalHeader::new(1, Path::new("/d/1.pdf"), None, 5, 0),
            through,
            ops,
            closing,
            base: None,
            replaces: None,
        }],
    }
}

fn five_pages() -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (0..5)
        .map(|at| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("PAGEMARK{at}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

fn waited_for_ledger(root: &Path) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while crate::removal_store::ledger::running(root) {
        assert!(std::time::Instant::now() < until, "the ledger's thread didn't end");
        std::thread::sleep(Duration::from_millis(2)); // ratchet: poll
    }
}

/// What is in the state folder (the ledger's own `removal/` left out: it is written by a thread of its own), for "nothing was written".
fn look(root: &Path) -> BTreeMap<PathBuf, String> {
    waited_for_ledger(root);
    tree(root).into_iter().filter(|(path, _)| !path.starts_with("removal")).collect()
}

/// The files only, not the folders (whose times change when a name came and went).
fn files(root: &Path) -> BTreeMap<PathBuf, String> {
    look(root).into_iter().filter(|(_, what)| !what.contains("is_dir: true")).collect()
}

/// A state folder with every kind of history the store writes, and the names a test acts on.
struct World {
    dir: TempDir,
    store: Arc<RecordStore>,
    file: PathBuf,
    stamp: FileStamp,
    journal: String,
    broken_sleep: (FileStamp, PathBuf),
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-hold-");
        let paths = StatePaths::overridden(&dir).unwrap();
        paths.prepare().unwrap();
        let store = Arc::new(RecordStore::open(&paths, Flavor::App));
        let file = dir.join("doc.pdf");
        std::fs::write(&file, five_pages()).unwrap();
        let stamp = file_stamp(&file).unwrap();
        let folder = dir.join("session").join(JOURNALS);
        store.write_record(&empty_record(), &PlaceDelta::default(), false).unwrap();
        // A long journal (so a compaction is worth it), and a closed one for the file, to be opened again.
        let journal = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        for through in 1..=60 {
            assert!(store.write_journals(&append(&journal, vec![rotation()], through, false), false).1.is_none());
        }
        let closed_for = record::journal_file_name(Some(stamp), &file, None);
        let (list, _) = PageEdits::replay(5, None, &[rotation()]).unwrap();
        let header = JournalHeader::new(1, &file, Some(stamp), 5, 0);
        std::fs::write(
            folder.join(closed_journal_name(&closed_for).unwrap()),
            record::journal_start(&header, Some(&list.to_record())),
        )
        .unwrap();
        // A sleep file for the file, one that doesn't read, and one of an older format.
        assert!(store.write_sleep(&file, &sleep_of(stamp, 5)) > 0);
        let other = FileStamp {
            size: 3,
            mtime_ns: 3,
            device: 1,
            inode: 33,
        };
        let broken = folder.join(record::sleep_file_name(other, Path::new("/d/other.pdf")));
        std::fs::write(&broken, b"not a sleep file").unwrap();
        let older = FileStamp {
            size: 4,
            mtime_ns: 4,
            device: 1,
            inode: 44,
        };
        let older_sleep_path = folder.join(record::sleep_file_name(older, Path::new("/d/older.pdf")));
        let good = encode_sleep(&sleep_of(older, 3));
        let facts = std::str::from_utf8(&good[good.iter().position(|byte| *byte == b'\n').unwrap() + 1..]).unwrap();
        let now = format!("{{\"sleep_format\":{},", recto_session::sleep::SLEEP_FORMAT);
        let mut bytes = facts.trim_end().replacen(&now, "{\"sleep_format\":0,", 1).into_bytes();
        bytes.push(b'\n');
        std::fs::write(&older_sleep_path, record::seal(recto_session::sleep::SLEEP_KIND, 0, &bytes)).unwrap();
        // A sidecar that doesn't read, beside the journal.
        let sidecar = page_ids::name_of(&journal).unwrap();
        std::fs::write(folder.join(sidecar), b"not a sidecar").unwrap();
        Self {
            dir,
            store,
            file,
            stamp,
            journal,
            broken_sleep: (other, broken),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }
}

type Row = (&'static str, fn(&World));

/// Every write path of the store: what each does when nothing is held is written to disk (its row's change), and when held, nothing.
const WRITES: &[Row] = &[
    ("write_journals appends", |w| {
        let (_, refused) = w.store.write_journals(&append(&w.journal, vec![rotation()], 61, false), false);
        let _ = refused;
    }),
    ("write_journals retires", |w| {
        let mut work = append(&w.journal, Vec::new(), 60, false);
        work.appends.clear();
        work.retired = vec![w.journal.clone()];
        w.store.write_journals(&work, false);
    }),
    ("write_checkpoint", |w| {
        let (list, _) = PageEdits::replay(5, None, &[rotation(), rotation()]).unwrap();
        let _ = w
            .store
            .write_checkpoint(&w.journal, &JournalHeader::new(1, Path::new("/d/1.pdf"), None, 5, 0), &list.to_record());
    }),
    ("compact", |w| {
        let (list, _) = PageEdits::replay(5, None, &[]).unwrap();
        let _ = w
            .store
            .compact(&w.journal, &JournalHeader::new(1, Path::new("/d/1.pdf"), None, 5, 0), &list.to_record());
    }),
    ("write_record", |w| {
        let mut record = empty_record();
        record.next_id = 9;
        let _ = w.store.write_record(&record, &PlaceDelta::default(), false);
    }),
    ("write_sleep", |w| {
        w.store.write_sleep(&w.file, &sleep_of(w.stamp, 4));
    }),
    ("remove_sleep", |w| w.store.remove_sleep(w.stamp, &w.file)),
    ("remove_journal", |w| w.store.remove_journal(&w.journal)),
    ("read_sleep removes one that doesn't read", |w| {
        let (stamp, _) = w.broken_sleep;
        let _ = w.store.read_sleep(stamp, Path::new("/d/other.pdf"));
    }),
    ("remove_older_sleep_files", |w| {
        let _ = w.store.remove_older_sleep_files(100);
    }),
    ("keep_pages_of", |w| {
        let target = FileStamp {
            size: 9,
            mtime_ns: 1,
            device: 1,
            inode: 2,
        };
        let _ = w.store.keep_pages_of(&w.file, target, &[1, 3]);
    }),
    ("history_of reopens a closed journal", |w| {
        let _ = w.store.history_of(Some(w.stamp), &w.file);
    }),
    ("read_list removes a sidecar that doesn't read", |w| {
        let _ = HistorySource::read_list(&*w.store, &w.journal);
    }),
];

/// Failure: a journal, sleep file, kept copy, sidecar or record is written, renamed or deleted after `Quiesce` was acknowledged, outside the plan, because
/// one write path asks the gate only before its lock. Every write path is run on a fresh state, not held (it changes the folder, so a row that can't change
/// anything is caught) and held (the folder, with every size and time, is as it was, and a write that can say so says `removal_pending`).
#[test]
fn a_hold_refuses_every_write_and_writes_nothing() {
    for (name, run) in WRITES {
        let free = World::new();
        let before = look(free.root());
        run(&free);
        assert_ne!(look(free.root()), before, "{name}: with nothing held it changes the folder");

        let held = World::new();
        let hold = held.store.hold_for_removal().expect("held");
        let before = look(held.root());
        run(&held);
        assert_eq!(look(held.root()), before, "{name}: held, nothing is written, renamed or deleted");
        drop(hold);
    }
    let w = World::new();
    let _hold = w.store.hold_for_removal().unwrap();
    let (_, refused) = w.store.write_journals(&append(&w.journal, vec![rotation()], 61, false), false);
    assert_eq!(refused.map(|error| error.code), Some("removal_pending"));
    assert_eq!(
        w.store.write_record(&empty_record(), &PlaceDelta::default(), false).unwrap_err().code,
        "removal_pending"
    );
    assert_eq!(w.store.write_sleep(&w.file, &sleep_of(w.stamp, 4)), 0);
}

/// Failure (the race the review found): a write that looked at the gate before the hold lands after it, because only its first look, outside the lock, asked.
/// `write_journals` is stopped between its first check and its lock; the hold is taken; the write is let go and refuses, and nothing is written.
#[test]
fn a_write_that_checked_before_the_hold_refuses_after_it() {
    let w = World::new();
    let (parked_tx, parked_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (parked_tx, go_rx) = (Mutex::new(parked_tx), Mutex::new(go_rx));
    park::set(
        &w.store.root,
        false,
        Arc::new(move || {
            parked_tx.lock().unwrap().send(()).unwrap();
            go_rx.lock().unwrap().recv_timeout(Duration::from_secs(20)).unwrap();
        }),
    );
    let before = look(w.root());
    let writer = {
        let (store, name) = (Arc::clone(&w.store), w.journal.clone());
        std::thread::spawn(move || store.write_journals(&append(&name, vec![rotation()], 61, false), false))
    };
    parked_rx.recv_timeout(Duration::from_secs(20)).expect("the write got as far as its lock");
    let hold = w.store.hold_for_removal().expect("held while a write waits at its lock");
    go_tx.send(()).unwrap();
    let (outcome, refused) = writer.join().unwrap();
    assert_eq!(refused.map(|error| error.code), Some("removal_pending"));
    assert!(outcome.written.is_empty());
    assert_eq!(look(w.root()), before, "nothing was written");
    park::clear(&w.store.root);
    drop(hold);
}

/// Failure: a released hold leaves writes refused for ever, or lets them go on over a barrier begun while it was held. Dropped with no record, writes go
/// on; dropped with a barrier's record on disk, they refuse with `removal_pending`.
#[test]
fn releasing_looks_at_the_disk_again() {
    let w = World::new();
    let hold = w.store.hold_for_removal().unwrap();
    assert!(w.store.write_journals(&append(&w.journal, vec![rotation()], 61, false), false).1.is_some());
    drop(hold);
    assert!(
        w.store.write_journals(&append(&w.journal, vec![rotation()], 61, false), false).1.is_none(),
        "released with no record: writes go on"
    );
    assert!(w.store.hold_for_removal().is_ok(), "and it can be held again");

    let w = World::new();
    let hold = w.store.hold_for_removal().unwrap();
    pending_at(w.root(), 0);
    drop(hold);
    let (_, refused) = w.store.write_journals(&append(&w.journal, vec![rotation()], 61, false), false);
    assert_eq!(
        refused.map(|error| error.code),
        Some("removal_pending"),
        "released over a barrier: still refused"
    );
}

/// Failure: two holds at once, a hold over a pending record, one for a store that doesn't keep the record or can't read it. Each refusal alone.
#[test]
fn a_hold_is_refused_while_a_record_is_there_or_another_holds_it_or_the_store_isnt_ours() {
    let w = World::new();
    let first = w.store.hold_for_removal().unwrap();
    assert_eq!(w.store.hold_for_removal().unwrap_err(), HoldRefused::AlreadyHeld);
    drop(first);

    // A record begun before this store first looked (the gate is looked at once and kept until the next load).
    let dir = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-hold-");
    let paths = StatePaths::overridden(&dir).unwrap();
    paths.prepare().unwrap();
    pending_at(dir.path(), 0);
    let store = Arc::new(RecordStore::open(&paths, Flavor::App));
    assert_eq!(store.hold_for_removal().unwrap_err(), HoldRefused::Withheld);

    let w = World::new();
    let paths = StatePaths::overridden(w.root()).unwrap();
    let second = Arc::new(RecordStore::open(&paths, Flavor::App));
    assert!(!second.holds_the_record());
    assert_eq!(second.hold_for_removal().unwrap_err(), HoldRefused::OtherInstance);

    let w = World::new();
    w.store.progress().read_only = true;
    assert_eq!(w.store.hold_for_removal().unwrap_err(), HoldRefused::Unreadable);
}

/// Failure: a load while held sweeps temporaries or rotates a damaged record (history moved outside the plan): its reset of the store's state forgets the
/// hold, so the next write goes on too.
#[test]
fn a_load_while_held_touches_nothing() {
    let w = World::new();
    let session = w.root().join("session");
    std::fs::write(session.join("record.a.json"), b"damaged, not a record").unwrap();
    std::fs::write(session.join("record.b.json.tmp123"), b"a temporary").unwrap();
    let hold = w.store.hold_for_removal().unwrap();
    let before = look(w.root());
    assert!(matches!(w.store.load(), Loaded::Withheld(Withheld::Pending)));
    assert_eq!(look(w.root()), before, "a load touched nothing");
    assert_eq!(w.store.hold_for_removal().unwrap_err(), HoldRefused::AlreadyHeld, "and the hold is still there");
    assert!(w.store.write_record(&empty_record(), &PlaceDelta::default(), false).is_err());
    drop(hold);
}

/// Failure: a hold that comes in while a kept copy is being made leaves the copy: pages of the target's document written after the walk was told the writers
/// were quiet, or its temporary behind. The copy runs without the lock (it can be 64 MiB); the lock is taken to put it in place. A hold that lands mid-copy
/// leaves no kept file and no temporary.
#[test]
fn a_hold_that_lands_mid_copy_leaves_no_kept_pages_and_no_temp() {
    let w = World::new();
    let before = files(w.root());
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    // The copy is stopped by the sleep seam of the lock's neighbour: the hold is taken from inside the call, between the copy and the rename.
    let store = Arc::clone(&w.store);
    let held = Mutex::new(None);
    park::set(
        &w.store.root,
        true,
        Arc::new(move || {
            *held.lock().unwrap() = store.hold_for_removal().ok();
            assert!(held.lock().unwrap().is_some(), "held between the copy and its place");
        }),
    );
    assert!(w.store.keep_pages_of(&w.file, target, &[1, 3]).is_none());
    park::clear(&w.store.root);
    assert_eq!(files(w.root()), before, "no kept copy, and no temporary");
}
