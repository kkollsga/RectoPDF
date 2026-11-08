//! A page's images: where each one is, and one of them handed over as itself. Both go through the worker's contents walk with only images asked for (`recto_proto::contents::IMAGES_LIST` and a number), so they're counted the same way, a number from the list is the image it names, and nothing else of the page is read or drawn.
//!
//! **Losslessly** (SPEC section 6, "Click an image to copy it losslessly"): a JPEG goes as its own bytes, with only what's around its picture corrected where the page would read it differently (`JpegHeaderFixed`), and anything else as PNG at the image's own resolution, its pixels as PDFium decodes them. A masked image goes as PNG with its mask as alpha. An image PDFium can draw but not hand over is drawn alone at the scale asked for, and says so.

use super::{Output, Shared, page_index};
use crate::{AgentError, exit};
use recto_proto::contents::{IMAGES_LIST, ImageCoding, Record, TextMode, Why, hints};
use recto_render_service::{ContentsEvent, ContentsRequest, DocRenderer, OutputCaps, RenderFailure};
use recto_svg::{PngChannels, PngWriter};
use std::cell::RefCell;
use std::io::{self, Write};
use std::path::Path;

/// The most images a list keeps. A page with more says how many it has, and these are the first, in the order they're drawn.
pub(crate) const MAX_LISTED: usize = 4096;

/// Where an image is on a page, and what it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedImage {
    /// From 1, in the order the page draws them.
    pub number: u32,
    /// `x, y, width, height` in points from the page's top left, as it's shown, held to the page.
    pub rect: [f32; 4],
    /// Its own size in pixels.
    pub width: u32,
    pub height: u32,
    /// Its stream is a JPEG. It can still go as PNG, if its bytes wouldn't mean what the page means by them.
    pub jpeg: bool,
}

/// A page's images, as listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ImageList {
    pub images: Vec<PlacedImage>,
    /// Every image the page has, which is more than `images` past [`MAX_LISTED`].
    pub total: u32,
    pub page_size: (f32, f32),
}

impl ImageList {
    /// The image a click lands on: the last drawn, so the one on top, of those whose box holds the point. `x` and `y` are on the page as it's shown.
    pub(crate) fn at(&self, x: f32, y: f32) -> Option<PlacedImage> {
        self.images.iter().rev().find(|image| {
            let [left, top, width, height] = image.rect;
            x >= left && x <= left + width && y >= top && y <= top + height
        }).copied()
    }
}

/// Takes `[a b c d e f]` of a point, as PDF writes a transform.
fn apply(m: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Where an image's unit square lands, through its own matrix and then the page's, as a box held to the page.
pub(crate) fn shown_box(image: &[f32; 6], page: &[f32; 6], (width, height): (f32, f32)) -> Option<[f32; 4]> {
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(x, y)| {
        let (x, y) = apply(image, x, y);
        apply(page, x, y)
    });
    let left = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min).max(0.0);
    let right = corners.iter().map(|c| c.0).fold(f32::NEG_INFINITY, f32::max).min(width);
    let top = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min).max(0.0);
    let bottom = corners.iter().map(|c| c.1).fold(f32::NEG_INFINITY, f32::max).min(height);
    (right > left && bottom > top && [left, right, top, bottom].iter().all(|v| v.is_finite())).then_some([left, top, right - left, bottom - top])
}

/// Lists a page's images. A page whose drawing needs the larger limit waits for it, as any export does, so never on a UI thread. `page` is from 0.
pub(crate) fn list(doc: &mut DocRenderer, path: &Path, page: u32, caps: &OutputCaps) -> Result<ImageList, AgentError> {
    let request = ContentsRequest { page, generation: 0, scale: 1.0, rect: None, text: TextMode::Paths, hints: super::svg::hints_for(path, page), images: IMAGES_LIST, max_time: caps.max_time };
    let mut list = ImageList::default();
    let mut root = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    doc.read_contents(&request, |event| {
        match event {
            ContentsEvent::Restart => list = ImageList::default(),
            ContentsEvent::Record(Record::Page { width, height, matrix }) => {
                list.page_size = (width, height);
                root = matrix;
            }
            ContentsEvent::Record(Record::ImagePlace { width, height, jpeg, matrix }) => {
                list.total = list.total.saturating_add(1);
                // An image wholly off the page keeps its number, and can't be clicked.
                if list.images.len() < MAX_LISTED
                    && let Some(rect) = shown_box(&matrix, &root, list.page_size)
                {
                    list.images.push(PlacedImage { number: list.total, rect, width, height, jpeg });
                }
            }
            ContentsEvent::Record(_) => {}
        }
        Ok(())
    })?;
    Ok(list)
}

/// How an image is wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    /// A JPEG as its own bytes where it can be, and PNG otherwise.
    AsItIs,
    /// PNG whatever it is, decoded as the page decodes it: for `.png`, and the clipboard's PNG beside a JPEG.
    Png,
    /// Its own JPEG bytes or nothing, for `.jpg`: anything else stops at once, before a byte of it is written.
    JpegOnly,
}

/// What an image came out as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coded {
    /// Its own JPEG bytes, byte for byte.
    OriginalJpeg,
    /// Its JPEG picture byte for byte, with what's around it corrected so it reads as the page reads it.
    JpegHeaderFixed,
    /// PNG of its pixels as decoded, at its own size.
    Png,
    /// PNG at its own size with its mask as alpha, through the renderer's drawing.
    PngWithMask,
    /// PNG of the image drawn alone at the scale asked for, because it couldn't be handed over as itself.
    PngDrawn,
}

impl Coded {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::OriginalJpeg => "original_jpeg",
            Self::JpegHeaderFixed => "jpeg_header_fixed",
            Self::Png => "png",
            Self::PngWithMask => "png_with_mask",
            Self::PngDrawn => "png_drawn",
        }
    }

    pub(crate) fn is_jpeg(self) -> bool {
        matches!(self, Self::OriginalJpeg | Self::JpegHeaderFixed)
    }

    /// Whether it's the image exactly: its own bytes, or its own pixels without loss.
    pub(crate) fn lossless(self) -> bool {
        !matches!(self, Self::PngDrawn)
    }

    /// In words, for a person.
    pub(crate) fn says(self) -> &'static str {
        match self {
            Self::OriginalJpeg => "the image's own JPEG bytes, unchanged",
            Self::JpegHeaderFixed => "the image's own JPEG picture, byte for byte, with what's around it corrected so it shows as it does on the page",
            Self::Png => "a lossless PNG of the image at its own size",
            Self::PngWithMask => "a lossless PNG of the image at its own size, with its mask as transparency",
            Self::PngDrawn => "a PNG of the image drawn by itself, as it couldn't be handed over as it's stored",
        }
    }
}

/// An image as it came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CopiedImage {
    pub coded: Coded,
    pub width: u32,
    pub height: u32,
}

enum Writing<'a, O: Output> {
    Nothing,
    Jpeg,
    Png(PngWriter<Shared<'a, O>>),
    /// A mask a worker sent though it wasn't asked to apart: skipped, as the JPEG after it is the image.
    Mask,
    Done,
}

/// A JPEG asked for, of an image that can't go as its own JPEG bytes.
pub(crate) fn not_a_jpeg(page: u32, number: u32) -> AgentError {
    AgentError::new("not_a_jpeg", format!("Image {number} on page {} can't go as its own JPEG bytes, so a .jpg would be re-encoded, which loses something.", page + 1), exit::FAILED).then("Use a name ending in .png: that's lossless.")
}

fn no_such_image(page: u32, number: u32) -> AgentError {
    AgentError::new("no_such_image", format!("Page {} has no image {number}.", page + 1), exit::NOT_FOUND).then(format!("`recto images {}` lists them.", page + 1))
}

/// Writes image `number` of a page into `out`, whole or failing. `scale` is only for an image that has to be drawn. Can sleep waiting for the larger limit, so never on a UI thread. `page` is from 0.
#[allow(clippy::too_many_arguments)]
pub(crate) fn copy_into<O: Output>(doc: &mut DocRenderer, path: &Path, page: u32, number: u32, want: Want, scale: f32, caps: &OutputCaps, out: &RefCell<O>) -> Result<CopiedImage, AgentError> {
    if number == 0 || number == IMAGES_LIST {
        return Err(no_such_image(page, number));
    }
    // A masked JPEG comes as one picture with its mask in it, which is what a clipboard can hold. For PNG, a JPEG is decoded as the page decodes it.
    let mut wanted_hints = super::svg::hints_for(path, page) | hints::MASK_NOT_SEPARABLE;
    if want == Want::Png {
        wanted_hints |= hints::JPEG_READ_DIFFERENTLY;
    }
    let request = ContentsRequest { page, generation: 0, scale, rect: None, text: TextMode::Paths, hints: wanted_hints, images: number, max_time: caps.max_time };
    let mut writing: Writing<'_, O> = Writing::Nothing;
    let mut copied: Option<CopiedImage> = None;
    let mut sink_error: Option<io::Error> = None;
    let read = doc.read_contents(&request, |event| {
        let step = (|| -> io::Result<()> {
            match event {
                ContentsEvent::Restart => {
                    writing = Writing::Nothing;
                    copied = None;
                    out.try_borrow_mut().map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?.start_over()?;
                }
                ContentsEvent::Record(Record::ImageBegin { coding, why, width, height, .. }) => {
                    if why == Why::JpegMask {
                        writing = Writing::Mask;
                        return Ok(());
                    }
                    if want == Want::JpegOnly && !matches!(coding, ImageCoding::Jpeg) {
                        return Err(io::ErrorKind::Unsupported.into());
                    }
                    let coded = match (coding, why) {
                        (ImageCoding::Jpeg, Why::JpegHeaderFixed) => Coded::JpegHeaderFixed,
                        (ImageCoding::Jpeg, _) => Coded::OriginalJpeg,
                        (_, Why::Image) => Coded::Png,
                        (_, Why::ImageWithMask) => Coded::PngWithMask,
                        _ => Coded::PngDrawn,
                    };
                    writing = match coding {
                        ImageCoding::Jpeg => Writing::Jpeg,
                        ImageCoding::Pixels { channels } => {
                            let channels = match channels {
                                1 => PngChannels::Grey,
                                3 => PngChannels::Rgb,
                                _ => PngChannels::Rgba,
                            };
                            Writing::Png(PngWriter::new(Shared(out), width, height, channels)?)
                        }
                    };
                    copied = Some(CopiedImage { coded, width, height });
                }
                ContentsEvent::Record(Record::ImageBytes(bytes)) => match &mut writing {
                    Writing::Jpeg => Shared(out).write_all(bytes)?,
                    Writing::Png(png) => png.write_all(bytes)?,
                    Writing::Mask => {}
                    Writing::Nothing | Writing::Done => return Err(io::ErrorKind::InvalidData.into()),
                },
                ContentsEvent::Record(Record::ImageEnd) => {
                    match std::mem::replace(&mut writing, Writing::Done) {
                        Writing::Png(png) => {
                            png.finish()?;
                        }
                        Writing::Mask => writing = Writing::Nothing,
                        _ => {}
                    }
                }
                ContentsEvent::Record(_) => {}
            }
            Ok(())
        })();
        step.map_err(|error| {
            sink_error = Some(error);
            RenderFailure::Unavailable
        })
    });
    if let Err(failure) = read {
        return Err(match sink_error {
            Some(error) if error.kind() == io::ErrorKind::Unsupported => not_a_jpeg(page, number),
            Some(error) if error.kind() == io::ErrorKind::FileTooLarge => AgentError::new("output_too_large", "This image would be larger than RectoPDF will hand over.", exit::FAILED),
            Some(_) => AgentError::new("write_failed", "The image couldn't be written.", exit::FAILED),
            None => failure.into(),
        });
    }
    match (copied, writing) {
        (Some(copied), Writing::Done) => Ok(copied),
        _ => Err(no_such_image(page, number)),
    }
}

/// `recto images`: a page's images, where they are and what they are.
pub(crate) fn list_command(path: &Path, page: u32) -> Result<serde_json::Value, AgentError> {
    let index = page_index(page)?;
    let mut doc = super::open_for_text(path)?;
    let list = list(&mut doc, path, index, &OutputCaps::default())?;
    let images: Vec<serde_json::Value> = list
        .images
        .iter()
        .map(|image| serde_json::json!({ "number": image.number, "rect": image.rect.map(round_points), "width": image.width, "height": image.height, "jpeg": image.jpeg, "suggested_ending": if image.jpeg { "jpg" } else { "png" } }))
        .collect();
    Ok(serde_json::json!({ "worker_peak_bytes": doc.stats().peak_footprint_bytes, "page": page, "count": list.total, "listed": images.len(), "width_pt": round_points(list.page_size.0), "height_pt": round_points(list.page_size.1), "images": images }))
}

/// Points to a hundredth, so a list reads as numbers a person would write.
fn round_points(value: f32) -> f64 {
    (f64::from(value) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: an image's box comes out upside down or off where it's drawn, so a click on it copies nothing, or copies the image beside it. A PDF image's unit square goes through its matrix and then the page's, whose y runs down.
    #[test]
    fn an_images_box_is_where_the_page_shows_it() {
        // A US letter page, shown from its top left, y down.
        let page = [1.0, 0.0, 0.0, -1.0, 0.0, 792.0];
        let image = [100.0, 0.0, 0.0, 60.0, 100.0, 500.0];
        assert_eq!(shown_box(&image, &page, (612.0, 792.0)), Some([100.0, 232.0, 100.0, 60.0]));
        // Turned a quarter, and past the page's right edge: held to it.
        let turned = [0.0, 50.0, -80.0, 0.0, 600.0, 100.0];
        assert_eq!(shown_box(&turned, &page, (612.0, 792.0)), Some([520.0, 642.0, 80.0, 50.0]));
        assert_eq!(shown_box(&[10.0, 0.0, 0.0, 10.0, 700.0, 10.0], &page, (612.0, 792.0)), None, "wholly off the page");
        assert_eq!(shown_box(&[f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0], &page, (612.0, 792.0)), None);
    }

    /// Failure: a click where two images overlap copies the one underneath, which isn't what the reader sees.
    #[test]
    fn a_click_takes_the_image_on_top() {
        let image = |number, rect| PlacedImage { number, rect, width: 1, height: 1, jpeg: false };
        let list = ImageList { images: vec![image(1, [0.0, 0.0, 100.0, 100.0]), image(2, [50.0, 50.0, 100.0, 100.0])], total: 2, page_size: (612.0, 792.0) };
        assert_eq!(list.at(75.0, 75.0).map(|image| image.number), Some(2));
        assert_eq!(list.at(10.0, 10.0).map(|image| image.number), Some(1));
        assert_eq!(list.at(300.0, 300.0), None);
    }
}
