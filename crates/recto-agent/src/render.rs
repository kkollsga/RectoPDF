//! The renderer's commands. A transient worker per command for now, because the agent holds no state: it starts, answers, and goes away
//! with the call. The session's scheduler will own long-lived ones, behind the same calls.

use crate::{AgentError, exit};
use recto_index::{hidden_report, normalize};
use recto_proto::{BYTES_PER_PIXEL, TileKind};
use recto_render_service::{BudgetOptions, DocRenderer, Policy, RenderFailure, ServiceConfig, for_each_strip, measure_budget};
use serde_json::{Value, json};
use std::path::Path;

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
    let page = page_index(page)?;
    // Only ever a new picture. This also keeps `out` off the PDF itself, because only the append writer may change one.
    if out.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("png")) {
        return Err(AgentError::new("bad_output", "The output file must end in .png.", exit::FAILED));
    }
    if !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&scale) {
        return Err(AgentError::new("bad_scale", format!("Scale must be between {} and {}.", recto_proto::MIN_SCALE, recto_proto::MAX_SCALE), exit::FAILED));
    }
    let mut doc = open(path, Policy::default())?;
    let partial = out.with_extension("png.partial");
    let written = write_page(&mut doc, page, scale, &partial);
    let (width, height, kind) = match written {
        Ok(done) => done,
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            return Err(error);
        }
    };
    // In one step, so a stopped renderer never leaves half a picture under the real name.
    if std::fs::rename(&partial, out).is_err() {
        let _ = std::fs::remove_file(&partial);
        return Err(AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED));
    }
    Ok(json!({
        "page": page + 1,
        "scale": scale,
        "out": out,
        "width": width,
        "height": height,
        "bytes": std::fs::metadata(out).map(|meta| meta.len()).ok(),
        "kind": kind,
    }))
}

fn write_page(doc: &mut DocRenderer, page: u32, scale: f32, partial: &Path) -> Result<(u32, u32, &'static str), AgentError> {
    let write_failed = || AgentError::new("write_failed", "The picture couldn't be saved there.", exit::FAILED);
    let (width_points, height_points) = doc.page_size(page)?;
    let (width, height) = recto_proto::device_size(width_points, height_points, scale).map_err(|_| RenderFailure::TooLargeAtThisScale)?;
    let mut writer = png::RgbWriter::create(partial, width, height).map_err(|_| write_failed())?;
    let mut surface = None;
    let mut kind = None;
    let mut row = vec![0u8; width as usize * BYTES_PER_PIXEL as usize];
    let mut write_error = false;
    for_each_strip(doc, page, scale, &mut surface, |memory, rect, stride, strip_kind| {
        kind = Some(merge_kinds(kind, strip_kind));
        for y in 0..rect.height as usize {
            // A copy out, never a look in place: the worker still holds this memory, and it isn't trusted.
            if !memory.copy_out(y * stride as usize, &mut row) {
                return Err(RenderFailure::Unavailable);
            }
            if writer.write_bgrx_row(&row).is_err() {
                write_error = true;
                return Err(RenderFailure::Unavailable);
            }
        }
        Ok(())
    })
    .map_err(|failure| if write_error { write_failed() } else { failure.into() })?;
    writer.finish().map_err(|_| write_failed())?;
    Ok((width, height, kind.map_or("blank", kind_name)))
}

fn merge_kinds(so_far: Option<TileKind>, next: TileKind) -> TileKind {
    match (so_far, next) {
        (None, kind) => kind,
        (Some(a), b) if a == b => a,
        (Some(TileKind::Colour), _) | (_, TileKind::Colour) => TileKind::Colour,
        _ => TileKind::Grey,
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
        "hidden_text": { "checked": hidden.checked, "flagged": hidden.flagged, "chars": hidden.chars },
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
    let options = BudgetOptions { first_page, pages: args.pages, scale: args.scale, fresh_worker_per_page: args.fresh, tiles: args.tiles };
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
        "pages_drawn": report.pages_drawn,
        "pages_failed": report.pages_failed,
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
