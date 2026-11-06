//! `budget render`: what a worker costs, measured from outside on the real path. Also the shared way to draw a whole page in strips, so a
//! large page never needs a whole-page buffer.

use crate::host::{DocRenderer, RenderFailure, TileRequest, TileResult};
use recto_platform::isolate::SharedMemory;
use recto_proto::{BYTES_PER_PIXEL, TILE_EDGE, TileKind, TileRect, device_size};
use std::time::Instant;

/// The most one strip of a page may hold. A letter page at 2x is 7.7 MiB; the largest real page at 2x is about 69 MiB.
pub const STRIP_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Draws a whole page into `surface`, in strips no larger than [`STRIP_MAX_BYTES`], calling `each` with every strip's rows. Grows
/// `surface` once if it's too small. Used by `render` and `budget render`.
pub fn for_each_strip(
    doc: &mut DocRenderer,
    page: u32,
    scale: f32,
    surface: &mut Option<SharedMemory>,
    mut each: impl FnMut(&SharedMemory, TileRect, u32, TileKind) -> Result<(), RenderFailure>,
) -> Result<(u32, u32, u64), RenderFailure> {
    let (width_points, height_points) = doc.page_size(page)?;
    let (width, height) = device_size(width_points, height_points, scale).map_err(|_| RenderFailure::TooLargeAtThisScale)?;
    if width > recto_proto::MAX_TILE_EDGE {
        return Err(RenderFailure::TooLargeAtThisScale);
    }
    let stride = width * BYTES_PER_PIXEL;
    let strip_rows = (STRIP_MAX_BYTES / u64::from(stride)).clamp(1, u64::from(recto_proto::MAX_TILE_EDGE)) as u32;
    let strip_bytes = u64::from(stride) * u64::from(strip_rows.min(height));
    if surface.as_ref().is_none_or(|memory| (memory.len() as u64) < strip_bytes) {
        *surface = Some(SharedMemory::create(strip_bytes as usize).map_err(|_| RenderFailure::Unavailable)?);
    }
    let Some(memory) = surface.as_ref() else { return Err(RenderFailure::Unavailable) };
    let mut render_us = 0u64;
    let mut y = 0;
    while y < height {
        let rect = TileRect { x: 0, y, width, height: strip_rows.min(height - y) };
        match doc.render_tile(&TileRequest { page, rect, scale, stride, generation: 0 }, memory)? {
            TileResult::Done(tile) => {
                render_us += u64::from(tile.render_us);
                each(memory, rect, stride, tile.kind)?;
            }
            TileResult::Cancelled { .. } => return Err(RenderFailure::Unavailable),
        }
        y += rect.height;
    }
    Ok((width, height, render_us))
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
}

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
    pub pages_drawn: u32,
    pub pages_failed: u32,
    pub spawns: u32,
    pub retirements: u32,
    pub memory_kills: u32,
    pub deadline_kills: u32,
    pub crashes: u32,
    pub tiles_blank: u32,
    pub tiles_grey: u32,
    pub tiles_colour: u32,
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
    for page in options.first_page..end {
        if options.fresh_worker_per_page {
            doc.kill_worker();
        }
        let started = Instant::now();
        let drawn = if options.tiles { draw_tiles(doc, page, options.scale, &mut surface, &mut report) } else { draw_whole(doc, page, options.scale, &mut surface, &mut report) };
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
    let stats = doc.stats();
    report.peak_footprint_bytes = stats.peak_footprint_bytes;
    report.spawns = stats.spawns;
    report.retirements = stats.retirements;
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

fn draw_whole(doc: &mut DocRenderer, page: u32, scale: f32, surface: &mut Option<SharedMemory>, report: &mut BudgetReport) -> Result<u64, RenderFailure> {
    let (_, _, render_us) = for_each_strip(doc, page, scale, surface, |_, _, _, kind| {
        count(report, kind);
        Ok(())
    })?;
    Ok(render_us)
}

fn draw_tiles(doc: &mut DocRenderer, page: u32, scale: f32, surface: &mut Option<SharedMemory>, report: &mut BudgetReport) -> Result<u64, RenderFailure> {
    let (width_points, height_points) = doc.page_size(page)?;
    let (width, height) = device_size(width_points, height_points, scale).map_err(|_| RenderFailure::TooLargeAtThisScale)?;
    let stride = TILE_EDGE * BYTES_PER_PIXEL;
    let tile_bytes = u64::from(stride) * u64::from(TILE_EDGE);
    if surface.as_ref().is_none_or(|memory| (memory.len() as u64) < tile_bytes) {
        *surface = Some(SharedMemory::create(tile_bytes as usize).map_err(|_| RenderFailure::Unavailable)?);
    }
    let Some(memory) = surface.as_ref() else { return Err(RenderFailure::Unavailable) };
    let mut render_us = 0;
    for y in (0..height).step_by(TILE_EDGE as usize) {
        for x in (0..width).step_by(TILE_EDGE as usize) {
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
