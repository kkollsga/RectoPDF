//! The engine on the real worker: what the app does, with no window. A pretend host applies what it's told and looks at the surfaces.

use recto_platform::local::process_counters;
use recto_reader::engine::EngineEvents;
use recto_reader::paper::dim_rgb;
use recto_reader::tiles::{Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
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

/// How long a test waits for something that must happen: only a hang reaches it. On 2026-11-12 a stall of about 10 s hit unrelated tests at once in the main checkout while other checks ran, and bounds of 5 to 25 s that measured the machine failed. A test that waits longer than this has found a bug, and says what it saw.
const HANG: Duration = Duration::from_secs(60);

/// Waits for the engine to settle, for as long as [`HANG`].
fn settles(engine: &Engine) -> bool {
    engine.wait_settled(HANG)
}

fn view(y: f64) -> Viewport {
    Viewport {
        x: 0.0,
        y,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
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
    let list: Vec<Page> = (0..pages_of_text)
        .map(|index| Page::new(text_lines(&[&format!("Page number {index}"), "Some words on it, so it isn't blank"])))
        .collect();
    TempPdf::new("engine.pdf", &pages(&list))
}

fn started(file: &TempPdf, count: u32) -> (Engine, Arc<Events>) {
    let events = Arc::new(Events::default());
    let engine = Engine::new(EngineConfig::default(), events.clone());
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        count,
        0,
        PageMap {
            files: vec![file.0.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    (engine, events)
}

/// Failure: the page view never gets tiles from a real worker onto layers: the whole path, from a viewport to a surface a layer can show. Each surface must be a real one, at the size it says, with ink in it somewhere.
#[test]
fn a_real_page_reaches_the_host_as_surfaces() {
    let file = document(3);
    let (engine, events) = started(&file, 3);
    let ops = pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
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
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
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
    let list = vec![
        Page::new(slow_page(4_000_000)),
        Page::new(text_lines(&["A quick page"])),
        Page::new(text_lines(&["Another"])),
    ];
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
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let took = moved.elapsed();
    let report = engine.report().0;
    assert!(report["visible_pages"].as_array().unwrap().iter().any(|page| page == 3), "{report}");
    // The slow draw being cancelled is the property: it ended by return or kill instead of running to its end. A fixed time for the last page to
    // arrive was a machine's timing, not the property: a loaded Mac took 4.3 s against a 4 s limit (2026-11-08). The settle above is bounded.
    assert!(
        report["counts"]["cancelled"].as_u64().unwrap() >= 1,
        "the slow draw wasn't what ended, after {took:?}: {report}"
    );
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
    let white = corners(&pump(&engine, HANG));
    assert!(!white.is_empty() && white.iter().all(|corner| *corner == [0xFF, 0xFF, 0xFF]), "{white:?}");
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
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
    let extra = vec![Object::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceRGB".into(),
        data: red,
    }];
    let file = TempPdf::new("picture.pdf", &pages_with(&[page], &format!("/XObject << /Im0 {image} 0 R >>"), extra));
    let (engine, _events) = started(&file, 1);
    pump(&engine, HANG);
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let dimmed = dim_rgb(0xFF_00_00);
    let want = [(dimmed >> 16) as u8, 0, 0];
    let colours: Vec<u32> = ops
        .iter()
        .filter_map(|op| if let Op::Attach { rgb, surface: 0, .. } = op { *rgb } else { None })
        .collect();
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
    let extra = vec![Object::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 40 /Height 40 /BitsPerComponent 8 /ColorSpace /DeviceRGB".into(),
        data: chart,
    }];
    let file = TempPdf::new("chart.pdf", &pages_with(&[page], &format!("/XObject << /Im0 {image} 0 R >>"), extra));
    let (engine, _events) = started(&file, 1);
    pump(&engine, HANG);
    engine.set_paper(PaperKind::Dark);
    let ops = pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let rgb = PaperKind::Dark.rgb();
    let paper = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    let surfaces = corners(&ops);
    assert!(!surfaces.is_empty(), "something was drawn: {ops:?}");
    // A tile's corner falls on the chart's white or on a bar, turned over.
    let dimmed = dim_rgb(0xFF_FF_FF);
    let dimmed = [(dimmed >> 16) as u8, (dimmed >> 8) as u8, dimmed as u8];
    assert!(
        surfaces.contains(&paper) && !surfaces.contains(&dimmed),
        "the chart's white is the paper's tone: {surfaces:?} against {paper:?}, and dimmed white is {dimmed:?}"
    );
}

/// Failure: another app, or a save, puts a new file in the open one's place, and the first tile drawn of the new file on dark paper goes by the old file's look of that page, kept from before. That tile is of the new version, so nothing ever draws it again: a part of the page where the old file had a picture stays paper as drawn for white, only dimmed, never turned over. The replacement is a rename of a changed copy, as editors and saves do, and the old page had a picture at its top left that the new one hasn't. Every tile of the new page is on the paper's tone.
#[test]
fn a_replaced_file_gets_its_pages_looked_at_again_on_dark_paper() {
    let red = [0xFF, 0x00, 0x00].repeat(64);
    let image = first_extra_object(1);
    // The top left quarter is a picture, kept as it is on dark paper, and the rest is paper, turned over.
    let page = Page::new(b"q 306 0 0 396 0 396 cm /Im0 Do Q".to_vec());
    let extra = vec![Object::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 8 /Height 8 /BitsPerComponent 8 /ColorSpace /DeviceRGB".into(),
        data: red,
    }];
    let file = TempPdf::new("replaced.pdf", &pages_with(&[page], &format!("/XObject << /Im0 {image} 0 R >>"), extra));
    let (engine, _events) = started(&file, 1);
    engine.set_paper(PaperKind::Dark);
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    // The new file: the same page with text and no picture, written beside it and renamed over it.
    let copy = file.0.with_extension("new");
    std::fs::write(&copy, pages(&[Page::new(text_lines(&["Only words now"]))])).unwrap();
    std::fs::rename(&copy, &file.0).unwrap();
    // Anything that draws again does: here a zoom.
    engine.set_viewport(Viewport {
        magnification: 1.5,
        ..view(0.0)
    });
    let ops = pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let rgb = PaperKind::Dark.rgb();
    let paper = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    let colours: Vec<u32> = ops
        .iter()
        .filter_map(|op| if let Op::Attach { rgb, surface: 0, .. } = op { *rgb } else { None })
        .collect();
    let surfaces = corners(&ops);
    assert!(!colours.is_empty() || !surfaces.is_empty(), "something was drawn: {ops:?}");
    assert!(
        colours.iter().all(|colour| *colour == rgb),
        "every blank tile is the paper's tone: {colours:06x?} against {rgb:06x}"
    );
    // The one line of words is near the top, between the tiles' corners, so every corner is paper, the old picture's quarter included.
    assert!(
        surfaces.iter().all(|corner| *corner == paper),
        "every tile is on the paper's tone: {surfaces:?} against {paper:?}"
    );
}

/// Failure: a tab's file doesn't answer, on a dead volume, and the page view's one thread sits in `File::open` for it, so a healthy tab gets no tiles. A FIFO with no writer is a file whose open never returns, as a dead volume's does. The reader moving on is all it takes, and that tab's page says what's wrong.
#[test]
fn a_file_that_doesnt_answer_holds_up_no_other_tab() {
    let folder = recto_testpdf::TempDir::new("recto-fifo-");
    std::fs::create_dir_all(&folder).unwrap();
    let dead = folder.join("dead.pdf");
    let _ = std::fs::remove_file(&dead);
    assert!(std::process::Command::new("/usr/bin/mkfifo").arg(&dead).status().unwrap().success());
    let events = Arc::new(Events::default());
    let engine = Engine::new(EngineConfig::default(), events);
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        1,
        0,
        PageMap {
            files: vec![dead.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    // Its open is under way and stuck. A healthy tab comes up.
    std::thread::sleep(Duration::from_millis(100));
    let file = document(1);
    engine.activate(
        2,
        1,
        0,
        PageMap {
            files: vec![file.0.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[1],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    // The property is an order, not a time: the healthy tab settles while the dead file's open is still stuck. The fifo gets its writer only at the foot of this test, so the
    // open can't have ended before this point; a tab held up behind it never settles, and fails at the hang bound with the engine's report.
    pump(&engine, HANG);
    assert!(settles(&engine), "the healthy tab never settled: {}", engine.report().0);
    assert!(engine.report().0["tiles"]["attached"].as_u64().unwrap() >= 1);
    // And back on it, it says so rather than waiting for ever.
    engine.activate(
        1,
        1,
        0,
        PageMap {
            files: vec![dead.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[2],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    let deadline = Instant::now() + HANG;
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
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    // The window's gone: whatever it's told now, nobody takes.
    engine.window_closed();
    let before = events.ops.load(Ordering::SeqCst);
    engine.window_opened();
    engine.set_viewport(view(0.0));
    let deadline = Instant::now() + HANG;
    while events.ops.load(Ordering::SeqCst) == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(events.ops.load(Ordering::SeqCst) > before, "tiles were drawn, and nobody was told");
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
}

/// Failure: a closed window keeps its worker and file, so it's over the budget for a closed window. Everything goes, and comes back when the window does.
#[test]
fn a_closed_window_lets_the_worker_go() {
    let file = document(1);
    let (engine, _events) = started(&file, 1);
    pump(&engine, HANG);
    assert!(engine.workers_footprint().0 > 0);
    engine.window_closed();
    engine.applied();
    let deadline = Instant::now() + HANG;
    while engine.workers_footprint().0 > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(engine.workers_footprint().0, 0, "the worker should be gone");
    assert_eq!(engine.report().0["tiles"]["attached"], 0);
    engine.window_opened();
    engine.set_viewport(view(0.0));
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    assert!(engine.report().0["tiles"]["attached"].as_u64().unwrap() >= 1);
}

/// Failure: a heavy page at rest holds its worker's hundreds of megabytes for the 30 s of the idle rule, when it could have gone once the reader had paused. Set so that any worker counts as big. It must go by itself, with nobody calling, and the page must still be on screen.
#[test]
fn a_worker_holding_a_lot_is_let_go_at_rest() {
    let file = document(1);
    let events = Arc::new(Events::default());
    let policy = recto_render_service::Policy {
        retire_bytes: 1,
        ..recto_render_service::Policy::default()
    };
    let engine = Engine::new(
        EngineConfig {
            service: None,
            policy: Some(policy),
            keys: None,
        },
        events,
    );
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        1,
        0,
        PageMap {
            files: vec![file.0.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let deadline = Instant::now() + HANG;
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
    ops.extend(pump(engine, HANG));
    let shown = ops.iter().rev().find_map(|op| match op {
        Op::ShowPage { page: on, x, y, fit, .. } if *on == page => Some((*x, *y, *fit)),
        _ => None,
    });
    shown.expect("the page should be showing")
}

/// Failure: selection blocks on the worker on the UI thread, a press on a page not read is lost, or nothing tells the host when the text arrives, so a highlight comes only on the next move. The host applies what it's told and hears nothing else, as the real one does.
#[test]
fn a_press_returns_at_once_and_the_highlight_comes_when_the_text_does() {
    let file = TempPdf::new(
        "select.pdf",
        &pages(&[
            Page::new(text_lines(&["Alpha beta gamma delta"])),
            Page::new(text_lines(&["Second page words"])),
        ]),
    );
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
    later.extend(pump(&engine, HANG));
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
    let file = TempPdf::new(
        "zoom.pdf",
        &pages(&[Page::new(text_lines(&["Alpha beta gamma delta", "Second line of words"]))]),
    );
    let (engine, _events) = started(&file, 1);
    let (x, y, fit) = settled_on_page(&engine, 0);
    let (press_x, press_y) = (x + 100.0 * fit, y + (792.0 - 720.0 - 4.0) * fit);
    let word_at = |magnification: f64| {
        engine.set_viewport(Viewport {
            magnification,
            width: 1168.0 / magnification,
            height: 900.0 / magnification,
            ..view(0.0)
        });
        engine.applied();
        pump(&engine, HANG);
        let mut ops = engine.selection_press(press_x, press_y, 2, false);
        ops.extend(engine.selection_release());
        engine.applied();
        ops.extend(pump(&engine, HANG));
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
    let ops = pump(&engine, HANG);
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
    assert!(
        copied.text.find("Page number 0").unwrap() < copied.text.find("Page number 59").unwrap(),
        "in page order"
    );
    assert!(pump(&engine, Duration::from_secs(5)).iter().all(|op| !matches!(op, Op::Attach { .. })) || settles(&engine));
}

/// A host that pulls from inside `copy_ready`: it selects again, on the thread it was called on, which goes through the page view's selection news once more.
#[derive(Default)]
struct PullsInCopyReady {
    engine: std::sync::OnceLock<Engine>,
    returned: std::sync::atomic::AtomicBool,
    pulled: std::sync::atomic::AtomicBool,
}

impl EngineEvents for PullsInCopyReady {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    fn copy_ready(&self) {
        if !self.pulled.swap(true, Ordering::SeqCst)
            && let Some(engine) = self.engine.get()
        {
            engine.select_all();
            self.returned.store(true, Ordering::SeqCst);
        }
    }
}

/// Failure: the page view tells the host a copy is ready with its own notice lock still held, so a host that pulls from inside `copy_ready`, or anything that reaches the page view's selection news from it, deadlocks on that lock (`recto_app::Listener`'s promise: the host is never called with the core's locks held). Bounded: a deadlock fails the wait, not the run.
#[test]
fn copy_ready_comes_with_no_lock_of_the_page_views_held() {
    let file = document(2);
    let events = Arc::new(PullsInCopyReady::default());
    let engine = Engine::new(EngineConfig::default(), events.clone());
    let _ = events.engine.set(engine.clone());
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        2,
        0,
        PageMap {
            files: vec![file.0.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    settled_on_page(&engine, 0);
    engine.select_all();
    pump(&engine, Duration::from_secs(20));
    engine.copy_selection(false);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !events.returned.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "copy_ready was called (pulled: {}) and the host's pull never returned",
            events.pulled.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
    assert!(engine.take_copied().is_some(), "the copy is there to take");
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
    let ops = pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
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
    pump(&engine, HANG);
    assert!(engine.pointer_is_over_text(on_x, on_y), "{}", engine.report().0);
    assert!(!engine.pointer_is_over_text(x + 80.0 * fit, y + 600.0 * fit), "the blank part of the page");
    assert!(!engine.pointer_is_over_text(x - 30.0, on_y), "the margin");
}

/// Failure: the thread that draws dies, and no page draws again, with nothing said: the page just stays blank. It comes back, and pages draw.
#[test]
fn the_thread_that_draws_comes_back_after_a_panic() {
    let file = document(30);
    let (engine, events) = started(&file, 30);
    pump(&engine, HANG);
    events.panics.store(1, Ordering::SeqCst);
    engine.jump_to(10);
    engine.applied();
    let view_at = engine.report().0["document_height"].as_f64().unwrap() / 30.0 * 10.0;
    engine.set_viewport(Viewport {
        scrolled_to: u64::MAX,
        ..view(view_at)
    });
    engine.applied();
    let ops = pump(&engine, HANG);
    assert_eq!(events.panics.load(Ordering::SeqCst), 0, "it did panic");
    assert!(settles(&engine), "{}", engine.report().0);
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
    pump(&engine, HANG);
    assert!(engine.note().is_some_and(|note| note.contains("draw")), "{:?}", engine.note());
    assert!(settles(&engine), "nothing's left to wait for: {}", engine.report().0);
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
    pump(&engine, HANG);
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
    assert_eq!(
        copied.text.matches("Page words").count() as u32 + copied.pages_failed,
        COUNT,
        "every page is in it, or counted as not"
    );
    pump(&engine, HANG);
    assert!(engine.wait_settled(Duration::from_secs(1)), "{}", engine.report().0);
    assert_eq!(engine.report().0["selection"]["copying"], false);
}

/// Failure: the file changes on disk, and a selection and a copy go on with places and pages that were about the old one. A copy stopped this way says why.
#[test]
fn a_file_changing_ends_a_selection_and_says_so_to_a_copy() {
    let file = TempPdf::new(
        "changing.pdf",
        &pages(&[
            Page::new(text_lines(&["First"])),
            Page::new(text_lines(&["Second"])),
            Page::new(text_lines(&["Third"])),
        ]),
    );
    let (engine, _events) = started(&file, 3);
    settled_on_page(&engine, 0);
    engine.select_all();
    pump(&engine, HANG);
    assert!(engine.has_selection());
    std::fs::write(&file.0, pages(&[Page::new(text_lines(&["Only this now"]))])).unwrap();
    for pid in engine.worker_pids() {
        let _ = std::process::Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
    }
    std::thread::sleep(Duration::from_millis(50));
    engine.copy_selection(false);
    let deadline = Instant::now() + HANG;
    while engine.has_selection() {
        assert!(Instant::now() < deadline, "still selected: {}", engine.report().0);
        pump(&engine, Duration::from_millis(50));
    }
    assert!(engine.take_copied().is_none(), "none of it is about this file");
    assert!(
        engine.selection_notice().is_some_and(|notice| notice.text.contains("changed")),
        "{:?}",
        engine.selection_notice()
    );
}

/// Failure (the saved point's review): a page added from another PDF is drawn by path, so once that PDF is rewritten, by another app or a save of its own, the page shows whatever is at its page number now. Drawing checks the file is still what it was when its pages were added, and says why it isn't drawn when it's not, while the tab's own pages draw.
#[test]
fn a_page_from_an_added_pdf_that_changed_isnt_drawn() {
    let own = document(1);
    let added = TempPdf::new("added.pdf", &pages(&[Page::new(text_lines(&["Added page as it was"]))]));
    let was = recto_platform::file_stamp(&added.0).unwrap();
    // Rewritten since, longer, in place.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&added.0, pages(&[Page::new(text_lines(&["Something else entirely, at the same page number"]))])).unwrap();
    let events = Arc::new(Events::default());
    let engine = Engine::new(EngineConfig::default(), events);
    engine.set_viewport(view(0.0));
    let map = PageMap {
        files: vec![own.0.clone(), added.0.clone()],
        slots: Some(vec![(1, 0), (0, 0)]),
        own: None,
        turns: None,
        expected: vec![None, Some(was)],
        follows: None,
    };
    engine.activate(1, 2, 0, map, &[], None, recto_reader::layout::Spread::Single);
    engine.set_viewport(view(0.0));
    let deadline = Instant::now() + HANG;
    while engine.note().is_none() && Instant::now() < deadline {
        pump(&engine, Duration::from_millis(200));
    }
    assert!(
        engine.note().is_some_and(|note| note.contains("changed or moved since it was added")),
        "{:?}",
        engine.note()
    );
    // With the file as it was, it draws.
    let fresh = TempPdf::new("added2.pdf", &pages(&[Page::new(text_lines(&["Added page as it was"]))]));
    let stamp = recto_platform::file_stamp(&fresh.0).unwrap();
    let engine = Engine::new(EngineConfig::default(), Arc::new(Events::default()));
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        2,
        0,
        PageMap {
            files: vec![own.0.clone(), fresh.0.clone()],
            slots: Some(vec![(1, 0), (0, 0)]),
            own: None,
            turns: None,
            expected: vec![None, Some(stamp)],
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    assert_eq!(engine.note(), None);
    assert!(engine.report().0["tiles"]["attached"].as_u64().unwrap() >= 1);
}

/// Failure: a snapshot box dragged from one page across the break onto the next reads the pointer in the next page's coordinates, so the box jumps to near the top of the pressed page, or comes out with the wrong size. The point must be taken onto the pressed page and held to its edge.
#[test]
fn a_snapshot_box_dragged_across_a_page_break_stays_on_its_page() {
    use recto_reader::snapshot::{SnapshotTool, Taken};
    let file = document(2);
    let (engine, _events) = started(&file, 2);
    settled_on_page(&engine, 0);
    let (press_x, press_y) = engine.document_point(0, 100.0, 700.0).unwrap();
    let (drag_x, drag_y) = engine.document_point(1, 300.0, 50.0).unwrap();
    assert_eq!(
        engine.point_on_page(drag_x, drag_y).map(|(point, _)| point.page),
        Some(1),
        "the drag really is over the next page"
    );
    let mut tool = SnapshotTool::default();
    tool.set_on(true);
    let (pressed, size) = engine.point_on_page(press_x, press_y).unwrap();
    tool.press(pressed, size);
    let point = engine.point_on_given_page(tool.pressed_page().unwrap(), drag_x, drag_y).unwrap();
    assert!(point.y > size.1, "past the pressed page's bottom: {point:?}");
    tool.drag(point);
    let Taken::Box { page, rect } = tool.release() else { panic!("a box") };
    assert_eq!(page, 0);
    assert!(
        (rect.y - 700.0).abs() < 0.5 && (rect.y + rect.height - size.1).abs() < 0.5,
        "from the press down to the page's edge: {rect:?}"
    );
    assert!((rect.x - 100.0).abs() < 0.5 && (rect.width - 200.0).abs() < 0.5, "{rect:?}");
}

/// A tab's pages, all of its one file's.
fn tab_map(file: &TempPdf) -> PageMap {
    PageMap {
        files: vec![file.0.clone()],
        slots: None,
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    }
}

/// Tab 1 drawn at its top, moved far down with nothing drawn there and left for tab 2: a `goto` or a quick scroll and a change of tab before the renderer answers. Returns the engine with drawing let go again, and tab 1's pages owed a picture there.
fn left_at_a_place_not_drawn(engine: &Engine, first: &TempPdf, second: &TempPdf, second_pages: u32) -> Vec<u32> {
    pump(engine, HANG);
    assert!(settles(engine), "{}", engine.report().0);
    engine.hold_drawing(true);
    engine.set_viewport(view(30_000.0));
    engine.activate(2, second_pages, 0, tab_map(second), &[1], None, recto_reader::layout::Spread::Single);
    engine.set_viewport(view(0.0));
    let left = engine.sleep_material(1, &tab_map(first)).expect("tab 1 is held off screen");
    assert!(!left.in_view.is_empty() && !left.in_view.contains(&0), "{:?}", left.in_view);
    assert!(
        left.pictures.iter().all(|picture| !left.in_view.contains(&picture.page)),
        "nothing's drawn at its place yet: a sleep file now would wake it bare"
    );
    engine.hold_drawing(false);
    left.in_view
}

/// Failure (owner, 2026-11-13: "I think it makes sense, and extra draw is not costly right?"): a tab left at a place it has no picture of sleeps without one and wakes to bare paper until the renderer catches up. On a real worker: once the tab showing is drawn, the tab left has a picture of every page at its place, at the preview's scale, in what its sleep file takes, and its worker's still the one it had.
#[test]
fn a_tab_left_at_a_place_not_drawn_sleeps_with_a_picture_of_it() {
    let (first, second) = (document(40), document(3));
    let (engine, _events) = started(&first, 40);
    let owed = left_at_a_place_not_drawn(&engine, &first, &second, 3);
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    sleep_draws_end(&engine);
    let material = engine.sleep_material(1, &tab_map(&first)).expect("tab 1 is held off screen");
    let pictured: Vec<u32> = material
        .pictures
        .iter()
        .map(|picture| picture.page)
        .filter(|page| owed.contains(page))
        .collect();
    assert_eq!(pictured, owed, "{}", engine.report().0["sleep_draws"]);
    assert!(
        material
            .pictures
            .iter()
            .filter(|picture| owed.contains(&picture.page))
            .all(|picture| picture.bgra.iter().any(|byte| *byte < 0x80)),
        "with ink in them: really drawn"
    );
    let draws = engine.report().0["sleep_draws"].clone();
    assert_eq!(draws["ended"]["done"], 1, "{draws}");
    assert_eq!(draws["ended"]["no_worker"], 0, "{draws}");
}

/// Failure: a tab off screen whose worker has left (after 30 s with nothing to do, as a warm tab's does) gets one started again to draw its place, a process and its memory for a picture. Nothing's drawn and no worker is started: the asking ends.
#[test]
fn a_tab_whose_worker_has_gone_gets_no_worker_started_for_its_place() {
    let (first, second) = (document(40), document(3));
    let events = Arc::new(Events::default());
    let policy = recto_render_service::Policy {
        idle_exit: Duration::from_millis(300),
        ..recto_render_service::Policy::default()
    };
    let engine = Engine::new(
        EngineConfig {
            service: None,
            policy: Some(policy),
            keys: None,
        },
        events,
    );
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        40,
        0,
        PageMap {
            files: vec![first.0.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let deadline = Instant::now() + HANG;
    // The list of workers is said again only when the engine next turns, so it's whether the process is still there that's looked at.
    while engine.workers_footprint().0 > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20)); // ratchet: poll
    }
    assert_eq!(engine.workers_footprint().0, 0, "tab 1's worker left by itself");
    left_at_a_place_not_drawn(&engine, &first, &second, 3);
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let draws = sleep_draws_end(&engine);
    assert_eq!(draws["ended"]["no_worker"], 1, "{draws}");
    assert_eq!(draws["pictures"], 0, "{draws}");
    assert!(engine.worker_pids().len() <= 1, "tab 2's at most: {:?}", engine.worker_pids());
}

/// An engine on tab 1, a document of `count` pages, whose workers go by `policy` and share `slot`.
fn engine_with(file: &TempPdf, count: u32, policy: recto_render_service::Policy, slot: &recto_render_service::HeavySlot) -> Engine {
    let service = recto_render_service::ServiceConfig {
        heavy_slot: slot.clone(),
        ..recto_render_service::ServiceConfig::discover().expect("the worker's here")
    };
    let engine = Engine::new(
        EngineConfig {
            service: Some(service),
            policy: Some(policy),
            keys: None,
        },
        Arc::new(Events::default()),
    );
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        count,
        0,
        PageMap {
            files: vec![file.0.clone()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    engine.set_viewport(view(0.0));
    engine
}

/// Waits, for as long as [`HANG`], for the drawing for a tab left to end, however it ends, and says how it went.
fn sleep_draws_end(engine: &Engine) -> serde_json::Value {
    let deadline = Instant::now() + HANG;
    loop {
        engine.drain();
        engine.applied();
        let draws = engine.report().0["sleep_draws"].clone();
        if draws["under_way"] == false || Instant::now() >= deadline {
            return draws;
        }
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// Ten pages of text, then thirty that each draw `content` as a form of their own: what's at the place a test leaves tab 1 at.
fn text_then(content: &[u8]) -> TempPdf {
    let list: Vec<Page> = (0..40)
        .map(|index| {
            if index < 10 {
                Page::new(text_lines(&[&format!("Page number {index}")]))
            } else {
                Page::new(b"q /S Do Q".to_vec())
            }
        })
        .collect();
    let form = first_extra_object(list.len());
    TempPdf::new(
        "place.pdf",
        &pages_with(
            &list,
            &format!("/XObject << /S {form} 0 R >>"),
            vec![Object::Stream {
                dict: "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /FormType 1".into(),
                data: content.to_vec(),
            }],
        ),
    )
}

/// Failure (owner, 2026-11-13: "no file is opened again for it"): a worker past its retirement line waits to go before its next job for another page, and a draw for a tab left was that job: it buried the worker and started a new one that opened the file again, to draw a picture nobody's looking at. With every worker past its line, nothing new is started for tab 1's file, and its worker still goes when the engine rests, not at its idle exit.
#[test]
fn a_worker_waiting_to_retire_is_not_replaced_to_draw_a_place_left() {
    let (first, second) = (document(40), document(1));
    let policy = recto_render_service::Policy {
        retire_bytes: 1,
        idle_exit: Duration::from_secs(600),
        ..recto_render_service::Policy::default()
    };
    let engine = engine_with(&first, 40, policy, &recto_render_service::HeavySlot::new());
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let before = engine.worker_pids();
    assert_eq!(before.len(), 1, "tab 1's, waiting to retire: {before:?}");
    left_at_a_place_not_drawn(&engine, &first, &second, 1);
    let draws = sleep_draws_end(&engine);
    let new: Vec<u32> = engine.worker_pids().into_iter().filter(|pid| !before.contains(pid)).collect();
    assert!(new.len() <= 1, "only tab 2's is new: {new:?}, {draws}");
    assert_eq!(draws["ended"]["no_worker"], 1, "{draws}");
    let deadline = Instant::now() + HANG;
    while engine.worker_pids().contains(&before[0]) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20)); // ratchet: poll
    }
    assert!(
        !engine.worker_pids().contains(&before[0]),
        "tab 1's worker went at rest, long before its idle exit"
    );
}

/// Failure: a page that's too much for a normal worker, at the place a tab was left, went on to a heavy one (up to a gigabyte, and the one slot the person's own heavy page needs) for a picture nobody's looking at. Nothing for it takes the slot.
#[test]
fn a_page_that_needs_a_heavy_worker_gets_none_for_a_place_left() {
    let mib = 1024 * 1024;
    let (first, second) = (text_then(b"q 1 0 0 1 0 0 cm /S Do Q q 1 0 0 1 5 0 cm /S Do Q"), document(1));
    let policy = recto_render_service::Policy {
        memory_limit_bytes: 64 * mib,
        heavy_limit_bytes: 128 * mib,
        heavy_min_free_percent: 0,
        ..recto_render_service::Policy::default()
    };
    let slot = recto_render_service::HeavySlot::new();
    let engine = engine_with(&first, 40, policy, &slot);
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    assert_eq!(slot.times_taken(), 0, "the pages at the top are light");
    left_at_a_place_not_drawn(&engine, &first, &second, 1);
    let draws = sleep_draws_end(&engine);
    pump(&engine, HANG);
    assert_eq!(slot.times_taken(), 0, "the slot was taken for a tab nobody's looking at: {draws}");
    assert_eq!(draws["pictures"], 0, "{draws}");
}

/// Failure: a worker killed while drawing for a tab left (a crash, or the system's memory killer) was tried again in a new one, a process and the file opened again for a picture nobody's looking at. Nothing new is started for tab 1's file.
#[test]
fn a_worker_killed_drawing_a_place_left_is_not_started_again() {
    let (first, second) = (text_then(&slow_page(200_000)), document(1));
    let policy = recto_render_service::Policy {
        page_deadline: Duration::from_secs(120),
        ..recto_render_service::Policy::default()
    };
    let engine = engine_with(&first, 40, policy, &recto_render_service::HeavySlot::new());
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let before = engine.worker_pids();
    assert_eq!(before.len(), 1, "{before:?}");
    left_at_a_place_not_drawn(&engine, &first, &second, 1);
    // Once its first page's size and weight are in, its preview's being drawn, slowly.
    let deadline = Instant::now() + HANG;
    while engine.report().0["sleep_draws"]["jobs"].as_u64().unwrap_or(0) < 3 && Instant::now() < deadline {
        engine.drain();
        engine.applied();
        std::thread::sleep(Duration::from_millis(2)); // ratchet: poll
    }
    assert!(
        engine.report().0["sleep_draws"]["jobs"].as_u64().unwrap_or(0) >= 3,
        "{}",
        engine.report().0["sleep_draws"]
    );
    let killed = std::process::Command::new("/bin/kill").arg("-9").arg(before[0].to_string()).status().unwrap();
    assert!(killed.success());
    let draws = sleep_draws_end(&engine);
    let new: Vec<u32> = engine.worker_pids().into_iter().filter(|pid| !before.contains(pid)).collect();
    assert!(new.len() <= 1, "only tab 2's is new: {new:?}, {draws}");
    assert_eq!(draws["ended"]["no_worker"], 1, "{draws}");
}

/// Ten pages of text, then thirty with a photograph over their top left quarter, of red and blue, which dark paper keeps as it is while it turns the rest over. The photograph is `edge` pixels square, run-length coded so the file stays small, and big enough decoded that drawing it takes its worker past a retirement line text leaves it under.
fn text_then_photos(edge: usize) -> TempPdf {
    let list: Vec<Page> = (0..40)
        .map(|index| {
            if index < 10 {
                Page::new(text_lines(&[&format!("Page number {index}")]))
            } else {
                Page::new(b"q 306 0 0 396 0 396 cm /Im0 Do Q".to_vec())
            }
        })
        .collect();
    // Squares of 50 pixels, each colour a run: a length byte of 257 less the run, then the colour's index.
    let mut data = Vec::new();
    for y in 0..edge {
        let mut x = 0;
        while x < edge {
            let colour = ((x / 50 + y / 50) % 2) as u8;
            let run = (50 - x % 50).min(edge - x).min(128);
            data.extend_from_slice(&[(257 - run) as u8, colour]);
            x += run;
        }
    }
    data.push(128);
    let image = first_extra_object(list.len());
    TempPdf::new(
        "photos.pdf",
        &pages_with(
            &list,
            &format!("/XObject << /Im0 {image} 0 R >>"),
            vec![Object::Stream {
                dict: format!(
                    "/Type /XObject /Subtype /Image /Width {edge} /Height {edge} /BitsPerComponent 8 /ColorSpace [/Indexed /DeviceRGB 1 <FF00000000FF>] /Filter /RunLengthDecode"
                ),
                data,
            }],
        ),
    )
}

/// The colours in the tiles these ops attach to these pages, red, green and blue, each once: every pixel of a surface, or a tile of one colour's colour.
#[allow(unsafe_code)]
fn tile_colours_on(ops: &[Op], pages: &[u32]) -> std::collections::BTreeSet<[u8; 3]> {
    let bytes = |rgb: u32| [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    let mut colours = std::collections::BTreeSet::new();
    for op in ops {
        match op {
            Op::Attach { page, surface: 0, rgb, .. } if pages.contains(page) => colours.extend(rgb.map(bytes)),
            Op::Attach { page, surface, .. } if pages.contains(page) => {
                // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                let Some(surface) = (unsafe { recto_platform::surface::PixelSurface::from_host_reference(*surface) }) else {
                    continue;
                };
                let _ = surface.with_pixels(|pixels| colours.extend(pixels.chunks_exact(4).map(|pixel| [pixel[2], pixel[1], pixel[0]])));
            }
            _ => {}
        }
    }
    colours
}

/// Whether tiles with these colours were put on dark paper by a look decided with the page's images known: the paper around the photograph turned over to the paper's tone, and none of it dimmed white, as a page judged whole, with nothing known of its images, is.
fn turned_over_around_the_photo(colours: &std::collections::BTreeSet<[u8; 3]>) -> bool {
    let bytes = |rgb: u32| [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    colours.contains(&bytes(PaperKind::Dark.rgb())) && !colours.contains(&bytes(dim_rgb(0xFF_FF_FF)))
}

/// What a failure says of these colours: how many, and whether the paper's tone and dimmed white are among them.
fn said(colours: &std::collections::BTreeSet<[u8; 3]>) -> String {
    let bytes = |rgb: u32| [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    format!(
        "{} colours, the paper's tone {}, dimmed white {}",
        colours.len(),
        colours.contains(&bytes(PaperKind::Dark.rgb())),
        colours.contains(&bytes(dim_rgb(0xFF_FF_FF)))
    )
}

/// Tab 1 of `file` shown again where it was left, on dark paper: the colours of what the host is told to attach to `pages`.
fn shown_again_at_its_place(engine: &Engine, file: &TempPdf, pages: &[u32]) -> std::collections::BTreeSet<[u8; 3]> {
    engine.activate(1, 40, 0, tab_map(file), &[2], None, recto_reader::layout::Spread::Single);
    let mut ops = engine.set_viewport(view(30_000.0));
    engine.applied();
    ops.extend(pump(engine, HANG));
    assert!(settles(engine), "{}", engine.report().0);
    tile_colours_on(&ops, pages)
}

/// What a fresh engine that never drew anything for a tab left puts on dark paper for `pages` of `file`, at the place the tests leave tab 1 at: the control.
fn drawn_fresh_at_the_place(file: &TempPdf, pages: &[u32]) -> std::collections::BTreeSet<[u8; 3]> {
    let engine = engine_with(file, 40, recto_render_service::Policy::default(), &recto_render_service::HeavySlot::new());
    engine.set_paper(PaperKind::Dark);
    let mut ops = engine.set_viewport(view(30_000.0));
    engine.applied();
    ops.extend(pump(&engine, HANG));
    assert!(settles(&engine), "{}", engine.report().0);
    tile_colours_on(&ops, pages)
}

/// Failure (review round 2, B1): on dark paper, a preview drawn for a tab left takes its worker past its retirement line (a page with a photograph, as is common), and the page's image boxes, asked straight after in the same job, were refused as though for another page. The look decided without them (the page judged whole, so dimmed) was kept for the page, and the person's own tiles of it went by that look when they came back: the page a dimmed white instead of turned over around its photograph, until the look was cleared. Back at its place, tab 1's tiles of the page drawn for it are turned over as a fresh engine's are.
#[test]
fn a_page_drawn_for_a_tab_left_by_a_worker_past_its_line_keeps_its_look_on_dark_paper() {
    let mib = 1024 * 1024;
    let (first, second) = (text_then_photos(3000), document(1));
    // Text leaves a worker at about 6 MiB, and drawing the photograph takes it to about 16 (measured, debug).
    let policy = recto_render_service::Policy {
        retire_bytes: 11 * mib,
        idle_exit: Duration::from_secs(600),
        ..recto_render_service::Policy::default()
    };
    let engine = engine_with(&first, 40, policy, &recto_render_service::HeavySlot::new());
    engine.set_paper(PaperKind::Dark);
    let owed = left_at_a_place_not_drawn(&engine, &first, &second, 1);
    let draws = sleep_draws_end(&engine);
    let material = engine.sleep_material(1, &tab_map(&first)).expect("tab 1 is held off screen");
    let pictured: Vec<u32> = material
        .pictures
        .iter()
        .map(|picture| picture.page)
        .filter(|page| owed.contains(page))
        .collect();
    assert!(
        !pictured.is_empty(),
        "the first page at its place was drawn, by the worker that was there: {draws}"
    );
    let fresh = drawn_fresh_at_the_place(&first, &pictured);
    assert!(turned_over_around_the_photo(&fresh), "the control: {}", said(&fresh));
    let back = shown_again_at_its_place(&engine, &first, &pictured);
    assert!(!back.is_empty(), "tab 1's page {pictured:?} was shown");
    assert!(
        turned_over_around_the_photo(&back),
        "as a fresh draw has it ({}): {}, {draws}",
        said(&fresh),
        said(&back)
    );
}

/// Failure (review round 2, B1): a preview drawn for a tab left whose worker went in the moment after it was drawn (the system's memory killer, a crash) had the page's image boxes and size refused, as no worker's started for it, and its look decided without them (the page judged whole, so dimmed) was kept for the page: the person's own tiles of it went by that look when they came back, and the picture put on dark paper by it went into the sleep file. Nothing decided from a refused input is kept: the asking ends for want of a worker, there's no picture of the page, and back at its place its tiles are turned over as a fresh engine's are. The worker is killed while the preview's answer waits at its seam, before the image boxes are asked.
#[test]
fn a_look_refused_for_want_of_a_worker_is_kept_for_nothing() {
    use recto_platform::slow::{Seam, gate};
    let (first, second) = (text_then_photos(64), document(1));
    let engine = engine_with(&first, 40, recto_render_service::Policy::default(), &recto_render_service::HeavySlot::new());
    engine.set_paper(PaperKind::Dark);
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let tab_1s = engine.worker_pids();
    assert_eq!(tab_1s.len(), 1, "{tab_1s:?}");
    gate::hold(&[Seam::Worker]);
    let owed = left_at_a_place_not_drawn(&engine, &first, &second, 1);
    // Every worker's answer is let through, one at a time, until the tab left's third job, its first page's preview after its size and weight, has its answer waiting.
    let deadline = Instant::now() + HANG;
    loop {
        engine.drain();
        engine.applied();
        let jobs = engine.report().0["sleep_draws"]["jobs"].as_u64().unwrap_or(0);
        let waiting = gate::waiting(Seam::Worker);
        if jobs >= 3 && waiting > 0 {
            break;
        }
        if waiting > 0 {
            gate::let_through(Seam::Worker, 1);
        }
        assert!(Instant::now() < deadline, "{}", engine.report().0["sleep_draws"]);
        std::thread::sleep(Duration::from_millis(2)); // ratchet: poll
    }
    assert_eq!(engine.report().0["sleep_draws"]["jobs"], 3, "{}", engine.report().0["sleep_draws"]);
    let killed = std::process::Command::new("/bin/kill").arg("-9").arg(tab_1s[0].to_string()).status().unwrap();
    assert!(killed.success());
    // Gone, not just signalled: no process, or a zombie till it's collected.
    let gone = || {
        let stat = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &tab_1s[0].to_string()])
            .output()
            .unwrap()
            .stdout;
        let stat = String::from_utf8_lossy(&stat);
        stat.trim().is_empty() || stat.trim().starts_with('Z')
    };
    while !gone() {
        assert!(Instant::now() < deadline, "tab 1's worker didn't go");
        std::thread::sleep(Duration::from_millis(2)); // ratchet: poll
    }
    gate::open(Seam::Worker);
    let draws = sleep_draws_end(&engine);
    assert_eq!(draws["ended"]["no_worker"], 1, "{draws}");
    assert_eq!(
        draws["pictures"], 0,
        "a picture put on dark paper by a look decided without the page's images: {draws}"
    );
    let material = engine.sleep_material(1, &tab_map(&first)).expect("tab 1 is held off screen");
    assert!(material.pictures.iter().all(|picture| !owed.contains(&picture.page)), "{draws}");
    let fresh = drawn_fresh_at_the_place(&first, &owed[..1]);
    assert!(turned_over_around_the_photo(&fresh), "the control: {}", said(&fresh));
    let back = shown_again_at_its_place(&engine, &first, &owed[..1]);
    assert!(
        turned_over_around_the_photo(&back),
        "as a fresh draw has it ({}): {}",
        said(&fresh),
        said(&back)
    );
}

/// Failure (review round 2, F12): the shown tab's worker holding a heavy page's memory was let go only at rest, so while drawing for a tab left went on it held its hundreds of megabytes for the whole of that asking (up to 5 s and a page's deadline) before its own rest even began. It goes once it's rested as long as a heavy worker's idle rule (100 ms here), between the jobs for the tab left, while their asking is still going. The tab left's jobs are let through one at a time, each over 100 ms after the one before, so the asking lasts as long as the test says.
#[test]
fn the_tab_showing_s_big_worker_rests_on_time_while_a_tab_left_is_drawn() {
    use recto_platform::slow::{Seam, gate};
    let mib = 1024 * 1024;
    let rest = Duration::from_millis(100);
    let (first, second) = (document(40), text_then_photos(3000));
    // Text leaves a worker at about 6 MiB, and drawing the photograph takes it to about 16 (measured, debug): the tab showing's is big, the tab left's isn't.
    let policy = recto_render_service::Policy {
        retire_bytes: 11 * mib,
        heavy_idle_exit: rest,
        ..recto_render_service::Policy::default()
    };
    let engine = engine_with(&first, 40, policy, &recto_render_service::HeavySlot::new());
    pump(&engine, HANG);
    assert!(settles(&engine), "{}", engine.report().0);
    let tab_1s = engine.worker_pids();
    assert_eq!(tab_1s.len(), 1, "{tab_1s:?}");
    gate::hold(&[Seam::Worker]);
    // Tab 1 moved far down with nothing drawn there and left for tab 2, shown at its photographs.
    engine.hold_drawing(true);
    engine.set_viewport(view(30_000.0));
    engine.activate(2, 40, 0, tab_map(&second), &[1], None, recto_reader::layout::Spread::Single);
    engine.set_viewport(view(30_000.0));
    engine.hold_drawing(false);
    let deadline = Instant::now() + HANG;
    let mut let_go = Instant::now();
    let tab_2s = loop {
        engine.drain();
        engine.applied();
        let draws = engine.report().0["sleep_draws"].clone();
        let waiting = gate::waiting(Seam::Worker);
        // Tab 2's had everything, and the first job for tab 1 has its answer waiting.
        if draws["jobs"].as_u64().unwrap_or(0) >= 1 && waiting > 0 {
            break engine.worker_pids().into_iter().filter(|pid| !tab_1s.contains(pid)).collect::<Vec<_>>();
        }
        if waiting > 0 {
            gate::let_through(Seam::Worker, 1);
            let_go = Instant::now();
        }
        assert!(Instant::now() < deadline, "{draws}");
        std::thread::sleep(Duration::from_millis(2)); // ratchet: poll
    };
    assert_eq!(tab_2s.len(), 1, "tab 2's, big: {tab_2s:?} beside tab 1's {tab_1s:?}");
    let footprint = engine.workers_footprint().0;
    while engine.worker_pids().contains(&tab_2s[0]) {
        let draws = engine.report().0["sleep_draws"].clone();
        assert_eq!(
            draws["under_way"], true,
            "tab 2's worker was still there when the asking ended ({footprint} bytes of workers at its start): {draws}"
        );
        if gate::waiting(Seam::Worker) > 0 && let_go.elapsed() > rest {
            gate::let_through(Seam::Worker, 1);
            let_go = Instant::now();
        }
        engine.drain();
        engine.applied();
        assert!(Instant::now() < deadline, "{draws}");
        std::thread::sleep(Duration::from_millis(2)); // ratchet: poll
    }
    gate::open(Seam::Worker);
    assert!(engine.worker_pids().contains(&tab_1s[0]), "tab 1's worker isn't big, and stays");
}
