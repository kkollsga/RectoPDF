//! Dark paper's judgement on real files, sharp and at a lower resolution, through the real worker. Skipped where the benchmark files aren't, as on CI: `paper.rs` holds the rule without them, and `tests/engine.rs` holds it on a made picture.

use recto_platform::surface::PixelSurface;
use recto_reader::engine::EngineEvents;
use recto_reader::paper::{DARK_PAPER, dim_rgb};
use recto_reader::tiles::{Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_render_service::Policy;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BENCH: &str = "/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100";

struct Quiet;
impl EngineEvents for Quiet {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
}

fn view(y: f64) -> Viewport {
    Viewport { x: 0.0, y, width: 1168.0, height: 900.0, frame_width: 1168.0, magnification: 1.0, backing: 2.0, live: false, scrolled_to: u64::MAX }
}

/// What's at the top left of the page, as red, green, blue, on dark paper: the colour of a tile with none of its own, or a surface's first pixel. And whether it came out at a lower resolution.
#[allow(unsafe_code)]
fn corner_on_dark_paper(file: &Path, pages: u32, page: u32, policy: Policy) -> (u32, bool) {
    let engine = Engine::new(EngineConfig { service: None, policy: Some(policy) }, Arc::new(Quiet));
    engine.set_viewport(view(0.0));
    engine.set_paper(PaperKind::Dark);
    engine.activate(1, pages, page, PageMap { files: vec![file.to_path_buf()], slots: None, own: None, turns: None, expected: Vec::new() }, &[], None, recto_reader::layout::Spread::Single);
    let mut corner = None;
    let started = Instant::now();
    let mut y = 0.0;
    while started.elapsed() < Duration::from_secs(60) {
        let mut ops = engine.set_viewport(view(y));
        ops.extend(engine.drain());
        engine.applied();
        for op in ops {
            match op {
                Op::ScrollTo { y: to, .. } => y = to,
                Op::Attach { x, y: top, surface, rgb, page: on, .. } => {
                    // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                    let surface = unsafe { PixelSurface::from_host_reference(surface) };
                    if on == page && x == 0.0 && top == 0.0 {
                        let colour = surface.and_then(|surface| surface.with_pixels(|pixels| (u32::from(pixels[2]) << 16) | (u32::from(pixels[1]) << 8) | u32::from(pixels[0])).ok());
                        corner = colour.or(rgb).or(Some(DARK_PAPER));
                    }
                }
                _ => {}
            }
        }
        if engine.wait_settled(Duration::from_millis(10)) {
            break;
        }
    }
    let report = engine.report().0;
    assert_eq!(report["settled"], true, "{report}");
    // A blank tile of the paper's own colour needs nothing put on a layer, so nothing's said about it.
    (corner.unwrap_or(DARK_PAPER), !report["low_res_pages"].as_array().unwrap().is_empty())
}

fn file(name: &str) -> Option<PathBuf> {
    let path = Path::new(BENCH).join(name);
    path.is_file().then_some(path)
}

/// Failure: a scanned page shows as a bright dimmed page beside text pages turned over, as the owner saw, or a map goes negative, its band mint and its white margins black: on a real scan and a real map. Sharp, and at a lower resolution where the images aren't asked about and the page is judged whole, as the two must agree. The scan's paper lands on the dark paper's own tone, and the map's white margin is only dimmed.
#[test]
fn a_real_scan_turns_over_and_a_real_map_is_dimmed() {
    let (Some(scan), Some(map)) = (file("gov_nasa_ntrs20150023275.pdf"), file("gov_usgs_usgs70277338.pdf")) else {
        eprintln!("skipped: the benchmark files aren't here");
        return;
    };
    let low = |limit: u64| Policy { memory_limit_bytes: limit * 1024 * 1024, heavy_limit_bytes: limit * 1024 * 1024, ..Policy::default() };

    let (corner, low_res) = corner_on_dark_paper(&scan, 8, 2, Policy::default());
    assert!(!low_res);
    assert_eq!(corner, DARK_PAPER, "the scan's paper is the paper's tone: {corner:06x}");

    let (corner, low_res) = corner_on_dark_paper(&map, 1, 0, Policy::default());
    assert!(!low_res);
    assert_eq!(corner, dim_rgb(0xFF_FF_FF), "the map's margin, true and dimmed: {corner:06x}");
    let (corner, low_res) = corner_on_dark_paper(&map, 1, 0, low(350));
    assert!(low_res, "at this limit it's a lower-resolution page");
    assert_eq!(corner, dim_rgb(0xFF_FF_FF), "the same, judged whole: {corner:06x}");

    // A made scan's lower-resolution version goes the same way: this one's image needs little, so a small limit does it.
    let (corner, low_res) = corner_on_dark_paper(&scan, 8, 2, low(48));
    if low_res {
        assert_eq!(corner, DARK_PAPER, "judged whole: {corner:06x}");
    } else {
        eprintln!("the scan fitted in that limit, so its lower-resolution path wasn't reached");
    }
}
