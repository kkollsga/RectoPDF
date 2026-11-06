//! The real worker and real PDFium, from the host's side.

mod common;

use common::{kill_process, real_config, renderer, tile_surface, wait_until};
use recto_platform::isolate::process_memory;
use recto_render_service::{DocRenderer, KillReason, Policy, RenderFailure, TileKind, TileRect, TileRequest, TileResult};
use recto_testpdf::{Page, TempPdf, busy_page, form_bomb, pages, slow_page, text_lines};
use std::time::Duration;

fn tile_request(page: u32, generation: u64) -> TileRequest {
    TileRequest { page, rect: TileRect { x: 0, y: 0, width: 512, height: 512 }, scale: 2.0, stride: 2048, generation }
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

/// Failure: pixels don't reach the host's surface through the shared memory, the whole point of the worker.
#[test]
fn a_tile_drawn_in_the_worker_arrives_in_the_hosts_surface() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    let surface = tile_surface();
    let result = doc.render_tile(&tile_request(0, 1), &surface).unwrap();
    assert!(matches!(result, TileResult::Done(tile) if tile.kind == TileKind::Colour), "{result:?}");
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
    assert!(!doc.has_worker());
    assert_eq!(doc.stats().spawns, 0);
    assert_eq!(doc.worker_pid(), None);
    draws(&mut doc, 0);
    assert!(doc.has_worker());
    assert_eq!(doc.stats().spawns, 1);
}

/// Failure: a worker is killed mid-page, from outside, and the host dies with it, or the document stays broken. The page that was in hand
/// reports a failure, and the next page draws in a new process.
#[test]
fn a_worker_killed_mid_page_is_replaced() {
    let file = TempPdf::new("kill.pdf", &form_bomb());
    let policy = Policy { memory_limit_bytes: 2 << 30, page_deadline: Duration::from_secs(60), ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    assert_eq!(doc.page_count().unwrap(), 2);
    let first_worker = doc.worker_pid().unwrap();
    // The bomb runs until something stops it, so this kill lands inside the page whenever it arrives.
    let killer = std::thread::spawn(move || {
        wait_until("the bomb to start growing", || process_memory(first_worker).is_some_and(|memory| memory.footprint_bytes > 32 * 1024 * 1024));
        kill_process(first_worker);
    });
    let surface = tile_surface();
    let failure = doc.render_tile(&tile_request(0, 0), &surface).unwrap_err();
    killer.join().unwrap();
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::Crashed });
    assert_eq!(draws(&mut doc, 1), TileKind::Grey, "the next page draws in a new worker");
    assert_ne!(doc.worker_pid(), Some(first_worker));
    assert_eq!(doc.stats().crashes, 1);
}

/// Failure: a page that stopped a worker is tried again on every scroll, each try costing seconds and hundreds of MiB.
#[test]
fn a_page_that_stopped_a_fresh_worker_isnt_retried_until_asked() {
    let file = TempPdf::new("again.pdf", &form_bomb());
    let policy = Policy { memory_limit_bytes: 64 * 1024 * 1024, ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    let first = doc.render_tile(&tile_request(0, 0), &surface).unwrap_err();
    assert_eq!(first, RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
    let spawns = doc.stats().spawns;
    assert_eq!(doc.render_tile(&tile_request(0, 0), &surface).unwrap_err(), first, "remembered");
    assert_eq!(doc.stats().spawns, spawns, "remembering it must not start a worker");
    doc.forget_failures();
    assert_eq!(doc.render_tile(&tile_request(0, 0), &surface).unwrap_err(), first, "tried again once asked");
    assert_eq!(doc.stats().spawns, spawns + 1);
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
    assert!(matches!(doc.render_tile(&tile_request(0, 5), &surface).unwrap(), TileResult::Done(_)), "the current generation still draws");
}

/// Failure: cancelling a slow page neither returns nor kills, so a stale page holds the worker and the page you've scrolled to waits for it.
/// Either way out is right: the worker's pause callback returns, or the page was in a part with no pause and the kill covers it.
#[test]
fn cancelling_a_slow_page_returns_or_kill_covers_it() {
    let file = TempPdf::new("slow.pdf", &pages(&[Page::new(slow_page(400_000)), Page::new(text_lines(&["Next"]))]));
    let policy = Policy { page_deadline: Duration::from_secs(120), ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    assert_eq!(doc.page_count().unwrap(), 2);
    let cancel = doc.cancel_handle();
    let pid = doc.worker_pid().unwrap();
    let opened = doc.stats().opened_footprint_bytes;
    let canceller = std::thread::spawn(move || {
        // Parsing this page's 400,000 objects grows the worker, so growth means the job is in hand. The page takes far longer than this.
        wait_until("the page to start loading", || process_memory(pid).is_some_and(|memory| memory.footprint_bytes > opened + 1024 * 1024));
        cancel.cancel_before(2);
    });
    let surface = tile_surface();
    let result = doc.render_tile(&tile_request(0, 1), &surface).unwrap();
    canceller.join().unwrap();
    assert!(matches!(result, TileResult::Cancelled { .. }), "a page this slow should still be drawing: {result:?}");
    assert!(matches!(doc.render_tile(&tile_request(1, 2), &surface).unwrap(), TileResult::Done(_)), "the next job runs at once");
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
    let policy = Policy { idle_exit: Duration::from_millis(100), ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    draws(&mut doc, 0);
    let pid = doc.worker_pid().unwrap();
    wait_until("the worker to leave by itself", || !doc.has_worker());
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
    let policy = Policy { retire_bytes: 1024, ..Policy::default() };
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
    let policy = Policy { retire_bytes: 1024, ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    for x in 0..3 {
        let request = TileRequest { rect: TileRect { x: x * 300, y: 0, width: 300, height: 300 }, ..tile_request(0, 0) };
        assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    }
    assert_eq!(doc.stats().spawns, 1);
    assert_eq!(doc.stats().retirements, 0);
    assert!(doc.has_worker());
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
    let off_page = TileRequest { rect: TileRect { x: 100_000, y: 0, width: 512, height: 512 }, ..tile_request(0, 0) };
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
    assert!(!doc.has_worker(), "no process for a request that can't be met");
    let zero = TileRequest { rect: TileRect { x: 0, y: 0, width: 0, height: 512 }, ..tile_request(0, 0) };
    assert_eq!(doc.render_tile(&zero, &tile_surface()).unwrap_err(), RenderFailure::BadRequest);
    let nan = TileRequest { scale: f32::NAN, ..tile_request(0, 0) };
    assert_eq!(doc.render_tile(&nan, &tile_surface()).unwrap_err(), RenderFailure::BadRequest);
}

/// Failure: a file that isn't a PDF is reported as a crash or an unknown error, rather than saying what's wrong.
#[test]
fn files_that_arent_pdfs_say_so() {
    let junk = TempPdf::new("junk.pdf", b"not a pdf at all");
    let mut doc = renderer(&junk.0, Policy::default());
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(DocRenderer::new(std::path::Path::new("/no/such/file.pdf"), None, real_config(), Policy::default()).err(), Some(RenderFailure::NotFound));
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
    let policy = Policy { memory_limit_bytes: 64 * 1024 * 1024, ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    // The ordinary page: checked.
    assert!(doc.page_characters(1, false, true).unwrap().hidden_checked);
    let expected = RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit };
    assert_eq!(doc.page_characters(0, false, true).unwrap_err(), expected);
    let spawns = doc.stats().spawns;
    assert_eq!(doc.page_characters(0, false, true).unwrap_err(), expected);
    assert_eq!(doc.page_characters(0, false, false).unwrap_err(), expected);
    assert_eq!(doc.render_tile(&tile_request(0, 0), &tile_surface()).unwrap_err(), expected);
    assert_eq!(doc.stats().spawns, spawns, "each of those should have been answered from memory");
}

/// Failure: a worker that left stays a zombie until the document is next touched. Nothing calls the document here, so only the reaper can have collected it.
#[test]
fn a_worker_that_left_is_collected_with_nobody_asking() {
    let file = plain_doc();
    let policy = Policy { idle_exit: Duration::from_millis(50), ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    draws(&mut doc, 0);
    let pid = doc.worker_pid().unwrap();
    wait_until("the worker to be collected", || process_memory(pid).is_none());
}

/// Failure: `kill_worker` waits for the process to end, so a worker stuck in a read from a dead volume hangs whoever called it. It must return, and the next job must still work.
#[test]
fn killing_a_worker_doesnt_wait_for_it() {
    let file = plain_doc();
    let mut doc = renderer(&file.0, Policy::default());
    draws(&mut doc, 0);
    let pid = doc.worker_pid().unwrap();
    doc.kill_worker();
    assert_eq!(doc.worker_pid(), None, "it's let go at once");
    wait_until("the reaper to collect it", || process_memory(pid).is_none());
    assert_eq!(draws(&mut doc, 1), TileKind::Grey);
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
