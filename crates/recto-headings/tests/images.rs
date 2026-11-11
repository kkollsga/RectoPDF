//! **No image decoder runs in the headings helper** (owner, 2026-11-08, asked what 0.1.0 should do about distillPDF's decoders reading untrusted PDFs outside any sandbox: "Turn its image decoding off"). Headings come from text.
//!
//! distillPDF has no switch for this, and needs none here: its page-at-a-time path, the only one `recto-headings` calls (`scan_document`, `scan_typography`, `page_stream`), places images without decoding them. It reads an image's dictionary to know where it is and never its samples. This holds that, so a distillPDF upgrade that starts decoding on that path fails here rather than quietly putting its decoders back in front of hostile files.
//!
//! One test in this binary on purpose: it reads this process's own peak footprint, which another test running beside it would move.

use flate2::Compression;
use flate2::write::ZlibEncoder;
use recto_index::corpus::CorpusKey;
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages_with};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// The bomb's side in pixels: 6000 x 6000 RGB is 108 MB of samples, inside distillPDF's 64 Mpx cap, so its decoder would take every byte.
const SIDE: usize = 6000;
const SAMPLE_BYTES: u64 = (SIDE * SIDE * 3) as u64;

/// A decompression bomb as an image: 108 MB of samples in about 100 KB of Flate. Made a megabyte at a time, so making it doesn't move this process's peak.
fn bomb_samples() -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    let chunk = vec![0u8; 1 << 20];
    let mut left = SAMPLE_BYTES as usize;
    while left > 0 {
        let take = left.min(chunk.len());
        encoder.write_all(&chunk[..take]).unwrap();
        left -= take;
    }
    encoder.finish().unwrap()
}

/// Six pages with a heading each and body text, as the other helper tests have, and a seventh of body text with the bomb drawn over it. The bomb's page has no heading, so the render worker, which checks headings for hidden text in its own sandbox, never draws it: only distillPDF in the helper sees it.
fn document_with_a_bomb() -> Vec<u8> {
    let body = "This is ordinary body text on the page, written in a smaller size than any heading so that it reads as a paragraph.";
    let titles = ["Introduction", "Background", "Method", "Results", "Discussion", "Conclusion"];
    let mut pages: Vec<Page> = titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let mut content = format!("BT /F1 20 Tf 72 720 Td ({} {title}) Tj ET\nBT /F1 10 Tf 12 TL 72 690 Td\n", index + 1);
            for _ in 0..30 {
                content += &format!("({body}) Tj T*\n");
            }
            content += "ET\n";
            Page::new(content)
        })
        .collect();
    pages.push(Page::new(format!(
        "BT /F1 10 Tf 72 740 Td ({body}) Tj ET\nq 468 0 0 600 72 100 cm /Bomb Do Q\n"
    )));
    let bomb = first_extra_object(pages.len());
    pages_with(
        &pages,
        &format!("/XObject << /Bomb {bomb} 0 R >>"),
        vec![Object::Stream {
            dict: format!("/Type /XObject /Subtype /Image /Width {SIDE} /Height {SIDE} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode"),
            data: bomb_samples(),
        }],
    )
}

fn peak_here() -> u64 {
    recto_platform::isolate::process_memory(std::process::id())
        .expect("this process's memory")
        .peak_footprint_bytes
}

/// The helper as the session runs it, with the session's limit: its report line.
fn helper_report(path: &std::path::Path) -> Value {
    let key = CorpusKey::of_file(path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_recto-headings"))
        .env_clear()
        .env("MallocLargeCache", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "file": path, "key": key.hex(), "memory_limit": 512u64 << 20 })).unwrap();
    let mut said = String::new();
    child.stdout.take().unwrap().read_to_string(&mut said).unwrap();
    drop(stdin);
    let _ = child.wait();
    said.lines().next().and_then(|line| serde_json::from_str(line).ok()).unwrap_or(Value::Null)
}

/// Failure: the headings helper decodes an image in a hostile PDF, so distillPDF's image decoders (`image`, `jpeg-decoder`, `fax`) run on untrusted bytes outside any sandbox, against the owner's decision of 2026-11-08. Here a 100 KB image that inflates to 108 MB of samples: decoded, the helper's peak footprint would pass the samples' size. And the contents must still come, from the text.
///
/// The second half shows the test can fail: the same file through distillPDF's own image extraction, which does decode, takes this process past the samples' size.
#[test]
fn the_headings_helper_never_decodes_an_image() {
    let pdf = TempPdf::new("bomb.pdf", &document_with_a_bomb());
    let report = helper_report(&pdf.0);
    assert_eq!(report["state"], "ready", "{report}");
    assert_eq!(report["kept"].as_u64(), Some(6), "every heading, from the text: {report}");
    let helper_peak = report["helper_peak_bytes"].as_u64().expect("the helper's peak");
    assert!(
        helper_peak < SAMPLE_BYTES,
        "the helper's peak, {} MiB, is past the image's samples, {} MiB: it decoded the image",
        helper_peak >> 20,
        SAMPLE_BYTES >> 20
    );

    let before = peak_here();
    let document = distillpdf::PdfDocument::open_with_engine(pdf.0.to_str().unwrap(), distillpdf::Engine::LazyStrict).unwrap();
    let images = document.extract_images();
    drop(document);
    let rise = peak_here().saturating_sub(before);
    eprintln!(
        "helper peak {} MiB; distillPDF's decoding took this process up {} MiB",
        helper_peak >> 20,
        rise >> 20
    );
    assert_eq!(images.len(), 1, "distillPDF's extraction found the image");
    assert!(
        rise >= SAMPLE_BYTES,
        "decoding the image took this process up only {} MiB, so the helper's peak above proves nothing",
        rise >> 20
    );
}
