//! `budget render`: what a worker costs, measured from outside on the real path. Also the shared way to draw a whole page in strips, so a
//! large page never needs a whole-page buffer.

use crate::host::{DocRenderer, DrawPurpose, RenderFailure, TileRequest, TileResult};
use recto_platform::isolate::SharedMemory;
use recto_proto::{BYTES_PER_PIXEL, TILE_EDGE, TileKind, TileRect, device_size};
use std::time::{Duration, Instant};

/// The most one strip of a page may hold. Whole-page drawing costs more memory the larger each piece is, and takes longer the smaller it is. Measured on the USGS map at
/// 4x: 64 MiB strips peak at 613 MiB in 1.5 s, 16 MiB at 450 in 3.4 s, 8 MiB at 423 in 5.6 s, 1 MiB at 405 in 38 s. At 2x, 8 MiB is 371 MiB in 0.7 s, no slower than
/// 64 MiB. A letter page at 2x is 7.7 MiB, so an ordinary page stays one strip. Memory first, so 8 MiB, which keeps every page measured under the normal limit. A page past
/// [`REGION_MIN_IMAGE_PIXELS`] uses [`recto_proto::MAX_REGION_BYTES`] instead.
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
    /// The picture's size, once, before any rows.
    Begin { width: u32, height: u32 },
    /// The next rows, top to bottom.
    Rows { memory: &'a SharedMemory, rect: TileRect, stride: u32, kind: TileKind },
}

/// How a whole page came out: sharp, at the scale asked for, always.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageDrawn {
    pub width: u32,
    pub height: u32,
    pub render_us: u64,
}

/// Draws a whole page in pieces under the same limits as tiles, for a file, a print or a snapshot. This is an export, in the sense of [`crate::DrawPurpose::Export`]: it comes out sharp or fails, and
/// a caller writing a file writes nothing on failure. So it can sleep waiting for the larger limit: call it on a job thread, never a UI thread. `generation` cancels it through
/// [`crate::CancelHandle`], which comes back as [`RenderFailure::Cancelled`].
pub fn draw_whole_page(
    doc: &mut DocRenderer,
    page: u32,
    scale: f32,
    generation: u64,
    caps: &OutputCaps,
    surface: &mut Option<SharedMemory>,
    mut sink: impl FnMut(PageEvent<'_>) -> Result<(), RenderFailure>,
) -> Result<PageDrawn, RenderFailure> {
    let started = Instant::now();
    let (width, height) = page_device_size(doc, page, scale, caps)?;
    sink(PageEvent::Begin { width, height })?;
    let render_us = strips(doc, page, scale, generation, (width, height), started, caps, surface, &mut sink)?;
    Ok(PageDrawn { width, height, render_us })
}

#[allow(clippy::too_many_arguments)]
fn strips(
    doc: &mut DocRenderer,
    page: u32,
    scale: f32,
    generation: u64,
    (width, height): (u32, u32),
    started: Instant,
    caps: &OutputCaps,
    surface: &mut Option<SharedMemory>,
    sink: &mut impl FnMut(PageEvent<'_>) -> Result<(), RenderFailure>,
) -> Result<u64, RenderFailure> {
    let stride = width.checked_mul(BYTES_PER_PIXEL).ok_or(RenderFailure::TooLargeAtThisScale)?;
    // A page with a huge image pays per pass, so it gets the largest piece the limits allow, as tiles do. The USGS map at 4x: 1.97 s at 32 MiB against 5.6 s at 8 MiB.
    let piece = if doc.largest_image_pixels(page, generation).unwrap_or(0) >= REGION_MIN_IMAGE_PIXELS { recto_proto::MAX_REGION_BYTES } else { STRIP_MAX_BYTES };
    let strip_rows = (piece / u64::from(stride)).clamp(1, u64::from(recto_proto::MAX_TILE_EDGE)) as u32;
    let strip_bytes = u64::from(stride) * u64::from(strip_rows.min(height));
    if surface.as_ref().is_none_or(|memory| (memory.len() as u64) < strip_bytes) {
        *surface = Some(SharedMemory::create(strip_bytes as usize).map_err(|_| RenderFailure::Unavailable)?);
    }
    let Some(memory) = surface.as_ref() else { return Err(RenderFailure::Unavailable) };
    let waited_before = doc.time_waiting_for_slot();
    let mut render_us = 0u64;
    let mut y = 0;
    while y < height {
        // Time asleep waiting for the larger limit isn't time drawing, and has a limit of its own.
        if started.elapsed().saturating_sub(doc.time_waiting_for_slot().saturating_sub(waited_before)) >= caps.max_time {
            return Err(RenderFailure::TookTooLong);
        }
        let rect = TileRect { x: 0, y, width, height: strip_rows.min(height - y) };
        match doc.render_for(DrawPurpose::Export, &TileRequest { page, rect, scale, stride, generation }, memory)? {
            TileResult::Done(tile) => {
                render_us += u64::from(tile.render_us);
                sink(PageEvent::Rows { memory, rect, stride, kind: tile.kind })?;
            }
            TileResult::Cancelled { .. } => return Err(RenderFailure::Cancelled),
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
    /// Pages whose tiles came out at a lower resolution, in whole or in part. Only with tiles: whole-page drawing is an export, which never does.
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
        let drawn = if options.tiles {
            draw_tiles(doc, page, options.scale, &mut surface, &mut report)
        } else {
            draw_whole(doc, page, options.scale, &mut surface, &mut report).map(|drawn| drawn.render_us)
        };
        report.pages_low_res += u32::from(doc.stats().low_res_tiles > low_res_before);
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
    draw_whole_page(doc, page, scale, 0, &OutputCaps::default(), surface, |event| {
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
    let grid: Vec<(u32, u32)> = (0..height.div_ceil(TILE_EDGE)).flat_map(|row| (0..width.div_ceil(TILE_EDGE)).map(move |column| (column, row))).collect();
    let mut render_us = 0;
    // The same choice the app will make, so this measures what a whole page in tiles will really cost.
    let passes = if doc.largest_image_pixels(page, 0).unwrap_or(0) >= REGION_MIN_IMAGE_PIXELS {
        plan_regions(&grid, (width, height)).into_iter().map(|rect| (rect, TILE_EDGE)).collect()
    } else {
        grid.iter().map(|&place| (tile_rect(place, (width, height)), 0)).collect::<Vec<_>>()
    };
    for (rect, tile_edge) in passes {
        if Instant::now() >= deadline {
            return Err(RenderFailure::TookTooLong);
        }
        let stride = if tile_edge == 0 { TILE_EDGE * BYTES_PER_PIXEL } else { rect.width * BYTES_PER_PIXEL };
        let bytes = u64::from(stride) * u64::from(rect.height.max(TILE_EDGE));
        if surface.as_ref().is_none_or(|memory| (memory.len() as u64) < bytes) {
            *surface = Some(SharedMemory::create(bytes as usize).map_err(|_| RenderFailure::Unavailable)?);
        }
        let Some(memory) = surface.as_ref() else { return Err(RenderFailure::Unavailable) };
        let request = TileRequest { page, rect, scale, stride, generation: 0 };
        let result = if tile_edge == 0 { doc.render_tile(&request, memory)? } else { doc.render_region(&request, tile_edge, memory)? };
        match result {
            TileResult::Done(tile) => {
                render_us += u64::from(tile.render_us);
                if tile.region_kinds.is_empty() {
                    count(report, tile.kind);
                }
                for kind in tile.region_kinds {
                    count(report, kind);
                }
            }
            TileResult::Cancelled { .. } => return Err(RenderFailure::Unavailable),
        }
    }
    Ok(render_us)
}

/// How a viewport's tiles get drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawStrategy {
    /// One render pass per 512 px tile.
    PerTile,
    /// One render pass per region of up to [`recto_proto::MAX_REGION_BYTES`], with tiles handed out of it.
    Region,
    /// Whichever suits each page: see [`REGION_MIN_IMAGE_PIXELS`].
    Auto,
}

/// Pages with an image of at least this many source pixels draw in regions, and others per tile. Measured over 369 pages of bench100, at 2x in a 3024 x 1964 window, total
/// time per tile against region, and mean peak:
///
/// | Largest image | Pages | ms per tile | ms region | Peak MiB per tile | Peak MiB region |
/// |---|---:|---:|---:|---:|---:|
/// | None | 208 | 8,149 | 7,789 | 11 | 26 |
/// | Under 1 Mpx | 51 | 4,419 | 4,108 | 24 | 40 |
/// | 1 to 4 Mpx | 51 | 9,658 | 12,879 | 43 | 103 |
/// | 4 to 10 Mpx | 38 | 39,066 | 60,402 | 120 | 280 |
/// | 10 to 20 Mpx | 17 | 3,640 | 3,812 | 90 | 125 |
/// | Over 20 Mpx | 4 | 12,509 | 2,740 | 235 | 316 |
///
/// So regions pay only for images past 20 Mpx, where they're 4.6 times faster. Below that they cost more memory, and between 1 and 20 Mpx more time as well, because PDFium's
/// buffers for transparency and shadings grow with the area drawn. Text pages gain 4% and pay 15 MiB, so memory first: they stay per tile.
pub const REGION_MIN_IMAGE_PIXELS: u64 = 20_000_000;

/// A simulated window onto a run of pages stacked top to bottom, as the reader lays them out.
#[derive(Clone, Copy, Debug)]
pub struct ViewportOptions {
    pub first_page: u32,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    /// Tiles beyond the visible area to draw, on each side.
    pub margin_tiles: u32,
    pub strategy: DrawStrategy,
}

/// One step of the simulation: what it needed and what it cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportStep {
    pub name: &'static str,
    pub micros: u64,
    pub tiles: u32,
    pub passes: u32,
    pub blank: u32,
    pub grey: u32,
    pub colour: u32,
    pub low_res: u32,
    pub failed: u32,
    /// The worker's highest footprint so far.
    pub peak_footprint_bytes: u64,
}

/// A tile of a page at a scale, by its place in the grid.
type TilePlace = (u32, u32, u32);

/// Measures what a reader does: the first view of a page (worker start included), one scroll step of a quarter of the window, and one zoom step of 1.5x.
pub fn measure_viewport(doc: &mut DocRenderer, options: ViewportOptions) -> Result<Vec<ViewportStep>, RenderFailure> {
    let mut sizes: std::collections::HashMap<u32, (f32, f32)> = std::collections::HashMap::new();
    let mut cached: std::collections::HashSet<TilePlace> = std::collections::HashSet::new();
    let mut surface: Option<SharedMemory> = None;
    let mut hand_out = vec![0u8; (TILE_EDGE * TILE_EDGE * BYTES_PER_PIXEL) as usize];
    let mut steps = Vec::new();
    let quarter = u64::from(options.height / 4);
    let zoomed = options.scale * 1.5;
    let started_run = Instant::now();
    for (name, scale, top) in [("first_view", options.scale, 0u64), ("scroll", options.scale, quarter), ("zoom", zoomed, (quarter as f64 * 1.5) as u64)] {
        if steps.is_empty() {
            cached.clear();
        }
        if name == "zoom" {
            // A new scale needs all new tiles.
            cached.clear();
        }
        let started = Instant::now();
        let mut step = ViewportStep { name, micros: 0, tiles: 0, passes: 0, blank: 0, grey: 0, colour: 0, low_res: 0, failed: 0, peak_footprint_bytes: 0 };
        let places = visible_places(doc, &options, scale, top, &mut sizes)?;
        for (page, columns, rows, width, height) in places {
            let needed: Vec<(u32, u32)> =
                rows.clone().flat_map(|row| columns.clone().map(move |column| (column, row))).filter(|&(column, row)| !cached.contains(&(page, column, row))).collect();
            if needed.is_empty() {
                continue;
            }
            if started_run.elapsed() >= BUDGET_MAX_TIME {
                step.failed += needed.len() as u32;
                continue;
            }
            step.tiles += needed.len() as u32;
            let use_regions = match options.strategy {
                DrawStrategy::PerTile => false,
                DrawStrategy::Region => true,
                // More than one tile, or there's nothing to gain, and a page known to be worth it.
                DrawStrategy::Auto => needed.len() > 1 && doc.largest_image_pixels(page, 0).unwrap_or(0) >= REGION_MIN_IMAGE_PIXELS,
            };
            let mut sharp = Vec::new();
            if use_regions {
                draw_regions(doc, page, scale, &needed, (width, height), &mut surface, &mut hand_out, &mut step, &mut sharp);
            } else {
                draw_per_tile(doc, page, scale, &needed, (width, height), &mut surface, &mut step, &mut sharp);
            }
            // Only what really arrived, sharp. Anything else has to be asked for again on the next step, as it will in the app, and should cost that here too.
            cached.extend(sharp.into_iter().map(|(column, row)| (page, column, row)));
        }
        step.micros = started.elapsed().as_micros() as u64;
        step.peak_footprint_bytes = doc.stats().peak_footprint_bytes.max(doc.worker_footprint().unwrap_or(0));
        steps.push(step);
    }
    Ok(steps)
}

/// The pages in view from `top` of the stack, each with the grid columns and rows that show, margin included.
#[allow(clippy::type_complexity)]
fn visible_places(
    doc: &mut DocRenderer,
    options: &ViewportOptions,
    scale: f32,
    top: u64,
    sizes: &mut std::collections::HashMap<u32, (f32, f32)>,
) -> Result<Vec<(u32, std::ops::Range<u32>, std::ops::Range<u32>, u32, u32)>, RenderFailure> {
    let total = doc.page_count()?;
    let bottom = top + u64::from(options.height);
    let mut places = Vec::new();
    let mut page_top = 0u64;
    let mut page = options.first_page;
    while page < total && page_top < bottom {
        let (width_points, height_points) = match sizes.get(&page) {
            Some(&size) => size,
            None => {
                let size = doc.page_size(page)?;
                sizes.insert(page, size);
                size
            }
        };
        let (width, height) = device_size(width_points, height_points, scale).map_err(|_| RenderFailure::TooLargeAtThisScale)?;
        let page_bottom = page_top + u64::from(height);
        if page_bottom > top {
            let in_top = top.saturating_sub(page_top) as u32;
            let in_bottom = (bottom - page_top).min(u64::from(height)) as u32;
            let margin = options.margin_tiles;
            let rows = (in_top / TILE_EDGE).saturating_sub(margin)..(in_bottom.div_ceil(TILE_EDGE) + margin).min(height.div_ceil(TILE_EDGE));
            let columns = 0..(options.width.min(width).div_ceil(TILE_EDGE) + margin).min(width.div_ceil(TILE_EDGE));
            places.push((page, columns, rows, width, height));
        }
        page_top = page_bottom;
        page += 1;
    }
    Ok(places)
}

fn tile_rect((column, row): (u32, u32), (width, height): (u32, u32)) -> TileRect {
    let (x, y) = (column * TILE_EDGE, row * TILE_EDGE);
    TileRect { x, y, width: TILE_EDGE.min(width - x), height: TILE_EDGE.min(height - y) }
}

fn count_step(step: &mut ViewportStep, kind: TileKind) {
    match kind {
        TileKind::Blank { .. } => step.blank += 1,
        TileKind::Grey => step.grey += 1,
        TileKind::Colour => step.colour += 1,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_per_tile(doc: &mut DocRenderer, page: u32, scale: f32, needed: &[(u32, u32)], size: (u32, u32), surface: &mut Option<SharedMemory>, step: &mut ViewportStep, sharp: &mut Vec<(u32, u32)>) {
    let stride = TILE_EDGE * BYTES_PER_PIXEL;
    let bytes = (stride * TILE_EDGE) as usize;
    if surface.as_ref().is_none_or(|memory| memory.len() < bytes) {
        *surface = SharedMemory::create(bytes).ok();
    }
    let Some(memory) = surface.as_ref() else {
        step.failed += needed.len() as u32;
        return;
    };
    for &place in needed {
        step.passes += 1;
        match doc.render_tile(&TileRequest { page, rect: tile_rect(place, size), scale, stride, generation: 0 }, memory) {
            Ok(TileResult::Done(tile)) => {
                step.low_res += u32::from(tile.low_res.is_some());
                count_step(step, tile.kind);
                if tile.low_res.is_none() {
                    sharp.push(place);
                }
            }
            _ => step.failed += 1,
        }
    }
}

/// The cheapest way to cover `needed` in passes under the cap: rows of the same columns merge, because a scroll step's new tiles are a band, and a band too large for
/// one region splits by rows.
#[must_use]
pub fn plan_regions(needed: &[(u32, u32)], (width, height): (u32, u32)) -> Vec<TileRect> {
    let Some(&(first_column, first_row)) = needed.first() else { return Vec::new() };
    let (mut left, mut right, mut top, mut bottom) = (first_column, first_column, first_row, first_row);
    for &(column, row) in needed {
        left = left.min(column);
        right = right.max(column);
        top = top.min(row);
        bottom = bottom.max(row);
    }
    let x = left * TILE_EDGE;
    let region_width = ((right + 1) * TILE_EDGE).min(width) - x;
    let row_bytes = u64::from(region_width) * u64::from(BYTES_PER_PIXEL) * u64::from(TILE_EDGE);
    let rows_per_region = (recto_proto::MAX_REGION_BYTES / row_bytes).max(1) as u32;
    let mut regions = Vec::new();
    let mut row = top;
    while row <= bottom {
        let last = (row + rows_per_region - 1).min(bottom);
        let y = row * TILE_EDGE;
        regions.push(TileRect { x, y, width: region_width, height: ((last + 1) * TILE_EDGE).min(height) - y });
        row = last + 1;
    }
    regions
}

#[allow(clippy::too_many_arguments)]
fn draw_regions(
    doc: &mut DocRenderer,
    page: u32,
    scale: f32,
    needed: &[(u32, u32)],
    size: (u32, u32),
    surface: &mut Option<SharedMemory>,
    hand_out: &mut [u8],
    step: &mut ViewportStep,
    sharp: &mut Vec<(u32, u32)>,
) {
    for rect in plan_regions(needed, size) {
        let stride = rect.width * BYTES_PER_PIXEL;
        let bytes = stride as usize * rect.height as usize;
        if surface.as_ref().is_none_or(|memory| memory.len() < bytes) {
            *surface = SharedMemory::create(bytes).ok();
        }
        let Some(memory) = surface.as_ref() else {
            step.failed += 1;
            continue;
        };
        step.passes += 1;
        match doc.render_region(&TileRequest { page, rect, scale, stride, generation: 0 }, TILE_EDGE, memory) {
            Ok(TileResult::Done(tile)) if tile.low_res.is_none() => {
                for (place, kind) in recto_proto::region_tiles(rect, TILE_EDGE).zip(tile.region_kinds) {
                    count_step(step, kind);
                    sharp.push((place.x / TILE_EDGE, place.y / TILE_EDGE));
                    // What the app will do for each tile with pixels: copy it into its own surface. Blank tiles need none.
                    if !matches!(kind, TileKind::Blank { .. }) {
                        let row_bytes = place.width as usize * BYTES_PER_PIXEL as usize;
                        for row in 0..place.height as usize {
                            let offset = (place.y - rect.y) as usize * stride as usize + (place.x - rect.x) as usize * BYTES_PER_PIXEL as usize + row * stride as usize;
                            memory.copy_out(offset, &mut hand_out[..row_bytes]);
                        }
                    }
                }
            }
            Ok(TileResult::Done(tile)) => {
                step.low_res += 1;
                count_step(step, tile.kind);
            }
            _ => step.failed += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(columns: std::ops::Range<u32>, rows: std::ops::Range<u32>) -> Vec<(u32, u32)> {
        rows.flat_map(|row| columns.clone().map(move |column| (column, row))).collect()
    }

    /// Failure: a region runs past the cap, or tiles are left out of the plan, so a view has holes or a pass costs more than a viewport's worth.
    #[test]
    fn regions_cover_what_is_needed_and_stay_under_the_cap() {
        let size = (8928, 8064);
        let needed = grid(0..16, 0..12);
        let regions = plan_regions(&needed, size);
        assert!(regions.len() > 1, "this view is past one region's cap");
        for region in &regions {
            let bytes = u64::from(region.width) * u64::from(region.height) * u64::from(BYTES_PER_PIXEL);
            assert!(bytes <= recto_proto::MAX_REGION_BYTES, "{region:?} is {bytes} bytes");
            assert_eq!(region.x % TILE_EDGE, 0, "{region:?}");
            assert_eq!(region.y % TILE_EDGE, 0, "{region:?}");
        }
        let covered: std::collections::HashSet<(u32, u32)> = regions
            .iter()
            .flat_map(|region| recto_proto::region_tiles(*region, TILE_EDGE))
            .map(|tile| (tile.x / TILE_EDGE, tile.y / TILE_EDGE))
            .collect();
        for place in needed {
            assert!(covered.contains(&place), "{place:?} was left out");
        }
    }

    /// Failure: a region runs off the page at its right or bottom edge, and the worker refuses the whole pass.
    #[test]
    fn regions_stop_at_the_page_edge() {
        let regions = plan_regions(&grid(0..3, 0..4), (1224, 1584));
        let last = regions.last().unwrap();
        assert_eq!(last.x + last.width, 1224);
        assert_eq!(last.y + last.height, 1584);
    }

    /// Failure: nothing needed still plans a pass, or a scroll's single new band becomes one pass per tile.
    #[test]
    fn a_band_of_new_tiles_is_one_pass() {
        assert!(plan_regions(&[], (1224, 1584)).is_empty());
        let band = plan_regions(&grid(0..6, 4..5), (4464, 4032));
        assert_eq!(band, vec![TileRect { x: 0, y: 2048, width: 3072, height: 512 }]);
    }

    /// Failure: one very wide row is past the cap by itself and the plan divides by zero or loops. It should still come out, one row at a time, for the host to refuse plainly.
    #[test]
    fn a_row_past_the_cap_doesnt_break_the_plan() {
        let regions = plan_regions(&grid(0..40, 0..2), (20_480, 1024));
        assert_eq!(regions.len(), 2);
    }
}
