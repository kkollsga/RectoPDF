//! The real worker and real PDFium, from the host's side.

mod common;

use common::{kill_process, real_config, renderer, tile_surface, wait_until};
use recto_platform::isolate::process_memory;
use recto_render_service::{DocRenderer, KillReason, Policy, RenderFailure, TileKind, TileRect, TileRequest, TileResult};
use recto_render_service::{HeavySlot, Surface, Workers};
use recto_testpdf::{Page, TempPdf, busy_page, form_bomb, pages, slow_page, text_lines};
use std::time::Duration;

fn tile_request(page: u32, generation: u64) -> TileRequest {
    TileRequest {
        page,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 512,
            height: 512,
        },
        scale: 2.0,
        stride: 2048,
        generation,
    }
}

fn plain_doc() -> TempPdf {
    TempPdf::new("plain.pdf", &pages(&[Page::new(busy_page()), Page::new(text_lines(&["Second page"]))]))
}

fn draws(doc: &mut DocRenderer, page: u32) -> TileKind {
    let surface = tile_surface();
    match doc.render_tile(&tile_request(page, 0), &surface).unwrap() {
        TileResult::Done(tile) => tile.kind,
        TileResult::Cancelled { .. } => panic!("nothing cancelled this"),
    }
}

/// Failure: a damaged file stays "not a PDF" for as long as the tab lives, after the user has replaced it with a good one, because the failure was remembered for good and the old file was still the one held open. Real worker, real PDFium, and replaced the way a save does it: a new file renamed over the old.
#[test]
fn a_damaged_file_replaced_with_a_good_one_opens_with_nobody_asking() {
    let file = TempPdf::new("damaged.pdf", b"this was never a PDF");
    let mut doc = renderer(&file.0, Policy::default());
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(doc.stats().spawns, 1, "the same file gets the same answer without asking again");
    let new = file.0.with_file_name("new.pdf");
    std::fs::write(&new, pages(&[Page::new(busy_page()), Page::new(text_lines(&["Second page"]))])).unwrap();
    std::fs::rename(&new, &file.0).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
    assert!(matches!(draws(&mut doc, 1), TileKind::Grey | TileKind::Colour));
}

/// What colour a page mostly is, drawn.
fn colour_drawn(doc: &mut DocRenderer) -> (u8, u8, u8) {
    let surface = tile_surface();
    assert!(matches!(doc.render_tile(&tile_request(0, 0), &surface).unwrap(), TileResult::Done(_)));
    let mut pixel = [0u8; 4];
    assert!(surface.copy_out(4 * (256 * 512 + 256), &mut pixel));
    (pixel[2], pixel[1], pixel[0])
}

/// Failure (review, the red frames): a worker that has a file open goes on drawing it after the file's been replaced, by a save or by another app, so a tab shows and keeps the old pages. It's opened again before the next job, says the file changed, and says another version, which is what lets everything drawn of the old one be thrown away. And a file rewritten with its size, time and number put back, which no stamp can tell, is told by its version as soon as it's opened again.
#[test]
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts or rebuilds; never over a user's file.
fn a_replaced_file_is_drawn_as_it_is_now() {
    // With a file ID, as anything that writes PDFs gives one: it's what tells two versions of the same length apart.
    let colour = |rgb: &str| {
        let mut document = lopdf::Document::load_mem(&pages(&[Page::new(format!("{rgb} rg 0 0 612 792 re f").into_bytes())])).unwrap();
        document.trailer.set(
            "ID",
            lopdf::Object::Array(vec![lopdf::Object::string_literal(rgb), lopdf::Object::string_literal(rgb)]),
        );
        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    };
    let file = TempPdf::new("colour.pdf", &colour("1 0 0"));
    let mut doc = renderer(&file.0, Policy::default());
    let (red, _, blue) = colour_drawn(&mut doc);
    assert!(red > 200 && blue < 50, "red to begin with");
    let first = doc.known_version().expect("a worker says which version");
    assert!(!doc.take_file_changed());
    // Replaced, as a save does: a new file put in its place.
    let replacement = file.0.with_extension("new");
    std::fs::write(&replacement, colour("0 0 1")).unwrap();
    std::fs::rename(&replacement, &file.0).unwrap();
    let (red, _, blue) = colour_drawn(&mut doc);
    assert!(blue > 200 && red < 50, "the new one, not what was open: {red} {blue}");
    assert!(doc.take_file_changed());
    let second = doc.known_version().unwrap();
    assert_ne!(first, second);
    // Rewritten in place with the same size, its time put back: the stamp's the same. Opened again, its version isn't.
    let modified = std::fs::metadata(&file.0).unwrap().modified().unwrap();
    std::fs::write(&file.0, colour("0 1 0")).unwrap();
    std::fs::File::options().write(true).open(&file.0).unwrap().set_modified(modified).unwrap();
    doc.kill_worker();
    let drawn = doc.render_tile(&tile_request(0, 0), &tile_surface());
    assert!(doc.take_file_changed() || drawn.is_err(), "it's seen to be another");
}

/// Failure: pixels don't reach the host's surface through the shared memory, the whole point of the worker.
#[test]
fn a_tile_drawn_in_the_worker_arrives_in_the_hosts_surface() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    let surface = tile_surface();
    let result = doc.render_tile(&tile_request(0, 1), &surface).unwrap();
    assert!(matches!(&result, TileResult::Done(tile) if tile.kind == TileKind::Colour), "{result:?}");
    let mut pixels = vec![0u8; 512 * 512 * 4];
    assert!(surface.copy_out(0, &mut pixels));
    assert!(pixels.as_chunks::<4>().0.iter().any(|px| px[..3] != [255, 255, 255]), "the surface is empty");
    assert_eq!(doc.page_count().unwrap(), 2);
    let (width, height) = doc.page_size(0).unwrap();
    assert_eq!((width, height), (612.0, 792.0));
}

/// Failure: a cold tab, or one nobody has looked at, starts a process. That breaks the budget of no cost per idle tab.
#[test]
fn nothing_starts_until_a_page_is_asked_for() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    assert!(!doc.worker_pid().is_some());
    assert_eq!(doc.stats().spawns, 0);
    assert_eq!(doc.worker_pid(), None);
    draws(&mut doc, 0);
    assert!(doc.worker_pid().is_some());
    assert_eq!(doc.stats().spawns, 1);
}

/// Failure: a worker is killed mid-page, from outside, and the host dies with it, or the document stays broken. The page that was in hand
/// reports a failure, and the next page draws in a new process.
#[test]
fn a_worker_killed_mid_page_is_replaced() {
    let file = TempPdf::new("kill.pdf", &form_bomb());
    let policy = Policy {
        memory_limit_bytes: 2 << 30,
        page_deadline: Duration::from_secs(60),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    assert_eq!(doc.page_count().unwrap(), 2);
    let first_worker = doc.worker_pid().unwrap();
    // The bomb runs until something stops it, so this kill lands inside the page whenever it arrives.
    let killer = std::thread::spawn(move || {
        wait_until("the bomb to start growing", || {
            process_memory(first_worker).is_some_and(|memory| memory.footprint_bytes > 32 * 1024 * 1024)
        });
        kill_process(first_worker);
    });
    let surface = tile_surface();
    let failure = doc.render_tile(&tile_request(0, 0), &surface).unwrap_err();
    killer.join().unwrap();
    assert_eq!(
        failure,
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::Crashed
        }
    );
    assert_eq!(draws(&mut doc, 1), TileKind::Grey, "the next page draws in a new worker");
    assert_ne!(doc.worker_pid(), Some(first_worker));
    assert_eq!(doc.stats().crashes, 1);
}

/// Failure: a page that stopped every step is tried again on every scroll, each try costing seconds and hundreds of MiB.
#[test]
fn a_page_that_fails_all_the_way_down_isnt_retried_until_asked() {
    let file = TempPdf::new("again.pdf", &form_bomb());
    let policy = Policy {
        memory_limit_bytes: 64 * 1024 * 1024,
        heavy_limit_bytes: 128 * 1024 * 1024,
        heavy_min_free_percent: 0,
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    let first = doc.render_tile(&tile_request(0, 0), &surface).unwrap_err();
    assert_eq!(
        first,
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::MemoryLimit
        }
    );
    let spawns = doc.stats().spawns;
    assert_eq!(doc.render_tile(&tile_request(0, 0), &surface).unwrap_err(), first, "remembered");
    assert_eq!(doc.stats().spawns, spawns, "remembering it must not start a worker");
    doc.forget_failures();
    assert_eq!(doc.render_tile(&tile_request(0, 0), &surface).unwrap_err(), first, "tried again once asked");
    assert!(doc.stats().spawns > spawns);
}

/// Failure: a job that's already stale still starts drawing, so fast scrolling queues up work nobody will see.
#[test]
fn a_stale_job_doesnt_draw() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    let surface = tile_surface();
    doc.cancel_handle().cancel_before(5);
    let result = doc.render_tile(&tile_request(0, 4), &surface).unwrap();
    assert_eq!(result, TileResult::Cancelled { worker_killed: false });
    let mut pixels = vec![1u8; 512 * 512 * 4];
    assert!(surface.copy_out(0, &mut pixels));
    assert!(pixels.iter().all(|&byte| byte == 0), "a stale job wrote pixels");
    assert!(
        matches!(doc.render_tile(&tile_request(0, 5), &surface).unwrap(), TileResult::Done(_)),
        "the current generation still draws"
    );
}

/// Failure: cancelling a slow page neither returns nor kills, so a stale page holds the worker and the page you've scrolled to waits for it.
/// Either way out is right: the worker's pause callback returns, or the page was in a part with no pause and the kill covers it.
#[test]
fn cancelling_a_slow_page_returns_or_kill_covers_it() {
    let file = TempPdf::new("slow.pdf", &pages(&[Page::new(slow_page(400_000)), Page::new(text_lines(&["Next"]))]));
    let policy = Policy {
        page_deadline: Duration::from_secs(120),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    assert_eq!(doc.page_count().unwrap(), 2);
    let cancel = doc.cancel_handle();
    let pid = doc.worker_pid().unwrap();
    let opened = doc.stats().opened_footprint_bytes;
    let canceller = std::thread::spawn(move || {
        // Parsing this page's 400,000 objects grows the worker, so growth means the job is in hand. The page takes far longer than this.
        wait_until("the page to start loading", || {
            process_memory(pid).is_some_and(|memory| memory.footprint_bytes > opened + 1024 * 1024)
        });
        cancel.cancel_before(2);
    });
    let surface = tile_surface();
    let result = doc.render_tile(&tile_request(0, 1), &surface).unwrap();
    canceller.join().unwrap();
    assert!(
        matches!(result, TileResult::Cancelled { .. }),
        "a page this slow should still be drawing: {result:?}"
    );
    assert!(
        matches!(doc.render_tile(&tile_request(1, 2), &surface).unwrap(), TileResult::Done(_)),
        "the next job runs at once"
    );
}

/// Failure: the worker reads the file by path, so it needs file-system reach and sees a replaced file. It must use only the descriptor it
/// was given: this file no longer has a path by the time the worker starts.
#[test]
fn the_worker_reads_through_the_passed_descriptor() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    std::fs::remove_file(&file.0).unwrap();
    assert_eq!(draws(&mut doc, 0), TileKind::Colour);
}

/// Failure: a worker stays alive after its last job and an idle document keeps its memory; or the next job fails because the worker left.
#[test]
fn an_idle_worker_leaves_and_the_next_job_starts_another() {
    let file = plain_doc();
    let policy = Policy {
        idle_exit: Duration::from_millis(100),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    draws(&mut doc, 0);
    let pid = doc.worker_pid().unwrap();
    wait_until("the worker to leave by itself", || !doc.worker_pid().is_some());
    assert_eq!(process_memory(pid), None, "the process is really gone, not a zombie");
    assert_eq!(doc.stats().idle_exits, 1);
    assert_eq!(draws(&mut doc, 0), TileKind::Colour);
    assert_eq!(doc.stats().spawns, 2);
    assert_eq!(doc.stats().crashes, 0, "leaving when idle isn't a crash");
}

/// Failure: a worker past the footprint keeps going into another page, so memory only ever grows through a long document.
#[test]
fn a_worker_past_the_footprint_is_retired_before_the_next_page() {
    let file = plain_doc();
    let policy = Policy {
        retire_bytes: 1024,
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    draws(&mut doc, 0);
    let first = doc.worker_pid();
    assert_eq!(draws(&mut doc, 1), TileKind::Grey, "retiring doesn't lose the document");
    assert_ne!(doc.worker_pid(), first, "the next page should have a new worker");
    assert_eq!(doc.stats().retirements, 1);
    assert_eq!(doc.stats().spawns, 2);
    assert_eq!(doc.stats().crashes, 0, "retiring isn't a crash");
}

/// Failure: a page past the footprint by itself has its worker retired between its tiles, so every tile reloads it. Measured on a real map:
/// 72 respawns and 14 s to draw one page.
#[test]
fn tiles_of_one_page_share_a_worker_even_past_the_footprint() {
    let file = plain_doc();
    let policy = Policy {
        retire_bytes: 1024,
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    for x in 0..3 {
        let request = TileRequest {
            rect: TileRect {
                x: x * 300,
                y: 0,
                width: 300,
                height: 300,
            },
            ..tile_request(0, 0)
        };
        assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    }
    assert_eq!(doc.stats().spawns, 1);
    assert_eq!(doc.stats().retirements, 0);
    assert!(doc.worker_pid().is_some());
}

/// Failure: a request that can't be met kills the worker, so one bad scroll position costs a respawn.
#[test]
fn a_refused_job_leaves_the_worker_running() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    draws(&mut doc, 0);
    let pid = doc.worker_pid();
    let surface = tile_surface();
    assert_eq!(doc.render_tile(&tile_request(99, 0), &surface).unwrap_err(), RenderFailure::BadPage);
    assert_eq!(doc.page_size(99).unwrap_err(), RenderFailure::BadPage);
    let off_page = TileRequest {
        rect: TileRect {
            x: 100_000,
            y: 0,
            width: 512,
            height: 512,
        },
        ..tile_request(0, 0)
    };
    assert_eq!(doc.render_tile(&off_page, &surface).unwrap_err(), RenderFailure::BadRequest);
    assert_eq!(doc.worker_pid(), pid);
}

/// Failure: a job larger than its surface reaches the worker, which would write past the end. The host refuses it locally.
#[test]
fn a_job_larger_than_its_surface_never_leaves_the_host() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    let small = recto_render_service::Surface::create(1024).unwrap();
    assert_eq!(doc.render_tile(&tile_request(0, 0), &small).unwrap_err(), RenderFailure::BadRequest);
    assert!(!doc.worker_pid().is_some(), "no process for a request that can't be met");
    let zero = TileRequest {
        rect: TileRect {
            x: 0,
            y: 0,
            width: 0,
            height: 512,
        },
        ..tile_request(0, 0)
    };
    assert_eq!(doc.render_tile(&zero, &tile_surface()).unwrap_err(), RenderFailure::BadRequest);
    let nan = TileRequest {
        scale: f32::NAN,
        ..tile_request(0, 0)
    };
    assert_eq!(doc.render_tile(&nan, &tile_surface()).unwrap_err(), RenderFailure::BadRequest);
}

/// Failure: a file that isn't a PDF is reported as a crash or an unknown error, rather than saying what's wrong.
#[test]
fn files_that_arent_pdfs_say_so() {
    let junk = TempPdf::new("junk.pdf", b"not a pdf at all");
    let mut doc = renderer(&junk.0, Policy::default());
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(
        DocRenderer::new(std::path::Path::new("/no/such/file.pdf"), None, real_config(), Policy::default()).err(),
        Some(RenderFailure::NotFound)
    );
}

/// Failure: page text doesn't come through, or asks for a pixel check nobody can vouch for, so a false all-clear reaches the agent.
#[test]
fn page_characters_come_through_with_the_check_stated() {
    let file = TempPdf::new("text.pdf", &pages(&[Page::new(text_lines(&["Hello from the worker"]))]));
    let mut doc = renderer(&file.0, Policy::default());
    let checked = doc.page_characters(0, false, true).unwrap();
    assert!(checked.hidden_checked);
    assert!(checked.boxes.is_empty());
    let text: String = checked.chars.iter().map(|c| c.ch()).collect();
    assert!(text.contains("Hello from the worker"), "{text:?}");
    let with_boxes = doc.page_characters(0, true, false).unwrap();
    assert!(!with_boxes.hidden_checked, "nobody asked, so it mustn't claim to have checked");
    assert_eq!(with_boxes.boxes.len(), with_boxes.chars.len());
}

/// Failure: a page that explodes on loading is run again by each kind of job, so reading the text of a bomb after failing to draw it costs another full run, and
/// getting past the hidden-text check erases what was learned. One outcome: refused, and remembered for every kind of job after.
#[test]
fn a_page_that_cant_even_load_is_refused_once_and_remembered() {
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    let policy = Policy {
        memory_limit_bytes: 64 * 1024 * 1024,
        heavy_limit_bytes: 128 * 1024 * 1024,
        heavy_min_free_percent: 0,
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    // The ordinary page: checked.
    assert!(doc.page_characters(1, false, true).unwrap().hidden_checked);
    let expected = RenderFailure::PageReadFailed {
        page: 0,
        reason: KillReason::MemoryLimit,
    };
    assert_eq!(doc.page_characters(0, false, true).unwrap_err(), expected);
    let spawns = doc.stats().spawns;
    assert_eq!(doc.page_characters(0, false, true).unwrap_err(), expected);
    assert_eq!(doc.page_characters(0, false, false).unwrap_err(), expected);
    assert_eq!(doc.stats().spawns, spawns, "each of those should have been answered from memory");
    // Drawing has its own steps, and once they've failed they're remembered too.
    let drawn = RenderFailure::PageFailed {
        page: 0,
        reason: KillReason::MemoryLimit,
    };
    assert_eq!(doc.render_tile(&tile_request(0, 0), &tile_surface()).unwrap_err(), drawn);
    let spawns = doc.stats().spawns;
    assert_eq!(doc.render_tile(&tile_request(0, 0), &tile_surface()).unwrap_err(), drawn);
    assert_eq!(doc.stats().spawns, spawns);
}

/// Failure: a worker that left stays a zombie until the document is next touched. Nothing calls the document here, so only the reaper can have collected it.
#[test]
fn a_worker_that_left_is_collected_with_nobody_asking() {
    let file = plain_doc();
    let policy = Policy {
        idle_exit: Duration::from_millis(50),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    draws(&mut doc, 0);
    let pid = doc.worker_pid().unwrap();
    wait_until("the worker to be collected", || process_memory(pid).is_none());
}

/// Failure: a region drawn in the worker doesn't reach the host's surface, or its tiles' kinds are lost on the way. Each kind must match what one tile would report.
#[test]
fn a_region_through_the_worker_reports_each_tile() {
    let file = TempPdf::new("region.pdf", &pages(&[Page::new(text_lines(&["Only at the top"]))]));
    let mut doc = renderer(&file.0, Policy::default());
    let surface = recto_render_service::Surface::create(1024 * 1024 * 4).unwrap();
    let request = TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 1024,
            height: 1024,
        },
        scale: 2.0,
        stride: 4096,
        generation: 0,
    };
    let TileResult::Done(tile) = doc.render_region(&request, 512, &surface).unwrap() else {
        panic!()
    };
    assert_eq!(tile.region_kinds.len(), 4);
    let single = tile_surface();
    for (place, kind) in recto_proto::region_tiles(request.rect, 512).zip(&tile.region_kinds) {
        let alone = TileRequest {
            rect: place,
            stride: 2048,
            ..request
        };
        let TileResult::Done(one) = doc.render_tile(&alone, &single).unwrap() else {
            panic!()
        };
        assert_eq!(one.kind, *kind, "{place:?}");
    }
    let mut pixels = vec![0u8; 1024 * 1024 * 4];
    assert!(surface.copy_out(0, &mut pixels));
    assert!(pixels.as_chunks::<4>().0.iter().any(|px| px[0] < 128), "nothing reached the surface");
}

/// Failure: a region larger than the cap is sent anyway, so one call can cost far more than a viewport. The host refuses it, with no process started.
#[test]
fn a_region_past_the_cap_or_off_the_grid_never_leaves_the_host() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    let surface = recto_render_service::Surface::create(64 * 1024 * 1024).unwrap();
    let past_cap = TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 4096,
            height: 4096,
        },
        scale: 2.0,
        stride: 16384,
        generation: 0,
    };
    assert_eq!(doc.render_region(&past_cap, 512, &surface).unwrap_err(), RenderFailure::BadRequest);
    let off_grid = TileRequest {
        rect: TileRect {
            x: 100,
            y: 0,
            width: 512,
            height: 512,
        },
        stride: 2048,
        ..past_cap
    };
    assert_eq!(doc.render_region(&off_grid, 512, &surface).unwrap_err(), RenderFailure::BadRequest);
    assert_eq!(doc.render_region(&off_grid, 0, &surface).unwrap_err(), RenderFailure::BadRequest);
    assert!(!doc.worker_pid().is_some());
}

/// Failure: cancelling a slow region neither returns nor kills, so the page you've scrolled to waits for a pass nobody wants.
#[test]
fn cancelling_a_slow_region_returns_or_kill_covers_it() {
    let file = TempPdf::new("slow.pdf", &pages(&[Page::new(slow_page(400_000)), Page::new(text_lines(&["Next"]))]));
    let policy = Policy {
        page_deadline: Duration::from_secs(120),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    assert_eq!(doc.page_count().unwrap(), 2);
    let cancel = doc.cancel_handle();
    let pid = doc.worker_pid().unwrap();
    let opened = doc.stats().opened_footprint_bytes;
    let canceller = std::thread::spawn(move || {
        wait_until("the page to start loading", || {
            process_memory(pid).is_some_and(|memory| memory.footprint_bytes > opened + 1024 * 1024)
        });
        cancel.cancel_before(2);
    });
    let surface = recto_render_service::Surface::create(1024 * 1024 * 4).unwrap();
    let request = TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 1024,
            height: 1024,
        },
        scale: 2.0,
        stride: 4096,
        generation: 1,
    };
    let result = doc.render_region(&request, 512, &surface).unwrap();
    canceller.join().unwrap();
    assert!(
        matches!(result, TileResult::Cancelled { .. }),
        "a page this slow should still be drawing: {result:?}"
    );
    assert!(
        matches!(doc.render_tile(&tile_request(1, 2), &tile_surface()).unwrap(), TileResult::Done(_)),
        "the next job runs at once"
    );
}

/// Failure: the largest-image hint misreads a page, so the choice of how to draw it is wrong: an image page must report its image's pixels, a text page none, and a page's
/// answer must be remembered rather than asked for again.
#[test]
fn the_largest_image_is_found_and_remembered() {
    let image = recto_testpdf::Object::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 40 /Height 30 /ColorSpace /DeviceGray /BitsPerComponent 8".into(),
        data: vec![128; 40 * 30],
    };
    let first = recto_testpdf::first_extra_object(2);
    let bytes = recto_testpdf::pages_with(
        &[Page::new(b"q 100 0 0 100 50 50 cm /Im Do Q".to_vec()), Page::new(text_lines(&["Words"]))],
        &format!("/XObject << /Im {first} 0 R >>"),
        vec![image],
    );
    let file = TempPdf::new("image.pdf", &bytes);
    let mut doc = renderer(&file.0, Policy::default());
    assert_eq!(doc.largest_image_pixels(0, 0).unwrap(), 1200);
    assert_eq!(doc.largest_image_pixels(1, 0).unwrap(), 0);
    let jobs = doc.stats().jobs;
    assert_eq!(doc.largest_image_pixels(0, 0).unwrap(), 1200);
    assert_eq!(doc.stats().jobs, jobs, "asked again rather than remembered");
}

/// Failure: two documents share a worker or a global in the host, so one's failure breaks the other.
#[test]
fn two_documents_have_separate_workers() {
    let (a, b) = (plain_doc(), plain_doc());
    let (mut one, mut two) = (renderer(&a.0, Policy::default()), renderer(&b.0, Policy::default()));
    draws(&mut one, 0);
    draws(&mut two, 0);
    assert_ne!(one.worker_pid(), two.worker_pid());
    one.kill_worker();
    assert_eq!(draws(&mut two, 1), TileKind::Grey, "killing one doesn't touch the other");
}

/// Fills of the whole page, which take time in proportion to the pixels, so far longer at the check's 2x than for a tile at a low zoom, and a page of them has text that's quick to read.
fn full_page_fills(count: usize) -> Vec<u8> {
    let mut content = String::with_capacity(count * 28);
    for at in 0..count {
        content.push_str(&format!("0.{} g 0 0 612 792 re f\n", at % 9 + 1));
    }
    content.push_str("0 g\n");
    let mut content = content.into_bytes();
    content.extend_from_slice(&text_lines(&["Words on a slow page"]));
    content
}

/// A page that takes far longer to draw for the hidden-text check than these tests' short deadlines allow.
fn slow_to_check() -> TempPdf {
    let content = full_page_fills(20_000);
    TempPdf::new("slow.pdf", &pages(&[Page::new(text_lines(&["A plain page"])), Page::new(content)]))
}

/// Failure: reading text for a copy stops a slow page drawing for the rest of the session. The hidden-text check draws the whole page at 2x, and passing the deadline at that marked the page as failed at every scale, on the same renderer tiles use: a page that draws in 3.2 s at half zoom said "took too long to draw" after ⌘A ⌘C. A check that fails means not checked, and nothing about drawing. It's remembered as that, so it isn't tried again, and it doesn't count towards stopping the document.
#[test]
fn a_check_past_its_deadline_says_not_checked_and_leaves_drawing_alone() {
    let file = slow_to_check();
    let mut doc = renderer(
        &file.0,
        Policy {
            page_deadline: Duration::from_millis(250),
            ..Policy::default()
        },
    );
    let read = doc.page_characters(1, true, true).expect("the text still comes back");
    assert!(!read.hidden_checked, "not checked");
    assert!(read.chars.iter().any(|record| record.ch() == 'W'));
    assert!(doc.stats().deadline_kills >= 1, "{:?}", doc.stats());
    for scale in [0.25, 0.5, 1.0, 2.0, 4.0] {
        assert_eq!(doc.page_draw_state_at(1, scale), recto_render_service::PageDrawState::Sharp, "at {scale}x");
    }
    let kills = doc.stats().deadline_kills;
    assert!(!doc.page_characters(1, true, true).unwrap().hidden_checked);
    assert_eq!(doc.stats().deadline_kills, kills, "it isn't tried again");
}

/// Failure: a copy of a file with many slow pages uses up the kills a document is allowed, and then nothing in it draws. Checks stop being tried long before that, and what they cost isn't counted against drawing.
#[test]
fn checks_past_their_deadline_cant_stop_a_document_drawing() {
    let content = full_page_fills(20_000);
    let list: Vec<Page> = (0..6).map(|_| Page::new(content.clone())).collect();
    let file = TempPdf::new("slows.pdf", &pages(&list));
    // A check of one of these pages draws for over 2 s here, and reading its text takes 20 to 45 ms on a fresh worker, unloaded and at load 58. A deadline of 500 ms is far past a read and far short of a check, where 100 ms was within a few times of a read that a loaded run could take past it (the full check of 2026-11-08 at load 11).
    let mut doc = renderer(
        &file.0,
        Policy {
            page_deadline: Duration::from_millis(500),
            max_kills_per_document: 2,
            ..Policy::default()
        },
    );
    for page in 0..6 {
        let read = doc
            .page_characters(page, true, true)
            .unwrap_or_else(|failure| panic!("page {page}: {failure:?}, {:?}", doc.stats()));
        assert!(!read.hidden_checked);
    }
    assert!(doc.stats().deadline_kills >= 2, "{:?}", doc.stats());
    assert_eq!(doc.page_draw_state_at(0, 1.0), recto_render_service::PageDrawState::Sharp);
    assert!(doc.page_size(0).is_ok(), "the document hasn't been stopped");
}

/// Failure (seen in a full check at load 11, `checks_past_their_deadline_cant_stop_a_document_drawing`): a page's text read runs out of time because the Mac is too busy to give its worker the processor, and the page is remembered as unreadable for the rest of the session, so selecting and copying it say "took too long" though nothing's wrong with it. A worker that got under half its deadline in processor time is the Mac being busy, as an open already reckons it: counted as that, not remembered, and read again when it's next asked. Made here by stopping the worker, so it has no processor time at all.
#[test]
fn a_read_the_mac_was_too_busy_for_is_tried_again() {
    let file = TempPdf::new("busy-read.pdf", &pages(&[Page::new(text_lines(&["one"])), Page::new(text_lines(&["two"]))]));
    let mut doc = renderer(
        &file.0,
        Policy {
            page_deadline: Duration::from_millis(400),
            ..Policy::default()
        },
    );
    doc.page_size(0).unwrap();
    let pid = doc.worker_pid().expect("a worker with the file open");
    assert!(
        std::process::Command::new("/bin/kill")
            .arg("-STOP")
            .arg(pid.to_string())
            .status()
            .unwrap()
            .success()
    );
    let stopped = doc.page_characters(1, true, false);
    assert!(
        matches!(
            stopped,
            Err(RenderFailure::PageReadFailed {
                page: 1,
                reason: KillReason::Deadline
            })
        ),
        "{stopped:?}"
    );
    assert!(doc.stats().too_busy >= 1, "counted as the Mac being busy: {:?}", doc.stats());
    // Read again. Under a full check's load the new worker can be too busy for its 400 ms as well (seen 2026-11-11, at load 20+), and that's the
    // same thing again, not the page being remembered: a second try is allowed, as two busy reads in a row are still under the three that would be.
    let mut again = doc.page_characters(1, true, false);
    if matches!(
        again,
        Err(RenderFailure::PageReadFailed {
            reason: KillReason::Deadline,
            ..
        })
    ) {
        assert!(doc.stats().too_busy >= 2, "a deadline again is only ever the Mac being busy: {:?}", doc.stats());
        again = doc.page_characters(1, true, false);
    }
    let again = again.unwrap_or_else(|failure| panic!("read again, on a worker that has the processor: {failure:?}"));
    assert!(!again.chars.is_empty());
}

/// Failure (review 1): busy reads count towards the kills that stop a document, so a busy Mac stops a document drawing; or a page whose worker hangs every time, which looks just like a busy Mac, costs a deadline for every ask for ever. Busy kills don't count towards stopping, and three busy reads of a page in a row remember it as unreadable after all.
#[test]
fn busy_reads_dont_stop_a_document_and_three_in_a_row_are_remembered() {
    let list: Vec<Page> = (0..6).map(|at| Page::new(text_lines(&[&format!("page {at}")]))).collect();
    let file = TempPdf::new("busy-reads.pdf", &pages(&list));
    let mut doc = renderer(
        &file.0,
        Policy {
            page_deadline: Duration::from_millis(400),
            max_kills_per_document: 2,
            ..Policy::default()
        },
    );
    for (round, other) in [2u32, 3, 4].into_iter().enumerate() {
        doc.page_characters(other, false, false)
            .unwrap_or_else(|failure| panic!("round {round}: {failure:?}"));
        let pid = doc.worker_pid().expect("a worker");
        assert!(
            std::process::Command::new("/bin/kill")
                .arg("-STOP")
                .arg(pid.to_string())
                .status()
                .unwrap()
                .success()
        );
        let busy = doc.page_characters(1, false, false);
        assert!(
            matches!(
                busy,
                Err(RenderFailure::PageReadFailed {
                    page: 1,
                    reason: KillReason::Deadline
                })
            ),
            "round {round}: {busy:?}"
        );
    }
    assert!(doc.stats().too_busy >= 3, "{:?}", doc.stats());
    assert!(
        doc.page_size(5).is_ok() && doc.page_characters(5, false, false).is_ok(),
        "three kills past an allowance of two, and the document still reads: {:?}",
        doc.stats()
    );
    let spawns = doc.stats().spawns;
    assert!(
        matches!(
            doc.page_characters(1, false, false),
            Err(RenderFailure::PageReadFailed {
                page: 1,
                reason: KillReason::Deadline
            })
        ),
        "remembered"
    );
    assert_eq!(doc.stats().spawns, spawns, "with no worker started to learn it again");
}

/// Failure: a check nobody wants any more holds the thread that draws for as long as its deadline, up to 5 s, and on a hostile file again and again, while the page in view stays blank. It has to stop when it's told to, and a text job's generation has to reach the worker.
#[test]
fn a_check_stops_when_it_is_cancelled() {
    let file = TempPdf::new("cancel.pdf", &pages(&[Page::new(full_page_fills(200_000))]));
    let mut doc = renderer(
        &file.0,
        Policy {
            page_deadline: Duration::from_secs(60),
            ..Policy::default()
        },
    );
    let cancel = doc.cancel_handle();
    let stopping = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        cancel.cancel_before(8);
    });
    let started = std::time::Instant::now();
    let read = doc.page_characters_for(0, true, true, 7);
    stopping.join().unwrap();
    // 200,000 fills of the page at 2x is a minute or so, against its deadline of a minute, and it's the draw that's what's being stopped.
    assert!(started.elapsed() < Duration::from_secs(20), "it ran for {:?}", started.elapsed());
    if let Ok(read) = read {
        assert!(!read.hidden_checked, "stopped, so not checked");
    }
    assert!(doc.is_stale(7));
    assert_eq!(doc.stats().deadline_kills, 0, "it was told, and didn't have to be killed for its time");
}

/// Failure: a real change to the file goes unnoticed by whatever holds things about it, since what says so only ever fired when a cache of image places filled up, which has nothing to do with the file. Filling that says nothing about the file, and the file changing does.
#[test]
fn only_the_file_changing_says_the_file_changed() {
    let list: Vec<Page> = (0..4100).map(|_| Page::new(b"".to_vec())).collect();
    let file = TempPdf::new("places.pdf", &pages(&list));
    let mut doc = renderer(&file.0, Policy::default());
    for page in 0..4100 {
        let _ = doc.image_boxes(page, 0);
    }
    assert!(!doc.take_file_changed(), "a cache filling up");
    assert!(doc.take_image_places_cleared(), "and the cache says it's been cleared");
    assert!(!doc.take_image_places_cleared(), "once");
    // Written in place, with another number of pages, and found when a worker next opens it.
    assert_eq!(doc.page_count().unwrap(), 4100);
    std::fs::write(&file.0, pages(&[Page::new(text_lines(&["Only this now"]))])).unwrap();
    doc.kill_worker();
    let _ = doc.page_characters(0, true, false);
    assert!(doc.take_file_changed(), "the file changing");
    assert!(!doc.take_file_changed(), "once");
}

/// A draw only the worker that's there may do, as the page view's job for a tab off screen's place is.
fn only_there(doc: &mut DocRenderer, page: u32, generation: u64, surface: &Surface) -> Result<TileResult, RenderFailure> {
    DocRenderer::with_workers(
        doc,
        |doc| doc,
        Workers::OnlyTheOneThere,
        |doc| doc.render_tile(&tile_request(page, generation), surface),
    )
}

/// An ordinary draw, which must come out drawn.
fn drawn_after(doc: &mut DocRenderer, generation: u64, after: &str) {
    let result = doc.render_tile(&tile_request(1, generation), &tile_surface());
    assert!(
        matches!(result, Ok(TileResult::Done(_))),
        "the next ordinary job, after {after}, starts what it needs and draws: {result:?}"
    );
}

/// Failure (owner, 2026-11-13: "no file is opened again for it"), and the worse one it could bring: a job that may start no worker leaves that mode on, and the person's next page is refused a worker and stays bare. However a job only the worker there may do ends (drawn, refused for want of a worker, cancelled in the middle, killed in the middle, past its deadline, or a panic), it starts nothing itself, and the very next ordinary job starts a worker if it needs one and draws.
#[test]
fn a_job_that_may_start_no_worker_starts_none_and_leaves_the_next_one_free_to() {
    let file = TempPdf::new("slow.pdf", &pages(&[Page::new(slow_page(400_000)), Page::new(text_lines(&["Next"]))]));
    let policy = Policy {
        page_deadline: Duration::from_secs(120),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    assert_eq!(doc.page_count().unwrap(), 2);

    assert!(
        matches!(only_there(&mut doc, 1, 1, &surface), Ok(TileResult::Done(_))),
        "the worker there draws it"
    );
    drawn_after(&mut doc, 1, "one drawn");

    doc.kill_worker();
    let spawns = doc.stats().spawns;
    assert_eq!(only_there(&mut doc, 1, 1, &surface), Err(RenderFailure::WouldStart { heavy: false }));
    assert_eq!(doc.stats().spawns, spawns, "none started for it");
    drawn_after(&mut doc, 1, "one refused");
    assert_eq!(doc.stats().spawns, spawns + 1);

    // Cancelled while the slow page is in hand.
    let (cancel, pid, opened) = (doc.cancel_handle(), doc.worker_pid().unwrap(), doc.stats().opened_footprint_bytes);
    let canceller = std::thread::spawn(move || {
        wait_until("the page to start loading", || {
            process_memory(pid).is_some_and(|memory| memory.footprint_bytes > opened + 1024 * 1024)
        });
        cancel.cancel_before(3);
    });
    let result = only_there(&mut doc, 0, 2, &surface);
    canceller.join().unwrap();
    assert!(matches!(result, Ok(TileResult::Cancelled { .. })), "{result:?}");
    drawn_after(&mut doc, 3, "one cancelled");

    // Killed while the slow page is in hand: not tried again in a new worker.
    let (pid, opened, spawns) = (doc.worker_pid().unwrap(), doc.stats().opened_footprint_bytes, doc.stats().spawns);
    let killer = std::thread::spawn(move || {
        wait_until("the page to start loading", || {
            process_memory(pid).is_some_and(|memory| memory.footprint_bytes > opened + 1024 * 1024)
        });
        kill_process(pid);
    });
    let result = only_there(&mut doc, 0, 3, &surface);
    killer.join().unwrap();
    assert!(result.is_err(), "{result:?}");
    assert_eq!(doc.stats().spawns, spawns, "not tried again in a new one: {result:?}");
    drawn_after(&mut doc, 3, "one killed");

    // A panic inside it.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DocRenderer::with_workers(&mut doc, |doc| doc, Workers::OnlyTheOneThere, |_| panic!("a job's panic"))
    }));
    assert!(panicked.is_err());
    doc.kill_worker();
    drawn_after(&mut doc, 3, "a panic");

    // Past its deadline.
    let policy = Policy {
        page_deadline: Duration::from_millis(200),
        ..Policy::default()
    };
    let mut doc = renderer(&file.0, policy);
    drawn_after(&mut doc, 1, "nothing");
    let result = only_there(&mut doc, 0, 1, &surface);
    assert!(result.is_err(), "{result:?}");
    drawn_after(&mut doc, 1, "one past its deadline");
}

/// Failure: a page past the normal limit, drawn for nobody looking, went on to a heavy worker of up to the larger limit, and took the one heavy slot from whoever's page needs it. It's refused as needing one, and the worker that's there is kept.
#[test]
fn a_job_that_may_start_no_worker_never_goes_heavy() {
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    let slot = HeavySlot::new();
    let config = recto_render_service::ServiceConfig {
        heavy_slot: slot.clone(),
        ..real_config()
    };
    let policy = Policy {
        memory_limit_bytes: 64 * 1024 * 1024,
        heavy_limit_bytes: 128 * 1024 * 1024,
        heavy_min_free_percent: 0,
        ..Policy::default()
    };
    let mut doc = DocRenderer::new(&file.0, None, config, policy).unwrap();
    // Looking at the bomb's weight learns it's past the normal limit, at every scale.
    assert_eq!(doc.largest_image_pixels(0, 1).unwrap(), 0);
    assert_eq!(doc.page_draw_state(0), recto_render_service::PageDrawState::NeedsHeavy);
    drawn_after(&mut doc, 1, "learning that");
    let (pid, stats) = (doc.worker_pid(), doc.stats());
    let surface = tile_surface();
    assert_eq!(only_there(&mut doc, 0, 1, &surface), Err(RenderFailure::WouldStart { heavy: true }));
    assert_eq!(slot.times_taken(), 0, "the slot's free for whoever needs it");
    assert_eq!(
        (doc.worker_pid(), doc.stats().spawns),
        (pid, stats.spawns),
        "the worker there is the one there was"
    );
    assert_eq!(
        doc.stats().heavy_pages,
        stats.heavy_pages,
        "nor is it counted against the document's heavy pages"
    );
}
