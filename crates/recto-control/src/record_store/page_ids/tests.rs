//! Journals' sidecars on disk (`page_ids`): what goes in comes back for the list it's of and no other, and a sidecar goes on every path its journal
//! goes by. Each test names the failure it catches.

use super::super::{Flavor, RecordStore};
use super::*;
use crate::state::StatePaths;
use recto_platform::FileStamp;
use recto_session::edit::identity::Known;
use recto_session::record::{self, JournalOp};
use recto_session::{HistorySource, JournalAppend, JournalWork};
use std::path::PathBuf;

const STAMP: FileStamp = FileStamp {
    size: 9,
    mtime_ns: 1,
    device: 1,
    inode: 2,
};

fn state() -> (recto_testpdf::TempDir, StatePaths) {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "rs-ids");
    let paths = StatePaths::overridden(&root).unwrap();
    paths.prepare().unwrap();
    (root, paths)
}

fn known(seed: u8) -> Known {
    Known {
        digest: [seed; 16],
        object: (u32::from(seed) + 3, 0),
        frame: u32::from(seed),
    }
}

/// The journal of `/d/a.pdf` at [`STAMP`], with tab 1.
fn journal_name() -> String {
    record::journal_file_name(Some(STAMP), Path::new("/d/a.pdf"), None)
}

fn header() -> JournalHeader {
    JournalHeader::new(1, Path::new("/d/a.pdf"), Some(STAMP), 5, 0)
}

/// A list of five pages with page 2 turned and pages 1 to 3 read, and what of it waits for the sidecar.
fn read_list() -> (PageEdits, sidecar::Waiting) {
    let mut edits = PageEdits::new(5);
    edits.rotate(2, 1, 1).unwrap();
    assert!(identity::found(&mut edits, &[(1, known(1)), (2, known(2)), (3, known(3))], None));
    let waiting = identity::sidecar_waiting(&edits).expect("waiting");
    (edits, waiting)
}

fn append(name: &str, ops: Vec<JournalOp>, through: u64) -> JournalAppend {
    JournalAppend {
        tab: TabId(1),
        name: name.to_string(),
        header: header(),
        through,
        ops,
        closing: false,
        base: None,
        replaces: None,
    }
}

fn pages_for(name: &str, waiting: &sidecar::Waiting) -> PagesAppend {
    PagesAppend {
        tab: TabId(1),
        journal: name.to_string(),
        header: sidecar::Header::new(name, STAMP, 5),
        waiting: waiting.clone(),
    }
}

fn rotation() -> JournalOp {
    JournalOp::Rotate(2, 1, 1, record::AuthorName::Owner)
}

/// A store with the journal of `/d/a.pdf` written (a turn of page 3) and its sidecar holding pages 1 to 3.
fn written(paths: &StatePaths) -> (RecordStore, PathBuf) {
    let store = RecordStore::open(paths, Flavor::App);
    let (_, waiting) = read_list();
    let name = journal_name();
    let work = JournalWork {
        retired: Vec::new(),
        appends: vec![append(&name, vec![rotation()], 1)],
        pages: vec![pages_for(&name, &waiting)],
    };
    let (outcome, failed) = store.write_journals(&work, false);
    assert_eq!(failed, None);
    assert_eq!(outcome.pages, vec![(TabId(1), waiting, sidecar::Done::Written)]);
    let folder = store.main_folder().join(super::super::JOURNALS);
    (store, folder)
}

fn sidecar_path(folder: &Path) -> PathBuf {
    folder.join(name_of(&journal_name()).unwrap())
}

fn known_of(edits: &PageEdits, page: u32) -> Option<Known> {
    identity::identities(edits)?.known(page)
}

/// Failure caught (B1): what's read of an edit's pages doesn't come back with its list (after a restart, or a wake from dormancy), so the edits can't
/// follow their pages; or it's in the journal, which an app from before then can't read. The journal is byte for byte the journal of its steps alone;
/// the sidecar beside it holds the pages, which come back with the list read from the journal.
#[test]
fn what_was_read_comes_back_beside_a_journal_that_is_only_its_steps() {
    let (_root, paths) = state();
    let (store, folder) = written(&paths);
    let journal = std::fs::read(folder.join(journal_name())).unwrap();
    let mut alone = record::journal_start(&header(), None);
    alone.extend_from_slice(&record::journal_lines(&[rotation()]));
    assert_eq!(journal, alone, "the journal is its steps alone");
    assert!(sidecar_path(&folder).exists());
    let back = store.read_list(&journal_name()).expect("the list");
    for page in 1..=3 {
        assert_eq!(known_of(&back.edits, page), Some(known(page as u8)), "page {page}");
    }
    assert!(identity::sidecar_caught_up(&back.edits), "nothing to write again");
}

/// Failure caught: an append to a sidecar this process didn't write (one a launch read, one left by a copy that went back) is made on top of another
/// list's or another version's entries, so they'd be taken for this one's; or it goes on blindly. One whose header is this list's is appended to; one
/// whose header is another version's gets nothing appended, and asks for a whole write, which replaces it.
#[test]
fn an_append_goes_only_onto_this_lists_own_sidecar() {
    let (_root, paths) = state();
    let (store, folder) = written(&paths);
    let name = journal_name();
    let more = |page: u32| sidecar::Waiting {
        whole: false,
        bytes: sidecar::entries(&[(page, known(page as u8))], None),
        unwritten: 1,
        document: false,
    };
    // Another process (a launch) appends where this one's file ends.
    drop(store);
    let mut progress = Progress::default();
    progress.journal_bytes.insert(name.clone(), 1);
    let mut done = Vec::new();
    write_all(&folder, &mut progress, &[pages_for(&name, &more(4))], &[], false, &mut done);
    assert_eq!(done[0].2, sidecar::Done::Written);
    let read = sidecar::decode(&std::fs::read(sidecar_path(&folder)).unwrap()).unwrap();
    assert_eq!(read.pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    // Another version's header: nothing appended, a whole write asked for.
    let stale = PagesAppend {
        header: sidecar::Header::new(&name, FileStamp { size: 10, ..STAMP }, 5),
        ..pages_for(&name, &more(0))
    };
    let mut fresh = Progress::default();
    fresh.journal_bytes.insert(name.clone(), 1);
    let mut done = Vec::new();
    write_all(&folder, &mut fresh, std::slice::from_ref(&stale), &[], false, &mut done);
    assert_eq!(done[0].2, sidecar::Done::WantsWhole);
    assert_eq!(
        sidecar::decode(&std::fs::read(sidecar_path(&folder)).unwrap()).unwrap().pages.len(),
        4,
        "untouched"
    );
    let whole = PagesAppend {
        waiting: sidecar::Waiting { whole: true, ..more(0) },
        ..stale
    };
    let mut done = Vec::new();
    write_all(&folder, &mut fresh, &[whole], &[], false, &mut done);
    let read = sidecar::decode(&std::fs::read(sidecar_path(&folder)).unwrap()).unwrap();
    assert_eq!((read.header.size, read.pages.len()), (10, 1), "replaced whole");
}

/// Failure caught (B1, rule 6): a sidecar that isn't this journal's or this version's is believed, so an edit would follow a page it was never on:
/// one of another version of the file (a stale one a copy that went back left), one naming another journal, and one that isn't a sidecar at all. None
/// is taken, each goes, and the list itself comes back whole.
#[test]
fn a_stale_foreign_or_broken_sidecar_is_never_taken_and_goes() {
    let (_root, paths) = state();
    let (store, folder) = written(&paths);
    let entries = sidecar::entries(&[(2, known(9))], None);
    let headers = [
        sidecar::Header::new(&journal_name(), FileStamp { mtime_ns: 2, ..STAMP }, 5),
        sidecar::Header::new("journal-00000000000000ff.log", STAMP, 5),
        sidecar::Header::new(&journal_name(), STAMP, 6),
    ];
    let mut cases: Vec<Vec<u8>> = headers
        .iter()
        .map(|header| {
            let mut bytes = header.encode().unwrap();
            bytes.extend_from_slice(&entries);
            bytes
        })
        .collect();
    cases.push(b"not a sidecar at all".to_vec());
    for (at, bytes) in cases.iter().enumerate() {
        std::fs::write(sidecar_path(&folder), bytes).unwrap();
        let fresh = RecordStore::open(&paths, Flavor::App);
        let back = fresh.read_list(&journal_name()).expect("the list comes back");
        assert_eq!(known_of(&back.edits, 2), None, "case {at}");
        assert_eq!(back.edits.slots()[2].quarter_turns, 1, "case {at}: the edit");
        assert!(!sidecar_path(&folder).exists(), "case {at}: it goes");
    }
    drop(store);
}

/// Failure caught (B1): a sidecar outlives its journal, so a document's pages' digests stay on disk after its edits are gone (saved, discarded or
/// closed), or are read for a later list of the same name. Every path the store removes, replaces or closes a journal by takes its sidecar: retired,
/// replaced by a journal starting again, closed (an append that closes, and a base that closes), and removed as stale.
#[test]
fn a_sidecar_goes_with_its_journal_on_every_path() {
    let name = journal_name();
    type Path_ = fn(&RecordStore, &str);
    let paths_: [(&str, Path_); 5] = [
        ("retired", |store, name| {
            let work = JournalWork {
                retired: vec![name.to_string()],
                ..JournalWork::default()
            };
            assert_eq!(store.write_journals(&work, false).1, None);
        }),
        ("replaced", |store, name| {
            let other = record::journal_file_name(Some(FileStamp { size: 99, ..STAMP }), Path::new("/d/a.pdf"), None);
            let mut start = append(&other, Vec::new(), 2);
            start.base = Some(PageEdits::new(5).to_record());
            start.replaces = Some(name.to_string());
            let work = JournalWork {
                appends: vec![start],
                ..JournalWork::default()
            };
            assert_eq!(store.write_journals(&work, false).1, None);
        }),
        ("closed by an append", |store, name| {
            let mut last = append(name, vec![JournalOp::Undo], 2);
            last.closing = true;
            let work = JournalWork {
                appends: vec![last],
                ..JournalWork::default()
            };
            assert_eq!(store.write_journals(&work, false).1, None);
        }),
        ("closed by a base", |store, name| {
            let mut last = append(name, Vec::new(), 2);
            last.base = Some(PageEdits::new(5).to_record());
            last.closing = true;
            let work = JournalWork {
                appends: vec![last],
                ..JournalWork::default()
            };
            assert_eq!(store.write_journals(&work, false).1, None);
        }),
        ("removed as stale", |store, name| store.remove_journal(name)),
    ];
    for (what, go) in paths_ {
        let (_root, paths) = state();
        let (store, folder) = written(&paths);
        assert!(sidecar_path(&folder).exists(), "{what}: there first");
        go(&store, &name);
        assert!(!sidecar_path(&folder).exists(), "{what}");
    }
}

/// Failure caught (B1): a sidecar is lost when its journal is checkpointed or compacted (as a tab goes dormant, or a journal grows), so a list woken
/// from its base has none of its pages; or a launch's sweep takes the sidecar of a journal that's there. Both keep it, and it reads back.
#[test]
fn a_checkpoint_or_a_compaction_keeps_the_sidecar() {
    let (_root, paths) = state();
    let (store, folder) = written(&paths);
    let (edits, _) = read_list();
    store.write_checkpoint(&journal_name(), &header(), &edits.to_record()).unwrap();
    let _ = store.compact(&journal_name(), &header(), &edits.to_record());
    sweep(&folder);
    let back = store.read_list(&journal_name()).expect("the list");
    assert!(back.edits.has_history());
    assert_eq!(known_of(&back.edits, 3), Some(known(3)), "from the sidecar, though the base names no page");
}

/// Failure caught (B1): a launch leaves sidecars whose journal went a way the store didn't see (set aside as `.not-merged` or `.unreadable`, gone to
/// `closed-`, a crash between a journal's removal and its sidecar's) on disk for good, holding digests of documents no edit is about. A launch takes
/// every sidecar whose journal isn't there, and temp files a crash left, and keeps the others.
#[test]
fn a_launch_takes_every_sidecar_whose_journal_went() {
    let (_root, paths) = state();
    let (store, folder) = written(&paths);
    let gone = name_of("journal-00000000000000aa.log").unwrap();
    std::fs::write(folder.join(&gone), b"x").unwrap();
    std::fs::write(folder.join("pages-00000000000000bb.tmp123"), b"x").unwrap();
    std::fs::write(folder.join("journal-00000000000000cc.not-merged"), b"x").unwrap();
    std::fs::write(folder.join("pages-00000000000000cc.ids"), b"x").unwrap();
    drop(store);
    let fresh = RecordStore::open(&paths, Flavor::App);
    let _ = fresh.load();
    assert!(!folder.join(&gone).exists());
    assert!(!folder.join("pages-00000000000000bb.tmp123").exists());
    assert!(!folder.join("pages-00000000000000cc.ids").exists(), "its journal set aside");
    assert!(folder.join("journal-00000000000000cc.not-merged").exists(), "what was set aside stays");
    assert!(sidecar_path(&folder).exists(), "a sidecar whose journal is there stays");
}

/// Failure caught (the coordinator's condition, 2026-11-15): an app from before, or the release check, takes a sidecar for something of its own: a
/// journal it reads (0.1.0 would read it as a damaged journal and set it aside), a temp file it removes, a history it prunes, or a journal the carry
/// check compares byte for byte. 0.1.0's own patterns, as `git show v0.1.0:crates/recto-session/src/record.rs` and `.../record_store.rs` have them,
/// and `tools/release/carry/check_carry.py`'s, match neither a sidecar's name nor its temp file's.
#[test]
fn nothing_of_0_1_0_or_the_carry_check_takes_a_sidecar_for_its_own() {
    // 0.1.0's `is_journal_name`, word for word.
    fn is_journal_name_0_1_0(name: &str) -> bool {
        let Some(rest) = name.strip_prefix("journal-").and_then(|rest| rest.strip_suffix(".log")) else {
            return false;
        };
        let (key, tab) = rest.split_once('-').map_or((rest, None), |(key, tab)| (key, Some(tab)));
        key.len() == 16
            && key.bytes().all(|byte| byte.is_ascii_hexdigit())
            && tab.is_none_or(|tab| !tab.is_empty() && tab.len() <= 10 && tab.bytes().all(|byte| byte.is_ascii_digit()))
    }
    let journal = journal_name();
    let sidecar_name = name_of(&journal).unwrap();
    let temp = Path::new(&sidecar_name).with_extension("tmp123").display().to_string();
    let tabbed = name_of(&record::journal_file_name(Some(STAMP), Path::new("/d/a.pdf"), Some(7))).unwrap();
    for name in [sidecar_name.as_str(), temp.as_str(), tabbed.as_str()] {
        assert!(!is_journal_name_0_1_0(name), "0.1.0's journals: {name}");
        assert!(
            !name.starts_with("closed-") && !name.starts_with("kept-") && !name.starts_with("sleep-"),
            "0.1.0's prunes: {name}"
        );
        // 0.1.0's `remove_temp_files` prefixes.
        for prefix in ["record.", "journal-", "closed-", "kept-", "sleep-", "places.log"] {
            assert!(!name.starts_with(prefix), "0.1.0's temp files: {name}");
        }
        // check_carry.py: every `startswith("...")` it picks files by.
        for prefix in carry_prefixes() {
            assert!(!name.starts_with(&prefix), "the carry check's {prefix:?}: {name}");
        }
    }
    assert_eq!(
        journal_of(&tabbed).as_deref(),
        Some(record::journal_file_name(Some(STAMP), Path::new("/d/a.pdf"), Some(7)).as_str())
    );
}

/// The prefixes `tools/release/carry/check_carry.py` picks files by, from its source.
fn carry_prefixes() -> Vec<String> {
    let source = include_str!("../../../../../tools/release/carry/check_carry.py");
    let prefixes: Vec<String> = source
        .split("startswith(\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next().map(str::to_string))
        .collect();
    assert!(
        prefixes.iter().any(|prefix| prefix == "journal-"),
        "it still picks journals by their prefix: {prefixes:?}"
    );
    prefixes
}
