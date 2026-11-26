//! **The exports really ask again.** `ask_again.rs` holds the helper's own rules; here each route that asks the render service goes through it with a worker the Mac gave no processor at all (`kill -STOP`), as the person saw it on a busy Mac. Only the FIRST job is short of time (`under_first_job_deadline_for_test`), so the ask again, on a fresh worker, has the ordinary deadline and no machine speed decides the outcome.

use super::MemoryOut;
use super::image::{self, Want};
use super::svg::{SvgJob, write_svg};
use recto_proto::contents::TextMode;
use recto_render_service::{DocRenderer, OutputCaps, Policy, ServiceConfig};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages_with, text_lines};
use std::cell::RefCell;
use std::time::{Duration, Instant};

/// A document with its worker started and the file open, and the file.
fn started(name: &str, bytes: &[u8]) -> (TempPdf, DocRenderer) {
    let file = TempPdf::new(name, bytes);
    let mut doc = DocRenderer::new(&file.0, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap();
    doc.page_size(0).unwrap();
    (file, doc)
}

/// Stops the document's worker, runs `ask` with only its first job short of time, and wakes the pid again (a worker killed at its deadline is gone, and the signal says so).
fn with_the_worker_stopped<R>(doc: &mut DocRenderer, ask: impl FnOnce(&mut DocRenderer) -> R) -> R {
    let pid = doc.worker_pid().expect("a worker with the file open").to_string();
    let signal = |name: &str| {
        std::process::Command::new("/bin/kill")
            .arg(name)
            .arg(&pid)
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(signal("-STOP"));
    let done = recto_render_service::under_first_job_deadline_for_test(Duration::from_millis(250), || ask(doc));
    signal("-CONT");
    done
}

fn picture_page() -> Vec<u8> {
    let pixels: Vec<u8> = (0..16 * 16 * 3).map(|at| (at % 251) as u8).collect();
    let resources = format!("/XObject << /Im {} 0 R >>", first_extra_object(1));
    let dict = "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8".to_string();
    pages_with(
        &[Page::new("q 100 0 0 100 50 600 cm /Im Do Q\n")],
        &resources,
        vec![Object::Stream { dict, data: pixels }],
    )
}

/// Failure (the person saw "too busy to draw page 1. It will be tried again." and nothing tried again): `export svg` of a page whose worker the Mac was too busy for ended as `page_mac_busy`. It asks again, counts the Mac busy once, and the file holds the second ask's SVG alone. Removing `asked_again` from `write_svg` fails this.
#[test]
fn export_svg_asks_again_when_the_mac_was_too_busy() {
    let (file, mut doc) = started("busy-svg.pdf", &recto_testpdf::pages(&[Page::new(text_lines(&["Words on a page"]))]));
    let out = RefCell::new(MemoryOut::new(1 << 24));
    let job = SvgJob {
        path: &file.0,
        page: 0,
        rect: None,
        text: TextMode::Both,
        scale: 1.0,
        strip_metadata: false,
        quarter_turns: 0,
    };
    let written = with_the_worker_stopped(&mut doc, |doc| write_svg(doc, &job, &OutputCaps::default(), &out, Instant::now()));
    let written = written.unwrap_or_else(|error| panic!("{}, {:?}", error.message, doc.stats()));
    assert_eq!(doc.stats().too_busy, 1, "{:?}", doc.stats());
    let svg = String::from_utf8(out.into_inner().bytes).unwrap();
    assert_eq!(svg.matches("<svg").count(), 1, "the first ask's output was started over");
    assert_eq!(u64::try_from(svg.len()).unwrap(), written.bytes);
}

/// Failure: `images` (the list of a page's pictures) and the image copy, which ask the service once as `export svg` did, gave "too busy" on a busy Mac where a second ask lists the picture and copies it. Removing `asked_again` from `image::list` or `copy_into` fails this.
#[test]
fn the_image_list_and_the_image_copy_ask_again_when_the_mac_was_too_busy() {
    let (file, mut doc) = started("busy-image.pdf", &picture_page());
    let caps = OutputCaps::default();
    let listed = with_the_worker_stopped(&mut doc, |doc| image::list(doc, &file.0, 0, &caps));
    let listed = listed.unwrap_or_else(|error| panic!("list: {}, {:?}", error.message, doc.stats()));
    assert_eq!((listed.images.len(), doc.stats().too_busy), (1, 1), "{:?}", doc.stats());
    let out = RefCell::new(MemoryOut::new(1 << 24));
    let copied = with_the_worker_stopped(&mut doc, |doc| image::copy_into(doc, &file.0, 0, 1, Want::Png, 1.0, &caps, &out));
    copied.unwrap_or_else(|error| panic!("copy: {}, {:?}", error.message, doc.stats()));
    assert_eq!(doc.stats().too_busy, 2, "{:?}", doc.stats());
    let png = out.into_inner().bytes;
    assert!(png.starts_with(b"\x89PNG"), "one PNG, the first ask's bytes started over: {} bytes", png.len());
}
