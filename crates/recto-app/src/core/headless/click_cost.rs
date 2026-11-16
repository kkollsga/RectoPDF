//! **What a still click on a page costs, measured** (the review of `build/image-select`, S1: a click that picks nothing must cost what it costs on
//! `main`). Not a check: it asserts nothing about memory or time, which are one machine's, and runs only when asked, on a release build, with a real file:
//!
//! `RECTO_CLICK_FILE=/path/to/file.pdf tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-app -p recto-render-service --run-ignored only
//! --no-capture -E 'test(=core::headless::click_cost::click_cost)'`
//!
//! On page 1 of the file, shown as a window shows it: a click on body text, a blank margin, an image (if the page has one) and a double click on text, each
//! as the bridge's mouse calls make it, and each waited for until what it started has landed. For each: the person's `images` commands sent (each opens a
//! private worker), the worker processes seen that weren't running before it (sampled every 5 ms, this test's own build's worker only), the tree's
//! footprint at rest before it and its peak during it (this process and every worker at once), and how long it took to land. The points are found on the
//! page, not written in: text by the page's own hit test, an image by `recto images`, the margin as neither.

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp, applied_as_the_host, host_turns};
use recto_agent::Level;
use recto_platform::isolate::process_memory;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MIB: f64 = 1024.0 * 1024.0;

/// This build's worker, beside the test binary's folder.
fn worker() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("recto-render-service")
}

/// The workers running now, and this process's footprint with theirs.
fn tree(worker: &std::path::Path) -> (BTreeSet<u32>, u64) {
    let pids: BTreeSet<u32> = recto_platform::local::processes_running(worker).into_iter().collect();
    let mine = process_memory(std::process::id()).map_or(0, |memory| memory.footprint_bytes);
    let theirs: u64 = pids.iter().filter_map(|pid| process_memory(*pid)).map(|memory| memory.footprint_bytes).sum();
    (pids, mine + theirs)
}

/// The view a window would have.
fn shown(app: &HeadlessApp) {
    let mut view = recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    };
    applied_as_the_host(&app.core().engine.set_viewport(view));
    host_turns(app, &mut view, true);
}

/// Everything a click started landed: the page view not looking for an image, the snapshot tool's slot free, the app at rest.
fn landed(app: &HeadlessApp) {
    let finding = || app.core().panes.reading_engine(&app.core().engine).report().0["selection"]["image"]["finding"] == true;
    app.wait_until("the click landed", || !finding() && !app.core().tools.tool().busy());
    app.settle();
}

/// A press and release at `at` on page 1 with this many clicks, and the window's `LinkClicked`.
fn press(app: &HeadlessApp, (x, y): (f32, f32), clicks: u32) {
    let inner = app.core();
    let engine = inner.panes.reading_engine(&inner.engine);
    let (x, y) = engine.points().document_point(0, f64::from(x), f64::from(y)).expect("page 1 laid out");
    applied_as_the_host(&engine.selection().press(x, y, clicks, false));
    applied_as_the_host(&engine.selection().release());
    app.act(UiAction::LinkClicked { pane: 0 });
}

/// One case, measured: `clicks` at `at`, then everything landed.
fn measure(app: &HeadlessApp, what: &str, at: (f32, f32), clicks: &[u32]) {
    let worker = worker();
    let (before, rest) = tree(&worker);
    let (stop, peak, seen) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(rest)),
        Arc::new(std::sync::Mutex::new(BTreeSet::new())),
    );
    let sampler = {
        let (stop, peak, seen, worker) = (Arc::clone(&stop), Arc::clone(&peak), Arc::clone(&seen), worker.clone());
        std::thread::Builder::new()
            .name("test-click-cost-sampler".into())
            .spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let (pids, bytes) = tree(&worker);
                    peak.fetch_max(bytes, Ordering::SeqCst);
                    crate::lock(&seen).extend(pids);
                    // A sampler, not a wait: it reads the processes every 5 ms until told to stop.
                    std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
                }
            })
            .expect("a thread")
    };
    app.keep_sent();
    let started = Instant::now();
    for clicks in clicks {
        press(app, at, *clicks);
    }
    landed(app);
    let took = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    sampler.join().unwrap();
    let looks = app.sent().iter().filter(|command| command.tag() == "images").count();
    let started_workers = crate::lock(&seen).difference(&before).count();
    eprintln!(
        "click_cost {what:<22} at {:>6.1},{:>6.1}  looks sent {looks}  workers started {started_workers}  tree at rest {:>6.1} MiB  peak {:>6.1} MiB  landed in {:>5.2} s",
        at.0,
        at.1,
        rest as f64 / MIB,
        peak.load(Ordering::SeqCst) as f64 / MIB,
        took.as_secs_f64()
    );
}

/// Whether a point on page 1 is over text, as the page's own hit test says once its text is held.
fn over_text(app: &HeadlessApp, (x, y): (f32, f32)) -> bool {
    let engine = app.core().panes.reading_engine(&app.core().engine);
    let (x, y) = engine.points().document_point(0, f64::from(x), f64::from(y)).expect("page 1 laid out");
    engine.selection().is_over_text(x, y)
}

#[test]
#[ignore = "a measurement on a real file, release build: RECTO_CLICK_FILE"]
fn click_cost() {
    let Some(file) = std::env::var_os("RECTO_CLICK_FILE").map(PathBuf::from) else {
        eprintln!("click_cost: set RECTO_CLICK_FILE");
        return;
    };
    let disk = Disk::new();
    let copy = disk.files().join(file.file_name().unwrap());
    std::fs::copy(&file, &copy).expect("the file copied in");
    let app = disk.start();
    app.open(&copy);
    shown(&app);
    app.settle();
    let listed = app.recto(Level::ReadOnly, &["images", "1"]).expect("images");
    let (width, height) = (listed["width_pt"].as_f64().unwrap() as f32, listed["height_pt"].as_f64().unwrap() as f32);
    let rects: Vec<[f32; 4]> = listed["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|image| [0, 1, 2, 3].map(|at| image["rect"][at].as_f64().unwrap() as f32))
        .collect();
    // The page's text read for the hit test, as the pointer asks for it.
    let _ = over_text(&app, (1.0, 1.0));
    app.settle();
    let grid =
        |step: f32| (1..(height / step) as u32).flat_map(move |row| (1..(width / step) as u32).map(move |column| (column as f32 * step, row as f32 * step)));
    let inside = |(x, y): (f32, f32), rect: &[f32; 4]| x >= rect[0] && x <= rect[0] + rect[2] && y >= rect[1] && y <= rect[1] + rect[3];
    let on_text = grid(4.0).find(|point| over_text(&app, *point) && !rects.iter().any(|rect| inside(*point, rect)));
    let margin = grid(4.0).find(|point| !over_text(&app, *point) && !rects.iter().any(|rect| inside(*point, rect)));
    // The largest image, at a point off text.
    let mut by_size = rects.clone();
    by_size.sort_by(|one, two| (two[2] * two[3]).total_cmp(&(one[2] * one[3])));
    let on_image = by_size
        .first()
        .and_then(|rect| grid(4.0).find(|point| inside(*point, rect) && !over_text(&app, *point)));
    eprintln!(
        "click_cost file {} page 1, {width} x {height} pt, {} images listed, load {}",
        file.display(),
        rects.len(),
        String::from_utf8_lossy(&std::process::Command::new("uptime").output().map(|out| out.stdout).unwrap_or_default()).trim()
    );
    if let Some(at) = on_text {
        measure(&app, "body text", at, &[1]);
    }
    if let Some(at) = margin {
        measure(&app, "blank margin", at, &[1]);
    }
    if let Some(at) = on_image {
        measure(&app, "image", at, &[1]);
    }
    if let Some(at) = on_text {
        measure(&app, "double click on text", at, &[1, 2]);
    }
    drop(app.quit());
}
