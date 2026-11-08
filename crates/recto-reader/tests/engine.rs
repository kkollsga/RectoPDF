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
    copies: AtomicU32,
    panics: AtomicU32,
}

impl EngineEvents for Events {
    fn ops_ready(&self) {
        self.ops.fetch_add(1, Ordering::SeqCst);
    }
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {
        self.drawn.fetch_add(1, Ordering::SeqCst);
        // On the thread that draws: stands in for anything going wrong on it.
        let left = self.panics.load(Ordering::SeqCst);
        if left > 0 {
            self.panics.store(left - 1, Ordering::SeqCst);
            panic!("a test's panic on the thread that draws");
        }
    }
    fn copy_ready(&self) {
        self.copies.fetch_add(1, Ordering::SeqCst);
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
    engine.activate(1, count, 0, PageMap { files: vec![file.0.clone()], slots: None, own: None, turns: None }, &[], None, recto_reader::layout::Spread::Single);
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
    // A few goes: a thread that polls wakes it in all of them, and a busy machine only gets in the way of some. The noisiest has a bound too, so a burst or something that wakes now and then doesn't pass for rest.
    let woke: Vec<u64> = (0..4)
        .map(|_| {
            let before = process_counters(pid).unwrap();
            std::thread::sleep(Duration::from_secs(2));
            let after = process_counters(pid).unwrap();
            after.context_switches.saturating_sub(before.context_switches)
        })
        .collect();
    assert!(woke.iter().min().unwrap() <= &40, "context switches in each 2 s, with nothing to do: {woke:?}");
    assert!(woke.iter().max().unwrap() <= &400, "context switches in each 2 s, with nothing to do: {woke:?}");
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

/// Failure: a bar chart on white stays a light box on dark paper, as figure 8 of the EU competitiveness report did for the owner (2026-11-07: "images with a white background should be dark, as a chart with a white background was left light"): its bars are a quarter of it, more colour than a chart was allowed. A real worker and a real image: its white lands on the paper's own tone, as text pages' paper does, and isn't dimmed white.
#[test]
fn a_chart_on_white_turns_over_on_dark_paper() {
    // 40 by 40, white, with three flat bars and an axis, as figure 8 is drawn.
    let mut chart = Vec::with_capacity(40 * 40 * 3);
    for y in 0..40u32 {
        for x in 0..40u32 {
            let rgb: [u8; 3] = match x {
                4..10 if (14..38).contains(&y) => [21, 32, 240],
                14..20 if (24..38).contains(&y) => [74, 138, 198],
                24..30 if (12..38).contains(&y) => [21, 32, 240],
                _ if y == 38 => [80, 80, 80],
                _ => [255, 255, 255],
            };
            chart.extend_from_slice(&rgb);
        }
    }
    let image = first_extra_object(1);
    let page = Page::new(b"q 612 0 0 792 0 0 cm /Im0 Do Q".to_vec());
    let extra = vec![Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 40 /Height 40 /BitsPerComponent 8 /ColorSpace /DeviceRGB".into(), data: chart }];
    let file = TempPdf::new("chart.pdf", &pages_with(&[page], &format!("/XObject << /Im0 {image} 0 R >>"), extra));
    let (engine, _events) = started(&file, 1);
    pump(&engine, Duration::from_secs(20));
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    let rgb = PaperKind::Dark.rgb();
    let paper = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    let surfaces = corners(&ops);
    assert!(!surfaces.is_empty(), "something was drawn: {ops:?}");
    // A tile's corner falls on the chart's white or on a bar, turned over.
    let dimmed = dim_rgb(0xFF_FF_FF);
    let dimmed = [(dimmed >> 16) as u8, (dimmed >> 8) as u8, dimmed as u8];
    assert!(surfaces.contains(&paper) && !surfaces.contains(&dimmed), "the chart's white is the paper's tone: {surfaces:?} against {paper:?}, and dimmed white is {dimmed:?}");
}

/// Failure: another app, or a save, puts a new file in the open one's place, and the first tile drawn of the new file on dark paper goes by the old file's look of that page, kept from before. That tile is of the new version, so nothing ever draws it again: a part of the page where the old file had a picture stays paper as drawn for white, only dimmed, never turned over. The replacement is a rename of a changed copy, as editors and saves do, and the old page had a picture at its top left that the new one hasn't. Every tile of the new page is on the paper's tone.
#[test]
fn a_replaced_file_gets_its_pages_looked_at_again_on_dark_paper() {
    let red = [0xFF, 0x00, 0x00].repeat(64);
    let image = first_extra_object(1);
    // The top left quarter is a picture, kept as it is on dark paper, and the rest is paper, turned over.
    let page = Page::new(b"q 306 0 0 396 0 396 cm /Im0 Do Q".to_vec());
    let extra = vec![Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceRGB".into(), data: red }];
    let file = TempPdf::new("replaced.pdf", &pages_with(&[page], &format!("/XObject << /Im0 {image} 0 R >>"), extra));
    let (engine, _events) = started(&file, 1);
    engine.set_paper(PaperKind::Dark);
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    // The new file: the same page with text and no picture, written beside it and renamed over it.
    let copy = file.0.with_extension("new");
    std::fs::write(&copy, pages(&[Page::new(text_lines(&["Only words now"]))])).unwrap();
    std::fs::rename(&copy, &file.0).unwrap();
    // Anything that draws again does: here a zoom.
    engine.set_viewport(Viewport { magnification: 1.5, ..view(0.0) });
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    let rgb = PaperKind::Dark.rgb();
    let paper = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    let colours: Vec<u32> = ops.iter().filter_map(|op| if let Op::Attach { rgb, surface: 0, .. } = op { *rgb } else { None }).collect();
    let surfaces = corners(&ops);
    assert!(!colours.is_empty() || !surfaces.is_empty(), "something was drawn: {ops:?}");
    assert!(colours.iter().all(|colour| *colour == rgb), "every blank tile is the paper's tone: {colours:06x?} against {rgb:06x}");
    // The one line of words is near the top, between the tiles' corners, so every corner is paper, the old picture's quarter included.
    assert!(surfaces.iter().all(|corner| *corner == paper), "every tile is on the paper's tone: {surfaces:?} against {paper:?}");
}

/// Failure: a tab's file doesn't answer, on a dead volume, and the page view's one thread sits in `File::open` for it, so a healthy tab gets no tiles. A FIFO with no writer is a file whose open never returns, as a dead volume's does. The reader moving on is all it takes, and that tab's page says what's wrong.
#[test]
fn a_file_that_doesnt_answer_holds_up_no_other_tab() {
    let folder = std::env::temp_dir().join(format!("recto-fifo-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let dead = folder.join("dead.pdf");
    let _ = std::fs::remove_file(&dead);
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&dead).status().unwrap().success());
    let events = Arc::new(Events::default());
    let engine = Engine::new(EngineConfig::default(), events);
    engine.set_viewport(view(0.0));
    engine.activate(1, 1, 0, PageMap { files: vec![dead.clone()], slots: None, own: None, turns: None }, &[], None, recto_reader::layout::Spread::Single);
    engine.set_viewport(view(0.0));
    // Its open is under way and stuck. A healthy tab comes up.
    std::thread::sleep(Duration::from_millis(100));
    let file = document(1);
    engine.activate(2, 1, 0, PageMap { files: vec![file.0.clone()], slots: None, own: None, turns: None }, &[1], None, recto_reader::layout::Spread::Single);
    engine.set_viewport(view(0.0));
    let began = Instant::now();
    pump(&engine, Duration::from_secs(10));
    assert!(engine.wait_settled(Duration::from_secs(5)), "the healthy tab never settled: {}", engine.report().0);
    assert!(began.elapsed() < Duration::from_secs(8), "{:?}", began.elapsed());
    assert!(engine.report().0["tiles"]["attached"].as_u64().unwrap() >= 1);
    // And back on it, it says so rather than waiting for ever.
    engine.activate(1, 1, 0, PageMap { files: vec![dead.clone()], slots: None, own: None, turns: None }, &[2], None, recto_reader::layout::Spread::Single);
    engine.set_viewport(view(0.0));
    let deadline = Instant::now() + Duration::from_secs(8);
    while engine.note().is_none() && Instant::now() < deadline {
        engine.drain();
        engine.applied();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(engine.note().is_some_and(|note| note.contains("answering")), "{:?}", engine.note());
    // Lets its thread go.
    drop(std::fs::OpenOptions::new().write(true).open(&dead));
    let _ = std::fs::remove_dir_all(&folder);
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
    engine.activate(1, 1, 0, PageMap { files: vec![file.0.clone()], slots: None, own: None, turns: None }, &[], None, recto_reader::layout::Spread::Single);
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

/// The highlight on a page in what the host was last told, keeping what each op said. What's selected is said whole each time.
fn highlight_on(ops: &[Op], page: u32) -> Option<Vec<recto_index::select::Rect>> {
    ops.iter().rev().find_map(|op| match op {
        Op::Highlight { page: on, rects } if *on == page => Some(rects.clone()),
        _ => None,
    })
}

/// Lays a page out and draws it, and says where its top left is in the document.
fn settled_on_page(engine: &Engine, page: u32) -> (f64, f64, f64) {
    let mut ops = engine.set_viewport(view(0.0));
    engine.applied();
    ops.extend(pump(engine, Duration::from_secs(20)));
    let shown = ops.iter().rev().find_map(|op| match op {
        Op::ShowPage { page: on, x, y, fit, .. } if *on == page => Some((*x, *y, *fit)),
        _ => None,
    });
    shown.expect("the page should be showing")
}

/// Failure: selection blocks on the worker on the UI thread, a press on a page not read is lost, or nothing tells the host when the text arrives, so a highlight comes only on the next move. The host applies what it's told and hears nothing else, as the real one does.
#[test]
fn a_press_returns_at_once_and_the_highlight_comes_when_the_text_does() {
    let file = TempPdf::new("select.pdf", &pages(&[Page::new(text_lines(&["Alpha beta gamma delta"])), Page::new(text_lines(&["Second page words"]))]));
    let (engine, _events) = started(&file, 2);
    let (x, y, fit) = settled_on_page(&engine, 0);
    let at = |page_x: f64, page_y: f64| (x + page_x * fit, y + page_y * fit);
    // The first line's baseline, from the top of a page 792 tall.
    let (press_x, press_y) = at(74.0, 792.0 - 720.0 - 4.0);
    // Had this waited for the worker, it would have the answer. It's the host's next look that does.
    let ops = engine.selection_press(press_x, press_y, 2, false);
    assert_eq!(highlight_on(&ops, 0), None, "its page hasn't been read, so it can't be known");
    let mut later = engine.selection_release();
    engine.applied();
    later.extend(pump(&engine, Duration::from_secs(20)));
    let rects = highlight_on(&later, 0).expect("a double-click takes a word, once its page is read");
    assert_eq!(rects.len(), 1);
    assert!(rects[0].width > 10.0 && rects[0].width < 60.0, "one word: {:?}", rects[0]);
    assert!(engine.has_selection());
    // A click elsewhere clears it, on the UI thread, with no job.
    let ops = engine.selection_press(press_x, press_y, 1, false);
    assert_eq!(highlight_on(&ops, 0), Some(Vec::new()), "cleared at once, from the text that's here");
    engine.selection_release();
    assert!(!engine.has_selection());
}

/// Failure: what's selected is drawn in document points, so it's wrong at any zoom, or a click at a zoom hits a different word. The host's points are the document's whatever the magnification, so the same point must take the same word.
#[test]
fn a_click_takes_the_same_word_at_any_zoom() {
    let file = TempPdf::new("zoom.pdf", &pages(&[Page::new(text_lines(&["Alpha beta gamma delta", "Second line of words"]))]));
    let (engine, _events) = started(&file, 1);
    let (x, y, fit) = settled_on_page(&engine, 0);
    let (press_x, press_y) = (x + 100.0 * fit, y + (792.0 - 720.0 - 4.0) * fit);
    let word_at = |magnification: f64| {
        engine.set_viewport(Viewport { magnification, width: 1168.0 / magnification, height: 900.0 / magnification, ..view(0.0) });
        engine.applied();
        pump(&engine, Duration::from_secs(20));
        let mut ops = engine.selection_press(press_x, press_y, 2, false);
        ops.extend(engine.selection_release());
        engine.applied();
        ops.extend(pump(&engine, Duration::from_secs(20)));
        let report = engine.report().0;
        (report["selection"]["start"].clone(), report["selection"]["end"].clone(), ops)
    };
    let (start, end, ops) = word_at(1.0);
    assert!(start.is_object() && start != end, "{start} to {end}");
    assert!(highlight_on(&ops, 0).is_some_and(|rects| !rects.is_empty()));
    for magnification in [0.5, 1.5, 3.0, 8.0] {
        let (other_start, other_end, _) = word_at(magnification);
        assert_eq!((&other_start, &other_end), (&start, &end), "at {magnification}x");
    }
}

/// Failure: selecting everything reads every page, or a copy of a long document holds its text page after page, or a copy blocks the thread drawing tiles. It comes out whole, with what's held staying small.
#[test]
fn select_all_reads_what_shows_and_a_copy_stays_bounded() {
    const COUNT: u32 = 60;
    let file = document(COUNT as usize);
    let (engine, events) = started(&file, COUNT);
    settled_on_page(&engine, 0);
    engine.select_all();
    assert!(engine.has_selection());
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(highlight_on(&ops, 0).is_some_and(|rects| !rects.is_empty()), "what's showing is highlighted");
    let report = engine.report().0;
    let read = report["selection"]["text_reads"].as_u64().unwrap();
    assert!(read <= 12, "read {read} pages of {COUNT} just to select them");
    engine.copy_selection(false);
    let mut held_most = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while events.copies.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the copy didn't finish");
        held_most = held_most.max(engine.report().0["selection"]["text_pages_held"].as_u64().unwrap());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(held_most as usize <= recto_reader::selection::MAX_TEXT_PAGES, "{held_most} pages held");
    let copied = engine.take_copied().expect("the copy is ready");
    assert_eq!(copied.pages_read, COUNT);
    assert_eq!(copied.text.matches("Page number").count(), COUNT as usize);
    assert!(copied.text.find("Page number 0").unwrap() < copied.text.find("Page number 59").unwrap(), "in page order");
    assert!(pump(&engine, Duration::from_secs(5)).iter().all(|op| !matches!(op, Op::Attach { .. })) || engine.wait_settled(Duration::from_secs(5)));
}

/// Failure: a page with no text keeps being read, holding the settled state open for ever, or says it has a selection. A page that fails has to come to rest too.
#[test]
fn a_page_with_no_text_comes_to_rest_with_nothing_selected() {
    let scan = Page::new(b"0.5 g 100 100 300 300 re f".to_vec());
    let file = TempPdf::new("scan.pdf", &pages(&[scan]));
    let (engine, _events) = started(&file, 1);
    let (x, y, fit) = settled_on_page(&engine, 0);
    engine.selection_press(x + 200.0 * fit, y + 300.0 * fit, 2, false);
    engine.selection_drag(x + 300.0 * fit, y + 400.0 * fit);
    engine.selection_release();
    let ops = pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(5)), "{}", engine.report().0);
    assert_eq!(highlight_on(&ops, 0).unwrap_or_default().len(), 0);
    assert!(!engine.has_selection());
    assert!(!engine.pointer_is_over_text(x + 200.0 * fit, y + 300.0 * fit), "no I-beam on a scan");
}

/// Failure: the I-beam shows in the margin or between lines, or never shows, because the pointer's page is never read. It's read when there's nothing better to do, and only then says yes.
#[test]
fn the_pointer_asks_for_its_page_and_goes_to_an_i_beam_only_over_text() {
    let file = TempPdf::new("pointer.pdf", &pages(&[Page::new(text_lines(&["Words at the top of the page"]))]));
    let (engine, _events) = started(&file, 1);
    let (x, y, fit) = settled_on_page(&engine, 0);
    let (on_x, on_y) = (x + 80.0 * fit, y + (792.0 - 720.0 - 4.0) * fit);
    assert!(!engine.pointer_is_over_text(on_x, on_y), "not read yet");
    pump(&engine, Duration::from_secs(20));
    assert!(engine.pointer_is_over_text(on_x, on_y), "{}", engine.report().0);
    assert!(!engine.pointer_is_over_text(x + 80.0 * fit, y + 600.0 * fit), "the blank part of the page");
    assert!(!engine.pointer_is_over_text(x - 30.0, on_y), "the margin");
}

/// Failure: the thread that draws dies, and no page draws again, with nothing said: the page just stays blank. It comes back, and pages draw.
#[test]
fn the_thread_that_draws_comes_back_after_a_panic() {
    let file = document(30);
    let (engine, events) = started(&file, 30);
    pump(&engine, Duration::from_secs(20));
    events.panics.store(1, Ordering::SeqCst);
    engine.jump_to(10);
    engine.applied();
    let view_at = engine.report().0["document_height"].as_f64().unwrap() / 30.0 * 10.0;
    engine.set_viewport(Viewport { scrolled_to: u64::MAX, ..view(view_at) });
    engine.applied();
    let ops = pump(&engine, Duration::from_secs(20));
    assert_eq!(events.panics.load(Ordering::SeqCst), 0, "it did panic");
    assert!(engine.wait_settled(Duration::from_secs(20)), "{}", engine.report().0);
    let more = ops.iter().filter(|op| matches!(op, Op::Attach { .. })).count();
    assert!(more > 0, "pages draw again: {ops:?}");
    assert_eq!(engine.note(), None);
}

/// Failure: a page that panics the thread every time takes it down for good, and the reader sees a blank window, with no reason. Past a few goes it stops trying, and says so.
#[test]
fn a_thread_that_keeps_dying_says_so() {
    let file = document(30);
    let (engine, events) = started(&file, 30);
    events.panics.store(1000, Ordering::SeqCst);
    engine.set_viewport(view(0.0));
    engine.applied();
    pump(&engine, Duration::from_secs(20));
    assert!(engine.note().is_some_and(|note| note.contains("draw")), "{:?}", engine.note());
    assert!(engine.wait_settled(Duration::from_secs(5)), "nothing's left to wait for: {}", engine.report().0);
}

/// Failure: a worker dying in the middle of a copy loses the copy, or leaves it running for ever, or leaves pages undrawn. It comes out with what could be read, says what couldn't, and the page view goes on.
#[test]
fn a_worker_dying_in_a_copy_leaves_a_copy_that_says_what_happened() {
    const COUNT: u32 = 40;
    let mut content = slow_page(2_000);
    content.extend_from_slice(&text_lines(&["Page words"]));
    let list: Vec<Page> = (0..COUNT).map(|_| Page::new(content.clone())).collect();
    let file = TempPdf::new("dying.pdf", &pages(&list));
    let (engine, events) = started(&file, COUNT);
    settled_on_page(&engine, 0);
    engine.select_all();
    pump(&engine, Duration::from_secs(20));
    engine.copy_selection(false);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut killed = 0;
    while events.copies.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the copy didn't finish: {}", engine.report().0);
        if killed < 3
            && engine.report().0["selection"]["copying"] == true
            && let Some(pid) = engine.worker_pids().first()
        {
            let _ = std::process::Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
            killed += 1;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(killed > 0, "the worker never got killed, so this tested nothing");
    let copied = engine.take_copied().expect("the copy is ready");
    assert_eq!(copied.pages_read, COUNT);
    assert_eq!(copied.text.matches("Page words").count() as u32 + copied.pages_failed, COUNT, "every page is in it, or counted as not");
    pump(&engine, Duration::from_secs(20));
    assert!(engine.wait_settled(Duration::from_secs(1)), "{}", engine.report().0);
    assert_eq!(engine.report().0["selection"]["copying"], false);
}

/// Failure: the file changes on disk, and a selection and a copy go on with places and pages that were about the old one. A copy stopped this way says why.
#[test]
fn a_file_changing_ends_a_selection_and_says_so_to_a_copy() {
    let file = TempPdf::new("changing.pdf", &pages(&[Page::new(text_lines(&["First"])), Page::new(text_lines(&["Second"])), Page::new(text_lines(&["Third"]))]));
    let (engine, _events) = started(&file, 3);
    settled_on_page(&engine, 0);
    engine.select_all();
    pump(&engine, Duration::from_secs(20));
    assert!(engine.has_selection());
    std::fs::write(&file.0, pages(&[Page::new(text_lines(&["Only this now"]))])).unwrap();
    for pid in engine.worker_pids() {
        let _ = std::process::Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
    }
    std::thread::sleep(Duration::from_millis(50));
    engine.copy_selection(false);
    let deadline = Instant::now() + Duration::from_secs(20);
    while engine.has_selection() {
        assert!(Instant::now() < deadline, "still selected: {}", engine.report().0);
        pump(&engine, Duration::from_millis(50));
    }
    assert!(engine.take_copied().is_none(), "none of it is about this file");
    assert!(engine.selection_notice().is_some_and(|notice| notice.text.contains("changed")), "{:?}", engine.selection_notice());
}
