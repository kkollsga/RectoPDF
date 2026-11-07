//! `export svg`: a page, or a box on it, as a new SVG file, written as it's read. Nothing is held: records come from the worker a frame at a time, and go straight through `recto-svg` to a temp file that's only published if the whole page got there. So a worker that's stopped, or a limit that's hit, leaves nothing behind under the real name.

use super::{OwnTemp, largest_scale_that_fits, output_exists, page_index, publish, too_heavy_to_export, with_time_taken};
use crate::{AgentError, exit};
use recto_proto::contents::{ImageCoding, NoteCode, PaintStyle, Record, SEG_CURVE, SEG_LINE, TextMode, WireSeg, Why, hints};
use recto_render_service::{ContentsEvent, ContentsRequest, DocRenderer, ExportRefusal, OutputCaps, Policy, RenderFailure, ServiceConfig};
use recto_svg::{GenericFamily, ImageData, Matrix, Paint, PathStyle, PngChannels, Rgb, Seg, SegKind, Stroke, SvgWriter, TextStyle};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Seek, Write};
use std::path::Path;
use std::time::Instant;

/// What was asked for, from the command.
pub(crate) struct SvgArgs<'a> {
    pub path: &'a Path,
    /// One-based.
    pub page: u32,
    pub out: &'a Path,
    pub rect: Option<[f32; 4]>,
    pub text: Option<&'a str>,
    pub scale: f32,
}

/// Why each kind of picture is one, in words, and a stable code. PDFium has no call that reads these, so it's said rather than guessed.
fn why_words(why: Why) -> (&'static str, &'static str) {
    match why {
        Why::Image => ("image", "an image in the file"),
        Why::Shading => ("shading", "a gradient: the renderer has no way to read one, so it's a picture of just that element"),
        Why::TilingPattern => ("tiling_pattern", "a pattern fill, drawn as a picture of just that element"),
        Why::ShadingPattern => ("shading_pattern", "a gradient fill, drawn as a picture of just that element"),
        Why::BlendOrMask => ("blend_or_mask", "a blend mode or soft mask: drawn with what's under it where a blend mode may matter"),
        Why::GroupEffects => ("group_effects", "a group on a page that uses blend modes or soft masks, which can't be told apart from a plain one, drawn as a picture"),
        Why::TextOutline => ("text_no_outline", "text whose outlines can't be had, such as a Type 3 font, a font whose letters can't be matched to its shapes, or stroked text, drawn as a picture, with its real characters over it where it's selectable"),
        Why::TextClip => ("text_clip", "something clipped by text, which can't be read, drawn as a picture that obeys it"),
        Why::TooComplex => ("too_complex", "too many points, clips or nested forms to write directly, drawn as a picture"),
        Why::Annotations => ("annotations", "annotations and form fields, drawn as a picture"),
        Why::Flattened => ("page", "so much of the page had to be pictures that the rest went as one picture of the whole page, with its text kept selectable"),
        Why::MaskedImage => ("masked_image", "an image too large to take apart with its mask, drawn as a picture at this scale"),
        Why::ImageUnreadable => ("image_unreadable", "an image that can be drawn but not handed over, drawn as a picture at this scale"),
    }
}

fn note_words(code: NoteCode) -> (&'static str, &'static str) {
    match code {
        NoteCode::NotFinite => ("not_finite", "objects with numbers that make no sense, left out"),
        NoteCode::HiddenLayer => ("hidden_layer", "objects in layers that are off, left out"),
        NoteCode::InvisibleText => ("invisible_text_chars", "characters of text that draw nothing, kept as selectable text"),
        NoteCode::Unknown => ("unknown_object", "objects the renderer says nothing about, left out"),
        NoteCode::LayersUnchecked => ("layers_unchecked", "objects not checked for whether their layer is on, past a limit, kept"),
        NoteCode::ObjectsSkipped => ("objects_past_limit", "objects past the limit on one page, in the picture of the whole page"),
    }
}

/// Holds output to its limit, so a hostile page can't fill the disk.
struct Capped<'a> {
    file: &'a File,
    left: u64,
    written: u64,
}

impl Write for Capped<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.left = self.left.checked_sub(data.len() as u64).ok_or(io::ErrorKind::FileTooLarge)?;
        (&*self.file).write_all(data)?;
        self.written += data.len() as u64;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        (&*self.file).flush()
    }
}

/// What came through, for the answer.
#[derive(Default)]
struct Tally {
    paths: u64,
    clips: u64,
    glyphs: u64,
    glyph_runs: u64,
    text_runs: u64,
    text_chars: u64,
    images_png: u64,
    images_jpeg: u64,
    rasterised: BTreeMap<u8, u64>,
    notes: BTreeMap<u8, (NoteCode, u64)>,
    chars_dropped: u64,
    size: (f32, f32),
}

fn mat(m: &[f32; 6]) -> Matrix {
    Matrix::new(f64::from(m[0]), f64::from(m[1]), f64::from(m[2]), f64::from(m[3]), f64::from(m[4]), f64::from(m[5]))
}

fn seg(wire: WireSeg) -> Seg {
    let kind = match wire.kind {
        SEG_LINE => SegKind::Line,
        SEG_CURVE => SegKind::Curve,
        _ => SegKind::Move,
    };
    Seg { kind, close: wire.close, x: wire.x, y: wire.y }
}

fn paint(color: Option<[u8; 4]>) -> Paint {
    color.map_or(Paint::None, |[r, g, b, a]| Paint::Solid(Rgb(r, g, b), a))
}

fn path_style(style: &PaintStyle<'_>) -> PathStyle {
    PathStyle {
        fill: paint(style.fill),
        even_odd: style.even_odd,
        stroke: style.stroke.map(|color| Stroke { paint: paint(Some(color)), width: f64::from(style.width), cap: style.cap, join: style.join, dashes: style.dashes().collect(), dash_phase: f64::from(style.dash_phase) }),
    }
}

/// One record, written. The stream's rules have been checked by the time it gets here.
fn write_record<'file>(svg: &mut Option<SvgWriter<Capped<'file>>>, file: &'file File, limit: u64, title: Option<&str>, tally: &mut Tally, record: &Record<'_>) -> io::Result<()> {
    if let Record::Page { width, height, matrix } = record {
        tally.size = (*width, *height);
        *svg = Some(SvgWriter::begin(Capped { file, left: limit, written: 0 }, f64::from(*width), f64::from(*height), &mat(matrix), title)?);
        return Ok(());
    }
    let Some(writer) = svg.as_mut() else { return Err(io::ErrorKind::InvalidInput.into()) };
    match record {
        Record::Page { .. } => {}
        Record::Path { matrix, style, segs } => {
            tally.paths += 1;
            writer.path(&path_style(style), &mat(matrix), segs.iter().map(seg))?;
        }
        Record::ClipDefine { id, parent, matrix, segs } => {
            tally.clips += 1;
            writer.clip_define(*id, *parent, &mat(matrix), segs.iter().map(seg))?;
        }
        Record::Clip { id } => writer.set_clip(*id)?,
        Record::GroupBegin { opacity } => writer.group_begin(*opacity)?,
        Record::GroupEnd => writer.group_end()?,
        Record::GlyphDefine { id, segs } => {
            tally.glyphs += 1;
            writer.glyph(*id, segs.iter().map(seg))?;
        }
        Record::GlyphRun { matrix, fill, stroke, stroke_width, items } => {
            tally.glyph_runs += 1;
            writer.glyph_run(&mat(matrix), paint(*fill), stroke.map(|color| (paint(Some(color)), f64::from(*stroke_width))), items.iter())?;
        }
        Record::TextRun { matrix, fill, bold, italic, generic, family, items } => {
            tally.text_runs += 1;
            tally.text_chars += items.len() as u64;
            let style = TextStyle {
                fill: fill.map(|[r, g, b, a]| (Rgb(r, g, b), a)),
                family,
                generic: match generic {
                    0 => GenericFamily::Serif,
                    2 => GenericFamily::Monospace,
                    _ => GenericFamily::SansSerif,
                },
                bold: *bold,
                italic: *italic,
            };
            writer.text_run(&mat(matrix), &style, items.iter().map(|(code, x, y)| (char::from_u32(code).unwrap_or('\u{FFFD}'), x, y)))?;
        }
        Record::ImageBegin { coding, why, opacity, width, height, matrix, .. } => {
            let data = match coding {
                ImageCoding::Jpeg => ImageData::Jpeg,
                ImageCoding::Pixels { channels } => ImageData::Pixels {
                    width: *width,
                    height: *height,
                    channels: match channels {
                        1 => PngChannels::Grey,
                        3 => PngChannels::Rgb,
                        _ => PngChannels::Rgba,
                    },
                },
            };
            match (why, coding) {
                (Why::Image, ImageCoding::Jpeg) => tally.images_jpeg += 1,
                (Why::Image, _) => tally.images_png += 1,
                _ => *tally.rasterised.entry(*why as u8).or_default() += 1,
            }
            writer.image_begin(data, &mat(matrix), *opacity, *why != Why::Image)?;
        }
        Record::ImageBytes(bytes) => writer.image_bytes(bytes)?,
        Record::ImageEnd => writer.image_end()?,
        Record::Note { code, count } => tally.notes.entry(*code as u8).or_insert((*code, 0)).1 += u64::from(*count),
    }
    Ok(())
}

fn text_mode(word: Option<&str>) -> Result<TextMode, AgentError> {
    match word {
        None | Some("both") => Ok(TextMode::Both),
        Some("paths") => Ok(TextMode::Paths),
        Some("selectable") => Ok(TextMode::Selectable),
        Some(_) => Err(AgentError::new("bad_text_mode", "How text comes out is `both`, `paths` or `selectable`.", exit::FAILED)),
    }
}

fn hints_for(path: &Path, page: u32) -> u8 {
    let Some(effects) = recto_core::page_effects(path, page) else { return hints::ALL };
    (u8::from(effects.soft_mask) * hints::SOFT_MASK) | (u8::from(effects.blend) * hints::BLEND) | (u8::from(effects.shading_pattern) * hints::SHADING_PATTERN) | (u8::from(effects.hidden_layers) * hints::HIDDEN_LAYERS)
}

const WRITE_FAILED: &str = "The SVG couldn't be saved there.";

fn write_failed() -> AgentError {
    AgentError::new("write_failed", WRITE_FAILED, exit::FAILED)
}

fn from_io(error: &io::Error) -> AgentError {
    if error.kind() == io::ErrorKind::FileTooLarge { AgentError::new("output_too_large", "This SVG would be larger than RectoPDF will write.", exit::FAILED) } else { write_failed() }
}

pub(crate) fn export_svg(args: &SvgArgs<'_>) -> Result<Value, AgentError> {
    export_with(ServiceConfig::discover()?, args, &OutputCaps::default())
}

pub(super) fn export_with(config: ServiceConfig, args: &SvgArgs<'_>, caps: &OutputCaps) -> Result<Value, AgentError> {
    let page = page_index(args.page)?;
    // Only ever a new file. This also keeps `out` off the PDF itself, because only the append writer may change one.
    if args.out.extension().is_none_or(|extension| !extension.eq_ignore_ascii_case("svg")) {
        return Err(AgentError::new("bad_output", "The output file must end in .svg.", exit::FAILED).then("Choose a file name ending in .svg."));
    }
    if !(recto_proto::MIN_SCALE..=recto_proto::MAX_SCALE).contains(&args.scale) {
        return Err(AgentError::new("bad_scale", format!("Scale must be between {} and {}.", recto_proto::MIN_SCALE, recto_proto::MAX_SCALE), exit::FAILED));
    }
    let text = text_mode(args.text)?;
    if let Some([x, y, width, height]) = args.rect
        && (!x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() || x < 0.0 || y < 0.0 || width <= 0.0 || height <= 0.0)
    {
        return Err(bad_rect());
    }
    // `symlink_metadata`, so a dangling link counts as being there. Early, to save the reading. The link below is what actually holds.
    if std::fs::symlink_metadata(args.out).is_ok() {
        return Err(output_exists());
    }
    let started = Instant::now();
    let policy = Policy::default();
    let limit = policy.heavy_limit_bytes;
    let mut doc = DocRenderer::new(args.path, None, config, policy)?;
    let (width_points, height_points) = doc.page_size(page).map_err(|failure| with_time_taken(failure.into(), started))?;
    if let Some([x, y, _, _]) = args.rect
        && (x >= width_points || y >= height_points)
    {
        return Err(bad_rect().then("The page is smaller than that. A box starts on the page, and is cut off at its edge."));
    }
    // A picture standing in for part of the page is drawn at this scale. What's vector has no scale, so a page too large for this one only changes what its pictures would be, and it's said.
    let most = largest_scale_that_fits(width_points, height_points, caps).ok_or_else(|| AgentError::new("too_large_at_this_scale", format!("Page {} is {width_points} x {height_points} points, and no scale fits: a picture is at most {} Mpx.", page + 1, caps.max_pixels / 1_000_000), exit::FAILED).then("Tell the user this page can't be exported."))?;
    let raster_scale = args.scale.min(most);
    let request = ContentsRequest { page, generation: 0, scale: raster_scale, rect: args.rect, text, hints: hints_for(args.path, page), max_time: caps.max_time };
    let title = recto_core::document_title(args.path);
    let (temp, file) = OwnTemp::create(args.out).map_err(|_| write_failed())?;
    let mut tally = Tally::default();
    let mut sink_error = None;
    let mut svg: Option<SvgWriter<Capped<'_>>> = None;
    let read = doc.read_contents(&request, |event| {
        let written = match event {
            ContentsEvent::Restart => {
                // A worker was stopped and a larger one is starting over, so all of this goes.
                svg = None;
                tally = Tally::default();
                file.set_len(0).and_then(|()| (&file).rewind().map(|_| ()))
            }
            ContentsEvent::Record(record) => write_record(&mut svg, &file, caps.max_bytes, title.as_deref(), &mut tally, &record),
        };
        written.map_err(|error| {
            sink_error = Some(from_io(&error));
            RenderFailure::Unavailable
        })
    });
    let done = read.map_err(|failure| {
        let error = sink_error.take().unwrap_or_else(|| match failure {
            RenderFailure::ExportRefused { why: ExportRefusal::TooHeavy, .. } => too_heavy_to_export(failure, page, raster_scale, ((width_points * raster_scale) as u32, (height_points * raster_scale) as u32), limit),
            _ => failure.into(),
        });
        with_time_taken(error, started)
    })?;
    let writer = svg.take().ok_or_else(|| with_time_taken(RenderFailure::Unavailable.into(), started))?;
    tally.chars_dropped = writer.stats().chars_dropped;
    let bytes = writer.finish().map_err(|error| from_io(&error))?.written;
    publish(&temp.path, args.out, |from, to| std::fs::hard_link(from, to))?;
    drop(temp);
    let rasterised: Vec<Value> = tally
        .rasterised
        .iter()
        .filter_map(|(why, count)| {
            let why = WHYS.iter().find(|known| **known as u8 == *why)?;
            let (code, says) = why_words(*why);
            Some(json!({ "why": code, "count": count, "says": says }))
        })
        .collect();
    let notes: Vec<Value> = tally
        .notes
        .values()
        .map(|(code, count)| {
            let (name, says) = note_words(*code);
            json!({ "note": name, "count": count, "says": says })
        })
        .collect();
    Ok(json!({
        "page": page + 1,
        "out": args.out,
        "took_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "width_pt": f64::from(tally.size.0),
        "height_pt": f64::from(tally.size.1),
        "rect": args.rect,
        "text": match text { TextMode::Both => "both", TextMode::Paths => "paths", TextMode::Selectable => "selectable" },
        "raster_scale": super::as_written(raster_scale),
        "raster_scale_lowered": raster_scale < args.scale,
        "bytes": bytes,
        "all_vector": rasterised.is_empty(),
        "objects": { "paths": tally.paths, "clips": tally.clips, "glyphs": tally.glyphs, "glyph_runs": tally.glyph_runs, "text_runs": tally.text_runs, "text_chars": tally.text_chars, "images_png": tally.images_png, "images_original_jpeg": tally.images_jpeg, "chars_dropped": tally.chars_dropped },
        "rasterised": rasterised,
        "notes": notes,
        "worker_peak_bytes": doc.stats().peak_footprint_bytes,
        "restarts": done.restarts,
    }))
}

const WHYS: [Why; 13] = [Why::Image, Why::Shading, Why::TilingPattern, Why::ShadingPattern, Why::BlendOrMask, Why::GroupEffects, Why::TextOutline, Why::TextClip, Why::TooComplex, Why::Annotations, Why::Flattened, Why::MaskedImage, Why::ImageUnreadable];

fn bad_rect() -> AgentError {
    AgentError::new("bad_rect", "A box is x,y,width,height in points from the page's top left, each a number, with a width and a height above 0.", exit::FAILED)
}
