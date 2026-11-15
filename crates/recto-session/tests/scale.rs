//! Its own binary, because its counting allocator would be every test's in a binary it shared.
#![allow(unsafe_code)]

use recto_core::{DocFacts, OpenError, Toc};
use recto_session::{DocLoader, Session};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

/// One thread's counts.
struct Counts {
    /// Bytes allocated less bytes freed on this thread: what a session built and kept on it holds.
    live: Cell<isize>,
    /// Allocations made, freed or not: what a pin on a path's allocations counts.
    made: Cell<usize>,
    /// Bytes asked for, freed or not: what a pin on how much a path takes at its peak counts from above.
    asked: Cell<usize>,
}

thread_local! {
    /// **The measuring thread's own counts,** so a test counts what its calls do and nothing another thread does meanwhile. The test harness's main
    /// thread allocates right after it starts the test's thread: 5 allocations, seen here on 2026-11-15 by logging which thread made each (its table
    /// of running tests and its queue of time-outs, which `--test-threads=1` doesn't make, then 3 for its first wait for the result). It's done in
    /// microseconds, before the test's thread starts, unless the machine is busy: on CI run 43682616724, a 3-core runner near the end of a full run,
    /// they fell in the sidebar's first measurement, which a process-wide count then made 9 for 4.
    static HERE: Counts = const { Counts { live: Cell::new(0), made: Cell::new(0), asked: Cell::new(0) } };
}

/// Allocations made by every thread: only to say, when a count fails, what other threads made in the same window.
static MADE_ANYWHERE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only adds to counters: an atomic, and a thread-local with a constant initialiser and
// no destructor, which is never registered or torn down, so reading it doesn't allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        MADE_ANYWHERE.fetch_add(1, Ordering::Relaxed);
        // `try_with`, as an allocator mustn't panic; with no destructor the counts are never gone, so it never fails.
        let _ = HERE.try_with(|here| {
            here.live.set(here.live.get() + layout.size() as isize);
            here.made.set(here.made.get() + 1);
            here.asked.set(here.asked.get() + layout.size());
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = HERE.try_with(|here| here.live.set(here.live.get() - layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The bytes this thread holds.
fn live() -> isize {
    HERE.with(|here| here.live.get())
}

/// The bytes this thread has asked for.
fn asked() -> usize {
    HERE.with(|here| here.asked.get())
}

/// What a window made: on this thread, which is what's measured, and on others meanwhile, which is said when a count fails.
#[derive(Clone, Copy, Debug)]
struct Made {
    here: usize,
    elsewhere: usize,
}

/// Allocations made while `during` runs.
fn made_by<T>(during: impl FnOnce() -> T) -> (Made, T) {
    let (here, anywhere) = (HERE.with(|here| here.made.get()), MADE_ANYWHERE.load(Ordering::Relaxed));
    let out = during();
    let here = HERE.with(|counts| counts.made.get()) - here;
    (
        Made {
            here,
            elsewhere: (MADE_ANYWHERE.load(Ordering::Relaxed) - anywhere).saturating_sub(here),
        },
        out,
    )
}

struct Fake;

impl DocLoader for Fake {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 1000,
            byte_len: 1 << 30,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

fn path_for(i: usize) -> PathBuf {
    PathBuf::from(format!("/Users/someone/Documents/Reports/2026/A long but ordinary report name {i:05}.pdf"))
}

/// A session of `tabs` tabs, and the bytes it holds.
fn live_with(tabs: usize) -> (usize, Session<Fake>) {
    let before = live();
    let mut session = Session::new(Fake);
    for i in 0..tabs {
        session.open(&path_for(i)).unwrap();
    }
    (usize::try_from(live() - before).expect("a session holds what it's made"), session)
}

/// Failure: a per-tab cost creeps in, which a total alone would hide until someone has hundreds of PDFs open.
/// The budget is 4 KiB for one more idle tab, whatever the size of its PDF.
#[test]
fn one_more_idle_tab_costs_at_most_4_kib() {
    let (two, keep_two) = live_with(2);
    let (many, keep_many) = live_with(202);
    let per_tab = many.saturating_sub(two) / 200;
    println!("heap: 2 tabs {two} B, 202 tabs {many} B, one more tab {per_tab} B");
    assert!(per_tab <= 4096, "one more idle tab costs {per_tab} B");
    drop((keep_two, keep_many));
}

/// Failure: 1,000 tabs cost more than the budget in total.
#[test]
fn a_thousand_tabs_fit_in_4_mib() {
    let (total, session) = live_with(1000);
    println!("heap: 1000 tabs {total} B");
    assert!(total <= 4 << 20, "1000 tabs cost {total} B");
    drop(session);
}

/// Failure: closing tabs leaves memory behind, so a long session keeps growing.
#[test]
fn closing_tabs_returns_the_memory() {
    let mut session = Session::new(Fake);
    session.open(&path_for(0)).unwrap();
    let baseline = live();
    for round in 0..50 {
        let ids: Vec<_> = (1..=100).map(|i| session.open(&path_for(round * 1000 + i)).unwrap()).collect();
        for id in ids {
            session.close(id).unwrap();
        }
    }
    let growth = (live() - baseline).max(0);
    println!("heap growth after 5000 open and close cycles: {growth} B");
    assert!(growth < 64 * 1024, "grew {growth} B");
}

/// Failure: a tab costs more than it did, in bytes or in allocations, as it comes back cold from the record: the cost a thousand idle tabs pay. The budgets above have room to spare, so a component of `Session` or of a tab (R3.2) that brought a box or a buffer per tab would pass them; this holds a cold tab at what it cost on 2026-11-13: 304 bytes and 6 allocations a tab restored, and 10 allocations to open one more. Numbers that fall are lowered here; they're never raised to let something through.
#[test]
fn a_cold_tab_costs_no_more_than_it_did() {
    let (_, opened) = live_with(1000);
    let record = opened.to_record();
    drop(opened);
    let mut session = Session::new(Fake);
    let bytes = live();
    let (restoring, restored) = made_by(|| session.restore(&record, HashMap::new()));
    restored.unwrap();
    let per_tab_bytes = (live() - bytes) / 1000;
    let per_tab_made = restoring.here / 1000;
    println!(
        "restored cold: {per_tab_bytes} B and {per_tab_made} allocations a tab ({} by other threads meanwhile)",
        restoring.elsewhere
    );
    let (opening, opened) = made_by(|| session.open(&path_for(5000)));
    opened.unwrap();
    println!(
        "opening one more: {} allocations ({} by other threads meanwhile)",
        opening.here, opening.elsewhere
    );
    let opening = opening.here;
    assert!(per_tab_bytes <= 304, "a cold tab costs {per_tab_bytes} B, from 304");
    assert!(per_tab_made <= 6, "a cold tab makes {per_tab_made} allocations, from 6");
    assert!(opening <= 10, "opening a tab makes {opening} allocations, from 10");
    drop(session);
}

const SIDEBAR_ALLOCATIONS: usize = 4;
const RECORD_ALLOCATIONS_A_TAB: usize = 4;

/// Failure: building the sidebar, which every publish does, allocates for each tab it lists (a notice's words, a name copied, a box per row), so every
/// action at 1,000 tabs pays a thousand allocations for tabs nobody touched. The rows are one list, whatever the tabs; what else is made is the place's words
/// for the tab being read. Pinned at what it makes on 2026-11-13, the same at 100 tabs and at 1,000; the record, which the saver makes on each save of the
/// session's state, is per tab by nature (each tab's path and entry), and is pinned at what it makes a tab.
///
/// Each count is the measuring thread's alone (see [`HERE`]): the sidebar is built on the caller's thread, so whatever it makes is counted, and what the
/// harness's thread makes meanwhile isn't. When the two sizes differ, the failure says which made more, by how much, and what other threads made then.
#[test]
fn the_sidebar_allocates_nothing_a_tab_and_the_record_no_more_a_tab_than_it_did() {
    let mut sidebar_made = Vec::new();
    let mut record_made = Vec::new();
    for tabs in [100, 1000] {
        let (_, session) = live_with(tabs);
        let (made, sidebar) = made_by(|| session.sidebar());
        drop(sidebar);
        sidebar_made.push(made);
        let (made, record) = made_by(|| session.to_record());
        drop(record);
        record_made.push(made.here / tabs);
        println!(
            "{tabs} tabs: the sidebar made {} allocations ({} by other threads meanwhile), the record {} ({} a tab; {} by other threads)",
            sidebar_made.last().unwrap().here,
            sidebar_made.last().unwrap().elsewhere,
            made.here,
            made.here / tabs,
            made.elsewhere
        );
        drop(session);
    }
    let [few, many] = [sidebar_made[0], sidebar_made[1]];
    assert!(
        few.here == many.here,
        "the sidebar made {} allocations at 100 tabs and {} at 1,000 (other threads made {} and {} meanwhile, not counted): {}",
        few.here,
        many.here,
        few.elsewhere,
        many.elsewhere,
        if many.here > few.here {
            format!("{} more at 1,000 tabs: something's made for each tab", many.here - few.here)
        } else {
            format!(
                "{} more at 100 tabs, the first measured: something's made once, the first time the sidebar is built",
                few.here - many.here
            )
        }
    );
    assert!(
        many.here <= SIDEBAR_ALLOCATIONS,
        "the sidebar makes {} allocations, from {SIDEBAR_ALLOCATIONS}",
        many.here
    );
    assert!(
        record_made[1] <= RECORD_ALLOCATIONS_A_TAB,
        "the record makes {} allocations a tab, from {RECORD_ALLOCATIONS_A_TAB}",
        record_made[1]
    );
}

/// Failure (the third review, D-C; rules 1 and 6): a key a tab entry in the record doesn't have is kept whole while the entry is read, as a flattened
/// field has serde buffer every key it doesn't know, so a forged record grows in memory by many times what it says before any cap is met. A tab entry
/// with a key of its own holding 1 MiB of numbers is read past: it asks for no more than 64 KiB more than the same record without it.
#[test]
fn an_unknown_key_in_a_tab_entry_is_read_past_and_not_kept() {
    let (_, session) = live_with(1);
    let plain = serde_json::to_string(&session.to_record()).unwrap();
    // A list of half a million numbers, 1 MiB of text: kept, each is a value of serde's own, tens of bytes.
    let forged = plain.replacen("\"page_count\":", &format!("\"forged\":[{}0],\"page_count\":", "0,".repeat(1 << 19)), 1);
    assert_ne!(plain, forged, "the forged key went in a tab entry");
    let asked = |body: &str| {
        let before = asked();
        let read: recto_session::record::SessionRecord = serde_json::from_str(body).expect("it reads");
        let asked = asked() - before;
        drop(read);
        asked
    };
    let (plain_asked, forged_asked) = (asked(&plain), asked(&forged));
    assert!(
        forged_asked <= plain_asked + 64 * 1024,
        "a tab entry with a 1 MiB key of its own asked for {forged_asked} bytes, against {plain_asked} without it"
    );
}
