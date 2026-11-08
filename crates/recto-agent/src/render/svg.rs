//! `export svg`: a page, or a box on it, as a new SVG file, written as it's read. Nothing is held: records come from the worker a frame at a time, and go straight through `recto-svg` to a temp file that's only published if the whole page got there. So a worker that's stopped, or a limit that's hit, leaves nothing behind under the real name.

use super::{OwnTemp, hidden_text_facts, largest_scale_that_fits, output_exists, page_index, publish, too_heavy_to_export, with_time_taken};
use crate::{AgentError, exit};
use recto_proto::jpeg::{JpegFacts, JpegScan};
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
    /// Takes out of JPEGs what isn't the picture, and the document's title. Off by default: a JPEG goes as its original bytes (owner, 2026-11-07).
    pub strip_metadata: bool,
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
        Why::OutsideBox => ("outside_box", "something crossing the box's edge, drawn as a picture cut to the box, so nothing outside it is in the file"),
        Why::ClipRule => ("clip_rule", "something clipped by a shape that its fill rule would change, which can't be read, drawn as a picture that obeys it"),
        Why::ImageWithMask => ("image_with_mask", "an image at its own size, through the renderer's drawing to apply its mask"),
        Why::JpegHeaderFixed => ("jpeg_header_fixed", "a JPEG with what's around its picture corrected, and the picture byte for byte"),
        Why::JpegMask => ("jpeg_mask", "a JPEG's mask, at the JPEG's size"),
        Why::JpegOverMask => ("jpeg_with_mask", "a JPEG with its picture byte for byte, through its mask"),
    }
}

fn note_words(code: NoteCode) -> (&'static str, &'static str) {
    match code {
        NoteCode::NotFinite => ("not_finite", "objects with numbers that make no sense, left out"),
        NoteCode::HiddenLayer => ("hidden_layer", "objects in layers that are off, left out"),
        NoteCode::InvisibleText => ("invisible_text_chars", "characters of text that draw nothing, kept as selectable text"),
        NoteCode::Unknown => ("unknown_object", "objects the renderer says nothing about, left out"),
        NoteCode::LayersUnchecked => ("layers_unchecked", "objects whose layer wasn't looked up, past a limit: they're in the picture of the whole page, which obeys it, and any text in them isn't kept"),
        NoteCode::ObjectsSkipped => ("objects_past_limit", "objects past the limit on one page, in the picture of the whole page"),
        NoteCode::JpegTurned => ("jpeg_turned", "JPEGs whose EXIF says to turn them, which the page takes no notice of and a browser does, so it now says upright, with the picture's bytes as they were"),
        NoteCode::JpegProfileRemoved => ("jpeg_profile_removed", "JPEGs whose own colour profile the page doesn't go by, taken out so a browser reads them as the page does, with the picture's bytes as they were"),
        NoteCode::JpegMeaningAsPng => ("jpeg_meaning_as_png", "JPEGs that went as PNG, in the colours the renderer gives, because correcting what's around the picture can't make a browser read them as the page does: a /Decode or /ColorTransform, one written in place, a colour space like CMYK, or a mask blended ahead of time"),
        NoteCode::JpegCutAsPng => ("jpeg_cut_as_png", "JPEGs that went as PNG because the box cuts them, so that nothing outside it is in the file"),
        NoteCode::JpegProfileAdded => ("jpeg_profile_added", "JPEGs given the colour profile the page says they're in, so a browser reads them as the page does, with the picture's bytes as they were"),
        NoteCode::JpegUnsound => ("jpeg_unsound", "JPEGs that aren't sound as JPEGs, so they went as PNG, as the renderer reads them"),
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
    images_masked: u64,
    /// JPEGs with what's around the picture corrected.
    images_jpeg_fixed: u64,
    /// JPEGs with their picture as it was, through a mask of their own.
    images_jpeg_masked: u64,
    /// Bytes in the file by kind of image, as the images' base64 stands there, from where the one in hand began.
    image_bytes: BTreeMap<&'static str, u64>,
    image_start: (u64, &'static str),
    /// Whether the JPEG being stripped is counted as original bytes.
    stripping_original: bool,
    /// JPEGs with what isn't the picture taken out, so they aren't original bytes.
    images_jpeg_stripped: u64,
    /// What's taking that out of the JPEG in hand, with what it's written so far.
    stripping: Option<JpegScan>,
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
#[allow(clippy::too_many_arguments)]
fn write_record<'file>(svg: &mut Option<SvgWriter<Capped<'file>>>, file: &'file File, limit: u64, title: Option<&str>, strip: bool, tally: &mut Tally, record: &Record<'_>) -> io::Result<()> {
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
            if strip && matches!(coding, ImageCoding::Jpeg) {
                tally.stripping = Some(JpegScan::new(true));
                tally.stripping_original = *why == Why::Image;
            }
            match (why, coding) {
                (Why::Image, ImageCoding::Jpeg) => tally.images_jpeg += 1,
                (Why::Image, _) => tally.images_png += 1,
                (Why::ImageWithMask, _) => tally.images_masked += 1,
                (Why::JpegHeaderFixed, _) => tally.images_jpeg_fixed += 1,
                (Why::JpegOverMask, _) => tally.images_jpeg_masked += 1,
                (Why::JpegMask, _) => {}
                _ => *tally.rasterised.entry(*why as u8).or_default() += 1,
            }
            tally.image_start = (writer_position(writer), image_kind(*why, coding));
            match why {
                Why::JpegMask => writer.mask_begin(data)?,
                Why::JpegOverMask => writer.image_begin_masked(data, &mat(matrix), *opacity)?,
                _ => writer.image_begin(data, &mat(matrix), *opacity, !matches!(why, Why::Image | Why::ImageWithMask | Why::JpegHeaderFixed))?,
            }
        }
        Record::ImageBytes(bytes) => match tally.stripping.as_mut() {
            Some(stripping) => {
                let mut kept = Vec::with_capacity(bytes.len());
                stripping.feed(bytes, &mut kept).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
                writer.image_bytes(&kept)?;
            }
            None => writer.image_bytes(bytes)?,
        },
        Record::ImageEnd => {
            if tally.stripping.take().is_some_and(|stripping| stripping.changed()) && tally.stripping_original {
                tally.images_jpeg = tally.images_jpeg.saturating_sub(1);
                tally.images_jpeg_stripped += 1;
            }
            writer.image_end()?;
            let (start, kind) = tally.image_start;
            *tally.image_bytes.entry(kind).or_default() += writer_position(writer).saturating_sub(start);
        }
        Record::Note { code, count } => tally.notes.entry(*code as u8).or_insert((*code, 0)).1 += u64::from(*count),
    }
    Ok(())
}

/// How far into the file the writer has got.
fn writer_position(writer: &SvgWriter<Capped<'_>>) -> u64 {
    writer.out().map_or(0, |out| out.written) + writer.buffered() as u64
}

/// What an image is, for the count of bytes by kind.
fn image_kind(why: Why, coding: &ImageCoding) -> &'static str {
    match (why, coding) {
        (Why::Image, ImageCoding::Jpeg) => "original_jpeg",
        (Why::JpegHeaderFixed, _) => "fixed_jpeg",
        (Why::JpegOverMask | Why::JpegMask, _) => "jpeg_with_mask",
        (Why::Image, _) => "png_image",
        (other, _) => why_words(other).0,
    }
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
    (u8::from(effects.soft_mask) * hints::SOFT_MASK) | (u8::from(effects.blend) * hints::BLEND) | (u8::from(effects.shading_pattern) * hints::SHADING_PATTERN) | (u8::from(effects.optional_content) * hints::OPTIONAL_CONTENT) | (u8::from(effects.layer_names_unsafe) * hints::LAYER_NAMES_UNSAFE) | (u8::from(effects.mask_not_separable) * hints::MASK_NOT_SEPARABLE) | (u8::from(effects.jpeg_read_differently) * hints::JPEG_READ_DIFFERENTLY)
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
    // Only ever a new file, where nothing reads it as anything else. The ending also keeps `out` off the PDF itself, because only the append writer may change one.
    crate::saving::check_export_path(args.out, &["svg"], "SVG")?;
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
    // A box is a piece of a document, so it doesn't say what the whole is called.
    let title = if args.rect.is_some() || args.strip_metadata { None } else { recto_core::document_title(args.path) };
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
            ContentsEvent::Record(record) => write_record(&mut svg, &file, caps.max_bytes, title.as_deref(), args.strip_metadata, &mut tally, &record),
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
    // The same check `text` gives, after the page is known to draw, so a refused page pays nothing for it. It's about the whole page, and only matters if text is going in the file.
    let hidden_text = if text == TextMode::Paths {
        json!({ "checked": false })
    } else {
        doc.page_characters(page, true, true).map_or_else(|_| json!({ "checked": false }), |characters| hidden_text_facts(&characters))
    };
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
    let mut notes: Vec<Value> = tally
        .notes
        .values()
        .map(|(code, count)| {
            let (name, says) = note_words(*code);
            json!({ "note": name, "count": count, "says": says })
        })
        .collect();
    notes.extend(jpeg_notes(&done.jpeg, args.strip_metadata));
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
        "image_bytes": tally.image_bytes,
        "objects": { "paths": tally.paths, "clips": tally.clips, "glyphs": tally.glyphs, "glyph_runs": tally.glyph_runs, "text_runs": tally.text_runs, "text_chars": tally.text_chars, "images_png": tally.images_png, "images_original_jpeg": tally.images_jpeg, "images_jpeg_header_fixed": tally.images_jpeg_fixed, "images_jpeg_with_mask": tally.images_jpeg_masked, "images_drawn_with_mask": tally.images_masked, "images_jpeg_metadata_removed": tally.images_jpeg_stripped, "chars_dropped": tally.chars_dropped },
        "strip_metadata": args.strip_metadata,
        "hidden_text": hidden_text,
        "rasterised": rasterised,
        "notes": notes,
        "worker_peak_bytes": doc.stats().peak_footprint_bytes,
        "restarts": done.restarts,
    }))
}

/// What the JPEGs carried besides their pictures, in the answer's words. Without stripping, it's all still in the file, because a JPEG goes as its original bytes. Stripping keeps only what a decoder or a reader needs for the colours.
fn jpeg_notes(facts: &JpegFacts, stripped: bool) -> Vec<Value> {
    let gone = |present: &'static str, removed: &'static str| if stripped { removed } else { present };
    let items: [(&str, u64, &str); 8] = [
        ("jpeg_exif", u64::from(facts.exif), gone("EXIF blocks in the JPEGs, kept as they were, which can say when, where and on what a photo was taken", "EXIF blocks removed from the JPEGs")),
        ("jpeg_gps", u64::from(facts.gps), gone("of those with a GPS position, kept", "of those with a GPS position, removed")),
        ("jpeg_thumbnail", u64::from(facts.thumbnails), gone("thumbnails in the JPEGs, kept, which can show the photo from before any cropping", "thumbnails removed from the JPEGs")),
        ("jpeg_xmp", u64::from(facts.xmp), gone("XMP blocks in the JPEGs, kept", "XMP blocks removed from the JPEGs")),
        ("jpeg_comment", u64::from(facts.comments), gone("comments in the JPEGs, kept", "comments removed from the JPEGs")),
        ("jpeg_other_metadata", u64::from(facts.other_app), gone("other application blocks in the JPEGs, kept", "other application blocks removed from the JPEGs")),
        ("jpeg_trailing_bytes", facts.trailing_bytes, gone("bytes after the end of the JPEGs, kept as they were, which aren't part of the picture", "bytes after the end of the JPEGs removed")),
        ("jpeg_color_profile", u64::from(facts.color_profiles), "colour profiles in the JPEGs, kept because they say what the colours mean"),
    ];
    items.into_iter().filter(|(_, count, _)| *count > 0).map(|(note, count, says)| json!({ "note": note, "count": count, "says": says })).collect()
}

const WHYS: [Why; 19] = [Why::JpegHeaderFixed, Why::JpegMask, Why::JpegOverMask, Why::Image, Why::Shading, Why::TilingPattern, Why::ShadingPattern, Why::BlendOrMask, Why::GroupEffects, Why::TextOutline, Why::TextClip, Why::TooComplex, Why::Annotations, Why::Flattened, Why::MaskedImage, Why::ImageUnreadable, Why::OutsideBox, Why::ClipRule, Why::ImageWithMask];

fn bad_rect() -> AgentError {
    AgentError::new("bad_rect", "A box is x,y,width,height in points from the page's top left, each a number, with a width and a height above 0.", exit::FAILED)
}
