//! **Taking things out of a page**: an image, as itself, and a snapshot of a box, as a picture, an SVG or text (SPEC section 6, Extraction). One place for what goes where: `recto images`, `recto image` and `recto snapshot` write files or answer here, and the app's snapshot tool asks the same functions for what goes on the clipboard, with the types and their order decided here.
//!
//! **A file that asks not to be copied** (owner, 2026-11-08: copying images and snapshots from a copy-restricted PDF gets the same notice as text, with "Copy anyway"): refused with exit 8 and `copy_restricted` unless `--anyway`, and the app's notice offers "Copy anyway". Each use of `--anyway` that gives something goes in the activity log.

use crate::render::image::{self, Coded, CopiedImage, Want};
use crate::render::snapshot::{self, Format};
use crate::render::{FileOut, LimitedOut, MemoryOut, OwnTemp, open_for_text, output_exists, publish_picture};
use crate::{AgentError, exit};
use recto_proto::contents::TextMode;
use recto_render_service::{CancelHandle, DocRenderer, OutputCaps};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The scale a picture is drawn at when nothing says: a snapshot, and an image that has to be drawn.
pub const DEFAULT_SCALE: f32 = 2.0;

/// The most one thing on the clipboard may be. Past it, that one is left off and the rest go: a snapshot's SVG of a map can be tens of MiB, and memory comes first.
pub const MAX_CLIP_ITEM: usize = 32 * 1024 * 1024;
/// The most all of one copy's clipboard may hold. Each item is made into a buffer capped at the room left, so nothing past this is ever made.
pub const MAX_CLIP_TOTAL: usize = 64 * 1024 * 1024;

/// The pasteboard type of each kind of thing, by its uniform type identifier, which is what a Mac's clipboard goes by.
pub const UTI_PNG: &str = "public.png";
pub const UTI_JPEG: &str = "public.jpeg";
pub const UTI_SVG: &str = "public.svg-image";
pub const UTI_TEXT: &str = "public.utf8-plain-text";

fn restricted() -> AgentError {
    AgentError::new("copy_restricted", "This file's permissions ask that it isn't copied.", exit::PROTECTED)
        .then("Tell the user. `--anyway` copies it, once they've said to. Each use goes in the activity log.")
}

/// **Exports of a file that asks not to be copied ask "anyway", as Copy does** (owner, 2026-11-08: "Yes, same step" for SVG export, PNG render and print to PDF). Exit 8 and `copy_restricted` unless `anyway`. Read from the file's own flag, with no page, so a refusal costs no worker. `page` is from 0.
pub(crate) fn refuse_if_restricted(path: &Path, page: u32, anyway: bool) -> Result<(), AgentError> {
    if !anyway && snapshot::copy_restricted(path, None, page) {
        return Err(restricted());
    }
    Ok(())
}

/// Whether a file asks not to be copied, for the app's own exports, on a job thread: a print to a PDF file asks "anyway" with the notice a copy has.
pub fn is_copy_restricted(path: &Path) -> bool {
    snapshot::copy_restricted(path, None, 0)
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
    let index = page
        .checked_sub(1)
        .ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))?;
    if number == 0 {
        return Err(bad_number());
    }
    crate::saving::check_export_path(out, &["png", "jpg", "jpeg"], "picture")?;
    let as_jpeg = out
        .extension()
        .is_some_and(|ending| ending.eq_ignore_ascii_case("jpg") || ending.eq_ignore_ascii_case("jpeg"));
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    let started = Instant::now();
    if !anyway && snapshot::copy_restricted(path, None, index) {
        return Err(restricted());
    }
    let mut doc = open_for_text(path)?;
    let caps = OutputCaps::default();
    let (temp, file) = OwnTemp::create(out)?;
    // Held to the bytes any export may write, as `render` is: an image's own bytes are as large as the file says.
    let written = RefCell::new(LimitedOut::new(FileOut(file), caps.max_bytes));
    let copied = image::copy_into(
        &mut doc,
        path,
        index,
        number,
        if as_jpeg { Want::JpegOnly } else { Want::Png },
        DEFAULT_SCALE,
        &caps,
        &written,
    )?;
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
    let page = asked
        .page
        .checked_sub(1)
        .ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))?;
    let format = format_of(asked.format)?;
    crate::render::check_scale_and_box(asked.scale, Some(asked.rect))?;
    let started = Instant::now();
    if format == Format::Text {
        if asked.out.is_some() {
            return Err(AgentError::new(
                "usage",
                "A snapshot as text comes back in the answer, as `copy` gives it, and writes no file.",
                exit::USAGE,
            )
            .then("Leave out `--out`, or choose `--as png`, `jpg` or `svg`."));
        }
        refuse_if_restricted(asked.path, page, asked.anyway)?;
        let mut doc = open_for_text(asked.path)?;
        let kept = snapshot::text(&mut doc, page, asked.rect)?;
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
    let out = asked
        .out
        .ok_or_else(|| AgentError::new("usage", format!("A snapshot as {} needs `--out`, a new file.", format.name()), exit::USAGE))?;
    crate::saving::check_export_path(out, format.endings(), if format == Format::Svg { "SVG" } else { "picture" })?;
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    refuse_if_restricted(asked.path, page, asked.anyway)?;
    let mut doc = open_for_text(asked.path)?;
    let (temp, file) = OwnTemp::create(out)?;
    let written = RefCell::new(FileOut(file));
    let mut data = json!({ "page": asked.page, "rect": asked.rect, "as": format.name(), "out": out, "anyway": asked.anyway });
    match format {
        Format::Svg => {
            let svg = crate::render::write_svg_box(
                &mut doc,
                asked.path,
                page,
                asked.rect,
                asked.scale,
                TextMode::Both,
                &OutputCaps::default(),
                &written,
                started,
                0,
            )?;
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

/// One thing on the clipboard: its type, and its bytes now, or what it may be when a paste asks for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipItem {
    /// Made with the copy.
    Ready { uti: &'static str, bytes: Vec<u8> },
    /// Offered by type, and made only when a paste asks for it, into at most `cap` bytes: a snapshot's SVG, which can take a worker of hundreds of MiB on a map, and which most pastes never want.
    Promised { uti: &'static str, cap: usize },
}

impl ClipItem {
    pub fn uti(&self) -> &'static str {
        match self {
            Self::Ready { uti, .. } | Self::Promised { uti, .. } => uti,
        }
    }

    /// The bytes made with the copy, or none for one made on a paste.
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Ready { bytes, .. } => Some(bytes),
            Self::Promised { .. } => None,
        }
    }

    fn len(&self) -> usize {
        self.bytes().map_or(0, <[u8]>::len)
    }
}

/// What a copy puts on the clipboard, in the order a Mac's clipboard should offer it, and what to say about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clip {
    pub items: Vec<ClipItem>,
    /// A quiet line for the reader: what went, and anything left off.
    pub note: String,
}

impl Clip {
    /// The types offered, in order, made or promised.
    pub fn types(&self) -> Vec<&'static str> {
        self.items.iter().map(ClipItem::uti).collect()
    }

    /// The room left for the next item, under both caps.
    fn room(&self) -> usize {
        MAX_CLIP_TOTAL.saturating_sub(self.items.iter().map(ClipItem::len).sum()).min(MAX_CLIP_ITEM)
    }
}

/// How asking for a clipboard came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipOutcome {
    Ready(Clip),
    /// The file asks not to be copied, and the reader hasn't said "Copy anyway".
    Restricted,
    /// There's no image where the click was.
    NoImage,
    /// A newer copy took its place, and this one stopped.
    Stopped,
    Failed(AgentError),
}

/// **Lets a newer copy stop this one**, from any thread: between the steps of a copy, and inside a drawing through the worker's own cancel word, so a stopped copy lets its worker go at once rather than finishing a map nobody wants.
#[derive(Clone, Debug, Default)]
pub struct ClipStop(Arc<StopInner>);

#[derive(Debug, Default)]
struct StopInner {
    stopped: AtomicBool,
    handle: Mutex<Option<CancelHandle>>,
}

impl ClipStop {
    /// Stops it, now or as soon as it looks. Cheap, for the UI thread: an atomic, a short lock and the worker's cancel word.
    pub fn stop(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
        if let Some(handle) = self.0.handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            // Every drawing here is generation 0, so this makes all of this worker's stale.
            handle.cancel_before(1);
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.0.stopped.load(Ordering::SeqCst)
    }

    /// The copy's worker is open: a stop from here on reaches its drawing too.
    fn attach(&self, doc: &DocRenderer) {
        let handle = doc.cancel_handle();
        *self.0.handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle.clone());
        if self.is_stopped() {
            handle.cancel_before(1);
        }
    }
}

fn in_memory(cap: usize, make: impl FnOnce(&RefCell<MemoryOut>) -> Result<(), AgentError>) -> Result<Vec<u8>, AgentError> {
    let out = RefCell::new(MemoryOut::new(cap));
    make(&out)?;
    Ok(out.into_inner().bytes)
}

fn name_of(uti: &str) -> &'static str {
    match uti {
        UTI_PNG => "PNG",
        UTI_JPEG => "JPG",
        UTI_SVG => "SVG",
        _ => "text",
    }
}

/// A JPEG's own bytes for the clipboard: up to the end of its picture, without whatever a file put after it, which no app pasting it wants and can be anything. A file written keeps every byte (owner's decision: an image goes as itself). Bytes that aren't a whole JPEG go as they are.
pub fn jpeg_for_clipboard(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut scan = recto_proto::jpeg::JpegScan::new(false);
    if scan.feed(&bytes, &mut Vec::new()).is_ok() && scan.finish().is_ok() {
        let trailing = usize::try_from(scan.facts().trailing_bytes).unwrap_or(0);
        bytes.truncate(bytes.len().saturating_sub(trailing));
    }
    bytes
}

/// The app's click on an image: what goes on the clipboard for the image at `x, y` on a page as it's shown. A JPEG goes as its own bytes, and as a PNG beside it for what can't take a JPEG; anything else as a lossless PNG. Opens a worker of its own and can wait for the larger limit, so it's for a job thread. `page` is from 0.
pub fn image_clip(path: &Path, page: u32, x: f32, y: f32, anyway: bool, stop: &ClipStop) -> ClipOutcome {
    let run = || -> Result<ClipOutcome, AgentError> {
        if !anyway && snapshot::copy_restricted(path, None, page) {
            return Ok(ClipOutcome::Restricted);
        }
        let mut doc = open_for_text(path)?;
        stop.attach(&doc);
        let caps = OutputCaps::default();
        let list = image::list(&mut doc, path, page, &caps)?;
        let Some(hit) = list.at(x, y) else { return Ok(ClipOutcome::NoImage) };
        if stop.is_stopped() {
            return Ok(ClipOutcome::Stopped);
        }
        let mut copied: Option<CopiedImage> = None;
        let mut clip = Clip::default();
        let first = in_memory(clip.room(), |out| {
            copied = Some(image::copy_into(&mut doc, path, page, hit.number, Want::AsItIs, DEFAULT_SCALE, &caps, out)?);
            Ok(())
        })?;
        let Some(copied) = copied else { return Ok(ClipOutcome::NoImage) };
        let mut left_off = Vec::new();
        if copied.coded.is_jpeg() {
            clip.items.push(ClipItem::Ready {
                uti: UTI_JPEG,
                bytes: jpeg_for_clipboard(first),
            });
            if stop.is_stopped() {
                return Ok(ClipOutcome::Stopped);
            }
            match in_memory(clip.room(), |out| {
                image::copy_into(&mut doc, path, page, hit.number, Want::Png, DEFAULT_SCALE, &caps, out).map(drop)
            }) {
                Ok(bytes) => clip.items.push(ClipItem::Ready { uti: UTI_PNG, bytes }),
                Err(_) => left_off.push("PNG"),
            }
        } else {
            clip.items.push(ClipItem::Ready { uti: UTI_PNG, bytes: first });
        }
        clip.note = image_note(&copied, &left_off);
        Ok(ClipOutcome::Ready(clip))
    };
    let outcome = run().unwrap_or_else(ClipOutcome::Failed);
    if stop.is_stopped() && !matches!(outcome, ClipOutcome::Ready(_)) {
        ClipOutcome::Stopped
    } else {
        outcome
    }
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

/// The app's snapshot of a box: PNG, text, SVG and JPG, all on the clipboard at once (SPEC section 6), each as far as it fits. The PNG and JPG come from one drawing; the SVG is promised, and made by [`snapshot_svg`] only if a paste asks for it. `rect` is on the page as the file shows it, from its top left. Opens a worker of its own and can wait for the larger limit, so it's for a job thread. `page` is from 0.
///
/// **The pictures are of the page as it's seen** (coordinator's decision, 2026-11-08): `quarter_turns` is how far the reader shows it turned, by an unsaved turn or the view's, and the PNG and JPG come out turned by as much, as the SVG does from [`snapshot_svg`]. The owner's rotate-all is for turning a sideways scan, and a snapshot of it mustn't come out sideways again.
pub fn snapshot_clip(path: &Path, page: u32, rect: [f32; 4], quarter_turns: u8, anyway: bool, stop: &ClipStop) -> ClipOutcome {
    let run = || -> Result<ClipOutcome, AgentError> {
        crate::render::check_scale_and_box(DEFAULT_SCALE, Some(rect))?;
        if !anyway && snapshot::copy_restricted(path, None, page) {
            return Ok(ClipOutcome::Restricted);
        }
        let mut doc = open_for_text(path)?;
        stop.attach(&doc);
        let caps = OutputCaps::default();
        // Text is an extra: a page whose characters can't be read still gives its picture.
        let text = snapshot::text(&mut doc, page, rect).ok();
        if stop.is_stopped() {
            return Ok(ClipOutcome::Stopped);
        }
        let mut clip = Clip::default();
        let mut left_off = Vec::new();
        let words = text.as_ref().map(|text| text.text.as_bytes()).filter(|words| !words.is_empty());
        // Room for the text first, so the pictures are held to what's left with it on the clipboard.
        let room = MAX_CLIP_TOTAL.saturating_sub(words.map_or(0, <[u8]>::len)).min(MAX_CLIP_ITEM);
        let (png, jpg) = (RefCell::new(MemoryOut::new(room)), RefCell::new(MemoryOut::new(room)));
        // The picture is what the box is, so a failure there is the snapshot's: the rest are extras.
        let (_, jpg_came) = snapshot::pictures_into(&mut doc, page, rect, DEFAULT_SCALE, Format::Png, &caps, &png, Some(&jpg), 0, quarter_turns)?;
        clip.items.push(ClipItem::Ready {
            uti: UTI_PNG,
            bytes: png.into_inner().bytes,
        });
        match words {
            Some(words) if words.len() <= clip.room() => clip.items.push(ClipItem::Ready {
                uti: UTI_TEXT,
                bytes: words.to_vec(),
            }),
            Some(_) => left_off.push("text"),
            None => {}
        }
        clip.items.push(ClipItem::Promised {
            uti: UTI_SVG,
            cap: MAX_CLIP_ITEM,
        });
        let jpg = jpg.into_inner().bytes;
        if jpg_came && jpg.len() <= clip.room() {
            clip.items.push(ClipItem::Ready { uti: UTI_JPEG, bytes: jpg });
        } else {
            left_off.push("JPG");
        }
        clip.note = snapshot_note(&clip, text.as_ref(), &left_off);
        Ok(ClipOutcome::Ready(clip))
    };
    let outcome = run().unwrap_or_else(ClipOutcome::Failed);
    if stop.is_stopped() && !matches!(outcome, ClipOutcome::Ready(_)) {
        ClipOutcome::Stopped
    } else {
        outcome
    }
}

/// **A snapshot's SVG, made when a paste asks for it**: `export svg --rect`'s drawing with text as outlines only, so hidden text stays out as it does from the text item, into at most `cap` bytes, turned by `quarter_turns` as [`snapshot_clip`]'s pictures are. Whether the file may be copied was settled when the snapshot was taken. Opens a worker of its own and waits for it, for as long as the paste is waiting. `page` is from 0.
pub fn snapshot_svg(path: &Path, page: u32, rect: [f32; 4], quarter_turns: u8, cap: usize) -> Result<Vec<u8>, AgentError> {
    crate::render::check_scale_and_box(DEFAULT_SCALE, Some(rect))?;
    let mut doc = open_for_text(path)?;
    in_memory(cap.min(MAX_CLIP_ITEM), |out| {
        crate::render::write_svg_box(
            &mut doc,
            path,
            page,
            rect,
            DEFAULT_SCALE,
            TextMode::Paths,
            &OutputCaps::default(),
            out,
            Instant::now(),
            quarter_turns,
        )
        .map(drop)
    })
}

fn snapshot_note(clip: &Clip, text: Option<&recto_index::select::BoxText>, left_off: &[&str]) -> String {
    let names: Vec<&str> = clip.items.iter().map(|item| name_of(item.uti())).collect();
    let listed = match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => (*last).to_string(),
        None => String::new(),
    };
    let mut note = format!("Copied the snapshot as {listed}.");
    if let Some(text) = text.filter(|text| text.hidden_left_out > 0) {
        note.push_str(&format!(" {} hidden characters left out of the text.", text.hidden_left_out));
    }
    if !left_off.is_empty() {
        note.push_str(&format!(" Too large to copy as {} as well.", left_off.join(" or ")));
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a JPEG on the clipboard carries what a file put after its end, which can be anything and no app pasting it wants; or a JPEG that isn't whole loses bytes it needs.
    #[test]
    fn a_jpeg_for_the_clipboard_ends_where_its_picture_does() {
        // Start, a frame of one pixel, a scan and its data, and the end.
        let whole = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F,
            0x00, 0x12, 0x34, 0xFF, 0xD9,
        ];
        let mut with_more = whole.to_vec();
        with_more.extend_from_slice(b"anything at all");
        assert_eq!(jpeg_for_clipboard(with_more), whole);
        assert_eq!(jpeg_for_clipboard(whole.to_vec()), whole);
        let cut = whole[..20].to_vec();
        assert_eq!(jpeg_for_clipboard(cut.clone()), cut, "not whole, so as it is");
    }

    /// Failure: one copy's clipboard grows past its cap, because each item was made whole and only then measured; the room left is what the next item may use.
    #[test]
    fn each_item_is_held_to_the_room_left() {
        let mut clip = Clip::default();
        assert_eq!(clip.room(), MAX_CLIP_ITEM);
        clip.items.push(ClipItem::Ready {
            uti: UTI_PNG,
            bytes: vec![0; MAX_CLIP_ITEM - 10],
        });
        clip.items.push(ClipItem::Ready {
            uti: UTI_JPEG,
            bytes: vec![0; MAX_CLIP_ITEM - 10],
        });
        assert_eq!(clip.room(), 20);
        clip.items.push(ClipItem::Promised { uti: UTI_SVG, cap: 1 });
        assert_eq!(clip.room(), 20, "a promise takes no room until it's made");
        let over = in_memory(clip.room(), |out| {
            std::io::Write::write_all(&mut *out.borrow_mut(), &[0; 21]).map_err(|_| AgentError::new("output_too_large", "", exit::FAILED))
        });
        assert_eq!(over.unwrap_err().code, "output_too_large");
    }

    /// Failure: a stop that comes before the worker is open is lost, so the stopped copy goes on drawing; or one after it doesn't reach the drawing.
    #[test]
    fn a_stop_is_kept_until_the_copy_looks() {
        let stop = ClipStop::default();
        assert!(!stop.is_stopped());
        let other = stop.clone();
        other.stop();
        assert!(stop.is_stopped(), "the same stop, from another thread");
    }
}
