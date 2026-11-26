//! What the provenance ledger costs, in its own binary because its counting allocator would be every test's in one it shared. `held_bytes` is a test of the
//! normal suite (what `note_tabs` keeps is capped); `costs` is the measurement behind D2b's report, ignored: `cargo test --release -p recto-control --test
//! ledger_cost -- --ignored --nocapture`.
#![allow(unsafe_code)]

use recto_agent::saving::SaveNote;
use recto_control::record_store::{Flavor, RecordStore};
use recto_control::removal_store::ledger::{note_save, note_tabs, read, running};
use recto_control::state::StatePaths;
use recto_session::record::{PlaceDelta, SessionRecord, TabEntry};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

struct Counting;

thread_local! {
    /// The measuring thread's own live bytes: allocated less freed on it, so a test counts what its calls keep and nothing another thread does meanwhile.
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

// SAFETY: forwards every call to the system allocator unchanged and only adds to a thread-local with a constant initialiser and no destructor.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = LIVE.try_with(|live| live.set(live.get() + layout.size() as isize));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = LIVE.try_with(|live| live.set(live.get() - layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> isize {
    LIVE.with(Cell::get)
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

/// Failure: what `note_tabs` keeps grows with the tabs opened over a session (a set per tab, or a set that is never cleared), against rule 1. Filled to its
/// cap on eight state folders (the most it holds, a ninth pushing the first out), the set holds under 1 MiB in all, counted on the calling thread, which is
/// where it is built; and a tab known already costs it nothing to hold again.
#[test]
fn held_bytes_are_capped() {
    let roots: Vec<_> = (0..9).map(|_| temp()).collect();
    let before = live();
    for (n, root) in roots.iter().enumerate() {
        // 4,096 hashes a root: sixty-four calls of sixty-four new pairs, each waited for (one thread at a time), then the same tabs again.
        for round in 0..64u64 {
            let batch: Vec<TabEntry> = (0..64u64).map(|i| tab(1, "/d/x.pdf", (n as u64 + 1) * 1_000_000 + round * 64 + i)).collect();
            note_tabs(root, &batch);
            let until = Instant::now() + Duration::from_secs(10);
            while running(root) {
                assert!(Instant::now() < until, "the ledger's thread ended");
                std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
            }
        }
    }
    let held = live() - before;
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
            let at = SaveNote::Landed {
                path: &other,
                to: None,
                original: &other,
                backup: None,
            };
            note_save(&backups, &at); // nothing to record: seeds the folder only
        }
        let (mut now, mut to) = (from, from);
        to.inode += 1;
        let notes = |label: &str, note: &SaveNote<'_>| {
            let took = mean(1, || note_save(&backups, note));
            println!("  {label}: {took:?} (first, recorded)");
            let again = mean(20, || note_save(&backups, note));
            println!("  {label}: {again:?} (already there)");
        };
        println!("a save's notes, ledger of {links} links so far:");
        notes("starting", &SaveNote::Starting { path: &file, from: Some(from) });
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
