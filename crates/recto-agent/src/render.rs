//! The renderer's commands. A transient worker per command for now, because the agent holds no state: it starts, answers, and goes away
//! with the call. The session's scheduler will own long-lived ones, behind the same calls.

use crate::{AgentError, exit};
use recto_index::{hidden_report, normalize};
use recto_proto::{BYTES_PER_PIXEL, TileKind};
use recto_render_service::{BUDGET_MAX_TIME, BudgetOptions, DocRenderer, OutputCaps, Policy, RenderFailure, ServiceConfig, for_each_strip, measure_budget, page_device_size};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

mod png;

pub const DEFAULT_SCALE: f32 = 2.0;

pub(crate) fn default_scale() -> f32 {
    DEFAULT_SCALE
}

/// How high `budget render` may raise a worker's limit. A measurement shouldn't be a way to take the whole Mac.
const MAX_LIMIT_MIB: u32 = 4096;

impl From<RenderFailure> for AgentError {
    fn from(failure: RenderFailure) -> Self {
        let exit = match failure {
            RenderFailure::NotFound | RenderFailure::BadPage => exit::NOT_FOUND,
            RenderFailure::NeedsPassword => exit::PROTECTED,
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
        let name = out.file_name().ok_or_else(failed)?.to_string_lossy().into_owned();
        for _ in 0..64 {
            let attempt = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = out.with_file_name(format!(".{name}.{}.{attempt}.partial", std::process::id()));
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

fn output_exists() -> AgentError {
    AgentError::new("output_exists", "A file is already there, and it won't be replaced.", exit::FAILED)
}

fn render_with_caps(path: &Path, page: u32, scale: f32, out: &Path, caps: &OutputCaps) -> Result<Value, AgentError> {
    let page = page_index(page)?;
    // Only ever a new picture. This also keeps `out` off the PDF itself, because only the append writer may change one.
    if out.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("png")) {
        return Err(AgentError::new("bad_output", "The output file must end in .png.", exit::FAILED));
    }
    if !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&scale) {
        return Err(AgentError::new("bad_scale", format!("Scale must be between {} and {}.", recto_proto::MIN_SCALE, recto_proto::MAX_SCALE), exit::FAILED));
    }
    // `symlink_metadata`, so a dangling link counts as being there. Early, to save the drawing. The link below is what actually holds.
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    let mut doc = open(path, Policy::default())?;
    let size = page_device_size(&mut doc, page, scale, caps)?;
    let (temp, file) = OwnTemp::create(out)?;
    let kind = write_page(&mut doc, page, scale, size, file, caps)?;
    // In one step and never over anything: a stopped renderer leaves no half a picture under the real name, and a file that appeared meanwhile stays as it was.
    match std::fs::hard_link(&temp.path, out) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Err(output_exists()),
        Err(_) => return Err(AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED)),
    }
    drop(temp);
    Ok(json!({
        "page": page + 1,
        "scale": scale,
        "out": out,
        "width": size.0,
        "height": size.1,
        "bytes": std::fs::metadata(out).map(|meta| meta.len()).ok(),
        "kind": kind,
    }))
}

fn write_page(doc: &mut DocRenderer, page: u32, scale: f32, (width, height): (u32, u32), file: File, caps: &OutputCaps) -> Result<&'static str, AgentError> {
    let write_failed = || AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED);
    let mut writer = png::RgbWriter::create(file, width, height, caps.max_bytes).map_err(|_| write_failed())?;
    let mut surface = None;
    let mut kind = None;
    let mut row = vec![0u8; width as usize * BYTES_PER_PIXEL as usize];
    let mut write_error = None;
    let deadline = Instant::now() + caps.max_time;
    for_each_strip(doc, page, scale, (width, height), deadline, &mut surface, |memory, rect, stride, strip_kind| {
        kind = Some(merge_kinds(kind, strip_kind));
        for y in 0..rect.height as usize {
            // A copy out, never a look in place: the worker still holds this memory, and it isn't trusted.
            if !memory.copy_out(y * stride as usize, &mut row) {
                return Err(RenderFailure::Unavailable);
            }
            if let Err(error) = writer.write_bgrx_row(&row) {
                write_error = Some(error);
                return Err(RenderFailure::Unavailable);
            }
        }
        Ok(())
    })
    .map_err(|failure| match &write_error {
        Some(error) => write_error_to_agent(error),
        None => failure.into(),
    })?;
    writer.finish().map_err(|error| write_error_to_agent(&error))?;
    Ok(kind.map_or("blank", kind_name))
}

fn write_error_to_agent(error: &std::io::Error) -> AgentError {
    if error.kind() == std::io::ErrorKind::FileTooLarge {
        AgentError::new("output_too_large", "This picture would be larger than RectoPDF will write.", exit::FAILED)
    } else {
        AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED)
    }
}

/// What a whole page holds, from its strips. Two different blanks aren't one blank, and a coloured blank strip beside a grey one is a page with colour.
fn merge_kinds(so_far: Option<TileKind>, next: TileKind) -> TileKind {
    let has_colour = |kind: TileKind| match kind {
        TileKind::Colour => true,
        TileKind::Grey => false,
        TileKind::Blank { rgb } => (rgb >> 16) & 0xFF != (rgb >> 8) & 0xFF || (rgb >> 8) & 0xFF != rgb & 0xFF,
    };
    match so_far {
        None => next,
        Some(kind) if kind == next => kind,
        Some(kind) if has_colour(kind) || has_colour(next) => TileKind::Colour,
        Some(_) => TileKind::Grey,
    }
}

fn kind_name(kind: TileKind) -> &'static str {
    match kind {
        TileKind::Blank { .. } => "blank",
        TileKind::Grey => "grey",
        TileKind::Colour => "colour",
    }
}

pub(crate) fn text(path: &Path, page: u32) -> Result<Value, AgentError> {
    let page = page_index(page)?;
    let mut doc = open(path, Policy::default())?;
    let characters = doc.page_characters(page, false, true)?;
    let normalized = normalize(&characters.chars, false);
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
        "hidden_text": { "checked": hidden.checked, "flagged": hidden.flagged || normalized.tag_chars_removed > 0, "chars": hidden.chars },
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

pub(crate) fn budget(path: &Path, args: &BudgetArgs) -> Result<Value, AgentError> {
    if !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&args.scale) {
        return Err(AgentError::new("bad_scale", "That scale can't be drawn.", exit::FAILED));
    }
    let mut policy = Policy::default();
    if let Some(limit) = args.limit_mib {
        policy.memory_limit_bytes = u64::from(limit.clamp(1, MAX_LIMIT_MIB)) * MIB;
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
    use std::time::Duration;

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
