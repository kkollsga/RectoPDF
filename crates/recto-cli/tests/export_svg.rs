//! `export svg` end to end, through the real `recto` and a real worker: what an agent sees, and what's really in the file.

mod common;

use common::State;
use recto_agent::Level;
use recto_svg::verify::{Embedded, check_document, embedded_images};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, form_bomb, pages, pages_with, text_lines};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A 4 x 4 baseline JPEG, red.
const JPEG_HEX: &str = "ffd8ffe000104a46494600010100000100010000ffdb0043000302020302020303030304030304050805050404050a070706080c0a0c0c0b0a0b0b0d0e12100d0e110e0b0b1016101113141515150c0f171816141812141514ffdb00430103040405040509050509140d0b0d1414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414ffc00011080004000403012200021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00f0aa28a2be68fedd3fffd9";

fn jpeg_bytes() -> Vec<u8> {
    (0..JPEG_HEX.len()).step_by(2).map(|at| u8::from_str_radix(&JPEG_HEX[at..at + 2], 16).unwrap()).collect()
}

fn export(file: &TempPdf, extra: &[&str]) -> (Value, i32, PathBuf) {
    let state = State::at(Level::Full);
    let out = file.0.with_file_name(format!("out-{}.svg", std::process::id()));
    let mut args = vec!["local", file.0.to_str().unwrap(), "export", "svg", "--page", "1", "--out", out.to_str().unwrap()];
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
    let mut names: Vec<String> = std::fs::read_dir(file.0.parent().unwrap()).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn rasterised(data: &Value, why: &str) -> u64 {
    data["rasterised"].as_array().unwrap().iter().find(|item| item["why"] == why).map_or(0, |item| item["count"].as_u64().unwrap())
}

fn note(data: &Value, name: &str) -> u64 {
    data["notes"].as_array().unwrap().iter().find(|item| item["note"] == name).map_or(0, |item| item["count"].as_u64().unwrap())
}

/// Failure: an ordinary page's text and shapes don't come through as what they are, or it's quietly a picture. It must have paths, outlines and real text, say nothing was rasterised, and leave nothing behind but the file.
#[test]
fn an_ordinary_page_is_all_vector() {
    let content = [b"0 0 1 rg 72 600 200 100 re f 1 0 0 RG 3 w 72 500 m 300 550 l S\n".as_slice(), &text_lines(&["Hello vector world"])].concat();
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(content)]));
    let before = folder_entries(&file);
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
    assert_eq!(data["width_pt"], 612.0);
    assert!(svg.contains("<path") && svg.contains("<use") && svg.contains(">Hello vector world</text>"), "{svg}");
    assert!(svg.contains("fill=\"#00f\""), "the blue fill: {svg}");
    assert!(!svg.contains("<image"), "{svg}");
    assert_eq!(folder_entries(&file), before, "something was left behind");
}

/// Failure: the three ways of writing text aren't what they say: outlines without characters come out selectable, or a page that wants real text gets shapes.
#[test]
fn each_text_mode_is_what_it_says() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words here"]))]));
    let (both, _) = svg_of(&file, &["--text", "both"]);
    assert!(both.contains("<use") && both.contains(">Words here</text>") && both.contains("fill-opacity=\"0\""), "{both}");
    let (paths, _) = svg_of(&file, &["--text", "paths"]);
    assert!(paths.contains("<use") && !paths.contains("<text"), "{paths}");
    let (selectable, data) = svg_of(&file, &["--text", "selectable"]);
    assert!(!selectable.contains("<use") && selectable.contains(">Words here</text>") && !selectable.contains("fill-opacity=\"0\""), "{selectable}");
    assert_eq!(data["text"], "selectable");
    assert!(selectable.len() < both.len() && paths.len() < both.len(), "both is the largest: {} {} {}", selectable.len(), paths.len(), both.len());
}

/// Failure: text in a PDF, or its title, breaks out of the XML, so a page reading `</svg><script>` runs when the file is opened. It must stay plain text, in a well-formed file, with no way out.
#[test]
fn hostile_text_and_a_hostile_title_cannot_break_out() {
    let hostile = "</svg><script>alert(1)</script>";
    let content = "BT /F1 12 Tf 72 700 Td (\\</svg\\><script\\>alert\\(1\\)</script\\> & ' \\\" ]]>) Tj ET\n";
    let info = first_extra_object(1);
    let bytes = pages_with(&[Page::new(content)], "", vec![Object::Dict(format!("<< /Title ({}) /Author (' onload='x) >>", hostile.replace('(', "\\(").replace(')', "\\)")))]);
    let bytes = String::from_utf8_lossy(&bytes).replace("/Root 1 0 R", &format!("/Root 1 0 R /Info {info} 0 R"));
    let file = TempPdf::new("doc.pdf", bytes.as_bytes());
    let (svg, _) = svg_of(&file, &[]);
    assert!(!svg.contains("<script"), "{svg}");
    assert!(svg.contains("&lt;script"), "the words stay as words: {svg}");
    assert!(svg.contains("<title>&lt;/svg&gt;&lt;script&gt;"), "the title is there, escaped: {svg}");
}

/// Failure: something outside a snapshot's box is in the file anyway, just hidden, so a box around one paragraph hands over the whole page. SPEC: left out rather than hidden.
#[test]
fn a_box_leaves_out_whats_outside_it() {
    let content = "BT /F1 12 Tf 72 700 Td (INSIDE) Tj ET BT /F1 12 Tf 72 100 Td (OUTSIDE) Tj ET 1 0 0 rg 400 100 50 50 re f\n";
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(content)]));
    // From the top left, and the first line's baseline is 92 points down.
    let (svg, data) = svg_of(&file, &["--rect", "60,70,200,40"]);
    assert!(svg.contains(">INSIDE</text>"), "{svg}");
    assert!(!svg.contains("OUTSIDE") && !svg.contains("#f00"), "{svg}");
    assert_eq!((data["width_pt"].as_f64(), data["height_pt"].as_f64()), (Some(200.0), Some(40.0)));
    assert!(svg.contains("viewBox=\"0 0 200 40\""), "{svg}");
    // Past the page's edge, the box stops at it.
    let (_, cut) = svg_of(&file, &["--rect", "600,780,500,500"]);
    assert_eq!((cut["width_pt"].as_f64(), cut["height_pt"].as_f64()), (Some(12.0), Some(12.0)));
}

/// Failure: a line crossing a box's edge hands over all its words, though only some are in the box. It's left out a character at a time.
#[test]
fn a_box_cuts_a_line_of_text_at_its_edge() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new("BT /F1 12 Tf 72 700 Td (KEEP THESE WORDS AND LOSE EVERYTHING AFTER THEM) Tj ET\n")]));
    let (svg, _) = svg_of(&file, &["--rect", "60,80,100,20"]);
    assert!(svg.contains("KEEP THESE"), "{svg}");
    assert!(!svg.contains("EVERYTHING"), "{svg}");
}

/// A page with one RGB image with no filter, with a gradient any change to a pixel would show.
fn raw_image_doc(width: u32, height: u32, cm: &str) -> (Vec<u8>, Vec<u8>) {
    let pixels: Vec<u8> = (0..height).flat_map(|y| (0..width).flat_map(move |x| [(x * 251 / width.max(1)) as u8, (y * 241 / height.max(1)) as u8, ((x ^ y) & 0xFF) as u8])).collect();
    let image = first_extra_object(1);
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = format!("/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8");
    let page = Page::new(format!("q {cm} cm /Im Do Q\n"));
    (pages_with(&[page], &resources, vec![Object::Stream { dict, data: pixels.clone() }]), pixels)
}

/// Failure: an image in a page is resampled or recompressed lossily on its way into the file, which is what "lossless" is there to rule out. Its pixels must come back byte for byte, at its own size and not the page's.
#[test]
fn an_image_comes_through_with_every_pixel_as_it_was() {
    let (bytes, pixels) = raw_image_doc(37, 23, "100 0 0 60 100 500");
    let file = TempPdf::new("doc.pdf", &bytes);
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["objects"]["images_png"], 1, "{data}");
    assert_eq!(data["all_vector"], true, "{data}");
    let images = embedded_images(svg.as_bytes());
    assert_eq!(images, vec![Embedded::Png { width: 37, height: 23, channels: 3, pixels }]);
}

/// Failure: a box around part of an image hands over all of it, or the cut part stands somewhere else. It must be cut, keep its own resolution, and hold exactly the pixels that were in the box.
#[test]
fn a_box_cuts_an_image_to_what_it_covers() {
    let (bytes, pixels) = raw_image_doc(100, 100, "100 0 0 100 100 500");
    let file = TempPdf::new("doc.pdf", &bytes);
    // The image's left half, from 100 to 150 across, and its full height of 192 to 292 down.
    let (svg, _) = svg_of(&file, &["--rect", "100,192,50,100"]);
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { width, height, channels, pixels: cut }] = &images[..] else { panic!("{images:?}") };
    assert_eq!((*width, *height, *channels), (50, 100, 3));
    let want: Vec<u8> = pixels.chunks(300).flat_map(|row| row[..150].to_vec()).collect();
    assert_eq!(*cut, want, "the left half, exactly");
}

fn jpeg_doc(extra_dict: &str, cm: &str) -> Vec<u8> {
    let image = first_extra_object(1);
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = format!("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode {extra_dict}");
    pages_with(&[Page::new(format!("q {cm} cm /Im Do Q\n"))], &resources, vec![Object::Stream { dict, data: jpeg_bytes() }])
}

/// Failure: a JPEG is decoded and encoded again, which is what SPEC's "JPG honesty" rules out: it's lossless only as the original bytes. They must be there, unchanged, and the answer must say so.
#[test]
fn a_jpeg_stays_its_original_bytes() {
    let file = TempPdf::new("doc.pdf", &jpeg_doc("", "40 0 0 40 100 500"));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["objects"]["images_original_jpeg"], 1, "{data}");
    assert_eq!(embedded_images(svg.as_bytes()), vec![Embedded::Jpeg(jpeg_bytes())]);
}

/// Failure: a JPEG that crosses a box's edge goes in whole, with what's outside the box still in it. A JPEG can't be cut losslessly, so it's a PNG of the part inside.
#[test]
fn a_jpeg_crossing_a_box_goes_as_the_part_inside() {
    let file = TempPdf::new("doc.pdf", &jpeg_doc("", "40 0 0 40 100 500"));
    let (svg, data) = svg_of(&file, &["--rect", "100,252,20,40"]);
    assert_eq!(data["objects"]["images_png"], 1, "{data}");
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { width, height, .. }] = &images[..] else { panic!("{images:?}") };
    assert_eq!((*width, *height), (2, 4));
}

/// Failure: a mask on a JPEG is lost by passing its bytes through, so what was cut out comes back as a solid block. An image with a mask must come through with it as alpha.
#[test]
fn an_image_with_a_soft_mask_keeps_it() {
    let mask = first_extra_object(1) + 1;
    let image = first_extra_object(1);
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = format!("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /SMask {mask} 0 R");
    let mask_dict = "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let bytes = pages_with(&[Page::new("q 40 0 0 40 100 500 cm /Im Do Q\n")], &resources, vec![Object::Stream { dict, data: jpeg_bytes() }, Object::Stream { dict: mask_dict.into(), data: vec![255, 0, 0, 255] }]);
    let file = TempPdf::new("doc.pdf", &bytes);
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["objects"]["images_original_jpeg"], 0, "{data}");
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { channels: 4, pixels, .. }] = &images[..] else { panic!("{images:?}") };
    let alphas: Vec<u8> = pixels.chunks(4).map(|pixel| pixel[3]).collect();
    assert!(alphas.contains(&255) && alphas.iter().any(|alpha| *alpha < 128), "the mask's holes and solid parts: {alphas:?}");
}

/// Failure: a stencil, which paints the current colour through a 1-bit shape, comes out as a grey picture of the shape, with its colour lost and its holes filled. It must keep both.
#[test]
fn a_stencil_keeps_its_colour_and_its_holes() {
    let image = first_extra_object(1);
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ImageMask true /BitsPerComponent 1".to_string();
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new("1 0 0 rg q 40 0 0 40 100 500 cm /Im Do Q\n")], &resources, vec![Object::Stream { dict, data: vec![0xF0; 8] }]));
    let (svg, _) = svg_of(&file, &[]);
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { width: 8, height: 8, channels: 4, pixels }] = &images[..] else { panic!("{images:?}") };
    assert!(pixels.chunks(4).any(|pixel| pixel == [255, 0, 0, 255]), "the shape, in the colour: {pixels:?}");
    assert!(pixels.chunks(4).any(|pixel| pixel[3] == 0), "and its holes");
}

/// Failure: drawing an image to find its mask obeys the image's clip as well, in coordinates that mean nothing there, so a JPEG reads as having a mask, loses its original bytes, and comes out with a clip's shape cut out of its pixels. It's the reason a NASA cover's photo came out with its side missing.
#[test]
fn a_clip_on_an_image_stays_out_of_its_pixels() {
    let image = first_extra_object(1);
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let raw = |extra: &str, data: Vec<u8>| Object::Stream { dict: format!("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8 {extra}"), data };
    let clip = "q 110 510 15 15 re W n q 40 0 0 40 100 500 cm /Im Do Q Q\n";
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new(clip)], &resources, vec![raw("/Filter /DCTDecode", jpeg_bytes())]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(embedded_images(svg.as_bytes()), vec![Embedded::Jpeg(jpeg_bytes())], "{data}");
    assert!(svg.contains("<clipPath"), "the clip goes in the file instead");
    // A real mask with a clip on top: the mask's in the pixels and the clip isn't.
    let mask = image + 1;
    let masked = raw(&format!("/SMask {mask} 0 R"), vec![200; 48]);
    let mask_image = Object::Stream { dict: "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8".into(), data: vec![255, 0, 0, 255] };
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new(clip)], &resources, vec![masked, mask_image]));
    let (svg, _) = svg_of(&file, &[]);
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { width: 4, height: 4, channels: 4, pixels }] = &images[..] else { panic!("{images:?}") };
    let alphas: Vec<u8> = pixels.chunks(4).map(|pixel| pixel[3]).collect();
    assert!(alphas[0] > 200 && alphas[3] < 60 && alphas[12] < 60 && alphas[15] > 200, "the mask's corners, whatever the clip: {alphas:?}");
}

/// Failure: a rotated image's clip is on a rotated image, and it's cut wrongly, or the image is no longer where it should be.
#[test]
fn a_turned_image_with_a_clip_keeps_every_pixel() {
    let (_, pixels) = raw_image_doc(16, 8, "0 40 -40 0 300 500");
    let image = first_extra_object(1);
    let resources = format!("/XObject << /Im {image} 0 R >>");
    let dict = "/Type /XObject /Subtype /Image /Width 16 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8".to_string();
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new("q 265 505 30 30 re W n q 0 40 -40 0 300 500 cm /Im Do Q Q\n")], &resources, vec![Object::Stream { dict, data: pixels.clone() }]));
    let (svg, _) = svg_of(&file, &[]);
    assert_eq!(embedded_images(svg.as_bytes()), vec![Embedded::Png { width: 16, height: 8, channels: 3, pixels }]);
}

/// Failure: stroked text comes out the wrong weight, because the pen's width goes by a transform PDFium doesn't tell, which made a figure's labels come out as black blobs. It's a picture, with its real characters over it.
#[test]
fn stroked_text_is_a_picture_with_its_characters_kept() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new("q 0.1 0 0 0.1 0 0 cm BT /F1 120 Tf 2 Tr 5 w 720 7000 Td (Heavy) Tj ET Q\n")]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(rasterised(&data, "text_no_outline"), 1, "{data}");
    assert!(svg.contains(">Heavy</text>") && !svg.contains("<use"), "{svg}");
}

/// Failure: what SVG here can't express is silently dropped, so a gradient, a pattern or a blend just isn't there, and nothing says so. Each must come through as a picture, and the answer must say what and why.
#[test]
fn what_cant_be_vector_is_a_picture_and_is_named() {
    let extra_object = first_extra_object(1);
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 200 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>";
    let cases: Vec<(&str, String, String, Vec<Object>)> = vec![
        ("shading", "q 100 500 200 100 re W n 1 0 0 1 100 0 cm /Sh sh Q\n".to_string(), format!("/Shading << /Sh {shading} >>"), vec![]),
        ("shading_pattern", "/Pattern cs /Pa scn 100 500 200 100 re f\n".to_string(), format!("/Pattern << /Pa << /PatternType 2 /Shading {shading} >> >>"), vec![]),
        (
            "tiling_pattern",
            "/Pattern cs /Pa scn 100 500 200 100 re f\n".to_string(),
            format!("/Pattern << /Pa {extra_object} 0 R >>"),
            vec![Object::Stream { dict: "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>".to_string(), data: b"1 0 0 rg 0 0 5 5 re f 0 1 0 rg 5 5 5 5 re f".to_vec() }],
        ),
        ("blend_or_mask", "0 0 1 rg 90 490 100 100 re f /G gs 1 1 0 rg 100 500 200 100 re f\n".to_string(), "/ExtGState << /G << /BM /Multiply >> >>".to_string(), vec![]),
    ];
    for (why, content, resources, objects) in cases {
        let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new(content)], &resources, objects));
        let (svg, data) = svg_of(&file, &[]);
        assert!(rasterised(&data, why) >= 1, "{why}: {data}");
        assert_eq!(data["all_vector"], false, "{why}");
        let images = embedded_images(svg.as_bytes());
        assert!(!images.is_empty(), "{why}: no picture");
        // It's really there: a picture of nothing would be nearly all one thing.
        let Embedded::Png { pixels, channels, .. } = &images[0] else { panic!("{why}") };
        let painted = pixels.chunks(*channels).filter(|pixel| *channels == 3 || pixel[3] > 0).count();
        assert!(painted > 100, "{why}: only {painted} pixels drawn");
    }
}

/// Failure: a Type 3 font's text goes missing, because it has no outlines to take. It's a picture, with its real characters still selectable.
#[test]
fn text_with_no_outlines_is_a_picture_and_stays_selectable() {
    let glyph = first_extra_object(1) + 1;
    let font = first_extra_object(1);
    let resources = format!("/Font << /T3 {font} 0 R >>");
    let font_dict = format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a {glyph} 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] >>");
    let bytes = pages_with(&[Page::new("BT /T3 40 Tf 100 500 Td (aaa) Tj ET\n")], &resources, vec![Object::Dict(font_dict), Object::Stream { dict: String::new(), data: b"1000 0 0 0 1000 1000 d1 100 100 800 800 re f".to_vec() }]);
    let file = TempPdf::new("doc.pdf", &bytes);
    let (svg, data) = svg_of(&file, &[]);
    assert!(rasterised(&data, "text_no_outline") >= 1, "{data}");
    assert!(!embedded_images(svg.as_bytes()).is_empty());
    assert!(svg.contains(">aaa</text>"), "its characters, over the picture: {}", &svg[..svg.len().min(600)]);
}

/// Failure: words whose only gap is a kern, as every line of TeX is, come out selected as one long word, since PDFium's made-up spaces aren't drawn. They're kept in the text, and only there.
#[test]
fn a_gap_that_reads_as_a_space_stays_in_the_selectable_text() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new("BT /F1 12 Tf 72 700 Td [(two) -400 (words)] TJ ET\n")]));
    let (svg, _) = svg_of(&file, &[]);
    assert!(svg.contains(">two words</text>"), "{svg}");
    assert_eq!(svg.matches("<use").count(), 8, "no glyph for the space: {svg}");
}

/// Failure: text meant to be unseen comes out seen, or an agent can't tell it's there. It's kept, as PDFium gives it, but draws nothing, and the answer counts it, since an OCR layer is unseen too.
#[test]
fn invisible_text_stays_unseen_and_is_counted() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new("BT /F1 12 Tf 3 Tr 72 700 Td (quiet words) Tj ET\n")]));
    let (svg, data) = svg_of(&file, &[]);
    assert!(svg.contains(">quiet words</text>") && svg.contains("fill-opacity=\"0\"") && !svg.contains("<use"), "{svg}");
    assert_eq!(note(&data, "invisible_text_chars"), 11, "{data}");
}

/// Failure: a layer that's off comes out in the export, so what the page doesn't show is handed over. It must be left out, and counted.
#[test]
fn a_layer_thats_off_is_left_out_and_counted() {
    let group = first_extra_object(1);
    let resources = format!("/Properties << /Off {group} 0 R >>");
    let content = "/OC /Off BDC 1 0 0 rg 100 100 50 50 re f EMC 0 1 0 rg 300 300 50 50 re f\n";
    let bytes = pages_with(&[Page::new(content)], &resources, vec![Object::Dict("<< /Type /OCG /Name (Secret) >>".into())]);
    let bytes = String::from_utf8_lossy(&bytes).replace("/Type /Catalog", &format!("/Type /Catalog /OCProperties << /OCGs [{group} 0 R] /D << /OFF [{group} 0 R] >> >>"));
    let file = TempPdf::new("doc.pdf", bytes.as_bytes());
    let (svg, data) = svg_of(&file, &[]);
    assert!(svg.contains("#0f0"), "{svg}");
    assert!(!svg.contains("#f00"), "{svg}");
    assert_eq!(note(&data, "hidden_layer"), 1, "{data}");
}

/// Failure: a page turned by `/Rotate` comes out on its side or off the document, with its size the wrong way round.
#[test]
fn a_rotated_page_stands_as_it_is_shown() {
    let page = Page { content: text_lines(&["Turned"]), page_extra: "/Rotate 90".to_string(), media: (612.0, 792.0) };
    let file = TempPdf::new("doc.pdf", &pages(&[page]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!((data["width_pt"].as_f64(), data["height_pt"].as_f64()), (Some(792.0), Some(612.0)));
    assert!(svg.contains("matrix(0 1 1 0"), "{svg}");
}

/// Failure: a hostile page takes the host's memory, or a stack, or comes out as a broken file: a path of millions of points, and clips nested far past what a reader follows. Each ends as a well-formed file with the excess as a picture.
#[test]
fn huge_paths_and_deep_clips_end_in_a_sound_file() {
    let mut huge = String::from("0 0 m\n");
    for i in 0..1_100_000u32 {
        huge.push_str(&format!("{} {} l\n", i % 600, i % 790));
    }
    huge.push_str("S\n");
    let file = TempPdf::new("huge.pdf", &pages(&[Page::new(huge)]));
    let (_, data) = svg_of(&file, &[]);
    assert!(rasterised(&data, "too_complex") >= 1, "{data}");
    let mut deep = String::new();
    for i in 0..200 {
        deep.push_str(&format!("q {} {} {} {} re W n\n", i % 7, i % 5, 600 - i, 700 - i));
    }
    deep.push_str("1 0 0 rg 100 100 200 200 re f\n");
    for _ in 0..200 {
        deep.push_str("Q\n");
    }
    let file = TempPdf::new("deep.pdf", &pages(&[Page::new(deep)]));
    let (svg, data) = svg_of(&file, &[]);
    assert!(rasterised(&data, "too_complex") >= 1, "{data}");
    assert!(svg.matches("<clipPath").count() <= usize::from(recto_proto::contents::MAX_CLIP_CHAIN), "{}", svg.matches("<clipPath").count());
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

/// Failure: `form_bomb` hangs an export, takes the whole Mac, or leaves half a file under the real name. It must stop at the limit, say so, and leave nothing, and the page after it must still export.
#[test]
fn a_form_bomb_fails_cleanly_with_nothing_left_behind() {
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    let before = folder_entries(&file);
    let (reply, code, out) = export(&file, &[]);
    assert_ne!(code, 0, "{reply}");
    assert!(matches!(reply["error"]["code"].as_str(), Some("export_page_too_heavy" | "export_memory_tight" | "page_too_slow" | "page_crashed_renderer")), "{reply}");
    assert!(!out.exists(), "no file");
    assert_eq!(folder_entries(&file), before, "nothing left behind");
    let state = State::at(Level::Full);
    let next = file.0.with_file_name("next.svg");
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "export", "svg", "--page", "2", "--out", next.to_str().unwrap()]);
    assert_eq!(code, 0, "{reply}");
    std::fs::remove_file(&next).unwrap();
}

/// Failure: an existing file is replaced, a link is followed, or `--out` is something other than an SVG, which could be the PDF itself. Same rules as `render`: only ever a new file.
#[test]
fn only_ever_a_new_file() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let taken = file.0.with_file_name("taken.svg");
    std::fs::write(&taken, b"precious").unwrap();
    let state = State::at(Level::Full);
    let path = file.0.to_str().unwrap();
    let (reply, code) = state.json(&["local", path, "export", "svg", "1", "--out", taken.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert_eq!(reply["error"]["code"], "output_exists", "{reply}");
    assert_eq!(std::fs::read(&taken).unwrap(), b"precious");
    std::fs::remove_file(&taken).unwrap();
    let link = file.0.with_file_name("link.svg");
    let target = file.0.with_file_name("target");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (reply, _) = state.json(&["local", path, "export", "svg", "1", "--out", link.to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "output_exists", "{reply}");
    assert!(!target.exists(), "the link was followed");
    std::fs::remove_file(&link).unwrap();
    let (reply, _) = state.json(&["local", path, "export", "svg", "1", "--out", path]);
    assert_eq!(reply["error"]["code"], "bad_output", "{reply}");
    assert!(Path::new(path).exists());
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

/// Failure: text that draws nothing, such as a line of spaces, turns into a picture of nothing, and the page reads as not all vector when it is.
#[test]
fn what_draws_nothing_is_no_picture() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new("BT /F1 12 Tf 72 700 Td (   ) Tj ET BT /F1 12 Tf 72 680 Td (Words) Tj ET\n")]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
    assert!(!svg.contains("<image"), "{svg}");
}

/// Failure: a soft mask or a blend mode that PDFium can't read reaches the file as plain paint, so a fade-out is a solid block. Either way it's a picture.
#[test]
fn soft_masks_are_pictures_and_plain_opacity_stays_vector() {
    let mask_group = first_extra_object(1);
    let resources = format!("/ExtGState << /M << /SMask << /Type /Mask /S /Luminosity /G {mask_group} 0 R >> >> >>");
    let group = Object::Stream { dict: "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >>".to_string(), data: b"0.5 g 0 0 612 792 re f".to_vec() };
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new("/M gs 1 0 0 rg 100 500 200 100 re f\n")], &resources, vec![group]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(rasterised(&data, "blend_or_mask"), 1, "{data}");
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { channels: 4, pixels, .. }] = &images[..] else { panic!("{images:?}") };
    assert!(pixels.chunks(4).any(|pixel| (100..200).contains(&pixel[3])), "half the mask's strength should be in the picture's alpha");
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new("/A gs 1 0 0 rg 100 500 200 100 re f\n")], "/ExtGState << /A << /ca 0.5 >> >>", vec![]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
    assert!(svg.contains("fill-opacity=\".502\""), "{svg}");
}

/// Failure: a form's opacity applies to each thing in it, or twice, or not at all. A transparency group is one `<g opacity>`, as it is in the file.
#[test]
fn a_transparent_group_is_one_group() {
    let form = first_extra_object(1);
    let resources = format!("/ExtGState << /A << /ca 0.5 >> >> /XObject << /F {form} 0 R >>");
    let dict = "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency >>".to_string();
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new("/A gs /F Do\n")], &resources, vec![Object::Stream { dict, data: b"1 0 0 rg 100 500 200 100 re f 0 1 0 rg 150 450 200 100 re f".to_vec() }]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
    assert_eq!(svg.matches("opacity=\".502\"").count(), 1, "{svg}");
    assert!(svg.contains("#f00") && svg.contains("#0f0"), "{svg}");
}

/// Failure: a page with hundreds of blended things is hundreds of pictures, each a draw of all the page, which is slow and enormous; or past some limit they're simply wrong. Past the limit it's one picture of the whole page, with the text still selectable, and it says so.
#[test]
fn past_the_limit_the_whole_page_is_one_picture() {
    let mut content = String::from("BT /F1 12 Tf 72 720 Td (Selectable words) Tj ET /G gs\n");
    for i in 0..600 {
        content.push_str(&format!("1 0 0 rg {} {} 5 5 re f\n", i % 600, i % 700));
    }
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new(content)], "/ExtGState << /G << /BM /Multiply >> >>", vec![]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(rasterised(&data, "page"), 1, "{data}");
    assert!(svg.contains(">Selectable words</text>"), "the words stay selectable");
    let pictures = embedded_images(svg.as_bytes());
    let Some(Embedded::Png { width, height, channels: 3, .. }) = pictures.last() else { panic!("{}", pictures.len()) };
    assert_eq!((*width, *height), (1224, 1584), "the last is the whole page, at the scale asked");
    assert!(svg.contains("pointer-events=\"none\""), "so the text under it can still be reached");
}

/// Failure: annotations aren't page objects, so a stamp or a form field just isn't in the export, though `render` shows it. They come through as a picture of just them, which says so.
#[test]
fn annotations_are_kept_as_a_picture() {
    let appearance = first_extra_object(1);
    let annot = format!("/Annots [<< /Type /Annot /Subtype /Stamp /Rect [100 500 200 600] /AP << /N {appearance} 0 R >> >>]");
    let page = Page { content: text_lines(&["Words"]), page_extra: annot, media: (612.0, 792.0) };
    let stamp = Object::Stream { dict: "/Type /XObject /Subtype /Form /BBox [0 0 100 100]".to_string(), data: b"1 0 0 rg 0 0 100 100 re f".to_vec() };
    let file = TempPdf::new("doc.pdf", &pages_with(&[page], "", vec![stamp]));
    let (svg, data) = svg_of(&file, &[]);
    assert_eq!(rasterised(&data, "annotations"), 1, "{data}");
    let images = embedded_images(svg.as_bytes());
    let [Embedded::Png { channels: 4, pixels, .. }] = &images[..] else { panic!("{images:?}") };
    assert!(pixels.chunks(4).any(|pixel| pixel == [255, 0, 0, 255]), "the stamp");
    assert!(pixels.chunks(4).any(|pixel| pixel[3] == 0), "and nothing else");
}

/// Failure: a link with no appearance, which nearly every paper has, makes every page read as not all vector. It draws nothing, so nothing's said.
#[test]
fn a_link_is_nothing_to_draw() {
    let page = Page { content: text_lines(&["Words"]), page_extra: "/Annots [<< /Type /Annot /Subtype /Link /Rect [100 500 200 600] >>]".to_string(), media: (612.0, 792.0) };
    let file = TempPdf::new("doc.pdf", &pages(&[page]));
    let (_, data) = svg_of(&file, &[]);
    assert_eq!(data["all_vector"], true, "{data}");
}

/// Failure: an export after a page is moved or turned shows the wrong page, or a turn it doesn't show goes unsaid, as `render` has to say. Page numbers mean the document as edited, and what isn't in the file is said plainly.
#[test]
fn an_edited_tab_is_exported_honestly() {
    let file = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["First page"])), Page::new(text_lines(&["Second page"]))]));
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

/// Failure: a person sees numbers but not what became a picture and why, or a page that had to be drawn reads as if it's vector. Plain output says each kind in words.
#[test]
fn plain_output_says_whats_a_picture_and_why() {
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 200 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>";
    let file = TempPdf::new("doc.pdf", &pages_with(&[Page::new("q 100 500 200 100 re W n 1 0 0 1 100 0 cm /Sh sh Q\n")], &format!("/Shading << /Sh {shading} >>"), vec![]));
    let out = file.0.with_file_name("said.svg");
    let state = State::at(Level::Full);
    let (said, _, code) = state.text(&["local", file.0.to_str().unwrap(), "export", "svg", "1", "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(said.contains("As a picture at 2x") && said.contains("gradient"), "{said}");
    std::fs::remove_file(&out).unwrap();
    let plain = TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let out = plain.0.with_file_name("said.svg");
    let (said, _, _) = state.text(&["local", plain.0.to_str().unwrap(), "export", "svg", "1", "--out", out.to_str().unwrap()]);
    assert!(said.contains("Nothing had to become a picture"), "{said}");
    std::fs::remove_file(&out).unwrap();
}
