//! The engine on the real worker: what the app does, with no window. A pretend host applies what it's told and looks at the surfaces.

use recto_platform::local::process_counters;
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::{Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_testpdf::{Page, TempPdf, pages, slow_page, text_lines};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Events {
    ops: AtomicU32,
    drawn: AtomicU32,
}

impl EngineEvents for Events {
    fn ops_ready(&self) {
        self.ops.fetch_add(1, Ordering::SeqCst);
    }
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {
        self.drawn.fetch_add(1, Ordering::SeqCst);
    }
}

fn view(y: f64) -> Viewport {
    Viewport { x: 0.0, y, width: 1168.0, height: 900.0, frame_width: 1168.0, magnification: 1.0, backing: 2.0, live: false, scrolled_to: u64::MAX }
}

/// Stands in for the host: drains what's there, applies it to nothing, and says so.
fn pump(engine: &Engine, limit: Duration) -> Vec<Op> {
    let started = Instant::now();
    let mut all = Vec::new();
    while started.elapsed() < limit {
        let ops = engine.drain();
        if !ops.is_empty() {
            engine.applied();
            all.extend(ops);
        }
        if engine.wait_settled(Duration::from_millis(5)) {
            break;
        }
    }
    all
}

fn document(pages_of_text: usize) -> TempPdf {
    let list: Vec<Page> = (0..pages_of_text).map(|index| Page::new(text_lines(&[&format!("Page number {index}"), "Some words on it, so it isn't blank"]))).collect();
    TempPdf::new("engine.pdf", &pages(&list))
}

fn started(file: &TempPdf, count: u32) -> (Engine, Arc<Events>) {
    let events = Arc::new(Events::default());
    let engine = Engine::new(EngineConfig::default(), events.clone());
    engine.set_viewport(view(0.0));
    engine.activate(1, count, 0, PageMap { files: vec![file.0.clone()], slots: None }, &[]);
    engine.set_viewport(view(0.0));
    (engine, events)
}

/// Failure: the page view never gets tiles from a real worker onto layers: the whole path, from a viewport to a surface a layer can show. Each surface must be a real one, at the size it says, with ink in it somewhere.
#[test]
fn a_real_page_reaches_the_host_as_surfaces() {
    let file = document(3);
    let (engine, events) = started(&file, 3);
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    assert!(ops.iter().any(|op| matches!(op, Op::ShowPage { page: 0, .. })), "{ops:?}");
    let surfaces = ops.iter().filter(|op| matches!(op, Op::Attach { surface, .. } if *surface != 0)).count();
    assert!(surfaces >= 1, "{ops:?}");
    assert!(events.drawn.load(Ordering::SeqCst) >= 1);
    let report = engine.report().0;
    assert_eq!(report["settled"], true);
    assert_eq!(report["low_res_pages"].as_array().unwrap().len(), 0);
    assert!(engine.workers_footprint().0 > 0, "a worker is running, and can be measured");
}

/// Failure: a document held in the page view spins or polls with nothing to do. Budget: idle CPU about 0%, and no more than 1 wake-up a second. Runs in its own process via nextest, and uses this process's own counters.
#[test]
fn the_engine_is_silent_at_rest() {
    let file = document(2);
    let (engine, _events) = started(&file, 2);
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)));
    let pid = std::process::id();
    let before = process_counters(pid).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let after = process_counters(pid).unwrap();
    let woke = after.context_switches.saturating_sub(before.context_switches);
    assert!(woke <= 40, "{woke} context switches in 2 s, with nothing to do");
}

/// Failure: scrolling away from a page that's slow to draw leaves the worker drawing it, so the next page waits. `cancel_before` must end it, by return or kill, well inside the time it would have taken.
#[test]
fn a_slow_page_scrolled_away_from_is_cancelled() {
    let list = vec![Page::new(slow_page(4_000_000)), Page::new(text_lines(&["A quick page"])), Page::new(text_lines(&["Another"]))];
    let file = TempPdf::new("slow.pdf", &pages(&list));
    let (engine, _events) = started(&file, 3);
    // Let the sizes arrive, and the slow draw begin.
    let begun = Instant::now();
    while begun.elapsed() < Duration::from_secs(2) {
        engine.drain();
        engine.applied();
        if engine.report().0["counts"]["jobs"].as_u64().unwrap_or(0) >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let height = engine.report().0["document_height"].as_f64().unwrap();
    let away = height - 900.0;
    engine.set_viewport(view(away));
    let moved = Instant::now();
    pump(&engine, Duration::from_secs(10));
    assert!(engine.wait_settled(Duration::from_secs(10)), "{}", engine.report().0);
    let took = moved.elapsed();
    let report = engine.report().0;
    assert!(report["visible_pages"].as_array().unwrap().iter().any(|page| page == 3), "{report}");
    assert!(report["counts"]["cancelled"].as_u64().unwrap() >= 1, "the slow draw wasn't what ended: {report}");
    assert!(took < Duration::from_secs(4), "the last page took {took:?} to arrive, and the slow one alone takes longer");
}

/// Failure: dark paper's tiles reach the screen as white ones, because the turn was skipped for one path. The surface's first pixel must be the paper's colour, not white.
#[test]
fn dark_paper_changes_what_is_in_the_surface() {
    let file = document(1);
    let (engine, _events) = started(&file, 1);
    pump(&engine, Duration::from_secs(20));
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)));
    assert!(ops.iter().any(|op| matches!(op, Op::ShowPage { paper_rgb, .. } if *paper_rgb == PaperKind::Dark.rgb())), "{ops:?}");
    assert_eq!(engine.report().0["paper"], "dark");
}

/// Failure: a closed window keeps its worker and file, so it's over the budget for a closed window. Everything goes, and comes back when the window does.
#[test]
fn a_closed_window_lets_the_worker_go() {
    let file = document(1);
    let (engine, _events) = started(&file, 1);
    pump(&engine, Duration::from_secs(20));
    assert!(engine.workers_footprint().0 > 0);
    engine.window_closed();
    engine.applied();
    let deadline = Instant::now() + Duration::from_secs(5);
    while engine.workers_footprint().0 > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(engine.workers_footprint().0, 0, "the worker should be gone");
    assert_eq!(engine.report().0["tiles"]["attached"], 0);
    engine.window_opened();
    engine.set_viewport(view(0.0));
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    assert!(engine.report().0["tiles"]["attached"].as_u64().unwrap() >= 1);
}
