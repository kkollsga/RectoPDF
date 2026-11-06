//! Its own binary, because it counts this process's open files and other tests running beside it would disturb that. Stands in as its own
//! worker, like `hostile.rs`.

use recto_platform::isolate::{self, Channel, SharedMemory};
use recto_proto::{self as proto, Kind, RenderJob, TileKind, TileRect, TileReply, TileStatus};
use recto_render_service::transport::{FrameReader, send_all};
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileRequest, TileResult};
use recto_testpdf::TempPdf;
use std::ffi::OsString;

/// Not a test: a worker that attaches a handle to every tile it sends. Does nothing unless started as one.
#[test]
fn fake_worker() {
    if std::env::var("RECTO_FAKE").is_err() {
        return;
    }
    let channel = Channel::inherited().unwrap();
    send_all(&channel, &proto::encode_hello(std::process::id()), &[]).unwrap();
    let mut reader = FrameReader::default();
    loop {
        let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
        match frame.kind {
            Kind::Open => send_all(&channel, &proto::encode_opened(1), &[]).unwrap(),
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
                let extra = SharedMemory::create(4096).unwrap();
                send_all(&channel, &reply.encode(), &[extra.handle()]).unwrap();
            }
            _ => isolate::exit_now(1),
        }
    }
}

/// Failure: a worker sends handles to the host and the host keeps them, until it runs out of descriptors.
#[test]
fn handles_from_a_worker_are_not_kept() {
    let file = TempPdf::new("stand-in.pdf", b"%PDF-1.7\n");
    let config = ServiceConfig {
        worker_exe: std::env::current_exe().unwrap(),
        pdfium_lib: "/nonexistent".into(),
        extra_env: vec![("RECTO_FAKE".into(), "1".into())],
        replace_args: Some(["--exact", "fake_worker", "--nocapture", "--test-threads=1"].into_iter().map(OsString::from).collect()),
    };
    let mut doc = DocRenderer::new(&file.0, None, config, Policy::default()).unwrap();
    let surface = SharedMemory::create(512 * 512 * 4).unwrap();
    let request = TileRequest { page: 0, rect: TileRect { x: 0, y: 0, width: 512, height: 512 }, scale: 2.0, stride: 2048, generation: 1 };
    doc.page_count().unwrap();
    assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    let before = recto_platform::open_handle_count();
    for _ in 0..20 {
        assert!(matches!(doc.render_tile(&request, &surface).unwrap(), TileResult::Done(_)));
    }
    assert_eq!(recto_platform::open_handle_count(), before);
}
