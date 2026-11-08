//! **Taking things out of a page**: an image, as itself, and a snapshot of a box, as a picture, an SVG or text (SPEC section 6, Extraction). One place for what goes where: `recto images`, `recto image` and `recto snapshot` write files or answer here, and the app's snapshot tool asks the same functions for what goes on the clipboard, with the types and their order decided here.
//!
//! **A file that asks not to be copied** (owner, 2026-11-08: copying images and snapshots from a copy-restricted PDF gets the same notice as text, with "Copy anyway"): refused with exit 8 and `copy_restricted` unless `--anyway`, and the app's notice offers "Copy anyway". Each use of `--anyway` that gives something goes in the activity log.

use crate::render::image::{self, Coded, CopiedImage, Want};
use crate::render::snapshot::{self, Format};
use crate::render::{FileOut, MemoryOut, OwnTemp, open_for_text, output_exists, publish_picture};
use crate::{AgentError, exit};
use recto_render_service::OutputCaps;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::Path;
use std::time::Instant;

/// The scale a picture is drawn at when nothing says: a snapshot, and an image that has to be drawn.
pub const DEFAULT_SCALE: f32 = 2.0;

/// The most one thing on the clipboard may be. Past it, that one is left off and the rest go: a snapshot's SVG of a map can be tens of MiB, and memory comes first.
pub const MAX_CLIP_ITEM: usize = 32 * 1024 * 1024;
/// The most all of one copy's clipboard may hold.
pub const MAX_CLIP_TOTAL: usize = 64 * 1024 * 1024;

/// The pasteboard type of each kind of thing, by its uniform type identifier, which is what a Mac's clipboard goes by.
pub const UTI_PNG: &str = "public.png";
pub const UTI_JPEG: &str = "public.jpeg";
pub const UTI_SVG: &str = "public.svg-image";
pub const UTI_TEXT: &str = "public.utf8-plain-text";

fn restricted() -> AgentError {
    AgentError::new("copy_restricted", "This file's permissions ask that it isn't copied.", exit::PROTECTED).then("Tell the user. `--anyway` copies it, once they've said to. Each use goes in the activity log.")
}

fn bad_number() -> AgentError {
    AgentError::new("usage", "An image is numbered from 1, as `recto images` lists them.", exit::USAGE)
}

/// `recto images`: where a page's images are.
pub(crate) fn images(path: &Path, page: u32) -> Result<Value, AgentError> {
    image::list_command(path, page)
}

/// `recto image`: one of a page's images to a new file, as itself. `.jpg` takes a JPEG's own bytes and refuses anything else, as that would be re-encoded; `.png` takes any image, losslessly.
pub(crate) fn image_to_file(path: &Path, page: u32, number: u32, out: &Path, anyway: bool) -> Result<Value, AgentError> {
    let index = page.checked_sub(1).ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))?;
    if number == 0 {
        return Err(bad_number());
    }
    crate::saving::check_export_path(out, &["png", "jpg", "jpeg"], "picture")?;
    let as_jpeg = out.extension().is_some_and(|ending| ending.eq_ignore_ascii_case("jpg") || ending.eq_ignore_ascii_case("jpeg"));
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    let started = Instant::now();
    let mut doc = open_for_text(path)?;
    if !anyway && snapshot::copy_restricted(&mut doc, index)? {
        return Err(restricted());
    }
    let (temp, file) = OwnTemp::create(out)?;
    let written = RefCell::new(FileOut(file));
    let copied = image::copy_into(&mut doc, path, index, number, if as_jpeg { Want::JpegOnly } else { Want::Png }, DEFAULT_SCALE, &OutputCaps::default(), &written)?;
    if as_jpeg && !copied.coded.is_jpeg() {
        return Err(image::not_a_jpeg(index, number));
    }
    drop(written);
    publish_picture(&temp.path, out)?;
    drop(temp);
    Ok(json!({
        "page": page,
        "number": number,
        "out": out,
        "coded": copied.coded.name(),
        "lossless": copied.coded.lossless(),
        "says": copied.coded.says(),
        "width": copied.width,
        "height": copied.height,
        "bytes": std::fs::metadata(out).map(|meta| meta.len()).ok(),
        "took_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "anyway": anyway,
        "worker_peak_bytes": doc.stats().peak_footprint_bytes,
    }))
}

/// What `recto snapshot` was asked.
pub(crate) struct SnapshotAsked<'a> {
    pub path: &'a Path,
    pub page: u32,
    pub rect: [f32; 4],
    pub format: &'a str,
    pub out: Option<&'a Path>,
    pub scale: f32,
    pub anyway: bool,
}

fn format_of(name: &str) -> Result<Format, AgentError> {
    Format::from_name(name).ok_or_else(|| AgentError::new("usage", "A snapshot is `png`, `jpg`, `svg` or `text`.", exit::USAGE))
}

/// Whether a snapshot in this format writes a file, which is what decides its level: text comes back in the answer, as `copy` does.
pub(crate) fn snapshot_writes(format: &str) -> bool {
    Format::from_name(format) != Some(Format::Text)
}

/// `recto snapshot`: a box on a page, as a picture, an SVG or text.
pub(crate) fn snapshot_command(asked: &SnapshotAsked<'_>) -> Result<Value, AgentError> {
    let page = asked.page.checked_sub(1).ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))?;
    let format = format_of(asked.format)?;
    crate::render::check_scale_and_box(asked.scale, Some(asked.rect))?;
    let started = Instant::now();
    if format == Format::Text {
        if asked.out.is_some() {
            return Err(AgentError::new("usage", "A snapshot as text comes back in the answer, as `copy` gives it, and writes no file.", exit::USAGE).then("Leave out `--out`, or choose `--as png`, `jpg` or `svg`."));
        }
        let mut doc = open_for_text(asked.path)?;
        let kept = snapshot::text(&mut doc, page, asked.rect)?;
        if kept.restricted && !asked.anyway {
            return Err(restricted());
        }
        return Ok(json!({
            "page": asked.page,
            "rect": asked.rect,
            "as": "text",
            "chars": kept.text.chars().count(),
            "hidden_left_out": kept.hidden_left_out,
            "chrome_left_out": kept.chrome_left_out,
            "checked": kept.checked,
            "stopped": kept.stopped,
            "anyway": asked.anyway,
            "took_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "untrusted": { "text": kept.text },
        }));
    }
    let out = asked.out.ok_or_else(|| AgentError::new("usage", format!("A snapshot as {} needs `--out`, a new file.", format.name()), exit::USAGE))?;
    crate::saving::check_export_path(out, format.endings(), if format == Format::Svg { "SVG" } else { "picture" })?;
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    let mut doc = open_for_text(asked.path)?;
    if !asked.anyway && snapshot::copy_restricted(&mut doc, page)? {
        return Err(restricted());
    }
    let (temp, file) = OwnTemp::create(out)?;
    let written = RefCell::new(FileOut(file));
    let mut data = json!({ "page": asked.page, "rect": asked.rect, "as": format.name(), "out": out, "anyway": asked.anyway });
    match format {
        Format::Svg => {
            let svg = crate::render::write_svg_box(&mut doc, asked.path, page, asked.rect, asked.scale, &OutputCaps::default(), &written, started)?;
            data["all_vector"] = json!(svg.all_vector());
            data["raster_scale"] = json!(svg.raster_scale);
        }
        _ => {
            let (width, height) = snapshot::picture_into(&mut doc, page, asked.rect, asked.scale, format, &OutputCaps::default(), &written)?;
            data["width"] = json!(width);
            data["height"] = json!(height);
            data["scale"] = json!(asked.scale);
            // SPEC, "JPG honesty": only an image's own bytes are a lossless JPEG, and a snapshot never is.
            data["reencoded"] = json!(format == Format::Jpg);
        }
    }
    drop(written);
    publish_picture(&temp.path, out)?;
    drop(temp);
    data["bytes"] = json!(std::fs::metadata(out).map(|meta| meta.len()).ok());
    data["worker_peak_bytes"] = json!(doc.stats().peak_footprint_bytes);
    data["took_ms"] = json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    Ok(data)
}

/// One thing on the clipboard: its type and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipItem {
    pub uti: &'static str,
    pub bytes: Vec<u8>,
}

/// What a copy puts on the clipboard, in the order a Mac's clipboard should offer it, and what to say about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clip {
    pub items: Vec<ClipItem>,
    /// A quiet line for the reader: what went, and anything left off.
    pub note: String,
}

/// How asking for a clipboard came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipOutcome {
    Ready(Clip),
    /// The file asks not to be copied, and the reader hasn't said "Copy anyway".
    Restricted,
    /// There's no image where the click was.
    NoImage,
    Failed(AgentError),
}

/// Takes one more thing for the clipboard, if it fits, and says what didn't.
fn add_item(clip: &mut Clip, uti: &'static str, made: Result<Vec<u8>, AgentError>, left_off: &mut Vec<&'static str>) {
    let used: usize = clip.items.iter().map(|item| item.bytes.len()).sum();
    match made {
        Ok(bytes) if used + bytes.len() <= MAX_CLIP_TOTAL => clip.items.push(ClipItem { uti, bytes }),
        _ => left_off.push(match uti {
            UTI_PNG => "PNG",
            UTI_JPEG => "JPG",
            UTI_SVG => "SVG",
            _ => "text",
        }),
    }
}

fn in_memory(make: impl FnOnce(&RefCell<MemoryOut>) -> Result<(), AgentError>) -> Result<Vec<u8>, AgentError> {
    let out = RefCell::new(MemoryOut::new(MAX_CLIP_ITEM));
    make(&out)?;
    Ok(out.into_inner().bytes)
}

/// The app's click on an image: what goes on the clipboard for the image at `x, y` on a page as it's shown. A JPEG goes as its own bytes, and as a PNG beside it for what can't take a JPEG; anything else as a lossless PNG. Opens a worker of its own and can wait for the larger limit, so it's for a job thread. `page` is from 0.
pub fn image_clip(path: &Path, page: u32, x: f32, y: f32, anyway: bool) -> ClipOutcome {
    let run = || -> Result<ClipOutcome, AgentError> {
        let mut doc = open_for_text(path)?;
        if !anyway && snapshot::copy_restricted(&mut doc, page)? {
            return Ok(ClipOutcome::Restricted);
        }
        let caps = OutputCaps::default();
        let list = image::list(&mut doc, path, page, &caps)?;
        let Some(hit) = list.at(x, y) else { return Ok(ClipOutcome::NoImage) };
        let mut copied: Option<CopiedImage> = None;
        let first = in_memory(|out| {
            copied = Some(image::copy_into(&mut doc, path, page, hit.number, Want::AsItIs, DEFAULT_SCALE, &caps, out)?);
            Ok(())
        });
        let mut clip = Clip::default();
        let mut left_off = Vec::new();
        let first = first?;
        let Some(copied) = copied else { return Ok(ClipOutcome::NoImage) };
        if copied.coded.is_jpeg() {
            clip.items.push(ClipItem { uti: UTI_JPEG, bytes: first });
            let png = in_memory(|out| image::copy_into(&mut doc, path, page, hit.number, Want::Png, DEFAULT_SCALE, &caps, out).map(drop));
            add_item(&mut clip, UTI_PNG, png, &mut left_off);
        } else {
            clip.items.push(ClipItem { uti: UTI_PNG, bytes: first });
        }
        clip.note = image_note(&copied, &left_off);
        Ok(ClipOutcome::Ready(clip))
    };
    run().unwrap_or_else(ClipOutcome::Failed)
}

fn image_note(copied: &CopiedImage, left_off: &[&str]) -> String {
    let what = match copied.coded {
        Coded::OriginalJpeg => "Copied the image as its own JPEG",
        Coded::JpegHeaderFixed => "Copied the image as its own JPEG, colours corrected",
        Coded::Png | Coded::PngWithMask => "Copied the image, lossless",
        Coded::PngDrawn => "Copied the image as it's drawn",
    };
    let mut note = format!("{what}, {} × {} pixels.", copied.width, copied.height);
    if !left_off.is_empty() {
        note.push_str(&format!(" Too large to copy as {} as well.", left_off.join(" or ")));
    }
    note
}

/// The app's snapshot of a box: PNG, text, SVG and JPG, all on the clipboard at once (SPEC section 6), each as far as it fits. `rect` is on the page as it's shown, from its top left. Opens a worker of its own and can wait for the larger limit, so it's for a job thread. `page` is from 0.
pub fn snapshot_clip(path: &Path, page: u32, rect: [f32; 4], anyway: bool) -> ClipOutcome {
    let run = || -> Result<ClipOutcome, AgentError> {
        crate::render::check_scale_and_box(DEFAULT_SCALE, Some(rect))?;
        let mut doc = open_for_text(path)?;
        let caps = OutputCaps::default();
        let text = snapshot::text(&mut doc, page, rect)?;
        if text.restricted && !anyway {
            return Ok(ClipOutcome::Restricted);
        }
        let mut clip = Clip::default();
        let mut left_off = Vec::new();
        // The picture is what the box is, so a failure there is the snapshot's: the rest are extras.
        let png = in_memory(|out| snapshot::picture_into(&mut doc, page, rect, DEFAULT_SCALE, Format::Png, &caps, out).map(drop))?;
        clip.items.push(ClipItem { uti: UTI_PNG, bytes: png });
        if !text.text.is_empty() {
            add_item(&mut clip, UTI_TEXT, Ok(text.text.clone().into_bytes()), &mut left_off);
        }
        let svg = in_memory(|out| crate::render::write_svg_box(&mut doc, path, page, rect, DEFAULT_SCALE, &caps, out, Instant::now()).map(drop));
        add_item(&mut clip, UTI_SVG, svg, &mut left_off);
        let jpg = in_memory(|out| snapshot::picture_into(&mut doc, page, rect, DEFAULT_SCALE, Format::Jpg, &caps, out).map(drop));
        add_item(&mut clip, UTI_JPEG, jpg, &mut left_off);
        clip.note = snapshot_note(&clip, &text, &left_off);
        Ok(ClipOutcome::Ready(clip))
    };
    run().unwrap_or_else(ClipOutcome::Failed)
}

fn snapshot_note(clip: &Clip, text: &recto_index::select::BoxText, left_off: &[&str]) -> String {
    let names: Vec<&str> = clip.items.iter().map(|item| match item.uti {
        UTI_PNG => "PNG",
        UTI_TEXT => "text",
        UTI_SVG => "SVG",
        _ => "JPG",
    }).collect();
    let listed = match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => (*last).to_string(),
        None => String::new(),
    };
    let mut note = format!("Copied the snapshot as {listed}.");
    if text.hidden_left_out > 0 {
        note.push_str(&format!(" {} hidden characters left out of the text.", text.hidden_left_out));
    }
    if !left_off.is_empty() {
        note.push_str(&format!(" Too large to copy as {} as well.", left_off.join(" or ")));
    }
    note
}
