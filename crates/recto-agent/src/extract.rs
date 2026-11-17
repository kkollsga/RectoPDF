//! **Taking things out of a page**: an image, as itself, and a snapshot of a box, as a picture, an SVG or text (SPEC section 6, Extraction). One place for what goes where: `recto images`, `recto image` and `recto snapshot` write files or answer here, and the app's snapshot tool asks the same functions for what goes on the clipboard, with the types and their order decided here.
//!
//! **A file that asks not to be copied** (owner, 2026-11-08: copying images and snapshots from a copy-restricted PDF gets the same notice as text, with "Copy anyway"): refused with exit 8 and `copy_restricted` unless `--anyway`, and the app's notice offers "Copy anyway". Each use of `--anyway` that gives something goes in the activity log.

use crate::Code;
use crate::copying::{Copying, asks_anyway, refuse_unless_anyway};
use crate::render::image::{self, Coded, CopiedImage, Want};
use crate::render::snapshot::{self, Format};
use crate::render::{FileOut, LimitedOut, MemoryOut, OwnTemp, open_for_text, output_exists, publish_picture};
use crate::{AgentError, exit};
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
/// The file's own highlights a tab has taken out, for a drawing of one of its pages: see [`crate::hidden_in`].
pub type Hidden = Option<std::sync::Arc<recto_proto::hidden::HiddenAnnots>>;

pub const UTI_PNG: &str = "public.png";
pub const UTI_JPEG: &str = "public.jpeg";
pub const UTI_SVG: &str = "public.svg-image";
pub const UTI_TEXT: &str = "public.utf8-plain-text";

/// **Exports of a file that asks not to be copied ask "anyway", as Copy does** (owner, 2026-11-08: "Yes, same step" for SVG export, PNG render and print to PDF). Exit 8 and `copy_restricted` unless `anyway`. Read from the file's own flag, with no page, so a refusal costs no worker. `page` is from 0.
pub(crate) fn refuse_if_restricted(path: &Path, page: u32, anyway: bool) -> Result<(), AgentError> {
    refuse_unless_anyway(anyway, || snapshot::copy_restricted(path, None, page), Copying::File)
}

/// Whether a file asks not to be copied, for the app's own exports, on a job thread: a print to a PDF file asks "anyway" with the notice a copy has.
pub fn is_copy_restricted(path: &Path) -> bool {
    snapshot::copy_restricted(path, None, 0)
}

fn bad_number() -> AgentError {
    AgentError::new(Code::USAGE, "An image is numbered from 1, as `recto images` lists them.", exit::USAGE)
}

/// `recto images`: where a page's images are, or with `at` the one on top there, as [`image_clip`] finds it. Its page is opened as `whose` says: with the
/// person's password for their own document, as the snapshot tool's click opens it. **`stop` ends it** (the person's look for a click's image, which a
/// newer click, a copy, a let-go or a quit stops): before its worker is opened, nothing is; after, the worker's cancel word reaches the listing. Either way
/// it's refused as `cancelled`.
pub(crate) fn images(whose: &ClipPage<'_>, at: Option<[f32; 2]>, stop: &ClipStop) -> Result<Value, AgentError> {
    let stopped = || AgentError::new(Code::CANCELLED, "A newer click took its place.", exit::FAILED);
    if stop.is_stopped() {
        return Err(stopped());
    }
    let mut doc = whose.whose.open(whose.path)?;
    stop.attach(&doc);
    let listed = image::list_command(&mut doc, whose.path, whose.file_page + 1, at);
    if stop.is_stopped() {
        return Err(stopped());
    }
    listed
}

/// `recto image`: one of a page's images to a new file, as itself. `.jpg` takes a JPEG's own bytes and refuses anything else, as that would be re-encoded; `.png` takes any image, losslessly.
pub(crate) fn image_to_file(path: &Path, page: u32, number: u32, out: &Path, anyway: bool) -> Result<Value, AgentError> {
    let index = page
        .checked_sub(1)
        .ok_or_else(|| AgentError::new(Code::NO_SUCH_PAGE, "Pages are numbered from 1.", exit::NOT_FOUND))?;
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
    refuse_unless_anyway(anyway, || snapshot::copy_restricted(path, None, index), Copying::File)?;
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

mod drawn;
pub(crate) use drawn::{Destination, Drawn, drawn_for};

/// What `recto snapshot` was asked.
pub(crate) struct SnapshotAsked<'a> {
    pub path: &'a Path,
    pub page: u32,
    pub rect: [f32; 4],
    pub format: &'a str,
    pub out: Option<&'a Path>,
    pub scale: f32,
    pub anyway: bool,
    /// The file's own highlights the tab has taken out, left out of a picture (R4.3).
    pub hidden: crate::render::Hidden,
    /// The page's unsaved turn, which [`drawn_for`] says what a file or the answer makes of.
    pub unsaved_quarter_turns: u8,
}

fn format_of(name: &str) -> Result<Format, AgentError> {
    Format::from_name(name).ok_or_else(|| AgentError::new(Code::USAGE, "A snapshot is `png`, `jpg`, `svg` or `text`.", exit::USAGE))
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
        .ok_or_else(|| AgentError::new(Code::NO_SUCH_PAGE, "Pages are numbered from 1.", exit::NOT_FOUND))?;
    let format = format_of(asked.format)?;
    crate::render::check_scale_and_box(asked.scale, Some(asked.rect))?;
    let started = Instant::now();
    if format == Format::Text {
        if asked.out.is_some() {
            return Err(AgentError::new(
                Code::USAGE,
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
        .ok_or_else(|| AgentError::new(Code::USAGE, format!("A snapshot as {} needs `--out`, a new file.", format.name()), exit::USAGE))?;
    crate::saving::check_export_path(out, format.endings(), if format == Format::Svg { "SVG" } else { "picture" })?;
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(output_exists());
    }
    refuse_if_restricted(asked.path, page, asked.anyway)?;
    let mut doc = open_for_text(asked.path)?;
    doc.hidden.draw_for(0, asked.hidden.clone());
    let (temp, file) = OwnTemp::create(out)?;
    let written = RefCell::new(FileOut(file));
    let mut data = json!({ "page": asked.page, "rect": asked.rect, "as": format.name(), "out": out, "anyway": asked.anyway });
    let drawn = drawn_for(Destination::FileOrAnswer, asked.unsaved_quarter_turns);
    match format {
        Format::Svg => {
            let svg = crate::render::write_svg_box(
                &mut doc,
                asked.path,
                page,
                asked.rect,
                asked.scale,
                drawn.text,
                &OutputCaps::default(),
                &written,
                started,
                drawn.quarter_turns,
            )?;
            data["all_vector"] = json!(svg.all_vector());
            data["raster_scale"] = json!(svg.raster_scale);
        }
        _ => {
            let ((width, height), _) = snapshot::pictures_into(
                &mut doc,
                page,
                asked.rect,
                asked.scale,
                format,
                &OutputCaps::default(),
                &written,
                None,
                0,
                drawn.quarter_turns,
            )?;
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
    image_clip_for(path, page, (x, y), (anyway, PersonsPage::default()), stop)
}

/// [`image_clip`], for the person's own page if `whose` says it's theirs: opened, and its flag read, with their password by the print's rule.
fn image_clip_for(path: &Path, page: u32, (x, y): (f32, f32), (anyway, whose): (bool, PersonsPage<'_>), stop: &ClipStop) -> ClipOutcome {
    let run = || -> Result<ClipOutcome, AgentError> {
        if asks_anyway(anyway, || whose.copy_restricted(path, page)) {
            return Ok(ClipOutcome::Restricted);
        }
        let mut doc = whose.open(path)?;
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

/// The app's snapshot of a box: PNG, text, SVG and JPG, prepared before they are offered (SPEC section 6), each as far as it fits. PNG and JPG share one drawing, and SVG uses the same worker afterward, so a synchronous paste never owes rendering. `rect` is on the page as the file shows it, from its top left. Opens a worker of its own and can wait for the larger limit, so it's for a job thread. `page` is from 0.
///
/// **The pictures are of the page as it's seen** (coordinator's decision, 2026-11-08): `unsaved_quarter_turns` is how far the page is turned by an unsaved turn, and the PNG and JPG come out as [`drawn_for`] says the clipboard has it, turned by as much, as the SVG does from [`snapshot_svg`]. The owner's rotate-all is for turning a sideways scan, and a snapshot of it mustn't come out sideways again.
///
/// `hidden`: the file's own highlights the tab has taken out, left out of every picture as the page view leaves them out ([`crate::hidden_in`]).
pub fn snapshot_clip(path: &Path, page: u32, rect: [f32; 4], (unsaved_quarter_turns, hidden): (u8, Hidden), anyway: bool, stop: &ClipStop) -> ClipOutcome {
    let drawn = drawn_for(Destination::Clipboard, unsaved_quarter_turns);
    snapshot_clip_for(path, page, rect, (drawn, hidden), (anyway, PersonsPage::default()), stop)
}

/// [`snapshot_clip`], for the person's own page if `whose` says it's theirs: opened, and its flag read, with their password by the print's rule.
fn snapshot_clip_for(
    path: &Path,
    page: u32,
    rect: [f32; 4],
    (drawn, hidden): (Drawn, Hidden),
    (anyway, whose): (bool, PersonsPage<'_>),
    stop: &ClipStop,
) -> ClipOutcome {
    let run = || -> Result<ClipOutcome, AgentError> {
        crate::render::check_scale_and_box(DEFAULT_SCALE, Some(rect))?;
        if asks_anyway(anyway, || whose.copy_restricted(path, page)) {
            return Ok(ClipOutcome::Restricted);
        }
        let mut doc = whose.open(path)?;
        doc.hidden.draw_for(0, hidden);
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
        let (_, jpg_came) = snapshot::pictures_into(
            &mut doc,
            page,
            rect,
            DEFAULT_SCALE,
            Format::Png,
            &caps,
            &png,
            Some(&jpg),
            0,
            drawn.quarter_turns,
        )?;
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
        let svg_at = clip.items.len();
        let jpg = jpg.into_inner().bytes;
        if jpg_came && jpg.len() <= clip.room() {
            clip.items.push(ClipItem::Ready { uti: UTI_JPEG, bytes: jpg });
        } else {
            left_off.push("JPG");
        }
        if stop.is_stopped() {
            return Ok(ClipOutcome::Stopped);
        }
        // Reserve the already made types first. The SVG writer is capped before it allocates its output, including the room the JPG takes.
        let svg = in_memory(clip.room(), |out| {
            crate::render::write_svg_box(
                &mut doc,
                path,
                page,
                rect,
                DEFAULT_SCALE,
                drawn.text,
                &caps,
                out,
                Instant::now(),
                drawn.quarter_turns,
            )
            .map(drop)
        });
        if stop.is_stopped() {
            return Ok(ClipOutcome::Stopped);
        }
        let mut svg_unavailable = false;
        match svg {
            Ok(bytes) => clip.items.insert(svg_at, ClipItem::Ready { uti: UTI_SVG, bytes }),
            Err(error) if error.code == Code::OUTPUT_TOO_LARGE => left_off.push("SVG"),
            Err(_) => svg_unavailable = true,
        }
        clip.note = snapshot_note(&clip, text.as_ref(), &left_off);
        if svg_unavailable {
            clip.note.push_str(" SVG couldn't be copied.");
        }
        Ok(ClipOutcome::Ready(clip))
    };
    let outcome = run().unwrap_or_else(ClipOutcome::Failed);
    if stop.is_stopped() && !matches!(outcome, ClipOutcome::Ready(_)) {
        ClipOutcome::Stopped
    } else {
        outcome
    }
}

/// **A snapshot's SVG, made when a paste asks for it**: `export svg --rect`'s drawing as [`drawn_for`] says the clipboard has it (text as outlines only, so hidden text stays out as it does from the text item), into at most `cap` bytes, turned by `unsaved_quarter_turns` as [`snapshot_clip`]'s pictures are. Whether the file may be copied was settled when the snapshot was taken. Opens a worker of its own and waits for it, for as long as the paste is waiting. `page` is from 0.
/// `hidden` as for [`snapshot_clip`].
pub fn snapshot_svg(path: &Path, page: u32, rect: [f32; 4], unsaved_quarter_turns: u8, hidden: Hidden, cap: usize) -> Result<Vec<u8>, AgentError> {
    let drawn = drawn_for(Destination::Clipboard, unsaved_quarter_turns);
    snapshot_svg_for(path, page, rect, (drawn, hidden), (cap, PersonsPage::default()), &ClipStop::default())
}

/// [`snapshot_svg`], for the person's own page if `whose` says it's theirs, and stopped by `stop`: a paste waits for it only so long.
fn snapshot_svg_for(
    path: &Path,
    page: u32,
    rect: [f32; 4],
    (drawn, hidden): (Drawn, Hidden),
    (cap, whose): (usize, PersonsPage<'_>),
    stop: &ClipStop,
) -> Result<Vec<u8>, AgentError> {
    crate::render::check_scale_and_box(DEFAULT_SCALE, Some(rect))?;
    let mut doc = whose.open(path)?;
    doc.hidden.draw_for(0, hidden);
    stop.attach(&doc);
    in_memory(cap.min(MAX_CLIP_ITEM), |out| {
        crate::render::write_svg_box(
            &mut doc,
            path,
            page,
            rect,
            DEFAULT_SCALE,
            drawn.text,
            &OutputCaps::default(),
            out,
            Instant::now(),
            drawn.quarter_turns,
        )
        .map(drop)
    })
}

/// **Whose page a clipboard snapshot reads** (PLAN, the person's snapshot of a document opened with its password): the person's own snapshot of a page of
/// their tab's own file is opened, and its copy flag read, with the password they typed for that very file, by the print's rule ([`crate::print::key_for`],
/// [`crate::print::copy_restricted`]); anything else as any export is, with none.
#[derive(Clone, Copy, Default)]
pub(crate) struct PersonsPage<'a> {
    pub keys: Option<&'a crate::keys::PersonsKeys>,
    /// The page is of the tab's own file, as the page view reads with the password.
    pub own: bool,
}

impl PersonsPage<'_> {
    fn as_printed(&self, path: &Path, page: u32) -> crate::print::PrintPage {
        crate::print::PrintPage {
            path: path.to_path_buf(),
            page,
            quarter_turns: 0,
            hidden: None,
            own: self.own,
        }
    }

    fn copy_restricted(&self, path: &Path, page: u32) -> bool {
        match self.keys {
            Some(keys) => crate::print::copy_restricted(&self.as_printed(path, page), Some(keys)),
            None => snapshot::copy_restricted(path, None, page),
        }
    }

    fn open(&self, path: &Path) -> Result<DocRenderer, AgentError> {
        let Some(keys) = self.keys else { return open_for_text(path) };
        let file = DocRenderer::open_file(path)?;
        let key = crate::print::key_for(&file, &self.as_printed(path, 0), Some(keys));
        Ok(DocRenderer::with_file(
            file,
            path,
            key,
            recto_render_service::ServiceConfig::discover()?,
            recto_render_service::Policy::default(),
        )?)
    }
}

/// **The page as the person's snapshot saw it** (coordinator, 2026-11-14: the clipboard's types are one copy of one moment): carried by the box and the
/// click, as the view showed the page they were taken from, and by the paste of a box's promised SVG, as the box's answer said it. Each is refused if
/// another page is in its place by the time it runs (the branch's review, S1: the box and click wait their turn in the person's lane, behind a page
/// delete or move), and the SVG is drawn turned as the PNG was. Clipboard-only, as `to_clipboard`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AsSeen {
    /// The page's unsaved turn, as it was seen: what its drawing goes by ([`drawn_for`]).
    pub quarter_turns: u8,
    /// From 0, in its file.
    pub file_page: u32,
    /// From the tab's own file, rather than a PDF added to it.
    pub own: bool,
}

impl AsSeen {
    /// Whether `now` is still this page of the same file: refused as moved if not. Its turn isn't the page's: a turn asked before the box was let go
    /// is in what's copied (coordinator, 2026-11-14).
    pub(crate) fn still(self, now: &ClipPage<'_>) -> Result<(), AgentError> {
        if (self.file_page, self.own) == (now.file_page, now.whose.own) {
            Ok(())
        } else {
            Err(crate::clipboard::page_moved())
        }
    }
}

/// **A page for the person's clipboard:** the page in its file, as the tab has it, and what the snapshot is told of the tab.
pub(crate) struct ClipPage<'a> {
    pub path: &'a Path,
    /// From 0, in that file.
    pub file_page: u32,
    pub unsaved_quarter_turns: u8,
    pub hidden: Hidden,
    pub whose: PersonsPage<'a>,
}

/// What a snapshot to the clipboard comes to, as a command's answer or refusal: what's offered is given to the window's [`crate::clipboard::ClipboardSink`]
/// first, and the answer says what went. The refusals keep the codes an agent's export has: `copy_restricted` (exit 8), `no_such_image`, and `cancelled`
/// for one a newer one stopped.
fn offered(sink: &dyn crate::clipboard::ClipboardSink, outcome: ClipOutcome, mut answer: Value) -> Result<Value, AgentError> {
    match outcome {
        ClipOutcome::Ready(clip) => {
            answer["says"] = json!(clip.note);
            answer["types"] = json!(clip.types());
            sink.offer(clip);
            Ok(answer)
        }
        // Refused as any export of such a file is, in the same words.
        ClipOutcome::Restricted => refuse_unless_anyway(false, || true, Copying::File).and(Ok(answer)),
        ClipOutcome::NoImage => Err(AgentError::new(Code::NO_SUCH_IMAGE, "There's no image there.", exit::NOT_FOUND)),
        ClipOutcome::Stopped => Err(AgentError::new(Code::CANCELLED, "A newer snapshot took its place.", exit::FAILED)),
        ClipOutcome::Failed(error) => Err(error),
    }
}

/// **The person's box, to the window's clipboard** (`snapshot --to-clipboard`): PNG, text, the SVG promised and JPG at once, as [`snapshot_clip`] makes
/// them, drawn as the clipboard has a snapshot ([`drawn_for`]). `as svg` is the promised SVG, made for the paste waiting for it, which gets its bytes.
pub(crate) fn box_to_clipboard(
    sink: &dyn crate::clipboard::ClipboardSink,
    page: ClipPage<'_>,
    rect: [f32; 4],
    format: &str,
    (anyway, as_seen): (bool, AsSeen),
) -> Result<Value, AgentError> {
    let format = format_of(format)?;
    // Only the page it was taken from, which a move or a deletion since may have put elsewhere.
    as_seen.still(&page)?;
    let seen = AsSeen {
        quarter_turns: page.unsaved_quarter_turns,
        file_page: page.file_page,
        own: page.whose.own,
    };
    let answer = json!({ "rect": rect, "as": "clipboard", "anyway": anyway, "as_seen": seen });
    if format == Format::Svg {
        // As the box saw it: its turn.
        let stop = sink.begin_paste();
        let drawn = drawn_for(Destination::Clipboard, as_seen.quarter_turns);
        let svg = snapshot_svg_for(page.path, page.file_page, rect, (drawn, page.hidden), (MAX_CLIP_ITEM, page.whose), &stop)?;
        let bytes = svg.len();
        sink.made(svg);
        return Ok(json!({ "rect": rect, "as": "svg", "bytes": bytes }));
    }
    let stop = sink.begin();
    let drawn = drawn_for(Destination::Clipboard, page.unsaved_quarter_turns);
    let outcome = snapshot_clip_for(page.path, page.file_page, rect, (drawn, page.hidden), (anyway, page.whose), &stop);
    offered(sink, outcome, answer)
}

/// **The person's click on an image, to the window's clipboard** (`image --at x,y --to-clipboard`): the image there as itself, as [`image_clip`] takes it.
pub(crate) fn image_to_clipboard(
    sink: &dyn crate::clipboard::ClipboardSink,
    page: ClipPage<'_>,
    at: [f32; 2],
    (anyway, as_seen): (bool, AsSeen),
) -> Result<Value, AgentError> {
    as_seen.still(&page)?;
    let answer = json!({ "at": at, "as": "clipboard", "anyway": anyway });
    let stop = sink.begin();
    let outcome = image_clip_for(page.path, page.file_page, (at[0], at[1]), (anyway, page.whose), &stop);
    offered(sink, outcome, answer)
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
    use recto_proto::contents::TextMode;

    /// Failure: a copied box advertises SVG before its bytes exist, so the system's synchronous clipboard request has to render while the UI waits.
    #[test]
    fn a_snapshot_has_its_svg_before_the_clipboard_can_ask() {
        let file = recto_testpdf::TempPdf::new(
            "ready-svg.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Copied before a paste"]))]),
        );
        let ClipOutcome::Ready(clip) = snapshot_clip(&file.0, 0, [20.0, 20.0, 300.0, 120.0], (0, None), false, &ClipStop::default()) else {
            panic!("the box must copy");
        };
        let svg = clip.items.iter().find(|item| item.uti() == UTI_SVG).and_then(ClipItem::bytes);
        assert!(
            svg.is_some_and(|bytes| String::from_utf8_lossy(bytes).contains("<svg")),
            "SVG must already be made"
        );
        assert!(
            clip.items.iter().all(|item| item.bytes().is_some()),
            "no rendering can be owed to a synchronous paste"
        );
        assert!(
            clip.items.iter().map(ClipItem::len).sum::<usize>() <= MAX_CLIP_TOTAL,
            "prepared types share the copy's byte cap"
        );
    }

    /// A window's clipboard that keeps what reaches it.
    #[derive(Default)]
    struct Kept {
        offered: Mutex<Option<Clip>>,
        made: Mutex<Option<Vec<u8>>>,
    }

    impl crate::clipboard::ClipboardSink for Kept {
        fn begin(&self) -> ClipStop {
            ClipStop::default()
        }
        fn begin_paste(&self) -> ClipStop {
            ClipStop::default()
        }
        fn offer(&self, clip: Clip) {
            *self.offered.lock().unwrap() = Some(clip);
        }
        fn made(&self, bytes: Vec<u8>) {
            *self.made.lock().unwrap() = Some(bytes);
        }
    }

    /// One box of `file`'s page 1, drawn to each destination: a PNG and an SVG to a new file, the person's box to the clipboard (its PNG) and the paste
    /// of its SVG. `round` names the files.
    fn drawings(file: &recto_testpdf::TempPdf, round: &str) -> Vec<(&'static str, Vec<u8>)> {
        let rect = [20.0, 20.0, 300.0, 120.0];
        let to_file = |format: &str| {
            let out = file.folder().join(format!("{round}.{format}"));
            snapshot_command(&SnapshotAsked {
                path: &file.0,
                page: 1,
                rect,
                format,
                out: Some(&out),
                scale: DEFAULT_SCALE,
                anyway: false,
                hidden: None,
                unsaved_quarter_turns: 0,
            })
            .expect("written");
            std::fs::read(&out).expect("read back")
        };
        let kept = Kept::default();
        let page = || ClipPage {
            path: &file.0,
            file_page: 0,
            unsaved_quarter_turns: 0,
            hidden: None,
            whose: PersonsPage { keys: None, own: true },
        };
        let seen = AsSeen {
            quarter_turns: 0,
            file_page: 0,
            own: true,
        };
        box_to_clipboard(&kept, page(), rect, "png", (false, seen)).expect("the box");
        box_to_clipboard(&kept, page(), rect, "svg", (false, seen)).expect("the paste");
        let png = kept
            .offered
            .lock()
            .unwrap()
            .take()
            .and_then(|clip| clip.items.first().and_then(ClipItem::bytes).map(<[u8]>::to_vec));
        vec![
            ("a PNG to a file", to_file("png")),
            ("an SVG to a file", to_file("svg")),
            ("the box's PNG to the clipboard", png.expect("a PNG offered")),
            ("its paste's SVG", kept.made.lock().unwrap().take().expect("an SVG made")),
        ]
    }

    /// **Failure (the branch's review, F2): a drawing of a snapshot decides its turn or its SVG's text somewhere other than [`drawn_for`],** so changing
    /// the rule there leaves that drawing as it was (as a file's SVG did, which asked it for its text and drew no turn, and a file's picture, which never
    /// asked). Another answer seeded for one destination changes that destination's drawings and only them.
    #[test]
    fn every_drawing_of_a_snapshot_goes_by_drawn_for() {
        let file = recto_testpdf::TempPdf::new(
            "drawn.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Words drawn for each destination"]))]),
        );
        let changed = |round: &str, seed: (Destination, Drawn), was: &[(&'static str, Vec<u8>)]| {
            drawn::seed(Some(seed));
            let now = drawings(&file, round);
            drawn::seed(None);
            now.iter()
                .zip(was)
                .filter(|(now, was)| now.1 != was.1)
                .map(|(now, _)| now.0)
                .collect::<Vec<_>>()
        };
        let was = drawings(&file, "as-it-is");
        let to_files = Drawn {
            quarter_turns: 1,
            text: TextMode::Paths,
        };
        assert_eq!(
            changed("files-seeded", (Destination::FileOrAnswer, to_files), &was),
            ["a PNG to a file", "an SVG to a file"]
        );
        let to_the_clipboard = Drawn {
            quarter_turns: 1,
            text: TextMode::Both,
        };
        assert_eq!(
            changed("clipboard-seeded", (Destination::Clipboard, to_the_clipboard), &was),
            ["the box's PNG to the clipboard", "its paste's SVG"]
        );
    }

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
            std::io::Write::write_all(&mut *out.borrow_mut(), &[0; 21]).map_err(|_| AgentError::new(Code::OUTPUT_TOO_LARGE, "", exit::FAILED))
        });
        assert_eq!(over.unwrap_err().code, "output_too_large");
    }

    /// Failure: a stop that comes before the worker is open is lost, so the stopped copy goes on drawing; or one after it doesn't reach the drawing.
    #[test]
    fn a_stop_is_kept_until_the_copy_looks() {
        let file = recto_testpdf::TempPdf::new(
            "stop.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Words"]))]),
        );
        // Stopped before its worker is open: the drawing it then starts is already stale.
        let early = ClipStop::default();
        early.clone().stop();
        let doc = open_for_text(&file.0).unwrap();
        early.attach(&doc);
        assert!(doc.is_stale(0), "a stop before the worker was open was lost");
        // Stopped after, from another clone: it reaches the drawing.
        let late = ClipStop::default();
        let doc = open_for_text(&file.0).unwrap();
        late.attach(&doc);
        assert!(!doc.is_stale(0) && !late.is_stopped());
        late.clone().stop();
        assert!(
            late.is_stopped() && doc.is_stale(0),
            "a stop after the worker was open didn't reach its drawing"
        );
    }
}
