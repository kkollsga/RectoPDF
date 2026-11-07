//! The memory sequence, step by step, against a stand-in worker that fits or doesn't fit each limit by design. So nothing here depends on how much memory a real page
//! takes on this machine: limits and growth are the test's own. Owner, 2026-11-06: normal limit, one page alone at the larger limit if the Mac can spare it, a
//! lower-resolution version reported as such, then a plain failure, remembered.

mod common;

use recto_platform::isolate::{self, Channel, SharedMemory};
use recto_proto::{self as proto, CharRecord, FailCode, Kind, PageSize, RegionReply, RenderJob, TextJob, TextReply, TileKind, TileRect, TileReply, TileStatus};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{
    DocRenderer, DrawStrategy, ExportRefusal, HeavySlot, KillReason, LowResWhy, OutputCaps, PageDrawState, PageEvent, Policy, RenderFailure, ServiceConfig, TileRequest, TileResult, ViewportOptions, control,
    draw_whole_page, measure_viewport,
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
    let mut cancel_word = None;
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        let reply = match frame.kind {
            Kind::Open => {
                let mut handles = frame.handles;
                cancel_word = Some(SharedMemory::map(handles.pop().unwrap(), control::LEN).unwrap());
                match mode.as_str() {
                    "open_unreadable" => proto::encode_failed(0, FailCode::Unreadable),
                    "open_not_pdf" => proto::encode_failed(0, FailCode::NotPdf),
                    _ => proto::encode_opened(3),
                }
            }
            Kind::SizeRequest => {
                let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                PageSize { job_id, width: 612.0, height: 792.0 }.encode()
            }
            Kind::TraitsRequest => {
                let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                proto::PageTraits { job_id, largest_image_pixels: 0 }.encode()
            }
            Kind::TextRequest => {
                let job = TextJob::decode(&frame.payload).unwrap();
                TextReply { job_id: job.job_id, page: job.page, hidden_checked: job.hidden_check, chars: vec![CharRecord::new('a' as u32, 0)], ..TextReply::default() }.encode().unwrap()
            }
            Kind::Render => {
                let job = RenderJob::decode(&frame.payload).unwrap();
                let low_res = job.scale < 1.0;
                let in_heavy = limit >= HEAVY;
                if mode == "stale_then_grows" {
                    // In hand, then waits to be cancelled, and only then overruns: so the cancel is known to come before the kill.
                    std::fs::write(std::env::var("RECTO_FAKE_MARK").unwrap(), b"in hand").unwrap();
                    let word = cancel_word.as_ref().unwrap().atomic_u64(control::CANCEL_BEFORE_OFFSET).unwrap();
                    while job.generation >= word.load(std::sync::atomic::Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    leak_for_ever();
                }
                if mode == "slow" {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let fits = match mode.as_str() {
                    "slow" => true,
                    // A higher zoom is too much even for the larger limit, a lower one needs it, and a much lower one fits anywhere.
                    "heavy_below_3x" => job.scale <= 1.0 || (in_heavy && job.scale < 3.0),
                    "every_page_fits_heavy" => in_heavy,
                    "every_page_fits_nothing" => false,
                    _ if job.page != 0 => true,
                    _ => match mode.as_str() {
                        "fits_normal" => true,
                        "fits_heavy" => in_heavy,
                        "fits_low_res" => low_res,
                        "fits_heavy_or_low_res" => in_heavy || low_res,
                        _ => false,
                    },
                };
                if !fits {
                    leak_for_ever();
                }
                let tile = TileReply {
                    generation: job.generation,
                    job_id: job.job_id,
                    width: job.rect.width,
                    height: job.rect.height,
                    stride: job.stride,
                    status: TileStatus::Done(TileKind::Colour),
                    render_us: 1,
                };
                if job.tile_edge == 0 {
                    tile.encode()
                } else {
                    RegionReply { tile, kinds: vec![TileKind::Colour; proto::region_tiles(job.rect, job.tile_edge).count()] }.encode()
                }
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

/// Failure: every start of a heavy worker uses up the document's allowance, whether it was let go for resting, sent its fill of surfaces or truly overran. A page that needs the larger limit goes to a lower resolution for good after eight rests, and nothing ever restores it. Only overrunning counts.
#[test]
fn a_heavy_worker_let_go_again_and_again_uses_up_nothing() {
    let slot = HeavySlot::new();
    let (mut doc, _file) = doc_with("fits_heavy", policy(), &slot);
    let rests = policy().max_heavy_retries * 2 + 1;
    for _ in 0..rests {
        let tile = done(doc.render_tile(&request(0), &surface()));
        assert_eq!(tile.low_res, None, "still sharp, after {} starts", doc.stats().heavy_retries);
        doc.kill_worker();
        // The heavy slot is free when that process has really gone.
        common::wait_until("the heavy slot", || !slot.is_taken());
    }
    assert!(doc.stats().heavy_retries > policy().max_heavy_retries, "this has to pass the old allowance: {:?}", doc.stats());
    assert_eq!(doc.stats().heavy_pages, 1, "one page");
    assert_eq!(doc.page_draw_state(0), PageDrawState::NeedsHeavy);
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
    common::wait_until("the slot to free once that page is done with", || !slot.is_taken());
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
    common::wait_until("the slot to free", || !slot.is_taken());
    let sharp = done(second.render_tile(&request(0), &surface()));
    assert_eq!(sharp.low_res, None, "with the slot free it should go sharp");
    assert_eq!(second.stats().heavy_retries, 1);
}

fn region(page: u32) -> TileRequest {
    TileRequest { page, rect: TileRect { x: 512, y: 512, width: 1024, height: 1024 }, scale: 2.0, stride: 4096, generation: 1 }
}

fn region_surface() -> SharedMemory {
    SharedMemory::create(1024 * 1024 * 4).unwrap()
}

/// Failure: a region escapes the memory sequence, so a heavy page drawn as a region is stopped when one drawn as tiles would have gone on to the larger limit.
#[test]
fn a_region_goes_down_the_same_sequence() {
    let (mut heavy, _a) = doc("fits_heavy");
    let tile = done(heavy.render_region(&region(0), 512, &region_surface()));
    assert_eq!(tile.low_res, None);
    assert_eq!(tile.region_kinds.len(), 4);
    assert_eq!(heavy.stats().heavy_retries, 1);
    let (mut low, _b) = doc("fits_low_res");
    let tile = done(low.render_region(&region(0), 512, &region_surface()));
    let low_res = tile.low_res.expect("it should say this is a lower resolution");
    assert_eq!(low_res.rect, TileRect { x: 128, y: 128, width: 178, height: 256 }, "this region runs past the page's right edge, and the smaller one stops there");
    assert!(tile.region_kinds.is_empty(), "a lower-resolution pass isn't on the tile grid");
    let (mut none, _c) = doc("fits_nothing");
    assert_eq!(none.render_region(&region(0), 512, &region_surface()).unwrap_err(), RenderFailure::PageFailed { page: 0, reason: KillReason::MemoryLimit });
}

/// Failure: a heavy worker that left by itself keeps the slot, so no other page can ever have it.
#[test]
fn a_heavy_worker_gone_idle_gives_the_slot_back() {
    let slot = HeavySlot::new();
    let quick = Policy { idle_exit: Duration::from_millis(30), idle_grace: Duration::from_millis(30), ..policy() };
    let (mut doc, _file) = doc_with("fits_heavy", quick, &slot);
    done(doc.render_tile(&request(0), &surface()));
    assert!(slot.is_taken());
    // Nobody touches the document, so this is the process going that frees the slot.
    common::wait_until("the slot to free", || !slot.is_taken());
    assert!(!doc.has_worker());
}

fn export(doc: &mut DocRenderer) -> Result<recto_render_service::PageDrawn, RenderFailure> {
    let mut surface = None;
    draw_whole_page(doc, 0, 2.0, 0, &OutputCaps::default(), &mut surface, |_| Ok(()))
}

/// Failure: an export comes out smaller than asked, because a smaller version happened to fit. Owner, 2026-11-07: sharp or not at all. This stand-in would draw the lower scale happily,
/// so only a refusal passes, and no job below the asked-for scale may even be tried.
#[test]
fn an_export_never_draws_a_lower_resolution() {
    let (mut doc, _file) = doc("fits_low_res");
    let (mut begins, mut pieces) = (Vec::new(), None);
    let failure = draw_whole_page(&mut doc, 0, 2.0, 0, &OutputCaps::default(), &mut pieces, |event| {
        if let PageEvent::Begin { width, height } = event {
            begins.push((width, height));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(failure, RenderFailure::ExportRefused { page: 0, why: ExportRefusal::TooHeavy });
    assert_eq!(failure.code(), "export_page_too_heavy");
    assert!(failure.message().contains("no file was written"), "{}", failure.message());
    assert_eq!(begins, vec![(1224, 1584)], "only ever the size asked for");
    assert_eq!(doc.stats().memory_kills, 2, "the normal limit and the larger one");
    assert_eq!(doc.stats().low_res_tiles, 0);
    assert_eq!(doc.page_draw_state(0), PageDrawState::LowResOnly, "the viewer can still show it");
    assert!(done(doc.render_tile(&request(0), &surface())).low_res.is_some(), "and does");
}

/// Failure: a Mac short of memory is asked for another gigabyte by an export, or the export quietly settles for less. It must refuse plainly, without taking the slot.
#[test]
fn an_export_refuses_when_the_mac_has_no_memory_to_spare() {
    let slot = HeavySlot::new();
    // Above any possible reading, so this doesn't depend on what this Mac has free.
    let tight = Policy { heavy_min_free_percent: 101, ..policy() };
    let (mut doc, _file) = doc_with("fits_heavy_or_low_res", tight, &slot);
    let failure = export(&mut doc).unwrap_err();
    assert_eq!(failure, RenderFailure::ExportRefused { page: 0, why: ExportRefusal::MemoryTight });
    assert_eq!(failure.code(), "export_memory_tight");
    assert_eq!(doc.stats().heavy_retries, 0);
    assert_eq!(slot.waiting(), 0, "there's no point waiting for a slot it couldn't use");
    assert!(!slot.is_taken());
}

/// Failure: an export past the normal limit fails or comes out low-res, though the larger limit fits it. With the slot free, it should go straight to sharp.
#[test]
fn an_export_uses_the_larger_limit_when_the_slot_is_free() {
    let (mut doc, _file) = doc("fits_heavy");
    let drawn = export(&mut doc).unwrap();
    assert_eq!((drawn.width, drawn.height), (1224, 1584));
    assert_eq!(doc.stats().heavy_retries, 1);
}

/// What an export on its own thread comes back with: its result, and how many heavy workers it used.
type ExportEnd = (Result<recto_render_service::PageDrawn, RenderFailure>, u32);

/// Starts an export of a heavy page on its own thread, and waits until it's asleep waiting for the slot.
fn export_waiting_for(slot: &HeavySlot, policy: Policy) -> (recto_render_service::CancelHandle, std::thread::JoinHandle<ExportEnd>, TempPdf) {
    let (mut waiter, file) = doc_with("fits_heavy", policy, slot);
    let cancel = waiter.cancel_handle();
    let thread = std::thread::spawn(move || {
        let result = export(&mut waiter);
        (result, waiter.stats().heavy_retries)
    });
    common::wait_until("the export to be waiting", || slot.waiting() == 1);
    (cancel, thread, file)
}

/// Failure: an export that finds the slot taken gives up or comes out low-res, rather than waiting as the owner decided. Here it must still be waiting while the slot is held, and draw sharp
/// once it frees.
#[test]
fn an_export_waits_for_the_slot_then_draws_sharp() {
    let slot = HeavySlot::new();
    let (mut holder, _a) = doc_with("fits_heavy", policy(), &slot);
    done(holder.render_tile(&request(0), &surface()));
    assert!(slot.is_taken());
    let (_cancel, thread, _b) = export_waiting_for(&slot, policy());
    assert!(!thread.is_finished(), "it should wait while the slot is held");
    holder.kill_worker();
    let (result, heavy_retries) = thread.join().unwrap();
    assert_eq!(result.unwrap().width, 1224, "sharp, at the size asked for");
    assert_eq!(heavy_retries, 1);
    assert_eq!(slot.waiting(), 0);
}

/// Failure: the slot stays with a heavy worker that left by itself, because nobody touched its document, so an export waits out its whole limit for a slot nothing is using.
#[test]
fn an_idle_heavy_worker_lets_a_waiting_export_through_with_nobody_touching_it() {
    let slot = HeavySlot::new();
    let quick = Policy { idle_exit: Duration::from_millis(200), idle_grace: Duration::from_millis(50), ..policy() };
    let (mut holder, _a) = doc_with("fits_heavy", quick, &slot);
    done(holder.render_tile(&request(0), &surface()));
    let (_cancel, thread, _b) = export_waiting_for(&slot, policy());
    // `holder` isn't called again: the reaper ends it and frees the slot.
    let (result, _) = thread.join().unwrap();
    assert!(result.is_ok(), "{result:?}");
    drop(holder);
}

/// Failure: a cancelled wait stays asleep until the slot frees or time runs out, or keeps its place, so viewers are held back by an export nobody wants. It must end at once, with nothing
/// drawn, and a viewer must be able to have the slot straight after.
#[test]
fn a_cancelled_wait_ends_at_once_and_gives_up_its_place() {
    let slot = HeavySlot::new();
    let (mut holder, _a) = doc_with("fits_heavy", policy(), &slot);
    done(holder.render_tile(&request(0), &surface()));
    // A wait far longer than this test, so only the cancel can end it.
    let (cancel, thread, _b) = export_waiting_for(&slot, Policy { export_slot_wait: Duration::from_secs(3600), ..policy() });
    cancel.cancel_before(1);
    let (result, heavy_retries) = thread.join().unwrap();
    assert_eq!(result.unwrap_err(), RenderFailure::Cancelled);
    assert_eq!(heavy_retries, 0);
    assert_eq!(slot.waiting(), 0, "it kept its place");
    holder.kill_worker();
    common::wait_until("the holder's slot to free", || !slot.is_taken());
    let (mut viewer, _c) = doc_with("fits_heavy", policy(), &slot);
    assert_eq!(done(viewer.render_tile(&request(0), &surface())).low_res, None, "a viewer should get the slot now");
}

/// Failure: an export waits for ever, or a timed-out wait keeps its place. It must refuse plainly when its limit runs out.
#[test]
fn an_export_refuses_when_its_wait_runs_out() {
    let slot = HeavySlot::new();
    let (mut holder, _a) = doc_with("fits_heavy", policy(), &slot);
    done(holder.render_tile(&request(0), &surface()));
    let (mut waiter, _b) = doc_with("fits_heavy", Policy { export_slot_wait: Duration::from_millis(10), ..policy() }, &slot);
    let failure = export(&mut waiter).unwrap_err();
    assert_eq!(failure, RenderFailure::ExportRefused { page: 0, why: ExportRefusal::WaitTimedOut });
    assert_eq!(failure.code(), "export_wait_timed_out");
    assert_eq!(slot.waiting(), 0);
}

/// Failure: an on-screen page starts waiting too, or takes the slot from under an export that's waiting. A viewer must still get its lower resolution at once, with the export asleep beside it.
#[test]
fn on_screen_pages_never_wait_even_beside_a_waiting_export() {
    let slot = HeavySlot::new();
    let (mut holder, _a) = doc_with("fits_heavy", policy(), &slot);
    done(holder.render_tile(&request(0), &surface()));
    let (_cancel, thread, _b) = export_waiting_for(&slot, policy());
    let (mut viewer, _c) = doc_with("fits_heavy_or_low_res", policy(), &slot);
    assert_eq!(done(viewer.render_tile(&request(0), &surface())).low_res.unwrap().why, LowResWhy::HeavyBusy);
    assert_eq!(viewer.stats().heavy_retries, 0);
    assert_eq!(slot.waiting(), 1, "it never joined the wait");
    holder.kill_worker();
    assert!(thread.join().unwrap().0.is_ok());
    // With the slot held by the export's worker or free, and nobody waiting, the viewer is back to taking it if it can.
    assert_eq!(slot.waiting(), 0);
}

/// Failure: the slot leaks when its holder is killed from outside or its thread panics, so no other heavy page can ever draw sharp, and every export times out.
#[test]
fn the_slot_is_never_leaked_by_a_kill_or_a_panic() {
    let slot = HeavySlot::new();
    let (mut killed, _a) = doc_with("fits_heavy", policy(), &slot);
    done(killed.render_tile(&request(0), &surface()));
    assert!(slot.is_taken());
    common::kill_process(killed.worker_pid().unwrap());
    common::wait_until("the slot to free after an outside kill", || !slot.is_taken());
    drop(killed);

    let (mut panicking, _b) = doc_with("fits_heavy", policy(), &slot);
    let thread = std::thread::spawn(move || {
        done(panicking.render_tile(&request(0), &surface()));
        std::panic::resume_unwind(Box::new("on purpose"));
    });
    assert!(thread.join().is_err());
    common::wait_until("the slot to free after a panic", || !slot.is_taken());
}

/// Failure: a temporary condition becomes permanent: with the slot busy, the page's smaller version doesn't fit either, and it's marked failed for good, though the larger limit was never tried.
/// It must say it can't be drawn right now, without running that kill on every tile, and go sharp as soon as the slot frees.
#[test]
fn a_busy_slot_never_makes_a_page_fail_for_good() {
    let slot = HeavySlot::new();
    let (mut holder, _a) = doc_with("fits_heavy", policy(), &slot);
    done(holder.render_tile(&request(0), &surface()));
    // Fits the larger limit, but not the lower resolution in a normal worker.
    let (mut doc, _b) = doc_with("fits_heavy", policy(), &slot);
    let first = doc.render_tile(&request(0), &surface()).unwrap_err();
    assert_eq!(first, RenderFailure::NotDrawableNow { page: 0, why: LowResWhy::HeavyBusy });
    assert_eq!(first.code(), "page_not_drawable_now");
    assert_eq!(doc.page_draw_state(0), PageDrawState::NeedsHeavy, "not given up on");
    let kills = doc.stats().memory_kills;
    assert_eq!(doc.render_tile(&request(0), &surface()).unwrap_err(), first);
    assert_eq!(doc.stats().memory_kills, kills, "it ran the kill again");
    holder.kill_worker();
    common::wait_until("the slot to free", || !slot.is_taken());
    assert_eq!(done(doc.render_tile(&request(0), &surface())).low_res, None, "with the slot free it should go sharp");
}

/// Failure: tight memory does the same.
#[test]
fn tight_memory_never_makes_a_page_fail_for_good() {
    let (mut doc, _file) = doc_with("fits_heavy", Policy { heavy_min_free_percent: 101, ..policy() }, &HeavySlot::new());
    assert_eq!(doc.render_tile(&request(0), &surface()).unwrap_err(), RenderFailure::NotDrawableNow { page: 0, why: LowResWhy::MemoryTight });
    assert_eq!(doc.page_draw_state(0), PageDrawState::NeedsHeavy);
}

/// Failure: a scale too low to have a smaller version marks the page failed at every zoom, though that says nothing about the others.
#[test]
fn a_tiny_scale_doesnt_fail_the_page_at_other_zooms() {
    let (mut doc, _file) = doc("fits_low_res");
    let tiny = TileRequest { scale: 1.0 / 32.0, rect: TileRect { x: 0, y: 0, width: 4, height: 4 }, stride: 16, ..request(0) };
    assert!(done(doc.render_tile(&tiny, &surface())).low_res.is_none(), "this stand-in fits under scale 1, so this one simply draws");
    let (mut none, _b) = self::doc("fits_nothing");
    assert_eq!(none.render_tile(&tiny, &surface()).unwrap_err(), RenderFailure::TooLargeAtThisScale);
    assert!(!matches!(none.page_draw_state(0), PageDrawState::Failed(_)), "{:?}", none.page_draw_state(0));
}

/// Failure: one kill at a very high zoom leaves the page at a lower resolution at every zoom, for as long as the tab is open.
#[test]
fn a_lower_zoom_gets_its_own_try() {
    let (mut doc, _file) = doc("fits_low_res");
    assert!(done(doc.render_tile(&request(0), &surface())).low_res.is_some());
    assert_eq!(doc.page_draw_state_at(0, 2.0), PageDrawState::LowResOnly);
    assert_eq!(doc.page_draw_state_at(0, 8.0), PageDrawState::LowResOnly, "a higher zoom costs more, so it holds there too");
    assert_eq!(doc.page_draw_state_at(0, 1.9), PageDrawState::LowResOnly, "a small change in zoom isn't a new try");
    assert_eq!(doc.page_draw_state_at(0, 0.5), PageDrawState::Sharp, "a much lower zoom starts clean");
    let lower = TileRequest { scale: 0.5, rect: TileRect { x: 0, y: 0, width: 256, height: 256 }, ..request(0) };
    assert_eq!(done(doc.render_tile(&lower, &surface())).low_res, None, "and this page fits at that zoom");
}

/// Failure: with the slot busy, each tile of a page that needs it kills and respawns a normal worker on the way to learning that. The normal worker should stay.
#[test]
fn a_busy_slot_costs_no_respawn() {
    let slot = HeavySlot::new();
    let (mut holder, _a) = doc_with("fits_heavy", policy(), &slot);
    done(holder.render_tile(&request(0), &surface()));
    let (mut doc, _b) = doc_with("fits_heavy_or_low_res", policy(), &slot);
    done(doc.render_tile(&request(0), &surface()));
    let spawns = doc.stats().spawns;
    let worker = doc.worker_pid();
    for _ in 0..5 {
        assert!(done(doc.render_tile(&request(0), &surface())).low_res.is_some());
    }
    assert_eq!(doc.stats().spawns, spawns);
    assert_eq!(doc.worker_pid(), worker);
}

/// Failure: a hostile file with a heavy page every few pages asks for a gigabyte as often as it likes. Past the cap, pages go to their lower resolution without it.
#[test]
fn a_document_has_a_cap_on_heavy_retries() {
    let (mut doc, _file) = doc_with("every_page_fits_heavy", Policy { max_heavy_retries: 1, ..policy() }, &HeavySlot::new());
    assert_eq!(done(doc.render_tile(&request(0), &surface())).low_res, None);
    assert_eq!(doc.stats().heavy_retries, 1);
    // The second page's lower resolution doesn't fit this stand-in either, so it fails, and it never got a heavy worker.
    assert!(doc.render_tile(&request(1), &surface()).is_err());
    assert_eq!(doc.stats().heavy_retries, 1, "past the cap");
    doc.forget_failures();
    assert_eq!(done(doc.render_tile(&request(1), &surface())).low_res, None, "asked to try again, it may");
}

/// Failure: a file of more bad pages than can be remembered re-runs the kills on every pass, for ever. Past the cap the document stops drawing, with no process started and a plain reason.
#[test]
fn a_document_has_a_cap_on_kills() {
    let (mut doc, _file) = doc_with("every_page_fits_nothing", Policy { max_kills_per_document: 2, ..policy() }, &HeavySlot::new());
    let _ = doc.render_tile(&request(0), &surface());
    assert!(doc.stats().memory_kills >= 2);
    let spawns = doc.stats().spawns;
    let stopped = doc.render_tile(&request(1), &surface()).unwrap_err();
    assert_eq!(stopped, RenderFailure::DocumentStopped);
    assert!(stopped.message().contains("stopped drawing"), "{}", stopped.message());
    assert_eq!(doc.stats().spawns, spawns, "it started another process");
    assert_eq!(doc.page_characters(1, false, false).unwrap_err(), RenderFailure::DocumentStopped);
}

/// Failure: a heavy worker keeps its gigabyte and the slot for the normal idle time, so a page you've stopped looking at holds up everything else for half a minute. A normal idle time of an hour
/// here means only the heavy worker's own shorter one can free the slot.
#[test]
fn a_heavy_worker_lets_go_sooner_than_a_normal_one() {
    let slot = HeavySlot::new();
    let quick = Policy { idle_exit: Duration::from_secs(3600), idle_grace: Duration::from_millis(30), heavy_idle_exit: Duration::from_millis(30), ..policy() };
    let (mut doc, _file) = doc_with("fits_heavy", quick, &slot);
    done(doc.render_tile(&request(0), &surface()));
    assert!(slot.is_taken());
    common::wait_until("the slot to free", || !slot.is_taken());
}

/// Failure: a job cancelled during a step goes on to the next: takes the slot and starts a heavy worker for a tile nobody wants.
#[test]
fn a_cancel_between_steps_ends_the_sequence() {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let mark = file.0.with_file_name("in-hand");
    let slot = HeavySlot::new();
    let mut config = config("stale_then_grows", &slot);
    config.extra_env.push(("RECTO_FAKE_MARK".into(), mark.to_str().unwrap().into()));
    // A long grace, so that the kill is memory's and not the cancel's.
    let mut doc = DocRenderer::new(&file.0, None, config, Policy { cancel_grace: Duration::from_secs(600), ..policy() }).unwrap();
    let cancel = doc.cancel_handle();
    let watched = mark.clone();
    let canceller = std::thread::spawn(move || {
        common::wait_until("the job to be in hand", || watched.exists());
        cancel.cancel_before(2);
    });
    let result = doc.render_tile(&request(0), &surface()).unwrap();
    canceller.join().unwrap();
    let _ = std::fs::remove_file(&mark);
    assert_eq!(result, TileResult::Cancelled { worker_killed: false });
    assert_eq!(doc.stats().memory_kills, 1);
    assert_eq!(doc.stats().heavy_retries, 0, "a stale job started a heavy worker");
    assert!(!slot.is_taken());
}

/// Failure: a file the worker refuses as unreadable isn't remembered, because that word is also for not getting as far as asking, so each page of a search starts a process to hear the same answer. Or it's remembered for the renderer's life, though it covers a failed read of the file's size and errors with no reason given, and may pass. It holds for a moment, and no more.
#[test]
fn a_temporary_open_failure_holds_only_for_a_moment() {
    let (mut held, _file) = doc_with("open_unreadable", Policy { open_retry_pause: Duration::from_secs(60), ..policy() }, &HeavySlot::new());
    for _ in 0..5 {
        assert_eq!(held.page_characters(0, false, false).unwrap_err(), RenderFailure::Unavailable);
    }
    assert_eq!(held.stats().spawns, 1, "one process was enough to learn that");
    assert!(!held.has_worker());
    let (mut passes, _file) = doc_with("open_unreadable", Policy { open_retry_pause: Duration::from_millis(50), ..policy() }, &HeavySlot::new());
    assert_eq!(passes.page_count().unwrap_err(), RenderFailure::Unavailable);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(passes.page_count().unwrap_err(), RenderFailure::Unavailable);
    assert_eq!(passes.stats().spawns, 2, "the moment has passed, with nobody having to say so");
    held.forget_failures();
    assert_eq!(held.page_count().unwrap_err(), RenderFailure::Unavailable);
    assert_eq!(held.stats().spawns, 2, "and asking again is what forgetting is for");
}

/// Failure: what's learned about a file's bytes sticks for the renderer's life, so a damaged file the user replaces stays "not a PDF" until the tab closes, and no production code calls `forget_failures`. It holds for that file, and a different one gets a fresh try.
#[test]
fn a_failure_about_the_bytes_holds_for_that_file_and_no_other() {
    let (mut doc, file) = doc("open_not_pdf");
    for _ in 0..5 {
        assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    }
    assert_eq!(doc.stats().spawns, 1, "the same file gets the same answer, for as long as it takes");
    std::fs::write(&file.0, b"%PDF-1.7\nand now it's a different file").unwrap();
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf, "the stand-in says the same of everything");
    assert_eq!(doc.stats().spawns, 2, "but it was asked again");
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
    assert_eq!(doc.stats().spawns, 2, "and the new file's answer holds in turn");
    std::fs::remove_file(&file.0).unwrap();
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotFound, "a file that's gone says so");
    std::fs::write(&file.0, b"%PDF-1.7\nback").unwrap();
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::NotPdf);
}

/// Failure: a kill at a lower zoom overwrites what a higher zoom learned, and the other way round, so a reader going between two zooms runs the same kills again each time, for as long as its heavy retries last. Each scale keeps what it learned.
#[test]
fn what_one_zoom_learns_doesnt_take_away_what_another_learned() {
    let (mut doc, _file) = doc("heavy_below_3x");
    let at = |scale: f32| TileRequest { scale, ..request(0) };
    assert!(done(doc.render_tile(&at(4.0), &surface())).low_res.is_some(), "too much even for the larger limit at 4x");
    assert_eq!(doc.page_draw_state_at(0, 4.0), PageDrawState::LowResOnly);
    assert_eq!(doc.page_draw_state_at(0, 2.0), PageDrawState::Sharp, "a much lower zoom gets its own try");
    assert_eq!(done(doc.render_tile(&at(2.0), &surface())).low_res, None, "and fits the larger limit at 2x");
    assert_eq!(doc.page_draw_state_at(0, 2.0), PageDrawState::NeedsHeavy);
    assert_eq!(doc.page_draw_state_at(0, 4.0), PageDrawState::LowResOnly, "what 2x learned mustn't take away what 4x did");
    assert_eq!(doc.page_draw_state(0), PageDrawState::LowResOnly, "the worst that's known");
    let kills = doc.stats().memory_kills;
    for _ in 0..3 {
        assert!(done(doc.render_tile(&at(4.0), &surface())).low_res.is_some());
        assert_eq!(done(doc.render_tile(&at(2.0), &surface())).low_res, None);
    }
    assert_eq!(doc.stats().memory_kills, kills, "going between the two runs no kill it has run already");
    doc.forget_failures();
    assert_eq!(doc.page_draw_state(0), PageDrawState::Sharp);
}

/// Failure: the viewport budget's cap on time is only looked at between pages, so a page of many slow tiles or regions runs on past it, at each one's own deadline. Here each pass takes at least 100 ms and one page alone needs more than 4 of them, so with a cap of 250 ms no more than 4 can start if it's looked at before each pass, and every one of them would if it isn't. Past the cap, what's left counts as failed rather than vanishing from the numbers.
#[test]
fn the_viewport_budget_stops_at_its_cap_within_a_page() {
    let cap = Duration::from_millis(250);
    // At 2x, one page is 3 x 4 tiles. At 12x it's too large for fewer than about 10 regions of 32 MiB.
    for (strategy, scale, window) in [(DrawStrategy::PerTile, 2.0, 2048), (DrawStrategy::Region, 12.0, 10_000)] {
        let options = |max_time| ViewportOptions { first_page: 0, scale, width: window, height: window, margin_tiles: 0, strategy, max_time };
        let (mut no_time, _file) = doc("slow");
        let steps = measure_viewport(&mut no_time, options(Duration::ZERO)).unwrap();
        assert!(steps.iter().all(|step| step.passes == 0 && step.tiles == 0 && step.failed > 0), "{strategy:?}, no time at all: {steps:?}");
        let (mut capped, _file) = doc("slow");
        let steps = measure_viewport(&mut capped, options(cap)).unwrap();
        let (passes, failed): (u32, u32) = (steps.iter().map(|step| step.passes).sum(), steps.iter().map(|step| step.failed).sum());
        assert!(passes <= 4, "{strategy:?}: {passes} passes of at least 100 ms each started inside 250 ms: {steps:?}");
        assert!(failed > 0, "{strategy:?}: what wasn't drawn must say so: {steps:?}");
    }
}

/// Failure: the viewport budget counts low-resolution tiles as cached, so the next step looks cheaper than the app would find it, and the numbers we rely on flatter the page.
#[test]
fn the_viewport_budget_doesnt_count_low_res_tiles_as_done() {
    let (mut doc, _file) = doc("fits_low_res");
    let options = ViewportOptions { first_page: 0, scale: 2.0, width: 1024, height: 1024, margin_tiles: 0, strategy: DrawStrategy::PerTile, max_time: recto_render_service::BUDGET_MAX_TIME };
    let steps = measure_viewport(&mut doc, options).unwrap();
    assert_eq!(steps[0].low_res, steps[0].tiles, "every tile of this page comes back low-res: {steps:?}");
    assert!(steps[1].tiles >= steps[0].tiles, "they aren't done, so the scroll asks for them again: {steps:?}");
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
