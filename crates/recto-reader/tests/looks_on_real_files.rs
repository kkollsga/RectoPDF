//! Dark paper's judgement through the real worker: on real files, skipped where the benchmark files aren't, as on CI (`paper.rs` holds the rule without them), and on made pages, from pictures of each size a look can be decided from. Nothing here races a memory sample: the one page taken to a lower resolution passes its limit by about twice over drawn sharp and stays under it by about half at the lower resolution. The steps that take a page there are held by the stand-in worker in `recto-render-service`'s `tests/sequence.rs`.

use recto_platform::surface::PixelSurface;
use recto_proto::{TileRect, device_size};
use recto_reader::engine::EngineEvents;
use recto_reader::paper::{DARK_PAPER, PageLook};
use recto_reader::tiles::{Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileRequest, TileResult};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages_with, text_lines};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BENCH: &str = "/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100";

struct Quiet;
impl EngineEvents for Quiet {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
}

fn view(y: f64) -> Viewport {
    Viewport {
        x: 0.0,
        y,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// A tile the host was told to attach to a page: where on the page, in points from its top left, and what's in it, each pixel of what was drawn as `0xRRGGBB`, row by row, or the one colour of a tile with no surface.
struct Shown {
    page: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    pixels: Option<(Vec<u32>, usize, usize)>,
    rgb: Option<u32>,
}

impl Shown {
    /// What's at a point of the page, if this tile covers it.
    fn at(&self, x: f64, y: f64) -> Option<u32> {
        if x < self.x || y < self.y || x >= self.x + self.width || y >= self.y + self.height {
            return None;
        }
        let Some((pixels, width, height)) = &self.pixels else { return self.rgb };
        let column = (((x - self.x) / self.width) * *width as f64) as usize;
        let row = (((y - self.y) / self.height) * *height as f64) as usize;
        pixels.get(row.min(height - 1) * width + column.min(width - 1)).copied()
    }

    /// Device pixels a point, as drawn.
    fn density(&self) -> f64 {
        self.pixels.as_ref().map_or(0.0, |(_, width, _)| *width as f64 / self.width)
    }
}

/// The tiles of `page` the engine has the host attach, on dark paper, shown from its top at `viewport`, and the engine's report once it's settled.
#[allow(unsafe_code)]
fn shown_on_dark_paper(file: &Path, pages: u32, page: u32, policy: Policy, viewport: Viewport) -> (Vec<Shown>, serde_json::Value) {
    let engine = Engine::new(
        EngineConfig {
            service: None,
            policy: Some(policy),
            keys: None,
        },
        Arc::new(Quiet),
    );
    let at = |y: f64| Viewport { y, ..viewport };
    engine.set_viewport(at(0.0));
    engine.set_paper(PaperKind::Dark);
    engine.activate(
        1,
        pages,
        page,
        PageMap {
            files: vec![file.to_path_buf()],
            slots: None,
            own: None,
            turns: None,
            expected: Vec::new(),
            follows: None,
        },
        &[],
        None,
        recto_reader::layout::Spread::Single,
    );
    let mut shown = Vec::new();
    let started = Instant::now();
    let mut y = 0.0;
    while started.elapsed() < Duration::from_secs(60) {
        let mut ops = engine.set_viewport(at(y));
        ops.extend(engine.drain());
        engine.applied();
        for op in ops {
            match op {
                Op::ScrollTo { y: to, .. } => y = to,
                Op::Attach {
                    x,
                    y: top,
                    width,
                    height,
                    surface,
                    rgb,
                    crop_width,
                    crop_height,
                    page: on,
                    ..
                } if on == page => {
                    // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                    let surface = unsafe { PixelSurface::from_host_reference(surface) };
                    let pixels = surface.and_then(|surface| {
                        let (drawn_width, drawn_height) = (
                            (crop_width * f64::from(surface.width())).round() as usize,
                            (crop_height * f64::from(surface.height())).round() as usize,
                        );
                        let stride = surface.stride() as usize;
                        surface
                            .with_pixels(|pixels| {
                                let mut colours = Vec::with_capacity(drawn_width * drawn_height);
                                for row in 0..drawn_height {
                                    for column in 0..drawn_width {
                                        let at = row * stride + column * 4;
                                        colours.push((u32::from(pixels[at + 2]) << 16) | (u32::from(pixels[at + 1]) << 8) | u32::from(pixels[at]));
                                    }
                                }
                                (colours, drawn_width, drawn_height)
                            })
                            .ok()
                    });
                    shown.push(Shown {
                        page: on,
                        x,
                        y: top,
                        width,
                        height,
                        pixels,
                        rgb,
                    });
                }
                _ => {}
            }
        }
        if engine.wait_settled(Duration::from_millis(10)) {
            break;
        }
    }
    let report = engine.report().0;
    assert_eq!(report["settled"], true, "{report}");
    (shown, report)
}

/// What's at the top left of the page, as red, green, blue, on dark paper: the colour of a tile with none of its own, or a surface's first pixel. And whether it came out at a lower resolution.
fn corner_on_dark_paper(file: &Path, pages: u32, page: u32, policy: Policy) -> (u32, bool) {
    let (shown, report) = shown_on_dark_paper(file, pages, page, policy, view(0.0));
    // A blank tile of the paper's own colour needs nothing put on a layer, so nothing's said about it.
    let corner = shown
        .iter()
        .rev()
        .find(|tile| tile.page == page && tile.x == 0.0 && tile.y == 0.0)
        .and_then(|tile| tile.at(0.0, 0.0));
    (corner.unwrap_or(DARK_PAPER), !report["low_res_pages"].as_array().unwrap().is_empty())
}

fn file(name: &str) -> Option<PathBuf> {
    let path = Path::new(BENCH).join(name);
    path.is_file().then_some(path)
}

/// Failure: a scanned page shows as a bright dimmed page beside text pages turned over, as the owner saw, or a map sheet whose map sits on white paper among its legends and tables is dimmed whole to light grey with black text, as the owner was shown of a photo over 53% of a page (2026-11-13: "The image should flip if the background is mostly white"). On a real scan and a real USGS sheet, drawn sharp through the engine: the scan's paper and the sheet's white margin both land on the dark paper's own tone, the sheet's map kept as a picture.
#[test]
fn a_real_scan_and_a_real_map_sheet_on_white_turn_over() {
    let (Some(scan), Some(map)) = (file("gov_nasa_ntrs20150023275.pdf"), file("gov_usgs_usgs70277338.pdf")) else {
        eprintln!("skipped: the benchmark files aren't here");
        return;
    };
    let (corner, low_res) = corner_on_dark_paper(&scan, 8, 2, Policy::default());
    assert!(!low_res);
    assert_eq!(corner, DARK_PAPER, "the scan's paper is the paper's tone: {corner:06x}");
    let (corner, low_res) = corner_on_dark_paper(&map, 1, 0, Policy::default());
    assert!(!low_res);
    assert_eq!(corner, DARK_PAPER, "the sheet's white margin turns over round its map: {corner:06x}");
}

/// A renderer of a file, with the real worker.
fn renderer(file: &Path) -> DocRenderer {
    DocRenderer::new(file, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap()
}

/// What's decided for a page from a picture of it whose longest side is `edge`, drawn by `doc`, with the images it's told of, or none if `whole`: dimmed, kept and turned, as a person sees it.
fn look_of(doc: &mut DocRenderer, page: u32, edge: f32, whole: bool) -> (bool, Vec<[u16; 4]>, Vec<[u16; 4]>) {
    let (width, height) = doc.page_size(page).unwrap();
    let scale = edge / width.max(height);
    let (w, h) = device_size(width, height, scale).unwrap();
    let surface = PixelSurface::create(w, h).unwrap();
    let request = TileRequest {
        page,
        rect: TileRect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        scale,
        stride: surface.stride(),
        generation: 1,
    };
    assert!(matches!(doc.render_tile(&request, &surface), Ok(TileResult::Done(_))));
    let boxes = if whole {
        None
    } else {
        Some(doc.image_boxes(page, 1).unwrap().expect("its images are known"))
    };
    let stride = surface.stride() as usize;
    let look = surface.with_pixels(|pixels| PageLook::decide(boxes.as_deref(), pixels, stride, w, h)).unwrap();
    (look.dim, look.keep, look.whiten.iter().map(|image| image.sides).collect())
}

/// Failure: the real USGS sheet, whose map sits on white among its legends and tables, is judged whole where its images are known, and so dimmed to light grey where the sharp page turned over; or judged by its images, it's dimmed. Decided in process from one picture of it at the look picture's size: with its images it turns over and keeps its map; judged whole, as a page is only when its images can't be had (a page whose drawing failed for time or a crash, a walk that was stopped, or one with too many pictures apart), it's dimmed, as its map can't be told from its paper. The real scan turns over either way.
#[test]
fn the_real_sheet_judged_by_its_images_turns_over_and_judged_whole_is_dimmed() {
    let (Some(scan), Some(map)) = (file("gov_nasa_ntrs20150023275.pdf"), file("gov_usgs_usgs70277338.pdf")) else {
        eprintln!("skipped: the benchmark files aren't here");
        return;
    };
    let (mut map, mut scan) = (renderer(&map), renderer(&scan));
    let (dim, keep, _) = look_of(&mut map, 0, 512.0, false);
    assert!(!dim && !keep.is_empty(), "by its images: dim {dim}, kept {keep:?}");
    assert!(look_of(&mut map, 0, 512.0, true).0, "judged whole");
    assert!(!look_of(&mut scan, 2, 512.0, false).0 && !look_of(&mut scan, 2, 512.0, true).0, "the scan");
}

/// Made pages, each with a photo, an image shaded through many colours, drawn on it: over 59% of a white page with text, over 59% of a navy page, and over 30% of a white page under a coloured band.
fn photo_pages() -> TempPdf {
    let (w, h) = (120usize, 90usize);
    let mut photo = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            photo.extend_from_slice(&if y % 40 < 22 {
                [70 + (x % 60) as u8, 130 + (y % 50) as u8, 220 - (x % 40) as u8]
            } else {
                [30 + (x % 20) as u8, 90 + (x * 3 % 70) as u8, 40]
            });
        }
    }
    let lines: Vec<String> = (0..48)
        .map(|at| format!("Line {at} of the text that runs down the page beside the photograph."))
        .collect();
    let text = text_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>());
    let pages = [
        [text.clone(), b"q 459 0 0 620 76 86 cm /Im0 Do Q".to_vec()].concat(),
        [b"0.12 0.2 0.4 rg 0 0 612 792 re f\n".to_vec(), b"q 459 0 0 620 76 86 cm /Im0 Do Q".to_vec()].concat(),
        [
            b"0.8 0.3 0.1 rg 0 700 612 92 re f\n".to_vec(),
            text,
            b"q 400 0 0 360 150 150 cm /Im0 Do Q".to_vec(),
        ]
        .concat(),
    ];
    let list: Vec<Page> = pages.into_iter().map(Page::new).collect();
    let image = first_extra_object(list.len());
    TempPdf::new(
        "photos.pdf",
        &pages_with(
            &list,
            &format!("/XObject << /Im0 {image} 0 R >>"),
            vec![Object::Stream {
                dict: format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /BitsPerComponent 8 /ColorSpace /DeviceRGB"),
                data: photo,
            }],
        ),
    )
}

/// Failure: a page's look on dark paper hangs on the size of the picture it's decided from: the largest preview's 1,144, the look picture's 512, or the look picture at a lower resolution's 128, as a page too heavy for its limit gets. A renderer dropped and made again, as a sleeping tab's is, decides from whichever of those is drawn first (review of 2026-11-13: the USGS sheet turned over, then went light grey after a wake), so a look that changed with the size would change across a sleep. Each made page, decided from the real worker's pictures at the three sizes and the boxes it gives, comes out the same: turned over with its photo kept on white, dimmed on navy, turned over under half on white with a band. One renderer draws all three sizes: what a fresh one would answer is the same walk and the same picture, so a renderer each (nine workers) held nothing more. That a page drawn at a lower resolution is decided with its images known, through the engine, is held by `a_page_at_a_lower_resolution_keeps_its_picture_and_turns_its_paper_over`.
#[test]
fn a_page_has_the_same_look_from_any_size_of_picture_of_it() {
    let file = photo_pages();
    let mut doc = renderer(&file.0);
    let expected = [(false, 1), (true, 0), (false, 1)];
    for (page, (dim, kept)) in expected.into_iter().enumerate() {
        let looks: Vec<_> = [1144.0, 512.0, 128.0]
            .into_iter()
            .map(|edge| look_of(&mut doc, page as u32, edge, false))
            .collect();
        assert!(looks.iter().all(|look| *look == looks[0]), "page {}: {looks:?}", page + 1);
        assert_eq!((looks[0].0, looks[0].1.len()), (dim, kept), "page {}: {:?}", page + 1, looks[0]);
    }
}

/// A page 612 by 1,600 points with a picture across its top 480 points, under a third of it: one solid colour, `0xC81E1E`, 5,000 by 4,096 pixels, so the page is heavy and drawn in regions, inside an isolated transparency group, which PDFium draws through a bitmap of its own the size of what it covers. Drawn sharp, a region of the window's size, that bitmap takes a worker to about 39 MB; at a quarter of the scale about 10, and its look picture about 7 (measured, debug, 2026-11-13).
fn heavy_picture_page() -> TempPdf {
    let (w, h) = (5_000usize, 4_096usize);
    // One bit a pixel, all of them the first colour of the two: a run of 128 zero bytes is two.
    let mut data = Vec::new();
    let mut left = w.div_ceil(8) * h;
    while left > 0 {
        let run = left.min(128);
        data.extend_from_slice(&[(257 - run) as u8, 0]);
        left -= run;
    }
    data.push(128);
    let mut page = Page::new(b"q /Group Do Q".to_vec());
    page.media = (612.0, 1_600.0);
    let image = first_extra_object(1);
    TempPdf::new(
        "heavy-picture.pdf",
        &pages_with(
            &[page],
            &format!("/XObject << /Group {} 0 R >>", image + 1),
            vec![
                Object::Stream {
                    dict: format!(
                        "/Type /XObject /Subtype /Image /Width {w} /Height {h} /BitsPerComponent 1 /ColorSpace [/Indexed /DeviceRGB 1 <C81E1E1E50C8>] /Filter /RunLengthDecode"
                    ),
                    data,
                },
                Object::Stream {
                    dict: format!(
                        "/Type /XObject /Subtype /Form /BBox [0 0 612 1600] /Group << /S /Transparency /I true >> /Resources << /XObject << /Im0 {image} 0 R >> >>"
                    ),
                    data: b"q 612 0 0 480 0 1120 cm /Im0 Do Q".to_vec(),
                },
            ],
        ),
    )
}

/// Failure: a page drawn at a lower resolution, as a page too heavy for its worker's limit is, comes out on dark paper by a look that isn't its own: judged whole, with its images not asked, so dimmed to light grey, as the owner saw the USGS sheet do after a wake (review of 2026-11-13); or its kept picture placed by the sharp tile's rectangle and scale rather than the lower resolution's, so paper beside it is kept bright or the picture is turned into its negative. Through the engine and the real worker, made to go to a lower resolution without racing a memory sample: a normal limit of 20 MiB that the sharp region certainly passes (about 39 MB) and the lower resolution and the look picture certainly don't (about 10 and 7), with the larger limit no larger, so there's no heavy retry. Every pixel of what's shown inside the picture keeps its colour, dimmed as a kept picture is, and every pixel of paper outside it is the dark paper's own tone. Seen failing with the look decided with no boxes and with the boxes mapped by the sharp rectangle and scale.
#[test]
fn a_page_at_a_lower_resolution_keeps_its_picture_and_turns_its_paper_over() {
    let mib = 1024 * 1024;
    let policy = Policy {
        memory_limit_bytes: 20 * mib,
        heavy_limit_bytes: 20 * mib,
        heavy_min_free_percent: 0,
        ..Policy::default()
    };
    let window = Viewport {
        width: 1_400.0,
        height: 1_400.0,
        frame_width: 1_400.0,
        magnification: 2.0,
        ..view(0.0)
    };
    let file = heavy_picture_page();
    let (shown, report) = shown_on_dark_paper(&file.0, 1, 0, policy, window);
    assert_eq!(report["low_res_pages"].as_array().map(Vec::len), Some(1), "{report}");
    // Kept, each channel dimmed by the kept picture's 218 in 256, give or take a step of the renderer's.
    let kept = [200u32 * 218 / 256, 30 * 218 / 256, 30 * 218 / 256];
    let is_kept = |rgb: u32| {
        [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF]
            .iter()
            .zip(kept)
            .all(|(channel, want)| channel.abs_diff(want) <= 3)
    };
    let sharpest = shown.iter().map(Shown::density).fold(0.0, f64::max);
    let (mut inside, mut outside) = (0, 0);
    for tile in &shown {
        // What's drawn at a lower resolution is what's asked about: a quarter of the sharp scale or less.
        let low = tile.density() > 0.0 && tile.density() < 2.0;
        let mut y = tile.y + 0.5;
        while y < tile.y + tile.height {
            let mut x = tile.x + 0.5;
            while x < (tile.x + tile.width).min(612.0) {
                if let Some(rgb) = tile.at(x, y) {
                    // Two points clear of the picture's edge, where a pixel is all one or all the other.
                    if y < 478.0 {
                        assert!(is_kept(rgb), "inside the picture at ({x}, {y}): {rgb:06x}, density {}", tile.density());
                        inside += usize::from(low);
                    } else if y > 482.0 {
                        assert_eq!(rgb, DARK_PAPER, "paper at ({x}, {y}): {rgb:06x}, density {}", tile.density());
                        outside += usize::from(low);
                    }
                }
                x += 3.0;
            }
            y += 3.0;
        }
    }
    assert!(
        inside > 0 && outside > 0,
        "at a lower resolution, {inside} looked at inside and {outside} outside; sharpest {sharpest}: {report}"
    );
}
