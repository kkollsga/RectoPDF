//! `budget render`: what a worker costs, measured from outside on the real path. Also the shared way to draw a whole page in strips, so a
//! large page never needs a whole-page buffer.

use crate::host::{DocRenderer, LowResWhy, RenderFailure, TileRequest, TileResult};
use recto_platform::isolate::SharedMemory;
use recto_proto::{BYTES_PER_PIXEL, TILE_EDGE, TileKind, TileRect, device_size};
use std::time::{Duration, Instant};

/// The most one strip of a page may hold. Whole-page drawing costs more memory the larger each piece is, and takes longer the smaller it is. Measured on the USGS map at
/// 4x: 64 MiB strips peak at 613 MiB in 1.5 s, 16 MiB at 450 in 3.4 s, 8 MiB at 423 in 5.6 s, 1 MiB at 405 in 38 s. At 2x, 8 MiB is 371 MiB in 0.7 s, no slower than
/// 64 MiB. A letter page at 2x is 7.7 MiB, so an ordinary page stays one strip. Memory first, so 8 MiB, which keeps every page measured under the normal limit.
pub const STRIP_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// What drawing one whole page may cost. Without these, a tiny PDF asking for a huge scale costs minutes and gigabytes, and each strip staying under its own
/// deadline doesn't stop that.
#[derive(Clone, Copy, Debug)]
pub struct OutputCaps {
    /// Pixels on one page: 100 Mpx is the largest real page at 4x and then some, and 300 MiB as raw RGB.
    pub max_pixels: u64,
    /// Bytes of output, for anything writing a file.
    pub max_bytes: u64,
    pub max_time: Duration,
}

impl Default for OutputCaps {
    fn default() -> Self {
        Self { max_pixels: 100_000_000, max_bytes: 320 * 1024 * 1024, max_time: Duration::from_secs(60) }
    }
}

/// A page's size in device pixels, or a plain refusal if it's past the caps. Asks the worker once: callers keep this answer and pass it on, because a hostile
/// worker's second answer needn't match the first.
pub fn page_device_size(doc: &mut DocRenderer, page: u32, scale: f32, caps: &OutputCaps) -> Result<(u32, u32), RenderFailure> {
    let (width_points, height_points) = doc.page_size(page)?;
    let (width, height) = device_size(width_points, height_points, scale).map_err(|_| RenderFailure::TooLargeAtThisScale)?;
    if u64::from(width) * u64::from(height) > caps.max_pixels || width > recto_proto::MAX_TILE_EDGE {
        return Err(RenderFailure::TooLargeAtThisScale);
    }
    Ok((width, height))
}

/// What [`draw_whole_page`] hands over as it goes.
pub enum PageEvent<'a> {
    /// A picture of this size starts here. A second one means the first is abandoned: the page couldn't be drawn at the scale asked for.
    Begin { width: u32, height: u32, scale: f32, low_res: Option<LowResWhy> },
    /// The next rows, top to bottom.
    Rows { memory: &'a SharedMemory, rect: TileRect, stride: u32, kind: TileKind },
}

/// How a whole page came out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageDrawn {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Set if this is a lower-resolution version, and why.
    pub low_res: Option<LowResWhy>,
    pub render_us: u64,
}

/// Draws a whole page in strips no larger than [`STRIP_MAX_BYTES`], so it stays under the same limits as tiles. What `render` uses, and what print and snapshot will.
/// If the page can only be drawn at a lower resolution, it starts again at that scale and says so, rather than pasting one strip among others.
pub fn draw_whole_page(
    doc: &mut DocRenderer,
    page: u32,
    scale: f32,
    caps: &OutputCaps,
    surface: &mut Option<SharedMemory>,
    mut sink: impl FnMut(PageEvent<'_>) -> Result<(), RenderFailure>,
) -> Result<PageDrawn, RenderFailure> {
    let deadline = Instant::now() + caps.max_time;
    let (width, height) = page_device_size(doc, page, scale, caps)?;
    sink(PageEvent::Begin { width, height, scale, low_res: None })?;
    match strips(doc, DocRenderer::render_exact, page, scale, (width, height), deadline, surface, &mut sink) {
        Ok(render_us) => Ok(PageDrawn { width, height, scale, low_res: None, render_us }),
        Err(RenderFailure::LowResOnly { scale: lower, why, .. }) => {
            // Scaled from the same answer, not a new one: a hostile worker's second answer needn't match its first.
            let (width, height) = device_size_from(width, height, scale, lower)?;
            sink(PageEvent::Begin { width, height, scale: lower, low_res: Some(why) })?;
            let render_us = strips(doc, DocRenderer::render_low_res_pass, page, lower, (width, height), deadline, surface, &mut sink)?;
            Ok(PageDrawn { width, height, scale: lower, low_res: Some(why), render_us })
        }
        Err(failure) => Err(failure),
    }
}

fn device_size_from(width: u32, height: u32, scale: f32, lower: f32) -> Result<(u32, u32), RenderFailure> {
    let shrink = |edge: u32| ((f64::from(edge) * f64::from(lower) / f64::from(scale)).round() as u32).max(1);
    Ok((shrink(width), shrink(height)))
}

#[allow(clippy::too_many_arguments)]
fn strips(
    doc: &mut DocRenderer,
    draw: fn(&mut DocRenderer, &TileRequest, &SharedMemory) -> Result<TileResult, RenderFailure>,
    page: u32,
    scale: f32,
    (width, height): (u32, u32),
    deadline: Instant,
    surface: &mut Option<SharedMemory>,
    sink: &mut impl FnMut(PageEvent<'_>) -> Result<(), RenderFailure>,
) -> Result<u64, RenderFailure> {
    let stride = width.checked_mul(BYTES_PER_PIXEL).ok_or(RenderFailure::TooLargeAtThisScale)?;
    let strip_rows = (STRIP_MAX_BYTES / u64::from(stride)).clamp(1, u64::from(recto_proto::MAX_TILE_EDGE)) as u32;
    let strip_bytes = u64::from(stride) * u64::from(strip_rows.min(height));
    if surface.as_ref().is_none_or(|memory| (memory.len() as u64) < strip_bytes) {
        *surface = Some(SharedMemory::create(strip_bytes as usize).map_err(|_| RenderFailure::Unavailable)?);
    }
    let Some(memory) = surface.as_ref() else { return Err(RenderFailure::Unavailable) };
    let mut render_us = 0u64;
    let mut y = 0;
    while y < height {
        if Instant::now() >= deadline {
            return Err(RenderFailure::TookTooLong);
        }
        let rect = TileRect { x: 0, y, width, height: strip_rows.min(height - y) };
        match draw(doc, &TileRequest { page, rect, scale, stride, generation: 0 }, memory)? {
            TileResult::Done(tile) => {
                render_us += u64::from(tile.render_us);
                sink(PageEvent::Rows { memory, rect, stride, kind: tile.kind })?;
            }
            TileResult::Cancelled { .. } => return Err(RenderFailure::Unavailable),
        }
        y += rect.height;
    }
    Ok(render_us)
}

/// What to measure.
#[derive(Clone, Copy, Debug)]
pub struct BudgetOptions {
    pub first_page: u32,
    pub pages: u32,
    pub scale: f32,
    /// Give every page a fresh worker, to see what one page costs by itself.
    pub fresh_worker_per_page: bool,
    /// Draw 512 px tiles across each page, as the app will, rather than whole pages.
    pub tiles: bool,
    /// The whole run stops here, and says so.
    pub max_time: Duration,
}

/// How long `budget render` may run by default. Each page has its own deadline, but a long document at a heavy scale still adds up.
pub const BUDGET_MAX_TIME: Duration = Duration::from_secs(600);

/// One page's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSample {
    pub page: u32,
    /// Round trip, as a caller sees it.
    pub micros: u64,
    /// Inside the worker.
    pub render_us: u64,
    pub footprint_after_bytes: Option<u64>,
    pub failure: Option<&'static str>,
}

/// What a run of pages cost.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetReport {
    pub idle_footprint_bytes: u64,
    pub opened_footprint_bytes: u64,
    pub peak_footprint_bytes: u64,
    pub last_footprint_bytes: u64,
    /// The largest surface used. Shared memory counts in a footprint only while a process has it mapped and has touched it. The worker unmaps
    /// after each job and the host hasn't read these, so this memory is in nobody's footprint and must be added by whoever is counting.
    pub surface_bytes: u64,
    pub pages_drawn: u32,
    pub pages_failed: u32,
    /// Pages that came out at a lower resolution, in whole or in part.
    pub pages_low_res: u32,
    pub heavy_retries: u32,
    pub spawns: u32,
    pub retirements: u32,
    pub memory_kills: u32,
    pub deadline_kills: u32,
    pub crashes: u32,
    pub tiles_blank: u32,
    pub tiles_grey: u32,
    pub tiles_colour: u32,
    /// It reached its time cap with pages left to do.
    pub stopped_early: bool,
    pub samples: Vec<PageSample>,
}

/// Runs pages through a real worker and reports what it cost. Failures are reported per page and don't stop the run.
pub fn measure_budget(doc: &mut DocRenderer, options: BudgetOptions) -> Result<BudgetReport, RenderFailure> {
    let total = doc.page_count()?;
    let mut report = BudgetReport {
        idle_footprint_bytes: doc.stats().ready_footprint_bytes,
        opened_footprint_bytes: doc.stats().opened_footprint_bytes,
        ..BudgetReport::default()
    };
    let mut surface: Option<SharedMemory> = None;
    let end = options.first_page.saturating_add(options.pages).min(total);
    let run_deadline = Instant::now() + options.max_time;
    for page in options.first_page..end {
        if Instant::now() >= run_deadline {
            report.stopped_early = true;
            break;
        }
        if options.fresh_worker_per_page {
            doc.kill_worker();
        }
        let started = Instant::now();
        let low_res_before = doc.stats().low_res_tiles;
        let mut whole_low_res = false;
        let drawn = if options.tiles {
            draw_tiles(doc, page, options.scale, &mut surface, &mut report)
        } else {
            draw_whole(doc, page, options.scale, &mut surface, &mut report).map(|drawn| {
                whole_low_res = drawn.low_res.is_some();
                drawn.render_us
            })
        };
        report.pages_low_res += u32::from(whole_low_res || doc.stats().low_res_tiles > low_res_before);
        let micros = started.elapsed().as_micros() as u64;
        let footprint_after_bytes = doc.worker_footprint();
        report.last_footprint_bytes = footprint_after_bytes.unwrap_or(report.last_footprint_bytes);
        let (render_us, failure) = match drawn {
            Ok(render_us) => {
                report.pages_drawn += 1;
                (render_us, None)
            }
            Err(failure) => {
                report.pages_failed += 1;
                (0, Some(failure.code()))
            }
        };
        report.samples.push(PageSample { page, micros, render_us, footprint_after_bytes, failure });
    }
    report.surface_bytes = surface.as_ref().map_or(0, |memory| memory.len() as u64);
    let stats = doc.stats();
    report.peak_footprint_bytes = stats.peak_footprint_bytes;
    report.spawns = stats.spawns;
    report.retirements = stats.retirements;
    report.heavy_retries = stats.heavy_retries;
    report.memory_kills = stats.memory_kills;
    report.deadline_kills = stats.deadline_kills;
    report.crashes = stats.crashes;
    Ok(report)
}

fn count(report: &mut BudgetReport, kind: TileKind) {
    match kind {
        TileKind::Blank { .. } => report.tiles_blank += 1,
        TileKind::Grey => report.tiles_grey += 1,
        TileKind::Colour => report.tiles_colour += 1,
    }
}

fn draw_whole(doc: &mut DocRenderer, page: u32, scale: f32, surface: &mut Option<SharedMemory>, report: &mut BudgetReport) -> Result<PageDrawn, RenderFailure> {
    draw_whole_page(doc, page, scale, &OutputCaps::default(), surface, |event| {
        if let PageEvent::Rows { kind, .. } = event {
            count(report, kind);
        }
        Ok(())
    })
}

fn draw_tiles(doc: &mut DocRenderer, page: u32, scale: f32, surface: &mut Option<SharedMemory>, report: &mut BudgetReport) -> Result<u64, RenderFailure> {
    let caps = OutputCaps::default();
    let (width, height) = page_device_size(doc, page, scale, &caps)?;
    let deadline = Instant::now() + caps.max_time;
    let stride = TILE_EDGE * BYTES_PER_PIXEL;
    let tile_bytes = u64::from(stride) * u64::from(TILE_EDGE);
    if surface.as_ref().is_none_or(|memory| (memory.len() as u64) < tile_bytes) {
        *surface = Some(SharedMemory::create(tile_bytes as usize).map_err(|_| RenderFailure::Unavailable)?);
    }
    let Some(memory) = surface.as_ref() else { return Err(RenderFailure::Unavailable) };
    let mut render_us = 0;
    for y in (0..height).step_by(TILE_EDGE as usize) {
        for x in (0..width).step_by(TILE_EDGE as usize) {
            if Instant::now() >= deadline {
                return Err(RenderFailure::TookTooLong);
            }
            let rect = TileRect { x, y, width: TILE_EDGE.min(width - x), height: TILE_EDGE.min(height - y) };
            match doc.render_tile(&TileRequest { page, rect, scale, stride, generation: 0 }, memory)? {
                TileResult::Done(tile) => {
                    render_us += u64::from(tile.render_us);
                    count(report, tile.kind);
                }
                TileResult::Cancelled { .. } => return Err(RenderFailure::Unavailable),
            }
        }
    }
    Ok(render_us)
}
