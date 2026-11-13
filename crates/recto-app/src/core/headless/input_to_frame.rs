//! **Input to frame, measured** (step R2.3): how long a person's action takes from the window's call to what the frame is made from being published, on the
//! headless app, for the actions on the hot path. Not a check: it asserts nothing about time, which is one machine's, and runs only when asked, on a release build:
//!
//! `tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-app --run-ignored only --no-capture -E 'test(=core::headless::input_to_frame::input_to_frame)'`
//!
//! Each case times the window's own call (the UI thread's part: it must never wait) and then a flush of the session thread's queue, which returns once the call
//! and the look after it are done: what the frame waits for. The window's calls as it makes them, `UiAction`s (which before C4 were the bridge's own methods, the
//! same calls), before and after R2.3 moves a person's actions onto commands, so the runs compare.

use crate::RowKind;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use std::time::{Duration, Instant};

/// How many times each case is timed.
const RUNS: usize = 500;

/// The window's call, and then the wait for what it made to be published.
fn timed(app: &HeadlessApp, call: impl FnOnce(&HeadlessApp)) -> (Duration, Duration) {
    let began = Instant::now();
    call(app);
    let called = began.elapsed();
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the call");
    (called, began.elapsed())
}

fn micros(sorted: &[Duration], at: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * at).round() as usize;
    sorted[index].as_secs_f64() * 1e6
}

/// One line of the table: the case, the tabs open, the window's call at p99, and call-to-published at p50, p95 and p99, in microseconds.
fn report(case: &str, tabs: usize, mut called: Vec<Duration>, mut published: Vec<Duration>) {
    called.sort_unstable();
    published.sort_unstable();
    eprintln!(
        "{case:<14} {tabs:>5} tabs  ui p99 {:>8.1}  published p50 {:>8.1}  p95 {:>8.1}  p99 {:>8.1}",
        micros(&called, 0.99),
        micros(&published, 0.5),
        micros(&published, 0.95),
        micros(&published, 0.99)
    );
}

/// Times one case, alternating between two forms of it so each run changes something, with `between` run untimed before each.
fn case(app: &HeadlessApp, name: &str, tabs: usize, between: impl Fn(&HeadlessApp), call: impl Fn(&HeadlessApp, bool)) {
    let (mut called, mut published) = (Vec::with_capacity(RUNS), Vec::with_capacity(RUNS));
    for run in 0..RUNS {
        between(app);
        let (ui, done) = timed(app, |app| call(app, run % 2 == 0));
        called.push(ui);
        published.push(done);
    }
    app.settle();
    report(name, tabs, called, published);
}

fn measure(tabs: usize) {
    let disk = Disk::new();
    let files: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("f{at:04}.pdf"), 10)).collect();
    let app = disk.start();
    // One at a time: a few files are read at once, and past that one doesn't open.
    for file in &files {
        app.open(file);
    }
    let rows = app.tabs();
    assert_eq!(rows.len(), tabs, "every file has its tab");
    // The last two opened, which are warm.
    let (a, b) = (rows[rows.len() - 1].clone(), rows[rows.len().saturating_sub(2)].clone());
    let nothing = |_: &HeadlessApp| {};
    case(&app, "tap row", tabs, nothing, |app, first| {
        app.act(UiAction::Row {
            row: if first { b.clone() } else { a.clone() },
            adding: false,
        });
    });
    app.act(UiAction::Row { row: a.clone(), adding: false });
    app.settle();
    let contents = |page: u32| crate::SidebarRow {
        kind: RowKind::Contents,
        page: Some(page),
        ..a.clone()
    };
    case(&app, "contents jump", tabs, nothing, |app, first| {
        app.act(UiAction::Row {
            row: contents(if first { 7 } else { 3 }),
            adding: false,
        });
    });
    case(&app, "typed page", tabs, nothing, |app, first| {
        app.act(UiAction::PageTyped {
            text: (if first { "5" } else { "2" }).into(),
        });
    });
    case(&app, "find typed", tabs, nothing, |app, first| {
        app.act(UiAction::FindTyped {
            text: (if first { "page 1" } else { "page" }).into(),
        });
    });
    case(&app, "zoom", tabs, nothing, |app, first| {
        app.act(UiAction::Menu {
            id: (if first { "view.zoom_in" } else { "view.zoom_out" }).into(),
        });
    });
    let rotate = |app: &HeadlessApp| {
        app.act(UiAction::Menu { id: "rotate.right".into() });
        assert!(app.core().core.flush(WAIT_LIMIT));
    };
    case(&app, "undo", tabs, rotate, |app, _| {
        app.act(UiAction::Menu { id: "edit.undo".into() });
    });
    drop(app.quit());
}

/// Measures, and asserts nothing about time: see the module's words.
#[test]
#[ignore = "a measurement, run on a release build when asked"]
fn input_to_frame() {
    for tabs in [1, 100, 1000] {
        measure(tabs);
    }
}
