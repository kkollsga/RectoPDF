//! The renderer's commands. A transient worker per command for now, because the agent holds no state: it starts, answers, and goes away
//! with the call. The session's scheduler will own long-lived ones, behind the same calls.

use crate::{AgentError, exit};
use recto_index::{hidden_report, normalize};
use recto_proto::{BYTES_PER_PIXEL, TileKind};
use recto_render_service::{
    BUDGET_MAX_TIME, BudgetOptions, DocRenderer, DrawStrategy, OutputCaps, PageDrawn, PageEvent, Policy, RenderFailure, ServiceConfig, ViewportOptions, draw_whole_page, measure_budget, measure_viewport,
};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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
    render_with(ServiceConfig::discover()?, path, page, scale, out, caps)
}

fn render_with(config: ServiceConfig, path: &Path, page: u32, scale: f32, out: &Path, caps: &OutputCaps) -> Result<Value, AgentError> {
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
    let mut doc = DocRenderer::new(path, None, config, Policy::default())?;
    let (temp, drawn, kind) = write_page(&mut doc, page, scale, out, caps)?;
    // In one step and never over anything: a stopped renderer leaves no half a picture under the real name, and a file that appeared meanwhile stays as it was.
    match std::fs::hard_link(&temp.path, out) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Err(output_exists()),
        Err(_) => return Err(AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED)),
    }
    drop(temp);
    Ok(json!({
        "page": page + 1,
        "asked_scale": scale,
        "scale": drawn.scale,
        "out": out,
        "width": drawn.width,
        "height": drawn.height,
        "bytes": std::fs::metadata(out).map(|meta| meta.len()).ok(),
        "kind": kind,
        "low_res": drawn.low_res.map(|why| json!({ "why": why.code(), "message": why.message(), "may_clear": why.may_clear() })),
    }))
}

/// Writes the page to a new temp file, starting a new one if the page comes down to a lower resolution. Returns the file, ready to publish.
fn write_page(doc: &mut DocRenderer, page: u32, scale: f32, out: &Path, caps: &OutputCaps) -> Result<(OwnTemp, PageDrawn, &'static str), AgentError> {
    let write_failed = || AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED);
    let mut surface = None;
    let mut kind = None;
    let mut picture: Option<(OwnTemp, png::RgbWriter, Vec<u8>)> = None;
    let mut sink_error = None;
    let drawn = draw_whole_page(doc, page, scale, caps, &mut surface, |event| match event {
        PageEvent::Begin { width, height, .. } => {
            // Dropping any earlier picture deletes its temp file.
            picture = None;
            kind = None;
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
    .map_err(|failure| sink_error.take().unwrap_or_else(|| failure.into()))?;
    let (temp, writer, _) = picture.ok_or_else(write_failed)?;
    writer.finish().map_err(|error| write_error_to_agent(&error))?;
    Ok((temp, drawn, kind.map_or("blank", kind_name)))
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
    let largest_image_pixels = doc.largest_image_pixels(first_page).unwrap_or(0);
    doc.kill_worker();
    let steps = measure_viewport(&mut doc, ViewportOptions { first_page, scale, width, height, margin_tiles: VIEWPORT_MARGIN_TILES, strategy })?;
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
                Kind::Open => proto::encode_opened(1),
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

    /// Failure: a page past every limit comes out of `recto render` as a smaller picture with nothing saying so, and an agent takes it for what it asked for. And since only
    /// the picture that finished may remain, the abandoned one must leave no file behind.
    #[test]
    fn a_lower_resolution_picture_says_so() {
        let file = doc();
        let before = folder_entries(&file);
        let out = file.0.with_file_name("low.png");
        let config = ServiceConfig {
            worker_exe: std::env::current_exe().unwrap(),
            pdfium_lib: "/nonexistent".into(),
            heavy_slot: recto_render_service::HeavySlot::new(),
            extra_env: vec![("RECTO_FAKE".into(), "1".into())],
            replace_args: Some(["--exact", "render::tests::fake_worker", "--nocapture", "--test-threads=1"].into_iter().map(Into::into).collect()),
        };
        let reply = render_with(config, &file.0, 1, 2.0, &out, &OutputCaps::default()).unwrap();
        assert_eq!(reply["asked_scale"], 2.0);
        assert_eq!(reply["scale"], 0.5);
        assert_eq!((&reply["width"], &reply["height"]), (&json!(306), &json!(396)));
        assert!(reply["low_res"]["message"].as_str().unwrap().contains("lower-resolution"), "{reply}");
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 306, "the file must be the size the reply says");
        std::fs::remove_file(&out).unwrap();
        assert_eq!(folder_entries(&file), before, "the abandoned picture was left behind");
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
