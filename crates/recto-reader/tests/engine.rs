//! The engine on the real worker: what the app does, with no window. A pretend host applies what it's told and looks at the surfaces.

use recto_platform::local::process_counters;
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::{Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_reader::paper::dim_rgb;
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages, pages_with, slow_page, text_lines};
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
    // The quietest of a few goes: a thread that polls wakes it in all of them, and a busy machine only gets in the way of some.
    let woke = (0..4)
        .map(|_| {
            let before = process_counters(pid).unwrap();
            std::thread::sleep(Duration::from_secs(2));
            let after = process_counters(pid).unwrap();
            after.context_switches.saturating_sub(before.context_switches)
        })
        .min()
        .unwrap();
    assert!(woke <= 40, "{woke} context switches in the quietest 2 s, with nothing to do");
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

/// What's at a surface's top left, red, green and blue, for each surface attached in these ops.
#[allow(unsafe_code)]
fn corners(ops: &[Op]) -> Vec<[u8; 3]> {
    ops.iter()
        .filter_map(|op| if let Op::Attach { surface, .. } = op { Some(*surface) } else { None })
        .filter(|surface| *surface != 0)
        .filter_map(|reference| {
            // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
            let surface = unsafe { recto_platform::surface::PixelSurface::from_host_reference(reference) }?;
            surface.with_pixels(|pixels| [pixels[2], pixels[1], pixels[0]]).ok()
        })
        .collect()
}

/// Failure: dark paper's tiles reach the screen as white ones, because the turn was skipped for one path. Reads the surfaces themselves: a page's blank corner is white in what's drawn for white paper, and the paper's colour in what's drawn for dark.
#[test]
fn dark_paper_changes_what_is_in_the_surface() {
    let file = document(1);
    let (engine, _events) = started(&file, 1);
    let white = corners(&pump(&engine, Duration::from_secs(20)));
    assert!(!white.is_empty() && white.iter().all(|corner| *corner == [0xFF, 0xFF, 0xFF]), "{white:?}");
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)));
    assert_eq!(engine.report().0["paper"], "dark");
    let dark = corners(&ops);
    let rgb = PaperKind::Dark.rgb();
    let paper = [((rgb >> 16) & 0xFF) as u8, ((rgb >> 8) & 0xFF) as u8, (rgb & 0xFF) as u8];
    assert!(!dark.is_empty() && dark.iter().all(|corner| *corner == paper), "{dark:?} against {paper:?}");
}

/// Failure: a page that's mostly a picture goes negative on dark paper, as the USGS map's band went mint. Owner, 2026-11-07: "I agree" that it's dimmed instead. A real worker and a real image over the whole page: every surface and colour on the layers is the image's true colour, dimmed, and none is turned over.
#[test]
fn a_page_that_is_a_picture_keeps_its_colours_on_dark_paper() {
    let red = [0xFF, 0x00, 0x00].repeat(64);
    let image = first_extra_object(1);
    let page = Page::new(b"q 612 0 0 792 0 0 cm /Im0 Do Q".to_vec());
    let extra = vec![Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceRGB".into(), data: red }];
    let file = TempPdf::new("picture.pdf", &pages_with(&[page], &format!("/XObject << /Im0 {image} 0 R >>"), extra));
    let (engine, _events) = started(&file, 1);
    pump(&engine, Duration::from_secs(20));
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    let dimmed = dim_rgb(0xFF_00_00);
    let want = [(dimmed >> 16) as u8, 0, 0];
    let colours: Vec<u32> = ops.iter().filter_map(|op| if let Op::Attach { rgb, surface: 0, .. } = op { *rgb } else { None }).collect();
    assert!(colours.iter().all(|rgb| *rgb == dimmed), "{colours:06x?} against {dimmed:06x}");
    let surfaces = corners(&ops);
    assert!(surfaces.iter().all(|corner| *corner == want), "{surfaces:?} against {want:?}");
    assert!(!colours.is_empty() || !surfaces.is_empty(), "something was drawn: {ops:?}");
}

/// Failure: closing the window leaves "there are ops" raised with no host to take them, so when the window's back nobody is ever told again, and tiles stop arriving. The host must hear of new ones, even if no ops were taken in between.
#[test]
fn tiles_keep_arriving_after_the_window_comes_back() {
    let file = document(1);
    let (engine, events) = started(&file, 1);
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)));
    // The window's gone: whatever it's told now, nobody takes.
    engine.window_closed();
    let before = events.ops.load(Ordering::SeqCst);
    engine.window_opened();
    engine.set_viewport(view(0.0));
    let deadline = Instant::now() + Duration::from_secs(20);
    while events.ops.load(Ordering::SeqCst) == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(events.ops.load(Ordering::SeqCst) > before, "tiles were drawn, and nobody was told");
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
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

/// Failure: a heavy page at rest holds its worker's hundreds of megabytes for the 30 s of the idle rule, when it could have gone once the reader had paused. Set so that any worker counts as big. It must go by itself, with nobody calling, and the page must still be on screen.
#[test]
fn a_worker_holding_a_lot_is_let_go_at_rest() {
    let file = document(1);
    let events = Arc::new(Events::default());
    let policy = recto_render_service::Policy { retire_bytes: 1, ..recto_render_service::Policy::default() };
    let engine = Engine::new(EngineConfig { service: None, policy: Some(policy) }, events);
    engine.set_viewport(view(0.0));
    engine.activate(1, 1, 0, PageMap { files: vec![file.0.clone()], slots: None }, &[]);
    engine.set_viewport(view(0.0));
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)));
    let deadline = Instant::now() + Duration::from_secs(8);
    while engine.workers_footprint().0 > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(engine.workers_footprint().0, 0, "it should have gone by now");
    assert!(engine.report().0["tiles"]["attached"].as_u64().unwrap() >= 1, "the page stays on screen");
}
