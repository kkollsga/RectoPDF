//! Pages and pictures written out: `render`, `export svg`, `images`, `image`, `snapshot` and `print`.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

/// Draws a page to a PNG file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Render {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
    /// A number, or `"max"` for the largest that fits, which reads as [`render::LARGEST_THAT_FITS`].
    #[serde(default = "crate::render::default_scale", deserialize_with = "crate::render::scale_or_max")]
    pub scale: f32,
    pub out: PathBuf,
    /// Draws a page of a file that asks not to be copied. Only once the user has said to; logged.
    #[serde(default)]
    pub anyway: bool,
}

impl Cmd for Render {
    const TAG: &'static str = "render";
    const ROWS: &'static [Spec] = ROWS_RENDER;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, page, out, anyway, .. } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "anyway": anyway, "untrusted": super::log_file(out) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, page, scale, out, anyway } = self;
        let (access, events) = (cx.access, &mut *cx.events);
        {
            require_absolute(out)?;
            let (id, position, source) = locate(access, tab, *page)?;
            extract::refuse_if_restricted(&source.path, source.page, *anyway)?;
            let drawn = render::render(&source.path, source.page + 1, *scale, out);
            if !is_about_the_output(drawn.as_ref().err().map(|error| error.code)) {
                note_draw(access, id, position, drawn.as_ref().err().map(|error| error.code), events);
            }
            let mut data = drawn?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
    }
    fn human(data: &Value) -> String {
        render_result(data)
    }
}

/// Writes a page, or a box on it, to a new SVG file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportSvg {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
    pub out: PathBuf,
    /// `x, y, width, height`, in points from the page's top left.
    #[serde(default)]
    pub rect: Option<[f32; 4]>,
    /// `both`, `paths` or `selectable`.
    #[serde(default)]
    pub text: Option<String>,
    /// The scale of anything that has to go as a picture.
    #[serde(default = "crate::render::default_scale")]
    pub scale: f32,
    /// Takes out of JPEGs what isn't the picture, and the document's title.
    #[serde(default)]
    pub strip_metadata: bool,
    /// Exports a page of a file that asks not to be copied. Only once the user has said to; logged.
    #[serde(default)]
    pub anyway: bool,
}

impl Cmd for ExportSvg {
    const TAG: &'static str = "export_svg";
    const ROWS: &'static [Spec] = ROWS_EXPORT_SVG;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab), "page": self.page, "anyway": self.anyway, "untrusted": super::log_file(&self.out) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            page,
            out,
            rect,
            text,
            scale,
            strip_metadata,
            anyway,
        } = self;
        let (access, events) = (cx.access, &mut *cx.events);
        {
            require_absolute(out)?;
            let (id, position, source) = locate(access, tab, *page)?;
            extract::refuse_if_restricted(&source.path, source.page, *anyway)?;
            let args = render::SvgArgs {
                path: &source.path,
                page: source.page + 1,
                out,
                rect: *rect,
                text: text.as_deref(),
                scale: *scale,
                strip_metadata: *strip_metadata,
            };
            let drawn = render::export_svg(&args);
            if !is_about_the_output(drawn.as_ref().err().map(|error| error.code)) {
                note_draw(access, id, position, drawn.as_ref().err().map(|error| error.code), events);
            }
            let mut data = drawn?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
    }
    fn human(data: &Value) -> String {
        export_svg_result(data)
    }
}

/// Where a page's images are, and what each is.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Images {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
}

impl Cmd for Images {
    const TAG: &'static str = "images";
    const ROWS: &'static [Spec] = ROWS_IMAGES;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab, page } = self;
        json!({ "tab": super::log_tab(tab), "page": page })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab, page } = self;
        let access = cx.access;
        {
            let (_, _, source) = locate(access, tab, *page)?;
            let mut data = extract::images(&source.path, source.page + 1)?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
    }
    fn human(data: &Value) -> String {
        images(data)
    }
}

/// One of a page's images to a new file, as itself: a JPEG's own bytes, or a lossless PNG.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
    /// From 1, as `images` lists them.
    pub number: u32,
    pub out: PathBuf,
    #[serde(default)]
    pub anyway: bool,
}

impl Cmd for Image {
    const TAG: &'static str = "image";
    const ROWS: &'static [Spec] = ROWS_IMAGE;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self {
            tab,
            page,
            number,
            out,
            anyway,
        } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "number": number, "anyway": anyway, "untrusted": super::log_file(out) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            page,
            number,
            out,
            anyway,
        } = self;
        let access = cx.access;
        {
            require_absolute(out)?;
            let (_, _, source) = locate(access, tab, *page)?;
            let mut data = extract::image_to_file(&source.path, source.page + 1, *number, out, *anyway)?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
    }
    fn human(data: &Value) -> String {
        format!(
            "Image {} of page {} written to {} in {}: {}, {} x {} pixels, {}.",
            number(&data["number"]),
            number(&data["page"]),
            text(&data["out"]),
            duration(number(&data["took_ms"])),
            text(&data["says"]),
            number(&data["width"]),
            number(&data["height"]),
            bytes(number(&data["bytes"]))
        )
    }
}

/// A box on a page as PNG, JPG, SVG or text.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    pub page: u32,
    /// `x, y, width, height`, in points from the page's top left, as it's shown.
    pub rect: [f32; 4],
    /// `png`, `jpg`, `svg` or `text`.
    #[serde(rename = "as")]
    pub format: String,
    #[serde(default)]
    pub out: Option<PathBuf>,
    #[serde(default = "crate::render::default_scale")]
    pub scale: f32,
    #[serde(default)]
    pub anyway: bool,
}

impl Cmd for Snapshot {
    const TAG: &'static str = "snapshot";
    const ROWS: &'static [Spec] = ROWS_SNAPSHOT;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        // Text comes back in the answer, as `copy` gives it. A file written is an export.
        if self.out.is_none() && !extract::snapshot_writes(&self.format) {
            return Access::At(Level::ReadOnly);
        }
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self {
            tab,
            page,
            format,
            out,
            anyway,
            ..
        } = self;
        json!({ "tab": super::log_tab(tab), "page": page, "as": recto_core::clean_line(format), "anyway": anyway, "untrusted": out.as_deref().map(super::log_file) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            page,
            rect,
            format,
            out,
            scale,
            anyway,
        } = self;
        let access = cx.access;
        {
            if let Some(out) = out {
                require_absolute(out)?;
            }
            let (_, _, source) = locate(access, tab, *page)?;
            let mut data = extract::snapshot_command(&extract::SnapshotAsked {
                path: &source.path,
                page: source.page + 1,
                rect: *rect,
                format,
                out: out.as_deref(),
                scale: *scale,
                anyway: *anyway,
            })?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
    }
    fn human(data: &Value) -> String {
        snapshot(data)
    }
}

/// Pages printed to a new PDF, through what the app prints with.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Print {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
    /// `4`, `4-6` or `4-`: all of them if left out.
    #[serde(default)]
    pub pages: Option<String>,
    #[serde(default = "crate::print::default_dpi")]
    pub dpi: u32,
    pub to_pdf: PathBuf,
    /// Prints a file that asks not to be copied to a PDF. Only once the user has said to; logged. Print to paper never asks.
    #[serde(default)]
    pub anyway: bool,
}

impl Cmd for Print {
    const TAG: &'static str = "print";
    const ROWS: &'static [Spec] = ROWS_PRINT;
    const DRAWS: bool = true;
    const READS_PAGES: bool = true;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self {
            tab,
            pages,
            dpi,
            to_pdf,
            anyway,
        } = self;
        json!({ "tab": super::log_tab(tab), "pages": pages.as_deref().map(clean_range), "dpi": dpi, "anyway": anyway, "untrusted": super::log_file(to_pdf) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            tab,
            pages,
            dpi,
            to_pdf,
            anyway,
        } = self;
        let access = cx.access;
        {
            require_absolute(to_pdf)?;
            let list = print::print_pages(access, tab, pages.as_deref())?;
            print::print_to_file(&list, *dpi, to_pdf, *anyway)
        }
    }
    fn human(data: &Value) -> String {
        format!(
            "Printed {} to {} at {} dpi in {}, {}.{}",
            count(number(&data["pages"]), "page", "pages"),
            text(&data["out"]),
            number(&data["dpi"]),
            duration(number(&data["took_ms"])),
            bytes(number(&data["bytes"])),
            if number(&data["turned_pages"]) > 0 {
                format!(
                    " {} turned as the unsaved edits have them.",
                    count(number(&data["turned_pages"]), "page", "pages")
                )
            } else {
                String::new()
            }
        )
    }
}

pub(crate) const ROWS_RENDER: &[Spec] = &[Spec {
    name: "render",
    tag: "render",
    group: GROUP_READ,
    access: Access::At(Level::Full),
    summary: "Draws a page to a new PNG file, sharp or not at all. It never replaces a file, and doesn't show an unsaved rotation.",
    args: &[
        PAGE_OR_BARE,
        Arg {
            key: "out",
            flag: Some("--out"),
            kind: Kind::Path,
            required: true,
            help: "A new file ending in `.png`.",
        },
        SCALE_OR_MAX,
        EXPORT_ANYWAY,
        TAB,
    ],
    bare_page: true,
    data: "`{ page, source_page, from_added_file, scale, largest_that_fits, out, took_ms, width, height, bytes, kind, unsaved_rotation_degrees, untrusted: { added_file } }`. A file whose permissions ask that it isn't copied is exit 8, `copy_restricted`, unless `--anyway` is given.",
    emits: &["page_drawn", "page_failed"],
    examples: &[
        "recto render --page 1 --out page1.png",
        "recto render 1 --out page1.png",
        "recto render --page 2 --scale 3x --out /tmp/p2.png --json",
        "recto render 2 --scale max --out /tmp/p2.png",
    ],
    client_wait_s: 600,
    ..BASE
}];

pub(crate) const ROWS_EXPORT_SVG: &[Spec] = &[Spec {
    name: "export svg",
    tag: "export_svg",
    group: GROUP_READ,
    access: Access::At(Level::Full),
    summary: "Writes a page, or a box on it, to a new SVG file, whole or not at all. It never replaces a file, and doesn't show an unsaved rotation. Paths, clips, outlines and selectable text are vector, and images stay at their own resolution: original JPEG bytes where they can be, otherwise PNG. What can't be vector goes as a picture of just that element, and the answer lists each kind.",
    args: &[
        PAGE_OR_BARE,
        Arg {
            key: "out",
            flag: Some("--out"),
            kind: Kind::Path,
            required: true,
            help: "A new file ending in `.svg`.",
        },
        Arg {
            key: "rect",
            flag: Some("--rect"),
            kind: Kind::Rect,
            required: false,
            help: "A box on the page: `x,y,width,height` in points from its top left, as the page is shown. What's wholly outside is left out, and images are cut to it. The whole page if left out.",
        },
        Arg {
            key: "text",
            flag: Some("--text"),
            kind: Kind::Choice(&["both", "paths", "selectable"]),
            required: false,
            help: "`both`: exact outlines, with invisible real text over them. `paths`: exact outlines, smaller, can't be selected. `selectable`: real text only, smallest, in the named font if the reader has it, so it can look different. `both` if left out.",
        },
        Arg {
            key: "scale",
            flag: Some("--scale"),
            kind: Kind::Scale,
            required: false,
            help: "The scale of anything that has to go as a picture: `2` or `2x`. 2x if left out. The rest has no scale.",
        },
        Arg {
            key: "strip_metadata",
            flag: Some("--strip-metadata"),
            kind: Kind::Switch,
            required: false,
            help: "Takes out of JPEGs what isn't the picture: EXIF with any GPS and thumbnail, XMP, comments, other application blocks and bytes after the end. Keeps JFIF, Adobe's marker and a colour profile, which decide the colours, and the compressed picture byte for byte. Also leaves out the document's title. Off by default, when a JPEG goes as its original bytes.",
        },
        EXPORT_ANYWAY,
        TAB,
    ],
    bare_page: true,
    data: "`{ page, source_page, from_added_file, out, took_ms, width_pt, height_pt, rect, text, raster_scale, raster_scale_lowered, bytes, all_vector, objects: { paths, clips, glyphs, glyph_runs, text_runs, text_chars, images_png, images_original_jpeg, images_jpeg_header_fixed, images_jpeg_with_mask, images_drawn_with_mask, images_jpeg_metadata_removed, chars_dropped }, image_bytes, strip_metadata, hidden_text: { checked, flagged, chars }, rasterised: [ { why, count, says } ], notes: [ { note, count, says } ], worker_peak_bytes, restarts, unsaved_rotation_degrees, untrusted: { added_file } }`. `all_vector: false` means `rasterised` lists what's a picture and why. `notes` says what was left out or kept unseen, such as text that draws nothing, and what the JPEGs carry besides their pictures, which is all still there unless `--strip-metadata` took it out. A JPEG with anything taken out counts in `images_jpeg_metadata_removed`, not `images_original_jpeg`. `hidden_text` is the check `text` gives, for the whole page, whatever the box. `checked: false` means it wasn't checked, not that there's none. `raster_scale_lowered: true` means a page too large for the scale asked had its pictures drawn at the largest that fits. `restarts` counts starts over in a larger worker, past a memory limit. Text from the file goes into the file, escaped, and nothing from it reaches this answer. A file whose permissions ask that it isn't copied is exit 8, `copy_restricted`, unless `--anyway` is given.",
    emits: &["page_drawn", "page_failed"],
    examples: &[
        "recto export svg --page 1 --out page1.svg",
        "recto export svg 1 --out page1.svg",
        "recto export svg --page 2 --rect 72,72,300,200 --out /tmp/box.svg --json",
        "recto export svg 3 --text paths --scale 3x --out /tmp/p3.svg",
        "recto export svg 1 --strip-metadata --out /tmp/share.svg",
    ],
    client_wait_s: 600,
    ..BASE
}];

pub(crate) const ROWS_IMAGES: &[Spec] = &[Spec {
    name: "images",
    tag: "images",
    group: GROUP_READ,
    summary: "Where a page's images are, numbered in the order the page draws them, with each one's own size and whether it's a JPEG. `recto image` takes one out by its number.",
    args: &[PAGE_OR_BARE, TAB],
    bare_page: true,
    data: "`{ page, source_page, from_added_file, count, listed, worker_peak_bytes, width_pt, height_pt, images: [ { number, rect: [x, y, width, height], width, height, jpeg, suggested_ending } ], untrusted: { added_file } }`. `rect` is in points from the page's top left, as it's shown, held to the page. `width` and `height` are the image's own pixels. `count` is every image, and `listed` how many are here: at most 4,096, the first drawn. An image wholly off the page keeps its number and isn't listed. `jpeg: true` means its stream is a JPEG, so `.jpg` may take its own bytes.",
    examples: &["recto images 3", "recto images --page 3 --json"],
    client_wait_s: 600,
    ..BASE
}];

pub(crate) const ROWS_IMAGE: &[Spec] = &[Spec {
    name: "image",
    tag: "image",
    group: GROUP_READ,
    access: Access::At(Level::Full),
    summary: "One of a page's images to a new file, as itself, whole or not at all. `.jpg` takes a JPEG's own bytes, unchanged, and refuses an image that isn't one, as that would be re-encoded. `.png` takes any image, losslessly, at its own resolution, a mask as transparency. It never replaces a file.",
    args: &[
        PAGE_OR_BARE,
        Arg {
            key: "number",
            flag: Some("--number"),
            kind: Kind::Positive(1_000_000),
            required: true,
            help: "Which image, from 1, as `recto images` lists them.",
        },
        Arg {
            key: "out",
            flag: Some("--out"),
            kind: Kind::Path,
            required: true,
            help: "A new file ending in `.png`, or `.jpg` for a JPEG's own bytes.",
        },
        Arg {
            key: "anyway",
            flag: Some("--anyway"),
            kind: Kind::Switch,
            required: false,
            help: "Take it from a file whose permissions ask that it isn't copied. Only once the user has said to. Each use goes in the activity log.",
        },
        TAB,
    ],
    bare_page: true,
    data: "`{ page, source_page, from_added_file, number, out, coded, lossless, says, width, height, bytes, took_ms, anyway, worker_peak_bytes, untrusted: { added_file } }`. `coded` is `original_jpeg` (its own bytes), `jpeg_header_fixed` (its own picture, with what's around it corrected so it shows as on the page), `png`, `png_with_mask`, or `png_drawn`: an image that can be drawn but not handed over, drawn alone at 2x, with `lossless: false`. A file whose permissions ask that it isn't copied is exit 8, `copy_restricted`, unless `--anyway` is given.",
    examples: &[
        "recto image 3 --number 1 --out /tmp/photo.jpg",
        "recto image --page 3 --number 2 --out /tmp/chart.png --json",
    ],
    client_wait_s: 600,
    ..BASE
}];

pub(crate) const ROWS_SNAPSHOT: &[Spec] = &[Spec {
    name: "snapshot",
    tag: "snapshot",
    group: GROUP_READ,
    access: Access::At(Level::Full),
    summary: "A box on a page, as the snapshot tool takes it: a PNG or JPG drawn sharp at the scale asked for, an SVG cut to the box as `export svg --rect` writes it, or its text, by a copy's rules. A file is new, whole or not at all. Text comes back in the answer, at Read-only, as `copy` gives it.",
    args: &[
        PAGE_OR_BARE,
        Arg {
            key: "rect",
            flag: Some("--rect"),
            kind: Kind::Rect,
            required: true,
            help: "The box: `x,y,width,height` in points from the page's top left, as it's shown. The part on the page is taken.",
        },
        Arg {
            key: "as",
            flag: Some("--as"),
            kind: Kind::Choice(&["png", "jpg", "svg", "text"]),
            required: true,
            help: "`png`, `jpg` (re-encoded, so never lossless), `svg`, or `text`.",
        },
        Arg {
            key: "out",
            flag: Some("--out"),
            kind: Kind::Path,
            required: false,
            help: "A new file ending in `.png`, `.jpg`, or `.svg`. Not for text.",
        },
        SCALE,
        Arg {
            key: "anyway",
            flag: Some("--anyway"),
            kind: Kind::Switch,
            required: false,
            help: "Take it from a file whose permissions ask that it isn't copied. Only once the user has said to. Each use goes in the activity log.",
        },
        TAB,
    ],
    bare_page: true,
    data: "`{ page, source_page, from_added_file, rect, as, out, width, height, scale, reencoded, all_vector, raster_scale, bytes, chars, hidden_left_out, chrome_left_out, checked, stopped, took_ms, anyway, worker_peak_bytes, untrusted: { text, added_file } }`. A picture has `width`, `height`, `scale`, and `reencoded: true` for JPG. An SVG has `all_vector` and `raster_scale`, as `export svg` says them. Text has `chars`, what was left out as hidden or as running heads and page numbers, `checked: false` if it couldn't be checked for hidden text, and the text under `untrusted`: characters whose middle is in the box, in reading order, a line at a time. A file whose permissions ask that it isn't copied is exit 8, `copy_restricted`, unless `--anyway` is given.",
    examples: &[
        "recto snapshot 3 --rect 72,72,300,200 --as png --out /tmp/box.png",
        "recto snapshot 3 --rect 72,72,300,200 --as text --json",
        "recto snapshot --page 2 --rect 0,0,612,396 --as svg --out /tmp/top.svg",
        "recto snapshot 1 --rect 50,50,200,100 --as jpg --scale 3x --out /tmp/box.jpg",
    ],
    client_wait_s: 600,
    ..BASE
}];

pub(crate) const ROWS_PRINT: &[Spec] = &[Spec {
    name: "print",
    tag: "print",
    group: GROUP_READ,
    access: Access::At(Level::Full),
    summary: "Prints pages to a new PDF, through the same drawing the app's File > Print uses: the real pages, drawn at print resolution, never the screen's, with no paper colour, and unsaved turns as the edits have them. Whole or not at all, and it never replaces a file.",
    args: &[
        Arg {
            key: "to_pdf",
            flag: Some("--to-pdf"),
            kind: Kind::Path,
            required: true,
            help: "A new file ending in `.pdf`.",
        },
        Arg {
            key: "pages",
            flag: Some("--pages"),
            kind: Kind::PagesOpen,
            required: false,
            help: "Only these pages: `4`, `4-6`, or `4-` to the end. Every page if left out.",
        },
        Arg {
            key: "dpi",
            flag: Some("--dpi"),
            kind: Kind::Positive(600),
            required: false,
            help: "Dots per inch, from 72 to 600. 300 if left out.",
        },
        EXPORT_ANYWAY,
        TAB,
    ],
    data: "`{ out, pages, dpi, bytes, heavy_pages, scaled_down, worker_peak_bytes, took_ms, turned_pages, anyway }`. Each page is one picture at its own size in points, `/Rotate`d by its unsaved turn. `heavy_pages` counts pages that needed the larger limit, which waits for the heavy slot as any export does. `scaled_down: [ { page, dpi } ]` lists pages too large to draw whole at the resolution asked, printed at the most they can be. A PDF's flag against printing is ignored (owner, 2026-11-08). A file whose permissions ask that it isn't copied is exit 8, `copy_restricted`, unless `--anyway` is given.",
    examples: &[
        "recto print --to-pdf /tmp/printed.pdf",
        "recto print --pages 3-5 --dpi 150 --to-pdf /tmp/three.pdf --json",
    ],
    client_wait_s: 3600,
    ..BASE
}];

fn render_result(data: &Value) -> String {
    let largest = if data["largest_that_fits"] == true { ", the largest that fits," } else { "" };
    let mut out = format!(
        "Page {} drawn at {}x{largest} to {} in {}: {} x {} px, {}.",
        number(&data["page"]),
        data["scale"],
        text(&data["out"]),
        duration(number(&data["took_ms"])),
        number(&data["width"]),
        number(&data["height"]),
        bytes(number(&data["bytes"]))
    );
    if let Some(degrees) = data["unsaved_rotation_degrees"].as_u64() {
        let _ = write!(out, " It doesn't show the unsaved turn of {degrees} degrees.");
    }
    out
}

fn export_svg_result(data: &Value) -> String {
    let part = if data["rect"].is_array() { "A box of page" } else { "Page" };
    let mut out = format!(
        "{part} {} exported to {} in {}: {} x {} pt, {}.",
        number(&data["page"]),
        text(&data["out"]),
        duration(number(&data["took_ms"])),
        data["width_pt"].as_f64().unwrap_or(0.0),
        data["height_pt"].as_f64().unwrap_or(0.0),
        bytes(number(&data["bytes"]))
    );
    let objects = &data["objects"];
    let _ = write!(
        out,
        "\n{} paths, {} of text, {} images ({} as the original JPEG bytes, {} as PNG with every pixel as decoded, {} drawn with their masks at their own size, which isn't byte for byte).",
        number(&objects["paths"]),
        count(number(&objects["text_chars"]), "character", "characters"),
        number(&objects["images_png"])
            + number(&objects["images_original_jpeg"])
            + number(&objects["images_jpeg_header_fixed"])
            + number(&objects["images_jpeg_with_mask"])
            + number(&objects["images_drawn_with_mask"]),
        number(&objects["images_original_jpeg"]),
        number(&objects["images_png"]),
        number(&objects["images_drawn_with_mask"])
    );
    if number(&objects["images_jpeg_header_fixed"]) > 0 {
        let _ = write!(
            out,
            "\n{} with what's around the JPEG's picture corrected and the picture byte for byte.",
            number(&objects["images_jpeg_header_fixed"])
        );
    }
    if number(&objects["images_jpeg_with_mask"]) > 0 {
        let _ = write!(
            out,
            "\n{} as the JPEG's picture byte for byte through a mask beside it, at the image's size.",
            number(&objects["images_jpeg_with_mask"])
        );
    }
    if data["all_vector"] == true {
        out.push_str("\nNothing had to become a picture.");
    }
    for item in data["rasterised"].as_array().map_or(&[][..], Vec::as_slice) {
        let _ = write!(
            out,
            "\nAs a picture at {}x, {}: {}.",
            data["raster_scale"].as_f64().unwrap_or(0.0),
            number(&item["count"]),
            text(&item["says"])
        );
    }
    for note in data["notes"].as_array().map_or(&[][..], Vec::as_slice) {
        let _ = write!(out, "\n{} {}.", number(&note["count"]), text(&note["says"]));
    }
    if data["hidden_text"]["flagged"] == true {
        let _ = write!(
            out,
            "\nThis page has text that looks hidden, {}: `recto text` shows where. It's in the file as selectable text, like the rest.",
            count(number(&data["hidden_text"]["chars"]), "character", "characters")
        );
    }
    if data["raster_scale_lowered"] == true {
        let _ = write!(
            out,
            "\nThis page is too large for the scale asked, so its pictures are at {}x, the largest that fits.",
            data["raster_scale"].as_f64().unwrap_or(0.0)
        );
    }
    if let Some(degrees) = data["unsaved_rotation_degrees"].as_u64() {
        let _ = write!(out, "\nIt doesn't show the unsaved turn of {degrees} degrees.");
    }
    out
}

fn images(data: &Value) -> String {
    let rows = data["images"].as_array().map_or(&[][..], Vec::as_slice);
    if number(&data["count"]) == 0 {
        return format!("Page {} has no images.", number(&data["page"]));
    }
    let mut out = format!("Page {} has {}.", number(&data["page"]), count(number(&data["count"]), "image", "images"));
    for row in rows {
        let rect = &row["rect"];
        let _ = write!(
            out,
            "
{}: {} x {} pixels{}, at {}, {}, {} x {} pt.",
            number(&row["number"]),
            number(&row["width"]),
            number(&row["height"]),
            if row["jpeg"] == true { ", a JPEG" } else { "" },
            rect[0],
            rect[1],
            rect[2],
            rect[3]
        );
    }
    if number(&data["listed"]) < number(&data["count"]) {
        let _ = write!(
            out,
            "
The rest aren't listed: {} are off the page or past the first 4,096.",
            number(&data["count"]) - number(&data["listed"])
        );
    }
    out
}

fn snapshot(data: &Value) -> String {
    if data["as"] == "text" {
        let mut out = format!(
            "{} in the box on page {}.",
            count(number(&data["chars"]), "character", "characters"),
            number(&data["page"])
        );
        if number(&data["hidden_left_out"]) > 0 {
            let _ = write!(
                out,
                "
{} that looked hidden left out.",
                count(number(&data["hidden_left_out"]), "character", "characters")
            );
        }
        if number(&data["chrome_left_out"]) > 0 {
            let _ = write!(
                out,
                "
{} of running heads, footers and page numbers left out.",
                count(number(&data["chrome_left_out"]), "character", "characters")
            );
        }
        if data["checked"] == false {
            out.push_str(
                "
It couldn't be checked for hidden text.",
            );
        }
        out.push_str(
            "
---",
        );
        write_page_lines(&mut out, text(&data["untrusted"]["text"]), &[]);
        return out;
    }
    let mut out = format!(
        "A box of page {} written to {} as {} in {}, {}",
        number(&data["page"]),
        text(&data["out"]),
        text(&data["as"]).to_uppercase(),
        duration(number(&data["took_ms"])),
        bytes(number(&data["bytes"]))
    );
    if data["width"].is_number() {
        let _ = write!(
            out,
            ", {} x {} pixels at {}x",
            number(&data["width"]),
            number(&data["height"]),
            data["scale"].as_f64().unwrap_or(0.0)
        );
    }
    out.push('.');
    if data["reencoded"] == true {
        out.push_str(
            "
A JPG is re-encoded, so it isn't lossless. PNG is.",
        );
    }
    if data["all_vector"] == false {
        let _ = write!(
            out,
            "
Some of it had to be pictures, at {}x.",
            data["raster_scale"].as_f64().unwrap_or(0.0)
        );
    }
    if let Some(degrees) = data["unsaved_rotation_degrees"].as_u64() {
        let _ = write!(
            out,
            "
It doesn't show the unsaved turn of {degrees} degrees."
        );
    }
    out
}
