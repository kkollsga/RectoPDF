//! Dark paper's judgement through the real worker: on real files, skipped where the benchmark files aren't, as on CI (`paper.rs` holds the rule without them), and on made pages, from pictures of each size a look can be decided from. Nothing here races a memory limit: a page at a lower resolution is held by the stand-in worker in `recto-render-service`'s `tests/sequence.rs`.

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

/// What's at the top left of the page, as red, green, blue, on dark paper: the colour of a tile with none of its own, or a surface's first pixel. And whether it came out at a lower resolution.
#[allow(unsafe_code)]
fn corner_on_dark_paper(file: &Path, pages: u32, page: u32, policy: Policy) -> (u32, bool) {
    let engine = Engine::new(
        EngineConfig {
            service: None,
            policy: Some(policy),
            keys: None,
        },
        Arc::new(Quiet),
    );
    engine.set_viewport(view(0.0));
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
    let mut corner = None;
    let started = Instant::now();
    let mut y = 0.0;
    while started.elapsed() < Duration::from_secs(60) {
        let mut ops = engine.set_viewport(view(y));
        ops.extend(engine.drain());
        engine.applied();
        for op in ops {
            match op {
                Op::ScrollTo { y: to, .. } => y = to,
                Op::Attach {
                    x,
                    y: top,
                    surface,
                    rgb,
                    page: on,
                    ..
                } => {
                    // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                    let surface = unsafe { PixelSurface::from_host_reference(surface) };
                    if on == page && x == 0.0 && top == 0.0 {
                        let colour = surface.and_then(|surface| {
                            surface
                                .with_pixels(|pixels| (u32::from(pixels[2]) << 16) | (u32::from(pixels[1]) << 8) | u32::from(pixels[0]))
                                .ok()
                        });
                        corner = colour.or(rgb).or(Some(DARK_PAPER));
                    }
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
    // A blank tile of the paper's own colour needs nothing put on a layer, so nothing's said about it.
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

/// What a renderer of its own, as a woken tab gets, decides for a page from a picture of it whose longest side is `edge`, with the images it's told of, or none if `whole`: dimmed, kept and turned, as a person sees it.
fn look_of(file: &Path, page: u32, edge: f32, whole: bool) -> (bool, Vec<[u16; 4]>, Vec<[u16; 4]>) {
    let mut doc = DocRenderer::new(file, None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap();
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

/// Failure: the real USGS sheet, whose map sits on white among its legends and tables, is judged whole where its images are known, and so dimmed to light grey where the sharp page turned over; or judged by its images, it's dimmed. Decided in process from one picture of it at the look picture's size: with its images it turns over and keeps its map; judged whole, as a page is only when its images can't be had (a page that fails at every zoom, or a walk that was stopped), it's dimmed, as its map can't be told from its paper. The real scan turns over either way.
#[test]
fn the_real_sheet_judged_by_its_images_turns_over_and_judged_whole_is_dimmed() {
    let (Some(scan), Some(map)) = (file("gov_nasa_ntrs20150023275.pdf"), file("gov_usgs_usgs70277338.pdf")) else {
        eprintln!("skipped: the benchmark files aren't here");
        return;
    };
    let (dim, keep, _) = look_of(&map, 0, 512.0, false);
    assert!(!dim && !keep.is_empty(), "by its images: dim {dim}, kept {keep:?}");
    assert!(look_of(&map, 0, 512.0, true).0, "judged whole");
    assert!(!look_of(&scan, 2, 512.0, false).0 && !look_of(&scan, 2, 512.0, true).0, "the scan");
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

/// Failure: a page's look on dark paper hangs on which picture of it was drawn first, a preview at the window's size, the look picture, or the look picture at a lower resolution as a page too heavy for its limit gets, or on whether its renderer was dropped and made again, as a sleeping tab's is (review of 2026-11-13: the USGS sheet turned over, then went light grey after a wake). Each made page, from a picture at the largest preview's 1,144, the look picture's 512 and a lower resolution's 128, each by a renderer of its own, comes out the same: turned over with its photo kept on white, dimmed on navy, turned over under half on white with a band.
#[test]
fn a_page_has_the_same_look_from_any_picture_of_it_by_any_renderer() {
    let file = photo_pages();
    let expected = [(false, 1), (true, 0), (false, 1)];
    for (page, (dim, kept)) in expected.into_iter().enumerate() {
        let looks: Vec<_> = [1144.0, 512.0, 128.0]
            .into_iter()
            .map(|edge| look_of(&file.0, page as u32, edge, false))
            .collect();
        assert!(looks.iter().all(|look| *look == looks[0]), "page {}: {looks:?}", page + 1);
        assert_eq!((looks[0].0, looks[0].1.len()), (dim, kept), "page {}: {:?}", page + 1, looks[0]);
    }
}
