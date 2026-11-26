//! **The exports really ask again.** `ask_again.rs` holds the helper's own rules; here each route that asks the render service goes through it with a worker the Mac gave no processor at all (`kill -STOP`), as the person saw it on a busy Mac. Only ONE job is short of time (`under_job_deadline_for_test`), so the ask again, on a fresh worker, has the ordinary deadline and no machine speed decides the outcome.

use super::image::{self, Want};
use super::svg::{SvgJob, write_svg};
use super::{MemoryOut, Output};
use recto_proto::contents::TextMode;
use recto_render_service::{DocRenderer, OutputCaps, Policy, ServiceConfig};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages_with, text_lines};
use std::cell::RefCell;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// A document with its worker started and the file open, and the file.
fn started(name: &str, bytes: &[u8]) -> (TempPdf, DocRenderer) {
    let file = TempPdf::new(name, bytes);
    let mut doc = DocRenderer::new(&file.0, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap();
    doc.page_size(0).unwrap();
    (file, doc)
}

/// Sends `name` to a worker's process.
fn signal(pid: u32, name: &str) -> bool {
    std::process::Command::new("/bin/kill")
        .arg(name)
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

/// Runs `ask` with the document's worker stopped as the `nth` job starts (from 0) and only that job short of time, then wakes the pid again (a worker killed at its deadline is gone, and the signal says so).
fn stopped_at_job<R>(doc: &mut DocRenderer, nth: u32, ask: impl FnOnce(&mut DocRenderer) -> R) -> R {
    let pid = doc.worker_pid().expect("a worker with the file open");
    let done = recto_render_service::under_job_deadline_for_test(
        nth,
        Duration::from_millis(250),
        move || {
            assert!(signal(pid, "-STOP"));
        },
        || ask(doc),
    );
    signal(pid, "-CONT");
    done
}

/// With the first job stopped.
fn with_the_worker_stopped<R>(doc: &mut DocRenderer, ask: impl FnOnce(&mut DocRenderer) -> R) -> R {
    stopped_at_job(doc, 0, ask)
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

/// Failure (the person saw "too busy to draw page 1. It will be tried again." and nothing tried again): `export svg` of a page whose worker the Mac was too busy for ended as `page_mac_busy`. Guards `write_svg`'s PAGE-SIZE ask, the first job of an export (a stopped worker is met there first): it asks again, counts the Mac busy once, and the file holds the second ask's SVG alone. Not the contents ask: see `a_busy_frame_after_records_were_written_starts_the_svg_over`.
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

/// Failure: `images` (the list of a page's pictures) and the image copy, which ask the service once as `export svg` did, gave "too busy" on a busy Mac where a second ask lists the picture and copies it. Guards `image::list`'s contents ask (the first job of its read) and `copy_into`'s FIRST frame, with the output empty when it is busy; a busy LATER frame, after bytes were written, is `a_busy_frame_after_png_bytes_were_written_starts_the_image_over`.
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

/// A page with one raw picture of about 1.5 MB, which the worker sends in more than one frame (a frame ends at about a megabyte), so a busy answer can come after records were written.
fn big_picture_page() -> Vec<u8> {
    let pixels: Vec<u8> = (0..700 * 700 * 3).map(|at: usize| (at % 251) as u8).collect();
    let resources = format!("/XObject << /Im {} 0 R >>", first_extra_object(1));
    let dict = "/Type /XObject /Subtype /Image /Width 700 /Height 700 /ColorSpace /DeviceRGB /BitsPerComponent 8".to_string();
    pages_with(
        &[Page::new("0 0 1 rg 10 10 50 50 re f\nq 400 0 0 400 50 300 cm /Im Do Q\n")],
        &resources,
        vec![Object::Stream { dict, data: pixels }],
    )
}

/// Bytes held, counting the times the export started them over.
struct CountsStartsOver {
    bytes: Vec<u8>,
    overs: u32,
    /// Whether starting over fails, as a file that can't be truncated.
    fails_to_start_over: bool,
}

impl Write for CountsStartsOver {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Output for CountsStartsOver {
    fn start_over(&mut self) -> io::Result<()> {
        self.bytes.clear();
        self.overs += 1;
        if self.fails_to_start_over {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}

/// Failure: the Mac was too busy for a LATER frame of a page, after the first frame's records were written, and the export asked again without starting its output over, so the file holds the page twice (or the first part of it twice); or without starting its writer over, so the second try is written onto a half-made SVG. Guards `write_svg`'s CONTENTS ask and its `start_over`: job 2 of the export (its size, then two frames) is the stopped one. The file is one whole SVG, with the picture once, and was started over once.
#[test]
fn a_busy_frame_after_records_were_written_starts_the_svg_over() {
    let (file, mut doc) = started("busy-frame-svg.pdf", &big_picture_page());
    let out = RefCell::new(CountsStartsOver {
        bytes: Vec::new(),
        overs: 0,
        fails_to_start_over: false,
    });
    let job = SvgJob {
        path: &file.0,
        page: 0,
        rect: None,
        text: TextMode::Both,
        scale: 1.0,
        strip_metadata: false,
        quarter_turns: 0,
    };
    let written = stopped_at_job(&mut doc, 2, |doc| write_svg(doc, &job, &OutputCaps::default(), &out, Instant::now()));
    written.unwrap_or_else(|error| panic!("{}, {:?}", error.message, doc.stats()));
    let out = out.into_inner();
    assert_eq!((out.overs, doc.stats().too_busy), (1, 1), "{:?}", doc.stats());
    let svg = String::from_utf8(out.bytes).unwrap();
    assert_eq!(svg.matches("<svg").count(), 1, "one SVG");
    assert_eq!(recto_svg::verify::embedded_images(svg.as_bytes()).len(), 1, "the picture once");
}

/// Failure: the same for the image copy: the Mac was too busy for the picture's second frame after the first frame's bytes were already in the PNG, and the ask again wrote a second PNG after the half of the first. Guards `copy_into`'s `start_over` (the writer and the output): one PNG, started over once.
#[test]
fn a_busy_frame_after_png_bytes_were_written_starts_the_image_over() {
    let (file, mut doc) = started("busy-frame-image.pdf", &big_picture_page());
    let out = RefCell::new(CountsStartsOver {
        bytes: Vec::new(),
        overs: 0,
        fails_to_start_over: false,
    });
    let copied = stopped_at_job(&mut doc, 1, |doc| {
        image::copy_into(doc, &file.0, 0, 1, Want::Png, 1.0, &OutputCaps::default(), &out)
    });
    copied.unwrap_or_else(|error| panic!("{}, {:?}", error.message, doc.stats()));
    let out = out.into_inner();
    assert_eq!((out.overs, doc.stats().too_busy), (1, 1), "{:?}", doc.stats());
    assert_eq!(
        out.bytes.windows(4).filter(|window| *window == b"IHDR").count(),
        1,
        "one PNG: {} bytes",
        out.bytes.len()
    );
}

/// Failure: an output that can't be started over (a file that can't be truncated) after the Mac was too busy ended the image copy as the Mac being unavailable, which hides that a file could not be written. It is a write failure, as `export svg` says it.
#[test]
fn an_output_that_cant_start_over_is_a_write_failure_for_the_image_too() {
    let (file, mut doc) = started("busy-start-over.pdf", &picture_page());
    let out = RefCell::new(CountsStartsOver {
        bytes: Vec::new(),
        overs: 0,
        fails_to_start_over: true,
    });
    let copied = with_the_worker_stopped(&mut doc, |doc| {
        image::copy_into(doc, &file.0, 0, 1, Want::Png, 1.0, &OutputCaps::default(), &out)
    });
    assert_eq!(copied.unwrap_err().code, "write_failed");
}
