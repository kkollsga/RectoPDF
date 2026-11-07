//! A worker drawing straight into a surface the host made: the path the app's pages take, on the real worker.

use recto_platform::surface::PixelSurface;
use recto_render_service::{DocRenderer, Policy, ServiceConfig, Surface, TILE_EDGE, TileRect, TileRequest, TileResult};
use recto_testpdf::{Page, TempPdf, busy_page, pages, text_lines};

fn doc(file: &TempPdf) -> DocRenderer {
    DocRenderer::new(&file.0, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap()
}

fn tile_rect() -> TileRect {
    TileRect { x: 0, y: 0, width: TILE_EDGE, height: TILE_EDGE }
}

/// Failure: the surface reaches the worker but the pixels don't reach the surface, or come out different from the path everything else uses, so what's on screen isn't what `recto render` draws.
#[test]
fn a_tile_drawn_into_a_surface_is_the_same_as_one_in_shared_memory() {
    let file = TempPdf::new("surface.pdf", &pages(&[Page::new(text_lines(&["Words on a page, and more words, drawn twice"]))]));
    let mut doc = doc(&file);
    let surface = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
    let request = TileRequest { page: 0, rect: tile_rect(), scale: 2.0, stride: surface.stride(), generation: 1 };
    assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    let drawn: Vec<u8> = surface.with_pixels(|pixels| pixels[..(surface.stride() * TILE_EDGE) as usize].to_vec()).unwrap();
    assert!(drawn.chunks(4).any(|pixel| pixel[0] < 200), "there's ink in it");
    let memory = Surface::create((surface.stride() * TILE_EDGE) as usize).unwrap();
    assert!(matches!(doc.render_tile(&request, &memory).unwrap(), TileResult::Done(_)));
    let mut same = vec![0u8; drawn.len()];
    assert!(memory.copy_out(0, &mut same));
    // The last byte of each pixel isn't colour, and only colour has to agree.
    let colours = |bytes: &[u8]| bytes.chunks(4).map(|pixel| [pixel[0], pixel[1], pixel[2]]).collect::<Vec<_>>();
    assert!(colours(&drawn) == colours(&same), "a surface and shared memory differ");
}

/// Failure: a region can't be drawn into a surface, so a heavy page has no zero-copy path, or its padded rows are read wrongly.
#[test]
fn a_region_draws_into_a_surface_with_its_own_row_padding() {
    let file = TempPdf::new("region.pdf", &pages(&[Page::new(busy_page())]));
    let mut doc = doc(&file);
    let rect = TileRect { x: 0, y: 0, width: TILE_EDGE * 2 + 13, height: TILE_EDGE * 2 };
    let surface = PixelSurface::create(rect.width, rect.height).unwrap();
    let request = TileRequest { page: 0, rect, scale: 2.0, stride: surface.stride(), generation: 1 };
    let result = doc.render_region(&request, TILE_EDGE, &surface).unwrap();
    let TileResult::Done(tile) = result else { panic!("{result:?}") };
    assert_eq!(tile.region_kinds.len(), 6, "3 columns by 2 rows, the last column narrow");
}

/// Failure: a worker that's been replaced still has the old link, or a replaced worker gets no link, so the next page draws nothing after one was killed.
#[test]
fn a_new_worker_gets_a_link_of_its_own() {
    let file = TempPdf::new("link.pdf", &pages(&[Page::new(text_lines(&["A page"]))]));
    let mut doc = doc(&file);
    let surface = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
    let request = TileRequest { page: 0, rect: tile_rect(), scale: 1.0, stride: surface.stride(), generation: 1 };
    assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    let first = doc.worker_pid().unwrap();
    doc.kill_worker();
    assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    assert_ne!(doc.worker_pid().unwrap(), first);
}

/// Failure: tiles are charged to the worker, where they'd count against its memory limit and go with it, or the host copies them. A surface is the host's: many live ones must cost the worker nothing and cost the host about their size.
/// Whole-process footprints, in this test's own process, and only properties of them.
#[test]
fn surfaces_cost_the_host_and_not_the_worker() {
    let file = TempPdf::new("charged.pdf", &pages(&[Page::new(text_lines(&["A page"]))]));
    let mut doc = doc(&file);
    let probe = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
    let request = TileRequest { page: 0, rect: tile_rect(), scale: 1.0, stride: probe.stride(), generation: 1 };
    doc.render_tile(&request, &probe).unwrap();
    let worker_before = doc.worker_footprint().unwrap();
    let host_before = recto_platform::footprint_bytes().unwrap();
    let live: Vec<PixelSurface> = (0..64)
        .map(|_| {
            let surface = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
            doc.render_tile(&TileRequest { stride: surface.stride(), ..request }, &surface).unwrap();
            surface
        })
        .collect();
    let host_grew = recto_platform::footprint_bytes().unwrap().saturating_sub(host_before);
    let worker_grew = doc.worker_footprint().unwrap().saturating_sub(worker_before);
    assert!(host_grew >= 32 * 1024 * 1024, "the host's footprint rose by only {host_grew}: are the tiles elsewhere?");
    assert!(worker_grew < 16 * 1024 * 1024, "the worker's rose by {worker_grew}, with 64 MiB of surfaces in the host");
    drop(live);
}

/// Failure: an off-screen tile stays in the footprint, so scrolling a long document grows memory even though nothing's on screen. Marked discardable it leaves at once, and is still there unless the Mac needed it.
#[test]
fn a_discardable_surface_leaves_the_footprint_and_comes_back() {
    let surfaces: Vec<PixelSurface> = (0..64).map(|_| PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap()).collect();
    for surface in &surfaces {
        surface.with_pixels(|pixels| pixels.fill(0x5A)).unwrap();
    }
    let held = recto_platform::footprint_bytes().unwrap();
    for surface in &surfaces {
        surface.set_discardable();
    }
    let let_go = recto_platform::footprint_bytes().unwrap();
    assert!(held.saturating_sub(let_go) >= 32 * 1024 * 1024, "{held} to {let_go}");
    for surface in &surfaces {
        if surface.reclaim() == recto_platform::surface::Purge::Kept {
            surface.with_pixels(|pixels| assert_eq!(pixels[0], 0x5A)).unwrap();
        }
    }
}

/// Failure: a worker goes by its own idle rule in the moment after it was last looked at, the surface can't be sent, and a plain "unavailable" is taken as the whole document having failed, so a tab stays failed for good. It's the same as finding out from its reply: another worker, and the job goes ahead.
#[test]
fn a_surface_that_cant_be_sent_means_a_new_worker() {
    let file = TempPdf::new("gone.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let mut doc = doc(&file);
    let surface = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
    let request = |generation| TileRequest { page: 0, rect: tile_rect(), scale: 2.0, stride: surface.stride(), generation };
    assert!(matches!(doc.render_tile(&request(1), &surface).unwrap(), TileResult::Done(_)));
    let first = doc.worker_pid().expect("a worker");
    doc.break_surface_link_for_test();
    assert!(matches!(doc.render_tile(&request(2), &surface).unwrap(), TileResult::Done(_)), "the document is fine");
    assert_ne!(doc.worker_pid(), Some(first), "by a new worker");
}

/// Failure: a compromised worker holds on to every surface it was sent. The host is charged, so no limit on the worker's own memory sees it, and it can pin as much as it's sent. Past the cap it's retired, on any page, a one-page file included.
#[test]
fn a_worker_sent_too_much_is_let_go() {
    let file = TempPdf::new("pinned.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let surface = PixelSurface::create(TILE_EDGE, TILE_EDGE).unwrap();
    let policy = Policy { surface_bytes_cap: surface.len() as u64 * 3, ..Policy::default() };
    let mut doc = DocRenderer::new(&file.0, None, ServiceConfig::discover().unwrap(), policy).unwrap();
    let mut workers = std::collections::HashSet::new();
    for generation in 1..=16 {
        let request = TileRequest { page: 0, rect: tile_rect(), scale: 2.0, stride: surface.stride(), generation };
        assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
        workers.insert(doc.worker_pid().unwrap());
    }
    assert!(workers.len() >= 2, "one worker was sent all 16: {workers:?}");
}

/// Failure: "the job went stale" and "this page has no images" both answer empty, so a tile cancelled on the way is turned over knowing nothing, and a photo is kept as its negative. Stale isn't an answer about the page.
#[test]
fn image_boxes_say_when_they_dont_know() {
    let file = TempPdf::new("boxes.pdf", &pages(&[Page::new(text_lines(&["No images here"]))]));
    let mut doc = doc(&file);
    assert_eq!(doc.image_boxes(0, 1), Some(Vec::new()), "known: there are none");
    let mut other = self::doc(&file);
    other.cancel_handle().cancel_before(5);
    assert_eq!(other.image_boxes(0, 2), None, "stale, so nobody knows");
}

