//! How `PageLook` comes out on real files. Not a check, a measurement: it's what the constants in `paper.rs` were chosen from. It writes each image's picture for a person to look at.
//!
//!     RECTO_LOOK_LIST=list.txt RECTO_LOOK_OUT=folder cargo test --release -p recto-reader --test look_survey -- --ignored --nocapture
//!
//! A line of the list is a path, then a space, then pages from 1, as `1,5,20`. `RECTO_LOOK_EDGE` is the longest side of the picture each page is judged from, 512 by default as the look picture is drawn; a preview's is up to 1,144 and a lower resolution's a quarter.
//!
//! `sizes` judges each page with images from three pictures, the look picture's 512, the largest preview's 1,144 and the look picture's lower resolution's 128, and says where the looks differ: whether the page turns over, and which images on it turn over with it. Same list, no pictures written:
//!
//!     RECTO_LOOK_LIST=list.txt cargo test --release -p recto-reader --test look_survey -- --ignored sizes --nocapture

use recto_platform::surface::PixelSurface;
use recto_proto::{TileRect, device_size};
use recto_reader::paper::{PageLook, sample_pixels};
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
    let (mut pages, mut with_images, mut draws) = (0, 0, Vec::new());
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
            let Some(boxes) = doc.image_boxes(page, 1).ok().flatten() else { continue };
            if boxes.is_empty() {
                continue;
            }
            let Some((w, h, look_picture, ms)) = picture_of(&mut doc, page, 512.0) else {
                continue;
            };
            draws.push(ms);
            with_images += 1;
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
