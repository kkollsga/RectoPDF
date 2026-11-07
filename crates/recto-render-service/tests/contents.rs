//! A page's contents as the host reads them, against a stand-in worker that does what a compromised one would, or runs out of memory by design. So nothing here depends on PDFium, and the limits are the test's own.

use recto_platform::isolate::{self, Channel};
use recto_proto::contents::{ContentsHeader, ContentsJob, ImageCoding, MAX_CLIP_CHAIN, Record, TextMode, Why, begin_contents, finish_contents, put};
use recto_proto::{self as proto, FailCode, Kind, PageSize};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{ContentsEvent, ContentsRequest, DocRenderer, ExportRefusal, HeavySlot, KillReason, Policy, RenderFailure, ServiceConfig};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use std::ffi::OsString;
use std::time::{Duration, Instant};

const NORMAL: u64 = 16 * 1024 * 1024;
const HEAVY: u64 = 48 * 1024 * 1024;
const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn leak_for_ever() -> ! {
    let mut kept = Vec::new();
    loop {
        kept.push(vec![1u8; 4 * 1024 * 1024]);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One frame of what `RECTO_FAKE` says, for the `seq`-th ask.
fn frame_for(mode: &str, job: &ContentsJob, in_heavy: bool) -> Vec<u8> {
    let mut out = Vec::new();
    begin_contents(&mut out);
    let mut done = job.seq >= 2;
    if job.seq == 0 {
        put::page(&mut out, 100.0, 100.0, &IDENTITY);
    }
    match (mode, job.seq) {
        // Starts well, and then needs more than it's got, unless it has the larger limit.
        ("grows_midway", 1) if !in_heavy => leak_for_ever(),
        ("always_grows", 1) => leak_for_ever(),
        ("page_twice", 1) => put::page(&mut out, 100.0, 100.0, &IDENTITY),
        ("clip_too_deep", 1) => {
            for id in 1..=u32::from(MAX_CLIP_CHAIN) + 1 {
                put::clip_define_begin(&mut out, id, id - 1, &IDENTITY);
                let at = put::segs_begin(&mut out);
                put::segs_end(&mut out, at, 0);
            }
        }
        ("image_short", 1) => {
            put::image_begin(&mut out, ImageCoding::Pixels { channels: 1 }, Why::Image, 255, 2, 2, 4, &IDENTITY);
            put::image_end(&mut out);
        }
        // A row the host would have to hold 8 GiB of, three times over, filled from the next frames, with nothing reaching the file's cap until the row ends.
        ("row_too_long", 1) => {
            put::image_begin(&mut out, ImageCoding::Pixels { channels: 4 }, Why::Image, 255, i32::MAX as u32, 1, u64::from(i32::MAX as u32) * 4, &IDENTITY);
            done = false;
        }
        ("size_overflows", 1) => put::image_begin(&mut out, ImageCoding::Pixels { channels: 4 }, Why::Image, 255, u32::MAX, u32::MAX, 0, &IDENTITY),
        // Starts as a JPEG, and then isn't one, so a reader's decoder would get what the host never saw.
        ("not_really_a_jpeg", 1) => {
            put::image_begin(&mut out, ImageCoding::Jpeg, Why::Image, 255, 1, 1, 8, &IDENTITY);
            let at = put::image_bytes_begin(&mut out);
            out.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 2, 0x12, 0x34]);
            put::image_bytes_end(&mut out, at);
            put::image_end(&mut out);
        }
        // Past the count of what's an image, and the answer's arithmetic with it.
        ("jpeg_thats_not_an_image", 1) => put::image_begin(&mut out, ImageCoding::Jpeg, Why::Shading, 255, 1, 1, 8, &IDENTITY),
        ("jpeg_with_no_mask", 1) => put::image_begin(&mut out, ImageCoding::Jpeg, Why::JpegOverMask, 255, 1, 1, 8, &IDENTITY),
        ("mask_of_colour", 1) => put::image_begin(&mut out, ImageCoding::Pixels { channels: 3 }, Why::JpegMask, 255, 1, 1, 3, &IDENTITY),
        ("mask_with_no_jpeg", 1) => {
            put::image_begin(&mut out, ImageCoding::Pixels { channels: 1 }, Why::JpegMask, 255, 1, 1, 1, &IDENTITY);
            let at = put::image_bytes_begin(&mut out);
            out.push(9);
            put::image_bytes_end(&mut out, at);
            put::image_end(&mut out);
        }
        ("image_left_open", 1) => put::image_begin(&mut out, ImageCoding::Pixels { channels: 1 }, Why::Image, 255, 2, 2, 4, &IDENTITY),
        ("group_left_open", 1) => put::group_begin(&mut out, 7),
        ("garbage", 1) => out.extend_from_slice(b"\xFFnot a record"),
        ("not_finite", 1) => put::page(&mut out, f32::NAN, 1.0, &IDENTITY),
        ("endless", _) => done = false,
        (_, 1) => {
            put::path_begin(&mut out, &IDENTITY, Some([0, 0, 0, 255]), false, None, 0.0, 0, 0, 0.0, &[]);
            let at = put::segs_begin(&mut out);
            put::segs_end(&mut out, at, 0);
        }
        _ => {}
    }
    finish_contents(&mut out, job.job_id, if mode == "wrong_seq" && job.seq == 1 { 99 } else { job.seq }, done).unwrap();
    out
}

/// Not a test: the stand-in.
#[test]
fn fake_worker() {
    let Ok(mode) = std::env::var("RECTO_FAKE") else { return };
    let limit: u64 = std::env::var("RECTO_WORKER_LIMIT").unwrap().parse().unwrap();
    let channel = Channel::inherited().unwrap();
    send_all(&channel, &proto::encode_hello(std::process::id()), &[]).unwrap();
    let mut reader = FrameReader::default();
    let mut started = false;
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        let reply = match frame.kind {
            Kind::Open => proto::encode_opened(1, 1),
            Kind::SizeRequest => {
                let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                PageSize { job_id, width: 100.0, height: 100.0 }.encode()
            }
            Kind::ContentsRequest => {
                let job = ContentsJob::decode(&frame.payload).unwrap();
                started |= job.seq == 0;
                if (job.seq > 0 && !started) || (mode == "forgets_place" && job.seq == 1) {
                    proto::encode_failed(job.job_id, FailCode::BadRequest)
                } else {
                    frame_for(&mode, &job, limit >= HEAVY)
                }
            }
            _ => isolate::exit_now(0),
        };
        send_all(&channel, &reply, &[]).unwrap();
    }
}

fn renderer(mode: &str, file: &TempPdf) -> DocRenderer {
    let config = ServiceConfig {
        worker_exe: std::env::current_exe().unwrap(),
        pdfium_lib: "/nonexistent".into(),
        heavy_slot: HeavySlot::new(),
        extra_env: vec![("RECTO_FAKE".into(), mode.into())],
        replace_args: Some(["--exact", "fake_worker", "--nocapture", "--test-threads=1"].into_iter().map(OsString::from).collect()),
    };
    let policy = Policy { memory_limit_bytes: NORMAL, heavy_limit_bytes: HEAVY, heavy_min_free_percent: 0, page_deadline: Duration::from_secs(10), ..Policy::default() };
    DocRenderer::new(&file.0, None, config, policy).unwrap()
}

fn request() -> ContentsRequest {
    ContentsRequest { page: 0, generation: 0, scale: 2.0, rect: None, text: TextMode::Both, hints: 0, max_time: Duration::from_secs(20) }
}

fn file() -> TempPdf {
    TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]))
}

/// What came through, as tags.
fn run(mode: &str) -> (Result<u32, RenderFailure>, Vec<&'static str>) {
    let file = file();
    let mut doc = renderer(mode, &file);
    let mut seen = Vec::new();
    let result = doc.read_contents(&request(), |event| {
        seen.push(match event {
            ContentsEvent::Restart => "restart",
            ContentsEvent::Record(Record::Page { .. }) => "page",
            ContentsEvent::Record(Record::Path { .. }) => "path",
            ContentsEvent::Record(_) => "other",
        });
        Ok(())
    });
    (result.map(|done| done.restarts), seen)
}

/// Failure: a stream that's fine comes through wrong, or a stand-in proves nothing because even an honest one fails.
#[test]
fn an_honest_stream_comes_through_in_order() {
    let (result, seen) = run("honest");
    assert_eq!(result, Ok(0));
    assert_eq!(seen, ["page", "path"]);
}

/// Failure: a page that needs the larger limit comes out with what was read before the worker was stopped still in it, so the file holds half a page and then all of it. The caller is told to forget it, and the page then comes out whole.
#[test]
fn a_page_that_outgrows_its_worker_starts_again_in_a_larger_one() {
    let (result, seen) = run("grows_midway");
    assert_eq!(result, Ok(1), "{seen:?}");
    assert_eq!(seen, ["page", "restart", "page", "path"]);
}

/// Failure: a page past every limit comes out partly written, or at a lower resolution. Owner, 2026-11-07: an export is sharp or isn't written. It must fail, plainly, whatever the caller was told so far.
#[test]
fn a_page_past_every_limit_is_refused() {
    let (result, _) = run("always_grows");
    assert_eq!(result, Err(RenderFailure::ExportRefused { page: 0, why: ExportRefusal::TooHeavy }));
}

/// Failure: a compromised worker's records reach the writer, among them an image whose rows would fill the host's memory before any cap on the file sees a byte, or one whose size overflows the sum that checks it. Each of these breaks a rule, kills the worker, and ends in a failure, so nothing is published.
#[test]
fn a_stream_that_breaks_the_rules_is_stopped() {
    for mode in ["jpeg_thats_not_an_image", "jpeg_with_no_mask", "mask_of_colour", "mask_with_no_jpeg", "not_really_a_jpeg", "row_too_long", "size_overflows", "page_twice", "clip_too_deep", "image_short", "image_left_open", "group_left_open", "garbage", "not_finite", "wrong_seq"] {
        let (result, _) = run(mode);
        assert!(matches!(result, Err(RenderFailure::PageFailed { reason: KillReason::Rejected(_), .. })), "{mode}: {result:?}");
    }
}

/// Failure: a worker that keeps saying it's lost its place has the host asking again for ever. It gets a couple of tries, as a worker that's been replaced might need, and then it's stopped.
#[test]
fn a_worker_that_keeps_losing_its_place_is_stopped() {
    let (result, seen) = run("forgets_place");
    assert!(matches!(result, Err(RenderFailure::PageFailed { reason: KillReason::Rejected(_), .. })), "{result:?}: {seen:?}");
    assert!(seen.iter().filter(|tag| **tag == "restart").count() <= 2, "{seen:?}");
}

/// Failure: a worker that always says there's more holds a job for ever, each frame well inside its own deadline. The whole page has a time limit of its own.
#[test]
fn a_worker_that_never_finishes_runs_out_of_time() {
    let file = file();
    let mut doc = renderer("endless", &file);
    let started = Instant::now();
    let request = ContentsRequest { max_time: Duration::from_millis(300), ..request() };
    let result = doc.read_contents(&request, |_| Ok(()));
    assert_eq!(result, Err(RenderFailure::TookTooLong));
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// Failure: a cancelled export carries on writing, or a stale job leaves a worker running. A cancel ends it, between frames, with nothing published.
#[test]
fn a_cancel_ends_the_stream() {
    let file = file();
    let mut doc = renderer("endless", &file);
    let cancel = doc.cancel_handle();
    let mut frames = 0;
    let result = doc.read_contents(&request(), |_| {
        frames += 1;
        if frames == 1 {
            cancel.cancel_before(1);
        }
        Ok(())
    });
    assert_eq!(result, Err(RenderFailure::Cancelled));
}

/// Failure: an error from what's being written, such as a full disk, is swallowed, or the stream carries on after it. It ends there, and the caller gets it back.
#[test]
fn a_failure_from_the_sink_ends_the_stream() {
    let file = file();
    let mut doc = renderer("honest", &file);
    let result = doc.read_contents(&request(), |_| Err(RenderFailure::Unavailable));
    assert_eq!(result, Err(RenderFailure::Unavailable));
}

/// Failure: the header's fixed part is read with the fields the wrong way round, so a job looks answered that wasn't.
#[test]
fn a_reply_header_says_what_it_says() {
    let mut out = Vec::new();
    begin_contents(&mut out);
    finish_contents(&mut out, 5, 6, true).unwrap();
    let (header, records) = recto_proto::contents::decode_contents(&out[proto::HEADER_LEN..]).unwrap();
    assert_eq!(header, ContentsHeader { job_id: 5, seq: 6, done: true });
    assert!(records.is_empty());
}
