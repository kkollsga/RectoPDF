//! **What the UI thread's pulls cost:** `reader()`, `sidebar()` and `sidebar_tabs()` copy what's published, on every model change. Counted here in allocations, which a
//! counting allocator sees exactly, and timed, which is printed for a report and never asserted. Run with `--nocapture` (and `--release` for the times that matter).
#![allow(unsafe_code)]

use recto_app as app;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards to the system allocator, counting each allocation.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's layout, passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: given back as it was given.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// Allocations and nanoseconds per call of `pull`, over `rounds` calls.
fn cost<T>(rounds: u32, pull: impl Fn() -> T) -> (f64, f64) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let started = Instant::now();
    for _ in 0..rounds {
        black_box(pull());
    }
    let nanos = started.elapsed().as_nanos() as f64 / f64::from(rounds);
    let allocations = (ALLOCATIONS.load(Ordering::Relaxed) - before) as f64 / f64::from(rounds);
    (allocations, nanos)
}

fn text(words: &str) -> String {
    words.to_string()
}

/// A reader as a comparison of two documents shows it, with a notice up: every string the record can hold is filled.
fn reader() -> recto_ffi::ReaderView {
    let pane = |tab: u32| recto_ffi::PaneView {
        tab,
        title: text("A document with a long enough name"),
        reading: tab == 1,
        layout: text("single"),
        note: Some(text("This page couldn't be drawn.")),
    };
    recto_ffi::ReaderView {
        panes: vec![pane(1), pane(2)],
        compare_synced: true,
        home: false,
        note: Some(text("The file changed on disk.")),
        sidebar_hidden: false,
        layout: text("single"),
        edited: true,
        can_undo: true,
        can_redo: false,
        edit_mode: false,
        can_rotate: true,
        place: text("12 of 340"),
        paper: text("white"),
        paper_rgb: 0x00ff_ffff,
        agent_icon_hidden: false,
        page_tint: None,
        sidebar_width: 240.0,
        highlight_dark_colour: false,
        highlight_lighten: false,
        selection_notice: Some(recto_ffi::SelectionNotice {
            text: text("Copied 3 pages."),
            action_title: Some(text("Copy anyway")),
            action_id: Some(text("edit.copy_anyway.1")),
            fades: true,
        }),
        saving: false,
        save_tooltip: text("Save the rotated pages"),
        save_problem: None,
        save_as_note: Some(text("The new copy won't be signed.")),
        shown_tab: 1,
        document_name: text("A document with a long enough name"),
        document_folder: text("/Users/someone/Documents/Reading"),
        save_button: true,
        print_ready: false,
        password_prompt: None,
        save_enabled: true,
        can_go_back: true,
        can_go_forward: false,
        link_prompt: None,
        page_text: text("12"),
    }
}

fn row(tab: u32) -> app::SidebarRow {
    app::SidebarRow {
        kind: app::RowKind::Tab,
        tab,
        text: format!("Document number {tab}"),
        page: None,
        depth: 0,
        current: tab == 1,
        rail_filled: false,
        rail_first: false,
        rail_last: false,
        notice: None,
        action: None,
        expandable: false,
        expanded: false,
        in_comparison: false,
    }
}

/// Failure: a pull the UI thread makes on every model change grows past a copy of what's published: the reader allocates more than once per string it holds, the
/// sidebar's view allocates at all beyond its one note, or the tabs' copy at 1,000 tabs allocates more than its list and one name per tab.
#[test]
fn a_pull_costs_a_copy_of_whats_published_and_no_more() {
    let published_reader = reader();
    let (reader_allocations, reader_ns) = cost(10_000, || published_reader.clone());
    // Two panes of three strings and their list, nine strings of its own, and the notice's three.
    assert!(reader_allocations <= 19.0, "reader(): {reader_allocations} allocations a pull");

    let published_sidebar = app::empty_sidebar();
    let (sidebar_allocations, sidebar_ns) = cost(10_000, || published_sidebar.clone());
    assert!(sidebar_allocations <= 1.0, "sidebar(): {sidebar_allocations} allocations a pull");

    let tabs: std::sync::Arc<Vec<app::SidebarRow>> = std::sync::Arc::new((1..=1000).map(row).collect());
    let (tabs_allocations, tabs_ns) = cost(100, || tabs.iter().cloned().collect::<Vec<_>>());
    assert!(
        tabs_allocations <= 1001.0,
        "sidebar_tabs() at 1,000 tabs: {tabs_allocations} allocations a pull"
    );

    println!(
        "pull cost: reader() {reader_allocations} allocations, {reader_ns:.0} ns; sidebar() {sidebar_allocations} allocations, {sidebar_ns:.0} ns; sidebar_tabs() at 1,000 tabs {tabs_allocations} allocations, {:.1} us",
        tabs_ns / 1000.0
    );
}
