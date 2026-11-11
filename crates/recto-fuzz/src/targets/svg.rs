//! `export svg` and a snapshot as SVG, on what a compromised worker sends: the stream's rules and then `recto-svg`'s writer, on records as `recto_agent::svg_in_memory` takes them, which is the route `export svg` takes with the worker and the file taken away. And the JPEG reading on both sides of the worker: the host's scan of a JPEG's bytes as they arrive, stripped or not, and the worker's header fixes on the file's own bytes.
//!
//! Records come two ways: as the bytes say, which the record reader and the stream's rules mostly refuse; and made with the worker's own writers in an order and with numbers the bytes choose, so most get through to the writer, with NaN, infinities, huge counts, clips and glyphs that point anywhere, text that's markup, and JPEGs of any shape.
//!
//! What must hold: no panic, bounded time and memory; what's written is within its cap; and what's written passes the strict reader of what `recto-svg` writes, so nothing a worker says becomes markup, a script or a way out of the document.

use crate::guard::{Budget, within};
use crate::input::Take;
use recto_proto::contents::{ImageCoding, NoteCode, SEG_CURVE, SEG_LINE, SEG_MOVE, Why, put};
use recto_proto::jpeg::{HeaderFix, JpegScan, fix_header, frame_components, profile_says_srgb, scan_start};
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(60),
    memory: 512 * 1024 * 1024,
};
/// What one export may write here: well past what these inputs make, and small enough that the cap is reached now and then.
const CAP: u64 = 4 * 1024 * 1024;

/// Numbers as the record reader lets them through, all finite: the writer's to make sense of, however large or small.
const NUMBERS: &[f32] = &[
    0.0,
    1.0,
    0.5,
    72.0,
    612.0,
    792.0,
    -1.0,
    -0.0,
    1e-30,
    1e30,
    -1e30,
    3.4e38,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
];
/// And what it refuses, now and then, so the refusal is tried too.
const WILD_NUMBERS: &[f32] = &[f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
/// What a worker's text and glyph runs say a character is: markup, invisible and reordering characters, a surrogate and past Unicode's end.
const CODES: &[u32] = &[
    'a' as u32,
    '<' as u32,
    '>' as u32,
    '&' as u32,
    '"' as u32,
    '\'' as u32,
    ']' as u32,
    0,
    9,
    0x1F,
    0x7F,
    0x85,
    0x200B,
    0x202E,
    0xFEFF,
    0xFFFE,
    0x10FFFF,
    0x1F600,
];
const WILD_CODES: &[u32] = &[0xD800, 0x110000, u32::MAX];
/// Font families as a worker can name them: what a file's font is called is the file's to say.
const FAMILIES: &[&[u8]] = &[
    b"Helvetica",
    b"Times New Roman,Bold",
    b"</text><script>alert(1)</script>",
    b"a\"b'c",
    b"url(javascript:x)",
    b"\0\x01\xff\xfe",
    b"",
    b"ABCDEF+Garamond",
];

/// A JPEG's pieces: markers whose segments the scan reads and keeps or strips, an ICC profile in one or more parts, a scan with stuffed and restart bytes, and what can follow the end.
fn jpeg(take: &mut Take<'_>, wild: bool) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    if wild && take.u8().is_multiple_of(4) {
        out.clear();
    }
    for _ in 0..take.u8() % 8 {
        let (code, body): (u8, Vec<u8>) = match take.u8() % (9 + u8::from(wild)) {
            0 => (
                0xE0,
                b"JFIF\0\x01\x02\0\0\x01\0\x01\x02\x02\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff".to_vec(),
            ),
            1 => (
                0xE1,
                [
                    &b"Exif\0\0MM\0*\0\0\0\x08\0\x02\x01\x12\0\x03\0\0\0\x01\0"[..],
                    &[take.u8() % 9, 0, 0],
                    b"\x88\x25\0\x04\0\0\0\x01\0\0\0\0",
                ]
                .concat(),
            ),
            2 => (0xE1, b"http://ns.adobe.com/xap/1.0/\0<x/>".to_vec()),
            3 => {
                let (part, parts) = (take.u8() % 3, take.u8() % 3);
                let mut profile = vec![0u8; 132];
                profile[128..132].copy_from_slice(&1u32.to_be_bytes());
                profile.extend_from_slice(b"desc\0\0\0\x90\0\0\0\x20");
                profile.resize(144, 0);
                profile.extend_from_slice(take.pick(&[&b"descxxxxsRGB IEC61966-2.1"[..], b"mluc\0s\0R\0G\0B", b"descAdobe"]));
                (0xE2, [&b"ICC_PROFILE\0"[..], &[part, parts], &profile].concat())
            }
            4 => (0xEE, b"Adobe\0\x64\0\0\0\0\x01".to_vec()),
            5 => (0xFE, b"a comment".to_vec()),
            6 => (0xDB, vec![0; 65]),
            7 => (0xC0 + take.u8() % 3, vec![8, 0, 1, 0, 1, if wild { take.u8() % 5 } else { 1 }, 1, 0x11, 0]),
            8 => (0xC4, vec![0; 29]),
            _ => (take.u8(), (0..take.u8() % 32).map(|_| take.u8()).collect()),
        };
        out.extend_from_slice(&[0xFF, code]);
        let length = if wild && take.u8().is_multiple_of(4) {
            take.u16()
        } else {
            (body.len() + 2) as u16
        };
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&body);
    }
    // A frame before the scan, as a JPEG has to have.
    if !wild {
        out.extend_from_slice(&[0xFF, 0xC0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0]);
    }
    if !wild || !take.u8().is_multiple_of(4) {
        out.extend_from_slice(&[0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 0x3F, 0]);
        out.extend_from_slice(&[1, 2, 0xFF, 0x00, 3, 0xFF, 0xD0, 4]);
        if !wild || !take.u8().is_multiple_of(4) {
            out.extend_from_slice(&[0xFF, 0xD9]);
        }
    }
    let trailing = usize::from(take.u8() % 4) * 7;
    out.extend((0..trailing).map(|at| at as u8));
    out
}

/// A number for a record: one the reader takes, or, in a wild stream, one in sixteen that it doesn't.
fn number(take: &mut Take<'_>, wild: bool) -> f32 {
    if wild && take.u8().is_multiple_of(16) {
        take.pick(WILD_NUMBERS)
    } else {
        take.pick(NUMBERS)
    }
}

fn matrix(take: &mut Take<'_>, wild: bool) -> [f32; 6] {
    match take.u8() % 4 {
        0 | 1 => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        2 => [number(take, wild), 0.0, 0.0, number(take, wild), number(take, wild), number(take, wild)],
        _ => std::array::from_fn(|_| number(take, wild)),
    }
}

fn color(take: &mut Take<'_>) -> Option<[u8; 4]> {
    (take.u8() & 1 == 1).then(|| [take.u8(), take.u8(), take.u8(), take.pick(&[255, 0, 128])])
}

fn segs(out: &mut Vec<u8>, take: &mut Take<'_>, wild: bool) {
    let at = put::segs_begin(out);
    // Now and then very many points, which is what a page of hatching sends.
    let count = if take.u8() == 0xFF { 20_000 } else { u32::from(take.u8() % 24) };
    for point in 0..count {
        let kind = if wild {
            take.pick(&[SEG_MOVE, SEG_LINE, SEG_CURVE, 3, 0x7F])
        } else {
            take.pick(&[SEG_MOVE, SEG_LINE, SEG_LINE, SEG_CURVE])
        };
        put::seg(out, kind, point % 5 == 4, number(take, wild), number(take, wild));
    }
    put::segs_end(out, at, count);
}

/// A run's items: characters for text, when `glyphs` is 0, and otherwise glyphs numbered up to it.
fn items(take: &mut Take<'_>, glyphs: u32, wild: bool) -> Vec<(u32, f32, f32)> {
    (0..take.u8() % 24)
        .map(|_| {
            let code = if glyphs > 0 {
                1 + take.u32() % glyphs
            } else if wild && take.u8().is_multiple_of(8) {
                take.pick(WILD_CODES)
            } else {
                take.pick(CODES)
            };
            (code, number(take, wild), number(take, wild))
        })
        .collect()
}

/// Image bytes in pieces, as a worker sends a large one.
fn image_bytes(out: &mut Vec<u8>, take: &mut Take<'_>, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let size = (1 + usize::from(take.u8())).min(rest.len());
        let at = put::image_bytes_begin(out);
        out.extend_from_slice(&rest[..size]);
        put::image_bytes_end(out, at);
        rest = &rest[size..];
    }
}

/// A page's records, made the way the worker makes them, cut into frames where the bytes say.
fn made_stream(take: &mut Take<'_>) -> Vec<Vec<u8>> {
    let mut frames = vec![Vec::new()];
    // One stream in eight breaks the rules now and then, so the refusals are tried; the rest keep to them, so what's hostile reaches the writer.
    let wild = take.u8().is_multiple_of(8);
    let size = |take: &mut Take<'_>| {
        if wild {
            take.pick(&[612.0f32, 1.0, 14_400.0, 0.0, -1.0, 1e30, f32::NAN])
        } else {
            take.pick(&[612.0f32, 792.0, 1.0, 0.01, 14_400.0])
        }
    };
    put::page(frames.last_mut().unwrap_or_else(|| unreachable!()), size(take), size(take), &matrix(take, wild));
    let (mut glyphs, mut clips, mut groups) = (0u32, 0u32, 0u32);
    for _ in 0..256 {
        if take.is_empty() {
            break;
        }
        let out = frames.last_mut().unwrap_or_else(|| unreachable!());
        match take.u8() % 13 {
            0 | 1 => {
                let dashes: Vec<f32> = (0..take.u8() % 36).map(|_| number(take, wild)).collect();
                let (fill, stroke) = (color(take), color(take));
                put::path_begin(
                    out,
                    &matrix(take, wild),
                    fill,
                    take.u8() & 1 == 1,
                    stroke,
                    number(take, wild),
                    take.u8(),
                    take.u8(),
                    number(take, wild),
                    &dashes,
                );
                segs(out, take, wild);
            }
            2 => {
                let id = if wild && take.u8().is_multiple_of(4) { take.u32() } else { clips + 1 };
                let parent = if wild { take.u32() % (clips + 2) } else { take.u32() % (clips + 1) };
                put::clip_define_begin(out, id, parent, &matrix(take, wild));
                segs(out, take, wild);
                clips += 1;
            }
            3 => put::clip(out, take.u32() % (clips + 1 + u32::from(wild))),
            4 => {
                put::group_begin(out, take.u8());
                groups += 1;
            }
            5 if groups > 0 || wild => {
                put::group_end(out);
                groups = groups.saturating_sub(1);
            }
            6 => {
                glyphs += 1;
                put::glyph_define_begin(out, glyphs);
                segs(out, take, wild);
            }
            7 => {
                let (fill, stroke) = (color(take), color(take));
                // With none defined, a run has nothing in it, or in a wild stream glyphs nobody defined.
                let run = if glyphs == 0 && !wild { Vec::new() } else { items(take, glyphs.max(1), wild) };
                put::glyph_run(out, &matrix(take, wild), fill, stroke, number(take, wild), &run);
            }
            8 => {
                let fill = color(take);
                let (bold, italic, generic) = (take.u8() & 1 == 1, take.u8() & 1 == 1, take.u8() % (3 + u8::from(wild)));
                let family = take.pick(FAMILIES);
                let run = items(take, 0, wild);
                put::text_run(out, &matrix(take, wild), fill, bold, italic, generic, family, &run);
            }
            9 => {
                let channels = if wild { take.pick(&[1u8, 3, 4, 2]) } else { take.pick(&[1u8, 3, 4]) };
                let (width, height) = (1 + u32::from(take.u8() % 8), 1 + u32::from(take.u8() % 8));
                let total = u64::from(width * height * u32::from(channels));
                let why = take.pick(&[Why::Image, Why::Shading, Why::BlendOrMask, Why::TooComplex, Why::ImageWithMask, Why::OutsideBox]);
                put::image_begin(out, ImageCoding::Pixels { channels }, why, take.u8(), width, height, total, &matrix(take, wild));
                let pixels: Vec<u8> = (0..total).map(|_| take.u8()).collect();
                image_bytes(out, take, &pixels);
                put::image_end(out);
            }
            10 => {
                let bytes = jpeg(take, wild);
                let masked = take.u8().is_multiple_of(4);
                if masked {
                    put::image_begin(out, ImageCoding::Pixels { channels: 1 }, Why::JpegMask, 255, 2, 2, 4, &matrix(take, wild));
                    image_bytes(out, take, &[0, 128, 255, 7]);
                    put::image_end(out);
                }
                let why = if masked {
                    Why::JpegOverMask
                } else {
                    take.pick(&[Why::Image, Why::JpegHeaderFixed])
                };
                put::image_begin(out, ImageCoding::Jpeg, why, take.u8(), 1, 1, bytes.len() as u64, &matrix(take, wild));
                image_bytes(out, take, &bytes);
                put::image_end(out);
            }
            11 => {
                let code = take.pick(&[
                    NoteCode::NotFinite,
                    NoteCode::HiddenLayer,
                    NoteCode::JpegTurned,
                    NoteCode::JpegCutAsPng,
                    NoteCode::ObjectsSkipped,
                ]);
                put::note(out, code, take.u32());
            }
            _ => frames.push(Vec::new()),
        }
    }
    // A stream that keeps to the rules ends with its groups closed.
    if !wild {
        for _ in 0..groups {
            put::group_end(frames.last_mut().unwrap_or_else(|| unreachable!()));
        }
    }
    frames
}

/// Catches: a panic or a bomb in the stream's rules, the writer or the JPEG scan; an SVG past its cap; and anything a worker said reaching the document as markup, a script, a link or a way out of it.
pub fn check(data: &[u8]) {
    let mut take = Take(data);
    let route = take.u8() % 4;
    let (strip, turns) = (take.u8() & 1 == 1, take.u8() & 3);
    within("writing what a worker sent as SVG", BUDGET, || match route {
        0 => write(&[take.rest()], strip, turns),
        1 | 2 => {
            let frames = made_stream(&mut take);
            let frames: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
            write(&frames, strip, turns);
        }
        _ => jpegs(&mut take),
    });
}

fn write(frames: &[&[u8]], strip: bool, turns: u8) {
    let Ok(svg) = recto_agent::svg_in_memory(frames, strip, turns, CAP) else {
        return;
    };
    assert!(svg.len() as u64 <= CAP, "{} bytes past the cap", svg.len());
    recto_svg::verify::check_document(&svg);
}

/// The JPEG reading of both sides, on the bytes as they are and on a JPEG made from them.
fn jpegs(take: &mut Take<'_>) {
    let wild = take.u8() & 1 == 1;
    let made = jpeg(take, wild);
    let raw = take.rest();
    for bytes in [&made[..], raw] {
        for strip in [false, true] {
            let mut scan = JpegScan::new(strip);
            let mut out = Vec::new();
            let mut fed = true;
            for piece in bytes.chunks(1 + bytes.len() / 3) {
                if scan.feed(piece, &mut out).is_err() {
                    fed = false;
                    break;
                }
            }
            if fed && scan.finish().is_ok() {
                let _ = scan.facts();
                if !strip {
                    assert!(out.is_empty() || out == bytes, "a scan that doesn't strip changed the bytes");
                }
                assert!(out.len() <= bytes.len() || !strip, "stripping grew a JPEG");
                // What stripping kept reads as a JPEG again, and stripping it again takes nothing more.
                if strip && !out.is_empty() {
                    let mut again = JpegScan::new(true);
                    let mut twice = Vec::new();
                    assert!(
                        again.feed(&out, &mut twice).is_ok() && again.finish().is_ok(),
                        "what stripping kept isn't a JPEG"
                    );
                    assert_eq!(twice, out, "stripping twice took more");
                }
            }
        }
        let _ = (scan_start(bytes), frame_components(bytes), profile_says_srgb(bytes));
        let profile = [&[0u8; 128][..], &3u32.to_be_bytes(), bytes].concat();
        let _ = profile_says_srgb(&profile);
        for fix in [
            HeaderFix {
                upright: true,
                ..HeaderFix::default()
            },
            HeaderFix {
                drop_profile: true,
                add_profile: Some(&profile),
                ..HeaderFix::default()
            },
        ] {
            if let Some(fixed) = fix_header(bytes, &fix) {
                let (Some(from), Some(to)) = (scan_start(bytes), scan_start(&fixed)) else {
                    panic!("a fixed header with no scan");
                };
                assert_eq!(&bytes[from..], &fixed[to..], "a header fix changed the picture");
            }
        }
    }
}
