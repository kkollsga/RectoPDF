//! A snapshot: a box on a page as a picture, an SVG or text (SPEC section 6, "Snapshot tool"). Each comes from what the rest of the app already does, cut to the box: the export's drawing for PNG and JPG, `export svg` with a box for SVG, and a copy's text rules for text. The box is in points from the page's top left, as it's shown, as `export svg --rect` and `select --at` take it.
//!
//! A picture is an export: sharp at the scale asked for, or not made (owner, 2026-11-07). JPG is re-encoded, and says so (SPEC, "JPG honesty").

use super::jpeg::{JpegWriter, QUALITY};
use super::{Output, Shared, too_heavy_to_export, with_time_taken, write_error_to_agent};
use crate::{AgentError, exit};
use recto_index::chrome::EdgeLine;
use recto_index::select::{BoxText, PageText, Rect, neighbour_pages};
use recto_proto::BYTES_PER_PIXEL;
use recto_render_service::{
    DocRenderer, ExportRefusal, OutputCaps, PageEvent, Policy, RenderFailure, TileRect, draw_page_box, draw_page_region, page_box_region,
};
use recto_svg::{PngChannels, PngWriter};
use std::cell::RefCell;
use std::path::Path;
use std::time::Instant;

/// The formats a snapshot comes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpg,
    Svg,
    Text,
}

impl Format {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpg),
            "svg" => Some(Self::Svg),
            "text" | "txt" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
            Self::Svg => "svg",
            Self::Text => "text",
        }
    }

    /// What a file of it may end in, the first being what's suggested.
    pub fn endings(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["png"],
            Self::Jpg => &["jpg", "jpeg"],
            Self::Svg => &["svg"],
            Self::Text => &["txt"],
        }
    }
}

/// The most text a snapshot takes: a box can't hold more than its page, and a page past the selectable limit holds none.
pub(crate) const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// Draws the box at `scale` into `out` as PNG or JPG. Returns the picture's size in pixels. `page` is from 0. Can sleep waiting for the larger limit, so never on a UI thread.
pub(crate) fn picture_into<O: Output>(
    doc: &mut DocRenderer,
    page: u32,
    rect: [f32; 4],
    scale: f32,
    format: Format,
    caps: &OutputCaps,
    out: &RefCell<O>,
) -> Result<(u32, u32), AgentError> {
    pictures_into(doc, page, rect, scale, format, caps, out, None, 0, 0).map(|(size, _)| size)
}

/// How much of a turned snapshot is held at once, as RGB: a band of the box, drawn and then written out turned. A box at the usual scale is one band; a larger one is drawn in several, so memory stays at this whatever its size.
const TURN_BAND_BYTES: usize = 16 * 1024 * 1024;

/// [`picture_into`], and a JPG of the same rows into `jpg` as it goes, if given: one drawing for both, as the clipboard has them. The JPG is an extra, so one that fails, as by passing its buffer's cap, is dropped and the rest goes on; the second value says whether it came. `generation` is what a [`recto_render_service::CancelHandle`] stops.
///
/// **`quarter_turns` turns the picture clockwise**, as the page is seen with an unsaved turn: the box is still on the page as the file shows it, and the picture comes out turned. Turned, the box is drawn a band at a time, at most [`TURN_BAND_BYTES`] of it held, and each band written out turned: columns for a quarter turn, rows from the bottom for a half.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pictures_into<O: Output>(
    doc: &mut DocRenderer,
    page: u32,
    rect: [f32; 4],
    scale: f32,
    format: Format,
    caps: &OutputCaps,
    out: &RefCell<O>,
    jpg: Option<&RefCell<O>>,
    generation: u64,
    quarter_turns: u8,
) -> Result<((u32, u32), bool), AgentError> {
    let started = Instant::now();
    let mut encoders: Option<Encoders<'_, O>> = None;
    let mut sink_error = None;
    let mut size = None;
    let turns = quarter_turns % 4;
    let drawn = if turns == 0 {
        let mut row = Vec::new();
        let mut rgb = Vec::new();
        let mut surface = None;
        draw_page_box(doc, page, scale, rect, generation, caps, &mut surface, |event| match event {
            PageEvent::Begin { width, height } => {
                size = Some((width, height));
                row = vec![0u8; width as usize * BYTES_PER_PIXEL as usize];
                rgb = vec![0u8; width as usize * 3];
                encoders = Some(Encoders::new(format, out, jpg, width, height).map_err(|error| {
                    sink_error = Some(error);
                    RenderFailure::Unavailable
                })?);
                Ok(())
            }
            PageEvent::Rows { memory, rect, stride, .. } => {
                let encoders = encoders.as_mut().ok_or(RenderFailure::Unavailable)?;
                for y in 0..rect.height as usize {
                    // A copy out, never a look in place: the worker still holds this memory, and it isn't trusted.
                    if !memory.copy_out(y * stride as usize, &mut row) {
                        return Err(RenderFailure::Unavailable);
                    }
                    bgra_to_rgb(&row, &mut rgb);
                    encoders.row(&rgb).map_err(|error| {
                        sink_error = Some(error);
                        RenderFailure::Unavailable
                    })?;
                }
                Ok(())
            }
        })
        .map(|drawn| (drawn.width, drawn.height))
    } else {
        let emit = |turned: Turned<'_>| match turned {
            Turned::Begin { width, height } => {
                size = Some((width, height));
                encoders = Some(Encoders::new(format, out, jpg, width, height)?);
                Ok(())
            }
            Turned::Row(rgb) => encoders.as_mut().ok_or_else(|| AgentError::from(RenderFailure::Unavailable))?.row(rgb),
        };
        draw_turned(doc, page, rect, scale, caps, generation, (turns, TURN_BAND_BYTES), emit, &mut sink_error)
    }
    .map_err(|failure| {
        let error = sink_error.take().unwrap_or_else(|| match (failure, size) {
            (
                RenderFailure::ExportRefused {
                    why: ExportRefusal::TooHeavy, ..
                },
                Some(size),
            ) => too_heavy_to_export(failure, page, scale, size, Policy::default().heavy_limit_bytes),
            (RenderFailure::BadRequest, _) => AgentError::new("bad_rect", "That box isn't on the page.", exit::FAILED)
                .then("A box is x,y,width,height in points from the page's top left, as it's shown, and starts on the page."),
            _ => failure.into(),
        });
        with_time_taken(error, started)
    })?;
    let jpg_came = encoders.ok_or_else(|| AgentError::from(RenderFailure::Unavailable))?.finish()?;
    Ok((drawn, jpg_came))
}

/// One row of the worker's BGRA as RGB.
fn bgra_to_rgb(row: &[u8], rgb: &mut [u8]) {
    for (to, from) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(row.as_chunks::<{ BYTES_PER_PIXEL as usize }>().0) {
        to.copy_from_slice(&[from[2], from[1], from[0]]);
    }
}

/// What a turned drawing hands on: the turned picture's size, once, then its rows, top to bottom, as RGB.
enum Turned<'a> {
    Begin { width: u32, height: u32 },
    Row(&'a [u8]),
}

/// Draws the box turned `turns` quarter turns clockwise, 1 to 3, to `emit`, at most `band_bytes` of it held at once. An error from `emit` is kept in `sink_error` and the drawing stops. Returns the turned size.
#[allow(clippy::too_many_arguments)]
fn draw_turned(
    doc: &mut DocRenderer,
    page: u32,
    rect: [f32; 4],
    scale: f32,
    caps: &OutputCaps,
    generation: u64,
    (turns, band_bytes): (u8, usize),
    mut emit: impl FnMut(Turned<'_>) -> Result<(), AgentError>,
    sink_error: &mut Option<AgentError>,
) -> Result<(u32, u32), RenderFailure> {
    let region = page_box_region(doc, page, scale, rect, caps)?;
    let odd = turns % 2 == 1;
    let (wide, high) = (region.width, region.height);
    let turned = if odd { (high, wide) } else { (wide, high) };
    let kept = |result: Result<(), AgentError>, sink_error: &mut Option<AgentError>| {
        result.map_err(|error| {
            *sink_error = Some(error);
            RenderFailure::Unavailable
        })
    };
    kept(
        emit(Turned::Begin {
            width: turned.0,
            height: turned.1,
        }),
        sink_error,
    )?;
    // A band is whole columns for a quarter turn, whole rows for a half, as many as fit in the band's bytes.
    let (lines, line_bytes) = if odd { (wide, high as usize * 3) } else { (high, wide as usize * 3) };
    let per_band = u32::try_from((band_bytes / line_bytes.max(1)).max(1)).unwrap_or(u32::MAX).min(lines);
    let mut source = vec![0u8; wide as usize * BYTES_PER_PIXEL as usize];
    let mut out = vec![0u8; turned.0 as usize * 3];
    let mut surface = None;
    let mut done = 0u32;
    while done < lines {
        let count = per_band.min(lines - done);
        // A quarter turn takes columns from the left, three from the right; a half takes rows from the bottom.
        let first = if turns == 1 { done } else { lines - done - count };
        let part = if odd {
            TileRect {
                x: region.x + first,
                y: region.y,
                width: count,
                height: high,
            }
        } else {
            TileRect {
                x: region.x,
                y: region.y + first,
                width: wide,
                height: count,
            }
        };
        let mut band = vec![0u8; part.width as usize * part.height as usize * 3];
        let band_stride = part.width as usize * 3;
        draw_page_region(doc, page, scale, region, part, generation, caps, &mut surface, |event| {
            let PageEvent::Rows {
                memory, rect: rows, stride, ..
            } = event
            else {
                return Ok(());
            };
            let row_bytes = rows.width as usize * BYTES_PER_PIXEL as usize;
            for y in 0..rows.height as usize {
                let at = (rows.y - part.y) as usize + y;
                if !memory.copy_out(y * stride as usize, &mut source[..row_bytes]) || at >= part.height as usize {
                    return Err(RenderFailure::Unavailable);
                }
                bgra_to_rgb(&source[..row_bytes], &mut band[at * band_stride..(at + 1) * band_stride]);
            }
            Ok(())
        })?;
        let pixel = |x: usize, y: usize| &band[y * band_stride + x * 3..y * band_stride + x * 3 + 3];
        let (band_wide, band_high) = (part.width as usize, part.height as usize);
        for step in 0..count as usize {
            match turns {
                // Column `step` of the band, read from the bottom up.
                1 => {
                    for (c, to) in out.chunks_exact_mut(3).enumerate() {
                        to.copy_from_slice(pixel(step, band_high - 1 - c));
                    }
                }
                // The band's columns from its right, each read from the top down.
                3 => {
                    let x = band_wide - 1 - step;
                    for (c, to) in out.chunks_exact_mut(3).enumerate() {
                        to.copy_from_slice(pixel(x, c));
                    }
                }
                // The band's rows from its bottom, each read from the right.
                _ => {
                    let y = band_high - 1 - step;
                    for (c, to) in out.chunks_exact_mut(3).enumerate() {
                        to.copy_from_slice(pixel(band_wide - 1 - c, y));
                    }
                }
            }
            kept(emit(Turned::Row(&out)), sink_error)?;
        }
        done += count;
    }
    Ok(turned)
}

/// The PNG or JPG a snapshot is written as, and the JPG beside it if asked, fed a row of RGB at a time.
struct Encoders<'a, O: Output> {
    main: Writer<'a, O>,
    extra: Option<Box<JpegWriter<Shared<'a, O>>>>,
}

impl<'a, O: Output> Encoders<'a, O> {
    fn new(format: Format, out: &'a RefCell<O>, jpg: Option<&'a RefCell<O>>, width: u32, height: u32) -> Result<Self, AgentError> {
        let main = match format {
            Format::Jpg => JpegWriter::new(Shared(out), width, height, QUALITY).map(|jpeg| Writer::Jpeg(Box::new(jpeg))),
            _ => PngWriter::new(Shared(out), width, height, PngChannels::Rgb).map(Writer::Png),
        }
        .map_err(|error| write_error_to_agent(&error))?;
        let extra = jpg.and_then(|jpg| JpegWriter::new(Shared(jpg), width, height, QUALITY).ok().map(Box::new));
        Ok(Self { main, extra })
    }

    fn row(&mut self, rgb: &[u8]) -> Result<(), AgentError> {
        let written = match &mut self.main {
            Writer::Png(png) => std::io::Write::write_all(png, rgb),
            Writer::Jpeg(jpeg) => jpeg.write_rgb_row(rgb),
        };
        written.map_err(|error| write_error_to_agent(&error))?;
        if self.extra.as_mut().is_some_and(|jpeg| jpeg.write_rgb_row(rgb).is_err()) {
            self.extra = None;
        }
        Ok(())
    }

    /// Ends both, and says whether the JPG beside it came.
    fn finish(self) -> Result<bool, AgentError> {
        match self.main {
            Writer::Png(png) => png.finish().map(drop),
            Writer::Jpeg(jpeg) => jpeg.finish().map(drop),
        }
        .map_err(|error| write_error_to_agent(&error))?;
        Ok(self.extra.is_some_and(|jpeg| jpeg.finish().is_ok()))
    }
}

enum Writer<'a, O: Output> {
    Png(PngWriter<Shared<'a, O>>),
    Jpeg(Box<JpegWriter<Shared<'a, O>>>),
}

/// What the box holds as text, by a copy's rules: hidden text left out, running heads and page numbers left out of body text, in reading order. `page` is from 0.
pub(crate) fn text(doc: &mut DocRenderer, page: u32, rect: [f32; 4]) -> Result<BoxText, AgentError> {
    let characters = doc.page_characters(page, true, true)?;
    let own = PageText::build(
        &characters.chars,
        &characters.boxes,
        characters.frame,
        characters.hidden_checked,
        characters.copy_restricted,
    );
    let chrome = if own.edge_lines().is_empty() {
        Vec::new()
    } else {
        let pages = doc.page_count().unwrap_or(page + 1);
        let neighbours: Vec<(i64, Vec<EdgeLine>)> = neighbour_pages(page, pages)
            .filter_map(|other| Some((i64::from(other) - i64::from(page), doc.page_characters(other, true, false).ok()?)))
            .map(|(away, read)| {
                (
                    away,
                    PageText::build(&read.chars, &read.boxes, read.frame, false, read.copy_restricted)
                        .edge_lines()
                        .to_vec(),
                )
            })
            .collect();
        let borrowed: Vec<(i64, &[EdgeLine])> = neighbours.iter().map(|(away, lines)| (*away, lines.as_slice())).collect();
        own.chrome_spans(&borrowed)
    };
    let [x, y, width, height] = rect;
    Ok(own.text_in_box(Rect { x, y, width, height }, &chrome, MAX_TEXT_BYTES))
}

/// **Whether the file asks that it isn't copied**: the document-wide flag, read straight from the file by the structure reader, with no page and no worker (`recto_core::copy_restricted`). Only a file the structure reader can't open asks the renderer, through `doc` if one's open and a worker of its own if not, and a file neither can say is taken as not asking: the flag is advice, and an image copy mustn't fail because a page's characters can't be read. `page` is from 0.
pub(crate) fn copy_restricted(path: &Path, doc: Option<&mut DocRenderer>, page: u32) -> bool {
    if let Ok(flag) = recto_core::copy_restricted(path) {
        return flag;
    }
    let mut opened = None;
    let doc = match doc {
        Some(doc) => doc,
        None => match super::open_for_text(path) {
            Ok(made) => opened.insert(made),
            Err(_) => return false,
        },
    };
    doc.page_characters(page, false, false).is_ok_and(|read| read.copy_restricted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a turned snapshot too large for one band comes out with its bands in the wrong order, a seam, or a band's rows the wrong way round, which a box that fits in one band never shows. Drawn in bands of a few columns or rows each, the picture must be the one drawn in one band.
    #[test]
    fn a_turned_picture_is_the_same_in_many_bands_as_in_one() {
        let content = [
            recto_testpdf::text_lines(&["Bands of a turned snapshot", "joined without a seam"]),
            b"1 0 0 rg 100 600 80 40 re f 0 0 1 rg 300 520 40 120 re f\n".to_vec(),
        ]
        .concat();
        let file = recto_testpdf::TempPdf::new("bands.pdf", &recto_testpdf::pages(&[recto_testpdf::Page::new(content)]));
        let mut doc = crate::render::open_for_text(&file.0).unwrap();
        let caps = OutputCaps::default();
        let rect = [60.0, 50.0, 330.0, 250.0];
        for turns in 1..4u8 {
            let mut pictures = Vec::new();
            for band_bytes in [usize::MAX / 2, 7 * 1024] {
                let mut picture = Vec::new();
                let mut size = None;
                let mut sink_error = None;
                let drawn = draw_turned(
                    &mut doc,
                    0,
                    rect,
                    1.0,
                    &caps,
                    0,
                    (turns, band_bytes),
                    |turned| {
                        match turned {
                            Turned::Begin { width, height } => size = Some((width, height)),
                            Turned::Row(rgb) => picture.extend_from_slice(rgb),
                        }
                        Ok(())
                    },
                    &mut sink_error,
                )
                .unwrap();
                assert_eq!(Some(drawn), size);
                let (width, height) = drawn;
                assert_eq!(picture.len(), width as usize * height as usize * 3, "every row, turned {turns}");
                pictures.push(picture);
            }
            assert!(pictures[0] == pictures[1], "turned {turns}: bands don't join into the picture");
            assert!(pictures[0].iter().any(|&value| value < 128), "something was drawn");
        }
    }
}
