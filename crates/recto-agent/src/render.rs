//! The renderer's commands. A transient worker per command for now, because the agent holds no state: it starts, answers, and goes away
//! with the call. The session's scheduler will own long-lived ones, behind the same calls.

use crate::{AgentError, exit};
use recto_index::{hidden_report, read_page};
use recto_proto::{BYTES_PER_PIXEL, TileKind};
use recto_render_service::{
    BUDGET_MAX_TIME, BudgetOptions, DocRenderer, DrawStrategy, ExportRefusal, OutputCaps, PageDrawn, PageEvent, Policy, RenderFailure, ServiceConfig, ViewportOptions, draw_whole_page, largest_scale_that_fits, measure_budget, measure_viewport,
};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

mod png;
mod svg;

pub(crate) use svg::{SvgArgs, export_svg};

pub const DEFAULT_SCALE: f32 = 2.0;

pub(crate) fn default_scale() -> f32 {
    DEFAULT_SCALE
}

/// `render --scale max`, on its way to where the page's size is known. Nothing else can reach this: JSON has no infinity, and a number too big for a scale is refused on the way in.
pub(crate) const LARGEST_THAT_FITS: f32 = f32::INFINITY;

/// A scale off the wire: a number, or the word `max`.
pub(crate) fn scale_or_max<'de, D: serde::Deserializer<'de>>(from: D) -> Result<f32, D::Error> {
    match <Value as serde::Deserialize>::deserialize(from)? {
        Value::String(word) if word == "max" => Ok(LARGEST_THAT_FITS),
        Value::Number(number) => number.as_f64().map(|scale| scale as f32).filter(|scale| scale.is_finite()).ok_or_else(|| serde::de::Error::custom("that scale is out of range")),
        _ => Err(serde::de::Error::custom("a scale is a number, or max")),
    }
}

/// How high `budget render` may raise a worker's limit. A measurement shouldn't be a way to take the whole Mac.
const MAX_LIMIT_MIB: u32 = 4096;

impl From<RenderFailure> for AgentError {
    fn from(failure: RenderFailure) -> Self {
        let exit = match failure {
            RenderFailure::NotFound | RenderFailure::BadPage => exit::NOT_FOUND,
            RenderFailure::NeedsPassword => exit::PROTECTED,
            RenderFailure::ExportRefused { why: ExportRefusal::WaitTimedOut, .. } | RenderFailure::TookTooLong => exit::TIMED_OUT,
            _ => exit::FAILED,
        };
        Self::new(failure.code(), failure.message(), exit)
    }
}

fn open(path: &Path, policy: Policy) -> Result<DocRenderer, AgentError> {
    Ok(DocRenderer::new(path, None, ServiceConfig::discover()?, policy)?)
}

/// One-based pages for people and agents, zero-based inside.
fn page_index(page: u32) -> Result<u32, AgentError> {
    page.checked_sub(1).ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))
}

pub(crate) fn render(path: &Path, page: u32, scale: f32, out: &Path) -> Result<Value, AgentError> {
    render_with_caps(path, page, scale, out, &OutputCaps::default())
}

/// A temp file this process created, next to where the picture is going. Only ever deletes itself: never a file that was already there.
struct OwnTemp {
    path: PathBuf,
}

static NEXT_TEMP: AtomicU32 = AtomicU32::new(0);

impl OwnTemp {
    /// Makes a new file with a name nobody else has. `create_new` is `O_CREAT | O_EXCL`, which refuses an existing name and doesn't follow a symlink.
    fn create(out: &Path) -> Result<(Self, File), AgentError> {
        let failed = || AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED);
        if out.file_name().is_none() {
            return Err(failed());
        }
        for _ in 0..64 {
            let attempt = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            // Short and its own, not built from the output's name: a name near the file system's limit must still have a temp file.
            let path = out.with_file_name(format!(".recto-{}-{attempt}.partial", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self { path }, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(failed()),
            }
        }
        Err(failed())
    }
}

impl Drop for OwnTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Puts a finished picture at `out`, never over anything. A hard link does it in one step, so a stopped renderer leaves no half a picture under the real name, and a file that appeared meanwhile
/// stays as it was. A volume without hard links, such as exFAT, gets the same refusal through `create_new`, then a copy, removed if the copy fails.
fn publish(temp: &Path, out: &Path, link: impl Fn(&Path, &Path) -> std::io::Result<()>) -> Result<(), AgentError> {
    let failed = || AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED);
    match link(temp, out) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(output_exists()),
        Err(_) => {
            let mut target = match OpenOptions::new().write(true).create_new(true).open(out) {
                Ok(target) => target,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Err(output_exists()),
                Err(_) => return Err(failed()),
            };
            let copied = File::open(temp).and_then(|mut source| std::io::copy(&mut source, &mut target));
            if copied.is_err() {
                let _ = std::fs::remove_file(out);
                return Err(failed());
            }
            Ok(())
        }
    }
}

/// Puts a finished picture at `out`, never over anything, as `render` does.
pub fn publish_picture(temp: &Path, out: &Path) -> Result<(), AgentError> {
    publish(temp, out, |from, to| std::fs::hard_link(from, to))
}

/// What a request for a place that has something in it hears.
pub fn output_exists() -> AgentError {
    AgentError::new("output_exists", "A file is already there, and it won't be replaced.", exit::FAILED)
}

fn render_with_caps(path: &Path, page: u32, scale: f32, out: &Path, caps: &OutputCaps) -> Result<Value, AgentError> {
    render_with(ServiceConfig::discover()?, path, page, scale, out, caps)
}

fn render_with(config: ServiceConfig, path: &Path, page: u32, scale: f32, out: &Path, caps: &OutputCaps) -> Result<Value, AgentError> {
    let page = page_index(page)?;
    // Only ever a new picture. This also keeps `out` off the PDF itself, because only the append writer may change one.
    if out.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("png")) {
        return Err(AgentError::new("bad_output", "The output file must end in .png.", exit::FAILED));
    }
    let largest = scale == LARGEST_THAT_FITS;
    if !largest && !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&scale) {
        return Err(AgentError::new("bad_scale", format!("Scale must be between {} and {}.", recto_proto::MIN_SCALE, recto_proto::MAX_SCALE), exit::FAILED));
    }
    // `symlink_metadata`, so a dangling link counts as being there. Early, to save the drawing. The link below is what actually holds.
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    let started = Instant::now();
    let policy = Policy::default();
    let limit = policy.heavy_limit_bytes;
    let mut doc = DocRenderer::new(path, None, config, policy)?;
    let scale = if largest { largest_scale(&mut doc, page, caps)? } else { scale };
    let (temp, drawn, kind) = write_page(&mut doc, page, scale, out, caps, limit).map_err(|error| {
        let error = if error.code == "too_large_at_this_scale" { naming_the_largest_scale(error, &mut doc, page, scale, caps) } else { error };
        with_time_taken(error, started)
    })?;
    publish(&temp.path, out, |from, to| std::fs::hard_link(from, to))?;
    drop(temp);
    Ok(json!({
        "page": page + 1,
        "scale": as_written(scale),
        "largest_that_fits": largest,
        "out": out,
        "took_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "width": drawn.width,
        "height": drawn.height,
        "bytes": std::fs::metadata(out).map(|meta| meta.len()).ok(),
        "kind": kind,
    }))
}

/// The largest scale this page can be drawn at, or a plain refusal if there's none. An export still goes down the same limits from here: a page whose content is too heavy at this scale is refused for that, sharp or not at all.
fn largest_scale(doc: &mut DocRenderer, page: u32, caps: &OutputCaps) -> Result<f32, AgentError> {
    let (width, height) = doc.page_size(page)?;
    largest_scale_that_fits(width, height, caps).ok_or_else(|| AgentError::new("too_large_at_this_scale", format!("Page {} is {width} x {height} points, and no scale fits: a picture is at most {} Mpx.", page + 1, caps.max_pixels / 1_000_000), exit::FAILED).then("Tell the user this page can't be drawn."))
}

/// A scale as it was written. A float widened as it stands reads 4.71 as 4.71000003814697, in JSON and in plain words.
fn as_written(scale: f32) -> f64 {
    scale.to_string().parse().unwrap_or_else(|_| f64::from(scale))
}

/// Says the largest scale that fits this page, from its size and the caps, so an agent doesn't try 8, 4, 2 and 1 by hand. It's about size alone, and a heavy page can still be refused for memory, in words of its own.
fn naming_the_largest_scale(mut error: AgentError, doc: &mut DocRenderer, page: u32, scale: f32, caps: &OutputCaps) -> AgentError {
    let Ok((width, height)) = doc.page_size(page) else { return error };
    error.message = format!("Page {} is {width} x {height} points, too large to draw at {scale}x: a picture is at most {} Mpx.", page + 1, caps.max_pixels / 1_000_000);
    match largest_scale_that_fits(width, height, caps) {
        Some(most) => {
            error.message.push_str(&format!(" The largest scale that fits is {most}."));
            error.next = format!("Use `--scale {most}` or less. A page with heavy images can still be refused for memory, and that refusal says so.");
            error.candidates = vec![format!("--scale {most}")];
        }
        None => {
            error.message.push_str(" No scale fits.");
            error.next = "Tell the user this page can't be drawn.".to_string();
        }
    }
    error
}

/// Writes the page to a new temp file, as an export: sharp or not at all, and any failure takes the temp file with it. Returns the file, ready to publish. Can sleep waiting for the larger
/// limit, which is fine: this runs on a command's thread, never a UI thread.
fn write_page(doc: &mut DocRenderer, page: u32, scale: f32, out: &Path, caps: &OutputCaps, limit: u64) -> Result<(OwnTemp, PageDrawn, &'static str), AgentError> {
    let write_failed = || AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED);
    let mut size = None;
    let mut surface = None;
    let mut kind = None;
    let mut picture: Option<(OwnTemp, png::RgbWriter, Vec<u8>)> = None;
    let mut sink_error = None;
    let drawn = draw_whole_page(doc, page, scale, 0, caps, &mut surface, |event| match event {
        PageEvent::Begin { width, height } => {
            size = Some((width, height));
            let started = OwnTemp::create(out).and_then(|(temp, file)| {
                let writer = png::RgbWriter::create(file, width, height, caps.max_bytes).map_err(|_| write_failed())?;
                Ok((temp, writer, vec![0u8; width as usize * BYTES_PER_PIXEL as usize]))
            });
            match started {
                Ok(started) => {
                    picture = Some(started);
                    Ok(())
                }
                Err(error) => {
                    sink_error = Some(error);
                    Err(RenderFailure::Unavailable)
                }
            }
        }
        PageEvent::Rows { memory, rect, stride, kind: strip_kind } => {
            kind = Some(merge_kinds(kind, strip_kind));
            let Some((_, writer, row)) = picture.as_mut() else { return Err(RenderFailure::Unavailable) };
            for y in 0..rect.height as usize {
                // A copy out, never a look in place: the worker still holds this memory, and it isn't trusted.
                if !memory.copy_out(y * stride as usize, row) {
                    return Err(RenderFailure::Unavailable);
                }
                if let Err(error) = writer.write_bgrx_row(row) {
                    sink_error = Some(write_error_to_agent(&error));
                    return Err(RenderFailure::Unavailable);
                }
            }
            Ok(())
        }
    })
    .map_err(|failure| {
        sink_error.take().unwrap_or_else(|| match (failure, size) {
            (RenderFailure::ExportRefused { why: ExportRefusal::TooHeavy, .. }, Some(size)) => too_heavy_to_export(failure, page, scale, size, limit),
            _ => failure.into(),
        })
    })?;
    let (temp, writer, _) = picture.ok_or_else(write_failed)?;
    writer.finish().map_err(|error| write_error_to_agent(&error))?;
    Ok((temp, drawn, kind.map_or("blank", kind_name)))
}

/// What a picture must weigh, as a share of the limit, before a smaller scale can honestly be suggested. Below this, what's using the memory is the page's content: a hostile form takes all it's given at any size, and even a heavy map's peak barely moves with scale (468 MiB at 4x, 394 MiB at 2x).
const SCALE_MATTERS_SHARE: u64 = 8;

/// Says the limit, and suggests a smaller scale only if that can help: the picture itself must be a real share of the limit, and the scale must have somewhere to go.
fn too_heavy_to_export(failure: RenderFailure, page: u32, scale: f32, (width, height): (u32, u32), limit: u64) -> AgentError {
    let picture = u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL);
    let mut error = AgentError::from(failure);
    error.message = format!("Page {} wasn't exported, and no file was written: drawing it needs more than the {} one page may use.", page + 1, crate::human::bytes(limit));
    let large = picture.saturating_mul(SCALE_MATTERS_SHARE) >= limit;
    if large && scale > recto_proto::MIN_SCALE {
        error.message.push_str(&format!(" The picture alone is {} at {scale}x.", crate::human::bytes(picture)));
        error.next = "A smaller `--scale` may fit.".to_string();
    } else if large {
        error.message.push_str(&format!(" The picture alone is {} at {scale}x, the smallest scale there is.", crate::human::bytes(picture)));
        error.next = "Tell the user this page can't be drawn.".to_string();
    } else {
        error.message.push_str(&format!(" The picture is only {} at {scale}x, so it's what's on the page that uses the memory.", crate::human::bytes(picture)));
        error.next = "A smaller scale won't help. Tell the user this page can't be drawn.".to_string();
    }
    error
}

/// Says how long a failure took, when that was long enough to matter to whoever's waiting.
fn with_time_taken(mut error: AgentError, started: Instant) -> AgentError {
    let elapsed = started.elapsed();
    if elapsed.as_millis() >= 100 {
        error.message.push_str(&format!(" That took {}.", crate::human::duration(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))));
    }
    error
}

fn write_error_to_agent(error: &std::io::Error) -> AgentError {
    if error.kind() == std::io::ErrorKind::FileTooLarge {
        AgentError::new("output_too_large", "This picture would be larger than RectoPDF will write.", exit::FAILED)
    } else {
        AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED)
    }
}

/// What a whole page holds, from its strips.
fn merge_kinds(so_far: Option<TileKind>, next: TileKind) -> TileKind {
    so_far.map_or(next, |kind| recto_proto::merge_kinds(kind, next))
}

fn kind_name(kind: TileKind) -> &'static str {
    match kind {
        TileKind::Blank { .. } => "blank",
        TileKind::Grey => "grey",
        TileKind::Colour => "colour",
    }
}

/// A renderer for reading a page's text over several pages, under a reader's limits.
pub(crate) fn open_for_text(path: &Path) -> Result<DocRenderer, AgentError> {
    open(path, Policy::default())
}

/// What a page's characters say about hidden text. `text` and `export svg` both go by this, so they can't disagree.
fn hidden_text_facts(characters: &recto_render_service::PageCharacters) -> Value {
    let reading = read_page(&characters.chars, &characters.boxes, characters.hidden_checked);
    let hidden = hidden_report(&characters.chars, characters.hidden_checked);
    json!({ "checked": hidden.checked, "flagged": hidden.flagged || reading.hidden_flagged || reading.normalized.tag_chars_removed > 0, "chars": hidden.chars })
}

pub(crate) fn text(path: &Path, page: u32) -> Result<Value, AgentError> {
    let page = page_index(page)?;
    let mut doc = open(path, Policy::default())?;
    let characters = doc.page_characters(page, true, true)?;
    let reading = read_page(&characters.chars, &characters.boxes, characters.hidden_checked);
    let normalized = &reading.normalized;
    let hidden = hidden_report(&characters.chars, characters.hidden_checked);
    // Document text goes under `untrusted`, so nothing reading this mistakes it for an instruction.
    Ok(json!({
        "page": page + 1,
        "chars": characters.chars.len(),
        "hyphens_joined": normalized.hyphens_joined,
        "hyphens_kept": normalized.hyphens_kept,
        // Characters no eye can see, taken out of the text. Tag characters are never legitimate here, so they raise the same flag as hidden text.
        "invisible_chars_removed": normalized.invisible_removed,
        "tag_chars_removed": normalized.tag_chars_removed,
        "also_spelled_dropped": normalized.also_spelled_dropped,
        // Words the file drew late, put back on their lines. `search` reads the same text.
        "runs_moved": reading.runs_moved,
        "hidden_text": hidden_text_facts(&characters),
        // Where `hidden_runs` lie in the text, in characters. Numbers only, so it isn't untrusted.
        "hidden_spans": reading.hidden_spans.iter().map(|span| [span.0, span.1]).collect::<Vec<_>>(),
        "hidden_spans_dropped": reading.hidden_spans_dropped,
        "untrusted": { "text": normalized.text, "also_spelled": normalized.also_spelled, "hidden_runs": hidden.runs },
    }))
}

/// What `budget render` measures. Field meanings are on the command.
pub(crate) struct BudgetArgs {
    pub pages: u32,
    pub first: Option<u32>,
    pub scale: f32,
    pub fresh: bool,
    pub tiles: bool,
    pub samples: bool,
    pub limit_mib: Option<u32>,
    pub retire_mib: Option<u32>,
}

const MIB: u64 = 1024 * 1024;

/// One tile's worth of margin beyond the window, as the reader will prefetch.
const VIEWPORT_MARGIN_TILES: u32 = 1;

pub(crate) fn budget_viewport(path: &Path, viewport: &str, first: Option<u32>, scale: f32, strategy: Option<&str>) -> Result<Value, AgentError> {
    let bad = || AgentError::new("bad_viewport", "A viewport is WIDTHxHEIGHT in device pixels, such as 3024x1964.", exit::FAILED);
    let (width, height) = viewport.split_once('x').ok_or_else(bad)?;
    let (width, height): (u32, u32) = (width.parse().map_err(|_| bad())?, height.parse().map_err(|_| bad())?);
    if !(1..=32_767).contains(&width) || !(1..=32_767).contains(&height) {
        return Err(bad());
    }
    if !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&scale) {
        return Err(AgentError::new("bad_scale", "That scale can't be drawn.", exit::FAILED));
    }
    let mut doc = open(path, Policy::default())?;
    let first_page = first.map_or(Ok(0), page_index)?;
    let (strategy, strategy_name) = match strategy {
        None => (DrawStrategy::Auto, "auto"),
        Some("per_tile") => (DrawStrategy::PerTile, "per_tile"),
        Some("region") => (DrawStrategy::Region, "region"),
        Some(_) => return Err(AgentError::new("bad_strategy", "The strategy is per_tile or region, or left out to pick by page.", exit::FAILED)),
    };
    let largest_image_pixels = doc.largest_image_pixels(first_page, 0).unwrap_or(0);
    doc.kill_worker();
    let steps = measure_viewport(&mut doc, ViewportOptions { first_page, scale, width, height, margin_tiles: VIEWPORT_MARGIN_TILES, strategy, max_time: BUDGET_MAX_TIME })?;
    let steps: Vec<Value> = steps
        .iter()
        .map(|step| {
            json!({
                "step": step.name,
                "ms": step.micros as f64 / 1000.0,
                "tiles": step.tiles,
                "passes": step.passes,
                "blank": step.blank,
                "grey": step.grey,
                "colour": step.colour,
                "low_res": step.low_res,
                "failed": step.failed,
                "peak_footprint_bytes": step.peak_footprint_bytes,
            })
        })
        .collect();
    Ok(json!({
        "strategy": strategy_name,
        "largest_image_pixels": largest_image_pixels,
        "viewport": viewport,
        "scale": scale,
        "steps": steps,
        "spawns": doc.stats().spawns,
        "retirements": doc.stats().retirements,
        "host_footprint_bytes": recto_platform::footprint_bytes(),
    }))
}

pub(crate) fn budget(path: &Path, args: &BudgetArgs) -> Result<Value, AgentError> {
    if !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&args.scale) {
        return Err(AgentError::new("bad_scale", "That scale can't be drawn.", exit::FAILED));
    }
    let mut policy = Policy::default();
    if let Some(limit) = args.limit_mib {
        policy.memory_limit_bytes = u64::from(limit.clamp(1, MAX_LIMIT_MIB)) * MIB;
        // To measure what exactly this limit does, nothing rescues a page from it.
        policy.heavy_limit_bytes = 0;
    }
    if let Some(retire) = args.retire_mib {
        policy.retire_bytes = u64::from(retire.clamp(1, MAX_LIMIT_MIB)) * MIB;
    }
    let (limit, retire) = (policy.memory_limit_bytes, policy.retire_bytes);
    let mut doc = open(path, policy)?;
    let first_page = args.first.map_or(Ok(0), page_index)?;
    let options = BudgetOptions { first_page, pages: args.pages, scale: args.scale, fresh_worker_per_page: args.fresh, tiles: args.tiles, max_time: BUDGET_MAX_TIME };
    let report = measure_budget(&mut doc, options)?;
    let mut round_trip: Vec<u64> = report.samples.iter().filter(|s| s.failure.is_none()).map(|s| s.micros).collect();
    let mut inside: Vec<u64> = report.samples.iter().filter(|s| s.failure.is_none()).map(|s| s.render_us).collect();
    round_trip.sort_unstable();
    inside.sort_unstable();
    let ms = |sorted: &[u64], share: f64| -> Option<f64> {
        let last = sorted.len().checked_sub(1)?;
        Some(sorted[((last as f64 * share).round() as usize).min(last)] as f64 / 1000.0)
    };
    let mut data = json!({
        "memory_limit_bytes": limit,
        "retire_bytes": retire,
        "scale": args.scale,
        "idle_footprint_bytes": report.idle_footprint_bytes,
        "opened_footprint_bytes": report.opened_footprint_bytes,
        "peak_footprint_bytes": report.peak_footprint_bytes,
        "last_footprint_bytes": report.last_footprint_bytes,
        "host_footprint_bytes": recto_platform::footprint_bytes(),
        "surface_bytes": report.surface_bytes,
        "pages_drawn": report.pages_drawn,
        "pages_failed": report.pages_failed,
        "pages_low_res": report.pages_low_res,
        "heavy_retries": report.heavy_retries,
        "stopped_early": report.stopped_early,
        "spawns": report.spawns,
        "retirements": report.retirements,
        "memory_kills": report.memory_kills,
        "deadline_kills": report.deadline_kills,
        "crashes": report.crashes,
        "tiles": { "blank": report.tiles_blank, "grey": report.tiles_grey, "colour": report.tiles_colour },
        "ms": { "p50": ms(&round_trip, 0.5), "p95": ms(&round_trip, 0.95), "max": round_trip.last().map(|&value| value as f64 / 1000.0) },
        "worker_ms": { "p50": ms(&inside, 0.5), "p95": ms(&inside, 0.95), "max": inside.last().map(|&value| value as f64 / 1000.0) },
    });
    if args.samples {
        let samples: Vec<Value> = report
            .samples
            .iter()
            .map(|s| json!({ "page": s.page + 1, "us": s.micros, "worker_us": s.render_us, "footprint_after_bytes": s.footprint_after_bytes, "failure": s.failure }))
            .collect();
        data["samples"] = Value::Array(samples);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_testpdf::{Page, TempPdf, busy_page, pages, text_lines};
    use std::time::{Duration, Instant};

    fn doc() -> TempPdf {
        TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words"]))]))
    }

    fn folder_entries(file: &TempPdf) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(file.0.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// Failure: an existing file at `out` is silently replaced, though the agent report says export refuses existing paths. Its bytes must not change.
    #[test]
    fn an_existing_output_is_refused_and_left_alone() {
        let file = doc();
        let out = file.0.with_file_name("taken.png");
        std::fs::write(&out, b"precious").unwrap();
        assert_eq!(render(&file.0, 1, 1.0, &out).unwrap_err().code, "output_exists");
        assert_eq!(std::fs::read(&out).unwrap(), b"precious");
        std::fs::remove_file(&out).unwrap();
    }

    /// Failure: a link planted at `out` is written through, so an agent's picture overwrites whatever it points to, or creates a file there.
    #[test]
    fn a_link_at_the_output_is_refused_and_not_followed() {
        let file = doc();
        let out = file.0.with_file_name("link.png");
        let target = file.0.with_file_name("target-of-link");
        std::os::unix::fs::symlink(&target, &out).unwrap();
        assert_eq!(render(&file.0, 1, 1.0, &out).unwrap_err().code, "output_exists");
        assert!(!target.exists(), "the link was followed");
        std::fs::remove_file(&out).unwrap();
    }

    /// Failure: a link planted at the old fixed temp name `x.png.partial` redirected the write, and a failed render deleted a file of that name it hadn't made.
    #[test]
    fn a_file_at_the_old_temp_name_is_neither_followed_nor_deleted() {
        let file = doc();
        let out = file.0.with_file_name("fine.png");
        let target = file.0.with_file_name("victim");
        let link = file.0.with_file_name("fine.png.partial");
        std::fs::write(&target, b"victim").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        render(&file.0, 1, 1.0, &out).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"victim", "the write went through the link");
        // A failing render must leave it too.
        let failed = file.0.with_file_name("fine2.png");
        let plain = file.0.with_file_name("fine2.png.partial");
        std::fs::write(&plain, b"not ours").unwrap();
        assert!(render(&file.0, 99, 1.0, &failed).is_err());
        assert_eq!(std::fs::read(&plain).unwrap(), b"not ours", "a failed render deleted a file it didn't make");
        for path in [&out, &target, &link, &plain] {
            std::fs::remove_file(path).unwrap();
        }
    }

    /// Failure: a volume without hard links, such as exFAT, can't receive a picture at all; or the fallback writes over a file that appeared meanwhile.
    #[test]
    fn publishing_works_without_hard_links_and_still_never_replaces() {
        let file = doc();
        let temp = file.0.with_file_name("from");
        let out = file.0.with_file_name("to.png");
        std::fs::write(&temp, b"picture").unwrap();
        let unsupported = |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        publish(&temp, &out, unsupported).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"picture");
        std::fs::write(&out, b"precious").unwrap();
        assert_eq!(publish(&temp, &out, unsupported).unwrap_err().code, "output_exists");
        assert_eq!(std::fs::read(&out).unwrap(), b"precious");
        std::fs::remove_file(&temp).unwrap();
        std::fs::remove_file(&out).unwrap();
    }

    /// Failure: an output name near the file system's limit fails, because the temp name was built from it and came out too long.
    #[test]
    fn a_long_output_name_still_gets_a_temp_file() {
        let file = doc();
        let out = file.0.with_file_name(format!("{}.png", "n".repeat(250)));
        render(&file.0, 1, 1.0, &out).unwrap();
        std::fs::remove_file(&out).unwrap();
    }

    /// Failure: a failed or refused render leaves its temp file behind, so a folder fills up with partial pictures.
    #[test]
    fn nothing_is_left_after_a_success_or_a_failure() {
        let file = doc();
        let before = folder_entries(&file);
        let out = file.0.with_file_name("a.png");
        render(&file.0, 1, 1.0, &out).unwrap();
        std::fs::remove_file(&out).unwrap();
        let tiny = OutputCaps { max_bytes: 10, ..OutputCaps::default() };
        assert_eq!(render_with_caps(&file.0, 1, 1.0, &out, &tiny).unwrap_err().code, "output_too_large");
        assert_eq!(folder_entries(&file), before);
    }

    /// Failure: a tiny PDF asks for a huge scale and costs minutes and gigabytes, each strip staying under its own limit. Each cap must refuse plainly, and write nothing.
    #[test]
    fn caps_on_pixels_bytes_and_time_refuse_plainly() {
        let file = TempPdf::new("busy.pdf", &pages(&[Page::new(busy_page())]));
        let out = file.0.with_file_name("capped.png");
        let pixels = OutputCaps { max_pixels: 1000, ..OutputCaps::default() };
        assert_eq!(render_with_caps(&file.0, 1, 1.0, &out, &pixels).unwrap_err().code, "too_large_at_this_scale");
        let bytes = OutputCaps { max_bytes: 100, ..OutputCaps::default() };
        assert_eq!(render_with_caps(&file.0, 1, 1.0, &out, &bytes).unwrap_err().code, "output_too_large");
        let time = OutputCaps { max_time: Duration::ZERO, ..OutputCaps::default() };
        assert_eq!(render_with_caps(&file.0, 1, 1.0, &out, &time).unwrap_err().code, "render_took_too_long");
        assert!(!out.exists());
    }

    /// Failure: the default caps let a letter page at scale 16 through, 124 Mpx and over a gigabyte drawn before anything says no.
    #[test]
    fn a_huge_scale_is_refused_before_any_drawing() {
        let file = doc();
        let out = file.0.with_file_name("huge.png");
        let started = Instant::now();
        assert_eq!(render(&file.0, 1, 16.0, &out).unwrap_err().code, "too_large_at_this_scale");
        assert_eq!(render(&file.0, 1, 64.0, &out).unwrap_err().code, "too_large_at_this_scale", "wider than a strip can be");
        assert!(started.elapsed() < Duration::from_secs(30), "it should refuse from the size alone");
        assert!(!out.exists());
    }

    /// Failure: "too large at this zoom" leaves an agent trying 8, 4, 2, 1 by hand. It names the largest scale that fits, and that scale really draws, on a page the size of the USGS map that an agent did this on, under a cap that makes it quick to check.
    #[test]
    fn too_large_names_the_largest_scale_that_draws() {
        let file = TempPdf::new("map.pdf", &pages(&[Page { content: text_lines(&["A map"]), page_extra: String::new(), media: (2232.0, 2016.0) }]));
        let out = file.0.with_file_name("map.png");
        let caps = OutputCaps { max_pixels: 400_000, ..OutputCaps::default() };
        let refused = render_with_caps(&file.0, 1, 8.0, &out, &caps).unwrap_err();
        assert_eq!(refused.code, "too_large_at_this_scale");
        assert!(refused.message.contains("2232 x 2016"), "it says why: {}", refused.message);
        let named: f32 = refused.candidates[0].strip_prefix("--scale ").unwrap().parse().unwrap();
        assert!(refused.message.contains(&named.to_string()) && refused.next.contains(&format!("--scale {named}")), "{} {}", refused.message, refused.next);
        let drawn = render_with_caps(&file.0, 1, named, &out, &caps).unwrap();
        assert!(drawn["width"].as_u64().unwrap() * drawn["height"].as_u64().unwrap() <= caps.max_pixels, "{drawn}");
        std::fs::remove_file(&out).unwrap();
        let next_up = render_with_caps(&file.0, 1, named + 0.02, &out, &caps).unwrap_err();
        assert_eq!(next_up.code, "too_large_at_this_scale", "it's the edge, not somewhere well under it");
        assert!(!out.exists());
        // At the real caps: the map's own size, from the failure an agent met. 8 is refused and what it names is at least the 4 that worked.
        assert!(largest_scale_that_fits(2232.0, 2016.0, &OutputCaps::default()).is_some_and(|most| (4.0..8.0).contains(&most)));
    }

    /// Failure: a refusal tells an agent to try a smaller scale on a page where that can't work, so it tries each one in turn: `form_bomb` fails the same at 0.25. The suggestion needs a picture that's a real share of the limit, and a scale with somewhere to go.
    #[test]
    fn a_smaller_scale_is_suggested_only_when_it_can_help() {
        let refused = RenderFailure::ExportRefused { page: 0, why: ExportRefusal::TooHeavy };
        let gib = 1024 * 1024 * 1024;
        let small = too_heavy_to_export(refused, 0, 2.0, (1224, 1584), gib);
        assert!(small.next.contains("won't help") && !small.next.contains("may fit") && !small.next.contains("recto text"), "and no step that may fail just the same: {}", small.next);
        assert!(small.message.contains("1.00 GiB"), "it should say the limit: {}", small.message);
        assert!(small.message.contains("no file was written"), "{}", small.message);
        let large = too_heavy_to_export(refused, 0, 8.0, (8000, 8000), gib);
        assert!(large.next.contains("smaller `--scale` may fit"), "{}", large.next);
        // A 7200 x 7200 pt page at 1x really is too large a picture, and at half the scale it's a quarter of that. Scale 1 isn't the floor. Only the smallest scale is.
        let huge_page = too_heavy_to_export(refused, 0, 1.0, (7200, 7200), gib);
        assert!(huge_page.next.contains("may fit"), "{}", huge_page.next);
        let at_the_floor = too_heavy_to_export(refused, 0, recto_proto::MIN_SCALE, (8000, 8000), gib);
        assert!(!at_the_floor.next.contains("may fit"), "there's nowhere to go: {}", at_the_floor.next);
        assert_eq!(small.code, "export_page_too_heavy");
    }

    /// Failure: a drawing at 4.71x says it was at 4.71000003814697x, so a number an agent chose comes back looking like a different one.
    #[test]
    fn a_scale_reads_as_it_was_written() {
        let file = doc();
        let out = file.0.with_file_name("written.png");
        let drawn = render(&file.0, 1, 0.71, &out).unwrap();
        assert_eq!(drawn["scale"], 0.71, "{drawn}");
        assert!(crate::human::render("render", &drawn).contains("at 0.71x"), "{drawn}");
        std::fs::remove_file(&out).unwrap();
    }

    /// Failure: `--scale max` draws at something other than the largest that fits, or doesn't say which, and so leaves an agent where it was: trying numbers. It must be the same edge the refusal names, and be drawn.
    #[test]
    fn the_largest_scale_is_drawn_and_named() {
        let file = TempPdf::new("map.pdf", &pages(&[Page { content: text_lines(&["A map"]), page_extra: String::new(), media: (2232.0, 2016.0) }]));
        let out = file.0.with_file_name("max.png");
        let caps = OutputCaps { max_pixels: 400_000, ..OutputCaps::default() };
        let drawn = render_with_caps(&file.0, 1, LARGEST_THAT_FITS, &out, &caps).unwrap();
        let named = drawn["scale"].as_f64().unwrap() as f32;
        assert_eq!(drawn["largest_that_fits"], true, "{drawn}");
        assert_eq!(Some(named), largest_scale_that_fits(2232.0, 2016.0, &caps), "{drawn}");
        assert!(drawn["width"].as_u64().unwrap() * drawn["height"].as_u64().unwrap() <= caps.max_pixels, "{drawn}");
        let said = crate::human::render("render", &drawn);
        assert!(said.contains(&format!("at {named}x")) && said.contains("largest"), "{said}");
        std::fs::remove_file(&out).unwrap();
        let at_a_number = render_with_caps(&file.0, 1, 0.1, &out, &caps).unwrap();
        assert_eq!(at_a_number["largest_that_fits"], false);
        assert!(!crate::human::render("render", &at_a_number).contains("largest"));
        std::fs::remove_file(&out).unwrap();
    }

    /// Failure: `--scale max` on a page its memory limit stops comes out at a lower resolution, or leaves a file, or says nothing plain. Owner, 2026-11-07: an export is sharp or isn't written, and asking for the largest doesn't change that.
    #[test]
    fn the_largest_scale_still_fails_plainly_at_the_memory_limit() {
        let file = doc();
        let before = folder_entries(&file);
        let out = file.0.with_file_name("heavy.png");
        let error = render_with(stand_in(), &file.0, 1, LARGEST_THAT_FITS, &out, &OutputCaps::default()).unwrap_err();
        assert!(matches!(error.code, "export_page_too_heavy" | "export_memory_tight"), "{}", error.code);
        assert!(error.message.contains("no file was written"), "{}", error.message);
        assert_eq!(folder_entries(&file), before);
    }

    /// Failure: nobody can tell a slow draw from a fast one, so a slow page or a bad scale goes unnoticed.
    #[test]
    fn a_drawing_says_how_long_it_took() {
        let file = doc();
        let out = file.0.with_file_name("timed.png");
        let drawn = render(&file.0, 1, 1.0, &out).unwrap();
        assert!(drawn["took_ms"].is_u64(), "{drawn}");
        assert!(crate::human::render("render", &drawn).contains(" in "), "{drawn}");
        std::fs::remove_file(&out).unwrap();
    }

    /// Not a test: a stand-in worker in which page 1 fits only at a lower resolution. Does nothing unless started as one.
    #[test]
    fn fake_worker() {
        use recto_platform::isolate::{self, Channel};
        use recto_proto::{self as proto, Kind, PageSize, RenderJob, TileReply, TileStatus};
        use recto_render_service::transport::{FrameReader, send_all};
        if std::env::var("RECTO_FAKE").is_err() {
            return;
        }
        let channel = Channel::inherited().unwrap();
        send_all(&channel, &proto::encode_hello(std::process::id()), &[]).unwrap();
        let mut reader = FrameReader::default();
        let mut kept = Vec::new();
        loop {
            let Ok(frame) = reader.read_blocking(&channel) else { isolate::exit_now(0) };
            let reply = match frame.kind {
                Kind::Open => proto::encode_opened(1, 1),
                Kind::SizeRequest => {
                    let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                    PageSize { job_id, width: 612.0, height: 792.0 }.encode()
                }
                Kind::TraitsRequest => {
                    let (job_id, _) = proto::decode_size_request(&frame.payload).unwrap();
                    proto::PageTraits { job_id, largest_image_pixels: 0 }.encode()
                }
                _ => {
                    let job = RenderJob::decode(&frame.payload).unwrap();
                    if job.scale >= 1.0 {
                        loop {
                            kept.push(vec![1u8; 8 * 1024 * 1024]);
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    let status = TileStatus::Done(TileKind::Colour);
                    TileReply { generation: job.generation, job_id: job.job_id, width: job.rect.width, height: job.rect.height, stride: job.stride, status, render_us: 1 }.encode()
                }
            };
            send_all(&channel, &reply, &[]).unwrap();
        }
    }

    fn stand_in() -> ServiceConfig {
        ServiceConfig {
            worker_exe: std::env::current_exe().unwrap(),
            pdfium_lib: "/nonexistent".into(),
            heavy_slot: recto_render_service::HeavySlot::new(),
            extra_env: vec![("RECTO_FAKE".into(), "1".into())],
            replace_args: Some(["--exact", "render::tests::fake_worker", "--nocapture", "--test-threads=1"].into_iter().map(Into::into).collect()),
        }
    }

    /// Failure: a page past every limit comes out of `recto render` as a smaller picture, and someone sends it on, believing it's what they asked for. Owner, 2026-11-07: an export is sharp or
    /// isn't written. The stand-in fits only at the lower scale, so a low-res path would succeed: it must refuse, say so, and leave no file behind.
    #[test]
    fn an_export_never_produces_a_low_res_file() {
        let file = doc();
        let before = folder_entries(&file);
        let out = file.0.with_file_name("low.png");
        let error = render_with(stand_in(), &file.0, 1, 2.0, &out, &OutputCaps::default()).unwrap_err();
        // Which refusal depends on how much memory this Mac has free. None of them is a picture.
        assert!(matches!(error.code, "export_page_too_heavy" | "export_memory_tight"), "{}", error.code);
        assert!(error.message.contains("no file was written"), "{}", error.message);
        assert_eq!(folder_entries(&file), before, "something was left behind");
    }

    /// Failure: a page with a red blank strip and a white one reports `grey`, telling an agent there's no colour on a page that has some.
    #[test]
    fn whole_page_kinds_tell_the_truth() {
        let (white, black, red) = (TileKind::Blank { rgb: 0xFF_FFFF }, TileKind::Blank { rgb: 0 }, TileKind::Blank { rgb: 0xFF_0000 });
        assert_eq!(merge_kinds(None, white), white);
        assert_eq!(merge_kinds(Some(white), white), white);
        assert_eq!(merge_kinds(Some(white), black), TileKind::Grey, "two blanks of different greys");
        assert_eq!(merge_kinds(Some(white), red), TileKind::Colour);
        assert_eq!(merge_kinds(Some(red), TileKind::Grey), TileKind::Colour);
        assert_eq!(merge_kinds(Some(TileKind::Grey), white), TileKind::Grey);
        assert_eq!(merge_kinds(Some(TileKind::Colour), white), TileKind::Colour);
    }
}
