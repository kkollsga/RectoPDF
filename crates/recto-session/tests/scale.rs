//! Its own binary with a counting allocator, because other tests running beside it would disturb the count.
#![allow(unsafe_code)]

use recto_core::{DocFacts, OpenError, Toc};
use recto_session::{DocLoader, Session};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// Allocations made, freed or not: what a pin on a path's allocations counts.
static MADE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only adds a counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        MADE.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The counter is process-wide, so tests in this binary measure one at a time. Without this they count each other's
/// allocations and pass or fail by timing.
static MEASURING: Mutex<()> = Mutex::new(());

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

fn live_with(tabs: usize) -> (usize, Session<Fake>) {
    let before = LIVE.load(Ordering::Relaxed);
    let mut session = Session::new(Fake);
    for i in 0..tabs {
        session.open(&path_for(i)).unwrap();
    }
    (LIVE.load(Ordering::Relaxed) - before, session)
}

/// Failure: a per-tab cost creeps in, which a total alone would hide until someone has hundreds of PDFs open.
/// The budget is 4 KiB for one more idle tab, whatever the size of its PDF.
#[test]
fn one_more_idle_tab_costs_at_most_4_kib() {
    let _alone = MEASURING.lock().unwrap();
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
    let _alone = MEASURING.lock().unwrap();
    let (total, session) = live_with(1000);
    println!("heap: 1000 tabs {total} B");
    assert!(total <= 4 << 20, "1000 tabs cost {total} B");
    drop(session);
}

/// Failure: closing tabs leaves memory behind, so a long session keeps growing.
#[test]
fn closing_tabs_returns_the_memory() {
    let _alone = MEASURING.lock().unwrap();
    let mut session = Session::new(Fake);
    session.open(&path_for(0)).unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    for round in 0..50 {
        let ids: Vec<_> = (1..=100).map(|i| session.open(&path_for(round * 1000 + i)).unwrap()).collect();
        for id in ids {
            session.close(id).unwrap();
        }
    }
    let growth = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    println!("heap growth after 5000 open and close cycles: {growth} B");
    assert!(growth < 64 * 1024, "grew {growth} B");
}

/// Failure: a tab costs more than it did, in bytes or in allocations, as it comes back cold from the record: the cost a thousand idle tabs pay. The budgets above have room to spare, so a component of `Session` or of a tab (R3.2) that brought a box or a buffer per tab would pass them; this holds a cold tab at what it cost on 2026-11-13: 304 bytes and 6 allocations a tab restored, and 10 allocations to open one more. Numbers that fall are lowered here; they're never raised to let something through.
#[test]
fn a_cold_tab_costs_no_more_than_it_did() {
    let _alone = MEASURING.lock().unwrap();
    let (_, opened) = live_with(1000);
    let record = opened.to_record();
    drop(opened);
    let mut session = Session::new(Fake);
    let (bytes, made) = (LIVE.load(Ordering::Relaxed), MADE.load(Ordering::Relaxed));
    session.restore(&record, HashMap::new()).unwrap();
    let per_tab_bytes = (LIVE.load(Ordering::Relaxed) - bytes) / 1000;
    let per_tab_made = (MADE.load(Ordering::Relaxed) - made) / 1000;
    println!("restored cold: {per_tab_bytes} B and {per_tab_made} allocations a tab");
    let made = MADE.load(Ordering::Relaxed);
    session.open(&path_for(5000)).unwrap();
    let opening = MADE.load(Ordering::Relaxed) - made;
    println!("opening one more: {opening} allocations");
    assert!(per_tab_bytes <= 304, "a cold tab costs {per_tab_bytes} B, from 304");
    assert!(per_tab_made <= 6, "a cold tab makes {per_tab_made} allocations, from 6");
    assert!(opening <= 10, "opening a tab makes {opening} allocations, from 10");
    drop(session);
}

const SIDEBAR_ALLOCATIONS: usize = 4;
const RECORD_ALLOCATIONS_A_TAB: usize = 4;

/// Allocations made while `during` runs.
fn made_by<T>(during: impl FnOnce() -> T) -> (usize, T) {
    let before = MADE.load(Ordering::Relaxed);
    let out = during();
    (MADE.load(Ordering::Relaxed) - before, out)
}

/// Failure: building the sidebar, which every publish does, allocates for each tab it lists (a notice's words, a name copied, a box per row), so every
/// action at 1,000 tabs pays a thousand allocations for tabs nobody touched. The rows are one list, whatever the tabs; what else is made is the place's words
/// for the tab being read. Pinned at what it makes on 2026-11-13, the same at 100 tabs and at 1,000; the record, which the saver makes on each save of the
/// session's state, is per tab by nature (each tab's path and entry), and is pinned at what it makes a tab.
#[test]
fn the_sidebar_allocates_nothing_a_tab_and_the_record_no_more_a_tab_than_it_did() {
    let _alone = MEASURING.lock().unwrap();
    let mut sidebar_made = Vec::new();
    let mut record_made = Vec::new();
    for tabs in [100, 1000] {
        let (_, session) = live_with(tabs);
        let (made, sidebar) = made_by(|| session.sidebar());
        drop(sidebar);
        sidebar_made.push(made);
        let (made, record) = made_by(|| session.to_record());
        drop(record);
        record_made.push(made / tabs);
        println!(
            "{tabs} tabs: the sidebar made {} allocations, the record {made} ({} a tab)",
            sidebar_made.last().unwrap(),
            made / tabs
        );
        drop(session);
    }
    assert_eq!(
        sidebar_made[0], sidebar_made[1],
        "the sidebar makes {sidebar_made:?} allocations at 100 and 1,000 tabs: something's made for each tab"
    );
    assert!(
        sidebar_made[1] <= SIDEBAR_ALLOCATIONS,
        "the sidebar makes {} allocations, from {SIDEBAR_ALLOCATIONS}",
        sidebar_made[1]
    );
    assert!(
        record_made[1] <= RECORD_ALLOCATIONS_A_TAB,
        "the record makes {} allocations a tab, from {RECORD_ALLOCATIONS_A_TAB}",
        record_made[1]
    );
}
