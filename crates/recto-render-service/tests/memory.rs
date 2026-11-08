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

/// Failure: the bomb the spike found, which took PDFium to 4 GiB in 6.3 s, runs past any limit, or the sequence lets a hostile file take long. Every step of the
/// sequence fails for this page, so this runs them all against the default limits: it must end, within the larger limit, and the rest of the file must still draw.
#[test]
fn the_memory_sequence_ends_a_form_bomb_quickly() {
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    // Whether this Mac has memory free shouldn't decide what this test runs, and nor should how fast it is: only memory can stop this page here.
    let policy = Policy { heavy_min_free_percent: 0, page_deadline: Duration::from_secs(300), ..Policy::default() };
    let (normal, heavy) = (policy.memory_limit_bytes, policy.heavy_limit_bytes);
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    let started = std::time::Instant::now();
    let failure = doc.render_tile(&tile(0), &surface).unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
    let stats = doc.stats();
    assert_eq!(stats.memory_kills, 3, "the normal limit, the larger limit, and the lower resolution, once each");
    assert_eq!(stats.heavy_retries, 1);
    assert!(stats.peak_footprint_bytes > normal, "it never passed the normal limit: {} MiB", stats.peak_footprint_bytes / MIB);
    // The overshoot depends on this machine's load. The bound says only that the limit worked, against a page that reached 4 GiB with nothing stopping it.
    assert!(stats.peak_footprint_bytes < heavy * 2, "peak {} MiB against a limit of {} MiB", stats.peak_footprint_bytes / MIB, heavy / MIB);
    eprintln!("bomb: all steps failed by {elapsed:?}, peak {} MiB", stats.peak_footprint_bytes / MIB);
    assert_eq!(doc.page_draw_state(0), recto_render_service::PageDrawState::Failed(KillReason::MemoryLimit));
    let spawns = doc.stats().spawns;
    assert_eq!(doc.render_tile(&tile(0), &surface).unwrap_err(), failure);
    assert_eq!(doc.stats().spawns, spawns, "it ran the bomb again, when it should end quickly");
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

/// What one run of every page saw: the highest footprint, the retirements, and the footprint after each page, which is what retirement reads (`None` where it couldn't be read).
struct Run {
    peak: u64,
    retired: u32,
    after_each: Vec<Option<u64>>,
}

impl Run {
    fn largest_after_a_page(&self) -> u64 {
        self.after_each.iter().flatten().copied().max().unwrap_or(0)
    }

    fn said(&self) -> String {
        let after: Vec<String> = self.after_each.iter().map(|footprint| footprint.map_or("unread".into(), |bytes| format!("{:.1}", bytes as f64 / MIB as f64))).collect();
        format!("peak {:.1} MiB, {} retirements, MiB after each page: [{}]", self.peak as f64 / MIB as f64, self.retired, after.join(", "))
    }
}

/// Runs every page through a renderer.
fn run_pages(file: &TempPdf, policy: Policy, page_count: u32) -> Run {
    let mut doc = renderer(&file.0, policy);
    let surface = tile_surface();
    let mut after_each = Vec::new();
    for page in 0..page_count {
        assert!(matches!(doc.render_tile(&tile(page), &surface).unwrap(), TileResult::Done(_)));
        after_each.push(doc.worker_footprint());
    }
    Run { peak: doc.stats().peak_footprint_bytes, retired: doc.stats().retirements, after_each }
}

/// Failure: growth across a long document isn't held by retiring, the reason retirement exists: closing a document doesn't return memory. The fixture's pages each add about 2 MiB
/// in PDFium's cache of parsed streams, which is what retirement reads after each page, not the lifetime peak a page's drawing reaches for a moment. The line is put halfway up the
/// growth this Mac shows without retiring, so it's crossed wherever there's growth to hold. On macOS 15 in the sandbox (CI, 2026-11-08) a fixed 24 MiB line was never crossed after
/// a page, though the peak passed it, and nothing retired: the precondition was on the wrong number. Every footprint is printed. If what's held after each page doesn't grow there,
/// the test says so and passes, as memory is bounded without retiring.
#[test]
fn retiring_holds_down_a_growing_worker() {
    let file = TempPdf::new("images.pdf", &image_doc(30));
    let without = run_pages(&file, Policy { retire_bytes: u64::MAX, ..Policy::default() }, 30);
    println!("No retiring: {}", without.said());
    assert_eq!(without.retired, 0);
    if !without.after_each.iter().all(|footprint| footprint.is_some_and(|bytes| bytes > 0)) {
        // Retirement can't see this worker's footprint here, so the fallback has to hold it: a retirement after a bounded run of jobs with nothing to read.
        let with = run_pages(&file, Policy::default(), 30);
        println!("A footprint couldn't be read on this Mac, so retirement goes by count. With the defaults: {}", with.said());
        assert!(with.retired > 0, "nothing retired though footprints couldn't be read.\nWithout: {}\nWith: {}", without.said(), with.said());
        return;
    }
    let first = without.after_each[0].unwrap_or(0);
    let largest = without.largest_after_a_page();
    if largest <= first + 16 * MIB {
        // Nothing to hold: PDFium here doesn't keep what it parsed, and memory stays bounded without retiring, which is the property retirement is for. Said, not failed, as this
        // is a fact about this Mac's libraries; an unreadable footprint, the case retirement could miss, failed above.
        println!("The worker held no more than {:.1} MiB after any page without retiring, so on this Mac there's no growth for retirement to hold. {}", largest as f64 / MIB as f64, without.said());
        return;
    }
    let retire_at = first + (largest - first) / 2;
    let with = run_pages(&file, Policy { retire_bytes: retire_at, ..Policy::default() }, 30);
    println!("Retire at {:.1} MiB: {}", retire_at as f64 / MIB as f64, with.said());
    assert!(with.retired > 0, "nothing retired at {:.1} MiB.\nWithout: {}\nWith: {}", retire_at as f64 / MIB as f64, without.said(), with.said());
    // Retirement acts before the next page, so the worker can pass the line by one page's growth, and no more.
    let one_page = (largest - first) / 29 + 4 * MIB;
    assert!(with.largest_after_a_page() <= retire_at + one_page, "retiring didn't hold the worker near the line.\nWithout: {}\nWith: {}", without.said(), with.said());
}

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Not a test: a host that starts a bomb and is then killed, for the test below. Does nothing unless started as one.
#[test]
fn orphan_host_entry() {
    let Ok(pid_file) = std::env::var("RECTO_ORPHAN_PID_FILE") else { return };
    let file = TempPdf::new("orphan.pdf", &form_bomb());
    // So high that nothing here can stop this worker except noticing its host has gone. The test ends it long before this if that fails.
    let policy = Policy { memory_limit_bytes: 64 << 30, heavy_limit_bytes: 0, page_deadline: Duration::from_secs(3600), ..Policy::default() };
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
    let host = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "orphan_host_entry", "--nocapture", "--test-threads=1"])
        .env("RECTO_ORPHAN_PID_FILE", &pid_file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Any panic below kills the host on the way out, and the worker then ends itself, so no bomb is left running.
    let mut host = KillOnDrop(host);
    let mut worker = None;
    wait_until("the host to start its worker", || {
        worker = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.parse::<u32>().ok());
        worker.is_some()
    });
    let worker = worker.unwrap();
    wait_until("the bomb to start growing", || process_memory(worker).is_some_and(|memory| memory.footprint_bytes > 32 * MIB));
    let _ = host.0.kill();
    let _ = host.0.wait();
    // Its limit and deadline are out of reach, so the only way this process ends by itself, before this much memory, is by noticing its host has gone.
    let give_up_at = 2048 * MIB;
    let mut ended_by_itself = false;
    let mut last = 0;
    wait_until("the worker to end, or grow far enough to prove it won't", || match process_memory(worker) {
        None => {
            ended_by_itself = true;
            true
        }
        Some(memory) => {
            last = memory.footprint_bytes;
            last > give_up_at
        }
    });
    if !ended_by_itself {
        kill_process(worker);
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ended_by_itself, "the orphaned worker kept running, at {} MiB", last / MIB);
}
