//! The memory sequence, step by step, against a stand-in worker that fits or doesn't fit each limit by design. So nothing here depends on how much memory a real page
//! takes on this machine: limits and growth are the test's own. Owner, 2026-11-06: normal limit, one page alone at the larger limit if the Mac can spare it, a
//! lower-resolution version reported as such, then a plain failure, remembered.

mod common;

use recto_platform::isolate::{self, Channel, SharedMemory};
use recto_proto::{self as proto, CharRecord, FailCode, Kind, PageSize, RenderJob, TextJob, TextReply, TileKind, TileRect, TileReply, TileStatus};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{
    DocRenderer, HeavySlot, KillReason, LowResWhy, OutputCaps, PageDrawState, PageEvent, Policy, RenderFailure, ServiceConfig, TileRequest, TileResult, draw_whole_page,
};
use recto_testpdf::TempPdf;
use std::ffi::OsString;
use std::time::Duration;

const NORMAL: u64 = 16 * 1024 * 1024;
const HEAVY: u64 = 48 * 1024 * 1024;

fn leak_for_ever() -> ! {
    let mut kept = Vec::new();
    loop {
        kept.push(vec![1u8; 4 * 1024 * 1024]);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Not a test: the stand-in. Page 0 is the heavy one, and `RECTO_FAKE` says what it fits in. Other pages always fit.
#[test]
fn fake_worker() {
    let Ok(mode) = std::env::var("RECTO_FAKE") else { return };
    let limit: u64 = std::env::var("RECTO_WORKER_LIMIT").unwrap().parse().unwrap();
    let channel = Channel::inherited().unwrap();
    send_all(&channel, &proto::encode_hello(std::process::id()), &[]).unwrap();
    let mut reader = FrameReader::default();
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        let reply = match frame.kind {
            Kind::Open => proto::encode_opened(3),
            Kind::SizeRequest => {
                let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                PageSize { job_id, width: 612.0, height: 792.0 }.encode()
            }
            Kind::TextRequest => {
                let job = TextJob::decode(&frame.payload).unwrap();
                TextReply { job_id: job.job_id, page: job.page, hidden_checked: job.hidden_check, chars: vec![CharRecord::new('a' as u32, 0)], boxes: Vec::new() }.encode().unwrap()
            }
            Kind::Render => {
                let job = RenderJob::decode(&frame.payload).unwrap();
                let low_res = job.scale < 1.0;
                let in_heavy = limit >= HEAVY;
                let fits = job.page != 0
                    || match mode.as_str() {
                        "fits_normal" => true,
                        "fits_heavy" => in_heavy,
                        "fits_low_res" => low_res,
                        "fits_heavy_or_low_res" => in_heavy || low_res,
                        _ => false,
                    };
                if !fits {
                    leak_for_ever();
                }
                TileReply {
                    generation: job.generation,
                    job_id: job.job_id,
                    width: job.rect.width,
                    height: job.rect.height,
                    stride: job.stride,
                    status: TileStatus::Done(TileKind::Colour),
                    render_us: 1,
                }
                .encode()
            }
            _ => proto::encode_failed(0, FailCode::Internal),
        };
        send_all(&channel, &reply, &[]).unwrap();
    }
}

fn config(mode: &str, slot: &HeavySlot) -> ServiceConfig {
    ServiceConfig {
        worker_exe: std::env::current_exe().unwrap(),
        pdfium_lib: "/nonexistent".into(),
        heavy_slot: slot.clone(),
        extra_env: vec![("RECTO_FAKE".into(), mode.into())],
        replace_args: Some(["--exact", "fake_worker", "--nocapture", "--test-threads=1"].into_iter().map(OsString::from).collect()),
    }
}

/// Limits of the test's own, and memory always free unless a test says otherwise.
fn policy() -> Policy {
    Policy { memory_limit_bytes: NORMAL, heavy_limit_bytes: HEAVY, heavy_min_free_percent: 0, ..Policy::default() }
}

fn doc(mode: &str) -> (DocRenderer, TempPdf) {
    doc_with(mode, policy(), &HeavySlot::new())
}

fn doc_with(mode: &str, policy: Policy, slot: &HeavySlot) -> (DocRenderer, TempPdf) {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    (DocRenderer::new(&file.0, None, config(mode, slot), policy).unwrap(), file)
}

fn request(page: u32) -> TileRequest {
    TileRequest { page, rect: TileRect { x: 512, y: 512, width: 512, height: 512 }, scale: 2.0, stride: 2048, generation: 1 }
}

fn surface() -> SharedMemory {
    SharedMemory::create(512 * 512 * 4).unwrap()
}

fn done(result: Result<TileResult, RenderFailure>) -> recto_render_service::Tile {
    match result {
        Ok(TileResult::Done(tile)) => tile,
        other => panic!("{other:?}"),
    }
}

/// Failure: an ordinary page goes near the sequence at all, costing it a kill or a larger worker.
#[test]
fn step_1_an_ordinary_page_stays_at_the_normal_limit() {
    let (mut doc, _file) = doc("fits_normal");
    let tile = done(doc.render_tile(&request(0), &surface()));
    assert_eq!(tile.low_res, None);
    assert_eq!(doc.stats().heavy_retries, 0);
    assert_eq!(doc.stats().memory_kills, 0);
    assert_eq!(doc.page_draw_state(0), PageDrawState::Sharp);
}

/// Failure: a page past the normal limit fails, though it fits in the larger one the owner approved. It must come back sharp, and later tiles of it must go straight to the
/// larger limit rather than being killed again first.
#[test]
fn step_2_a_page_past_the_limit_is_retried_alone_at_the_larger_one() {
    let (mut doc, _file) = doc("fits_heavy");
    let tile = done(doc.render_tile(&request(0), &surface()));
    assert_eq!(tile.low_res, None, "it should have come back sharp");
    assert_eq!(doc.stats().memory_kills, 1);
    assert_eq!(doc.stats().heavy_retries, 1);
    assert_eq!(doc.page_draw_state(0), PageDrawState::NeedsHeavy);
    done(doc.render_tile(&request(0), &surface()));
    assert_eq!(doc.stats().memory_kills, 1, "the second tile was killed again");
    assert_eq!(doc.stats().spawns, 2, "and it shares the heavy worker");
}

/// Failure: the heavy worker goes on to other pages, so a page isn't alone and the larger limit stays in use after it's needed.
#[test]
fn step_2_the_heavy_worker_holds_that_page_alone() {
    let slot = HeavySlot::new();
    let (mut doc, _file) = doc_with("fits_heavy", policy(), &slot);
    done(doc.render_tile(&request(0), &surface()));
    let heavy = doc.worker_pid();
    assert!(slot.is_taken());
    done(doc.render_tile(&request(1), &surface()));
    assert_ne!(doc.worker_pid(), heavy, "another page went to the heavy worker");
    assert!(!slot.is_taken(), "the slot should be free once that page is done with");
}

/// Failure: a Mac short of memory is asked for another gigabyte anyway. The page must come back at a lower resolution, say why, and be free to try again later.
#[test]
fn step_2_needs_the_mac_to_have_memory_free() {
    // Above any possible reading, so this doesn't depend on what this Mac has free.
    let tight = Policy { heavy_min_free_percent: 101, ..policy() };
    let (mut doc, _file) = doc_with("fits_heavy_or_low_res", tight, &HeavySlot::new());
    let tile = done(doc.render_tile(&request(0), &surface()));
    let low_res = tile.low_res.expect("it should say this is a lower resolution");
    assert_eq!(low_res.why, LowResWhy::MemoryTight);
    assert!(low_res.why.may_clear());
    assert_eq!(doc.stats().heavy_retries, 0, "no larger worker when the Mac can't spare it");
    assert_eq!(doc.page_draw_state(0), PageDrawState::NeedsHeavy, "not given up on: memory may come back");
}

/// Failure: a page past every limit comes out blank or as an error, though a smaller version fits. It must come back at a lower resolution, reported as such, and the next tile
/// must go straight there rather than running the kills again.
#[test]
fn step_3_a_lower_resolution_version_is_drawn_and_reported() {
    let (mut doc, _file) = doc("fits_low_res");
    let tile = done(doc.render_tile(&request(0), &surface()));
    let low_res = tile.low_res.expect("it should say this is a lower resolution");
    assert_eq!(low_res.why, LowResWhy::TooHeavy);
    assert!(!low_res.why.may_clear());
    assert_eq!(low_res.scale, 0.5);
    assert_eq!(low_res.rect, TileRect { x: 128, y: 128, width: 128, height: 128 }, "the same part of the page, a quarter the size");
    assert_eq!(doc.page_draw_state(0), PageDrawState::LowResOnly);
    assert_eq!(doc.stats().memory_kills, 2);
    let spawns = doc.stats().spawns;
    done(doc.render_tile(&request(0), &surface()));
    assert_eq!(doc.stats().memory_kills, 2, "it ran the failed steps again");
    assert_eq!(doc.stats().spawns, spawns);
}

/// Failure: the lower-resolution rectangle runs off the page's smaller size at its edge, and the worker refuses the very tile that should have been saved.
#[test]
fn step_3_stays_on_the_page_at_its_edges() {
    let (mut doc, _file) = doc("fits_low_res");
    // The page is 1224 x 1584 at this scale, and 306 x 396 at a quarter of it.
    let corner = TileRequest { rect: TileRect { x: 1223, y: 1583, width: 1, height: 1 }, ..request(0) };
    let low_res = done(doc.render_tile(&corner, &surface())).low_res.unwrap();
    assert!(low_res.rect.x < 306 && low_res.rect.y < 396, "{:?}", low_res.rect);
    assert!(low_res.rect.width >= 1 && low_res.rect.height >= 1);
    let tiny = TileRequest { scale: proto::MIN_SCALE, rect: TileRect { x: 0, y: 0, width: 4, height: 4 }, stride: 16, ..request(0) };
    assert!(doc.render_tile(&tiny, &surface()).is_err(), "there's no lower to go, so it should say it can't be drawn");
}

/// Failure: a page that fails all the way down stays blank with no word, or runs its kills again on every tile, which is what a hostile file wants. Said plainly, and remembered.
#[test]
fn step_4_failing_all_the_way_down_is_said_plainly_and_remembered() {
    let (mut doc, _file) = doc("fits_nothing");
    let failure = doc.render_tile(&request(0), &surface()).unwrap_err();
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
    assert_eq!(failure.code(), "page_too_large");
    assert!(failure.message().contains("lower resolution"), "{}", failure.message());
    assert_eq!(doc.stats().memory_kills, 3, "each step once");
    let spawns = doc.stats().spawns;
    assert_eq!(doc.render_tile(&request(0), &surface()).unwrap_err(), failure);
    assert_eq!(doc.stats().spawns, spawns, "it ran a hostile page again");
    assert_eq!(doc.page_draw_state(0), PageDrawState::Failed(KillReason::MemoryLimit));
    assert!(done(doc.render_tile(&request(1), &surface())).low_res.is_none(), "the rest of the file is fine");
}

/// Failure: two heavy pages each get a large worker, so the worst case doubles past what the owner approved. The second must get its lower-resolution version at once, while the first
/// is still running, so nothing waits. When the slot frees, it must be able to go sharp.
#[test]
fn only_one_heavy_worker_across_the_session_and_nobody_waits_for_it() {
    let slot = HeavySlot::new();
    let (mut first, _a) = doc_with("fits_heavy", policy(), &slot);
    let (mut second, _b) = doc_with("fits_heavy_or_low_res", policy(), &slot);
    done(first.render_tile(&request(0), &surface()));
    assert!(slot.is_taken());
    let waiting = done(second.render_tile(&request(0), &surface()));
    let low_res = waiting.low_res.expect("the slot is taken, so this should be a lower resolution");
    assert_eq!(low_res.why, LowResWhy::HeavyBusy);
    assert!(low_res.why.may_clear());
    assert_eq!(second.stats().heavy_retries, 0, "a second heavy worker started");
    assert_eq!(second.page_draw_state(0), PageDrawState::NeedsHeavy, "not given up on");
    first.kill_worker();
    assert!(!slot.is_taken());
    let sharp = done(second.render_tile(&request(0), &surface()));
    assert_eq!(sharp.low_res, None, "with the slot free it should go sharp");
    assert_eq!(second.stats().heavy_retries, 1);
}

/// Failure: a heavy worker that left by itself keeps the slot, so no other page can ever have it.
#[test]
fn a_heavy_worker_gone_idle_gives_the_slot_back() {
    let slot = HeavySlot::new();
    let quick = Policy { idle_exit: Duration::from_millis(30), idle_grace: Duration::from_millis(30), ..policy() };
    let (mut doc, _file) = doc_with("fits_heavy", quick, &slot);
    done(doc.render_tile(&request(0), &surface()));
    assert!(slot.is_taken());
    common::wait_until("the worker to end", || !doc.has_worker());
    assert!(!slot.is_taken());
}

/// Failure: a whole-page export pastes a low-resolution strip among sharp ones, or writes a picture of the wrong size. It must start again at the lower scale and say so.
#[test]
fn a_whole_page_restarts_at_the_lower_scale_and_says_so() {
    let (mut doc, _file) = doc("fits_low_res");
    let (mut begins, mut rows) = (Vec::new(), 0u32);
    let mut surface = None;
    let drawn = draw_whole_page(&mut doc, 0, 2.0, &OutputCaps::default(), &mut surface, |event| {
        match event {
            PageEvent::Begin { width, height, scale, low_res } => {
                begins.push((width, height, scale, low_res));
                rows = 0;
            }
            PageEvent::Rows { rect, .. } => rows += rect.height,
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(begins, vec![(1224, 1584, 2.0, None), (306, 396, 0.5, Some(LowResWhy::TooHeavy))]);
    assert_eq!(rows, 396, "the rows of the second picture, whole");
    assert_eq!((drawn.width, drawn.height, drawn.scale, drawn.low_res), (306, 396, 0.5, Some(LowResWhy::TooHeavy)));
    assert_eq!(doc.page_draw_state(0), PageDrawState::LowResOnly);
}

/// Failure: a whole page past every limit reports success with an empty picture, or tries for ever.
#[test]
fn a_whole_page_that_fails_all_the_way_down_says_so() {
    let (mut doc, _file) = doc("fits_nothing");
    let mut surface = None;
    let failure = draw_whole_page(&mut doc, 0, 2.0, &OutputCaps::default(), &mut surface, |_| Ok(())).unwrap_err();
    assert_eq!(failure, RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
    assert_eq!(doc.stats().memory_kills, 3);
}

/// Failure: the larger limit is smaller than the normal one, as when a measurement sets its own limit, and a page is retried at a smaller limit than it just failed.
#[test]
fn no_larger_limit_means_no_heavy_step() {
    let none = Policy { heavy_limit_bytes: 0, ..policy() };
    let (mut doc, _file) = doc_with("fits_low_res", none, &HeavySlot::new());
    assert_eq!(done(doc.render_tile(&request(0), &surface())).low_res.unwrap().why, LowResWhy::TooHeavy);
    assert_eq!(doc.stats().heavy_retries, 0);
    assert_eq!(doc.stats().memory_kills, 1);
}

/// Failure: reading a heavy page's text kills its heavy worker or takes the slot, so selecting text on a map reloads the map.
#[test]
fn text_jobs_share_a_heavy_worker_and_dont_need_one() {
    let (mut doc, _file) = doc("fits_heavy");
    done(doc.render_tile(&request(0), &surface()));
    let heavy = doc.worker_pid();
    assert_eq!(doc.page_characters(0, false, false).unwrap().chars.len(), 1);
    assert_eq!(doc.worker_pid(), heavy);
}
