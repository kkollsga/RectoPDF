//! The page digest's tests: its definition, by bytes built by hand from the module doc; what it leaves out and what it doesn't; every cap at its limit
//! and its limit plus one; hostile pages; and the golden vectors, on made files, which are `recto-page/1` as bytes.

use super::*;
use recto_testpdf::{Object as Obj, build};
use std::io::Write;
use std::sync::Arc;

std::thread_local! {
    /// Bytes decrypted whole for the digest on this thread: what a file anyone may open costs ([`super::take_stream`]).
    pub(super) static DECRYPTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Streams' descriptors resolved for images and forms on this thread: how many times a page's drawn objects were read ([`super::xobject_record`]).
    pub(super) static DESCRIPTORS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// References a read of a stream's dictionary followed on this thread, not found already followed: how many times an object it points at was parsed ([`super::Refs`]).
    pub(super) static DEREFERENCES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Dictionaries resolved by reference (a resource dictionary, an `/XObject` table) on this thread: how many copies a page makes of what forms share.
    pub(super) static DICTIONARIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn flate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = flate2::write::ZlibEncoder::new(&mut out, flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap();
    out
}

/// A one-page file: the page dictionary's extra entries, its resources, its `/Contents` value, and the objects after the page (numbered from 4).
fn one_page(page_extra: &str, resources: &str, contents: &str, objects: Vec<Obj>) -> Vec<u8> {
    let mut all = vec![
        Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Obj::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} /Resources << {resources} >> {page_extra} >>"
        )),
    ];
    all.extend(objects);
    build(&all)
}

fn reads_of(bytes: &[u8], caps: &Caps) -> Vec<PageRead> {
    let reader = open_bytes_for_identity(Arc::from(bytes)).expect("it opens");
    let map = reader.page_map().expect("its pages");
    map.iter().map(|entry| read_page(&reader, entry, caps)).collect()
}

/// The page's digest under `caps`, or `None` where it has no identity.
fn digest_under(bytes: &[u8], caps: &Caps) -> Option<[u8; 32]> {
    reads_of(bytes, caps)[0].identity.map(|id| id.digest)
}

fn id_of(bytes: &[u8]) -> PageIdentity {
    reads_of(bytes, &Caps::STANDARD)[0].identity.expect("an identity")
}

fn digest_of(bytes: &[u8]) -> [u8; 32] {
    id_of(bytes).digest
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

const CONTENT: &[u8] = b"BT /F1 12 Tf 72 712 Td (One page) Tj ET q /Im1 Do Q";
const IMAGE: &[u8] = b"\xff\xd8 not really a JPEG, only its bytes \xff\xd9";
const JPEG_DICT: &str = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode";

fn stream(dict: &str, data: &[u8]) -> Obj {
    Obj::Stream {
        dict: dict.into(),
        data: data.to_vec(),
    }
}

/// A page with flate content drawing a JPEG-labelled image `/Im1`.
fn sample(page_extra: &str) -> Vec<u8> {
    one_page(
        page_extra,
        "/XObject << /Im1 5 0 R >>",
        "4 0 R",
        vec![stream("/Filter /FlateDecode", &flate(CONTENT)), stream(JPEG_DICT, IMAGE)],
    )
}

/// The bytes of one record of an image or form, as the module doc lists them: its name, kind, length and hash.
fn xobject_bytes(name: &[u8], kind: u8, data: &[u8]) -> Vec<u8> {
    let mut out = (name.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(name);
    out.push(kind);
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&sha(data));
    out
}

/// A content record's bytes.
fn content_bytes(kind: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&sha(data));
    out
}

/// Failure caught: the code hashes something other than what its definition says, so a digest made from the module doc (by the locator's later code,
/// or by a reader of the doc) wouldn't match. The bytes are built here by hand, step by step as the doc lists them: a page drawing an image, and a page
/// drawing a form that draws an image and a name it doesn't have.
#[test]
fn the_digest_is_what_its_definition_says() {
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, CONTENT));
    expected.extend_from_slice(b"X");
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(xobject_bytes(b"Im1", 1, IMAGE));
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(b"E");
    let id = id_of(&sample(""));
    assert_eq!(hex(&id.digest), hex(&sha(&expected)));
    assert_eq!(id.object, (3, 0));

    let form_content = b"/Im1 Do /Gone Do";
    let page = one_page(
        "",
        "/XObject << /Fm1 5 0 R >>",
        "4 0 R",
        vec![
            stream("", b"/Fm1 Do"),
            stream("/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Im1 6 0 R >> >>", form_content),
            stream(JPEG_DICT, IMAGE),
        ],
    );
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, b"/Fm1 Do"));
    expected.extend_from_slice(b"X");
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(xobject_bytes(b"Fm1", 0, form_content));
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend(xobject_bytes(b"Im1", 1, IMAGE));
    expected.extend_from_slice(&4u32.to_le_bytes());
    expected.extend_from_slice(b"Gone\x02");
    expected.extend_from_slice(b"E");
    assert_eq!(hex(&digest_of(&page)), hex(&sha(&expected)));
}

/// A form whose flate content draws `/A Do` and then fails: its Adler-32 cut off, so the decoder gives the bytes first and the error after.
fn page_with_a_failing_form() -> (Vec<u8>, Vec<u8>) {
    let mut stored = flate(b"/A Do ");
    stored.truncate(stored.len() - 4);
    let page = one_page(
        "",
        "/XObject << /F 5 0 R /A 6 0 R >>",
        "4 0 R",
        vec![
            stream("", b"/F Do"),
            stream("/Subtype /Form /BBox [0 0 1 1] /Filter /FlateDecode", &stored),
            stream(JPEG_DICT, IMAGE),
        ],
    );
    (page, stored)
}

/// Failure caught (second review, F1): a form whose content fails to decode keeps the names its first part gave `Do` as its children, so its digest
/// depends on how far a decoder got before its error, against the definition ("a form whose content wasn't decoded has no children"). A form drawing
/// `/A Do` and then failing is kind 3, as stored, with no children.
#[test]
fn a_form_that_fails_to_decode_has_no_children() {
    let (page, stored) = page_with_a_failing_form();
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, b"/F Do"));
    expected.extend_from_slice(b"X");
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(xobject_bytes(b"F", 3, &stored));
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(b"E");
    assert_eq!(hex(&digest_of(&page)), hex(&sha(&expected)));
}

/// Failure caught (review): a form drawing an image and a form drawing nothing beside the same image hash alike, as nothing said where a form's
/// records end. With every section and form framed by its count, they don't.
#[test]
fn a_forms_children_are_framed_by_their_count() {
    let form = |content: &[u8], page: &[u8]| {
        digest_of(&one_page(
            "",
            "/XObject << /A 5 0 R /B 6 0 R >>",
            "4 0 R",
            vec![stream("", page), stream("/Subtype /Form /BBox [0 0 1 1]", content), stream(JPEG_DICT, IMAGE)],
        ))
    };
    // The form's content is the same bytes either way, so only the framing tells them apart.
    assert_ne!(form(b"/B Do", b"/A Do"), form(b"/B Do", b"/A Do /B Do"));
}

/// Failure caught (review): every image of the page's effective resources is hashed, drawn or not, so documents sharing one `/Resources` at the page
/// tree's top put every image in every page's digest (any image changed changes every page). Only what the content draws counts, in the order it's
/// first drawn, through the resources the page inherits.
#[test]
fn only_what_the_content_draws_is_hashed() {
    let page = |content: &[u8], second: &[u8]| {
        digest_of(&one_page(
            "",
            "/XObject << /Im1 5 0 R /Im2 6 0 R >>",
            "4 0 R",
            vec![stream("", content), stream(JPEG_DICT, IMAGE), stream(JPEG_DICT, second)],
        ))
    };
    assert_eq!(page(b"/Im1 Do", b"one"), page(b"/Im1 Do", b"two"), "an image not drawn");
    assert_ne!(page(b"/Im2 Do", b"one"), page(b"/Im2 Do", b"two"), "an image drawn");
    assert_ne!(page(b"/Im1 Do /Im2 Do", b"x"), page(b"/Im2 Do /Im1 Do", b"x"), "the order drawn");
    // Inherited: resources at the page tree's top, shared by two pages drawing one image each.
    let tree = build(&[
        Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Obj::Dict("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << /XObject << /A 7 0 R /B 8 0 R >> >> >>".into()),
        Obj::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Contents 5 0 R >>".into()),
        Obj::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Contents 6 0 R >>".into()),
        stream("", b"/A Do"),
        stream("", b"/B Do"),
        stream(JPEG_DICT, b"a"),
        stream(JPEG_DICT, b"b"),
    ]);
    let reads = reads_of(&tree, &Caps::STANDARD);
    let alone = |content: &[u8], data: &[u8], name: &str| {
        digest_of(&one_page(
            "",
            &format!("/XObject << /{name} 5 0 R >>"),
            "4 0 R",
            vec![stream("", content), stream(JPEG_DICT, data)],
        ))
    };
    assert_eq!(reads[0].identity.unwrap().digest, alone(b"/A Do", b"a", "A"));
    assert_eq!(reads[1].identity.unwrap().digest, alone(b"/B Do", b"b", "B"));
}

/// Failure caught: a view of the same page (a turn, a crop, another media box), a highlight added (`/Annots`), another font, or the page under another
/// object number changes its digest, so edits on it would be set aside after a mere re-save or our own turn (research §3: these are what other
/// programs change). The frame tag is what says a turn or crop did change, and its low two bits which turn the page's own `/Rotate` is.
#[test]
fn what_a_view_changes_leaves_the_digest_alone_and_the_frame_says_it() {
    let plain = id_of(&sample(""));
    for extra in [
        "/Rotate 90",
        "/CropBox [10 10 500 700]",
        "/Annots [<< /Type /Annot /Subtype /Highlight /Rect [0 0 10 10] >>]",
    ] {
        let other = id_of(&sample(extra));
        assert_eq!(hex(&other.digest), hex(&plain.digest), "{extra}");
        assert_eq!(other.frame == plain.frame, extra.starts_with("/Annots"), "the frame tag, for {extra}");
    }
    // The tag says the page's own turns, so a turn is told from a crop (second review: slice 2 needs which of the two another program changed).
    assert_eq!(frame_turns(plain.frame), 0);
    for (rotate, turns) in [
        ("/Rotate 90", 1),
        ("/Rotate 180", 2),
        ("/Rotate 270", 3),
        ("/Rotate -90", 3),
        ("/Rotate 450", 1),
    ] {
        assert_eq!(frame_turns(id_of(&sample(rotate)).frame), turns, "{rotate}");
    }
    let cropped = id_of(&sample("/CropBox [10 10 500 700]")).frame;
    assert_eq!(frame_turns(cropped), 0, "a crop keeps the turns");
    assert_ne!(cropped >> 2, plain.frame >> 2, "and changes the rest of the tag");
    // Another media box, written in as many bytes, so the file's offsets hold.
    let wider = id_of(&sample("").replace_all(b"612 792", b"700 792"));
    assert_eq!(hex(&wider.digest), hex(&plain.digest));
    assert_ne!(wider.frame, plain.frame);
    // Another font, and the page under another number: padding objects ahead of it.
    let renumbered = build(&[
        Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Obj::Dict("<< /Type /Pages /Kids [5 0 R] /Count 1 >>".into()),
        Obj::Dict("<< /Unused true >>".into()),
        Obj::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".into()),
        Obj::Dict(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 7 0 R >> >> >>".into(),
        ),
        stream("/Filter /FlateDecode", &flate(CONTENT)),
        stream(JPEG_DICT, IMAGE),
    ]);
    let moved = id_of(&renumbered);
    assert_eq!(hex(&moved.digest), hex(&plain.digest));
    assert_eq!(moved.object, (5, 0));
}

trait ReplaceAll {
    fn replace_all(self, from: &[u8], to: &[u8]) -> Vec<u8>;
}

impl ReplaceAll for Vec<u8> {
    fn replace_all(self, from: &[u8], to: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        let mut at = 0;
        while at < self.len() {
            if self[at..].starts_with(from) {
                out.extend_from_slice(to);
                at += from.len();
            } else {
                out.push(self[at]);
                at += 1;
            }
        }
        out
    }
}

/// PNG rows of `row` bytes each (one pixel a byte), each filtered by the next of the five filters in turn, as an encoder would write them.
fn png_rows(data: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut previous = vec![0u8; row];
    for (at, current) in data.chunks(row).enumerate() {
        let filter = (at % 5) as u8;
        out.push(filter);
        for i in 0..current.len() {
            let left = if i >= 1 { current[i - 1] } else { 0 };
            let up = previous[i];
            let up_left = if i >= 1 { previous[i - 1] } else { 0 };
            let predicted = match filter {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                _ => decode_paeth(left, up, up_left),
            };
            out.push(current[i].wrapping_sub(predicted));
        }
        previous[..current.len()].copy_from_slice(current);
    }
    out
}

fn decode_paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(up_left));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        up
    } else {
        up_left
    }
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        let mut value = u32::from_be_bytes(word);
        let mut digits = [0u8; 5];
        for digit in digits.iter_mut().rev() {
            *digit = (value % 85) as u8 + b'!';
            value /= 85;
        }
        out.extend_from_slice(&digits[..chunk.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

/// Literal runs, and the last three bytes (which are one byte three times) as a repeated run.
fn run_length(data: &[u8]) -> Vec<u8> {
    let (literal, repeated) = data.split_at(data.len() - 3);
    assert!(repeated.iter().all(|byte| *byte == repeated[0]));
    let mut out = Vec::new();
    for chunk in literal.chunks(20) {
        out.push((chunk.len() - 1) as u8);
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&[254, repeated[0], 128]);
    out
}

/// Each general filter and both predictors, as `(dict, data)` streams of the same decoded bytes `data_of` (for TIFF, the bytes `CONTENT` with three
/// spaces after it, a whole number of 18-byte rows of 2 colours).
fn every_encoding(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let lzw = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).encode(data).unwrap();
    let lzw_late = weezl::encode::Encoder::new(weezl::BitOrder::Msb, 8).encode(data).unwrap();
    let hexed: String = data.iter().map(|byte| format!("{byte:02X} ")).collect::<String>() + ">";
    let tiff: Vec<u8> = data
        .chunks(18)
        .flat_map(|row| (0..row.len()).map(move |at| if at < 2 { row[at] } else { row[at].wrapping_sub(row[at - 2]) }))
        .collect();
    vec![
        (String::new(), data.to_vec()),
        ("/Filter /FlateDecode".into(), flate(data)),
        ("/Filter /Fl".into(), flate(data)),
        ("/Filter /LZWDecode".into(), lzw),
        ("/Filter /LZWDecode /DecodeParms << /EarlyChange 0 >>".into(), lzw_late),
        ("/Filter /ASCIIHexDecode".into(), hexed.into_bytes()),
        ("/Filter /ASCII85Decode".into(), ascii85(data)),
        ("/Filter /RunLengthDecode".into(), run_length(data)),
        (
            "/Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 9 >>".into(),
            flate(&png_rows(data, 9)),
        ),
        ("/Filter /FlateDecode /DecodeParms << /Predictor 2 /Colors 2 /Columns 9 >>".into(), flate(&tiff)),
        (
            "/Filter [/ASCIIHexDecode /FlateDecode] /DecodeParms [null << /Predictor 12 /Columns 9 >>]".into(),
            flate(&png_rows(data, 9))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .into_bytes(),
        ),
    ]
}

/// The content every encoding test decodes to: 54 bytes, six rows of 9 and three of 18.
const ROWS: &[u8] = b"BT /F1 12 Tf 72 712 Td (One page) Tj ET q /Im1 Do Q   ";

/// Failure caught (research §5.2, data table C): a re-encode that keeps the bytes changes the digest (qpdf wrote `/Filter [/FlateDecode]` for
/// `/Filter /FlateDecode` and re-compressed, which changed a page's digest under a mere rotation), or a general filter or predictor decodes wrongly, so a
/// page takes another's digest. Every general filter, both predictors with every PNG row filter, and a chain: one digest, the plain bytes'. And an image
/// whose general filter is written two ways keeps its digest (kind 0, decoded).
#[test]
fn the_same_bytes_encoded_another_way_are_the_same_page() {
    assert_eq!(ROWS.len(), 54);
    let with = |dict: &str, data: Vec<u8>| digest_of(&one_page("", "", "4 0 R", vec![Obj::Stream { dict: dict.into(), data }]));
    let plain = with("", ROWS.to_vec());
    for (dict, data) in every_encoding(ROWS) {
        assert_eq!(hex(&with(&dict, data)), hex(&plain), "{dict}");
    }
    let image = |filter: &str, data: Vec<u8>| {
        digest_of(&one_page(
            "",
            "/XObject << /Im1 5 0 R >>",
            "4 0 R",
            vec![
                stream("", b"/Im1 Do"),
                Obj::Stream {
                    dict: format!("/Type /XObject /Subtype /Image /Width 3 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB {filter}"),
                    data,
                },
            ],
        ))
    };
    let pixels = [1u8, 2, 3, 4, 5, 6, 7, 8, 9];
    assert_eq!(image("/Filter /FlateDecode", flate(&pixels)), image("", pixels.to_vec()));
    assert_eq!(image("/Filter [/FlateDecode]", flate(&pixels)), image("", pixels.to_vec()));
}

/// Failure caught: two pages that differ only in an image, or in an image inside a form, share a digest, so an edit could follow the wrong one of
/// them (a form wrapping each page is what `\includepdf` makes).
#[test]
fn a_different_image_is_a_different_page_inside_a_form_too() {
    let direct = |pixels: &[u8]| {
        digest_of(&one_page(
            "",
            "/XObject << /Im1 5 0 R >>",
            "4 0 R",
            vec![stream("", b"q 100 0 0 100 0 0 cm /Im1 Do Q"), stream(JPEG_DICT, pixels)],
        ))
    };
    assert_ne!(direct(b"\xff\xd8 one \xff\xd9"), direct(b"\xff\xd8 two \xff\xd9"));
    let wrapped = |pixels: &[u8]| {
        digest_of(&one_page(
            "",
            "/XObject << /Fm1 5 0 R >>",
            "4 0 R",
            vec![
                stream("", b"/Fm1 Do"),
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Im1 6 0 R >> >>",
                    b"q 100 0 0 100 0 0 cm /Im1 Do Q",
                ),
                stream(JPEG_DICT, pixels),
            ],
        ))
    };
    assert_ne!(wrapped(b"\xff\xd8 one \xff\xd9"), wrapped(b"\xff\xd8 two \xff\xd9"));
}

/// Two content streams: the first `first`, the second `second`.
fn two_streams(first: &[u8], second: &[u8]) -> Vec<u8> {
    one_page("", "", "[4 0 R 5 0 R]", vec![stream("", first), stream("", second)])
}

/// Failure caught (review): a page past a cap is hashed to where the cap fell, so its digest depends on where that was and is reproducible only by this
/// build; or the cap isn't where it's said to be. At the content's byte cap the page has an identity, and one byte past it none, on the decoded path
/// and on the path of a stream hashed as stored; at the standard cap too.
#[test]
fn a_page_past_the_content_cap_has_no_identity() {
    let caps = Caps {
        content_bytes: 10,
        ..Caps::STANDARD
    };
    assert!(digest_under(&two_streams(b"01234", b"56789"), &caps).is_some(), "at the cap");
    assert_eq!(digest_under(&two_streams(b"01234", b"56789A"), &caps), None, "one past it");
    let broken = |data: &[u8]| digest_under(&one_page("", "", "4 0 R", vec![stream("/Filter /FlateDecode", data)]), &caps);
    assert!(broken(b"not zlib!!").is_some(), "hashed as stored, at the cap");
    assert_eq!(broken(b"not zlib!!!"), None, "hashed as stored, one past it");
    let big = |len: usize| digest_under(&one_page("", "", "4 0 R", vec![stream("", &vec![b' '; len])]), &Caps::STANDARD);
    assert!(big(16 << 20).is_some());
    assert_eq!(big((16 << 20) + 1), None);
}

/// Failure caught (second review, F3): a decoder that decodes ahead of what's asked meets a bad byte past the cap's byte and fails the stream (hashed
/// as stored, an identity) where a decoder that gives no more than asked reaches the cap (no identity), so the digest depends on how far a decoder
/// reads ahead, not on the file. With the content's cap at 8 bytes: ASCII85 of 12 bytes, then a byte that isn't a digit, reaches the cap (as a lazy
/// decode does); with a cap the stream fits in, the same stream fails and is hashed as stored.
#[test]
fn a_bad_byte_past_the_cap_reaches_the_cap_as_a_lazy_decode_does() {
    let page = one_page("", "", "4 0 R", vec![stream("/Filter /ASCII85Decode", b"zz!!!!!x")]);
    let small = Caps {
        content_bytes: 8,
        ..Caps::STANDARD
    };
    assert_eq!(digest_under(&page, &small), None, "the cap's byte is reached before the bad byte");
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(1, b"zz!!!!!x"));
    expected.extend_from_slice(b"X");
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.extend_from_slice(b"E");
    assert_eq!(digest_under(&page, &Caps::STANDARD).map(|digest| hex(&digest)), Some(hex(&sha(&expected))));
    // A predictor's unit is its row, and RunLength's its run (the definition, [`decode`]): a bad row or a run cut short that holds the cap's byte
    // fails the stream, which is then hashed as stored, within the cap.
    let as_stored = |dict: &str, data: &[u8], cap: u64| {
        let page = one_page("", "", "4 0 R", vec![stream(dict, data)]);
        let mut expected = b"recto-page/1\0C".to_vec();
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend(content_bytes(1, data));
        expected.extend_from_slice(b"X");
        expected.extend_from_slice(&0u32.to_le_bytes());
        expected.extend_from_slice(b"E");
        let caps = Caps {
            content_bytes: cap,
            ..Caps::STANDARD
        };
        assert_eq!(digest_under(&page, &caps).map(|digest| hex(&digest)), Some(hex(&sha(&expected))), "{dict}");
    };
    let mut rows = vec![0u8; 42];
    rows.extend_from_slice(&[9; 21]);
    as_stored("/Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 20 >>", &flate(&rows), 50);
    as_stored("/Filter /RunLengthDecode", &[129, b'a', 9, b'x', b'x'], 129);
}

/// Failure caught: the stream count isn't capped where the definition says, so a page of 100,000 streams costs what it likes, or a page past it is
/// hashed by part of itself. At 1,024 streams the page has an identity (and the last counts); one more and it has none.
#[test]
fn the_stream_count_is_capped() {
    let streams = |count: usize, last: &[u8]| {
        let refs: String = (0..count).map(|at| format!("{} 0 R ", 4 + at)).collect();
        let mut objects: Vec<Obj> = (0..count).map(|_| stream("", b"0 0 m")).collect();
        if let Some(Obj::Stream { data, .. }) = objects.last_mut() {
            *data = last.to_vec();
        }
        digest_under(&one_page("", "", &format!("[{refs}]"), objects), &Caps::STANDARD)
    };
    assert_ne!(streams(1024, b"one").unwrap(), streams(1024, b"two").unwrap(), "the 1,024th counts");
    assert_eq!(streams(1025, b"one"), None, "1,025 streams");
}

/// A page drawing `count` distinct images `/I000...`, the last one's bytes `last`.
fn images_page(count: usize, last: &[u8]) -> Vec<u8> {
    let names: String = (0..count).map(|at| format!("/I{at:03} {} 0 R ", 5 + at)).collect();
    let drawn: String = (0..count).map(|at| format!("/I{at:03} Do ")).collect();
    let mut objects = vec![stream("", drawn.as_bytes())];
    objects.extend((0..count).map(|at| stream(JPEG_DICT, &if at + 1 == count { last.to_vec() } else { (at as u32).to_le_bytes().to_vec() })));
    one_page("", &format!("/XObject << {names} >>"), "4 0 R", objects)
}

/// A page drawing one form `/F` whose content draws `children` distinct images.
fn form_page(children: usize) -> Vec<u8> {
    let names: String = (0..children).map(|at| format!("/I{at:03} {} 0 R ", 6 + at)).collect();
    let drawn: String = (0..children).map(|at| format!("/I{at:03} Do ")).collect();
    let mut objects = vec![
        stream("", b"/F Do"),
        Obj::Stream {
            dict: format!("/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << {names} >> >>"),
            data: drawn.into_bytes(),
        },
    ];
    objects.extend((0..children).map(|at| stream(JPEG_DICT, &(at as u32).to_le_bytes())));
    one_page("", "/XObject << /F 5 0 R >>", "4 0 R", objects)
}

/// Failure caught: the records cap isn't counted exactly, or a form's children aren't counted in it, so a page draws what it likes or is refused one
/// short. With the cap set to 4 (the cap's value isn't what's tested here, its exactness is): 4 images drawn have an identity and the last counts, 5
/// have none; a form with 3 children is 4 records, and 4 children are one too many. The scan's own name cap is the same number, so the 5th name of a
/// form is refused there too.
#[test]
fn the_record_count_is_capped_at_its_limit_plus_one() {
    let caps = Caps { xobjects: 4, ..Caps::STANDARD };
    assert_ne!(
        digest_under(&images_page(4, b"one"), &caps).unwrap(),
        digest_under(&images_page(4, b"two"), &caps).unwrap(),
        "the 4th drawn counts"
    );
    assert_eq!(digest_under(&images_page(5, b"one"), &caps), None, "5 drawn");
    assert!(digest_under(&form_page(3), &caps).is_some(), "a form and 3 children");
    assert_eq!(digest_under(&form_page(4), &caps), None, "a form and 4 children");
    assert_eq!(digest_under(&form_page(5), &caps), None, "a form of 5 names overflows its own scan");
}

/// Failure caught: a page of more than 256 distinct images, as a scanned report cut into strips is, has no identity, so a page of it that moved can't be
/// found again (NASA report pages 127, 137, 156, 197 of bench100: `incomplete`). Under the standard caps 300 images drawn have one, and the last counts.
#[test]
fn a_page_of_many_images_has_an_identity() {
    let one = digest_under(&images_page(300, b"one"), &Caps::STANDARD);
    assert!(one.is_some(), "300 images drawn");
    assert_ne!(one, digest_under(&images_page(300, b"two"), &Caps::STANDARD), "the 300th counts");
}

/// Failure caught: the cap on forms the page draws that draw something isn't held exactly, so a page of thousands of such forms (each read and scanned)
/// costs what it likes. With the cap set to 2: two forms each drawing an image have an identity, a third is one too many.
#[test]
fn the_forms_a_page_draws_are_capped_at_their_limit_plus_one() {
    let forms = |count: usize| {
        let names: String = (0..count).map(|at| format!("/F{at} {} 0 R ", 5 + at)).collect();
        let drawn: String = (0..count).map(|at| format!("/F{at} Do ")).collect();
        let mut objects = vec![stream("", drawn.as_bytes())];
        objects.extend((0..count).map(|at| Obj::Stream {
            dict: format!("/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /I {} 0 R >> >>", 5 + count + at),
            data: b"/I Do".to_vec(),
        }));
        objects.extend((0..count).map(|at| stream(JPEG_DICT, &(at as u32).to_le_bytes())));
        one_page("", &format!("/XObject << {names} >>"), "4 0 R", objects)
    };
    let caps = Caps {
        forms_drawing: 2,
        ..Caps::STANDARD
    };
    assert!(digest_under(&forms(2), &caps).is_some(), "two forms");
    assert_eq!(digest_under(&forms(3), &caps), None, "three forms");
    assert!(digest_under(&forms(3), &Caps::STANDARD).is_some(), "three are nothing under the standard cap");
}

/// Failure caught: the page's own caps aren't held, so a page of many large images hashes a gigabyte, or a stream that decodes to little from very
/// much is read to its end. With the caps set small, each at its limit and one past it: an image's own cap, the page's byte cap, and the read cap
/// (charged each stream's stored length before it's read).
#[test]
fn the_page_caps_hold() {
    let page = |first: &[u8], second: &[u8], caps: &Caps| {
        digest_under(
            &one_page(
                "",
                "/XObject << /A 5 0 R /B 6 0 R >>",
                "4 0 R",
                vec![
                    stream("", b"/A Do /B Do"),
                    stream("/Subtype /Image /Filter /DCTDecode", first),
                    stream("/Subtype /Image /Filter /DCTDecode", second),
                ],
            ),
            caps,
        )
    };
    // An image past its own cap has an identity (kind 4); a form past it has none.
    let image_cap = Caps {
        xobject_bytes: 8,
        ..Caps::STANDARD
    };
    assert!(page(b"12345678", b"x", &image_cap).is_some());
    assert!(page(b"123456789", b"x", &image_cap).is_some());
    let form = |content: &[u8]| {
        digest_under(
            &one_page(
                "",
                "/XObject << /F 5 0 R >>",
                "4 0 R",
                vec![stream("", b"/F Do"), stream("/Subtype /Form /BBox [0 0 1 1]", content)],
            ),
            &image_cap,
        )
    };
    assert!(form(b"12345678").is_some());
    assert_eq!(form(b"123456789"), None);
    // 11 bytes of content, 8 of the first image and 1 of the second make 20.
    let page_cap = Caps {
        page_bytes: 20,
        ..Caps::STANDARD
    };
    assert!(page(b"12345678", b"x", &page_cap).is_some());
    assert_eq!(page(b"12345678", b"xy", &page_cap), None);
    // Stored: 11 + 8 + 1 = 20 bytes, each stream charged its length as it's opened.
    let read_cap = Caps {
        page_read: 20,
        ..Caps::STANDARD
    };
    assert!(page(b"12345678", b"x", &read_cap).is_some());
    assert_eq!(page(b"12345678", b"xy", &read_cap), None);
    let reads = reads_of(
        &one_page("", "", "4 0 R", vec![stream("/Filter /FlateDecode", &flate(&[0u8; 4096]))]),
        &Caps {
            page_read: 16,
            ..Caps::STANDARD
        },
    );
    assert_eq!(reads[0].identity, None, "a stream past the read cap is never opened");
}

/// An image of `data` flated, drawn, with `/Width` `width`.
fn page_with_image(data: &[u8], width: u32, filter: bool) -> Vec<u8> {
    let (dict, data) = if filter {
        (
            format!("/Subtype /Image /Width {width} /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode"),
            flate(data),
        )
    } else {
        (
            format!("/Subtype /Image /Width {width} /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray"),
            data.to_vec(),
        )
    };
    one_page("", "/XObject << /Im 5 0 R >>", "4 0 R", vec![stream("", b"/Im Do"), Obj::Stream { dict, data }])
}

/// Failure caught (the coordinator's ruling, 2026-11-15): a page with an image past its cap (a scan, a map) has no identity, or has one by its first
/// 16 MiB alone, so two scans sharing a white top margin are taken for one page. Held with the cap set small: past it, an image is its prefix, its
/// stored length and what it says of itself; and the honest limit: the same prefix, length and geometry, different after, hash alike.
#[test]
fn an_image_past_its_cap_is_its_prefix_its_length_and_its_geometry() {
    let caps = Caps {
        xobject_bytes: 8,
        ..Caps::STANDARD
    };
    let with = |data: &[u8], width: u32, filter: bool| digest_under(&page_with_image(data, width, filter), &caps).expect("an identity");
    let base = with(b"12345678AAAA", 12, false);
    assert_ne!(base, with(b"12345678AAAAA", 12, false), "another stored length");
    assert_ne!(base, with(b"12345678AAAA", 13, false), "another width");
    assert_ne!(base, with(b"12345678AAAA", 12, true), "another filter");
    assert_ne!(base, with(b"12345679AAAA", 12, false), "another prefix");
    assert_eq!(base, with(b"12345678BBBB", 12, false), "the honest limit: only the tail differs");
}

/// Failure caught: [`PageRead::bytes`] isn't what the page cost, so a job reading pages to a byte budget reads far past it. A page of 11 stored bytes of
/// content and an 8-byte image costs 38: each stream's stored length and each byte hashed.
#[test]
fn what_a_page_costs_is_said() {
    let read = reads_of(
        &one_page(
            "",
            "/XObject << /A 5 0 R >>",
            "4 0 R",
            vec![stream("", b"/A Do /B Do"), stream(JPEG_DICT, b"12345678")],
        ),
        &Caps::STANDARD,
    )[0];
    assert!(read.identity.is_some());
    assert_eq!(read.bytes, 2 * (11 + 8));
}

/// Failure caught: a hostile page panics the identity thread, or gives a different digest each time it's read (so a page would never match itself).
/// Broken references, a `/Contents` that isn't a stream, an image that isn't a reference, a filter that isn't a name, bad predictor parameters, a
/// form drawing itself, and a page with nothing at all.
#[test]
fn a_hostile_page_has_a_steady_digest_and_panics_nothing() {
    let cases: Vec<Vec<u8>> = vec![
        one_page("", "", "99 0 R", Vec::new()),
        one_page("", "", "(a string)", Vec::new()),
        one_page("", "", "[4 0 R 99 0 R (x) [4 0 R]]", vec![Obj::Dict("<< /Not a stream >>".into())]),
        one_page(
            "",
            "/XObject << /A << /Direct true >> /B 99 0 R /C 4 0 R >>",
            "4 0 R",
            vec![stream("/Filter 7", b"/A Do /B Do /C Do")],
        ),
        one_page(
            "",
            "",
            "4 0 R",
            vec![stream(
                "/Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 99999999999 /Colors -1 >>",
                &flate(b"abc"),
            )],
        ),
        one_page(
            "",
            "/XObject << /F 4 0 R >>",
            "4 0 R",
            vec![stream("/Subtype /Form /Resources << /XObject << /F 4 0 R >> >>", b"/F Do")],
        ),
        build(&[
            Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            Obj::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
        ]),
    ];
    for (at, bytes) in cases.iter().enumerate() {
        let first = reads_of(bytes, &Caps::STANDARD);
        let again = reads_of(bytes, &Caps::STANDARD);
        assert_eq!(first, again, "case {at}");
    }
}

/// `plain`, encrypted with an empty password, as a file anyone may open is.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts; never over a user's file.
fn encrypted(plain: &[u8]) -> Vec<u8> {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let mut document = lopdf::Document::load_mem(plain).unwrap();
    document.trailer.set(
        "ID",
        lopdf::Object::Array(vec![
            lopdf::Object::string_literal("0123456789abcdef"),
            lopdf::Object::string_literal("fedcba9876543210"),
        ]),
    );
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: Permissions::all(),
    })
    .unwrap();
    document.encrypt(&state).unwrap();
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    out
}

/// Failure caught: the identity of a page in a file anyone may open (encrypted with an empty password, as many government reports are) is of its
/// stored, encrypted bytes, which a re-save with a fresh key changes, rather than of its content. It's the same as the plain file's.
#[test]
fn a_file_anyone_may_open_is_identified_by_its_content() {
    let plain = sample("");
    let file = encrypted(&plain);
    assert_eq!(hex(&id_of(&file).digest), hex(&id_of(&plain).digest));
    let reader = open_bytes_for_identity(Arc::from(file.as_slice())).unwrap();
    let ids = document_ids(&reader).expect("it has an /ID pair");
    assert_ne!(ids.first, ids.second);
}

/// Failure caught (review S1, rules 1 and 6): in a file anyone may open, a stream's bytes come only whole and decrypted, and the page's read cap was
/// consulted only as the decrypted bytes were pulled through a decoder, after the whole stream had been decrypted; a stream that decodes to nothing
/// pulls almost none, so a `/Contents` of 1,024 references to one 16 MiB stream decrypted 16 GiB (twice, with the retry as stored) for one page. With
/// the read cap set small: a `/Contents` of 40 references to one 64 KiB stream that decodes to nothing decrypts no more than the cap.
#[test]
fn an_encrypted_stream_is_charged_to_the_read_cap_before_it_is_decrypted() {
    let mut data = flate(b"");
    data.extend(std::iter::repeat_n(b' ', 64 << 10));
    let refs: String = (0..40).map(|_| "4 0 R ").collect();
    let file = encrypted(&one_page("", "", &format!("[{refs}]"), vec![stream("/Filter /FlateDecode", &data)]));
    let caps = Caps {
        page_read: 256 << 10,
        ..Caps::STANDARD
    };
    DECRYPTED.with(|bytes| bytes.set(0));
    let _ = reads_of(&file, &caps);
    let decrypted = DECRYPTED.with(std::cell::Cell::get);
    assert!(decrypted <= caps.page_read, "decrypted {decrypted} bytes for a read cap of {}", caps.page_read);
    // And one stream past the size such a file's streams are decrypted to is never decrypted: the page has no identity.
    DECRYPTED.with(|bytes| bytes.set(0));
    let big = encrypted(&one_page("", "", "4 0 R", vec![stream("", &vec![b' '; (8 << 20) + 1])]));
    assert_eq!(reads_of(&big, &Caps::STANDARD)[0].identity, None);
    assert_eq!(DECRYPTED.with(std::cell::Cell::get), 0);
}

/// The golden vectors' pages, by what each holds.
fn golden_pages() -> Vec<(&'static str, Vec<u8>)> {
    let mut pages = vec![("a stream drawing an image", sample(""))];
    pages.push(("an empty stream", one_page("", "", "4 0 R", vec![stream("", b"")])));
    pages.push((
        "a page with nothing",
        build(&[
            Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            Obj::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
        ]),
    ));
    // An array of every encoding, a stream hashed as stored, and an entry that isn't one.
    let encodings = every_encoding(ROWS);
    let first = 4 + encodings.len();
    let refs: String = (0..encodings.len() + 1).map(|at| format!("{} 0 R ", 4 + at)).collect();
    let mut objects: Vec<Obj> = encodings.into_iter().map(|(dict, data)| Obj::Stream { dict, data }).collect();
    objects.push(stream("/Filter /JBIG2Decode", b"stored"));
    pages.push((
        "an array of every decoder, kind 1 and kind 2",
        one_page("", "", &format!("[{refs} (not a stream) {} 0 R]", first + 5), objects),
    ));
    // Each kind of image record, and a form with children (one of them a form, which has none of its own).
    pages.push((
        "each kind of image record and a form with children",
        one_page(
            "",
            "/XObject << /Jpeg 5 0 R /Flat 6 0 R /Bad 7 0 R /Form 8 0 R /Direct << /Not 1 >> >>",
            "4 0 R",
            vec![
                stream("", b"/Jpeg Do /Flat Do /Bad Do /Missing Do /Direct Do /Form Do"),
                stream(JPEG_DICT, IMAGE),
                stream("/Subtype /Image /Filter /FlateDecode", &flate(b"pixels")),
                stream("/Subtype /Image /Filter /FlateDecode", b"not flate"),
                stream(
                    "/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Inner 9 0 R /Jpeg 5 0 R >> >>",
                    b"/Jpeg Do /Inner Do",
                ),
                stream("/Subtype /Form /BBox [0 0 1 1]", b"/Jpeg Do"),
            ],
        ),
    ));
    pages.push(("encrypted with an empty password", encrypted(&sample(""))));
    // Images past the cap (kind 4): equal in their first 16 MiB and different after, with different stored lengths.
    let mut margin = vec![0xffu8; 16 << 20];
    margin.extend_from_slice(&[0u8; 4096]);
    pages.push(("an image past its cap", page_with_image(&margin, 4096, true)));
    margin.truncate(16 << 20);
    margin.extend(std::iter::repeat_n(7u8, 4096));
    pages.push(("an image past its cap, another tail", page_with_image(&margin, 4096, true)));
    // Added after the second review: a form whose content fails after drawing (kind 3, no children), names with `#` (a `#xx` is its byte, a lone `#`
    // finds nothing), and an inline image whose data holds what looks like a `Do`.
    pages.push(("a form that fails to decode", page_with_a_failing_form().0));
    pages.push((
        "names with #",
        one_page(
            "",
            "/XObject << /Im1 5 0 R >>",
            "4 0 R",
            vec![stream("", b"/Im#31 Do /B#2 Do"), stream(JPEG_DICT, IMAGE)],
        ),
    ));
    pages.push((
        "an inline image",
        one_page(
            "",
            "/XObject << /Im1 5 0 R /Im2 6 0 R >>",
            "4 0 R",
            vec![
                stream("", b"q BI /W 2 /H 1 /CS /G /BPC 8 ID \x00/Im1 Do EI\xff EI Q /Im2 Do"),
                stream(JPEG_DICT, IMAGE),
                stream(JPEG_DICT, b"\xff\xd8 the second \xff\xd9"),
            ],
        ),
    ));
    pages
}

/// **The golden vectors of `recto-page/1`: its definition as bytes, from 2026-11-15.** Failure caught: any change to what the digest hashes, which would
/// make every digest kept by an older build (the journals' sidecars now; the corpus and references later) stop matching, silently. If this fails, the
/// change is a new digest with a new name, not a new number here. Made files only, so CI runs them (the corpus and qpdf tests are skipped there); a
/// page at a cap is among them, with no identity.
#[test]
fn golden_vectors_of_recto_page_1() {
    let golden = [
        ("a stream drawing an image", "1545f27d009ce1d54dc9c3341a366aad0373c2e69a0e8bf2919013ae80105017"),
        ("an empty stream", "6cabc28584551159bebfa8d86f4281db3aca009da3331634a73eb171ecb858bb"),
        ("a page with nothing", "aa0e58897d3a53edc29402cda657c335c6bb2c1937da2e715b3e4513be6cc5ed"),
        (
            "an array of every decoder, kind 1 and kind 2",
            "fbdacee476c4067263029691f4d2c71b9492291471079e2600ee9014d6ea6428",
        ),
        (
            "each kind of image record and a form with children",
            "b9a8c397dc6c273e4387e83dac8f1ecbab697d8fd2626a55d25db737211d9caa",
        ),
        (
            "encrypted with an empty password",
            "1545f27d009ce1d54dc9c3341a366aad0373c2e69a0e8bf2919013ae80105017",
        ),
        ("an image past its cap", "1561009a1c2a34a243ebe93f4005ff79d84c8f7675590a0c50831c581867d919"),
        (
            "an image past its cap, another tail",
            "b5f1f59a255c1c1edadc116be2506dcc9c0d2116c7a70bdde5df81639ebc5695",
        ),
        (
            "a form that fails to decode",
            "fb7c753b33290acdcb3e39badac94cbee61542a3b407d511f472f68d942013bc",
        ),
        ("names with #", "e41dc5de3b9a58e4a9db446a05e0143f7356482e301ac7dfdec37262eb5e98b5"),
        ("an inline image", "919925aaeace7faf744205570b8319de9733839a2ac098934e560fc7b12860f1"),
    ];
    let pages = golden_pages();
    let got: Vec<(&str, String)> = pages.iter().map(|(what, bytes)| (*what, hex(&digest_of(bytes)))).collect();
    let want: Vec<(&str, String)> = golden.iter().map(|(what, digest)| (*what, (*digest).to_string())).collect();
    assert_eq!(got, want);
    assert_ne!(got[6].1, got[7].1, "images past the cap differing after it, by their stored length");
    // At a cap: the content's 16 MiB passed by one byte, so no identity.
    let capped = one_page("", "", "4 0 R", vec![stream("/Filter /FlateDecode", &flate(&vec![b' '; (16 << 20) + 1]))]);
    assert_eq!(reads_of(&capped, &Caps::STANDARD)[0].identity, None);
}

/// Failure caught (security, the probe's repro: a hostile page's identity cost 10.8 GB of a 3 MB file): an image drawn under many names had its stream's
/// descriptor (its dictionary, parsed whole and charged to nothing) resolved again for each name; 256 names to one image whose dictionary holds a
/// 100,000-element array took 1.35 s and 789 MiB for one page, with 4,902 bytes charged. A stream is read once for a page, and the identity is what the
/// definition says and always was: every name's record, each with the same kind, length and hash.
#[test]
fn the_once_read_hostile_dictionary_costs_one_resolution() {
    let names = 256;
    let junk = format!("/Junk [{}]", "0 ".repeat(100_000));
    let draws: String = (0..names).map(|n| format!("/N{n} Do ")).collect();
    let table: String = (0..names).map(|n| format!("/N{n} 5 0 R ")).collect();
    let page = one_page(
        "",
        &format!("/XObject << {table} >>"),
        "4 0 R",
        vec![stream("", draws.as_bytes()), stream(&format!("{JPEG_DICT} {junk}"), IMAGE)],
    );
    DESCRIPTORS.with(|count| count.set(0));
    let read = reads_of(&page, &Caps::STANDARD).remove(0);
    assert_eq!(DESCRIPTORS.with(std::cell::Cell::get), 1);
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, draws.as_bytes()));
    expected.push(b'X');
    expected.extend_from_slice(&(names as u32).to_le_bytes());
    for n in 0..names {
        expected.extend(xobject_bytes(format!("N{n}").as_bytes(), 1, IMAGE));
        expected.extend_from_slice(&0u32.to_le_bytes());
    }
    expected.push(b'E');
    assert_eq!(hex(&read.identity.expect("an identity").digest), hex(&sha(&expected)));
}

/// Failure caught (the memo's neutrality): a stream taken again is charged to the page as it was when read, so the read cap still counts every record
/// as it did. Three names to one image cost three stored lengths of the read cap, not one: one byte less and the page has no identity.
#[test]
fn a_stream_taken_again_is_charged_as_it_was_read() {
    let draws = b"/N0 Do /N1 Do /N2 Do ";
    let page = |caps: &Caps| {
        let bytes = one_page(
            "",
            "/XObject << /N0 5 0 R /N1 5 0 R /N2 5 0 R >>",
            "4 0 R",
            vec![stream("", draws), stream(JPEG_DICT, IMAGE)],
        );
        reads_of(&bytes, caps).remove(0)
    };
    let exact = Caps {
        page_read: (draws.len() + 3 * IMAGE.len()) as u64,
        ..Caps::STANDARD
    };
    let read = page(&exact);
    assert!(read.identity.is_some());
    assert_eq!(read.bytes, page(&Caps::STANDARD).bytes);
    assert!(
        page(&Caps {
            page_read: exact.page_read - 1,
            ..exact
        })
        .identity
        .is_none()
    );
    // And the bytes it hashed count too: three images' lengths of the page's own cap.
    let bytes = Caps {
        page_bytes: (draws.len() + 3 * IMAGE.len()) as u64,
        ..Caps::STANDARD
    };
    assert!(page(&bytes).identity.is_some());
    assert!(
        page(&Caps {
            page_bytes: bytes.page_bytes - 1,
            ..bytes
        })
        .identity
        .is_none()
    );
}

/// Failure caught (the memo's neutrality): an image under two names and a form with one child under two names (the child is the image again) hash
/// as the definition says, each name's record whole, the form's count of children after each of its names.
#[test]
fn an_image_and_a_form_under_two_names_each_are_hashed_as_defined() {
    let form_content = b"/C Do";
    let draws = b"/A Do /B Do /F Do /G Do";
    let page = one_page(
        "",
        "/XObject << /A 5 0 R /B 5 0 R /F 6 0 R /G 6 0 R /C 5 0 R >>",
        "4 0 R",
        vec![
            stream("", draws),
            stream(JPEG_DICT, IMAGE),
            stream("/Subtype /Form /BBox [0 0 1 1]", form_content),
        ],
    );
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, draws));
    expected.push(b'X');
    expected.extend_from_slice(&4u32.to_le_bytes());
    for name in [&b"A"[..], b"B"] {
        expected.extend(xobject_bytes(name, 1, IMAGE));
        expected.extend_from_slice(&0u32.to_le_bytes());
    }
    for name in [&b"F"[..], b"G"] {
        expected.extend(xobject_bytes(name, 0, form_content));
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend(xobject_bytes(b"C", 1, IMAGE));
    }
    expected.push(b'E');
    assert_eq!(hex(&digest_of(&page)), hex(&sha(&expected)));
}

/// Failure caught (memory, review of the memo): each form kept its own copy of its `/Resources`' `/XObject` table for the whole page, so 100 forms that
/// share one `/Resources 5 0 R` -> `/XObject 6 0 R` held 100 copies of a table that may be huge, where one at a time was held before. The shared
/// resource dictionary and table are resolved once for the page, whatever the number of forms, and the identity is the one it always had.
#[test]
fn forms_sharing_resources_resolve_them_once() {
    let forms = 100;
    let draws: String = (0..forms).map(|n| format!("/F{n} Do ")).collect();
    let table: String = (0..forms).map(|n| format!("/F{n} {} 0 R ", 8 + n)).collect();
    let mut objects = vec![
        stream("", draws.as_bytes()),
        Obj::Dict("<< /XObject 6 0 R >>".into()),
        Obj::Dict("<< /C 7 0 R >>".into()),
        stream(JPEG_DICT, IMAGE),
    ];
    for _ in 0..forms {
        objects.push(stream("/Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R", b"/C Do"));
    }
    let page = one_page("", &format!("/XObject << {table} >>"), "4 0 R", objects);
    DICTIONARIES.with(|count| count.set(0));
    let read = reads_of(&page, &Caps::STANDARD).remove(0);
    assert!(read.identity.is_some());
    assert_eq!(DICTIONARIES.with(std::cell::Cell::get), 2, "the shared /Resources and its /XObject, once each");
}

/// Failure caught (memo neutrality): forms sharing a large `/XObject` table but not drawn in a row (A, B with another table, A again) have the shared
/// table evicted and resolved again; a re-resolved table must give the children it always did, not B's or none. The expectation is the definition's.
#[test]
fn a_table_evicted_and_resolved_again_gives_the_same_children() {
    let (other, form) = (b"another image's bytes", b"/C Do");
    let filler: String = (0..300).map(|n| format!("/P{n} 7 0 R ")).collect();
    let objects = vec![
        stream("", b"/FA1 Do /FB Do /FA2 Do"),
        Obj::Dict("<< /XObject 9 0 R >>".into()),
        Obj::Dict("<< /XObject 10 0 R >>".into()),
        stream(JPEG_DICT, IMAGE),
        stream(JPEG_DICT, other),
        Obj::Dict(format!("<< /C 7 0 R {filler}>>")),
        Obj::Dict("<< /C 8 0 R >>".into()),
        stream("/Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R", form),
        stream("/Subtype /Form /BBox [0 0 1 1] /Resources 6 0 R", form),
        stream("/Subtype /Form /BBox [0 0 1 1] /Resources 5 0 R", form),
    ];
    let page = one_page("", "/XObject << /FA1 11 0 R /FB 12 0 R /FA2 13 0 R >>", "4 0 R", objects);
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, b"/FA1 Do /FB Do /FA2 Do"));
    expected.push(b'X');
    expected.extend_from_slice(&3u32.to_le_bytes());
    for (name, image) in [(&b"FA1"[..], IMAGE), (b"FB", other), (b"FA2", IMAGE)] {
        expected.extend(xobject_bytes(name, 0, form));
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend(xobject_bytes(b"C", 1, image));
    }
    expected.push(b'E');
    DICTIONARIES.with(|count| count.set(0));
    assert_eq!(hex(&digest_of(&page)), hex(&sha(&expected)));
    assert_eq!(DICTIONARIES.with(std::cell::Cell::get), 5, "R1, T, R2, U, then T again as it was evicted");
}

/// Failure caught (security, review of the memo): a name whose object isn't a stream failed its descriptor each time, and a failure wasn't remembered,
/// so 256 names to one `5 0 obj << /Junk [100,000 numbers] >>` parsed the dictionary 256 times, as the image repro did before the memo. A failure doesn't
/// depend on what's left of any cap, so it's remembered with the rest, and its record is the one the definition gives: nothing by that name, kind 2.
#[test]
fn a_name_that_is_not_a_stream_is_resolved_once() {
    let names = 256;
    let draws: String = (0..names).map(|n| format!("/N{n} Do ")).collect();
    let table: String = (0..names).map(|n| format!("/N{n} 5 0 R ")).collect();
    let junk = format!("<< /Junk [{}] >>", "0 ".repeat(100_000));
    let page = one_page(
        "",
        &format!("/XObject << {table} >>"),
        "4 0 R",
        vec![stream("", draws.as_bytes()), Obj::Dict(junk)],
    );
    DESCRIPTORS.with(|count| count.set(0));
    let read = reads_of(&page, &Caps::STANDARD).remove(0);
    assert_eq!(DESCRIPTORS.with(std::cell::Cell::get), 1);
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, draws.as_bytes()));
    expected.push(b'X');
    expected.extend_from_slice(&(names as u32).to_le_bytes());
    for n in 0..names {
        let name = format!("N{n}");
        expected.extend_from_slice(&(name.len() as u32).to_le_bytes());
        expected.extend_from_slice(name.as_bytes());
        expected.push(2);
        expected.extend_from_slice(&0u32.to_le_bytes());
    }
    expected.push(b'E');
    assert_eq!(hex(&read.identity.expect("an identity").digest), hex(&sha(&expected)));
}

/// Failure caught (security, review of the memo): an image whose decode failed is taken again from the memo only while the page has more left than one
/// stream may take; with less, each name read it again, and resolved its descriptor again: the same 256 parses of a 100,000-element dictionary the memo
/// was made to stop, by way of small streams that bring the page's budget down. Here the page's budget never exceeds a stream's, so every name is the
/// re-read case; the descriptor is kept for it, resolved once, and each name's record is what the definition gives: kind 3, the stored bytes.
#[test]
fn a_gave_up_image_re_read_for_want_of_headroom_is_resolved_once() {
    let names = 256;
    let stored = b"not a flate stream";
    let junk = format!("/Junk [{}]", "0 ".repeat(100_000));
    let draws: String = (0..names).map(|n| format!("/N{n} Do ")).collect();
    let table: String = (0..names).map(|n| format!("/N{n} 5 0 R ")).collect();
    let page = one_page(
        "",
        &format!("/XObject << {table} >>"),
        "4 0 R",
        vec![
            stream("", draws.as_bytes()),
            stream(&format!("/Subtype /Image /Filter /FlateDecode {junk}"), stored),
        ],
    );
    let caps = Caps {
        xobject_bytes: 1 << 16,
        page_bytes: 1 << 16,
        ..Caps::STANDARD
    };
    DESCRIPTORS.with(|count| count.set(0));
    let read = reads_of(&page, &caps).remove(0);
    assert_eq!(DESCRIPTORS.with(std::cell::Cell::get), 1);
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, draws.as_bytes()));
    expected.push(b'X');
    expected.extend_from_slice(&(names as u32).to_le_bytes());
    for n in 0..names {
        expected.extend(xobject_bytes(format!("N{n}").as_bytes(), 3, stored));
        expected.extend_from_slice(&0u32.to_le_bytes());
    }
    expected.push(b'E');
    assert_eq!(hex(&read.identity.expect("an identity").digest), hex(&sha(&expected)));
}

/// Failure caught (security, the same class as the memo's): an image's `/Filter` array of 8 references (the most a chain takes) to one object parsed it 8 times and
/// more in one read, whatever its size. (`deref` itself parses a non-stream twice, a stream probe and then the object, which is lopdf's API; the count here is
/// of the misses of `Refs`, the times a read went to the reader for an object.) Each object a read points at is parsed once for that read, and the
/// record is what the definition gives: a filter that isn't general, as stored.
#[test]
fn a_read_follows_each_reference_once() {
    let refs = 8;
    let filters: String = (0..refs).map(|_| "6 0 R ").collect();
    let page = one_page(
        "",
        "/XObject << /Im1 5 0 R >>",
        "4 0 R",
        vec![
            stream("", b"/Im1 Do"),
            stream(&format!("/Subtype /Image /Filter [{filters}]"), IMAGE),
            Obj::Dict("/DCTDecode".into()),
        ],
    );
    DEREFERENCES.with(|count| count.set(0));
    let read = reads_of(&page, &Caps::STANDARD).remove(0);
    assert_eq!(DEREFERENCES.with(std::cell::Cell::get), 1);
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, b"/Im1 Do"));
    expected.push(b'X');
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(xobject_bytes(b"Im1", 1, IMAGE));
    expected.extend_from_slice(&0u32.to_le_bytes());
    expected.push(b'E');
    assert_eq!(hex(&read.identity.expect("an identity").digest), hex(&sha(&expected)));
}

/// Failure caught (the memo's untested branch): a form drawn first as another form's child (a record with no children of its own) and then by the page
/// itself was taken from the memo as it was, so it had no children where the definition gives it the ones its content draws.
#[test]
fn a_form_drawn_as_a_child_and_then_directly_has_its_children() {
    let (g_content, f_content) = (b"/F Do", b"/C Do");
    let page = one_page(
        "",
        "/XObject << /G 5 0 R /F 6 0 R /C 7 0 R >>",
        "4 0 R",
        vec![
            stream("", b"/G Do /F Do"),
            stream("/Subtype /Form /BBox [0 0 1 1]", g_content),
            stream("/Subtype /Form /BBox [0 0 1 1]", f_content),
            stream(JPEG_DICT, IMAGE),
        ],
    );
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, b"/G Do /F Do"));
    expected.push(b'X');
    expected.extend_from_slice(&2u32.to_le_bytes());
    // G, with F as its one child (no children of its own); then F, drawn by the page, with its child C.
    expected.extend(xobject_bytes(b"G", 0, g_content));
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(xobject_bytes(b"F", 0, f_content));
    expected.extend(xobject_bytes(b"F", 0, f_content));
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(xobject_bytes(b"C", 1, IMAGE));
    expected.push(b'E');
    assert_eq!(hex(&digest_of(&page)), hex(&sha(&expected)));
}

/// Failure caught (the memo's untested branch): an image whose decode failed, drawn under a second name, was taken from the memo as it was even where
/// the page had less left than a stream may take, which can end the decode in a cap where it ended in an error when it was read with more. Its stream
/// decodes to exactly `xobject_bytes` and then fails; read after another name has taken `left` of the page, the second read is as it would be alone:
/// the same failure and the same record with `xobject_bytes` or more left, and with one less the decode reaches its cap, so no identity.
#[test]
fn a_gave_up_image_is_read_again_where_the_page_has_less_left_than_a_stream_may_take() {
    let decoded_len = 64u64;
    // A run of 64 bytes, then a literal run cut short (an error): the 64 bytes come, then the failure. Not an image, so none is hashed as a prefix.
    let stored = vec![0xc1, b'a', 5, b'x'];
    let draws = b"/A Do /B Do";
    let page = one_page(
        "",
        "/XObject << /A 5 0 R /B 5 0 R >>",
        "4 0 R",
        vec![stream("", draws), stream("/Filter /RunLengthDecode", &stored)],
    );
    let xobject_bytes_cap = decoded_len;
    // What's left when the second name is read: the page's cap less the content and the first read's stored bytes.
    let under = |left: u64| Caps {
        xobject_bytes: xobject_bytes_cap,
        page_bytes: left + draws.len() as u64 + stored.len() as u64,
        ..Caps::STANDARD
    };
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, draws));
    expected.push(b'X');
    expected.extend_from_slice(&2u32.to_le_bytes());
    for name in [&b"A"[..], b"B"] {
        expected.extend(xobject_bytes(name, 3, &stored));
        expected.extend_from_slice(&0u32.to_le_bytes());
    }
    expected.push(b'E');
    let expected = sha(&expected);
    assert_eq!(
        digest_under(&page, &under(xobject_bytes_cap + 1)),
        Some(expected),
        "more left than a stream may take"
    );
    assert_eq!(digest_under(&page, &under(xobject_bytes_cap)), Some(expected), "just what a stream may take");
    assert_eq!(
        digest_under(&page, &under(xobject_bytes_cap - 1)),
        None,
        "less: the decode that failed reaches the cap"
    );
}

/// Failure caught (memory, review of the held objects): a `Refs` kept every value it had followed, whatever its size, so a dictionary pointing at a
/// 100,000-element array held the whole array for the read. A small value is kept (a second look adds no parse); one past the byte bound is held by
/// nothing, and parsed again where it's asked for again.
#[test]
fn a_read_keeps_a_small_value_and_holds_nothing_for_a_big_one() {
    let big = format!("[{}]", "0 ".repeat(100_000));
    let bytes = one_page("", "", "4 0 R", vec![stream("", b""), Obj::Dict("[1 2 3]".into()), Obj::Dict(big)]);
    let reader = open_bytes_for_identity(Arc::from(bytes)).expect("it opens");
    let refs = Refs::new(&reader);
    let (small, large) = (Object::Reference((5, 0)), Object::Reference((6, 0)));
    DEREFERENCES.with(|count| count.set(0));
    assert!(matches!(refs.deref(&small), Some(Object::Array(items)) if items.len() == 3));
    assert!(matches!(refs.deref(&small), Some(Object::Array(_))));
    assert_eq!(DEREFERENCES.with(std::cell::Cell::get), 1, "a small value is followed once");
    assert!(matches!(refs.deref(&large), Some(Object::Array(items)) if items.len() == 100_000));
    assert!(!refs.seen.borrow().contains_key(&(6, 0)), "a big value isn't held");
    assert_eq!(refs.seen.borrow().len(), 1);
}

/// Failure caught (memory, review of the held objects): an image past its cap with a 100 KB `/ColorSpace` name has a record whose tail is as long, and the
/// page held it, and its descriptor, for each such image it drew. Such a record isn't kept for the page, so the image drawn under two names is read twice
/// (two descriptors), and each name's record is what the definition gives: the byte 4, the kind it would have been, its first `n` bytes' hash, its stored
/// length, then what it says of itself.
#[test]
fn a_record_with_a_long_tail_is_read_again_and_is_the_same() {
    let space = "S".repeat(100_000);
    let data = b"12345678";
    let page = one_page(
        "",
        "/XObject << /A 5 0 R /B 5 0 R >>",
        "4 0 R",
        vec![
            stream("", b"/A Do /B Do"),
            stream(
                &format!("/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /{space} /Filter /DCTDecode"),
                data,
            ),
        ],
    );
    let caps = Caps {
        xobject_bytes: 4,
        ..Caps::STANDARD
    };
    DESCRIPTORS.with(|count| count.set(0));
    let read = reads_of(&page, &caps).remove(0);
    assert_eq!(
        DESCRIPTORS.with(std::cell::Cell::get),
        2,
        "its record is too long to keep, so each name reads it"
    );
    let mut record = vec![4, 1];
    record.extend_from_slice(&4u64.to_le_bytes());
    record.extend_from_slice(&sha(&data[..4]));
    record.extend_from_slice(&(data.len() as u64).to_le_bytes());
    for integer in [1i64, 1, 8] {
        record.push(1);
        record.extend_from_slice(&integer.to_le_bytes());
    }
    record.push(2);
    record.extend_from_slice(&(space.len() as u32).to_le_bytes());
    record.extend_from_slice(space.as_bytes());
    record.push(4);
    record.extend_from_slice(&1u32.to_le_bytes());
    record.extend_from_slice(&(b"DCTDecode".len() as u32).to_le_bytes());
    record.extend_from_slice(b"DCTDecode");
    let mut expected = b"recto-page/1\0C".to_vec();
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend(content_bytes(0, b"/A Do /B Do"));
    expected.push(b'X');
    expected.extend_from_slice(&2u32.to_le_bytes());
    for name in [&b"A"[..], b"B"] {
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(name);
        expected.extend_from_slice(&record);
        expected.extend_from_slice(&0u32.to_le_bytes());
    }
    expected.push(b'E');
    assert_eq!(hex(&read.identity.expect("an identity").digest), hex(&sha(&expected)));
}

/// Failure caught (security, the cost of raising the names cap from 256 to 8,192): an image drawn under 8,192 names had its descriptor (its dictionary,
/// parsed whole, here 100,000 elements) resolved for each name, 32 times the probe's 10.8 GB. At the cap a page has an identity, and the one image is
/// resolved once.
#[test]
fn the_names_cap_does_not_multiply_the_cost_of_one_hostile_image() {
    let names = 8192;
    let junk = format!("/Junk [{}]", "0 ".repeat(100_000));
    let draws: String = (0..names).map(|n| format!("/N{n} Do ")).collect();
    let table: String = (0..names).map(|n| format!("/N{n} 5 0 R ")).collect();
    let page = one_page(
        "",
        &format!("/XObject << {table} >>"),
        "4 0 R",
        vec![stream("", draws.as_bytes()), stream(&format!("{JPEG_DICT} {junk}"), IMAGE)],
    );
    DESCRIPTORS.with(|count| count.set(0));
    assert!(reads_of(&page, &Caps::STANDARD).remove(0).identity.is_some(), "8,192 names drawn");
    assert_eq!(DESCRIPTORS.with(std::cell::Cell::get), 1);
}

/// A double-Flate stream that decodes to `kib` KiB of zeros and then fails (its inner Adler-32 cut off), from about a kilobyte stored.
fn failing_after(kib: usize) -> Vec<u8> {
    let mut inner = flate(&vec![0u8; kib << 10]);
    inner.truncate(inner.len() - 4);
    flate(&inner)
}

/// The least `decode_work` under which `page` still has an identity, found by halving: the page's decode work as the budget counts it.
fn least_decode_work(page: &[u8]) -> u64 {
    let has = |decode_work: u64| digest_under(page, &Caps { decode_work, ..Caps::STANDARD }).is_some();
    assert!(has(u64::MAX), "the page has an identity with no limit");
    let (mut low, mut high) = (0u64, 8u64 << 20);
    while low < high {
        let middle = low + (high - low) / 2;
        if has(middle) { high = middle } else { low = middle + 1 }
    }
    low
}

/// Failure caught (review round 1, blocker): a decode that fails is charged nothing but its stored length, so a page of failing double-Flate forms
/// (each a kilobyte stored, 16 MiB decoded before its error) costs records times 16 MiB, 128 GiB at 8,192 records, from 2 MB of file. Four such forms,
/// each failing after 256 KiB, have a decode work of at least 1 MiB; at that limit the page has an identity and one byte under it none.
#[test]
fn forms_that_fail_to_decode_are_charged_the_work_they_did() {
    let page = |count: usize| {
        let names: String = (0..count).map(|at| format!("/F{at} {} 0 R ", 5 + at)).collect();
        let drawn: String = (0..count).map(|at| format!("/F{at} Do ")).collect();
        let mut objects = vec![stream("", drawn.as_bytes())];
        objects.extend((0..count).map(|_| stream("/Subtype /Form /BBox [0 0 1 1] /Filter [/FlateDecode /FlateDecode]", &failing_after(256))));
        one_page("", &format!("/XObject << {names} >>"), "4 0 R", objects)
    };
    let page = page(4);
    let limit = least_decode_work(&page);
    assert!(limit >= 1 << 20, "four forms decoded 256 KiB each before failing, charged {limit}");
    assert!(
        digest_under(
            &page,
            &Caps {
                decode_work: limit,
                ..Caps::STANDARD
            }
        )
        .is_some(),
        "at the limit"
    );
    assert_eq!(
        digest_under(
            &page,
            &Caps {
                decode_work: limit - 1,
                ..Caps::STANDARD
            }
        ),
        None,
        "one byte under"
    );
}

/// Failure caught: the same for a page's content streams, which are read as stored after a failed decode: 1,024 streams each failing after 16 MiB.
#[test]
fn content_streams_that_fail_to_decode_are_charged_the_work_they_did() {
    let count = 4;
    let list: String = (0..count).map(|at| format!("{} 0 R ", 4 + at)).collect();
    let page = one_page(
        "",
        "",
        &format!("[{list}]"),
        (0..count).map(|_| stream("/Filter [/FlateDecode /FlateDecode]", &failing_after(256))).collect(),
    );
    let limit = least_decode_work(&page);
    assert!(limit >= 1 << 20, "four streams decoded 256 KiB each before failing, charged {limit}");
    assert!(
        digest_under(
            &page,
            &Caps {
                decode_work: limit,
                ..Caps::STANDARD
            }
        )
        .is_some(),
        "at the limit"
    );
    assert_eq!(
        digest_under(
            &page,
            &Caps {
                decode_work: limit - 1,
                ..Caps::STANDARD
            }
        ),
        None,
        "one byte under"
    );
}

/// Failure caught: a failing form drawn under many names is charged once, the memo giving it back free, so 8,192 names of one 16 MiB failure cost
/// 128 GiB of the work the cap counts. Drawn under 4 names, it is charged 4 times.
#[test]
fn a_failing_form_taken_from_the_memo_is_charged_again() {
    let draws: String = (0..4).map(|n| format!("/N{n} Do ")).collect();
    let table: String = (0..4).map(|n| format!("/N{n} 5 0 R ")).collect();
    let page = one_page(
        "",
        &format!("/XObject << {table} >>"),
        "4 0 R",
        vec![
            stream("", draws.as_bytes()),
            stream("/Subtype /Form /BBox [0 0 1 1] /Filter [/FlateDecode /FlateDecode]", &failing_after(256)),
        ],
    );
    assert!(least_decode_work(&page) >= 1 << 20, "four draws of a form that decodes 256 KiB before failing");
}
