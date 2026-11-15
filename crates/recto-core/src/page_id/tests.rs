//! The page digest's tests: its definition, by bytes built by hand from the module doc; what it leaves out and what it doesn't; every cap at its limit
//! plus one; hostile pages; and golden vectors, so a change to `recto-page/1` can't be made silently.

use super::*;
use recto_testpdf::{Object as Obj, build};
use std::io::Write;
use std::sync::Arc;

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

fn ids_of(bytes: &[u8], caps: &Caps) -> Vec<PageIdentity> {
    let reader = open_bytes_for_identity(Arc::from(bytes)).expect("it opens");
    let map = reader.page_map().expect("its pages");
    map.iter().map(|entry| identify_page(&reader, entry, caps)).collect()
}

fn id_of(bytes: &[u8]) -> PageIdentity {
    ids_of(bytes, &Caps::STANDARD)[0]
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

const CONTENT: &[u8] = b"BT /F1 12 Tf 72 712 Td (One page) Tj ET";
const IMAGE: &[u8] = b"\xff\xd8 not really a JPEG, only its bytes \xff\xd9";

/// A page with flate content and a JPEG-labelled image named `/Im1`.
fn sample(page_extra: &str) -> Vec<u8> {
    one_page(
        page_extra,
        "/XObject << /Im1 5 0 R >>",
        "4 0 R",
        vec![
            Obj::Stream {
                dict: "/Filter /FlateDecode".into(),
                data: flate(CONTENT),
            },
            Obj::Stream {
                dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode".into(),
                data: IMAGE.to_vec(),
            },
        ],
    )
}

/// Failure caught: the code hashes something other than what its definition says, so a digest made from the module doc (by the locator's later code,
/// or by a reader of the doc) wouldn't match. The bytes are built here by hand, step by step as the doc lists them.
#[test]
fn the_digest_is_what_its_definition_says() {
    let mut expected = Vec::new();
    expected.extend_from_slice(b"recto-page/1\0");
    // 2: one content stream, decoded (kind 0), its length and its hash.
    expected.push(0);
    expected.extend_from_slice(&(CONTENT.len() as u64).to_le_bytes());
    expected.extend_from_slice(&sha(CONTENT));
    expected.extend_from_slice(b"\0X");
    // 4: `/Im1`, an image codec (kind 1), as stored.
    expected.extend_from_slice(&3u32.to_le_bytes());
    expected.extend_from_slice(b"Im1");
    expected.push(1);
    expected.extend_from_slice(&(IMAGE.len() as u64).to_le_bytes());
    expected.extend_from_slice(&sha(IMAGE));
    // 5: no flags.
    expected.extend_from_slice(b"\0E\0");
    let id = id_of(&sample(""));
    assert_eq!(hex(&id.digest), hex(&sha(&expected)));
    assert_eq!(id.object, (3, 0));
}

/// Failure caught: a view of the same page (a turn, a crop, another media box), a highlight added (`/Annots`), another font, or the page under another
/// object number changes its digest, so edits on it would be set aside after a mere re-save or our own turn (research §3: these are what other
/// programs change). The frame tag is what says a turn or crop did change.
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
        Obj::Stream {
            dict: "/Filter /FlateDecode".into(),
            data: flate(CONTENT),
        },
        Obj::Stream {
            dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode".into(),
            data: IMAGE.to_vec(),
        },
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

/// Failure caught (research §5.2, data table C): a re-encode that keeps the bytes changes the digest: qpdf wrote `/Filter [/FlateDecode]` for
/// `/Filter /FlateDecode` and re-compressed, which changed a page's digest under a mere rotation. Stored plainly, flated, flated with a PNG predictor,
/// hex over flate, or split over two streams the same way: the content's the same, and so is the digest, where its decoded bytes are equal.
#[test]
fn the_same_bytes_encoded_another_way_are_the_same_page() {
    let with = |dict: &str, data: Vec<u8>| id_of(&one_page("", "", "4 0 R", vec![Obj::Stream { dict: dict.into(), data }])).digest;
    let plain = with("", CONTENT.to_vec());
    assert_eq!(with("/Filter /FlateDecode", flate(CONTENT)), plain);
    assert_eq!(with("/Filter [/FlateDecode]", flate(CONTENT)), plain);
    let hexed: String = flate(CONTENT).iter().map(|byte| format!("{byte:02x}")).collect::<String>() + ">";
    assert_eq!(with("/Filter [/ASCIIHexDecode /FlateDecode]", hexed.into_bytes()), plain);
    // PNG "None" rows of 13 bytes: the content is 39 bytes, three rows.
    assert_eq!(CONTENT.len(), 39);
    let rows: Vec<u8> = CONTENT.chunks(13).flat_map(|row| std::iter::once(0).chain(row.iter().copied())).collect();
    assert_eq!(with("/Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 13 >>", flate(&rows)), plain);
    // An image whose general filter is written two ways keeps its digest too (kind 0, decoded).
    let image = |filter: &str, data: Vec<u8>| {
        id_of(&one_page(
            "",
            "/XObject << /Im1 5 0 R >>",
            "4 0 R",
            vec![
                Obj::Stream {
                    dict: String::new(),
                    data: CONTENT.to_vec(),
                },
                Obj::Stream {
                    dict: format!("/Type /XObject /Subtype /Image /Width 3 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB {filter}"),
                    data,
                },
            ],
        ))
        .digest
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
        id_of(&one_page(
            "",
            "/XObject << /Im1 5 0 R >>",
            "4 0 R",
            vec![
                Obj::Stream {
                    dict: String::new(),
                    data: b"q 100 0 0 100 0 0 cm /Im1 Do Q".to_vec(),
                },
                Obj::Stream {
                    dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode".into(),
                    data: pixels.to_vec(),
                },
            ],
        ))
        .digest
    };
    assert_ne!(direct(b"\xff\xd8 one \xff\xd9"), direct(b"\xff\xd8 two \xff\xd9"));
    let wrapped = |pixels: &[u8]| {
        id_of(&one_page(
            "",
            "/XObject << /Fm1 5 0 R >>",
            "4 0 R",
            vec![
                Obj::Stream {
                    dict: String::new(),
                    data: b"/Fm1 Do".to_vec(),
                },
                Obj::Stream {
                    dict: "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Im1 6 0 R >> >>".into(),
                    data: b"q 100 0 0 100 0 0 cm /Im1 Do Q".to_vec(),
                },
                Obj::Stream {
                    dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode".into(),
                    data: pixels.to_vec(),
                },
            ],
        ))
        .digest
    };
    assert_ne!(wrapped(b"\xff\xd8 one \xff\xd9"), wrapped(b"\xff\xd8 two \xff\xd9"));
}

/// Two content streams: the first `first`, the second `second`, under `caps`.
fn two_streams(first: &[u8], second: &[u8], caps: &Caps) -> [u8; 32] {
    ids_of(
        &one_page(
            "",
            "",
            "[4 0 R 5 0 R]",
            vec![
                Obj::Stream {
                    dict: String::new(),
                    data: first.to_vec(),
                },
                Obj::Stream {
                    dict: String::new(),
                    data: second.to_vec(),
                },
            ],
        ),
        caps,
    )[0]
    .digest
}

/// Failure caught: the content cap isn't where it's said to be. Past it nothing counts (two pages differing only there are one, which is the price of a
/// bound), and before it everything does. Held at the cap plus one with the cap set small; and the stored-bytes path that the research's sketch left
/// without a cap is under it too: a stream that won't decode, larger than the cap, hashes no more than the cap.
#[test]
fn the_content_cap_is_where_it_is_said_on_every_path() {
    let caps = Caps {
        content_bytes: 10,
        ..Caps::STANDARD
    };
    assert_eq!(
        two_streams(b"0123456", b"789AAAA", &caps),
        two_streams(b"0123456", b"789BBBB", &caps),
        "past the cap"
    );
    assert_ne!(
        two_streams(b"0123456", b"78AAAAA", &caps),
        two_streams(b"0123456", b"789AAAA", &caps),
        "at its last byte"
    );
    // A stream that doesn't decode is hashed as stored, to the cap and no further.
    let broken = |tail: &[u8]| {
        let mut data = b"not zlib at all, past ten bytes: ".to_vec();
        data.extend_from_slice(tail);
        ids_of(
            &one_page(
                "",
                "",
                "4 0 R",
                vec![Obj::Stream {
                    dict: "/Filter /FlateDecode".into(),
                    data,
                }],
            ),
            &caps,
        )[0]
        .digest
    };
    assert_eq!(broken(b"one"), broken(b"two"));
    // At the standard cap, a stored stream of 16 MiB and one byte that differs only in that byte is the same page.
    let big = |last: u8| {
        let mut data = vec![b' '; (16 << 20) + 1];
        *data.last_mut().unwrap() = last;
        id_of(&one_page("", "", "4 0 R", vec![Obj::Stream { dict: String::new(), data }])).digest
    };
    assert_eq!(big(b'a'), big(b'b'));
}

/// Failure caught: the stream and image counts aren't capped where the definition says, so a page of 100,000 streams costs what it likes. At 1,024
/// streams and 64 images the 1,025th and 65th count (as a flag) but their bytes don't.
#[test]
fn the_stream_and_image_counts_are_capped() {
    let streams = |count: usize, last: &[u8]| {
        let refs: String = (0..count).map(|at| format!("{} 0 R ", 4 + at)).collect();
        let mut objects: Vec<Obj> = (0..count)
            .map(|_| Obj::Stream {
                dict: String::new(),
                data: b"0 0 m".to_vec(),
            })
            .collect();
        if let Some(Obj::Stream { data, .. }) = objects.last_mut() {
            *data = last.to_vec();
        }
        id_of(&one_page("", "", &format!("[{refs}]"), objects)).digest
    };
    assert_eq!(streams(1025, b"one"), streams(1025, b"two"), "the 1,025th's bytes don't count");
    assert_ne!(streams(1025, b"0 0 m"), streams(1024, b"0 0 m"), "but that there was one does");
    assert_ne!(streams(1024, b"one"), streams(1024, b"two"), "the 1,024th's do");
    let images = |count: usize, last: &[u8]| {
        let names: String = (0..count).map(|at| format!("/I{at:03} {} 0 R ", 5 + at)).collect();
        let mut objects = vec![Obj::Stream {
            dict: String::new(),
            data: CONTENT.to_vec(),
        }];
        objects.extend((0..count).map(|at| Obj::Stream {
            dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /DCTDecode".into(),
            data: if at + 1 == count { last.to_vec() } else { vec![at as u8] },
        }));
        id_of(&one_page("", &format!("/XObject << {names} >>"), "4 0 R", objects)).digest
    };
    assert_eq!(images(65, b"one"), images(65, b"two"), "the 65th's bytes don't count");
    assert_ne!(images(64, b"one"), images(64, b"two"), "the 64th's do");
    assert_ne!(images(65, b"x"), images(64, b"x"), "that there were more does");
}

/// Failure caught: the page's own caps aren't held, so a page of many large images hashes a gigabyte, or a stream that decodes to little from very
/// much is read to its end. With the caps set small: past the page's byte cap and its read cap nothing counts, and nothing panics.
#[test]
fn the_page_caps_hold() {
    let caps = Caps {
        page_bytes: 12,
        xobject_bytes: 8,
        ..Caps::STANDARD
    };
    let page = |second_image: &[u8]| {
        ids_of(
            &one_page(
                "",
                "/XObject << /A 5 0 R /B 6 0 R >>",
                "4 0 R",
                vec![
                    Obj::Stream {
                        dict: String::new(),
                        data: b"abc".to_vec(),
                    },
                    Obj::Stream {
                        dict: "/Subtype /Image /Filter /DCTDecode".into(),
                        data: b"12345678".to_vec(),
                    },
                    Obj::Stream {
                        dict: "/Subtype /Image /Filter /DCTDecode".into(),
                        data: second_image.to_vec(),
                    },
                ],
            ),
            &caps,
        )[0]
        .digest
    };
    // 3 bytes of content and 8 of the first image leave 1 for the second.
    assert_eq!(page(b"xAAA"), page(b"xBBB"), "past the page's cap");
    assert_ne!(page(b"xAAA"), page(b"yAAA"), "within it");
    let read_capped = Caps {
        page_read: 64,
        ..Caps::STANDARD
    };
    let zeros = |tail: u8| {
        let mut data = vec![0u8; 4096];
        data.push(tail);
        ids_of(&one_page("", "", "4 0 R", vec![Obj::Stream { dict: String::new(), data }]), &read_capped)[0].digest
    };
    assert_eq!(zeros(1), zeros(2), "past the read cap");
}

/// Failure caught: a hostile page panics the identity thread, or gives a different digest each time it's read (so a page would never match itself).
/// Broken references, a `/Contents` that isn't a stream, an image that isn't a reference, a filter that isn't a name, bad predictor parameters, a
/// form naming itself, and a page with nothing at all.
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
            vec![Obj::Stream {
                dict: "/Filter 7".into(),
                data: b"x".to_vec(),
            }],
        ),
        one_page(
            "",
            "",
            "4 0 R",
            vec![Obj::Stream {
                dict: "/Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 99999999999 /Colors -1 >>".into(),
                data: flate(b"abc"),
            }],
        ),
        one_page(
            "",
            "/XObject << /F 4 0 R >>",
            "4 0 R",
            vec![Obj::Stream {
                dict: "/Subtype /Form /Resources << /XObject << /F 4 0 R >> >>".into(),
                data: b"/F Do".to_vec(),
            }],
        ),
        build(&[
            Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
            Obj::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
        ]),
    ];
    for (at, bytes) in cases.iter().enumerate() {
        let first = ids_of(bytes, &Caps::STANDARD);
        let again = ids_of(bytes, &Caps::STANDARD);
        assert_eq!(first, again, "case {at}");
    }
}

/// Failure caught: the identity of a page in a file anyone may open (encrypted with an empty password, as many government reports are) is of its
/// stored, encrypted bytes, which a re-save with a fresh key changes, rather than of its content. It's the same as the plain file's.
#[test]
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts; never over a user's file.
fn a_file_anyone_may_open_is_identified_by_its_content() {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let plain = sample("");
    let mut document = lopdf::Document::load_mem(&plain).unwrap();
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
    let mut encrypted = Vec::new();
    document.save_to(&mut encrypted).unwrap();
    assert_eq!(hex(&id_of(&encrypted).digest), hex(&id_of(&plain).digest));
    let reader = open_bytes_for_identity(Arc::from(encrypted.as_slice())).unwrap();
    let ids = document_ids(&reader).expect("it has an /ID pair");
    assert_ne!(ids.first, ids.second);
}

/// **The golden vectors of `recto-page/1`.** Failure caught: any change to what the digest hashes, which would make every digest kept by an older
/// build (journals now; the corpus and references later) stop matching, silently. If this fails, the change is a new digest with a new name, not a
/// new number here.
#[test]
fn golden_vectors_of_recto_page_1() {
    let golden = [
        (sample(""), "cc90dc1fb2e141010085dcff0444960e4169627e490409236e2bbfff88380d00"),
        (
            one_page(
                "",
                "",
                "4 0 R",
                vec![Obj::Stream {
                    dict: String::new(),
                    data: Vec::new(),
                }],
            ),
            "7077da084ead392e51d16a6fe8e14c8100534dd6b8486d433dc39698151e385f",
        ),
        (
            build(&[
                Obj::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
                Obj::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
                Obj::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
            ]),
            "712ba2a9f49a01eb8780958ec9ce5e932f8275ef2b484156ea8e08c95616fefc",
        ),
    ];
    for (bytes, want) in golden {
        assert_eq!(hex(&id_of(&bytes).digest), want);
    }
}
