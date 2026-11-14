//! **The app still lets the system take what's off the layers.** A binary of its own, whose engines are never told
//! `Engine::qa().system_takes_only_when_told`: the switch is one for the process, and the tests that use it would hide its being left on, or the seam
//! returning early for everyone. Rule 1: what's drawn and off screen costs the app nothing while the Mac is short of memory.

use recto_reader::engine::EngineEvents;
use recto_reader::tiles::{PageMap, Viewport};
use recto_reader::{Engine, EngineConfig};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Quiet;

impl EngineEvents for Quiet {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>, _asked: u64) {}
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

/// Applies what the engine hands the host until it's settled, as a host does: only a hang reaches the limit.
fn pump(engine: &Engine) {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(60) {
        if !engine.drain().is_empty() {
            engine.applied();
        }
        if engine.wait_settled(Duration::from_millis(5)) {
            return;
        }
    }
    panic!("the engine didn't settle: {}", engine.report().0);
}

/// Failure: the one place the page view marks a surface discardable (`cache::let_system_take`) returns early in the app, as its switch for tests does, so
/// every tile and preview off screen stays in the footprint for as long as it's kept: up to the cache's 256 MiB a window. Pages drawn and scrolled past
/// leave tiles off their layers, and the kernel has been told it may have them (or has had them already).
#[test]
fn what_leaves_the_layers_is_the_systems_to_take() {
    let list: Vec<Page> = (0..6).map(|at| Page::new(text_lines(&[&format!("Page {at}")]))).collect();
    let file = TempPdf::new("marking.pdf", &pages(&list));
    let engine = Engine::new(EngineConfig::default(), Arc::new(Quiet));
    engine.set_viewport(view(0.0));
    engine.activate(
        1,
        6,
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
    pump(&engine);
    engine.set_viewport(view(4_000.0));
    pump(&engine);
    assert!(
        engine.qa().marked_for_the_system() > 0,
        "nothing kept is the system's to take: {}",
        engine.report().0
    );
}
