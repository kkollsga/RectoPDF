//! **What a person's action allocates on the window's thread, counted** (R3.7, a guard put in before `Inner`'s moves; AGENTS.md, "guards go in before a
//! move"). In a test binary of its own because counting needs a global allocator, and one installed in `recto-app`'s test binary would put its count in
//! every time measured there, the input-to-frame harness's included, which every branch is judged by.
#![allow(unsafe_code)]

use recto_app::RowKind;
use recto_app::core::controls::UiAction;
use recto_app::testkit::headless::{Disk, HeadlessApp};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The headless tests' own test files: a file with contents.
#[allow(dead_code)]
#[path = "../src/core/headless/files.rs"]
mod files;

struct Counting;

thread_local! {
    /// This thread's allocations: the test's own, the window's in the app, and not the session thread's or a worker's.
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

// SAFETY: forwards to the system allocator, counting each allocation on the thread that makes it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // A thread on its way out has no counter any more; what it allocates isn't an action's.
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        // SAFETY: the caller's layout, passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: given back as it was given.
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Every allocation of this binary goes through it: one thread-local add each. This binary holds this test alone, so no time is measured under it.
#[global_allocator]
static COUNTING: Counting = Counting;

/// The most allocations the window's call made, of `runs` calls alternating between two forms, each after the app has settled. The listener's record is
/// emptied with room kept before each call, so what it records, from whichever thread and however many calls came before, allocates nothing: the count
/// is the app's alone.
fn most_allocated(app: &HeadlessApp, runs: usize, action: impl Fn(bool) -> UiAction) -> u64 {
    let mut most = 0;
    for run in 0..runs {
        let made = action(run % 2 == 0);
        app.settle();
        app.listener().forget_keeping_room(LISTENER_ROOM);
        let before = allocations();
        app.act(made);
        most = most.max(allocations() - before);
    }
    app.settle();
    most
}

/// More of the Listener's calls than one action and what follows it make before the next settle.
const LISTENER_ROOM: usize = 256;

/// Failure: a move puts an allocation on the window's path for a person's action (a string cloned to pass a control on, a box for a call that was direct, a
/// list built to find a feature), which no test of behaviour sees and input-to-frame shows only as noise. The window's call for each hot action, as
/// input-to-frame times them, allocates no more on its thread than it did before the moves.
#[test]
fn a_persons_action_allocates_no_more_on_the_windows_thread() {
    let disk = Disk::new();
    let (a_file, b_file) = (disk.file("a.pdf", &files::with_sections(10)), disk.pdf("b.pdf", 10));
    let app = disk.start();
    app.open(&b_file);
    app.open(&a_file);
    let rows = app.tabs();
    let (a, b) = (rows[rows.len() - 1].clone(), rows[rows.len() - 2].clone());
    let contents = |page: u32| recto_app::SidebarRow {
        kind: RowKind::Contents,
        page: Some(page),
        ..a.clone()
    };
    let menu = |id: &str| UiAction::Menu { id: id.into() };
    let mut now = Vec::new();
    let mut measure = |name: &'static str, action: &dyn Fn(bool) -> UiAction| now.push((name, most_allocated(&app, 6, action)));
    measure("tap row", &|first| UiAction::Row {
        row: if first { b.clone() } else { a.clone() },
        adding: false,
    });
    app.act(UiAction::Row { row: a.clone(), adding: false });
    measure("contents jump", &|first| UiAction::Row {
        row: contents(if first { 7 } else { 3 }),
        adding: false,
    });
    measure("typed page", &|first| UiAction::PageTyped {
        text: (if first { "5" } else { "2" }).into(),
    });
    measure("find typed", &|first| UiAction::FindTyped {
        text: (if first { "page 1" } else { "page" }).into(),
    });
    app.act(UiAction::FindTyped { text: "page".into() });
    measure("find step", &|first| menu(if first { "find.next" } else { "find.previous" }));
    app.act(UiAction::FindClose);
    measure("compare click", &|_| UiAction::Row { row: b.clone(), adding: true });
    app.act(UiAction::Row { row: b.clone(), adding: true });
    measure("pane read", &|first| UiAction::PaneRead {
        tab: if first { a.tab } else { b.tab },
    });
    app.act(UiAction::Row { row: a.clone(), adding: false });
    measure("zoom", &|first| menu(if first { "view.zoom_in" } else { "view.zoom_out" }));
    measure("sidebar toggle", &|_| menu("view.sidebar"));
    measure("rotate", &|first| menu(if first { "rotate.right" } else { "rotate.left" }));
    measure("undo", &|_| menu("edit.undo"));
    measure("swipe", &|first| UiAction::Swiped {
        delta_x: if first { 1.0 } else { -1.0 },
    });
    measure("chevron", &|_| UiAction::Chevron { first: 0 });
    let over: Vec<String> = now
        .iter()
        .zip(ALLOCATED)
        .filter(|((_, made), (_, was))| made > was)
        .map(|((name, made), (_, was))| format!("{name}: {made} allocations, {was} before"))
        .collect();
    let names: Vec<&str> = now.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ALLOCATED.map(|(name, _)| name), "the actions measured");
    assert!(
        over.is_empty(),
        "more allocations on the window's thread, the most of six calls each: {over:#?}"
    );
}

/// The window's thread's allocations for each action on 2026-11-13, before the moves: one is the person's command boxed for the session thread's queue.
/// Fewer is fine, and is written down here.
const ALLOCATED: [(&str, u64); 13] = [
    ("tap row", 1),
    ("contents jump", 1),
    ("typed page", 2),
    ("find typed", 1),
    ("find step", 1),
    ("compare click", 1),
    ("pane read", 1),
    ("zoom", 13),
    ("sidebar toggle", 1),
    ("rotate", 2),
    ("undo", 1),
    ("swipe", 1),
    ("chevron", 0),
];

/// Failure (the read of 2026-11-16 before 0.1.2): a zoom lays the rows out across at each step, which sends the document's width and moves the pages, and
/// a step of a pinch or ⌘-scroll then allocates on the window's thread for each page it moves or each op it sends, which input to frame shows only as noise.
/// Over 30 steps of 3% from the fit, past the window's width, and the step that ends the zoom, each as the host makes it (`testkit::zoomed`), the most a step
/// allocates on the window's thread is no more than it was before rows were centred in the zoomed view.
#[test]
fn a_zoom_step_allocates_no_more_on_the_windows_thread() {
    let disk = Disk::new();
    let file = disk.pdf("z.pdf", 10);
    let app = disk.start();
    app.open(&file);
    app.settle();
    let mut view = app.zoom_view();
    app.settle();
    let pointer = recto_app::testkit::zoomed::FRAME / 2.0 + 100.0;
    let (mut most, mut magnification) = (0, 1.0);
    for step in 0..=30 {
        if step < 30 {
            magnification *= 1.03;
        }
        app.listener().forget_keeping_room(LISTENER_ROOM);
        let before = allocations();
        app.zoom_step(&mut view, magnification, pointer, step < 30);
        most = most.max(allocations() - before);
    }
    assert!(
        most <= ZOOM_STEP_ALLOCATED,
        "a zoom step allocated {most} times on the window's thread, {ZOOM_STEP_ALLOCATED} before"
    );
}

/// The most a zoom step allocated on the window's thread on 2026-11-16, before rows were centred in the zoomed view (`main` at ce54a211).
const ZOOM_STEP_ALLOCATED: u64 = 3;
