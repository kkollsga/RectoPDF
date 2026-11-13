//! **What goes to the journals, pinned:** the bytes a scripted run of edits, a refused write, a save, a sleep, a wake, a close and a history taken up
//! sends to each journal file, in order, with how many writes and bytes that is (the owner's "reduce SSD wear as much as we can"), and the rule the
//! journal desk owns: a list's operations reach its journal once and in order, and a retired journal's file goes.

use super::sleep_tests::MemorySource;
use super::*;
use std::fmt::Write as _;
use std::sync::Mutex;

type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// What the state record's store does with the journals' work, byte for byte as `RecordStore::write_journals` writes it, into memory a
/// [`MemorySource`] reads back from: a new file gets its header first, a known one is appended to, a journal starting again replaces whatever's at its
/// name and the one it takes over from goes, and a closing one is renamed `closed-`. Each write is logged, so a test can compare what went where.
struct Disk {
    files: Files,
    /// Every write and removal, as `(name, what, bytes)`, for the step being logged.
    log: Vec<(String, &'static str, Vec<u8>)>,
    /// Appends and replacements so far, and their bytes: the store's `appends` and `wrote_bytes`.
    writes: u64,
    bytes: u64,
    /// The count each file is written up to, for the rule that an append starts where its file ended.
    through: HashMap<String, u64>,
}

impl Disk {
    fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            log: Vec::new(),
            writes: 0,
            bytes: 0,
            through: HashMap::new(),
        }
    }

    /// The store's write of a session's journal work, and what it says back. Panics if an operation would reach a journal twice, or out of order:
    /// an append to a file that's there has to start exactly where that file ended.
    fn write(&mut self, work: &JournalWork) -> JournalOutcome {
        let mut files = self.files.lock().unwrap();
        let mut outcome = JournalOutcome::default();
        for name in &work.retired {
            files.remove(name);
            self.through.remove(name);
            self.log.push((name.clone(), "retire", Vec::new()));
        }
        for append in &work.appends {
            let name = append.name.clone();
            let bytes = if let Some(base) = &append.base {
                let header = JournalHeader {
                    base_seq: append.through,
                    ..append.header.clone()
                };
                let start = record::journal_start(&header, Some(base));
                files.insert(name.clone(), start.clone());
                if let Some(old) = append.replaces.as_ref().filter(|old| **old != name) {
                    files.remove(old);
                    self.through.remove(old);
                    self.log.push((old.clone(), "replaced", Vec::new()));
                }
                self.log.push((name.clone(), "base", start.clone()));
                start
            } else {
                let first = append.through - append.ops.len() as u64;
                let mut lines = Vec::new();
                match self.through.get(&name) {
                    Some(ended) => assert_eq!(
                        first, *ended,
                        "{name}: operations {first}.. sent to a journal written up to {ended}: one reached it twice or one was skipped"
                    ),
                    None => {
                        let header = JournalHeader {
                            base_seq: first,
                            ..append.header.clone()
                        };
                        lines.extend_from_slice(&record::journal_start(&header, None));
                    }
                }
                lines.extend_from_slice(&record::journal_lines(&append.ops));
                files.entry(name.clone()).or_default().extend_from_slice(&lines);
                self.log.push((name.clone(), "append", lines.clone()));
                lines
            };
            self.writes += 1;
            self.bytes += bytes.len() as u64;
            if append.closing {
                self.through.remove(&name);
                if let Some(bytes) = files.remove(&name) {
                    files.insert(record::closed_journal_name(&name).unwrap(), bytes);
                }
            } else {
                self.through.insert(name.clone(), append.through);
            }
            outcome.written.push((append.tab, append.through, name));
        }
        outcome
    }

    /// The store's compaction as a tab goes dormant (`RecordStore::settle_list`): the checkpoint moves up if it comes out well under half, and it's
    /// read back.
    fn settle(&mut self, name: &str, header: &JournalHeader, base: &record::EditsRecord) -> bool {
        let start = record::journal_start(header, Some(base));
        {
            let mut files = self.files.lock().unwrap();
            if files.get(name).is_some_and(|there| start.len() * 2 <= there.len()) {
                files.insert(name.to_string(), start.clone());
                self.through.insert(name.to_string(), header.base_seq);
                self.writes += 1;
                self.bytes += start.len() as u64;
                self.log.push((name.to_string(), "compact", start));
            }
        }
        MemorySource(Arc::clone(&self.files))
            .read_list(name)
            .is_some_and(|back| back.edits.to_record() == *base)
    }

    /// This step's writes, sorted by file (the order between files is a hash map's, on `main` too), each file's own in the order they went.
    fn take_step(&mut self, step: &str, out: &mut String) {
        let mut log = std::mem::take(&mut self.log);
        log.sort_by(|a, b| a.0.cmp(&b.0));
        writeln!(out, "== {step}").unwrap();
        for (name, what, bytes) in log {
            writeln!(out, "{what} {name} {} bytes", bytes.len()).unwrap();
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

/// A journaled session reading its lists back from `disk`.
fn on(disk: &Disk) -> Session<FakeLoader> {
    let mut s = journaled(5);
    s.journals_mut().set_history(Arc::new(MemorySource(Arc::clone(&disk.files))));
    s
}

/// The session's journal work written, and the outcome given back.
fn write(s: &mut Session<FakeLoader>, disk: &mut Disk) {
    let work = s.journal_work();
    let outcome = disk.write(&work);
    s.journal_outcome(outcome);
}

const SAVED: FileStamp = FileStamp {
    size: 9,
    mtime_ns: 1,
    device: 1,
    inode: 2,
};

/// The scripted run, with each step's writes logged.
fn scripted_run() -> String {
    let mut disk = Disk::new();
    let mut s = on(&disk);
    let mut out = String::new();
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.move_pages(0, 1, 2)).unwrap();
    s.edit_pages(b, |e| e.delete(4, 1)).unwrap();
    assert!(s.undo_edit(a));
    assert!(s.redo_edit(a));
    writeln!(out, "has work: {}", s.has_journal_work()).unwrap();
    write(&mut s, &mut disk);
    disk.take_step("edits, undo and redo on two tabs", &mut out);

    // The disk refuses: nothing's said written, and it all goes again, once.
    s.edit_pages_as(b, Author::Agent, |e| e.rotate(0, 2, 3)).unwrap();
    let refused = s.journal_work();
    s.journal_outcome(JournalOutcome {
        written: Vec::new(),
        not_retired: refused.retired.clone(),
    });
    s.edit_pages(b, |e| e.rotate(1, 1, 1)).unwrap();
    write(&mut s, &mut disk);
    disk.take_step("a refused write, then two more steps", &mut out);

    // A save: the journal goes by the new version's name, starting again from the whole list, and the old one goes once it's there.
    let snapshot = s.begin_save(a, false).unwrap().expect("a has unsaved edits");
    s.save_prepared(a, SAVED, None);
    assert_eq!(s.saved(a, SAVED, snapshot.slots.len() as u32, None, None), 0);
    s.end_save(a);
    write(&mut s, &mut disk);
    disk.take_step("a saved", &mut out);
    s.edit_pages(a, |e| e.rotate(2, 1, 2)).unwrap();
    write(&mut s, &mut disk);
    disk.take_step("an edit after the save", &mut out);

    // Sleep: a is shown, and b's list goes to disk, compacted and read back first.
    s.tap(a).unwrap();
    let (name, header, base) = s.list_to_sleep(b).expect("b's list is all in its journal");
    assert!(disk.settle(&name, &header, &base));
    assert!(s.go_dormant(b));
    assert!(s.lists().is_asleep(b));
    write(&mut s, &mut disk);
    disk.take_step("b goes dormant", &mut out);

    // Wake: an edit brings b's list back from its journal, and goes on from where that left off.
    s.edit_pages(b, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(!s.lists().is_asleep(b));
    write(&mut s, &mut disk);
    disk.take_step("b woken by an edit", &mut out);

    // Saved again and closed, with history and nothing unsaved: its journal goes on as a closing one, and its sleep file's retired.
    let snapshot = s.begin_save(a, false).unwrap().expect("a's edit since");
    let again = FileStamp { size: 10, ..SAVED };
    s.save_prepared(a, again, None);
    assert_eq!(s.saved(a, again, snapshot.slots.len() as u32, None, None), 0);
    s.end_save(a);
    s.close(a).unwrap();
    write(&mut s, &mut disk);
    disk.take_step("a closed with its history", &mut out);

    // a opened again picks its history up, which the store has put back at its name.
    let closed = disk.files.lock().unwrap().keys().find(|name| name.starts_with("closed-")).cloned();
    let c = s.open(Path::new("/d/a.pdf")).unwrap();
    if let Some(closed) = closed {
        let name = format!("journal-{}", closed.strip_prefix("closed-").unwrap());
        let bytes = disk.files.lock().unwrap().remove(&closed).unwrap();
        disk.files.lock().unwrap().insert(name.clone(), bytes);
        let back = MemorySource(Arc::clone(&disk.files)).read_list(&name).unwrap();
        disk.through.insert(name.clone(), back.seq);
        s.take_up_history(c, &name, back.edits, back.seq);
        writeln!(out, "taken up: {name}").unwrap();
    }
    s.edit_pages(c, |e| e.rotate(3, 1, 1)).unwrap();
    write(&mut s, &mut disk);
    disk.take_step("a reopened, its history taken up, and edited", &mut out);

    // Discard b: its journal goes.
    s.discard_edits(b);
    write(&mut s, &mut disk);
    disk.take_step("b discarded", &mut out);

    // What's read back by name, and what a session that keeps no journal as it goes writes when told to save.
    let mut lists: Vec<(String, record::EditsRecord)> = s.lists_by_journal().into_iter().collect();
    lists.sort_by(|x, y| x.0.cmp(&y.0));
    writeln!(out, "lists by journal: {lists:?}").unwrap();
    writeln!(out, "has work: {}", s.has_journal_work()).unwrap();
    let mut plain = session(5);
    let p = plain.open(Path::new("/d/p.pdf")).unwrap();
    plain.edit_pages(p, |e| e.rotate(0, 1, 1)).unwrap();
    let mut checkpoints = plain.checkpoints();
    checkpoints.sort_by(|x, y| x.0.cmp(&y.0));
    for (name, header, list) in checkpoints {
        writeln!(out, "checkpoint {name}").unwrap();
        out.push_str(&String::from_utf8_lossy(&record::journal_start(&header, Some(&list))));
    }
    writeln!(out, "writes: {}, bytes: {}", disk.writes, disk.bytes).unwrap();
    let mut left: Vec<String> = disk.files.lock().unwrap().keys().cloned().collect();
    left.sort();
    writeln!(out, "files left: {left:?}").unwrap();
    out
}

/// What the scripted run wrote on `main` before the journal desk (2026-11-13), and must go on writing.
const GOLDEN: &str = include_str!("journal_desk_writes.txt");

/// **Failure: what goes to the journals changes when the code that sends it moves** (the journal desk, R3.5): a byte of a journal is different, a
/// write lands in another file or another order, an operation's sent twice or not at all, a retired file stays, or there are more writes or bytes
/// for the same edits (the owner's "reduce SSD wear as much as we can"). A launch reads these files, so any of that is an edit lost, wrong or
/// doubled after a crash. The whole log is compared, so a failure shows where. `RECTO_JOURNAL_GOLDEN=write` writes it afresh, which is for a change
/// to the journal format, reviewed as one.
#[test]
fn the_journals_get_the_same_bytes_in_the_same_writes() {
    let run = scripted_run();
    assert_eq!(run, scripted_run(), "the run is the same twice, or a golden of it holds nothing");
    if std::env::var("RECTO_JOURNAL_GOLDEN").as_deref() == Ok("write") {
        std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/journal_desk_writes.txt"), &run).unwrap();
        return;
    }
    if run != GOLDEN {
        let first = run.lines().zip(GOLDEN.lines()).position(|(now, then)| now != then);
        panic!("the journals' writes changed, first at line {first:?}.\nNow:\n{run}\nThen:\n{GOLDEN}");
    }
}

/// **Failure: a list's operations reach its journal twice, out of order or not at all, or a retired journal's file stays** (the journal desk's rule).
/// Through refusals, a write that gets one journal and not another, a save, a close and a discard, each append starts where its file ended (the
/// [`Disk`] says so if not), every list reads back from its journal as it is in memory once its work has been written, a name the disk couldn't remove
/// comes back once and a removed one never again, and at the end the only files left are journals an open tab names.
#[test]
fn a_lists_operations_reach_its_journal_once_and_in_order_and_a_retired_journal_goes() {
    let mut disk = Disk::new();
    let mut s = on(&disk);
    let tabs: Vec<TabId> = ["/d/a.pdf", "/d/b.pdf", "/d/c.pdf"]
        .iter()
        .map(|path| s.open(Path::new(path)).unwrap())
        .collect();
    let reads_back = |s: &Session<FakeLoader>, disk: &Disk| {
        for &id in &tabs {
            let (Some(list), Some(name)) = (s.lists().get(id), s.journals().name_of(id)) else {
                continue;
            };
            let back = MemorySource(Arc::clone(&disk.files)).read_list(name).expect("its journal reads");
            assert_eq!(back.edits.to_record(), list.to_record(), "{name} reads back as the list");
            assert_eq!(back.seq, list.journal_seq(), "{name} has every operation, once");
        }
    };
    for round in 0..12u32 {
        for (n, &id) in tabs.iter().enumerate() {
            let n = n as u32;
            if s.tabs().tab(id).is_none() {
                continue;
            }
            match (round + n) % 4 {
                0 => s.edit_pages(id, |e| e.rotate(round % 5, 1, 1)).unwrap(),
                1 => s.edit_pages_as(id, Author::Agent, |e| e.move_pages(0, 1, 3)).unwrap(),
                2 => {
                    s.undo_edit(id);
                }
                _ => {
                    s.redo_edit(id);
                }
            }
        }
        let work = s.journal_work();
        match round % 3 {
            // Refused whole: what was to go goes again, and what was to be removed is asked again, once.
            0 => {
                s.journal_outcome(JournalOutcome {
                    written: Vec::new(),
                    not_retired: work.retired.clone(),
                });
                let again = s.journal_work();
                let mut retired = again.retired.clone();
                retired.sort();
                retired.dedup();
                assert_eq!(retired.len(), again.retired.len(), "a name to remove is asked for once");
                for name in &work.retired {
                    assert!(again.retired.contains(name), "{name}, not removed, is asked again");
                }
                let outcome = disk.write(&again);
                s.journal_outcome(outcome);
            }
            // Only the first journal got there.
            1 => {
                let mut outcome = disk.write(&JournalWork {
                    retired: work.retired.clone(),
                    appends: work.appends.iter().take(1).cloned().collect(),
                });
                outcome.not_retired.clear();
                s.journal_outcome(outcome);
                write(&mut s, &mut disk);
            }
            _ => {
                let outcome = disk.write(&work);
                s.journal_outcome(outcome);
            }
        }
        let after = s.journal_work();
        assert!(after.retired.is_empty(), "a name removed isn't asked for again: {:?}", after.retired);
        assert!(after.appends.is_empty(), "nothing written is sent again");
        reads_back(&s, &disk);
        match round {
            // A save starts the journal again under the file's new name, and the old one goes.
            3 => {
                let id = tabs[0];
                let snapshot = s.begin_save(id, false).unwrap().expect("a has unsaved edits");
                s.saved(id, SAVED, snapshot.slots.len() as u32, None, None);
                s.end_save(id);
                write(&mut s, &mut disk);
            }
            // A close keeps the history in a closing journal, which isn't a tab's any more.
            6 => {
                let id = tabs[1];
                s.discard_edits(id);
                s.close(id).unwrap();
                write(&mut s, &mut disk);
            }
            // A discard: its journal goes.
            9 => {
                s.discard_edits(tabs[2]);
                write(&mut s, &mut disk);
            }
            _ => {}
        }
        reads_back(&s, &disk);
    }
    let named: HashSet<String> = tabs.iter().filter_map(|id| s.journals().name_of(*id).map(String::from)).collect();
    let left: HashSet<String> = disk.files.lock().unwrap().keys().filter(|name| !name.starts_with("closed-")).cloned().collect();
    assert_eq!(left, named, "every journal file left is one a tab has: a retired one went");
}
