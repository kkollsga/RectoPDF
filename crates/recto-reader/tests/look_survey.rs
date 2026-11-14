//! How `PageLook` comes out on real files. Not a check, a measurement: it's what the constants in `paper.rs` were chosen from. It writes each image's picture for a person to look at.
//!
//!     RECTO_LOOK_LIST=list.txt RECTO_LOOK_OUT=folder cargo test --release -p recto-reader --test look_survey -- --ignored --nocapture
//!
//! A line of the list is a path, then a space, then pages from 1, as `1,5,20`. `RECTO_LOOK_EDGE` is the longest side of the picture each page is judged from, 512 by default as the look picture is drawn; a preview's is up to 1,144 and a lower resolution's a quarter.
//!
//! `sizes` judges each page, with images or none, from three pictures, the look picture's 512, the largest preview's 1,144 and the look picture's lower resolution's 128, and says where the looks differ: whether the page turns over, and which images on it turn over with it. Same list, no pictures written:
//!
//!     RECTO_LOOK_LIST=list.txt cargo test --release -p recto-reader --test look_survey -- --ignored sizes --nocapture
//!
//! `pages` decides every page of the list from the look picture and prints a line for each, with its background's darkness at several lines, for comparing `main` with a branch:
//!
//!     RECTO_LOOK_LIST=list.txt cargo test --release -p recto-reader --test look_survey -- --ignored --exact pages --nocapture

use recto_platform::surface::PixelSurface;
use recto_proto::{TileRect, device_size};
use recto_reader::paper::{PageLook, sample_lightness, sample_pixels};
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileRequest, TileResult};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

/// A picture of a whole page, its longest side `edge`, as `(width, height, BGRA rows packed, milliseconds to draw)`.
fn picture_of(doc: &mut DocRenderer, page: u32, edge: f32) -> Option<(u32, u32, Vec<u8>, f64)> {
    let (width, height) = doc.page_size(page).ok()?;
    let scale = edge / width.max(height);
    let (w, h) = device_size(width, height, scale).ok()?;
    let surface = PixelSurface::create(w, h).ok()?;
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
    let started = Instant::now();
    if !matches!(doc.render_tile(&request, &surface), Ok(TileResult::Done(_))) {
        return None;
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let stride = surface.stride() as usize;
    let rows = surface
        .with_pixels(|pixels| pixels.chunks(stride).take(h as usize).flat_map(|row| row[..w as usize * 4].to_vec()).collect())
        .ok()?;
    Some((w, h, rows, ms))
}

#[test]
#[ignore = "a measurement"]
fn sizes() {
    let list = std::fs::read_to_string(std::env::var("RECTO_LOOK_LIST").unwrap()).unwrap();
    // What a person sees of a look: dimmed or not, and which images are kept and which turn over. Not the gains a turned image's paper is brought to white by, which move a step with any picture.
    let seen = |look: &PageLook| (look.dim, look.keep.clone(), look.whiten.iter().map(|image| image.sides).collect::<Vec<_>>());
    let (mut pages, mut with_images, mut draws) = (0, 0usize, Vec::new());
    let mut differ = [(0, 0); 2];
    for line in list.lines() {
        let Some((path, list)) = line.rsplit_once(' ') else { continue };
        let Ok(mut doc) = DocRenderer::new(Path::new(path), None, ServiceConfig::discover().unwrap(), Policy::default()) else {
            continue;
        };
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        for page in list.split(',').filter_map(|page| page.parse::<u32>().ok()).map(|page| page - 1) {
            if doc.page_size(page).is_err() {
                continue;
            }
            pages += 1;
            // A page with no images is decided too, since 2026-11-13: its background may be dark.
            let Some(boxes) = doc.image_boxes(page, 1).ok().flatten() else { continue };
            let Some((w, h, look_picture, ms)) = picture_of(&mut doc, page, 512.0) else {
                continue;
            };
            draws.push(ms);
            with_images += usize::from(!boxes.is_empty());
            let look = seen(&PageLook::decide(Some(&boxes), &look_picture, w as usize * 4, w, h));
            for (index, edge) in [1144.0, 128.0].into_iter().enumerate() {
                let Some((w, h, other, _)) = picture_of(&mut doc, page, edge) else { continue };
                let other = seen(&PageLook::decide(Some(&boxes), &other, w as usize * 4, w, h));
                if other.0 != look.0 {
                    differ[index].0 += 1;
                    println!("{name} p{} at {edge}: page dim {} where the look picture says {}", page + 1, other.0, look.0);
                } else if other != look {
                    differ[index].1 += 1;
                    println!(
                        "{name} p{} at {edge}: images kept {} turned {}, where the look picture's kept {} turned {}",
                        page + 1,
                        other.1.len(),
                        other.2.len(),
                        look.1.len(),
                        look.2.len()
                    );
                }
            }
        }
    }
    draws.sort_by(f64::total_cmp);
    let at = |share: f64| draws.get(((draws.len().max(1) - 1) as f64 * share) as usize).copied().unwrap_or(0.0);
    println!(
        "{pages} pages, {with_images} with images. Against the look picture, 512: at 1144 {} pages differ in whether they turn over and {} more in which images do; at 128, {} and {}. Drawing the look picture: p50 {:.1} ms, p90 {:.1}, p99 {:.1}, most {:.1}.",
        differ[0].0,
        differ[0].1,
        differ[1].0,
        differ[1].1,
        at(0.5),
        at(0.9),
        at(0.99),
        at(1.0)
    );
}

/// The share of a picture's pixels outside `boxes` whose lightness, Rec. 601 luma as `paper.rs` turns over, is under each of `under`, read from every pixel: what a page's background's darkness would be at each line, for choosing it.
fn dark_shares(pixels: &[u8], stride: usize, w: u32, h: u32, boxes: &[[u16; 4]], under: &[i32]) -> (Vec<f64>, f64) {
    let place = |part: u16, whole: u32| (u64::from(part) * u64::from(whole)).div_ceil(65_535) as usize;
    let holes: Vec<[usize; 4]> = boxes
        .iter()
        .map(|sides| {
            [
                (u64::from(sides[0]) * u64::from(w) / 65_535) as usize,
                (u64::from(sides[1]) * u64::from(h) / 65_535) as usize,
                place(sides[2], w),
                place(sides[3], h),
            ]
        })
        .collect();
    let (mut counts, mut outside) = (vec![0usize; under.len()], 0usize);
    for (y, row) in pixels.chunks(stride).enumerate().take(h as usize) {
        for (x, pixel) in row.chunks(4).enumerate().take(w as usize) {
            if holes.iter().any(|hole| (hole[0]..hole[2]).contains(&x) && (hole[1]..hole[3]).contains(&y)) {
                continue;
            }
            outside += 1;
            let luma = (77 * i32::from(pixel[2]) + 150 * i32::from(pixel[1]) + 29 * i32::from(pixel[0])) >> 8;
            for (count, line) in counts.iter_mut().zip(under) {
                *count += usize::from(luma < *line);
            }
        }
    }
    let share = |part: usize| if outside == 0 { 0.0 } else { part as f64 / outside as f64 };
    (counts.into_iter().map(share).collect(), outside as f64 / (f64::from(w) * f64::from(h)))
}

/// `pages`: every page of the list, with images or none, decided from the look picture (512) as the engine decides it, one line each: whether it's dimmed, how many images are kept and turned, and the share of its background, outside every image's box, that's darker than each of several lines, to choose the line by. Run on `main` and on a branch, the lines say which pages a change moved. Same list, no pictures written:
///
///     RECTO_LOOK_LIST=list.txt cargo test --release -p recto-reader --test look_survey -- --ignored pages --nocapture
#[test]
#[ignore = "a measurement"]
fn pages() {
    let list = std::fs::read_to_string(std::env::var("RECTO_LOOK_LIST").unwrap()).unwrap();
    let under = [64, 96, 112, 128, 144, 160];
    for line in list.lines() {
        let Some((path, list)) = line.rsplit_once(' ') else { continue };
        let Ok(mut doc) = DocRenderer::new(Path::new(path), None, ServiceConfig::discover().unwrap(), Policy::default()) else {
            continue;
        };
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        for page in list.split(',').filter_map(|page| page.parse::<u32>().ok()).map(|page| page - 1) {
            if doc.page_size(page).is_err() {
                continue;
            }
            let boxes = doc.image_boxes(page, 1).ok().flatten();
            let Some((w, h, picture, _)) = picture_of(&mut doc, page, 512.0) else {
                println!("PAGE {name} p{} undrawn", page + 1);
                continue;
            };
            let look = PageLook::decide(boxes.as_deref(), &picture, w as usize * 4, w, h);
            let (shares, background) = dark_shares(&picture, w as usize * 4, w, h, boxes.as_deref().unwrap_or_default(), &under);
            let shares: Vec<String> = under.iter().zip(&shares).map(|(line, share)| format!("{line}:{share:.2}")).collect();
            // The background the dark test reads since review round 1 (2026-11-14): everything but the kept pictures, white-backed ones being light background, by every pixel, and the dark part of it as a share of the whole page.
            let place = |part: u16, whole: u32| (u64::from(part) * u64::from(whole) / 65_535) as usize;
            let kept: Vec<[u16; 4]> = boxes
                .iter()
                .flatten()
                .copied()
                .filter(|sides| {
                    let found = sample_pixels(
                        &picture,
                        w as usize * 4,
                        place(sides[0], w),
                        place(sides[1], h),
                        place(sides[2], w).min(w as usize),
                        place(sides[3], h).min(h as usize),
                    );
                    !found.has_white_background() && found.pixels >= 64
                })
                .collect();
            let (own, own_background) = dark_shares(&picture, w as usize * 4, w, h, &kept, &[128]);
            // The dark test as `decide` reads it, from the grid, the kept pictures' boxes rounded outward: how much of the background is dark, of the page, and of its ring, and what the share of the page rule (0.35, before review round 2) and the ring rule say.
            let up = |part: u16, whole: u32| (u64::from(part) * u64::from(whole)).div_ceil(65_535) as usize;
            let mut holes: Vec<[usize; 4]> = kept
                .iter()
                .map(|sides| [place(sides[0], w), place(sides[1], h), up(sides[2], w), up(sides[3], h)])
                .collect();
            holes.sort_unstable_by_key(|hole| hole[0]);
            let found = sample_lightness(&picture, w as usize * 4, w, h, &holes);
            let base = found.points >= 64 && found.of_page >= 0.05 && found.dark >= 0.5;
            let share_rule = base && found.dark * found.of_page >= 0.35;
            let ring_rule = base && (found.ring_points < 64 || found.ring >= 0.5);
            println!(
                "PAGE {name} p{} images={} dim={} keep={} whiten={} background={background:.2} dark {} own_background={own_background:.2} dark_of_it={:.2} dark_of_page={:.3} grid_dark={:.3} grid_of_page={:.3} ring={:.3} ring_points={} share_rule={share_rule} ring_rule={ring_rule}",
                page + 1,
                boxes.as_ref().map_or_else(|| "whole".to_string(), |boxes| boxes.len().to_string()),
                look.dim,
                look.keep.len(),
                look.whiten.len(),
                shares.join(" "),
                own[0],
                own[0] * own_background,
                found.dark,
                found.of_page,
                found.ring,
                found.ring_points
            );
        }
    }
}

#[test]
#[ignore = "a measurement"]
fn survey() {
    let list = std::fs::read_to_string(std::env::var("RECTO_LOOK_LIST").unwrap()).unwrap();
    let out = std::env::var("RECTO_LOOK_OUT").unwrap();
    let edge: f32 = std::env::var("RECTO_LOOK_EDGE").ok().and_then(|edge| edge.parse().ok()).unwrap_or(512.0);
    for line in list.lines() {
        let Some((path, pages)) = line.rsplit_once(' ') else { continue };
        let Ok(mut doc) = DocRenderer::new(Path::new(path), None, ServiceConfig::discover().unwrap(), Policy::default()) else {
            continue;
        };
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        for page in pages.split(',').filter_map(|page| page.parse::<u32>().ok()) {
            let page = page - 1;
            let Ok((width, height)) = doc.page_size(page) else { continue };
            let scale = edge / width.max(height);
            let Ok((w, h)) = device_size(width, height, scale) else { continue };
            let surface = PixelSurface::create(w, h).unwrap();
            let started = Instant::now();
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
            let done = matches!(doc.render_tile(&request, &surface), Ok(TileResult::Done(_)));
            let draw_ms = started.elapsed().as_secs_f64() * 1000.0;
            if !done {
                println!("{name} p{} didn't draw", page + 1);
                continue;
            }
            let boxes = doc.image_boxes(page, 1).ok().flatten();
            let stride = surface.stride() as usize;
            surface
                .with_pixels(|pixels| {
                    let started = Instant::now();
                    let look = PageLook::decide(boxes.as_deref(), pixels, stride, w, h);
                    let judge_us = started.elapsed().as_micros();
                    let mut rows = String::new();
                    for (index, sides) in boxes.iter().flatten().enumerate() {
                        let place = |part: u16, whole: u32| (u64::from(part) * u64::from(whole) / 65_535) as usize;
                        let (left, top, right, bottom) = (place(sides[0], w), place(sides[1], h), place(sides[2], w).min(w as usize), place(sides[3], h).min(h as usize));
                        let found = sample_pixels(pixels, stride, left, top, right, bottom);
                        let share = f64::from(sides[2].saturating_sub(sides[0])) * f64::from(sides[3].saturating_sub(sides[1])) / (65_535.0 * 65_535.0);
                        let _ = writeln!(rows, "  {name} p{} box{index} share={share:.2} paper={:.2} colour={:.2} purewhite={:.2} clustered={:.2} plain={:.2} px={} bg={:?} white={}", page + 1, found.paper, found.colour, found.white, found.clustered, found.plain, found.pixels, found.background, found.has_white_background());
                        // The image, as a picture to look at.
                        if right > left && bottom > top {
                            let mut ppm = format!("P6\n{} {}\n255\n", right - left, bottom - top).into_bytes();
                            for row in pixels.chunks(stride).skip(top).take(bottom - top) {
                                for pixel in row[left * 4..right * 4].chunks(4) {
                                    ppm.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                                }
                            }
                            let verdict = if found.has_white_background() { "flip" } else { "keep" };
                            let _ = std::fs::write(format!("{out}/{verdict}__{name}__p{}__box{index}__p{:.2}_c{:.2}_w{:.2}_s{share:.2}.ppm", page + 1, found.paper, found.colour, found.white), ppm);
                        }
                    }
                    println!("{name} p{} draw={draw_ms:.1}ms judge={judge_us}us boxes={} dim={} keep={} whiten={}\n{rows}", page + 1, boxes.as_ref().map_or(-1, |boxes| boxes.len() as i64), look.dim, look.keep.len(), look.whiten.len());
                })
                .unwrap();
        }
    }
}

/// `small_pictures`: for each page of the list with no images, the time to draw the picture a page with no images is decided from when nothing drawn covers it, 128 pixels on its longest side, against the look picture's 512, and the worker's footprint after each. Same list:
///
///     RECTO_LOOK_LIST=list.txt cargo test --release -p recto-reader --test look_survey -- --ignored --exact small_pictures --nocapture
#[test]
#[ignore = "a measurement"]
fn small_pictures() {
    let list = std::fs::read_to_string(std::env::var("RECTO_LOOK_LIST").unwrap()).unwrap();
    let (mut small, mut large, mut warm, mut footprint) = (Vec::new(), Vec::new(), Vec::new(), 0u64);
    let mut slowest: Vec<(f64, String)> = Vec::new();
    for line in list.lines() {
        let Some((path, list)) = line.rsplit_once(' ') else { continue };
        let Ok(mut doc) = DocRenderer::new(Path::new(path), None, ServiceConfig::discover().unwrap(), Policy::default()) else {
            continue;
        };
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        let mut file_small = Vec::new();
        for page in list.split(',').filter_map(|page| page.parse::<u32>().ok()).map(|page| page - 1) {
            if doc.page_size(page).is_err() || !doc.image_boxes(page, 1).ok().flatten().is_some_and(|boxes| boxes.is_empty()) {
                continue;
            }
            // The page is loaded by the size and the walk, as it would be by then in the app, and its first picture is drawn cold, as the small one is after a wake; then the look picture and the small one again, warm.
            let Some((_, _, _, ms)) = picture_of(&mut doc, page, 128.0) else { continue };
            small.push(ms);
            file_small.push(ms);
            slowest.push((ms, format!("{path} {}", page + 1)));
            footprint = footprint.max(doc.worker_footprint().unwrap_or(0));
            if let Some((_, _, _, ms)) = picture_of(&mut doc, page, 512.0) {
                large.push(ms);
            }
            if let Some((_, _, _, ms)) = picture_of(&mut doc, page, 128.0) {
                warm.push(ms);
            }
        }
        if !file_small.is_empty() {
            file_small.sort_by(f64::total_cmp);
            println!(
                "{name}: {} pages with no images, 128 px p50 {:.2} ms, most {:.2}",
                file_small.len(),
                file_small[file_small.len() / 2],
                file_small[file_small.len() - 1]
            );
        }
    }
    // The pages with no images slowest to draw small, the heaviest drawings in vectors, as `path page` lines a wake can be measured on (`wakes`).
    slowest.sort_by(|one, two| two.0.total_cmp(&one.0));
    for (ms, page) in slowest.iter().take(8) {
        println!("SLOW {ms:.2} ms {page}");
    }
    let at = |draws: &mut Vec<f64>, share: f64| {
        draws.sort_by(f64::total_cmp);
        draws.get(((draws.len().max(1) - 1) as f64 * share) as usize).copied().unwrap_or(0.0)
    };
    println!(
        "{} pages with no images. 128 px, the page's first draw: p50 {:.2} ms, p99 {:.2}, most {:.2}. Then 512 px: p50 {:.2}, p99 {:.2}. Then 128 px again: p50 {:.2}, p99 {:.2}. Largest worker footprint seen after a 128 px draw: {:.1} MiB",
        small.len(),
        at(&mut small, 0.5),
        at(&mut small, 0.99),
        at(&mut small, 1.0),
        at(&mut large, 0.5),
        at(&mut large, 0.99),
        at(&mut warm, 0.5),
        at(&mut warm, 0.99),
        footprint as f64 / 1_048_576.0
    );
}

/// The events a page view tells, none listened to.
struct Quiet;
impl recto_reader::engine::EngineEvents for Quiet {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
}

/// `wakes`: for each file of `RECTO_WAKE_LIST` (a path a line, its pages each a page to measure, two of them for a spread with `2up` after the path), on dark paper, a wake onto a sleep picture of the pages at the top, to the first tile sharper than it and to a sharp tile on every page in view, ten times each, p50 and worst. A wake's first job for a page is a tile, the sleep picture standing for its preview, so a page with no images is decided from a 128 px picture drawn for it where `main` took a page of text's look: run on `main` and on the branch, release, the load written down:
///
///     RECTO_WAKE_LIST=list.txt cargo test --release -p recto-reader --test look_survey -- --ignored --exact wakes --nocapture
#[test]
#[ignore = "a measurement"]
#[allow(unsafe_code)]
fn wakes() {
    use recto_reader::layout::{PageBox, Spread};
    use recto_reader::tiles::{Op, PageMap, SleepShot, SleepView, Viewport};
    use recto_reader::{Engine, EngineConfig, PaperKind};
    use std::time::Duration;
    let list = std::fs::read_to_string(std::env::var("RECTO_WAKE_LIST").unwrap()).unwrap();
    let view = Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    };
    for line in list.lines().filter(|line| !line.trim().is_empty()) {
        let (path, spread) = match line.split_once(' ') {
            Some((path, "2up")) => (path, Spread::TwoUp),
            _ => (line, Spread::Single),
        };
        let mut doc = DocRenderer::new(Path::new(path), None, ServiceConfig::discover().unwrap(), Policy::default()).unwrap();
        let (width, height) = doc.page_size(0).unwrap();
        let count = doc.known_page_count().unwrap();
        let version = doc.known_version();
        let in_view = if spread == Spread::TwoUp { 2 } else { 1 };
        let images: Vec<usize> = (0..in_view)
            .map(|page| doc.image_boxes(page, 1).ok().flatten().map_or(usize::MAX, |boxes| boxes.len()))
            .collect();
        drop(doc);
        let (mut first, mut all) = (Vec::new(), Vec::new());
        for _ in 0..10 {
            let engine = Engine::new(
                EngineConfig {
                    service: None,
                    policy: Some(Policy::default()),
                    keys: None,
                },
                std::sync::Arc::new(Quiet),
            );
            engine.set_viewport(view);
            engine.set_paper(PaperKind::Dark);
            engine.set_spread(spread);
            engine.waking().note_wake(1, Instant::now());
            let pictures = (0..in_view)
                .map(|page| SleepShot {
                    page,
                    scale: 0.5,
                    paper: PaperKind::Dark,
                    surface: PixelSurface::create((width * 0.5) as u32, (height * 0.5) as u32).unwrap(),
                })
                .collect();
            engine.waking().give_sleep(
                1,
                SleepView {
                    version,
                    usual: Some(PageBox { width, height }),
                    sizes: Vec::new(),
                    pictures,
                },
            );
            let map = PageMap {
                files: vec![Path::new(path).to_path_buf()],
                slots: None,
                own: None,
                turns: None,
                expected: Vec::new(),
                follows: None,
            };
            let started = Instant::now();
            engine.activate(1, count, 0, map, &[], None, spread);
            let (mut sharp_first, mut sharp_pages) = (None, std::collections::BTreeSet::new());
            while sharp_pages.len() < in_view as usize && started.elapsed() < Duration::from_secs(60) {
                let mut ops = engine.set_viewport(view);
                ops.extend(engine.drain());
                engine.applied();
                for op in ops {
                    if let Op::Attach {
                        surface,
                        crop_width,
                        width,
                        page,
                        ..
                    } = op
                    {
                        // SAFETY: each reference in an `Attach` is given up to whoever applies it, and this stand-in host applies it here, once.
                        let surface = unsafe { PixelSurface::from_host_reference(surface) };
                        let density = surface.map_or(0.0, |surface| crop_width * f64::from(surface.width()) / width);
                        if density > 1.0 {
                            sharp_first.get_or_insert(started.elapsed());
                            sharp_pages.insert(page);
                        }
                    }
                }
                engine.wait_settled(Duration::from_millis(1));
            }
            all.push(started.elapsed());
            engine.shut_down();
            first.push(sharp_first.expect("a sharp tile"));
        }
        let ms = |times: &mut Vec<Duration>| {
            times.sort_unstable();
            (times[times.len() / 2].as_secs_f64() * 1000.0, times[times.len() - 1].as_secs_f64() * 1000.0)
        };
        let ((first_p50, first_most), (all_p50, all_most)) = (ms(&mut first), ms(&mut all));
        println!(
            "WAKE {} {spread:?} images {images:?}: first sharp tile p50 {first_p50:.1} ms, worst {first_most:.1}; every page in view sharp p50 {all_p50:.1}, worst {all_most:.1}",
            Path::new(path).file_name().unwrap().to_string_lossy()
        );
    }
}
