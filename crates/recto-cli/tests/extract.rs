//! `images`, `image` and `snapshot` end to end, through the real `recto` and a real worker, and the clipboard the app's snapshot tool takes from the same functions: what an agent sees, and what's really in the file.

mod common;

use common::State;
use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
use recto_agent::Level;
use recto_agent::extract::{ClipOutcome, UTI_JPEG, UTI_PNG, UTI_SVG, UTI_TEXT, image_clip, snapshot_clip};
use recto_svg::verify::{Embedded, check_document, read_png};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages, pages_with, text_lines};
use serde_json::Value;
use std::path::PathBuf;

/// A 4 x 4 baseline JPEG, red, as `export_svg.rs` has it.
const JPEG_HEX: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffdb00430103040405040509050509140d0b0d1414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414ffc00011080004000403012200021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00f0aa28a2be68fedd3fffd9";

fn jpeg_bytes() -> Vec<u8> {
    (0..JPEG_HEX.len()).step_by(2).map(|at| u8::from_str_radix(&JPEG_HEX[at..at + 2], 16).unwrap()).collect()
}

/// A gradient any change to a pixel would show.
fn gradient(width: u32, height: u32) -> Vec<u8> {
    (0..height).flat_map(|y| (0..width).flat_map(move |x| [(x * 251 / width) as u8, (y * 241 / height) as u8, ((x ^ y) & 0xFF) as u8])).collect()
}

/// A page with text, a raw RGB image of 37 x 23 at 100, 232 (100 x 60 points, shown from the top), and the red JPEG over a 40 point square at 300, 252, drawn after it.
fn doc_with_images() -> (Vec<u8>, Vec<u8>) {
    let pixels = gradient(37, 23);
    let raw = first_extra_object(1);
    let jpeg = raw + 1;
    let resources = format!("/XObject << /Raw {raw} 0 R /Jpg {jpeg} 0 R >>");
    let content = [text_lines(&["Words beside the pictures"]), b"q 100 0 0 60 100 500 cm /Raw Do Q q 40 0 0 40 300 500 cm /Jpg Do Q\n".to_vec()].concat();
    let objects = vec![
        Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 37 /Height 23 /ColorSpace /DeviceRGB /BitsPerComponent 8".into(), data: pixels.clone() },
        Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode".into(), data: jpeg_bytes() },
    ];
    (pages_with(&[Page::new(content)], &resources, objects), pixels)
}

fn run(file: &TempPdf, args: &[&str]) -> (Value, i32) {
    let state = State::at(Level::Full);
    let mut all = vec!["local", file.0.to_str().unwrap()];
    all.extend_from_slice(args);
    state.json(&all)
}

fn ok(file: &TempPdf, args: &[&str]) -> Value {
    let (reply, code) = run(file, args);
    assert_eq!(code, 0, "{args:?}: {reply}");
    reply["data"].clone()
}

fn out_path(file: &TempPdf, name: &str) -> PathBuf {
    file.0.with_file_name(format!("{}-{name}", std::process::id()))
}

fn folder_entries(file: &TempPdf) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(file.0.parent().unwrap()).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// Failure: an image's place or number is wrong, so the click or the number an agent passes on takes another image, or none. Each must be where the page shows it, numbered in the order it's drawn, with its own size and whether it's a JPEG.
#[test]
fn a_pages_images_are_listed_where_they_show() {
    let (bytes, _) = doc_with_images();
    let file = TempPdf::new("doc.pdf", &bytes);
    let listed = ok(&file, &["images", "1"]);
    assert_eq!(listed["count"], 2, "{listed}");
    let images = listed["images"].as_array().unwrap();
    assert_eq!((images[0]["number"].as_u64(), images[0]["width"].as_u64(), images[0]["height"].as_u64(), images[0]["jpeg"].as_bool()), (Some(1), Some(37), Some(23), Some(false)));
    let rect: Vec<f64> = images[0]["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(rect, vec![100.0, 232.0, 100.0, 60.0], "{listed}");
    assert_eq!((images[1]["number"].as_u64(), images[1]["jpeg"].as_bool()), (Some(2), Some(true)));
    let none = TempPdf::new("plain.pdf", &pages(&[Page::new(text_lines(&["No pictures"]))]));
    assert_eq!(ok(&none, &["images", "1"])["count"], 0);
}

/// Failure: an image comes out of the page re-encoded or resampled, which "losslessly" rules out. A JPEG to `.jpg` must be its own bytes, and any image to `.png` its own pixels at its own size, byte for byte. A JPEG asked for as `.jpg` that can't go as itself is refused rather than re-encoded, and leaves nothing.
#[test]
fn an_image_comes_out_as_itself() {
    let (bytes, pixels) = doc_with_images();
    let file = TempPdf::new("doc.pdf", &bytes);
    let jpg = out_path(&file, "image.jpg");
    let data = ok(&file, &["image", "1", "--number", "2", "--out", jpg.to_str().unwrap()]);
    assert_eq!((data["coded"].as_str(), data["lossless"].as_bool()), (Some("original_jpeg"), Some(true)), "{data}");
    assert_eq!(std::fs::read(&jpg).unwrap(), jpeg_bytes(), "byte for byte");
    let png = out_path(&file, "image.png");
    let data = ok(&file, &["image", "1", "--number", "1", "--out", png.to_str().unwrap()]);
    assert_eq!(data["coded"], "png", "{data}");
    assert_eq!(read_png(&std::fs::read(&png).unwrap()), Embedded::Png { width: 37, height: 23, channels: 3, pixels });
    // The JPEG as PNG is its picture decoded, at its own size.
    let jpeg_as_png = out_path(&file, "jpeg.png");
    ok(&file, &["image", "1", "--number", "2", "--out", jpeg_as_png.to_str().unwrap()]);
    let Embedded::Png { width: 4, height: 4, pixels: red, .. } = read_png(&std::fs::read(&jpeg_as_png).unwrap()) else { panic!() };
    assert!(red.chunks(3).all(|pixel| pixel[0] >= 180 && pixel[1] < 80 && pixel[2] < 80), "red: {red:?}");
    for path in [&jpg, &png, &jpeg_as_png] {
        std::fs::remove_file(path).unwrap();
    }
    let before = folder_entries(&file);
    let refused = out_path(&file, "not.jpg");
    let (reply, code) = run(&file, &["image", "1", "--number", "1", "--out", refused.to_str().unwrap()]);
    assert_eq!((code, reply["error"]["code"].as_str()), (1, Some("not_a_jpeg")), "{reply}");
    let (reply, code) = run(&file, &["image", "1", "--number", "3", "--out", refused.to_str().unwrap()]);
    assert_eq!((code, reply["error"]["code"].as_str()), (recto_agent::exit::NOT_FOUND, Some("no_such_image")), "{reply}");
    assert_eq!(folder_entries(&file), before, "nothing left behind");
}

/// Failure: an image with a soft mask comes out as a solid block, its mask lost. It must be a PNG at its own size with the mask as alpha.
#[test]
fn a_masked_image_keeps_its_mask_as_alpha() {
    let image = first_extra_object(1);
    let mask = image + 1;
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = format!("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /SMask {mask} 0 R");
    let mask_dict = "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let alpha: Vec<u8> = (0..16).map(|at| if at % 2 == 0 { 255 } else { 0 }).collect();
    let bytes = pages_with(&[Page::new("q 40 0 0 40 100 500 cm /Im Do Q\n")], &resources, vec![Object::Stream { dict, data: jpeg_bytes() }, Object::Stream { dict: mask_dict.into(), data: alpha }]);
    let file = TempPdf::new("doc.pdf", &bytes);
    let png = out_path(&file, "masked.png");
    let data = ok(&file, &["image", "1", "--number", "1", "--out", png.to_str().unwrap()]);
    assert_eq!(data["coded"], "png_with_mask", "{data}");
    let Embedded::Png { width: 4, height: 4, channels: 4, pixels } = read_png(&std::fs::read(&png).unwrap()) else { panic!() };
    assert!(pixels.chunks(4).any(|pixel| pixel[3] == 255) && pixels.chunks(4).any(|pixel| pixel[3] < 128), "{pixels:?}");
    std::fs::remove_file(&png).unwrap();
}

/// Failure: a snapshot's picture isn't the box: the wrong size, or not the page's pixels there. The PNG must be the box at the scale asked, and the JPG the same picture, re-encoded and saying so, as a real decoder (PDFium's, by a page made of it) reads it.
#[test]
fn a_snapshot_picture_is_the_box_drawn_sharp() {
    let content = "1 0 0 rg 100 500 200 100 re f 0 0 1 rg 300 500 100 100 re f\n";
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(content)]));
    // From the top left: the red box is 100..300 across and 192..292 down; the blue one 300..400.
    let png = out_path(&file, "box.png");
    let data = ok(&file, &["snapshot", "1", "--rect", "100,192,300,100", "--as", "png", "--out", png.to_str().unwrap()]);
    assert_eq!((data["width"].as_u64(), data["height"].as_u64(), data["reencoded"].as_bool()), (Some(600), Some(200), Some(false)), "{data}");
    let Embedded::Png { width: 600, height: 200, channels: 3, pixels } = read_png(&std::fs::read(&png).unwrap()) else { panic!() };
    let at = |x: usize, y: usize| &pixels[(y * 600 + x) * 3..(y * 600 + x) * 3 + 3];
    assert_eq!(at(100, 100), [255, 0, 0], "red on the left");
    assert_eq!(at(500, 100), [0, 0, 255], "blue on the right");
    let jpg = out_path(&file, "box.jpg");
    let data = ok(&file, &["snapshot", "1", "--rect", "100,192,300,100", "--as", "jpg", "--out", jpg.to_str().unwrap()]);
    assert_eq!(data["reencoded"], true, "{data}");
    // Decoded by PDFium: a page that is the JPEG, drawn at 1x.
    let jpeg = std::fs::read(&jpg).unwrap();
    let image = first_extra_object(1);
    let made = pages_with(&[Page { content: b"q 600 0 0 200 0 0 cm /Im Do Q\n".to_vec(), page_extra: String::new(), media: (600.0, 200.0) }], &format!("/XObject << /Im {image} 0 R >>"), vec![Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 600 /Height 200 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode".into(), data: jpeg }]);
    let decoded = TempPdf::new("decoded.pdf", &made);
    let back = out_path(&decoded, "back.png");
    ok(&decoded, &["render", "1", "--scale", "1", "--out", back.to_str().unwrap()]);
    let Embedded::Png { pixels: seen, .. } = read_png(&std::fs::read(&back).unwrap()) else { panic!() };
    let differing = seen.iter().zip(&pixels).filter(|(a, b)| a.abs_diff(**b) > 24).count();
    assert!(differing * 100 < pixels.len(), "{differing} of {} channels differ by more than 24", pixels.len());
    for path in [&png, &jpg, &back] {
        std::fs::remove_file(path).unwrap();
    }
}

/// Failure: a snapshot's SVG holds what's outside the box, or its text comes from outside it or out of order. The SVG is `export svg --rect`'s, and the text is what's inside, a line at a time.
#[test]
fn a_snapshot_svg_and_text_hold_only_the_box() {
    let content = "BT /F1 12 Tf 72 700 Td (INSIDE FIRST) Tj ET BT /F1 12 Tf 72 680 Td (INSIDE SECOND) Tj ET BT /F1 12 Tf 72 100 Td (OUTSIDE) Tj ET\n";
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(content)]));
    let svg = out_path(&file, "box.svg");
    ok(&file, &["snapshot", "1", "--rect", "60,80,200,50", "--as", "svg", "--out", svg.to_str().unwrap()]);
    let bytes = std::fs::read(&svg).unwrap();
    check_document(&bytes);
    let svg_text = String::from_utf8(bytes).unwrap();
    assert!(svg_text.contains("INSIDE FIRST") && !svg_text.contains("OUTSIDE"), "{svg_text}");
    std::fs::remove_file(&svg).unwrap();
    let text = ok(&file, &["snapshot", "1", "--rect", "60,80,200,50", "--as", "text"]);
    assert_eq!(text["untrusted"]["text"], "INSIDE FIRST\nINSIDE SECOND", "{text}");
    // Text goes in the answer, below Full, as `copy` does, and a file is an export, at Full.
    let state = State::at(Level::Control);
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "snapshot", "1", "--rect", "60,80,200,50", "--as", "text"]);
    assert_eq!(code, 0, "{reply}");
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "snapshot", "1", "--rect", "60,80,200,50", "--as", "png", "--out", out_path(&file, "x.png").to_str().unwrap()]);
    assert_eq!((code, reply["error"]["code"].as_str()), (recto_agent::exit::PERMISSION, Some("permission_denied")), "a file is an export: {reply}");
}

/// Failure: an export lands somewhere a shell or an app acts on it, a hidden folder or one through a link to a file of another kind, which a review of Save found every write path must refuse; or a box off the page draws something.
#[test]
fn snapshots_and_images_keep_the_rules_on_where_they_write() {
    let (bytes, _) = doc_with_images();
    let file = TempPdf::new("doc.pdf", &bytes);
    let hidden = file.0.with_file_name(format!(".hidden-{}", std::process::id()));
    std::fs::create_dir_all(&hidden).unwrap();
    for args in [vec!["image", "1", "--number", "1", "--out"], vec!["snapshot", "1", "--rect", "0,0,10,10", "--as", "png", "--out"]] {
        let target = hidden.join("x.png");
        let mut all = args.clone();
        all.push(target.to_str().unwrap());
        let (reply, code) = run(&file, &all);
        assert_eq!((code, reply["error"]["code"].as_str()), (1, Some("bad_output")), "{reply}");
    }
    let (reply, _) = run(&file, &["snapshot", "1", "--rect", "900,900,10,10", "--as", "png", "--out", out_path(&file, "off.png").to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "bad_rect", "{reply}");
    let (reply, _) = run(&file, &["snapshot", "1", "--rect", "0,0,10,10", "--as", "png", "--out", out_path(&file, "wrong.jpg").to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "bad_output", "{reply}");
    std::fs::remove_dir_all(&hidden).unwrap();
}

/// A file anyone can read and that asks not to be copied, with an image on it.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts; never over a user's file.
fn restricted() -> Vec<u8> {
    let (bytes, _) = doc_with_images();
    let mut document = lopdf::Document::load_mem(&bytes).unwrap();
    document.trailer.set("ID", lopdf::Object::Array(vec![lopdf::Object::string_literal("0123456789abcdef"), lopdf::Object::string_literal("0123456789abcdef")]));
    let state = EncryptionState::try_from(EncryptionVersion::V2 { document: &document, owner_password: "owner", user_password: "", key_length: 128, permissions: Permissions::all() - Permissions::COPYABLE }).unwrap();
    document.encrypt(&state).unwrap();
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    out
}

/// Failure: an image or a snapshot gets round a file's ask not to be copied, though the owner decided on 2026-11-08 they get the same notice as text; or `--anyway` doesn't get past it. Exit 8 without it, for every format.
#[test]
fn a_file_that_asks_not_to_be_copied_refuses_images_and_snapshots_unless_anyway() {
    let file = TempPdf::new("restricted.pdf", &restricted());
    let png = out_path(&file, "r.png");
    let (reply, code) = run(&file, &["image", "1", "--number", "1", "--out", png.to_str().unwrap()]);
    assert_eq!((code, reply["error"]["code"].as_str()), (recto_agent::exit::PROTECTED, Some("copy_restricted")), "{reply}");
    for format in ["png", "text"] {
        let mut args = vec!["snapshot", "1", "--rect", "0,0,300,300", "--as", format];
        if format == "png" {
            args.extend(["--out", png.to_str().unwrap()]);
        }
        let (reply, code) = run(&file, &args);
        assert_eq!((code, reply["error"]["code"].as_str()), (recto_agent::exit::PROTECTED, Some("copy_restricted")), "{format}: {reply}");
    }
    assert!(!png.exists());
    let data = ok(&file, &["image", "1", "--number", "1", "--out", png.to_str().unwrap(), "--anyway"]);
    assert_eq!(data["anyway"], true);
    std::fs::remove_file(&png).unwrap();
    assert!(matches!(snapshot_clip(&file.0, 0, [0.0, 0.0, 300.0, 300.0], false), ClipOutcome::Restricted));
    assert!(matches!(image_clip(&file.0, 0, 150.0, 260.0, false), ClipOutcome::Restricted));
    assert!(matches!(image_clip(&file.0, 0, 150.0, 260.0, true), ClipOutcome::Ready(_)));
}

/// Failure: the clipboard gets the wrong types, or in an order that hands a JPEG's paste a re-encoded copy: a click on a JPEG must give its own bytes first, with a PNG beside it for what can't take a JPEG; on another image, a lossless PNG alone; and a snapshot PNG, text, SVG and JPG, all at once. A click off any image gives nothing.
#[test]
fn the_clipboard_gets_the_right_types_in_the_right_order() {
    let (bytes, pixels) = doc_with_images();
    let file = TempPdf::new("doc.pdf", &bytes);
    let ClipOutcome::Ready(jpeg) = image_clip(&file.0, 0, 320.0, 270.0, false) else { panic!("the JPEG") };
    assert_eq!(jpeg.items.iter().map(|item| item.uti).collect::<Vec<_>>(), vec![UTI_JPEG, UTI_PNG], "{}", jpeg.note);
    assert_eq!(jpeg.items[0].bytes, jpeg_bytes());
    let ClipOutcome::Ready(raw) = image_clip(&file.0, 0, 150.0, 260.0, false) else { panic!("the raw image") };
    assert_eq!(raw.items.iter().map(|item| item.uti).collect::<Vec<_>>(), vec![UTI_PNG]);
    assert_eq!(read_png(&raw.items[0].bytes), Embedded::Png { width: 37, height: 23, channels: 3, pixels });
    assert!(matches!(image_clip(&file.0, 0, 500.0, 700.0, false), ClipOutcome::NoImage));
    let ClipOutcome::Ready(snap) = snapshot_clip(&file.0, 0, [60.0, 50.0, 400.0, 270.0], false) else { panic!("the snapshot") };
    assert_eq!(snap.items.iter().map(|item| item.uti).collect::<Vec<_>>(), vec![UTI_PNG, UTI_TEXT, UTI_SVG, UTI_JPEG], "{}", snap.note);
    assert_eq!(String::from_utf8(snap.items[1].bytes.clone()).unwrap(), "Words beside the pictures");
    check_document(&snap.items[2].bytes);
}
