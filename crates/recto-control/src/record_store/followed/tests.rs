//! A follow's crash points (the page matcher, slice 2), as the disk is left at each, then a launch; and the journals the rule must leave as `main` does.
//! The follow's order (`recto_session::version_change::followed`, then `write_journals`): the fresh journal for the new version (a whole list under the new
//! version's name), the old journal removed, the new journal's sidecar, and the record later. A crash can stop between any two.

use super::super::*;
use recto_session::edit::identity::{self, Known};
use recto_session::record::{AuthorName, FileRecord, PathField, SessionRecord, TabEntry};
use recto_session::{JournalAppend, PagesAppend, TabId};

fn state() -> (recto_testpdf::TempDir, StatePaths) {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "rsf");
    let paths = StatePaths::overridden(&root).unwrap();
    paths.prepare().unwrap();
    (root, paths)
}

fn rotation() -> JournalOp {
    JournalOp::Rotate(0, 1, 1, AuthorName::Owner)
}

fn tab(id: u32, file: &Path, stamp: FileStamp, journal: &str) -> TabEntry {
    TabEntry {
        id,
        path: PathField::from_path(file),
        file: Some(FileRecord::from(stamp)),
        page: 0,
        page_count: 5,
        place: None,
        view_rotation: 0,
        toc_open: true,
        layout: Default::default(),
        renderer_only: false,
        selection: None,
        notice: None,
        journal: Some(journal.to_string()),
        edits: None,
        generated_contents: false,
        contents_folds: Vec::new(),
        back: Vec::new(),
        forward: Vec::new(),
        password: false,
    }
}

fn session(tabs: Vec<TabEntry>) -> SessionRecord {
    SessionRecord {
        app: Default::default(),
        view: Default::default(),
        find: Default::default(),
        edit_mode: false,
        active: tabs.first().map(|tab| tab.id),
        next_id: tabs.iter().map(|tab| tab.id).max().unwrap_or(0) + 1,
        tabs,
        comparison: None,
        recent: Vec::new(),
        withheld_places: Default::default(),
    }
}

/// A journal of one turn for tab `tab` about `stamp` of `file`, as an edit leaves it (appended), under its version's name.
fn edited_journal(store: &RecordStore, tab: u32, file: &Path, stamp: FileStamp) -> String {
    let name = record::journal_file_name(Some(stamp), file, None);
    let work = JournalWork {
        pages: Vec::new(),
        retired: Vec::new(),
        appends: vec![JournalAppend {
            tab: TabId(tab),
            name: name.clone(),
            header: JournalHeader::new(tab, file, Some(stamp), 5, 0),
            through: 1,
            ops: vec![rotation()],
            closing: false,
            base: None,
            replaces: None,
        }],
    };
    assert!(store.write_journals(&work, false).1.is_none());
    name
}

/// The followed list: its turn on the new version, its pages known there, and "not yet told".
fn followed_list() -> PageEdits {
    let (mut list, _) = PageEdits::replay(5, None, &[rotation()]).unwrap();
    let known = Known {
        digest: [7; 16],
        object: (3, 0),
        frame: 0,
    };
    identity::followed(&mut list, &[(0, known)], None);
    identity::set_followed_untold(&mut list, true);
    list
}

/// The follow's journal work: the fresh journal for `to` (taking over from `old` unless `old_stays`), with the new journal's sidecar if `sidecar`.
fn follow_work(tab: u32, file: &Path, to: FileStamp, old: &str, old_stays: bool, sidecar: bool, seq: u64) -> (JournalWork, String) {
    let name = record::journal_file_name(Some(to), file, None);
    let list = followed_list();
    let pages = if sidecar {
        let mut header = identity::sidecar::Header::new(&name, to, 5);
        header.followed_untold = true;
        vec![PagesAppend {
            tab: TabId(tab),
            journal: name.clone(),
            header,
            waiting: identity::sidecar_waiting(&list).expect("waiting"),
        }]
    } else {
        Vec::new()
    };
    let work = JournalWork {
        pages,
        retired: Vec::new(),
        appends: vec![JournalAppend {
            tab: TabId(tab),
            name: name.clone(),
            header: JournalHeader::new(tab, file, Some(to), 5, 0),
            through: seq,
            ops: Vec::new(),
            closing: false,
            base: Some(list.to_record()),
            replaces: (!old_stays).then(|| old.to_string()),
        }],
    };
    (work, name)
}

/// A file on disk, its first version, and the other program's.
struct Bed {
    root: recto_testpdf::TempDir,
    paths: StatePaths,
    file: PathBuf,
    old_stamp: FileStamp,
}

impl Bed {
    fn new() -> Self {
        let (root, paths) = state();
        let file = root.join("doc.pdf");
        std::fs::write(&file, b"the version the edits were made on").unwrap();
        let old_stamp = file_stamp(&file).unwrap();
        Self { root, paths, file, old_stamp }
    }

    /// Another program puts a new version in place, by a rename (another inode).
    fn replaced(&self, bytes: &[u8]) -> FileStamp {
        let new = self.file.with_extension("new");
        std::fs::write(&new, bytes).unwrap();
        std::fs::rename(&new, &self.file).unwrap();
        file_stamp(&self.file).unwrap()
    }

    fn journals(&self) -> PathBuf {
        self.root.join("session").join(JOURNALS)
    }

    fn load(&self) -> Restored {
        let store = RecordStore::open(&self.paths, Flavor::App);
        match store.load() {
            Loaded::Restored(restored) => *restored,
            other => panic!("{other:?}"),
        }
    }
}

/// The edits' journal and the record naming it, the file then changed by another program; the store and the old journal's name.
fn before_the_follow(bed: &Bed) -> (RecordStore, String, FileStamp) {
    let store = RecordStore::open(&bed.paths, Flavor::App);
    let old = edited_journal(&store, 1, &bed.file, bed.old_stamp);
    store
        .write_record(&session(vec![tab(1, &bed.file, bed.old_stamp, &old)]), &PlaceDelta::default(), false)
        .unwrap();
    let to = bed.replaced(b"another program's version, longer than the first");
    (store, old, to)
}

/// **Failure caught (the follow's crash-safe order, 2026-11-15), at each point a crash can stop it:** the next launch brings back a mix (two tabs on one
/// file, the old list on the new version, the new list on the old), or loses the edits. Before the new journal: the old list, about the old version
/// (held by the session as before). With both journals (the old not yet removed): the new list, the old journal set aside. With the old removed and
/// the sidecar not yet written: the new list, whole, with nothing known of its pages. With the sidecar too: the new list, its pages known and "not yet
/// told". The record, never written after the follow in any of them, names the old journal.
#[test]
fn a_crash_at_each_point_of_a_follow_leaves_the_old_list_or_the_new_one_whole() {
    for point in ["before the new journal", "both journals", "old removed", "sidecar written"] {
        let bed = Bed::new();
        let (store, old, to) = before_the_follow(&bed);
        let new = match point {
            "before the new journal" => None,
            "both journals" => {
                let (work, new) = follow_work(1, &bed.file, to, &old, true, false, 2);
                assert!(store.write_journals(&work, false).1.is_none());
                Some(new)
            }
            "old removed" | "sidecar written" => {
                let (work, new) = follow_work(1, &bed.file, to, &old, false, point == "sidecar written", 2);
                assert!(store.write_journals(&work, false).1.is_none());
                Some(new)
            }
            _ => unreachable!(),
        };
        drop(store);
        let restored = bed.load();
        assert_eq!(restored.record.tabs.len(), 1, "{point}: one tab on the file");
        assert_eq!(restored.journals.len(), 1, "{point}: one list");
        let tab = &restored.record.tabs[0];
        match &new {
            None => {
                assert_eq!(tab.journal.as_deref(), Some(old.as_str()), "{point}");
                assert_eq!(tab.file, Some(FileRecord::from(bed.old_stamp)), "{point}: the old list, about the old version");
            }
            Some(new) => {
                assert_eq!(tab.journal.as_deref(), Some(new.as_str()), "{point}: the new list");
                assert_eq!(tab.file, Some(FileRecord::from(to)), "{point}: about the new version");
                let list = &restored.journals[new].edits;
                assert!(list.is_edited(), "{point}: whole");
                let sidecar = point == "sidecar written";
                assert_eq!(identity::followed_untold(list), sidecar, "{point}: not yet told, from the sidecar");
                assert_eq!(identity::identities(list).is_some(), sidecar, "{point}: its pages known only from a sidecar");
                assert!(!bed.journals().join(&old).exists(), "{point}: the old journal isn't a journal any more");
            }
        }
        if point == "both journals" {
            assert!(bed.journals().join(&old).with_extension("not-merged").exists(), "set aside, not removed");
        }
    }
}

/// Failure caught (slice 1's review, for slice 2): the new journal written with the old version's sidecar still at its name, its pages taken for the new
/// version's. A sidecar whose header is another version's is stale: dropped, the list whole, its pages read again.
#[test]
fn a_new_journal_beside_the_old_versions_sidecar_drops_the_sidecar() {
    let bed = Bed::new();
    let (store, old, to) = before_the_follow(&bed);
    let (work, new) = follow_work(1, &bed.file, to, &old, false, false, 2);
    assert!(store.write_journals(&work, false).1.is_none());
    // A sidecar of the old version's, under the new journal's name.
    let list = followed_list();
    let stale = identity::sidecar::Header::new(&new, bed.old_stamp, 5);
    let mut bytes = stale.encode().unwrap();
    bytes.extend_from_slice(&identity::sidecar_waiting(&list).unwrap().bytes);
    let sidecar = bed.journals().join(page_ids::name_of(&new).unwrap());
    std::fs::write(&sidecar, bytes).unwrap();
    drop(store);
    let restored = bed.load();
    let list = &restored.journals[&new].edits;
    assert!(list.is_edited());
    assert!(
        identity::identities(list).is_none() && !identity::followed_untold(list),
        "nothing of the stale sidecar taken"
    );
    assert!(!sidecar.exists(), "and it goes");
}

/// **Pinned before the rule, as `main` handles it:** two journals for one path that aren't linked (another tab's id in the second's header): the
/// second is restored as a tab of its own, as on `main`. The rule fires only on the tab's own id.
#[test]
fn two_journals_for_one_path_without_the_tabs_id_are_as_on_main() {
    let bed = Bed::new();
    let (store, old, to) = before_the_follow(&bed);
    let (work, new) = follow_work(7, &bed.file, to, &old, true, false, 2);
    assert!(store.write_journals(&work, false).1.is_none());
    drop(store);
    let restored = bed.load();
    let journals: Vec<Option<&str>> = restored.record.tabs.iter().map(|tab| tab.journal.as_deref()).collect();
    assert_eq!(
        journals,
        vec![Some(old.as_str()), Some(new.as_str())],
        "the tab keeps its own, and the other comes back beside it"
    );
    assert!(bed.journals().join(&old).exists() && bed.journals().join(&new).exists());
}

/// Failure caught: the tab's own id and path on a journal about a version that isn't the file now (or older than the tab's list) taken over the tab's
/// list. The tab keeps its own, held; the other is set aside, never a second tab on the file.
#[test]
fn a_linked_journal_about_another_version_than_now_is_set_aside() {
    for why in ["about a third version", "older than the tab's list"] {
        let bed = Bed::new();
        let (store, old, to) = before_the_follow(&bed);
        let (about, seq) = match why {
            "about a third version" => (FileStamp { size: to.size + 1, ..to }, 2),
            _ => (to, 0),
        };
        let (work, new) = follow_work(1, &bed.file, about, &old, true, false, seq);
        assert!(store.write_journals(&work, false).1.is_none());
        drop(store);
        let restored = bed.load();
        assert_eq!(restored.record.tabs.len(), 1, "{why}");
        assert_eq!(restored.record.tabs[0].journal.as_deref(), Some(old.as_str()), "{why}: the tab's own list");
        assert!(!restored.journals.contains_key(&new), "{why}");
        assert!(bed.journals().join(&new).with_extension("not-merged").exists(), "{why}: set aside");
    }
}

/// Failure caught: a forged journal with another tab's id, about this file as it is now, takes that tab's list. Its path isn't that tab's, so nothing
/// is taken from it.
#[test]
fn a_forged_journal_with_another_tabs_id_takes_nothing_from_it() {
    let bed = Bed::new();
    let store = RecordStore::open(&bed.paths, Flavor::App);
    let other = bed.root.join("other.pdf");
    std::fs::write(&other, b"another document").unwrap();
    let other_stamp = file_stamp(&other).unwrap();
    let theirs = edited_journal(&store, 2, &other, other_stamp);
    let old = edited_journal(&store, 1, &bed.file, bed.old_stamp);
    let record = session(vec![tab(1, &bed.file, bed.old_stamp, &old), tab(2, &other, other_stamp, &theirs)]);
    store.write_record(&record, &PlaceDelta::default(), false).unwrap();
    let to = bed.replaced(b"another program's version, longer than the first");
    // Tab 2's id, on a journal about doc.pdf as it is now.
    let (work, forged) = follow_work(2, &bed.file, to, &old, true, false, 9);
    assert!(store.write_journals(&work, false).1.is_none());
    drop(store);
    let restored = bed.load();
    let two = restored.record.tabs.iter().find(|tab| tab.id == 2).expect("tab 2");
    assert_eq!(two.journal.as_deref(), Some(theirs.as_str()), "tab 2 keeps its own");
    assert!(restored.journals.contains_key(&theirs));
    let one = restored.record.tabs.iter().find(|tab| tab.id == 1).expect("tab 1");
    assert_eq!(one.journal.as_deref(), Some(old.as_str()), "tab 1 too: the forged journal isn't its id");
    let _ = forged;
}

/// Failure caught (the coordinator's check 3: ids are the record's, so a leftover journal can carry a tab's id and path): an old leftover with the
/// tab's id and path whose file happens to be the file now displaces the journal the tab rightly claimed, which is about the file now too. The rule
/// fires only when the claimed journal isn't about the file now: the tab keeps its own, and the leftover is set aside.
#[test]
fn a_leftover_with_the_tabs_id_never_displaces_a_journal_about_the_file_now() {
    let bed = Bed::new();
    let store = RecordStore::open(&bed.paths, Flavor::App);
    let own = edited_journal(&store, 1, &bed.file, bed.old_stamp);
    store
        .write_record(&session(vec![tab(1, &bed.file, bed.old_stamp, &own)]), &PlaceDelta::default(), false)
        .unwrap();
    // A leftover under another name, about this file as it is, newer by its steps.
    let leftover = record::journal_file_name(Some(bed.old_stamp), &bed.file, Some(3));
    let (mut work, _) = follow_work(1, &bed.file, bed.old_stamp, &own, true, false, 9);
    work.appends[0].name = leftover.clone();
    assert!(store.write_journals(&work, false).1.is_none());
    drop(store);
    let restored = bed.load();
    assert_eq!(restored.record.tabs.len(), 1);
    assert_eq!(
        restored.record.tabs[0].journal.as_deref(),
        Some(own.as_str()),
        "the tab keeps the journal it claimed"
    );
    assert!(bed.journals().join(&leftover).with_extension("not-merged").exists(), "the leftover set aside");
}
