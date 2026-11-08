//! Its own binary with a counting allocator, because other tests running beside it would disturb the count.
#![allow(unsafe_code)]

use recto_core::{DocFacts, OpenError, Toc};
use recto_session::{DocLoader, Session};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only adds a counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
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
