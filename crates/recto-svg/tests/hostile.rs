//! The writer against hostile input, read back by a small strict checker: what comes out must be one well-formed document, with nothing in it that can run or reach outside.

use recto_svg::verify::check_document;
use recto_svg::{GenericFamily, GradientStop, ImageData, Matrix, Paint, PathStyle, PngChannels, Rgb, Seg, SegKind, Stroke, SvgWriter, TextStyle};

/// Numbers that hostile input can reach us with, all finite or not.
const AWKWARD: &[f32] = &[
    0.0,
    1.0,
    -1.0,
    0.5,
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
    1e-30,
    1e30,
    3.25,
    -7.125,
];

fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *seed >> 32
}

fn awkward(seed: &mut u64) -> f32 {
    AWKWARD[next(seed) as usize % AWKWARD.len()]
}

fn hostile_strings() -> Vec<&'static str> {
    vec![
        "</svg><script>alert(1)</script>",
        "\"/><script href=\"http://evil/x.js\"/>",
        "' onload='alert(1)",
        "]]><![CDATA[<script>",
        "<!-- <!DOCTYPE x [<!ENTITY y SYSTEM \"file:///etc/passwd\">]> &y;",
        "<?xml-stylesheet href=\"http://evil\"?>",
        "&#x3c;script&#x3e;",
        "\u{0}\u{1}\u{8}\u{B}\u{C}\u{E}\u{1F}\u{7F}\u{85}\u{FFFE}\u{FFFF}\u{202E}gnp.exe\u{202C}\u{E0041}\u{2067}",
        "javascript:alert(1)",
    ]
}

/// Failure: text from a PDF breaks out of the XML, injects a tag or an attribute, or carries a character that makes the whole file invalid, so a title like `</svg><script>` runs when the file is opened.
#[test]
fn hostile_strings_cannot_break_out_anywhere() {
    for hostile in hostile_strings() {
        let mut out = Vec::new();
        let mut svg = SvgWriter::begin(&mut out, 100.0, 100.0, &Matrix::IDENTITY, Some(hostile)).unwrap();
        let chars: Vec<(char, f32, f32)> = hostile.chars().enumerate().map(|(i, ch)| (ch, i as f32 * 500.0, 0.0)).collect();
        let style = TextStyle {
            fill: Some((Rgb(0, 0, 0), 255)),
            family: hostile.as_bytes(),
            generic: GenericFamily::Serif,
            bold: true,
            italic: true,
        };
        svg.text_run(&Matrix::new(0.01, 0.0, 0.0, 0.01, 10.0, 10.0), &style, chars.iter().copied())
            .unwrap();
        let hidden = TextStyle {
            fill: None,
            family: b"ABCDEF+Real'\"><script>Name",
            generic: GenericFamily::Monospace,
            bold: false,
            italic: false,
        };
        svg.text_run(&Matrix::IDENTITY, &hidden, chars.iter().copied()).unwrap();
        svg.finish().unwrap();
        check_document(&out);
        let written = String::from_utf8(out).unwrap();
        assert!(!written.contains("<script"), "{written}");
        assert!(!written.contains("<!"), "{written}");
        assert!(!written.contains("onload='"), "{written}");
    }
}

/// Failure: the positions and characters of a run fall out of step once some characters are dropped, so the rest of a line is drawn in the wrong places.
#[test]
fn dropped_characters_take_their_positions_with_them() {
    let chars = [
        ('a', 10.0, 0.0),
        ('\u{202E}', 20.0, 0.0),
        ('b', 30.0, 0.0),
        ('\u{1}', 40.0, 0.0),
        ('c', 50.0, 0.0),
    ];
    let mut out = Vec::new();
    let mut svg = SvgWriter::begin(&mut out, 10.0, 10.0, &Matrix::IDENTITY, None).unwrap();
    let style = TextStyle {
        fill: None,
        family: b"F",
        generic: GenericFamily::Serif,
        bold: false,
        italic: false,
    };
    svg.text_run(&Matrix::IDENTITY, &style, chars.into_iter()).unwrap();
    assert_eq!(svg.stats().chars_dropped, 2);
    svg.finish().unwrap();
    let written = String::from_utf8(out).unwrap();
    assert!(written.contains("x=\"10 30 50\"") && written.contains(">abc</text>"), "{written}");
}

/// Failure: a NaN or an infinite number from a hostile page reaches the file, and a reader refuses the whole document or draws nothing past that point. Every number anywhere must stay a plain number, whatever comes in, on every call.
#[test]
fn hostile_numbers_stay_numbers_on_every_call() {
    let mut seed = 42;
    for _ in 0..400 {
        let mut out = Vec::new();
        let m = |seed: &mut u64| {
            Matrix::new(
                f64::from(awkward(seed)),
                f64::from(awkward(seed)),
                f64::from(awkward(seed)),
                f64::from(awkward(seed)),
                f64::from(awkward(seed)),
                f64::from(awkward(seed)),
            )
        };
        let root = m(&mut seed);
        let mut svg = SvgWriter::begin(&mut out, f64::from(awkward(&mut seed)), f64::from(awkward(&mut seed)), &root, None).unwrap();
        for _ in 0..8 {
            let segs: Vec<Seg> = (0..8)
                .map(|_| Seg {
                    kind: [SegKind::Move, SegKind::Line, SegKind::Curve][next(&mut seed) as usize % 3],
                    close: next(&mut seed).is_multiple_of(4),
                    x: awkward(&mut seed),
                    y: awkward(&mut seed),
                })
                .collect();
            let stroke = Stroke {
                paint: Paint::Solid(Rgb(1, 2, 3), 128),
                width: f64::from(awkward(&mut seed)),
                cap: next(&mut seed) as u8,
                join: next(&mut seed) as u8,
                dashes: vec![awkward(&mut seed), awkward(&mut seed)],
                dash_phase: f64::from(awkward(&mut seed)),
            };
            let style = PathStyle {
                fill: Paint::Solid(Rgb(9, 9, 9), next(&mut seed) as u8),
                even_odd: next(&mut seed).is_multiple_of(2),
                stroke: next(&mut seed).is_multiple_of(2).then_some(stroke),
            };
            svg.path(&style, &m(&mut seed), segs.iter().copied()).unwrap();
            svg.clip_define(
                1 + next(&mut seed) as u32 % 100,
                next(&mut seed) as u32 % 100,
                &m(&mut seed),
                segs.iter().copied(),
            )
            .unwrap();
            svg.set_clip(next(&mut seed) as u32 % 100).unwrap();
            svg.glyph(next(&mut seed) as u32, segs.iter().copied()).unwrap();
            svg.glyph_run(
                &m(&mut seed),
                Paint::Solid(Rgb(0, 0, 0), 255),
                Some((Paint::Solid(Rgb(0, 0, 0), 255), f64::from(awkward(&mut seed)))),
                [(1, awkward(&mut seed), awkward(&mut seed))],
            )
            .unwrap();
            let style = TextStyle {
                fill: None,
                family: b"Font",
                generic: GenericFamily::SansSerif,
                bold: false,
                italic: false,
            };
            svg.text_run(
                &m(&mut seed),
                &style,
                [('x', awkward(&mut seed), awkward(&mut seed)), ('y', awkward(&mut seed), awkward(&mut seed))].into_iter(),
            )
            .unwrap();
            svg.linear_gradient(
                1,
                (f64::from(awkward(&mut seed)), 0.0),
                (0.0, f64::from(awkward(&mut seed))),
                &[GradientStop {
                    offset: f64::from(awkward(&mut seed)),
                    color: Rgb(1, 1, 1),
                    opacity: 7,
                }],
            )
            .unwrap();
            svg.radial_gradient(2, (f64::from(awkward(&mut seed)), 0.0), f64::from(awkward(&mut seed)), &[])
                .unwrap();
            svg.group_begin(next(&mut seed) as u8).unwrap();
            svg.image_begin(ImageData::Jpeg, &m(&mut seed), 200, true).unwrap();
            svg.image_bytes(&[0xFF, 0xD8, 0xFF]).unwrap();
            svg.image_end().unwrap();
            svg.group_end().unwrap();
        }
        svg.finish().unwrap();
        check_document(&out);
    }
}

/// Failure: nesting from a hostile page turns into nesting in the file, and a reader's own stack is what gives out. It's refused here, past a fixed depth.
#[test]
fn groups_nest_only_so_far_and_every_way_out_closes_them() {
    let mut out = Vec::new();
    let mut svg = SvgWriter::begin(&mut out, 10.0, 10.0, &Matrix::IDENTITY, None).unwrap();
    for _ in 0..recto_svg_depth() {
        svg.group_begin(128).unwrap();
        svg.set_clip(3).unwrap();
    }
    assert!(svg.group_begin(128).is_err(), "past the depth");
    svg.finish().unwrap();
    check_document(&out);
    assert!(
        SvgWriter::begin(Vec::new(), 1.0, 1.0, &Matrix::IDENTITY, None).unwrap().group_end().is_err(),
        "nothing to close"
    );
}

fn recto_svg_depth() -> usize {
    recto_svg::MAX_GROUP_DEPTH
}

/// Failure: an image's bytes are cut off or run over, and what's written isn't the image: the writer refuses rather than closing a broken picture.
#[test]
fn an_image_short_of_rows_is_refused_and_bytes_outside_an_image_too() {
    let mut svg = SvgWriter::begin(Vec::new(), 10.0, 10.0, &Matrix::IDENTITY, None).unwrap();
    assert!(svg.image_bytes(b"x").is_err(), "no image is open");
    assert!(svg.image_end().is_err(), "no image is open");
    svg.image_begin(
        ImageData::Pixels {
            width: 2,
            height: 2,
            channels: PngChannels::Rgb,
        },
        &Matrix::IDENTITY,
        255,
        false,
    )
    .unwrap();
    svg.image_bytes(&[0; 6]).unwrap();
    assert!(svg.image_end().is_err(), "a row short");
}

/// A writer that says how much it's been given so far, and what it was given last, so a test can see what the writer is still holding.
#[derive(Clone, Default)]
struct Counting {
    total: std::rc::Rc<std::cell::Cell<u64>>,
    tail: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
}

impl std::io::Write for Counting {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.total.set(self.total.get() + data.len() as u64);
        *self.tail.borrow_mut() = data[data.len().saturating_sub(16)..].to_vec();
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Failure: the writer holds what it's given, so a page's output sits in memory until the end, where a huge page takes the machine with it. After every path, only a small buffer's worth may still be held, however long the page.
#[test]
fn what_goes_in_goes_out_as_it_goes() {
    let gone = Counting::default();
    let mut svg = SvgWriter::begin(gone.clone(), 100.0, 100.0, &Matrix::IDENTITY, None).unwrap();
    let style = PathStyle {
        fill: Paint::Solid(Rgb(0, 0, 0), 255),
        even_odd: false,
        stroke: None,
    };
    let segs: Vec<Seg> = (0..2000)
        .map(|i| Seg {
            kind: if i == 0 { SegKind::Move } else { SegKind::Line },
            close: false,
            x: i as f32,
            y: (i % 17) as f32,
        })
        .collect();
    let mut alone = Vec::new();
    let mut one = SvgWriter::begin(&mut alone, 100.0, 100.0, &Matrix::IDENTITY, None).unwrap();
    one.path(&style, &Matrix::IDENTITY, segs.iter().copied()).unwrap();
    one.finish().unwrap();
    let one_path = alone.len() as u64 - 200;
    for round in 1..=200u64 {
        svg.path(&style, &Matrix::IDENTITY, segs.iter().copied()).unwrap();
        assert!(
            gone.total.get() + 2 * 64 * 1024 >= one_path * round,
            "only {} of about {} has gone out",
            gone.total.get(),
            one_path * round
        );
    }
    svg.finish().unwrap();
    assert!(gone.total.get() > 1_000_000, "{}", gone.total.get());
    assert!(gone.tail.borrow().ends_with(b"</svg>\n"));
}

/// Failure: a long path drifts because each rounded step adds its error to the next, so its far end lands visibly off where it should. Read back and summed, every point must stay within the grid.
#[test]
fn a_long_path_does_not_drift() {
    let mut out = Vec::new();
    let mut svg = SvgWriter::begin(&mut out, 10.0, 10.0, &Matrix::IDENTITY, None).unwrap();
    let points: Vec<(f32, f32)> = (0..20_000)
        .map(|i| (i as f32 * 0.0137 + 0.0004, ((i * 7) % 113) as f32 * 0.0091 + 0.0003))
        .collect();
    let segs = points.iter().enumerate().map(|(i, &(x, y))| Seg {
        kind: if i == 0 { SegKind::Move } else { SegKind::Line },
        close: false,
        x,
        y,
    });
    svg.path(
        &PathStyle {
            fill: Paint::None,
            even_odd: false,
            stroke: None,
        },
        &Matrix::IDENTITY,
        segs,
    )
    .unwrap();
    svg.finish().unwrap();
    let written = String::from_utf8(out).unwrap();
    let start = written.find(" d=\"").unwrap() + 4;
    let d = &written[start..start + written[start..].find('"').unwrap()];
    let walked = read_d(d);
    assert_eq!(walked.len(), points.len());
    for (read, want) in walked.iter().zip(&points) {
        assert!(
            (read.0 - f64::from(want.0)).abs() < 0.002 && (read.1 - f64::from(want.1)).abs() < 0.002,
            "{read:?} against {want:?}"
        );
    }
}

/// Failure: a line of no width, or one that rounds to none, is written as `stroke-width="0"`, which SVG draws as nothing where PDF draws a hairline. Maps and CAD drawings lose their lines. A thin one that can be written stays what it is.
#[test]
fn a_stroke_too_thin_to_write_is_a_hairline() {
    let stroked = |width: f64, matrix: &Matrix| {
        let mut out = Vec::new();
        let mut svg = SvgWriter::begin(&mut out, 10.0, 10.0, &Matrix::IDENTITY, None).unwrap();
        let stroke = Stroke {
            paint: Paint::Solid(Rgb(0, 0, 0), 255),
            width,
            cap: 0,
            join: 0,
            dashes: Vec::new(),
            dash_phase: 0.0,
        };
        let segs = [
            Seg {
                kind: SegKind::Move,
                close: false,
                x: 0.0,
                y: 0.0,
            },
            Seg {
                kind: SegKind::Line,
                close: false,
                x: 5.0,
                y: 5.0,
            },
        ];
        svg.path(
            &PathStyle {
                fill: Paint::None,
                even_odd: false,
                stroke: Some(stroke),
            },
            matrix,
            segs,
        )
        .unwrap();
        svg.finish().unwrap();
        check_document(&out);
        String::from_utf8(out).unwrap()
    };
    let hairline = "vector-effect=\"non-scaling-stroke\" stroke-width=\"1\"";
    for width in [0.0, 0.00001] {
        let written = stroked(width, &Matrix::IDENTITY);
        assert!(written.contains(hairline), "{width}: {written}");
        assert!(!written.contains("stroke-width=\"0\""), "{written}");
    }
    let shrunk = stroked(1.0, &Matrix::new(0.00001, 0.0, 0.0, 0.00001, 0.0, 0.0));
    assert!(shrunk.contains(hairline), "a pen scaled away to nothing: {shrunk}");
    let thin = stroked(0.1, &Matrix::IDENTITY);
    assert!(!thin.contains("vector-effect") && thin.contains("stroke-width=\".1\""), "{thin}");
}

/// Failure: a curve comes back with its control points wrong, or a move, a line or a close changes meaning through the compact form. Read back, it's the same shape.
#[test]
fn every_kind_of_segment_reads_back_as_itself() {
    let mut out = Vec::new();
    let mut svg = SvgWriter::begin(&mut out, 10.0, 10.0, &Matrix::IDENTITY, None).unwrap();
    let seg = |kind, x, y, close| Seg { kind, close, x, y };
    let segs = [
        seg(SegKind::Move, 1.0, 2.0, false),
        seg(SegKind::Line, 5.0, 2.0, false),
        seg(SegKind::Line, 5.0, 9.0, false),
        seg(SegKind::Line, -3.5, -2.25, false),
        seg(SegKind::Curve, 1.0, 1.0, false),
        seg(SegKind::Curve, 2.0, 2.0, false),
        seg(SegKind::Curve, 3.0, 1.5, true),
        seg(SegKind::Line, 4.0, 4.0, false),
        seg(SegKind::Move, 7.0, 7.0, false),
        seg(SegKind::Line, 8.0, 8.0, true),
    ];
    svg.path(
        &PathStyle {
            fill: Paint::Solid(Rgb(0, 0, 0), 255),
            even_odd: true,
            stroke: None,
        },
        &Matrix::IDENTITY,
        segs,
    )
    .unwrap();
    svg.finish().unwrap();
    let written = String::from_utf8(out).unwrap();
    let start = written.find(" d=\"").unwrap() + 4;
    let d = &written[start..start + written[start..].find('"').unwrap()];
    let want = [
        (1.0, 2.0),
        (5.0, 2.0),
        (5.0, 9.0),
        (-3.5, -2.25),
        (1.0, 1.0),
        (2.0, 2.0),
        (3.0, 1.5),
        (4.0, 4.0),
        (7.0, 7.0),
        (8.0, 8.0),
    ];
    let walked = read_d(d);
    assert_eq!(walked.len(), want.len(), "{d}");
    for (read, want) in walked.iter().zip(&want) {
        assert!(
            (read.0 - want.0).abs() < 1e-3 && (read.1 - want.1).abs() < 1e-3,
            "{read:?} against {want:?} in {d}"
        );
    }
    assert_eq!(d.matches('z').count(), 2, "{d}");
}

/// A small reader of `d`: every point it passes through, control points included, in order.
fn read_d(d: &str) -> Vec<(f64, f64)> {
    let bytes = d.as_bytes();
    let mut at = 0;
    let (mut x, mut y) = (0.0, 0.0);
    let (mut sx, mut sy) = (0.0, 0.0);
    let mut command = b' ';
    let mut out = Vec::new();
    let number = |at: &mut usize| -> f64 {
        while bytes[*at] == b' ' {
            *at += 1;
        }
        let start = *at;
        if bytes[*at] == b'-' {
            *at += 1;
        }
        let mut seen_point = false;
        while *at < bytes.len() && (bytes[*at].is_ascii_digit() || (bytes[*at] == b'.' && !seen_point)) {
            seen_point |= bytes[*at] == b'.';
            *at += 1;
        }
        d[start..*at].parse().unwrap_or_else(|_| panic!("a number at {start} in {d}"))
    };
    while at < bytes.len() {
        if bytes[at].is_ascii_alphabetic() {
            command = bytes[at];
            at += 1;
            if command == b'z' {
                (x, y) = (sx, sy);
                continue;
            }
        }
        match command {
            b'M' => {
                x = number(&mut at);
                y = number(&mut at);
                (sx, sy) = (x, y);
                out.push((x, y));
            }
            b'l' => {
                x += number(&mut at);
                y += number(&mut at);
                out.push((x, y));
            }
            b'h' => {
                x += number(&mut at);
                out.push((x, y));
            }
            b'v' => {
                y += number(&mut at);
                out.push((x, y));
            }
            b'c' => {
                let values: Vec<f64> = (0..6).map(|_| number(&mut at)).collect();
                out.push((x + values[0], y + values[1]));
                out.push((x + values[2], y + values[3]));
                x += values[4];
                y += values[5];
                out.push((x, y));
            }
            other => panic!("the command {}", other as char),
        }
    }
    out
}
