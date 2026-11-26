//! The host against a worker that has been taken over. This binary stands in as its own worker: the host starts it with `--exact
//! fake_worker`, and `RECTO_FAKE` says how to misbehave. So these need neither PDFium nor luck, and every bad message is deterministic.

mod common;

use common::wait_until;
use recto_platform::isolate::{self, Channel, Handle, SharedMemory};
use recto_proto::{
    self as proto, CharRecord, FailCode, HEADER_LEN, Header, Kind, PageSize, RegionReply, RenderJob, TextJob, TextReply, TileKind, TileRect, TileReply,
    TileStatus,
};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{
    DocRenderer, KillReason, Policy, RenderFailure, ServiceConfig, TileRequest, TileResult, control, under_page_deadline_for_test, under_start_timeout_for_test,
};
use recto_testpdf::TempPdf;
use std::ffi::OsString;
use std::sync::atomic::Ordering;
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

/// Tells the test a job is in hand, so it can act at the right moment rather than after a guessed sleep.
fn mark_job_in_hand() {
    if let Ok(path) = std::env::var("RECTO_FAKE_MARK") {
        std::fs::write(path, b"in hand").unwrap();
    }
}

fn answer_render(mode: &str, job: &RenderJob, channel: &Channel, retained: &mut Vec<Vec<u8>>, cancel_word: Option<&SharedMemory>) {
    let good = good_tile(job);
    mark_job_in_hand();
    let tiles = proto::region_tiles(job.rect, job.tile_edge.max(1)).count();
    let region = RegionReply {
        tile: good,
        kinds: vec![TileKind::Grey; tiles],
    };
    let reply = match mode {
        "region_short" => RegionReply {
            kinds: vec![TileKind::Grey; tiles - 1],
            ..region
        }
        .encode(),
        "region_long" => RegionReply {
            kinds: vec![TileKind::Grey; tiles + 1],
            ..region
        }
        .encode(),
        "region_wrong_size" => RegionReply {
            tile: TileReply {
                width: job.rect.width + 1,
                ..good
            },
            ..region
        }
        .encode(),
        "region_answered_as_a_tile" => good.encode(),
        "region_garbled" => {
            let mut bytes = region.encode();
            let last = bytes.len() - 5;
            bytes[last] = 9;
            bytes
        }
        "good" if job.tile_edge != 0 => region.encode(),
        "cancels_uncalled" => TileReply {
            status: TileStatus::Cancelled,
            ..good
        }
        .encode(),
        "done_after_cancel" => {
            // Sees the cancel arrive, and finishes anyway, as a page in its last object would.
            let word = cancel_word.unwrap().atomic_u64(control::CANCEL_BEFORE_OFFSET).unwrap();
            while job.generation >= word.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            if job.tile_edge != 0 { region.encode() } else { good.encode() }
        }
        "failed_for_another_job" => proto::encode_failed(job.job_id + 1, FailCode::BadPage),
        "wrong_width" => TileReply {
            width: job.rect.width + 1,
            ..good
        }
        .encode(),
        "wrong_stride" => TileReply { stride: 4, ..good }.encode(),
        "wrong_job" => TileReply {
            job_id: job.job_id + 1,
            ..good
        }
        .encode(),
        "wrong_generation" => TileReply {
            generation: job.generation + 1,
            ..good
        }
        .encode(),
        "huge_tile" => TileReply {
            width: u32::MAX,
            height: u32::MAX,
            stride: u32::MAX,
            ..good
        }
        .encode(),
        "garbage" => vec![0xEE; 64],
        "oversized_header" => Header {
            kind: Kind::Tile,
            len: 1 << 20,
        }
        .encode()
        .to_vec(),
        "bad_kind" => proto::encode_opened(proto::Opened {
            pages: 1,
            version: 1,
            repaired: false,
        }),
        "spin_in_draw" => spin_on_every_thread(),
        "silent" | "ignore_cancel" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        "huge_text_claim" => {
            let header = Header {
                kind: Kind::Text,
                len: proto::MAX_FRAME,
            }
            .encode();
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
        _ => good.encode(),
    };
    let _ = send_all(channel, &reply, &[]);
}

fn answer_text(mode: &str, job: &TextJob) -> Vec<u8> {
    let good = TextReply {
        job_id: job.job_id,
        page: job.page,
        hidden_checked: job.hidden_check,
        chars: vec![CharRecord::new('a' as u32, 0)],
        boxes: Vec::new(),
        ..TextReply::default()
    };
    match mode {
        "text_wrong_page" => TextReply { page: job.page + 1, ..good }.encode().unwrap(),
        "text_nan_box" => TextReply {
            boxes: vec![[f32::NAN; 4]],
            ..good
        }
        .encode()
        .unwrap(),
        "text_lies_about_check" => TextReply {
            hidden_checked: !job.hidden_check,
            ..good
        }
        .encode()
        .unwrap(),
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
        let _ = send_all(
            &channel,
            &proto::encode_opened(proto::Opened {
                pages: 1,
                version: 1,
                repaired: false,
            }),
            &[],
        );
        std::thread::sleep(Duration::from_secs(60));
    }
    if mode == "hang_before_hello" {
        std::thread::sleep(Duration::from_secs(600));
    }
    send_all(
        &channel,
        &proto::encode_hello(proto::Hello {
            pid: std::process::id(),
            walled: false,
        }),
        &[],
    )
    .unwrap();
    let mut reader = FrameReader::default();
    let mut retained = Vec::new();
    let mut cancel_word = None;
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        match frame.kind {
            Kind::Open => {
                if mode == "sleep_on_open" {
                    std::thread::sleep(Duration::from_secs(600));
                }
                if mode == "spin_on_open" {
                    spin_on_every_thread();
                }
                let mut handles = frame.handles;
                let shared = SharedMemory::map(handles.pop().unwrap(), control::LEN).unwrap();
                if mode == "scribble_cancel" {
                    shared.atomic_u64(control::CANCEL_BEFORE_OFFSET).unwrap().store(u64::MAX, Ordering::SeqCst);
                }
                cancel_word = Some(shared);
                if mode == "open_leak" {
                    leak_for_ever();
                }
                let reply = match mode.as_str() {
                    "open_lies" => vec![0xFF; 16],
                    "open_failed_for_a_job" => proto::encode_failed(7, FailCode::NotPdf),
                    "open_too_many_pages" => proto::encode_opened(proto::Opened {
                        pages: u32::MAX,
                        version: 1,
                        repaired: false,
                    }),
                    "open_not_pdf" => proto::encode_failed(0, FailCode::NotPdf),
                    _ => proto::encode_opened(proto::Opened {
                        pages: 3,
                        version: 1,
                        repaired: false,
                    }),
                };
                let _ = send_all(&channel, &reply, &[]);
            }
            Kind::Render => answer_render(
                &mode,
                &RenderJob::decode(&frame.payload).unwrap(),
                &channel,
                &mut retained,
                cancel_word.as_ref(),
            ),
            Kind::SizeRequest => {
                let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                let size = match mode.as_str() {
                    "size_nan" => PageSize {
                        job_id,
                        width: f32::NAN,
                        height: 792.0,
                    },
                    "size_huge" => PageSize {
                        job_id,
                        width: 1e30,
                        height: 792.0,
                    },
                    "size_wrong_job" => PageSize {
                        job_id: job_id + 1,
                        width: 612.0,
                        height: 792.0,
                    },
                    _ => PageSize {
                        job_id,
                        width: 612.0,
                        height: 792.0,
                    },
                };
                let _ = send_all(&channel, &size.encode(), &[]);
            }
            Kind::TextRequest => {
                let job = TextJob::decode(&frame.payload).unwrap();
                if mode == "dies_drawing_for_the_check" && job.hidden_check {
                    std::process::abort();
                }
                // A page whose check draws past any deadline: it computes and never answers, and the host stops it at its deadline, having used the
                // processor as a heavy page's drawing does, so the kill is the page's and not a busy Mac's (a parked check used none, and was taken for
                // one). Its text read answers at once.
                if mode == "check_runs_past_its_deadline" && job.hidden_check {
                    loop {
                        std::hint::spin_loop();
                    }
                }
                let _ = send_all(&channel, &answer_text(&mode, &job), &[]);
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
        heavy_slot: recto_render_service::HeavySlot::new(),
        extra_env: vec![("RECTO_FAKE".into(), mode.into())],
        replace_args: Some(
            ["--exact", "fake_worker", "--nocapture", "--test-threads=1"]
                .into_iter()
                .map(OsString::from)
                .collect(),
        ),
    }
}

fn policy() -> Policy {
    Policy {
        page_deadline: Duration::from_secs(10),
        ..Policy::default()
    }
}

/// A small document for `DocRenderer` to open a descriptor on. The stand-in never reads it.
fn doc(mode: &str, policy: Policy) -> (DocRenderer, TempPdf) {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    (DocRenderer::new(&file.0, None, config(mode), policy).unwrap(), file)
}

fn request() -> TileRequest {
    TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 512,
            height: 512,
        },
        scale: 2.0,
        stride: 2048,
        generation: 1,
    }
}

fn surface() -> SharedMemory {
    SharedMemory::create(512 * 512 * 4).unwrap()
}

fn tile_failure(mode: &str) -> RenderFailure {
    let (mut doc, _file) = doc(mode, policy());
    doc.render_tile(&request(), &surface()).unwrap_err()
}

fn is_rejected(failure: RenderFailure) -> bool {
    matches!(
        failure,
        RenderFailure::PageFailed {
            reason: KillReason::Rejected(_),
            ..
        } | RenderFailure::PageReadFailed {
            reason: KillReason::Rejected(_),
            ..
        }
    )
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
        assert!(!doc.worker_pid().is_some(), "{mode}: a worker that sent this must not survive it");
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
    for mode in [
        "text_wrong_page",
        "text_nan_box",
        "text_lies_about_check",
        "text_short_frame",
        "text_bad_scalar",
    ] {
        let (mut doc, _file) = doc(mode, policy());
        let failure = doc.page_characters(0, false, false).unwrap_err();
        assert!(is_rejected(failure), "{mode}: {failure:?}");
    }
}

/// Asks for the page in `doc` as many times as a busy Mac is told of, and the one after: a worker that gets no processor time looks like a busy Mac to the host, so each of the first asks is told so and not remembered, and the ask that makes `MAX_BUSY` in a row is remembered as failed at the deadline. One more ask is answered from memory with no worker started. Returns the footprint before and after.
fn asked_until_remembered(doc: &mut DocRenderer) -> (u64, u64) {
    let before = recto_platform::footprint_bytes().unwrap_or(0);
    for ask in 1..recto_render_service::MAX_BUSY {
        assert_eq!(
            doc.render_tile(&request(), &surface()).unwrap_err(),
            RenderFailure::TooBusy { page: 0 },
            "ask {ask}"
        );
    }
    assert_eq!(
        doc.render_tile(&request(), &surface()).unwrap_err(),
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::Deadline
        }
    );
    assert_eq!(doc.stats().deadline_kills, u32::from(recto_render_service::MAX_BUSY));
    let spawns = doc.stats().spawns;
    assert!(matches!(
        doc.render_tile(&request(), &surface()),
        Err(RenderFailure::PageFailed { page: 0, .. })
    ));
    assert_eq!(doc.stats().spawns, spawns, "remembered, with no worker started to learn it again");
    (before, recto_platform::footprint_bytes().unwrap_or(0))
}

/// Uses the processor on 9 threads at once, for ever.
fn spin_on_every_thread() -> ! {
    for _ in 0..8 {
        std::thread::spawn(|| {
            loop {
                std::hint::spin_loop();
            }
        });
    }
    loop {
        std::hint::spin_loop();
    }
}

/// Failure: a worker claims a 64 MiB frame and sends nothing, and the host allocates it up front. A claim costs nothing until bytes arrive;
/// the deadline ends it. The stand-in sleeps, as a worker the Mac gave no processor does, so the asks go as a busy Mac's do, and the memory held across all of them stays small.
#[test]
fn a_claimed_frame_costs_nothing_until_it_arrives() {
    let (mut doc, _file) = doc(
        "huge_text_claim",
        Policy {
            page_deadline: Duration::from_millis(300),
            ..Policy::default()
        },
    );
    let before = recto_platform::footprint_bytes().unwrap_or(0);
    assert_eq!(doc.render_tile(&request(), &surface()).unwrap_err(), RenderFailure::TooBusy { page: 0 });
    let after = recto_platform::footprint_bytes().unwrap_or(0);
    assert!(after.saturating_sub(before) < 32 * 1024 * 1024, "grew from {before} to {after}");
}

/// Failure: a worker stuck in a page holds the document for ever. Past the deadline it's killed, and the page is marked. The stand-in sleeps and so gets no processor time, which the host takes for a Mac too busy to run it: that isn't remembered until [`recto_render_service::MAX_BUSY`] asks in a row, each a deadline, and then it is.
#[test]
fn a_page_that_never_finishes_is_killed_at_the_deadline() {
    let (mut doc, _file) = doc(
        "silent",
        Policy {
            page_deadline: Duration::from_millis(100),
            ..Policy::default()
        },
    );
    asked_until_remembered(&mut doc);
    assert!(!doc.worker_pid().is_some());
}

/// A path the stand-in touches when a job is in hand, and a config that tells it.
fn with_mark(mode: &str, file: &TempPdf) -> (ServiceConfig, std::path::PathBuf) {
    let mark = file.0.with_file_name("job-in-hand");
    let mut config = config(mode);
    config.extra_env.push(("RECTO_FAKE_MARK".into(), mark.to_str().unwrap().into()));
    (config, mark)
}

/// Failure: a worker that ignores the cancel word blocks the next page for its whole deadline. It gets its grace, then the kill.
#[test]
fn a_worker_that_ignores_cancel_is_killed_after_the_grace() {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let (config, mark) = with_mark("ignore_cancel", &file);
    let policy = Policy {
        page_deadline: Duration::from_secs(300),
        cancel_grace: Duration::from_millis(50),
        ..Policy::default()
    };
    let mut doc = DocRenderer::new(&file.0, None, config, policy).unwrap();
    let cancel = doc.cancel_handle();
    let watched = mark.clone();
    let canceller = std::thread::spawn(move || {
        wait_until("the job to be in hand", || watched.exists());
        cancel.cancel_before(2);
    });
    let result = doc.render_tile(&request(), &surface()).unwrap();
    canceller.join().unwrap();
    let _ = std::fs::remove_file(&mark);
    assert_eq!(result, TileResult::Cancelled { worker_killed: true });
    assert_eq!(doc.stats().cancel_kills, 1);
    assert_eq!(doc.stats().crashes, 0, "cancelling isn't a crash");
}

/// Failure: a job cancelled while it was finishing reaches the caller as a finished tile, so a stale tile lands on screen after a jump.
#[test]
fn a_tile_cancelled_while_finishing_never_reads_as_done() {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let (config, mark) = with_mark("done_after_cancel", &file);
    let mut doc = DocRenderer::new(
        &file.0,
        None,
        config,
        Policy {
            cancel_grace: Duration::from_secs(60),
            ..policy()
        },
    )
    .unwrap();
    let cancel = doc.cancel_handle();
    let watched = mark.clone();
    let canceller = std::thread::spawn(move || {
        wait_until("the job to be in hand", || watched.exists());
        cancel.cancel_before(2);
    });
    let result = doc.render_tile(&request(), &surface()).unwrap();
    canceller.join().unwrap();
    let _ = std::fs::remove_file(&mark);
    assert_eq!(result, TileResult::Cancelled { worker_killed: false });
    assert_eq!(doc.stats().rejections, 0, "finishing a moment late isn't misbehaving");
}

/// Failure: a worker says it cancelled a job nobody cancelled, and the host takes that as normal, so a page silently never draws.
#[test]
fn a_cancel_nobody_asked_for_is_misbehaving() {
    let failure = tile_failure("cancels_uncalled");
    assert!(is_rejected(failure), "{failure:?}");
}

/// Failure: a refusal for some other job is believed, so a reply meant for nothing decides this job's outcome.
#[test]
fn a_failure_for_another_job_is_rejected() {
    let failure = tile_failure("failed_for_another_job");
    assert!(is_rejected(failure), "{failure:?}");
}

fn region_request() -> TileRequest {
    TileRequest {
        page: 0,
        rect: TileRect {
            x: 0,
            y: 512,
            width: 1024,
            height: 1024,
        },
        scale: 2.0,
        stride: 4096,
        generation: 1,
    }
}

fn region_surface() -> SharedMemory {
    SharedMemory::create(1024 * 1024 * 4).unwrap()
}

/// Failure: the host believes a region reply with the wrong number of tiles or the wrong shape, and hands tiles out of the wrong place or runs off the list of kinds.
#[test]
fn bad_region_replies_are_rejected_and_the_worker_is_killed() {
    for mode in [
        "region_short",
        "region_long",
        "region_wrong_size",
        "region_answered_as_a_tile",
        "region_garbled",
    ] {
        let (mut doc, _file) = doc(mode, policy());
        let failure = doc.render_region(&region_request(), 512, &region_surface()).unwrap_err();
        assert!(is_rejected(failure), "{mode}: {failure:?}");
        assert!(!doc.worker_pid().is_some(), "{mode}");
    }
}

/// Failure: the stand-in's region replies are broken, so the rejections above prove nothing. A well-formed one comes through with a kind per tile.
#[test]
fn a_good_region_reply_is_accepted() {
    let (mut doc, _file) = doc("good", policy());
    let TileResult::Done(tile) = doc.render_region(&region_request(), 512, &region_surface()).unwrap() else {
        panic!()
    };
    assert_eq!(tile.region_kinds, vec![TileKind::Grey; 4]);
}

/// Failure: a stale region holds the worker for its whole pass, so a scroll waits longer than today's rule allows. It gets the same grace as a tile, then the same kill.
#[test]
fn a_region_that_ignores_cancel_is_killed_after_the_grace() {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let (config, mark) = with_mark("ignore_cancel", &file);
    let policy = Policy {
        page_deadline: Duration::from_secs(300),
        cancel_grace: Duration::from_millis(50),
        ..Policy::default()
    };
    let mut doc = DocRenderer::new(&file.0, None, config, policy).unwrap();
    let cancel = doc.cancel_handle();
    let watched = mark.clone();
    let canceller = std::thread::spawn(move || {
        wait_until("the job to be in hand", || watched.exists());
        cancel.cancel_before(2);
    });
    let result = doc.render_region(&region_request(), 512, &region_surface()).unwrap();
    canceller.join().unwrap();
    let _ = std::fs::remove_file(&mark);
    assert_eq!(result, TileResult::Cancelled { worker_killed: true });
}

/// Failure: a region cancelled while it was finishing reaches the caller as drawn tiles, so stale tiles land on screen after a jump.
#[test]
fn a_region_cancelled_while_finishing_never_reads_as_done() {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let (config, mark) = with_mark("done_after_cancel", &file);
    let mut doc = DocRenderer::new(
        &file.0,
        None,
        config,
        Policy {
            cancel_grace: Duration::from_secs(60),
            ..policy()
        },
    )
    .unwrap();
    let cancel = doc.cancel_handle();
    let watched = mark.clone();
    let canceller = std::thread::spawn(move || {
        wait_until("the job to be in hand", || watched.exists());
        cancel.cancel_before(2);
    });
    let result = doc.render_region(&region_request(), 512, &region_surface()).unwrap();
    canceller.join().unwrap();
    let _ = std::fs::remove_file(&mark);
    assert_eq!(result, TileResult::Cancelled { worker_killed: false });
}

/// Failure: a compromised worker ignores its idle time and keeps its memory for ever, since nobody looks between jobs. Nothing calls the document here.
#[test]
fn a_worker_that_outstays_its_idle_time_is_killed_with_nobody_asking() {
    let (mut doc, _file) = doc(
        "good",
        Policy {
            idle_exit: Duration::from_millis(50),
            idle_grace: Duration::from_millis(50),
            ..policy()
        },
    );
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    let pid = doc.worker_pid().unwrap();
    wait_until("the reaper to end it", || isolate::process_memory(pid).is_none());
}

/// Failure: the hidden-text check's drawing stops the worker and the page's text is lost with it, though the text itself is fine. It must come back marked as not
/// checked, the check isn't tried again, and what stopped a draw at 2x of the whole page says nothing about drawing a tile: this used to leave the page failed at every scale.
#[test]
fn text_comes_back_unchecked_when_the_check_stops_the_worker() {
    let (mut doc, _file) = doc("dies_drawing_for_the_check", policy());
    let page = doc.page_characters(0, false, true).unwrap();
    assert!(!page.hidden_checked, "it claims a check that never finished");
    assert_eq!(page.chars.len(), 1, "the text was lost");
    let spawns = doc.stats().spawns;
    let again = doc.page_characters(0, false, true).unwrap();
    assert!(!again.hidden_checked);
    assert_eq!(doc.stats().crashes, 1, "the check was retried on a page known to stop it");
    assert_eq!(doc.stats().spawns, spawns);
    assert!(
        matches!(doc.render_tile(&request(), &surface()), Ok(TileResult::Done(_))),
        "a check that failed doesn't stop the page being drawn"
    );
}

/// Failure: checks that stop their worker, however it was stopped, are counted against the document's kills, and a copy of a file whose pages all stop
/// their check ends up unable to draw anything. Their cost is `left_out.check` and not a kill of the document, so the plain reads still come back (not
/// checked), and drawing goes on past more crashes than `max_kills_per_document` allows. The stand-in's check aborts the worker, so no machine speed decides
/// it. Replaces `checks_past_their_deadline_cant_stop_a_document_drawing` (`tests/worker.rs`), which made every check meet a 500 ms deadline that the reads after
/// it had to beat too, so a read lost the race to the clock on a slow runner (CI run 45876504679: 4 spawns, 4 deadline kills, a read killed on its fresh worker).
/// The deadline's own case stays in `a_check_past_its_deadline_says_not_checked_and_leaves_drawing_and_text_alone`.
#[test]
fn checks_that_stop_their_worker_cant_stop_a_document_drawing() {
    let (mut doc, _file) = doc(
        "dies_drawing_for_the_check",
        Policy {
            max_kills_per_document: 2,
            ..policy()
        },
    );
    for page in 0..3 {
        let read = doc
            .page_characters(page, false, true)
            .unwrap_or_else(|failure| panic!("page {page}: {failure:?}, {:?}", doc.stats()));
        assert!(!read.hidden_checked, "page {page}");
        assert_eq!(read.chars.len(), 1, "page {page}");
    }
    assert!(doc.stats().crashes >= 3, "more stops than the document is allowed: {:?}", doc.stats());
    assert!(
        matches!(doc.render_tile(&request(), &surface()), Ok(TileResult::Done(_))),
        "the document still draws: {:?}",
        doc.stats()
    );
    assert!(doc.page_size(0).is_ok(), "the document hasn't been stopped");
}

/// Failure: reading text for a copy stops a slow page drawing for the rest of the session. The hidden-text check draws the whole page at 2x, and passing the
/// deadline at that marked the page as failed at every scale, on the same renderer tiles use: a page that draws in 3.2 s at half zoom said "took too long to
/// draw" after ⌘A ⌘C. A check that fails means not checked, and nothing about drawing or the page's text. It's remembered as that, so it isn't tried
/// again. On the stand-in, whose check computes and never answers and whose read answers at once, so only the check meets the deadline: on a real page
/// whose read had to beat the test's own 250 ms deadline too, the read lost under CI's limits about one run in five (`worker.rs`, 2026-11-13, 4 of 20 at
/// load 4 to 13). Only the first ask is short of time (review round 2): the open and the read in it are the one round trip left to beat 250 ms, and the
/// second ask and the tile have the test's ordinary deadline.
#[test]
fn a_check_past_its_deadline_says_not_checked_and_leaves_drawing_and_text_alone() {
    let (mut doc, _file) = doc("check_runs_past_its_deadline", policy());
    // The read after the stopped check runs on a fresh worker under the same 250 ms seam, and a starved machine can lose that race (`PageReadFailed{Deadline}`,
    // which is not remembered and is read again: `a_read_the_mac_was_too_busy_for_is_tried_again`). That says nothing about the check, so the ask is repeated
    // under the ordinary deadline and each lost read is counted as the one more kill it is.
    let mut lost_reads = 0;
    let mut read =
        recto_render_service::under_page_deadline_for_test(&mut doc, |doc| doc, Duration::from_millis(250), |doc| doc.page_characters(0, false, true));
    while matches!(
        read,
        Err(recto_render_service::RenderFailure::PageReadFailed {
            reason: KillReason::Deadline,
            ..
        })
    ) && lost_reads < 5
    {
        lost_reads += 1;
        read = doc.page_characters(0, false, true);
    }
    let read = read.expect("the text still comes back");
    assert!(!read.hidden_checked, "not checked");
    assert_eq!(read.chars.len(), 1, "the page's text");
    assert_eq!(
        doc.stats().deadline_kills,
        1 + lost_reads,
        "the check was stopped at its deadline, once: {:?}",
        doc.stats()
    );
    for scale in [0.25, 0.5, 1.0, 2.0, 4.0] {
        assert_eq!(doc.page_draw_state_at(0, scale), recto_render_service::PageDrawState::Sharp, "at {scale}x");
    }
    assert!(!doc.page_characters(0, false, true).unwrap().hidden_checked);
    assert_eq!(doc.stats().deadline_kills, 1 + lost_reads, "it isn't tried again");
    assert!(
        matches!(doc.render_tile(&request(), &surface()), Ok(TileResult::Done(_))),
        "the page still draws"
    );
}

/// Failure: a file that stops every worker while opening starts a new one on every call, and the error blames "page 1", which isn't what happened.
#[test]
fn a_failure_at_open_is_remembered_and_doesnt_blame_a_page() {
    for mode in ["open_lies", "open_failed_for_a_job", "open_too_many_pages"] {
        let (mut doc, _file) = doc(mode, policy());
        let first = doc.page_count().unwrap_err();
        assert!(
            matches!(
                first,
                RenderFailure::OpenFailed {
                    reason: KillReason::Rejected(_)
                }
            ),
            "{mode}: {first:?}"
        );
        assert!(!first.message().contains("Page"), "{mode}: {}", first.message());
        assert_eq!(doc.stats().rejections, 1, "{mode}");
        assert_eq!(doc.render_tile(&request(), &surface()).unwrap_err(), first, "{mode}");
        assert_eq!(doc.page_size(0).unwrap_err(), first, "{mode}");
        assert_eq!(doc.stats().spawns, 1, "{mode}: it started another process to learn the same thing");
        doc.forget_failures();
        assert_eq!(doc.page_count().unwrap_err(), first, "{mode}");
        assert_eq!(doc.stats().spawns, 2, "{mode}: asked to try again, it should");
    }
}

/// Failure: a file that eats memory while opening reads as a bad page, and is opened again on every call.
#[test]
fn a_memory_kill_at_open_says_so_and_stays_remembered() {
    let (mut doc, _file) = doc(
        "open_leak",
        Policy {
            memory_limit_bytes: 32 * 1024 * 1024,
            ..policy()
        },
    );
    let failure = doc.page_count().unwrap_err();
    assert_eq!(
        failure,
        RenderFailure::OpenFailed {
            reason: KillReason::MemoryLimit
        }
    );
    assert_eq!(failure.code(), "file_too_large_to_open");
    assert_eq!(doc.stats().memory_kills, 1);
    assert_eq!(doc.page_count().unwrap_err(), failure);
    assert_eq!(doc.stats().spawns, 1);
}

/// Failure: a file that isn't a PDF starts a process on every call just to hear the same answer.
#[test]
fn a_refusal_at_open_is_remembered_too() {
    let (mut doc, _file) = doc("open_not_pdf", policy());
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(doc.page_size(0).unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(doc.stats().spawns, 1);
}

/// Failure: the memory limit trusts the worker's word, or there's no limit: this worker says nothing and only touches memory. The host must see it from outside and
/// stop it, at each limit in turn, without PDFium's help.
#[test]
fn a_worker_that_grows_is_killed_at_the_limit_from_outside() {
    let (limit, heavy) = (32 * 1024 * 1024, 64 * 1024 * 1024);
    // A long deadline, so only memory can stop it, however slow this machine is.
    let (mut doc, _file) = doc(
        "leak",
        Policy {
            memory_limit_bytes: limit,
            heavy_limit_bytes: heavy,
            heavy_min_free_percent: 0,
            page_deadline: Duration::from_secs(300),
            ..policy()
        },
    );
    assert_eq!(
        doc.render_tile(&request(), &surface()).unwrap_err(),
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::MemoryLimit
        }
    );
    let stats = doc.stats();
    assert_eq!(stats.memory_kills, 3, "the normal limit, the larger one, and the lower-resolution try");
    assert!(
        stats.peak_footprint_bytes > heavy,
        "killed before it reached the limit: {}",
        stats.peak_footprint_bytes
    );
    // How far past depends on this machine's speed and load, so this only says it was stopped at all, not by how much.
    assert!(
        stats.peak_footprint_bytes < 4 * 1024 * 1024 * 1024,
        "nothing stopped it: {}",
        stats.peak_footprint_bytes
    );
}

/// Failure: retirement depends on the worker reporting its own size, so a compromised one never retires. It's measured from outside.
#[test]
fn retirement_is_measured_from_outside() {
    let (mut doc, _file) = doc(
        "bloat_then_answer",
        Policy {
            retire_bytes: 16 * 1024 * 1024,
            ..policy()
        },
    );
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    let bloated = doc.worker_pid();
    let next_page = TileRequest { page: 1, ..request() };
    assert!(matches!(doc.render_tile(&next_page, &surface()).unwrap(), TileResult::Done(_)));
    assert_eq!(doc.stats().retirements, 1);
    assert_ne!(doc.worker_pid(), bloated);
}

/// Failure: a crash in a worker that has drawn other pages marks the page as bad, though the cause may be what's built up. It gets one more
/// try in a fresh worker, and only a fresh worker failing marks it.
#[test]
fn a_crash_gets_one_retry_in_a_fresh_worker_only_if_the_worker_had_history() {
    let (mut doc, _file) = doc("crash_on_page_one", policy());
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    let spawns = doc.stats().spawns;
    let page_one = TileRequest { page: 1, ..request() };
    assert_eq!(
        doc.render_tile(&page_one, &surface()).unwrap_err(),
        RenderFailure::PageFailed {
            page: 1,
            reason: KillReason::Crashed
        }
    );
    assert_eq!(doc.stats().crashes, 2, "the old worker, then the fresh one");
    assert_eq!(doc.stats().spawns, spawns + 1);
    // A crash in a fresh worker isn't retried.
    let (mut fresh, _file) = self::doc("crash", policy());
    assert_eq!(
        fresh.render_tile(&request(), &surface()).unwrap_err(),
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::Crashed
        }
    );
    assert_eq!(fresh.stats().crashes, 1);
}

/// Failure: a worker that exits with the "idle" code mid-job makes the host retry for ever, a hang made of respawns.
#[test]
fn claiming_to_be_idle_cant_loop_for_ever() {
    let failure = tile_failure("exit_zero");
    assert_eq!(
        failure,
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::Crashed
        }
    );
}

/// Failure: a reply arriving a byte at a time is treated as a bad frame, though it's only slow.
#[test]
fn a_slow_reply_is_still_a_reply() {
    let (mut doc, _file) = doc("drip", policy());
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
}

/// Failure: a worker that won't say hello hangs the caller, or wrong version bytes are accepted.
#[test]
fn a_worker_that_wont_start_properly_is_refused() {
    let (mut wrong, _file) = doc("wrong_hello", policy());
    assert_eq!(wrong.page_count().unwrap_err(), RenderFailure::Unavailable);
    assert_eq!(wrong.stats().spawns, 0);
}

/// Failure: a worker that never says hello hangs its caller for ever. And the count of busy starts: counted past the bound, the indexer (which watches it) pauses for ever and never says it couldn't be built; not counted within it, a busy Mac is called broken.
#[test]
fn a_worker_that_never_says_hello_is_given_up_on() {
    let (mut doc, _file) = doc(
        "hang_before_hello",
        Policy {
            start_timeout: Duration::from_millis(100),
            ..policy()
        },
    );
    // No file has reached it, so a first silence is the Mac being busy and is asked again; one that keeps on is a worker that hangs, and says so.
    for _ in 1..recto_render_service::MAX_BUSY {
        assert_eq!(doc.page_count().unwrap_err(), RenderFailure::TooBusyToStart);
    }
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::Unavailable);
    assert!(!doc.worker_pid().is_some());
    assert_eq!(
        doc.stats().too_busy,
        u32::from(recto_render_service::MAX_BUSY) - 1,
        "the start past the bound isn't counted as the Mac being busy"
    );
}

/// Failure (twice): a worker with no time to open a file on a busy Mac reads the same as a file that takes too long to open, and the document's index is given up on. Asleep, as a starved worker is, it's the Mac: `TooBusyToStart`, which isn't remembered (so a second ask opens it again, instead of repeating a failure at once) and isn't a kill that counts towards stopping the document (the first read after a wake is an open, 2026-11-25). Using what it's given, it's the file: `OpenFailed`.
#[test]
fn a_worker_too_busy_to_open_is_told_from_a_file_too_slow_to_open() {
    // Long enough that what it used getting this far, which a loaded Mac makes more of, stays well under half.
    let (mut asleep, _file) = doc(
        "sleep_on_open",
        Policy {
            page_deadline: Duration::from_secs(1),
            ..policy()
        },
    );
    assert_eq!(asleep.page_count().unwrap_err(), RenderFailure::TooBusyToStart);
    assert_eq!(asleep.stats().too_busy, 1);
    // Not remembered: asked again it starts a worker again (a remembered failure would answer at once, with no new worker). A starved worker uses almost no processor, so a short deadline tells it from one that works as well as a long one.
    let short = |doc: &mut DocRenderer| under_page_deadline_for_test(doc, |doc| doc, Duration::from_millis(150), |doc| doc.page_count());
    let spawns = asleep.stats().spawns;
    assert_eq!(short(&mut asleep).unwrap_err(), RenderFailure::TooBusyToStart);
    assert_eq!(asleep.stats().spawns, spawns + 1, "a new worker was started for the second ask");
    // Opens in a row are bounded as starts are: the MAX_BUSY-th is the file's after all, and that's remembered.
    assert_eq!(short(&mut asleep).unwrap_err(), RenderFailure::OpenFailed { reason: KillReason::Deadline });
    // Past the bound it's the file's: not counted as the Mac being busy, so it counts towards stopping the document and the indexer says "couldn't open", not paused (final review, 2026-11-25).
    assert_eq!(
        asleep.stats().too_busy,
        u32::from(recto_render_service::MAX_BUSY) - 1,
        "the kill judged the file's isn't a busy one"
    );
    let spawns = asleep.stats().spawns;
    assert_eq!(short(&mut asleep).unwrap_err(), RenderFailure::OpenFailed { reason: KillReason::Deadline });
    assert_eq!(asleep.stats().spawns, spawns, "remembered: no worker for a fourth ask");
    // On every thread it can: between them they have over half of this in processor time, even on a Mac with a share of a core for each.
    let (mut spins, _file) = doc(
        "spin_on_open",
        Policy {
            page_deadline: Duration::from_millis(400),
            ..policy()
        },
    );
    assert_eq!(spins.page_count().unwrap_err(), RenderFailure::OpenFailed { reason: KillReason::Deadline });
    assert_eq!(spins.stats().too_busy, 0, "it had the processor, so that's the file");
}

/// Failure (review of the busy renderer start, 2026-11-25): busy starts and busy opens share one count, so two busy starts and then one starved open end as the file's failure (`OpenFailed`), remembered, though no start or open had been tried `MAX_BUSY` times. Each is counted by itself.
#[test]
fn busy_starts_and_busy_opens_are_counted_apart() {
    let (mut doc, _file) = doc(
        "sleep_on_open",
        Policy {
            page_deadline: Duration::from_millis(150),
            ..policy()
        },
    );
    let no_time = |doc: &mut DocRenderer| under_start_timeout_for_test(doc, |doc| doc, Duration::ZERO, |doc| doc.page_count());
    for run in 1..recto_render_service::MAX_BUSY {
        assert_eq!(no_time(&mut doc).unwrap_err(), RenderFailure::TooBusyToStart, "start {run}");
    }
    assert_eq!(
        doc.page_count().unwrap_err(),
        RenderFailure::TooBusyToStart,
        "the first starved open after them"
    );
}

/// Failure (final review of the busy renderer start, 2026-11-25): a worker that said hello didn't end the run of busy starts unless it also opened the file, so two busy starts, a hello with a starved open, and one more busy start read as the third start in a row and said `Unavailable` (the renderer can't start) of a worker that had started. A hello ends the run of starts; the bound still holds, as alternating starts and opens end at the file's failure after at most twice `MAX_BUSY` tries.
#[test]
fn a_worker_that_said_hello_ends_the_run_of_busy_starts() {
    let max = recto_render_service::MAX_BUSY;
    let policy = || Policy {
        page_deadline: Duration::from_millis(150),
        ..policy()
    };
    let no_time = |doc: &mut DocRenderer| under_start_timeout_for_test(doc, |doc| doc, Duration::ZERO, |doc| doc.page_count());
    let (mut doc, _file) = doc("sleep_on_open", policy());
    for run in 1..max {
        assert_eq!(no_time(&mut doc).unwrap_err(), RenderFailure::TooBusyToStart, "start {run}");
    }
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::TooBusyToStart, "a hello, then a starved open");
    assert_eq!(
        no_time(&mut doc).unwrap_err(),
        RenderFailure::TooBusyToStart,
        "the run of starts began again at the hello"
    );
    // Alternating never runs either count to its end sooner than the opens do.
    let (mut doc, _file) = self::doc("sleep_on_open", policy());
    let mut tries = 0;
    loop {
        tries += 1;
        let failure = if tries % 2 == 1 { no_time(&mut doc) } else { doc.page_count() }.unwrap_err();
        if failure != RenderFailure::TooBusyToStart {
            assert_eq!(failure, RenderFailure::OpenFailed { reason: KillReason::Deadline });
            break;
        }
        assert!(tries <= 2 * u32::from(max), "alternating ran on past the bound");
    }
}

/// Failure: a draw that hangs while using the processor, on every thread it can, reads as a Mac too busy to run its worker, so a hostile page is asked again and again as `TooBusy` and never remembered. Using the processor, it's the page: the first ask is a failure at the deadline and nothing counts as busy. (The draw twin of the open test above; seeded by counting every deadline as busy.)
#[test]
fn a_draw_that_uses_the_processor_is_told_from_a_mac_too_busy_to_run_it() {
    let (mut spins, _file) = doc(
        "spin_in_draw",
        Policy {
            page_deadline: Duration::from_millis(400),
            ..policy()
        },
    );
    assert_eq!(
        spins.render_tile(&request(), &surface()).unwrap_err(),
        RenderFailure::PageFailed {
            page: 0,
            reason: KillReason::Deadline
        }
    );
    assert_eq!(spins.stats().too_busy, 0, "it had the processor, so that's the page");
}

/// Failure: a compromised worker writes to the shared cancel word and the host believes it, so every tile of the document reads as stale.
#[test]
fn a_worker_cant_make_the_host_think_everything_is_stale() {
    let (mut doc, _file) = doc("scribble_cancel", policy());
    doc.page_count().unwrap();
    assert!(matches!(doc.render_tile(&request(), &surface()).unwrap(), TileResult::Done(_)));
    assert_eq!(doc.stats().cancel_kills, 0);
}

/// Failure: a worker keeps sending handles on a long frame, and the host holds them until it runs out of descriptors.
#[test]
fn handles_on_a_dripped_frame_are_capped() {
    let (a, b) = Channel::pair().unwrap();
    let memory = SharedMemory::create(4096).unwrap();
    let frame = proto::encode_opened(proto::Opened {
        pages: 5,
        version: 1,
        repaired: false,
    });
    for byte in &frame {
        a.send(std::slice::from_ref(byte), &[memory.handle()]).unwrap();
    }
    let received = FrameReader::default().read_blocking(&b).unwrap();
    assert_eq!(received.kind, Kind::Opened);
    assert!(received.handles.len() <= isolate::MAX_HANDLES, "kept {}", received.handles.len());
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
    send_all(
        &a,
        &proto::encode_opened(proto::Opened {
            pages: 5,
            version: 1,
            repaired: false,
        }),
        &handles,
    )
    .unwrap();
    let frame = FrameReader::default().read_blocking(&b).unwrap();
    assert_eq!(frame.kind, Kind::Opened);
    assert_eq!(frame.handles.len(), 1);
    assert_eq!(proto::decode_opened(&frame.payload).map(|opened| (opened.pages, opened.version)), Ok((5, 1)));
}
