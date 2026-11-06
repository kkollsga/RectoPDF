//! Memory limits against real PDFium, in their own binary. Footprints are of the worker, measured from outside. Bounds are budgets, not
//! one machine's numbers, and PDFium is a release build in every profile.

mod common;

use common::{kill_process, renderer, tile_surface, wait_until};
use recto_platform::isolate::process_memory;
use recto_render_service::{KillReason, Policy, RenderFailure, TileRect, TileRequest, TileResult};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, form_bomb, pages, pages_with, text_lines};
use std::time::Duration;

const MIB: u64 = 1024 * 1024;

fn tile(page: u32) -> TileRequest {
    TileRequest { page, rect: TileRect { x: 0, y: 0, width: 512, height: 512 }, scale: 2.0, stride: 2048, generation: 0 }
}

/// Failure: the bomb the spike found, which took PDFium to 4 GiB in 6.3 s, runs past the limit. Asserted against the default limit, on the
/// bound the host promises, and the rest of the file must still draw.
#[test]
fn the_default_limit_stops_a_form_bomb() {
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    let policy = Policy::default();
    let limit = policy.memory_limit_bytes;
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    let started = std::time::Instant::now();
    let failure = doc.render_tile(&tile(0), &surface).unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
    let stats = doc.stats();
    assert_eq!(stats.memory_kills, 1);
    assert!(stats.peak_footprint_bytes > limit, "it never reached the limit: {} MiB", stats.peak_footprint_bytes / MIB);
    assert!(stats.peak_footprint_bytes < limit + 64 * MIB, "overshoot: peak {} MiB against a limit of {} MiB", stats.peak_footprint_bytes / MIB, limit / MIB);
    eprintln!("bomb: stopped at {} MiB after {elapsed:?}", stats.peak_footprint_bytes / MIB);
    assert!(matches!(doc.render_tile(&tile(1), &surface).unwrap(), TileResult::Done(_)), "the ordinary page after the bomb still draws");
}

/// Failure: an idle worker costs more than its budget. The spike measured 2.27 MiB; this bound leaves room for a debug build and a slower
/// allocator, and still catches a heavy dependency creeping in.
#[test]
fn an_idle_worker_stays_small() {
    let file = TempPdf::new("idle.pdf", &pages(&[Page::new(text_lines(&["x"]))]));
    let mut doc = renderer(&file.0, Policy::default());
    doc.page_count().unwrap();
    let stats = doc.stats();
    eprintln!("idle {} KiB, after open {} KiB", stats.ready_footprint_bytes / 1024, stats.opened_footprint_bytes / 1024);
    assert!(stats.ready_footprint_bytes > 0, "nothing was measured");
    assert!(stats.ready_footprint_bytes < 6 * MIB, "idle worker at {} KiB", stats.ready_footprint_bytes / 1024);
    assert!(stats.opened_footprint_bytes < 8 * MIB, "opened worker at {} KiB", stats.opened_footprint_bytes / 1024);
}

/// An unfiltered image, about 2 MiB. PDFium keeps a parsed stream's bytes until the document closes, so each new page adds that much.
fn image_stream(seed: u8) -> Object {
    let (edge, data) = (800, vec![seed; 800 * 800 * 3]);
    Object::Stream { dict: format!("/Type /XObject /Subtype /Image /Width {edge} /Height {edge} /ColorSpace /DeviceRGB /BitsPerComponent 8"), data }
}

/// A document where each page holds its own image, so the worker grows with every new page.
fn image_doc(page_count: usize) -> Vec<u8> {
    let first = first_extra_object(page_count);
    let mut resources = String::from("/XObject <<");
    let mut objects = Vec::new();
    let mut doc_pages = Vec::new();
    for i in 0..page_count {
        resources.push_str(&format!(" /Im{i} {} 0 R", first + i));
        objects.push(image_stream(i as u8));
        doc_pages.push(Page::new(format!("q 500 0 0 500 50 100 cm /Im{i} Do Q")));
    }
    resources.push_str(" >>");
    pages_with(&doc_pages, &resources, objects)
}

/// Runs every page through a renderer, and returns the highest footprint seen and the retirements.
fn run_pages(file: &TempPdf, policy: Policy, page_count: u32) -> (u64, u32) {
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    for page in 0..page_count {
        assert!(matches!(doc.render_tile(&tile(page), &surface).unwrap(), TileResult::Done(_)));
    }
    (doc.stats().peak_footprint_bytes, doc.stats().retirements)
}

/// Failure: growth across a long document isn't held by retiring, the reason retirement exists: closing a document doesn't return memory.
/// The fixture's pages each add about 2 MiB, so without retiring the peak passes the limit many times over.
#[test]
fn retiring_holds_down_a_growing_worker() {
    let file = TempPdf::new("images.pdf", &image_doc(30));
    let without = Policy { retire_bytes: u64::MAX, ..Policy::default() };
    let (peak_without, retired_without) = run_pages(&file, without, 30);
    assert_eq!(retired_without, 0);
    let retire_at = 24 * MIB;
    let with = Policy { retire_bytes: retire_at, ..Policy::default() };
    let (peak_with, retired_with) = run_pages(&file, with, 30);
    eprintln!("no retiring: peak {} MiB. Retire at {} MiB: peak {} MiB, {retired_with} retirements", peak_without / MIB, retire_at / MIB, peak_with / MIB);
    assert!(peak_without > retire_at, "this fixture must grow past the limit to prove anything: {} MiB", peak_without / MIB);
    assert!(retired_with > 0, "nothing retired");
    assert!(peak_with < peak_without, "retiring didn't lower the peak: {} against {} MiB", peak_with / MIB, peak_without / MIB);
}

/// Not a test: a host that starts a bomb and is then killed, for the test below. Does nothing unless started as one.
#[test]
fn orphan_host_entry() {
    let Ok(pid_file) = std::env::var("RECTO_ORPHAN_PID_FILE") else { return };
    let file = TempPdf::new("orphan.pdf", &form_bomb());
    let policy = Policy { memory_limit_bytes: 8 << 30, page_deadline: Duration::from_secs(600), ..Policy::default() };
    let mut doc = renderer(&file.0, policy);
    doc.page_count().unwrap();
    std::fs::write(&pid_file, doc.worker_pid().unwrap().to_string()).unwrap();
    let _ = doc.render_tile(&tile(0), &tile_surface());
}

/// Failure: the host dies while a hostile page is growing, nobody is left to enforce the limit, and the worker eats the Mac. The worker must
/// notice by itself.
#[test]
fn an_orphaned_worker_stops_itself() {
    let dir = std::env::temp_dir().join(format!("recto-orphan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pid");
    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "orphan_host_entry", "--nocapture", "--test-threads=1"])
        .env("RECTO_ORPHAN_PID_FILE", &pid_file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut worker = None;
    wait_until("the host to start its worker", || {
        worker = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.parse::<u32>().ok());
        worker.is_some()
    });
    let worker = worker.unwrap();
    wait_until("the bomb to start growing", || process_memory(worker).is_some_and(|memory| memory.footprint_bytes > 32 * MIB));
    let _ = host.kill();
    let _ = host.wait();
    let mut seen_peak = 0;
    let gone = (0..2000).any(|_| {
        std::thread::sleep(Duration::from_millis(5));
        match process_memory(worker) {
            Some(memory) => {
                seen_peak = memory.peak_footprint_bytes;
                false
            }
            None => true,
        }
    });
    if !gone {
        // Don't leave a bomb running if this fails.
        kill_process(worker);
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(gone, "the orphaned worker kept running, peak {} MiB", seen_peak / MIB);
}
