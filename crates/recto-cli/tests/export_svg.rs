//! `export svg` end to end, through the real `recto` and a real worker: that the command line reaches the export, its flags and refusals, and an
//! edited tab's pages through the session. What's in the file and what the answer says are `recto-agent`'s `render::svg::tests`, in process on the
//! same worker. The last five tests here hold rules no seeded bug was yet seen to break in process (R4.1, 2026-11-13: text clipped away, absurd
//! numbers, spaces that draw nothing, a link, a profile that isn't sRGB), so they stay on this route until one is.

mod common;

use common::State;
use recto_agent::Level;
use recto_svg::verify::{Embedded, check_document, read_png};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages, pages_with, text_lines};
use serde_json::Value;
use std::path::PathBuf;

fn export(file: &TempPdf, extra: &[&str]) -> (Value, i32, PathBuf) {
    let state = State::at(Level::Full);
    let out = file.0.with_file_name(format!("out-{}.svg", std::process::id()));
    let mut args = vec![
        "local",
        file.0.to_str().unwrap(),
        "export",
        "svg",
        "--page",
        "1",
        "--out",
        out.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let (reply, code) = state.json(&args);
    (reply, code, out)
}

/// Exports and hands back the file's text and the answer, with the file gone, having checked the file is what it should be: well formed, and with nothing in it that can run or lead out.
fn svg_of(file: &TempPdf, extra: &[&str]) -> (String, Value) {
    let (reply, code, out) = export(file, extra);
    assert_eq!(code, 0, "{reply}");
    let bytes = std::fs::read(&out).unwrap();
    check_document(&bytes);
    assert_eq!(reply["data"]["bytes"].as_u64(), Some(bytes.len() as u64), "the answer's size is the file's");
    std::fs::remove_file(&out).unwrap();
    (String::from_utf8(bytes).unwrap(), reply["data"].clone())
}

fn folder_entries(file: &TempPdf) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(file.0.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Failure: an ordinary page's text and shapes don't come through as what they are, or it's quietly a picture. It must have paths, outlines and real text, say nothing was rasterised, and leave nothing behind but the file.
#[test]
fn an_ordinary_page_is_all_vector() {
    let content = [
        b"0 0 1 rg 72 600 200 100 re f 1 0 0 RG 3 w 72 500 m 300 550 l S\n".as_slice(),
        &text_lines(&["Hello vector world"]),
    ]
    .concat();
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(content)]));
    let before = folder_entries(&file);
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
    assert_eq!(data["width_pt"], 612.0);
    assert!(
        svg.contains("<path") && svg.contains("<use") && svg.contains(">Hello vector world</text>"),
        "{svg}"
    );
    assert!(svg.contains("fill=\"#00f\""), "the blue fill: {svg}");
    assert!(!svg.contains("<image"), "{svg}");
    assert_eq!(folder_entries(&file), before, "something was left behind");
}

/// Failure: a box that isn't one is taken for something else, or a page that isn't there writes a file. Refused plainly, with nothing written.
#[test]
fn bad_boxes_and_pages_are_refused_and_write_nothing() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let before = folder_entries(&file);
    for rect in ["1,2,3", "1,2,3,0", "-1,0,5,5", "a,b,c,d", "1,2,3,nan", "1,2,3,4,5"] {
        let (reply, code, _) = export(&file, &["--rect", rect]);
        assert_ne!(code, 0, "{rect}: {reply}");
    }
    let (reply, code, _) = export(&file, &["--rect", "9000,9000,5,5"]);
    assert_ne!(code, 0, "{reply}");
    assert_eq!(reply["error"]["code"], "bad_rect", "{reply}");
    let state = State::at(Level::Full);
    let out = file.0.with_file_name("no.svg");
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "export", "svg", "99", "--out", out.to_str().unwrap()]);
    assert_ne!(code, 0, "{reply}");
    assert_eq!(folder_entries(&file), before);
}

/// Failure: an export after a page is moved or turned shows the wrong page, or a turn it doesn't show goes unsaid, as `render` has to say. Page numbers mean the document as edited, and what isn't in the file is said plainly.
#[test]
fn an_edited_tab_is_exported_honestly() {
    let file = TempPdf::new(
        "doc.pdf",
        &pages(&[Page::new(text_lines(&["First page"])), Page::new(text_lines(&["Second page"]))]),
    );
    let out = file.0.with_file_name("edited.svg");
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let (opened, _) = state.json(&["open", file.0.to_str().unwrap()]);
    assert_eq!(opened["ok"], true, "{opened}");
    state.json(&["page", "move", "2", "--to", "1"]);
    state.json(&["page", "rotate", "1", "--by", "90"]);
    let (reply, code) = state.json(&["export", "svg", "1", "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["data"]["source_page"], 2, "{reply}");
    assert_eq!(reply["data"]["unsaved_rotation_degrees"], 90, "{reply}");
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.contains(">Second page</text>"), "{svg}");
    std::fs::remove_file(&out).unwrap();
    let (said, _, _) = state.text(&["export", "svg", "1", "--out", out.to_str().unwrap()]);
    assert!(said.contains("unsaved turn of 90 degrees"), "{said}");
    std::fs::remove_file(&out).unwrap();
}

/// A 4 x 4 baseline JPEG, red.
const JPEG_HEX: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffdb00430103040405040509050509140d0b0d1414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414ffc00011080004000403012200021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00f0aa28a2be68fedd3fffd9";

fn jpeg_bytes() -> Vec<u8> {
    (0..JPEG_HEX.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&JPEG_HEX[at..at + 2], 16).unwrap())
        .collect()
}

/// A pixel that's the red the hidden things in these tests are.
fn red_pixels(image: &Embedded) -> usize {
    let Embedded::Png { channels, pixels, .. } = image else { return 0 };
    pixels
        .chunks(*channels)
        .filter(|pixel| pixel.len() >= 3 && pixel[0] > 180 && pixel[1] < 80 && pixel[2] < 80 && (pixel.len() < 4 || pixel[3] > 200))
        .count()
}

/// What `recto render` draws, which is PDFium's real rule, as its count of red pixels.
fn red_in_render(file: &TempPdf) -> usize {
    let state = State::at(Level::Full);
    let out = file.0.with_file_name(format!("render-{}.png", std::process::id()));
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "render", "1", "--scale", "2", "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{reply}");
    let image = read_png(&std::fs::read(&out).unwrap());
    std::fs::remove_file(&out).unwrap();
    red_pixels(&image)
}

fn icc_doc(profile: Vec<u8>, components: u32) -> Vec<u8> {
    let image = first_extra_object(1);
    let icc = image + 1;
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = format!("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace [/ICCBased {icc} 0 R] /BitsPerComponent 8 /Filter /DCTDecode");
    pages_with(
        &[Page::new("q 40 0 0 40 100 500 cm /Im Do Q\n")],
        &resources,
        vec![
            Object::Stream { dict, data: jpeg_bytes() },
            Object::Stream {
                dict: format!("/N {components} /Alternate /DeviceRGB"),
                data: profile,
            },
        ],
    )
}

/// Failure: text that clips is only noticed if it's in the box and on the page, so what it clips to nothing goes out as vector and text, with `all_vector` true. It has to count from wherever it is.
#[test]
fn what_text_clips_away_stays_out_wherever_the_text_is() {
    let art = "1 0 0 rg 300 100 200 200 re f BT /F1 12 Tf 0 Tr 320 200 Td (SECRET) Tj ET Q 0 1 0 rg 40 100 40 40 re f\n";
    let outside_the_box = format!("q BT /F1 40 Tf 7 Tr 72 700 Td (HEAD) Tj ET {art}");
    let off_the_page = format!("q BT /F1 40 Tf 7 Tr 72 -500 Td (HEAD) Tj ET {art}");
    let box_away = ["--rect", "20,400,560,380"];
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(outside_the_box)]));
    assert_eq!(red_in_render(&file), 0, "PDFium clips it away");
    let (svg, data) = svg_of(&file, &box_away);
    assert!(!svg.contains("#f00") && !svg.contains("SECRET"), "{data}");
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(off_the_page)]));
    assert_eq!(red_in_render(&file), 0, "PDFium clips it away");
    let (svg, data) = svg_of(&file, &[]);
    assert!(!svg.contains("#f00") && !svg.contains("SECRET"), "{data}");
}

/// Failure: numbers a hostile page writes get into the file as `inf`, or `1e+39`, or crash something on the way. They're left out or limited, and what comes out is sound.
#[test]
fn absurd_numbers_end_in_a_sound_file() {
    let content = "1e39 0 0 1e39 99999999999999999999999999999999999999999 0 cm 0 0 m 1e39 1e39 l S q 99999999999999999999 0 0 0 0 0 cm 0 0 10 10 re f Q BT /F1 99999999999 Tf (x) Tj ET\n";
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(content)]));
    let (reply, code, out) = export(&file, &[]);
    assert_eq!(code, 0, "{reply}");
    check_document(&std::fs::read(&out).unwrap());
    std::fs::remove_file(&out).unwrap();
}

/// Failure: text that draws nothing, such as a line of spaces, turns into a picture of nothing, and the page reads as not all vector when it is.
#[test]
fn what_draws_nothing_is_no_picture() {
    let file = TempPdf::new(
        "doc.pdf",
        &pages(&[Page::new("BT /F1 12 Tf 72 700 Td (   ) Tj ET BT /F1 12 Tf 72 680 Td (Words) Tj ET\n")]),
    );
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
    assert!(!svg.contains("<image"), "{svg}");
}

/// Failure: a link with no appearance, which nearly every paper has, makes every page read as not all vector. It draws nothing, so nothing's said.
#[test]
fn a_link_is_nothing_to_draw() {
    let page = Page {
        content: text_lines(&["Words"]),
        page_extra: "/Annots [<< /Type /Annot /Subtype /Link /Rect [100 500 200 600] >>]".to_string(),
        media: (612.0, 792.0),
    };
    let file = TempPdf::new("doc.pdf", &pages(&[page]));
    let (_, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
}

/// Failure: a JPEG with a colour profile passes through, profile dropped, so a reader takes it for sRGB, and a wide-gamut photo looks wrong. Only a profile that says it's sRGB may pass.
#[test]
fn only_an_srgb_profile_lets_a_jpeg_through() {
    let file = TempPdf::new("doc.pdf", &icc_doc(b"not a profile that says anything we know".to_vec(), 3));
    let (_, data) = svg_of(&file, &[]);
    assert_eq!(data["objects"]["images_original_jpeg"], 0, "{data}");
}
