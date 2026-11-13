//! Its own binary, because it counts this process's open files and other tests running beside it would disturb that, so tests here take turns.
//! Stands in as its own worker, like `hostile.rs`.

#![allow(unsafe_code)]

use recto_platform::isolate::{self, Channel, SharedMemory};
use recto_proto::{self as proto, Kind, RenderJob, TileKind, TileRect, TileReply, TileStatus};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{DocRenderer, KillReason, Policy, RenderFailure, ServiceConfig, TileRequest};
use recto_testpdf::TempPdf;
use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn turn() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How many handles there are once they've stopped changing. The test before this one's worker is reaped on a thread of its own, and its descriptors can still be closing when the next test starts. Counted then, the count goes below `before` and never comes back to it, which is what made these tests fail now and then.
fn steady_handle_count() -> Option<usize> {
    let mut last = recto_platform::open_handle_count();
    let mut same = 0;
    let started = Instant::now();
    while same < 20 && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
        let now = recto_platform::open_handle_count();
        same = if now == last { same + 1 } else { 0 };
        last = now;
    }
    last
}

/// Sends `count` descriptors in one message, past anything `Channel::send` would allow, as a compromised worker could.
fn flood(fd: i32, count: usize) {
    let file = std::fs::File::open("/dev/null").unwrap();
    let mut control = vec![0u64; (count * 4 + 64).div_ceil(8)];
    let byte = b'R';
    let mut iov = libc::iovec {
        iov_base: (&raw const byte).cast_mut().cast(),
        iov_len: 1,
    };
    // SAFETY: a zeroed `msghdr` is valid.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    // SAFETY: `control` is aligned and sized above for this header and these descriptors, and `msg` and all it points at outlive the calls.
    unsafe {
        msg.msg_controllen = libc::CMSG_SPACE((count * 4) as u32);
        let header = libc::CMSG_FIRSTHDR(&raw const msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN((count * 4) as u32);
        let data = libc::CMSG_DATA(header).cast::<i32>();
        for i in 0..count {
            data.add(i).write_unaligned(file.as_raw_fd());
        }
        assert!(libc::sendmsg(fd, &raw const msg, 0) == 1);
    }
}

/// Not a test: a worker that attaches a handle to every tile it sends. Does nothing unless started as one.
#[test]
fn fake_worker() {
    if std::env::var("RECTO_FAKE").is_err() {
        return;
    }
    let mode = std::env::var("RECTO_FAKE").unwrap();
    let channel = Channel::inherited().unwrap();
    if mode == "handle_on_hello" {
        let extra = SharedMemory::create(4096).unwrap();
        send_all(
            &channel,
            &proto::encode_hello(proto::Hello {
                pid: std::process::id(),
                walled: false,
            }),
            &[extra.handle()],
        )
        .unwrap();
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
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        match frame.kind {
            Kind::Open => send_all(
                &channel,
                &proto::encode_opened(proto::Opened {
                    pages: 1,
                    version: 1,
                    repaired: false,
                }),
                &[],
            )
            .unwrap(),
            Kind::Render => {
                let job = RenderJob::decode(&frame.payload).unwrap();
                let reply = TileReply {
                    generation: job.generation,
                    job_id: job.job_id,
                    width: job.rect.width,
                    height: job.rect.height,
                    stride: job.stride,
                    status: TileStatus::Done(TileKind::Colour),
                    render_us: 1,
                };
                match mode.as_str() {
                    "one_handle" => {
                        let extra = SharedMemory::create(4096).unwrap();
                        send_all(&channel, &reply.encode(), &[extra.handle()]).unwrap();
                    }
                    // Fd 3 is the channel, from `Channel::inherited`.
                    "flood" => flood(3, 200),
                    _ => send_all(&channel, &reply.encode(), &[]).unwrap(),
                }
            }
            // Refused once, as a page with more images than an answer holds is, and answered after that, to see which of them holds.
            Kind::ImagesRequest => {
                static ASKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let job_id = u32::from_le_bytes(frame.payload[..4].try_into().unwrap());
                if ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    send_all(&channel, &proto::encode_failed(job_id, proto::FailCode::TooLarge), &[]).unwrap();
                } else {
                    send_all(
                        &channel,
                        &proto::PageImages {
                            job_id,
                            boxes: vec![[0, 0, 100, 100]],
                        }
                        .encode(),
                        &[],
                    )
                    .unwrap();
                }
            }
            _ => isolate::exit_now(1),
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

/// Waits for a state, not a time. The bound only turns a hang into a failure.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(30), "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Failure: a page's images are asked about once for each tile while they're not known, and an answer that failed then one that didn't leave some of its tiles dimmed and some turned over. A page gets one decision, a failed answer included: a refusal for too many images, as here, is remembered as not known, so the page is judged whole and its walk isn't run again.
#[test]
fn a_page_gets_one_decision_about_its_images() {
    let _turn = turn();
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let mut doc = DocRenderer::new(&file.0, None, config("images"), Policy::default()).unwrap();
    assert_eq!(doc.image_boxes(0, 1), Ok(None), "it couldn't say");
    assert_eq!(doc.image_boxes(0, 2), Ok(None), "and it's not asked again, though now it could");
    assert_eq!(doc.image_boxes(1, 3), Ok(Some(vec![[0, 0, 100, 100]])), "another page is another question");
}

/// Failure: a worker sends handles to the host and the host keeps them, until it runs out of descriptors. The host never expects one, so it's a violation.
#[test]
fn a_handle_from_a_worker_is_a_violation_and_isnt_kept() {
    let _turn = turn();
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let before = steady_handle_count();
    let mut doc = DocRenderer::new(&file.0, None, config("one_handle"), Policy::default()).unwrap();
    let surface = SharedMemory::create(512 * 512 * 4).unwrap();
    let failure = doc.render_tile(&request(), &surface).unwrap_err();
    assert!(
        matches!(
            failure,
            RenderFailure::PageFailed {
                reason: KillReason::Rejected(_),
                ..
            }
        ),
        "{failure:?}"
    );
    drop((doc, surface));
    wait_until("descriptors to go back to where they were", || recto_platform::open_handle_count() == before);
}

/// Failure: a worker sends ~200 descriptors in one message. The old code read past its control buffer, wrapped what it found as descriptors, and could close
/// files it didn't own. All of them must be closed, the worker killed, and the host's own files left alone.
#[test]
fn a_flood_of_handles_kills_the_worker_and_closes_every_one() {
    let _turn = turn();
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let held = std::fs::File::open("/etc/hosts").unwrap();
    let before = steady_handle_count();
    let mut doc = DocRenderer::new(&file.0, None, config("flood"), Policy::default()).unwrap();
    let surface = SharedMemory::create(512 * 512 * 4).unwrap();
    let failure = doc.render_tile(&request(), &surface).unwrap_err();
    assert!(
        matches!(
            failure,
            RenderFailure::PageFailed {
                reason: KillReason::Rejected(_),
                ..
            }
        ),
        "{failure:?}"
    );
    assert!(!doc.worker_pid().is_some());
    drop((doc, surface));
    wait_until("descriptors to go back to where they were", || recto_platform::open_handle_count() == before);
    assert!(std::fs::read_to_string("/etc/hosts").is_ok());
    assert!(held.metadata().is_ok(), "a file the host held open was closed under it");
}

/// Failure: a handle on a worker's very first message is taken as a good hello, when a worker that's misbehaving already should go no further.
#[test]
fn a_handle_on_the_hello_is_refused() {
    let _turn = turn();
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let mut doc = DocRenderer::new(&file.0, None, config("handle_on_hello"), Policy::default()).unwrap();
    assert_eq!(doc.page_count().unwrap_err(), RenderFailure::Unavailable);
    assert_eq!(doc.stats().spawns, 0);
}

/// Failure: the reaper's thread and queue stay after the last worker, so a session that has closed every document still holds a handle and a thread.
#[test]
fn no_workers_means_no_handle_left_behind() {
    let _turn = turn();
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let before = steady_handle_count();
    let mut doc = DocRenderer::new(&file.0, None, config("good"), Policy::default()).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
    assert!(recto_platform::open_handle_count() > before);
    drop(doc);
    wait_until("the last handle to close", || recto_platform::open_handle_count() == before);
}
