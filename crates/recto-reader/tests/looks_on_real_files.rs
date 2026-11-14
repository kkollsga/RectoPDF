//! Dark paper's judgement through the real worker: on real files, skipped where the benchmark files aren't, as on CI (`paper.rs` holds the rule without them), and on made pages, from pictures of each size a look can be decided from. Nothing here races a memory sample: the one page taken to a lower resolution passes its limit by about twice over drawn sharp and stays under it by about half at the lower resolution. The steps that take a page there are held by the stand-in worker in `recto-render-service`'s `tests/sequence.rs`.

use recto_platform::surface::PixelSurface;
use recto_proto::{TileRect, device_size};
use recto_reader::engine::EngineEvents;
use recto_reader::layout::PageBox;
use recto_reader::paper::{DARK_PAPER, PageLook};
use recto_reader::tiles::{Op, PageMap, SleepShot, SleepView, Viewport};
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
fn shown_on_dark_paper(file: &Path, pages: u32, page: u32, policy: Policy, viewport: Viewport) -> (Vec<Shown>, serde_json::Value) {
    shown_on_dark_paper_after(file, pages, page, policy, viewport, |_| {})
}

/// The same, with `prepare` done to the engine before the tab's activated, as giving it a sleep picture is.
#[allow(unsafe_code)]
fn shown_on_dark_paper_after(
    file: &Path,
    pages: u32,
    page: u32,
    policy: Policy,
    viewport: Viewport,
    prepare: impl FnOnce(&Engine),
) -> (Vec<Shown>, serde_json::Value) {
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
    prepare(&engine);
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

/// Made pages, each with a photo, an image shaded through many colours, drawn on it: over 59% of a white page with text, over 59% of a navy page, over 30% of a white page under a coloured band, and over 30% of a navy page with text; and a black page of white text with no picture.
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
            text.clone(),
            b"q 400 0 0 360 150 150 cm /Im0 Do Q".to_vec(),
        ]
        .concat(),
        [
            b"0.12 0.2 0.4 rg 0 0 612 792 re f\n1 g\n".to_vec(),
            text.clone(),
            b"q 400 0 0 360 150 150 cm /Im0 Do Q".to_vec(),
        ]
        .concat(),
        [b"0 0 0 rg 0 0 612 792 re f\n1 g\n".to_vec(), text].concat(),
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

/// Failure: a page's look on dark paper hangs on the size of the picture it's decided from: the largest preview's 1,144, the look picture's 512, or the look picture at a lower resolution's 128, as a page too heavy for its limit gets. A renderer dropped and made again, as a sleeping tab's is, decides from whichever of those is drawn first (review of 2026-11-13: the USGS sheet turned over, then went light grey after a wake), so a look that changed with the size would change across a sleep. Each made page, decided from the real worker's pictures at the three sizes and the boxes it gives, comes out the same: turned over with its photo kept on white, dimmed on navy, turned over under half on white with a band, and, as its background is dark (owner, 2026-11-13), dimmed under half on navy and with no picture on black. One renderer draws all three sizes: what a fresh one would answer is the same walk and the same picture, so a renderer each (nine workers) held nothing more. That a page drawn at a lower resolution is decided with its images known, through the engine, is held by `a_page_at_a_lower_resolution_keeps_its_picture_and_turns_its_paper_over`.
#[test]
fn a_page_has_the_same_look_from_any_size_of_picture_of_it() {
    let file = photo_pages();
    let mut doc = renderer(&file.0);
    let expected = [(false, 1), (true, 0), (false, 1), (true, 0), (true, 0)];
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

/// A black slide with white text and no picture, and a white page of text, letter size.
fn slide_and_text_pages() -> TempPdf {
    let lines: Vec<String> = (0..30).map(|at| format!("Line {at} of the text on the page.")).collect();
    let text = text_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>());
    let pages = [[b"0 0 0 rg 0 0 612 792 re f\n1 g\n".to_vec(), text.clone()].concat(), text];
    let list: Vec<Page> = pages.into_iter().map(Page::new).collect();
    TempPdf::new("slide-and-text.pdf", &pages_with(&list, "", Vec::new()))
}

/// What's shown of a page at points clear of its text, on every tile drawn of it: its top left corner, the middle of its right margin and its foot.
fn background_shown(shown: &[Shown], page: u32, sharper_than: f64) -> Vec<u32> {
    let mut found = Vec::new();
    for tile in shown.iter().filter(|tile| tile.page == page && tile.density() > sharper_than) {
        for (x, y) in [(4.0, 4.0), (590.0, 400.0), (300.0, 780.0)] {
            found.extend(tile.at(x, y));
        }
    }
    found
}

fn luma(rgb: u32) -> u32 {
    (((rgb >> 16) & 0xFF) * 77 + ((rgb >> 8) & 0xFF) * 150 + (rgb & 0xFF) * 29) >> 8
}

/// Failure: a black slide with no picture is shown turned over, a white page, on a tile drawn before any picture of the whole page, because a page with no images was given a page of text's look without a look at its pixels, or was decided from one tile of it (owner, 2026-11-13: a page already dark is never turned over). Zoomed out past where a preview is drawn, the first job for a page is a tile: with the page in one tile its look is decided from that tile, and with it in several from a small picture of it. Either way every tile of the slide shown is dark, and every tile of a white page of text is turned over to the dark paper's tone as before. That no preview was drawn is checked, so these are the paths without one.
#[test]
fn zoomed_out_past_a_preview_a_dark_page_is_never_shown_light() {
    let file = slide_and_text_pages();
    // 0.12 puts a letter page in one tile at a backing of 2; 0.3 puts it in two across.
    for magnification in [0.12, 0.3] {
        let viewport = Viewport { magnification, ..view(0.0) };
        for (page, dark) in [(0, true), (1, false)] {
            let (shown, report) = shown_on_dark_paper(&file.0, 2, page, Policy::default(), viewport);
            assert_eq!(report["tiles"]["previews_attached"], 0, "at {magnification}: {report}");
            let seen = background_shown(&shown, page, 0.0);
            assert!(!seen.is_empty(), "page {} at {magnification}: nothing shown: {report}", page + 1);
            for rgb in seen {
                if dark {
                    assert!(luma(rgb) < 40, "the slide at {magnification}: {rgb:06x}");
                } else {
                    assert_eq!(rgb, DARK_PAPER, "the page of text at {magnification}: {rgb:06x}");
                }
            }
        }
    }
}

/// Failure (review of 2026-11-13, S4): a page with no images is decided from one tile of it, its top left, as when a tile that isn't the whole page is taken for one, so a white page whose dark band fills most of that tile is taken for a dark page and dimmed whole. Zoomed out just past where a preview is drawn, with the page two tiles across, a white page with a navy band across its top 330 points, 42% of it and 58% of its top left tile, turns over: its white is the dark paper's tone on every tile shown. That no preview was drawn is checked, as a page is decided from its preview when there is one. Seen failing with `covers_page` always saying yes, but only while the top left tile is the first drawn, as the order of a page's tiles isn't this test's to say: `engine/look/tests.rs`'s `only_a_tile_that_is_the_whole_page_decides_it` asks which picture decided, whatever the order.
#[test]
fn a_white_page_whose_band_fills_its_first_tile_turns_over() {
    let lines: Vec<String> = (0..20).map(|at| format!("Line {at} of the text under the band.")).collect();
    let mut content = b"0.13 0.23 0.42 rg 0 462 612 330 re f\n0 g\n".to_vec();
    content.extend(text_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>()));
    // The text from 440 points down, under the band.
    let content = String::from_utf8(content).unwrap().replace("72 720 Td", "72 440 Td").into_bytes();
    let file = TempPdf::new("band.pdf", &pages_with(&[Page::new(content)], "", Vec::new()));
    // At 0.37 a letter page is 0.92 pixels a point at a backing of 2, 566 pixels across, two tiles, its top left tile its top 557 points; a preview is drawn from 0.375 up.
    let viewport = Viewport {
        magnification: 0.37,
        ..view(0.0)
    };
    let (shown, report) = shown_on_dark_paper(&file.0, 1, 0, Policy::default(), viewport);
    assert_eq!(report["tiles"]["previews_attached"], 0, "{report}");
    assert_eq!(report["tiles"]["kept"], 0, "no preview drawn: {report}");
    assert!(
        shown.iter().filter(|tile| tile.page == 0).count() >= 2,
        "the page in more than one tile: {report}"
    );
    let seen: Vec<u32> = shown
        .iter()
        .filter(|tile| tile.page == 0)
        .flat_map(|tile| [(590.0, 500.0), (300.0, 780.0), (40.0, 700.0)].into_iter().filter_map(|(x, y)| tile.at(x, y)))
        .collect();
    assert!(!seen.is_empty(), "nothing shown of the white part: {report}");
    assert!(seen.iter().all(|rgb| *rgb == DARK_PAPER), "turned over to the paper's tone: {seen:06x?}");
}

/// Failure: a tab woken onto its sleep picture, where the slide shows dark, then turns the slide light on the first sharp tile, as a page with no images got a page of text's look when no picture of the whole page was at hand: the sleep picture stands for the page's preview, so the first job is a tile, on a renderer that has decided nothing. Every sharper tile of the slide shown after the sleep picture is dark, and the white page of text turns over as before.
#[test]
fn woken_onto_a_sleep_picture_a_dark_page_stays_dark() {
    let file = slide_and_text_pages();
    // The version a worker says the file is, as a sleep file keeps it: a picture of no known version gives way to a preview.
    let version = {
        let mut doc = renderer(&file.0);
        doc.page_size(0).unwrap();
        doc.known_version()
    };
    assert!(version.is_some());
    for (page, dark) in [(0, true), (1, false)] {
        let picture = PixelSurface::create(306, 396).unwrap();
        let colour = if dark { [0u8, 0, 0, 0xFF] } else { [0x23, 0x20, 0x1E, 0xFF] };
        picture
            .with_pixels(|pixels| pixels.chunks_mut(4).for_each(|pixel| pixel.copy_from_slice(&colour)))
            .unwrap();
        let view_of_sleep = SleepView {
            version,
            usual: Some(PageBox { width: 612.0, height: 792.0 }),
            sizes: Vec::new(),
            pictures: vec![SleepShot {
                page,
                scale: 0.5,
                paper: PaperKind::Dark,
                surface: picture,
            }],
        };
        let (shown, report) = shown_on_dark_paper_after(&file.0, 2, page, Policy::default(), view(0.0), |engine| {
            engine.waking().note_wake(1, Instant::now());
            engine.waking().give_sleep(1, view_of_sleep);
        });
        assert_eq!(report["last_wake"]["from_sleep"], true, "{report}");
        // No preview was drawn, at about 1 a point here: the sleep picture stood for it, so the first job was a tile.
        assert!(
            !shown.iter().any(|tile| tile.density() > 0.6 && tile.density() < 1.5),
            "a preview was drawn: {:?}",
            shown.iter().map(Shown::density).collect::<Vec<_>>()
        );
        // Sharper than the sleep picture's 0.5 a point: what was drawn after it.
        let seen = background_shown(&shown, page, 0.6);
        assert!(!seen.is_empty(), "page {}: nothing sharp shown: {report}", page + 1);
        for rgb in seen {
            if dark {
                assert!(luma(rgb) < 40, "the slide: {rgb:06x}");
            } else {
                assert_eq!(rgb, DARK_PAPER, "the page of text: {rgb:06x}");
            }
        }
    }
}

/// Failure: the real USGS cover `70278165`, a photo under half of a black page, turns over on dark paper and comes out a white page round its photo (owner, 2026-11-13: a page already dark is never turned over). Decided from the look picture, with its images known and judged whole, it's shown as it is, dimmed.
#[test]
fn the_real_black_cover_is_never_turned_over() {
    let Some(cover) = file("gov_usgs_usgs70278165.pdf") else {
        eprintln!("skipped: the benchmark files aren't here");
        return;
    };
    let mut cover = renderer(&cover);
    for whole in [false, true] {
        let (dim, keep, whiten) = look_of(&mut cover, 0, 512.0, whole);
        assert!(
            dim && keep.is_empty() && whiten.is_empty(),
            "judged whole {whole}: dim {dim}, kept {keep:?}, turned {whiten:?}"
        );
    }
}

/// Failure: the owner's own example of a vector drawing (`today-shots.sh darkimg`, page 4: a light sky in bands, green ground, a red house and a sun, across 56% of a white page with text, drawn in vectors with no image), which the owner wanted left turning over, is taken for a dark page and dimmed whole, as a dark figure in vectors over half a white page is: its ground and house are dark, about 0.19 of the page. Decided through the real worker from the look picture, as a page with no images is when nothing drawn covers it, it turns over as a page of text does.
#[test]
fn the_owners_vector_drawing_turns_over() {
    let mut drawing =
        String::from("BT /F1 18 Tf 72 740 Td (Page 4: a vector drawing, 56% of the page, with text) Tj ET\n0.30 0.55 0.90 rg 72 400 468 300 re f\n");
    for band in 0..12 {
        drawing.push_str(&format!(
            "{:.2} {:.2} {:.2} rg 72 {} 468 25 re f\n",
            0.35 + f64::from(band) * 0.04,
            0.6 + f64::from(band) * 0.025,
            0.92 - f64::from(band) * 0.02,
            400 + band * 25
        ));
    }
    drawing.push_str("0.15 0.50 0.20 rg 72 220 468 180 re f\n0.75 0.15 0.15 rg 200 250 120 110 re f\n0.25 0.15 0.08 rg 245 250 30 60 re f\n0.98 0.85 0.25 rg 420 600 m 460 600 l 460 640 l 420 640 l f\n0 g\n");
    drawing.push_str("BT /F1 14 Tf 72 195 Td (Figure 4. Drawn in vectors, no image.) Tj ET\n");
    for line in 0..6 {
        drawing.push_str(&format!(
            "BT /F1 11 Tf 72 {} Td (Line {}: the quick brown fox jumps over the lazy dog, and the dog sleeps.) Tj ET\n",
            170 - line * 15,
            line + 1
        ));
    }
    let file = TempPdf::new("drawing.pdf", &pages_with(&[Page::new(drawing.into_bytes())], "", Vec::new()));
    let mut doc = renderer(&file.0);
    assert_eq!(doc.image_boxes(0, 1).unwrap().map(|boxes| boxes.len()), Some(0), "no images");
    assert_eq!(
        look_of(&mut doc, 0, 512.0, false),
        (false, Vec::new(), Vec::new()),
        "it turns over as a page of text"
    );
}

/// **Where the dark rule's line is** (second review of 2026-11-14, the coordinator's word: "a line has to sit somewhere"): a white page with a dark navy sidebar down its left side beside a photo over 45% of it running off its right side, with a title and a column of text under the photo, as `look_survey.rs`'s made pages p5 and p18 are. With the sidebar 30% wide the page turns over, keeping its photo; a third wide, the page is dark and dimmed whole, as the photo takes the page's white side out of its ring and the sidebar is then half of it (0.48 and 0.51 at the look picture's size). Failure: the ring's share changes and the line moves unseen, either way: seen failing with the ring's line at 0.52 (the third turned over) and at 0.47 (the 30% page dimmed); its width at 0.03 or 0.08 of each side leaves this line where it is, the sidebar running the page's height. Decided through the real worker from the look picture, at 512, as a page with images is when no preview covers it; at 128 the third is under half, which is what being on the line is.
#[test]
fn a_dark_sidebar_beside_a_photo_is_on_the_line_at_a_third_of_the_page() {
    let (w, h) = (120usize, 140usize);
    let mut photo = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            photo.extend_from_slice(&if y < h * 55 / 100 {
                [70 + (y * 120 / h) as u8, 130 + (y * 110 / h) as u8, 230 - (y * 40 / h) as u8]
            } else {
                [30, 90 + (70 * ((x * 3) % w) / w) as u8, 40]
            });
        }
    }
    let page = |side: u32, photo_x: u32, photo_w: u32, photo_h: u32, title_y: u32| {
        let mut content = format!(
            "0.129 0.231 0.420 rg 0 0 {side} 792 re f\n0.95 g BT /F1 14 Tf 20 740 Td (Sidebar) Tj ET\nq {photo_w} 0 0 {photo_h} {photo_x} 190 cm /Im0 Do Q\n0.12 g BT /F1 14 Tf {photo_x} {title_y} Td (A photo and a dark sidebar) Tj ET\n"
        );
        for line in 0..10 {
            content.push_str(&format!(
                "BT /F1 9 Tf {photo_x} {} Td (Line {}: the quick brown fox jumps over the lazy dog.) Tj ET\n",
                170 - line * 15,
                line + 1
            ));
        }
        Page::new(content.into_bytes())
    };
    // p5's places, and p18's.
    let list = [page(184, 196, 416, 523, 745), page(202, 214, 398, 548, 760)];
    let image = first_extra_object(list.len());
    let file = TempPdf::new(
        "sidebars.pdf",
        &pages_with(
            &list,
            &format!("/XObject << /Im0 {image} 0 R >>"),
            vec![Object::Stream {
                dict: format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /BitsPerComponent 8 /ColorSpace /DeviceRGB"),
                data: photo,
            }],
        ),
    );
    let mut doc = renderer(&file.0);
    let (dim, keep, _) = look_of(&mut doc, 0, 512.0, false);
    assert!(
        !dim && keep.len() == 1,
        "a sidebar 30% wide turns over, keeping its photo: dim {dim}, kept {keep:?}"
    );
    assert!(look_of(&mut doc, 1, 512.0, false).0, "a sidebar a third wide is dark, and dimmed whole");
}

/// A document of `count` pages of text, no pictures.
fn text_document(count: usize) -> TempPdf {
    let lines: Vec<String> = (0..48)
        .map(|at| format!("Line {at} of a page of text, long enough to fill most of the line across."))
        .collect();
    let text = text_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>());
    let list: Vec<Page> = (0..count).map(|_| Page::new(text.clone())).collect();
    TempPdf::new("text-document.pdf", &pages_with(&list, "", Vec::new()))
}

/// Drives `engine` until it's settled, applying what it asks, each surface it hands over let go of, for at most `limit`. How long that took.
#[allow(unsafe_code)]
fn settle(engine: &Engine, viewport: Viewport, limit: Duration) -> Duration {
    let started = Instant::now();
    loop {
        let mut ops = engine.set_viewport(viewport);
        ops.extend(engine.drain());
        engine.applied();
        for op in ops {
            if let Op::Attach { surface, .. } = op {
                // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                drop(unsafe { PixelSurface::from_host_reference(surface) });
            }
        }
        if engine.wait_settled(Duration::from_millis(5)) || started.elapsed() > limit {
            return started.elapsed();
        }
    }
}

/// Measures, and isn't a check: what a page view on dark paper takes over a text document, the time to open it, scroll through it page by page, zoom out to the least zoom two pages across, where no previews are drawn, and in again, and wake onto a sleep picture to the first sharp tile. Run on `main` and on a branch, release, the load written down:
///
///     RECTO_LOOK_PAGES=500 cargo test --release -p recto-reader --test looks_on_real_files -- --ignored dark_paper_view_costs --nocapture
#[test]
#[ignore = "a measurement"]
#[allow(unsafe_code)]
fn dark_paper_view_costs() {
    let count: usize = std::env::var("RECTO_LOOK_PAGES").ok().and_then(|pages| pages.parse().ok()).unwrap_or(500);
    let file = text_document(count);
    let map = || PageMap {
        files: vec![file.0.clone()],
        slots: None,
        own: None,
        turns: None,
        expected: Vec::new(),
        follows: None,
    };
    let new_engine = || {
        let engine = Engine::new(
            EngineConfig {
                service: None,
                policy: Some(Policy::default()),
                keys: None,
            },
            Arc::new(Quiet),
        );
        engine.set_viewport(view(0.0));
        engine.set_paper(PaperKind::Dark);
        engine
    };
    let engine = new_engine();
    let started = Instant::now();
    engine.activate(1, count as u32, 0, map(), &[], None, recto_reader::layout::Spread::Single);
    let opened = settle(&engine, view(0.0), Duration::from_secs(60));
    let height = engine.report().0["document_height"].as_f64().unwrap();
    let mut steps = Vec::new();
    for page in 1..count {
        steps.push(settle(&engine, view(height * page as f64 / count as f64), Duration::from_secs(60)));
    }
    let scrolled = started.elapsed();
    let zoomed_in = settle(
        &engine,
        Viewport {
            magnification: 2.0,
            ..view(0.0)
        },
        Duration::from_secs(60),
    );
    engine.shut_down();
    // Opened zoomed out as far as a person can, two pages across, by a page view that's decided nothing: no previews are drawn at this zoom, so each page's first job is a tile.
    let engine = new_engine();
    engine.set_spread(recto_reader::layout::Spread::TwoUp);
    let overview = Viewport {
        magnification: recto_reader::tiles::MIN_ZOOM,
        ..view(0.0)
    };
    engine.set_viewport(overview);
    let started = Instant::now();
    engine.activate(1, count as u32, 0, map(), &[], None, recto_reader::layout::Spread::TwoUp);
    settle(&engine, overview, Duration::from_secs(60));
    let overview_took = started.elapsed();
    let pages_in_overview = engine.report().0["visible_pages"].as_array().map_or(0, Vec::len);
    assert_eq!(engine.report().0["tiles"]["previews_attached"], 0);
    engine.shut_down();
    steps.sort_unstable();
    let at = |share: f64| steps[((steps.len() - 1) as f64 * share) as usize].as_secs_f64() * 1000.0;
    println!(
        "{count} pages of text: opened in {:.1} ms, scrolled through in {:.1} ms all told, {:.2} ms a page; a step to the next page p50 {:.2} ms, p99 {:.2}; zoomed in to 2 {:.1} ms; opened zoomed out to {pages_in_overview} pages with no previews {:.1} ms",
        opened.as_secs_f64() * 1000.0,
        scrolled.as_secs_f64() * 1000.0,
        scrolled.as_secs_f64() * 1000.0 / count as f64,
        at(0.5),
        at(0.99),
        zoomed_in.as_secs_f64() * 1000.0,
        overview_took.as_secs_f64() * 1000.0
    );
    // A wake onto a sleep picture of page 1, to the first tile sharper than it, ten times.
    let version = {
        let mut doc = renderer(&file.0);
        doc.page_size(0).unwrap();
        doc.known_version()
    };
    let mut wakes = Vec::new();
    for _ in 0..10 {
        let engine = new_engine();
        let picture = PixelSurface::create(306, 396).unwrap();
        engine.waking().note_wake(1, Instant::now());
        engine.waking().give_sleep(
            1,
            SleepView {
                version,
                usual: Some(PageBox { width: 612.0, height: 792.0 }),
                sizes: Vec::new(),
                pictures: vec![SleepShot {
                    page: 0,
                    scale: 0.5,
                    paper: PaperKind::Dark,
                    surface: picture,
                }],
            },
        );
        let started = Instant::now();
        engine.activate(1, count as u32, 0, map(), &[], None, recto_reader::layout::Spread::Single);
        let mut sharp = None;
        while sharp.is_none() && started.elapsed() < Duration::from_secs(30) {
            let mut ops = engine.set_viewport(view(0.0));
            ops.extend(engine.drain());
            engine.applied();
            for op in ops {
                if let Op::Attach {
                    surface, crop_width, width, ..
                } = op
                {
                    // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                    let surface = unsafe { PixelSurface::from_host_reference(surface) };
                    let density = surface.map_or(0.0, |surface| crop_width * f64::from(surface.width()) / width);
                    if density > 1.5 && sharp.is_none() {
                        sharp = Some(started.elapsed());
                    }
                }
            }
            engine.wait_settled(Duration::from_millis(1));
        }
        engine.shut_down();
        wakes.push(sharp.expect("a sharp tile"));
    }
    wakes.sort_unstable();
    println!(
        "a wake onto a sleep picture to the first sharp tile: p50 {:.1} ms, most {:.1}",
        wakes[wakes.len() / 2].as_secs_f64() * 1000.0,
        wakes[wakes.len() - 1].as_secs_f64() * 1000.0
    );
}
