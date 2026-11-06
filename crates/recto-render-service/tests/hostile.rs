//! The host against a worker that has been taken over. This binary stands in as its own worker: the host starts it with `--exact
//! fake_worker`, and `RECTO_FAKE` says how to misbehave. So these need neither PDFium nor luck, and every bad message is deterministic.

mod common;

use common::wait_until;
use recto_platform::isolate::{self, Channel, Handle, SharedMemory};
use recto_proto::{self as proto, CharRecord, FailCode, HEADER_LEN, Header, Kind, PageSize, RenderJob, TextJob, TextReply, TileKind, TileRect, TileReply, TileStatus};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{DocRenderer, KillReason, Policy, RenderFailure, ServiceConfig, TileRequest, TileResult};
use recto_testpdf::TempPdf;
use std::ffi::OsString;
use std::time::Duration;

const ALLOC_STEP: usize = 8 * 1024 * 1024;

fn good_tile(job: &RenderJob) -> TileReply {
    TileReply {
        generation: job.generation,
        job_id: job.job_id,
        width: job.rect.width,
        height: job.rect.height,
        stride: job.stride,
        status: TileStatus::Done(TileKind::Colour),
        render_us: 1,
    }
}

/// Touches memory until something stops this process.
fn leak_for_ever() -> ! {
    let mut kept = Vec::new();
    loop {
        kept.push(vec![1u8; ALLOC_STEP]);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn answer_render(mode: &str, job: &RenderJob, channel: &Channel, retained: &mut Vec<Vec<u8>>) {
    let good = good_tile(job);
    let reply = match mode {
        "wrong_width" => TileReply { width: job.rect.width + 1, ..good }.encode(),
        "wrong_stride" => TileReply { stride: 4, ..good }.encode(),
        "wrong_job" => TileReply { job_id: job.job_id + 1, ..good }.encode(),
        "wrong_generation" => TileReply { generation: job.generation + 1, ..good }.encode(),
        "huge_tile" => TileReply { width: u32::MAX, height: u32::MAX, stride: u32::MAX, ..good }.encode(),
        "garbage" => vec![0xEE; 64],
        "oversized_header" => Header { kind: Kind::Tile, len: 1 << 20 }.encode().to_vec(),
        "bad_kind" => proto::encode_opened(1),
        "silent" | "ignore_cancel" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        "huge_text_claim" => {
            let header = Header { kind: Kind::Text, len: proto::MAX_FRAME }.encode();
            let _ = send_all(channel, &header, &[]);
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        "leak" => leak_for_ever(),
        "crash" => std::process::abort(),
        "crash_on_page_one" if job.page == 1 => std::process::abort(),
        "exit_zero" => isolate::exit_now(0),
        "bloat_then_answer" => {
            // Lies by omission: says nothing of its size, and the host must notice from outside.
            retained.push(vec![1u8; 32 * 1024 * 1024]);
            good.encode()
        }
        "drip" => {
            for byte in good.encode() {
                let _ = send_all(channel, &[byte], &[]);
                std::thread::sleep(Duration::from_millis(1));
            }
            return;
        }
        "attach_handles" => {
            let extra = SharedMemory::create(4096).unwrap();
            let _ = send_all(channel, &good.encode(), &[extra.handle()]);
            return;
        }
        _ => good.encode(),
    };
    let _ = send_all(channel, &reply, &[]);
}

fn answer_text(mode: &str, job: &TextJob) -> Vec<u8> {
    let good = TextReply { job_id: job.job_id, page: job.page, hidden_checked: job.hidden_check, chars: vec![CharRecord::new('a' as u32, 0)], boxes: Vec::new() };
    match mode {
        "text_wrong_page" => TextReply { page: job.page + 1, ..good }.encode().unwrap(),
        "text_nan_box" => TextReply { boxes: vec![[f32::NAN; 4]], ..good }.encode().unwrap(),
        "text_lies_about_check" => TextReply { hidden_checked: !job.hidden_check, ..good }.encode().unwrap(),
        "text_short_frame" => {
            let mut bytes = good.encode().unwrap();
            bytes.pop();
            let len = (bytes.len() - HEADER_LEN) as u32;
            bytes[8..12].copy_from_slice(&len.to_le_bytes());
            bytes
        }
        "text_bad_scalar" => {
            let mut bytes = good.encode().unwrap();
            let at = bytes.len() - 4;
            bytes[at..].copy_from_slice(&0xD800u32.to_le_bytes());
            bytes
        }
        _ => good.encode().unwrap(),
    }
}

/// Not a test: the stand-in worker. Does nothing unless `RECTO_FAKE` says this process was started as one.
#[test]
fn fake_worker() {
    let Ok(mode) = std::env::var("RECTO_FAKE") else { return };
    let channel = Channel::inherited().unwrap();
    if mode == "wrong_hello" {
        let _ = send_all(&channel, &proto::encode_opened(1), &[]);
        std::thread::sleep(Duration::from_secs(60));
    }
    if mode == "hang_before_hello" {
        std::thread::sleep(Duration::from_secs(600));
    }
    send_all(&channel, &proto::encode_hello(std::process::id()), &[]).unwrap();
    let mut reader = FrameReader::default();
    let mut retained = Vec::new();
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        match frame.kind {
            Kind::Open => {
                let reply = if mode == "open_lies" { vec![0xFF; 16] } else { proto::encode_opened(3) };
                let _ = send_all(&channel, &reply, &[]);
            }
            Kind::Render => answer_render(&mode, &RenderJob::decode(&frame.payload).unwrap(), &channel, &mut retained),
            Kind::SizeRequest => {
                let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                let size = match mode.as_str() {
                    "size_nan" => PageSize { job_id, width: f32::NAN, height: 792.0 },
                    "size_huge" => PageSize { job_id, width: 1e30, height: 792.0 },
                    "size_wrong_job" => PageSize { job_id: job_id + 1, width: 612.0, height: 792.0 },
                    _ => PageSize { job_id, width: 612.0, height: 792.0 },
                };
                let _ = send_all(&channel, &size.encode(), &[]);
            }
            Kind::TextRequest => {
                let reply = answer_text(&mode, &TextJob::decode(&frame.payload).unwrap());
                let _ = send_all(&channel, &reply, &[]);
            }
            _ => {
                let _ = send_all(&channel, &proto::encode_failed(0, FailCode::Internal), &[]);
            }
        }
    }
}

fn config(mode: &str) -> ServiceConfig {
    ServiceConfig {
        worker_exe: std::env::current_exe().unwrap(),
        pdfium_lib: "/nonexistent".into(),
        extra_env: vec![("RECTO_FAKE".into(), mode.into())],
        replace_args: Some(["--exact", "fake_worker", "--nocapture", "--test-threads=1"].into_iter().map(OsString::from).collect()),
    }
}

fn policy() -> Policy {
    Policy { page_deadline: Duration::from_secs(10), ..Policy::default() }
}

/// A small document for `DocRenderer` to open a descriptor on. The stand-in never reads it.
fn doc(mode: &str, policy: Policy) -> (DocRenderer, TempPdf) {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    (DocRenderer::new(&file.0, None, config(mode), policy).unwrap(), file)
}

fn request() -> TileRequest {
    TileRequest { page: 0, rect: TileRect { x: 0, y: 0, width: 512, height: 512 }, scale: 2.0, stride: 2048, generation: 1 }
}

fn surface() -> SharedMemory {
    SharedMemory::create(512 * 512 * 4).unwrap()
}

fn tile_failure(mode: &str) -> RenderFailure {
    let (mut doc, _file) = doc(mode, policy());
    doc.render_tile(&request(), &surface()).unwrap_err()
}

fn is_rejected(failure: RenderFailure) -> bool {
    matches!(failure, RenderFailure::PageFailed { reason: KillReason::Rejected(_), .. })
}

/// Failure: the stand-in itself is broken, so every rejection below proves nothing. A well-behaved fake must pass.
#[test]
fn a_well_behaved_worker_is_accepted() {
    let (mut doc, _file) = doc("good", policy());
    assert_eq!(doc.page_count().unwrap(), 3);
    assert_eq!(doc.page_size(0).unwrap(), (612.0, 792.0));
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    assert_eq!(doc.page_characters(0, false, false).unwrap().chars.len(), 1);
    assert_eq!(doc.stats().rejections, 0);
}

/// Failure: the host believes a tile's dimensions from the worker, and reads outside the surface or draws a skewed page.
#[test]
fn bad_tile_metadata_is_rejected_and_the_worker_is_killed() {
    for mode in ["wrong_width", "wrong_stride", "wrong_job", "wrong_generation", "huge_tile"] {
        let (mut doc, _file) = doc(mode, policy());
        let failure = doc.render_tile(&request(), &surface()).unwrap_err();
        assert!(is_rejected(failure), "{mode}: {failure:?}");
        assert!(!doc.has_worker(), "{mode}: a worker that sent this must not survive it");
        assert_eq!(doc.stats().rejections, 1, "{mode}");
    }
}

/// Failure: bytes that aren't a frame, or a frame claiming more than its kind allows, are read as if they were.
#[test]
fn garbage_on_the_channel_is_rejected() {
    for mode in ["garbage", "oversized_header", "bad_kind"] {
        let failure = tile_failure(mode);
        assert!(is_rejected(failure), "{mode}: {failure:?}");
    }
}

/// Failure: a hostile page size reaches layout as NaN or a huge number.
#[test]
fn bad_page_sizes_are_rejected() {
    for mode in ["size_nan", "size_huge", "size_wrong_job"] {
        let (mut doc, _file) = doc(mode, policy());
        let failure = doc.page_size(0).unwrap_err();
        assert!(is_rejected(failure), "{mode}: {failure:?}");
    }
}

/// Failure: a text reply with bad lengths or values reaches the normaliser, or claims a hidden-text check that didn't run.
#[test]
fn bad_text_is_rejected() {
    for mode in ["text_wrong_page", "text_nan_box", "text_lies_about_check", "text_short_frame", "text_bad_scalar"] {
        let (mut doc, _file) = doc(mode, policy());
        let failure = doc.page_characters(0, false, false).unwrap_err();
        assert!(is_rejected(failure), "{mode}: {failure:?}");
    }
}

/// Failure: a worker claims a 64 MiB frame and sends nothing, and the host allocates it up front. A claim costs nothing until bytes arrive;
/// the deadline ends it.
#[test]
fn a_claimed_frame_costs_nothing_until_it_arrives() {
    let (mut doc, _file) = doc("huge_text_claim", Policy { page_deadline: Duration::from_millis(300), ..Policy::default() });
    let before = recto_platform::footprint_bytes().unwrap_or(0);
    let failure = doc.render_tile(&request(), &surface()).unwrap_err();
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::Deadline });
    let after = recto_platform::footprint_bytes().unwrap_or(0);
    assert!(after.saturating_sub(before) < 32 * 1024 * 1024, "grew from {before} to {after}");
}

/// Failure: a worker stuck in a page holds the document for ever. Past the deadline it's killed, and the page is marked.
#[test]
fn a_page_that_never_finishes_is_killed_at_the_deadline() {
    let (mut doc, _file) = doc("silent", Policy { page_deadline: Duration::from_millis(100), ..Policy::default() });
    assert_eq!(doc.render_tile(&request(), &surface()).unwrap_err(), RenderFailure::PageFailed { page: 0, reason: KillReason::Deadline });
    assert_eq!(doc.stats().deadline_kills, 1);
    assert!(!doc.has_worker());
}

/// Failure: a worker that ignores the cancel word blocks the next page for its whole deadline. It gets its grace, then the kill.
#[test]
fn a_worker_that_ignores_cancel_is_killed_after_the_grace() {
    let (mut doc, _file) = doc("ignore_cancel", Policy { page_deadline: Duration::from_secs(300), cancel_grace: Duration::from_millis(50), ..Policy::default() });
    let cancel = doc.cancel_handle();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel.cancel_before(2);
    });
    let result = doc.render_tile(&request(), &surface()).unwrap();
    canceller.join().unwrap();
    assert_eq!(result, TileResult::Cancelled { worker_killed: true });
    assert_eq!(doc.stats().cancel_kills, 1);
    assert_eq!(doc.stats().crashes, 0, "cancelling isn't a crash");
}

/// Failure: the memory limit trusts the worker's word, or there's no limit: this worker says nothing and only touches memory. The host must
/// see it from outside and stop it, without PDFium's help.
#[test]
fn a_worker_that_grows_is_killed_at_the_limit_from_outside() {
    let limit = 64 * 1024 * 1024;
    let (mut doc, _file) = doc("leak", Policy { memory_limit_bytes: limit, ..policy() });
    assert_eq!(doc.render_tile(&request(), &surface()).unwrap_err(), RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
    let stats = doc.stats();
    assert_eq!(stats.memory_kills, 1);
    assert!(stats.peak_footprint_bytes > limit, "killed before it reached the limit: {}", stats.peak_footprint_bytes);
    assert!(stats.peak_footprint_bytes < limit + 64 * 1024 * 1024, "overshoot too large: {}", stats.peak_footprint_bytes);
}

/// Failure: retirement depends on the worker reporting its own size, so a compromised one never retires. It's measured from outside.
#[test]
fn retirement_is_measured_from_outside() {
    let (mut doc, _file) = doc("bloat_then_answer", Policy { retire_bytes: 16 * 1024 * 1024, ..policy() });
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    assert_eq!(doc.stats().retirements, 1);
    assert!(!doc.has_worker());
}

/// Failure: a crash in a worker that has drawn other pages marks the page as bad, though the cause may be what's built up. It gets one more
/// try in a fresh worker, and only a fresh worker failing marks it.
#[test]
fn a_crash_gets_one_retry_in_a_fresh_worker_only_if_the_worker_had_history() {
    let (mut doc, _file) = doc("crash_on_page_one", policy());
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    let spawns = doc.stats().spawns;
    let page_one = TileRequest { page: 1, ..request() };
    assert_eq!(doc.render_tile(&page_one, &surface()).unwrap_err(), RenderFailure::PageFailed { page: 1, reason: KillReason::Crashed });
    assert_eq!(doc.stats().crashes, 2, "the old worker, then the fresh one");
    assert_eq!(doc.stats().spawns, spawns + 1);
    // A crash in a fresh worker isn't retried.
    let (mut fresh, _file) = self::doc("crash", policy());
    assert_eq!(fresh.render_tile(&request(), &surface()).unwrap_err(), RenderFailure::PageFailed { page: 0, reason: KillReason::Crashed });
    assert_eq!(fresh.stats().crashes, 1);
}

/// Failure: a worker that exits with the "idle" code mid-job makes the host retry for ever, a hang made of respawns.
#[test]
fn claiming_to_be_idle_cant_loop_for_ever() {
    let failure = tile_failure("exit_zero");
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::Crashed });
}

/// Failure: a reply arriving a byte at a time is treated as a bad frame, though it's only slow.
#[test]
fn a_slow_reply_is_still_a_reply() {
    let (mut doc, _file) = doc("drip", policy());
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
}

/// Failure: a worker sends handles to the host and the host keeps them, until it runs out of descriptors.
#[test]
fn handles_from_a_worker_are_not_kept() {
    let (mut doc, _file) = doc("attach_handles", policy());
    let surface = surface();
    doc.page_count().unwrap();
    assert!(matches!(doc.render_tile(&request(), &surface).unwrap(), TileResult::Done(_)));
    let before = recto_platform::open_handle_count();
    for _ in 0..20 {
        assert!(matches!(doc.render_tile(&request(), &surface).unwrap(), TileResult::Done(_)));
    }
    assert_eq!(recto_platform::open_handle_count(), before);
}

/// Failure: a worker that won't say hello hangs the caller, or wrong version bytes are accepted.
#[test]
fn a_worker_that_wont_start_properly_is_refused() {
    let (mut wrong, _file) = doc("wrong_hello", policy());
    assert_eq!(wrong.page_count().unwrap_err(), RenderFailure::Unavailable);
    assert_eq!(wrong.stats().spawns, 0);
}

/// Failure: a lie at open time is believed, or panics the host.
#[test]
fn a_bad_answer_to_opening_is_rejected() {
    let (mut doc, _file) = doc("open_lies", policy());
    assert!(is_rejected(doc.page_count().unwrap_err()));
    assert!(!doc.has_worker());
}

/// Failure: a dropped document leaves its worker running, so closing a tab doesn't return its memory.
#[test]
fn dropping_a_document_ends_its_worker() {
    let (mut doc, _file) = doc("good", policy());
    doc.page_count().unwrap();
    let pid = doc.worker_pid().unwrap();
    drop(doc);
    wait_until("the worker to end", || isolate::process_memory(pid).is_none());
}

/// Failure: an unusable handle type or the message shape changes silently, breaking every worker. The same frame the worker sends parses.
#[test]
fn the_frame_helpers_round_trip_handles() {
    let (a, b) = Channel::pair().unwrap();
    let memory = SharedMemory::create(4096).unwrap();
    let handles: Vec<&Handle> = vec![memory.handle()];
    send_all(&a, &proto::encode_opened(5), &handles).unwrap();
    let frame = FrameReader::default().read_blocking(&b).unwrap();
    assert_eq!(frame.kind, Kind::Opened);
    assert_eq!(frame.handles.len(), 1);
    assert_eq!(proto::decode_opened(&frame.payload), Ok(5));
}
