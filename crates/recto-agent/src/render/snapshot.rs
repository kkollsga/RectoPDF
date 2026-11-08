//! A snapshot: a box on a page as a picture, an SVG or text (SPEC section 6, "Snapshot tool"). Each comes from what the rest of the app already does, cut to the box: the export's drawing for PNG and JPG, `export svg` with a box for SVG, and a copy's text rules for text. The box is in points from the page's top left, as it's shown, as `export svg --rect` and `select --at` take it.
//!
//! A picture is an export: sharp at the scale asked for, or not made (owner, 2026-11-07). JPG is re-encoded, and says so (SPEC, "JPG honesty").

use super::jpeg::{JpegWriter, QUALITY};
use super::{Output, Shared, too_heavy_to_export, with_time_taken, write_error_to_agent};
use crate::{AgentError, exit};
use recto_index::chrome::EdgeLine;
use recto_index::select::{BoxText, PageText, Rect, neighbour_pages};
use recto_proto::BYTES_PER_PIXEL;
use recto_render_service::{DocRenderer, ExportRefusal, OutputCaps, PageEvent, Policy, RenderFailure, draw_page_box};
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
pub(crate) fn picture_into<O: Output>(doc: &mut DocRenderer, page: u32, rect: [f32; 4], scale: f32, format: Format, caps: &OutputCaps, out: &RefCell<O>) -> Result<(u32, u32), AgentError> {
    pictures_into(doc, page, rect, scale, format, caps, out, None, 0).map(|(size, _)| size)
}

/// [`picture_into`], and a JPG of the same rows into `jpg` as it goes, if given: one drawing for both, as the clipboard has them. The JPG is an extra, so one that fails, as by passing its buffer's cap, is dropped and the rest goes on; the second value says whether it came. `generation` is what a [`recto_render_service::CancelHandle`] stops.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pictures_into<O: Output>(doc: &mut DocRenderer, page: u32, rect: [f32; 4], scale: f32, format: Format, caps: &OutputCaps, out: &RefCell<O>, jpg: Option<&RefCell<O>>, generation: u64) -> Result<((u32, u32), bool), AgentError> {
    let started = Instant::now();
    let mut writer: Option<Writer<'_, O>> = None;
    let mut extra: Option<Box<JpegWriter<Shared<'_, O>>>> = None;
    let mut row = Vec::new();
    let mut rgb = Vec::new();
    let mut size = None;
    let mut sink_error = None;
    let mut surface = None;
    let drawn = draw_page_box(doc, page, scale, rect, generation, caps, &mut surface, |event| match event {
        PageEvent::Begin { width, height } => {
            size = Some((width, height));
            row = vec![0u8; width as usize * BYTES_PER_PIXEL as usize];
            rgb = vec![0u8; width as usize * 3];
            extra = jpg.and_then(|jpg| JpegWriter::new(Shared(jpg), width, height, QUALITY).ok().map(Box::new));
            let made = match format {
                Format::Jpg => JpegWriter::new(Shared(out), width, height, QUALITY).map(|jpeg| Writer::Jpeg(Box::new(jpeg))),
                _ => PngWriter::new(Shared(out), width, height, PngChannels::Rgb).map(Writer::Png),
            };
            match made {
                Ok(made) => {
                    writer = Some(made);
                    Ok(())
                }
                Err(error) => {
                    sink_error = Some(write_error_to_agent(&error));
                    Err(RenderFailure::Unavailable)
                }
            }
        }
        PageEvent::Rows { memory, rect, stride, .. } => {
            let Some(writer) = writer.as_mut() else { return Err(RenderFailure::Unavailable) };
            for y in 0..rect.height as usize {
                // A copy out, never a look in place: the worker still holds this memory, and it isn't trusted.
                if !memory.copy_out(y * stride as usize, &mut row) {
                    return Err(RenderFailure::Unavailable);
                }
                for (to, from) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(row.as_chunks::<{ BYTES_PER_PIXEL as usize }>().0) {
                    to.copy_from_slice(&[from[2], from[1], from[0]]);
                }
                let written = match writer {
                    Writer::Png(png) => std::io::Write::write_all(png, &rgb),
                    Writer::Jpeg(jpeg) => jpeg.write_rgb_row(&rgb),
                };
                if let Err(error) = written {
                    sink_error = Some(write_error_to_agent(&error));
                    return Err(RenderFailure::Unavailable);
                }
                if extra.as_mut().is_some_and(|jpeg| jpeg.write_rgb_row(&rgb).is_err()) {
                    extra = None;
                }
            }
            Ok(())
        }
    })
    .map_err(|failure| {
        let error = sink_error.take().unwrap_or_else(|| match (failure, size) {
            (RenderFailure::ExportRefused { why: ExportRefusal::TooHeavy, .. }, Some(size)) => too_heavy_to_export(failure, page, scale, size, Policy::default().heavy_limit_bytes),
            (RenderFailure::BadRequest, _) => AgentError::new("bad_rect", "That box isn't on the page.", exit::FAILED).then("A box is x,y,width,height in points from the page's top left, as it's shown, and starts on the page."),
            _ => failure.into(),
        });
        with_time_taken(error, started)
    })?;
    let finished = match writer {
        Some(Writer::Png(png)) => png.finish().map(drop),
        Some(Writer::Jpeg(jpeg)) => jpeg.finish().map(drop),
        None => return Err(RenderFailure::Unavailable.into()),
    };
    finished.map_err(|error| write_error_to_agent(&error))?;
    let jpg_came = extra.is_some_and(|jpeg| jpeg.finish().is_ok());
    Ok(((drawn.width, drawn.height), jpg_came))
}

enum Writer<'a, O: Output> {
    Png(PngWriter<Shared<'a, O>>),
    Jpeg(Box<JpegWriter<Shared<'a, O>>>),
}

/// What the box holds as text, by a copy's rules: hidden text left out, running heads and page numbers left out of body text, in reading order. `page` is from 0.
pub(crate) fn text(doc: &mut DocRenderer, page: u32, rect: [f32; 4]) -> Result<BoxText, AgentError> {
    let characters = doc.page_characters(page, true, true)?;
    let own = PageText::build(&characters.chars, &characters.boxes, characters.frame, characters.hidden_checked, characters.copy_restricted);
    let chrome = if own.edge_lines().is_empty() {
        Vec::new()
    } else {
        let pages = doc.page_count().unwrap_or(page + 1);
        let neighbours: Vec<(i64, Vec<EdgeLine>)> = neighbour_pages(page, pages).filter_map(|other| Some((i64::from(other) - i64::from(page), doc.page_characters(other, true, false).ok()?))).map(|(away, read)| (away, PageText::build(&read.chars, &read.boxes, read.frame, false, read.copy_restricted).edge_lines().to_vec())).collect();
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
