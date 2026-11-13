//! How `PageLook` comes out on real files. Not a check, a measurement: it's what the constants in `paper.rs` were chosen from. It writes each image's picture for a person to look at.
//!
//!     RECTO_LOOK_LIST=list.txt RECTO_LOOK_OUT=folder cargo test --release -p recto-reader --test look_survey -- --ignored --nocapture
//!
//! A line of the list is a path, then a space, then pages from 1, as `1,5,20`.

use recto_platform::surface::PixelSurface;
use recto_proto::{TileRect, device_size};
use recto_reader::paper::{PageLook, sample_pixels};
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileRequest, TileResult};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

#[test]
#[ignore = "a measurement"]
fn survey() {
    let list = std::fs::read_to_string(std::env::var("RECTO_LOOK_LIST").unwrap()).unwrap();
    let out = std::env::var("RECTO_LOOK_OUT").unwrap();
    for line in list.lines() {
        let Some((path, pages)) = line.rsplit_once(' ') else { continue };
        let Ok(mut doc) = DocRenderer::new(Path::new(path), None, ServiceConfig::discover().unwrap(), Policy::default()) else {
            continue;
        };
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        for page in pages.split(',').filter_map(|page| page.parse::<u32>().ok()) {
            let page = page - 1;
            let Ok((width, height)) = doc.page_size(page) else { continue };
            let scale = 512.0 / width.max(height);
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
