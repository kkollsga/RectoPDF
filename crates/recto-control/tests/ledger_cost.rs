//! What the provenance ledger costs, in its own binary because its counting allocator would be every test's in one it shared. `held_bytes` is a test of the
//! normal suite (what `note_tabs` keeps is capped); `costs` is the measurement behind D2b's report, ignored: `cargo test --release -p recto-control --test
//! ledger_cost -- --ignored --nocapture`.
#![allow(unsafe_code)]

use recto_agent::saving::SaveNote;
use recto_control::record_store::{Flavor, RecordStore};
use recto_control::removal_store::ledger::{finish, note_save, note_tabs, read, running, take_new};
use recto_control::state::StatePaths;
use recto_session::record::{PlaceDelta, SessionRecord, TabEntry};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

struct Counting;

/// Live bytes of the whole process: allocated less freed, by any thread. (One thread's own count would miss a block made here and freed on the ledger's
/// thread.) Only this binary's one measuring test runs at a time, with `--test-threads=1` for the measurement.
static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only adds to an atomic.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> isize {
    LIVE.load(Ordering::Relaxed)
}

fn temp() -> recto_testpdf::TempDir {
    recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-ledger-cost-")
}

fn tab(id: u32, path: &str, inode: u64) -> TabEntry {
    serde_json::from_value(serde_json::json!({
        "id": id, "path": path, "page": 1, "page_count": 5,
        "file": { "size": 100, "mtime_ns": 1, "device": 1, "inode": inode }
    }))
    .unwrap()
}

fn tabs(n: u32, salt: u64) -> Vec<TabEntry> {
    (1..=n).map(|id| tab(id, &format!("/d/{id}.pdf"), salt * 1_000_000 + u64::from(id))).collect()
}

/// Failure: what `note_tabs` keeps grows with the tabs opened over a session (a set that is never cleared when full), against rule 1. Nine state folders each
/// take 200 calls of 64 new pairs (12,800 hashes a root, three times the set's cap, so it is cleared several times): what is held in all (live bytes of the
/// process, the set being all that is kept) stays under 1 MiB. Without the clearing it is about 1.2 MiB. (That at most 8 roots are held is
/// `roots_are_held_only_with_something_to_note_and_never_while_running`'s; it can't be told from bytes.)
#[test]
fn held_bytes_are_capped() {
    let roots: Vec<_> = (0..9).map(|_| temp()).collect();
    let template = tab(1, "/d/x.pdf", 0);
    let before = live();
    for (n, root) in roots.iter().enumerate() {
        let mut batch: Vec<TabEntry> = vec![template.clone(); 64];
        for round in 0..200u64 {
            for (i, tab) in batch.iter_mut().enumerate() {
                tab.file.as_mut().unwrap().inode = (n as u64 + 1) * 10_000_000 + round * 64 + i as u64;
            }
            assert_eq!(take_new(root, &batch).len(), 64, "root {n}, round {round}");
            finish(root);
        }
    }
    let held = live() - before;
    println!("held by the seen sets, 9 roots of 12,800 pairs offered: {held} bytes");
    assert!(held < 1 << 20, "held {held} bytes");
}

fn pdf() -> Vec<u8> {
    recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["words"]))])
}

fn mean(runs: u32, mut one: impl FnMut()) -> Duration {
    let began = Instant::now();
    for _ in 0..runs {
        one();
    }
    began.elapsed() / runs
}

/// The numbers behind D2b's report. Release build; the load is the machine's.
#[test]
#[ignore = "a measurement"]
fn costs() {
    // `note_tabs` at 1,000 known tabs, against `write_record` at 1,000 tabs (whole records, one changing each time).
    let dir = temp();
    let paths = StatePaths::overridden(&dir.join("state")).unwrap();
    paths.prepare().unwrap();
    let thousand = tabs(1_000, 1);
    note_tabs(&paths.root, &thousand);
    while running(&paths.root) {
        std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
    }
    // Sixty-four new pairs a call go to the thread: until every one is known (a call that starts no thread).
    loop {
        note_tabs(&paths.root, &thousand);
        let started = running(&paths.root);
        while running(&paths.root) {
            std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
        }
        if !started {
            break;
        }
    }
    let known = mean(2_000, || note_tabs(&paths.root, &thousand));
    println!("note_tabs, 1000 known tabs: {known:?} a call");
    let store = RecordStore::open(&paths, Flavor::App);
    let mut record: SessionRecord = serde_json::from_value(serde_json::json!({ "next_id": 1001, "tabs": [] })).unwrap();
    record.tabs = thousand.clone();
    let mut flip = false;
    let write = mean(50, || {
        flip = !flip;
        record.edit_mode = flip;
        store.write_record(&record, &PlaceDelta::default(), false).unwrap();
    });
    println!("write_record, 1000 tabs, whole, with the ledger's call in it: {write:?} a call");

    // What the three notes add to one save of an ordinary file, over a new ledger and over one holding 4,000 links.
    let file = dir.join("doc.pdf");
    std::fs::write(&file, pdf()).unwrap();
    let from = recto_platform::file_stamp(&file).unwrap();
    for links in [0u64, 4_000] {
        let state = dir.join(format!("save-{links}"));
        let backups = state.join("backups.noindex");
        for n in 0..links {
            let other = dir.join(format!("o{n}.pdf"));
            std::fs::write(&other, b"x").unwrap();
            note_save(
                &backups,
                &SaveNote::Starting {
                    path: &other,
                    from: None,
                    destination: None,
                },
            );
        }
        let (mut now, mut to) = (from, from);
        to.inode += 1;
        let notes = |label: &str, note: &SaveNote<'_>| {
            let took = mean(1, || note_save(&backups, note));
            println!("  {label}: {took:?} (first, recorded)");
            let again = mean(20, || note_save(&backups, note));
            println!("  {label}: {again:?} (already there)");
        };
        let seeded = read(&state);
        println!("a save's notes, ledger of {} links so far (gap {:?}):", seeded.links.len(), seeded.gap);
        notes(
            "starting",
            &SaveNote::Starting {
                path: &file,
                from: Some(from),
                destination: None,
            },
        );
        notes(
            "prepared",
            &SaveNote::Prepared {
                path: &file,
                from: Some(from),
                to,
                over_itself: true,
            },
        );
        now.inode = from.inode;
        notes(
            "landed",
            &SaveNote::Landed {
                path: &file,
                to: Some(now),
                original: &file,
                backup: None,
            },
        );
        println!("  the ledger reads {} links", read(&state).links.len());
    }
}
