//! A streaming SVG writer over [`std::io::Write`]. It holds no document: what's handed to it goes out, and memory stays at one small buffer, one row and one compressor however large the page is.
//!
//! **Everything from outside is hostile.** Text comes from PDFs. A string can't break out of the XML, because text is escaped and characters XML can't hold are dropped; it can't reach an attribute, because
//! attributes hold only numbers, fixed words and a font name cleaned to letters and digits; and it can't name anything, because ids are numbers given by the caller and references are built from them. So
//! there's no `<script>`, no event attribute and no external reference to be had: the only hrefs written are `#g12` and `data:image/...`.
//!
//! **Numbers** go through integers ([`fixed`]), so scientific notation, `NaN` and `inf` can't be written. A value that isn't finite becomes 0, and one past [`MAX_COORD`] stops there.
//!
//! **Paths** use relative commands on a fixed grid, each step measured from where the rounded path really is, so rounding doesn't add up along a long path.

mod fixed;
mod png;
pub mod verify;

pub use fixed::{MAX_COORD, fixed_grid};
pub use png::PngWriter;

use png::Base64;
use std::io::{self, Write};

/// A 2D transform, as PDF writes it: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// This one first, then `next`.
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        Self {
            a: self.a * next.a + self.b * next.c,
            b: self.a * next.b + self.b * next.d,
            c: self.c * next.a + self.d * next.c,
            d: self.c * next.b + self.d * next.d,
            e: self.e * next.a + self.f * next.c + next.e,
            f: self.e * next.b + self.f * next.d + next.f,
        }
    }

    /// If this scales equally in every direction, and doesn't skew, by how much. A stroke's width can then be scaled with the path, which needs no `transform`.
    pub fn uniform_scale(&self) -> Option<f64> {
        let (x, y) = (self.a.hypot(self.b), self.c.hypot(self.d));
        let dot = self.a * self.c + self.b * self.d;
        ((x - y).abs() <= 1e-4 * x.max(y) && dot.abs() <= 1e-4 * x.max(y) * x.max(y)).then_some(x)
    }

    fn largest_scale(&self) -> f64 {
        self.a.hypot(self.b).max(self.c.hypot(self.d))
    }
}

/// A colour, 8 bits a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// What paints a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    None,
    /// With its opacity, 0 to 255.
    Solid(Rgb, u8),
    /// A gradient defined earlier by its id, with its opacity.
    Gradient(u32, u8),
}

/// How a stroke is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f64,
    /// PDF's numbers: 0 butt, 1 round, 2 square.
    pub cap: u8,
    /// PDF's numbers: 0 miter, 1 round, 2 bevel.
    pub join: u8,
    pub dashes: Vec<f32>,
    pub dash_phase: f64,
}

/// How a path is painted.
#[derive(Clone, Debug, PartialEq)]
pub struct PathStyle {
    pub fill: Paint,
    pub even_odd: bool,
    pub stroke: Option<Stroke>,
}

/// One point of a path, as PDF gives it: three [`SegKind::Curve`] in a row make one curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub kind: SegKind,
    pub close: bool,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    Move,
    Line,
    Curve,
}

/// What an image's bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageData {
    /// Rows of raw pixels, compressed here into a PNG.
    Pixels { width: u32, height: u32, channels: PngChannels },
    /// A file's original JPEG bytes, passed through untouched.
    Jpeg,
}

/// How many channels a PNG's rows hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngChannels {
    Grey = 1,
    Rgb = 3,
    Rgba = 4,
}

/// The style of a run of text. `family` comes from a file, and is cleaned before it's written.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle<'a> {
    /// `None` keeps it selectable and draws nothing.
    pub fill: Option<(Rgb, u8)>,
    pub family: &'a [u8],
    pub generic: GenericFamily,
    pub bold: bool,
    pub italic: bool,
}

/// What to fall back on if the named font isn't there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenericFamily {
    Serif,
    SansSerif,
    Monospace,
}

/// One stop of a gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// 0 to 1.
    pub offset: f64,
    pub color: Rgb,
    pub opacity: u8,
}

/// What this writer left out, for the caller's report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteStats {
    /// Characters XML can't hold, or that would change how text reads without being seen: bidi overrides and tag characters.
    pub chars_dropped: u64,
}

/// The local units of a glyph run: a thousandth of the font's size. See [`SvgWriter::glyph`].
pub const GLYPH_UNITS_PER_EM: f64 = 1000.0;
/// How deep [`SvgWriter::group_begin`] goes. Hostile nesting must not become hostile XML.
pub const MAX_GROUP_DEPTH: usize = 32;
/// Written out when this much has built up.
const BUFFER: usize = 64 * 1024;
const COORD_DIGITS: u32 = 3;
const LINEAR_DIGITS: u32 = 8;

/// An image in progress.
enum OpenImage<W: Write> {
    None,
    Png(PngWriter<Base64<Sink<W>>>),
    Jpeg(Base64<Sink<W>>),
}

/// Where everything finally goes, with a buffer in front. Taken apart and put back while an image goes through it, so the image's encoders can own it.
struct Sink<W: Write> {
    out: W,
    buffer: Vec<u8>,
}

impl<W: Write> Sink<W> {
    fn drain(&mut self) -> io::Result<()> {
        self.out.write_all(&self.buffer)?;
        self.buffer.clear();
        Ok(())
    }
}

impl<W: Write> Write for Sink<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buffer.len() + data.len() > BUFFER {
            self.drain()?;
        }
        if data.len() > BUFFER {
            self.out.write_all(data)?;
        } else {
            self.buffer.extend_from_slice(data);
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.drain()?;
        self.out.flush()
    }
}

/// The writer. Calls come in document order, and each writes as it goes.
pub struct SvgWriter<W: Write> {
    sink: Option<Sink<W>>,
    image: OpenImage<W>,
    defs_open: bool,
    clip_open: bool,
    depth: usize,
    stats: WriteStats,
}

fn err_state() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "calls in the wrong order")
}

impl<W: Write> SvgWriter<W> {
    /// Starts a document `width` by `height` units, with a title from a file if it has one, escaped. `root` takes the coordinates every later call uses to the document's, top left, y down.
    pub fn begin(out: W, width: f64, height: f64, root: &Matrix, title: Option<&str>) -> io::Result<Self> {
        let mut writer = Self { sink: Some(Sink { out, buffer: Vec::with_capacity(BUFFER) }), image: OpenImage::None, defs_open: false, clip_open: false, depth: 0, stats: WriteStats::default() };
        writer.push(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" version=\"1.1\" width=\"")?;
        writer.number(width, COORD_DIGITS)?;
        writer.push(b"\" height=\"")?;
        writer.number(height, COORD_DIGITS)?;
        writer.push(b"\" viewBox=\"0 0 ")?;
        writer.number(width, COORD_DIGITS)?;
        writer.push(b" ")?;
        writer.number(height, COORD_DIGITS)?;
        writer.push(b"\">\n")?;
        if let Some(title) = title {
            writer.push(b"<title>")?;
            for ch in title.chars().take(1024) {
                writer.text_char(ch)?;
            }
            writer.push(b"</title>\n")?;
        }
        // PDF's default miter limit is 10, SVG's is 4, and PDFium has no call to read a file's own.
        writer.push(b"<g stroke-miterlimit=\"10\" transform=\"")?;
        writer.matrix(root)?;
        writer.push(b"\">\n")?;
        Ok(writer)
    }

    /// What's been dropped so far.
    pub fn stats(&self) -> WriteStats {
        self.stats
    }

    fn sink(&mut self) -> io::Result<&mut Sink<W>> {
        self.sink.as_mut().ok_or_else(err_state)
    }

    fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.sink()?.write_all(bytes)
    }

    fn number(&mut self, value: f64, digits: u32) -> io::Result<()> {
        let mut text = [0u8; fixed::MAX_LEN];
        let len = fixed::write(&mut text, fixed_grid(value, digits), digits);
        self.push(&text[..len])
    }

    fn matrix(&mut self, m: &Matrix) -> io::Result<()> {
        self.push(b"matrix(")?;
        for (i, value) in [m.a, m.b, m.c, m.d].into_iter().enumerate() {
            if i > 0 {
                self.push(b" ")?;
            }
            self.number(value, LINEAR_DIGITS)?;
        }
        self.push(b" ")?;
        self.number(m.e, COORD_DIGITS + 1)?;
        self.push(b" ")?;
        self.number(m.f, COORD_DIGITS + 1)?;
        self.push(b")")
    }

    fn id(&mut self, prefix: &[u8], id: u32) -> io::Result<()> {
        self.push(prefix)?;
        // Whole, not through a coordinate's limit: two ids must never come out the same.
        let mut text = [0u8; fixed::MAX_LEN];
        let len = fixed::write(&mut text, i64::from(id), 0);
        self.push(&text[..len])
    }

    fn color(&mut self, color: Rgb) -> io::Result<()> {
        let Rgb(r, g, b) = color;
        let hex = |v: u8| b"0123456789abcdef"[usize::from(v & 15)];
        let digits = if r >> 4 == r & 15 && g >> 4 == g & 15 && b >> 4 == b & 15 {
            vec![b'#', hex(r), hex(g), hex(b)]
        } else {
            vec![b'#', hex(r >> 4), hex(r), hex(g >> 4), hex(g), hex(b >> 4), hex(b)]
        };
        self.push(&digits)
    }

    fn opacity(&mut self, attribute: &[u8], opacity: u8) -> io::Result<()> {
        if opacity == 255 {
            return Ok(());
        }
        self.push(attribute)?;
        self.number(f64::from(opacity) / 255.0, 3)?;
        self.push(b"\"")
    }

    fn paint(&mut self, attribute: &[u8], opacity_attribute: &[u8], paint: Paint) -> io::Result<()> {
        self.push(attribute)?;
        match paint {
            Paint::None => self.push(b"none\""),
            Paint::Solid(color, opacity) => {
                self.color(color)?;
                self.push(b"\"")?;
                self.opacity(opacity_attribute, opacity)
            }
            Paint::Gradient(id, opacity) => {
                self.id(b"url(#p", id)?;
                self.push(b")\"")?;
                self.opacity(opacity_attribute, opacity)
            }
        }
    }

    fn open_defs(&mut self) -> io::Result<()> {
        if !self.defs_open {
            self.defs_open = true;
            self.push(b"<defs>")?;
        }
        Ok(())
    }

    fn close_defs(&mut self) -> io::Result<()> {
        if self.defs_open {
            self.defs_open = false;
            self.push(b"</defs>\n")?;
        }
        Ok(())
    }

    /// Writes a path's `d`. `to_units` takes each point to the numbers written, on a grid of `digits` decimal places.
    fn d(&mut self, segs: impl IntoIterator<Item = Seg>, to_units: &Matrix, digits: u32) -> io::Result<()> {
        let mut pen = PathPen::new(digits);
        let mut pending: [(f64, f64); 2] = [(0.0, 0.0); 2];
        let mut pending_len = 0;
        for seg in segs {
            let point = to_units.apply(f64::from(seg.x), f64::from(seg.y));
            // A broken curve from a hostile worker becomes lines, and `d` stays well formed whatever comes.
            if seg.kind != SegKind::Curve || pending_len == 2 {
                if seg.kind == SegKind::Curve {
                    pen.curve(pending[0], pending[1], point, self.sink()?)?;
                    pending_len = 0;
                } else {
                    for &p in &pending[..pending_len] {
                        pen.line(p, self.sink()?)?;
                    }
                    pending_len = 0;
                    match seg.kind {
                        SegKind::Move => pen.move_to(point, self.sink()?)?,
                        _ => pen.line(point, self.sink()?)?,
                    }
                }
                if seg.close {
                    pen.close(self.sink()?)?;
                }
            } else {
                pending[pending_len] = point;
                pending_len += 1;
            }
        }
        for &p in &pending[..pending_len] {
            pen.line(p, self.sink()?)?;
        }
        Ok(())
    }

    /// One path. `matrix` takes its points to the coordinates [`Self::begin`] set up. A stroke that a matrix would stretch unevenly keeps the matrix, so its pen stretches too; otherwise it's taken into the numbers, which is shorter.
    pub fn path(&mut self, style: &PathStyle, matrix: &Matrix, segs: impl IntoIterator<Item = Seg>) -> io::Result<()> {
        self.close_defs()?;
        let scale = matrix.uniform_scale();
        let bake = style.stroke.is_none() || scale.is_some();
        self.push(b"<path")?;
        self.paint(b" fill=\"", b" fill-opacity=\"", style.fill)?;
        if style.even_odd && style.fill != Paint::None {
            self.push(b" fill-rule=\"evenodd\"")?;
        }
        if let Some(stroke) = &style.stroke {
            let factor = if bake { scale.unwrap_or(1.0) } else { 1.0 };
            self.paint(b" stroke=\"", b" stroke-opacity=\"", stroke.paint)?;
            // PDF's zero is the thinnest line a device can draw, and SVG's draws nothing. A width that would be written as zero means the same, and comes out as one pixel whatever the zoom.
            if fixed_grid(stroke.width * factor, COORD_DIGITS + 1) == 0 {
                self.push(b" vector-effect=\"non-scaling-stroke\" stroke-width=\"1\"")?;
            } else if (stroke.width - 1.0).abs() > f64::EPSILON || factor != 1.0 {
                self.push(b" stroke-width=\"")?;
                self.number(stroke.width * factor, COORD_DIGITS + 1)?;
                self.push(b"\"")?;
            }
            match stroke.cap {
                1 => self.push(b" stroke-linecap=\"round\"")?,
                2 => self.push(b" stroke-linecap=\"square\"")?,
                _ => {}
            }
            match stroke.join {
                1 => self.push(b" stroke-linejoin=\"round\"")?,
                2 => self.push(b" stroke-linejoin=\"bevel\"")?,
                _ => {}
            }
            // An all-zero list is an error to SVG, which turns the stroke off. PDF means a solid line.
            if stroke.dashes.iter().any(|dash| *dash > 0.0) {
                self.push(b" stroke-dasharray=\"")?;
                for (i, dash) in stroke.dashes.iter().enumerate() {
                    if i > 0 {
                        self.push(b" ")?;
                    }
                    self.number(f64::from(dash.max(0.0)) * factor, COORD_DIGITS)?;
                }
                self.push(b"\"")?;
                if stroke.dash_phase != 0.0 {
                    self.push(b" stroke-dashoffset=\"")?;
                    self.number(stroke.dash_phase * factor, COORD_DIGITS)?;
                    self.push(b"\"")?;
                }
            }
        }
        if bake {
            self.push(b" d=\"")?;
            self.d(segs, matrix, COORD_DIGITS)?;
        } else {
            self.push(b" transform=\"")?;
            self.matrix(matrix)?;
            self.push(b"\" d=\"")?;
            // Small numbers under a large matrix need more places for the same accuracy on the page.
            let extra = matrix.largest_scale().clamp(1.0, 1e4).log10().ceil() as u32;
            self.d(segs, &Matrix::IDENTITY, (COORD_DIGITS + extra).min(7))?;
        }
        self.push(b"\"/>\n")
    }

    /// Defines clip `id`, a shape in the coordinates `matrix` leads from, itself clipped by `parent` if that's not 0. Defined once and referred to by number.
    pub fn clip_define(&mut self, id: u32, parent: u32, matrix: &Matrix, segs: impl IntoIterator<Item = Seg>) -> io::Result<()> {
        self.open_defs()?;
        self.id(b"<clipPath id=\"c", id)?;
        if parent != 0 {
            self.id(b"\" clip-path=\"url(#c", parent)?;
            self.push(b")")?;
        }
        self.push(b"\"><path d=\"")?;
        self.d(segs, matrix, COORD_DIGITS)?;
        self.push(b"\"/></clipPath>")
    }

    /// What's written from here on is clipped by this, or by nothing if it's 0, until the next call.
    pub fn set_clip(&mut self, id: u32) -> io::Result<()> {
        self.close_defs()?;
        if self.clip_open {
            self.clip_open = false;
            self.push(b"</g>\n")?;
        }
        if id != 0 {
            self.clip_open = true;
            self.id(b"<g clip-path=\"url(#c", id)?;
            self.push(b")\">\n")?;
        }
        Ok(())
    }

    /// A group at one opacity, as a transparency group is. Past [`MAX_GROUP_DEPTH`] it's refused.
    pub fn group_begin(&mut self, opacity: u8) -> io::Result<()> {
        if self.depth >= MAX_GROUP_DEPTH {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "groups nested too deeply"));
        }
        self.set_clip(0)?;
        self.depth += 1;
        self.push(b"<g")?;
        self.opacity(b" opacity=\"", opacity)?;
        self.push(b">\n")
    }

    pub fn group_end(&mut self) -> io::Result<()> {
        if self.depth == 0 {
            return Err(err_state());
        }
        self.set_clip(0)?;
        self.depth -= 1;
        self.push(b"</g>\n")
    }

    /// Defines glyph `id`, in thousandths of an em with y up, ready for [`Self::glyph_run`]. It costs no more than its outline however often it's used.
    pub fn glyph(&mut self, id: u32, segs: impl IntoIterator<Item = Seg>) -> io::Result<()> {
        self.open_defs()?;
        self.id(b"<path id=\"g", id)?;
        self.push(b"\" d=\"")?;
        let units = Matrix::new(GLYPH_UNITS_PER_EM, 0.0, 0.0, GLYPH_UNITS_PER_EM, 0.0, 0.0);
        self.d(segs, &units, 0)?;
        self.push(b"\"/>")
    }

    /// Glyphs along a run. `matrix` takes the run's local coordinates to the document's, and each glyph sits at its `(id, x, y)` there, in the same thousandths of an em.
    pub fn glyph_run(&mut self, matrix: &Matrix, fill: Paint, stroke: Option<(Paint, f64)>, glyphs: impl IntoIterator<Item = (u32, f32, f32)>) -> io::Result<()> {
        self.close_defs()?;
        self.push(b"<g transform=\"")?;
        self.matrix(matrix)?;
        self.push(b"\"")?;
        self.paint(b" fill=\"", b" fill-opacity=\"", fill)?;
        if let Some((paint, width)) = stroke {
            self.paint(b" stroke=\"", b" stroke-opacity=\"", paint)?;
            self.push(b" stroke-width=\"")?;
            self.number(width, 1)?;
            self.push(b"\"")?;
        }
        self.push(b">")?;
        for (id, x, y) in glyphs {
            self.id(b"<use xlink:href=\"#g", id)?;
            self.push(b"\" x=\"")?;
            self.number(f64::from(x), 0)?;
            if fixed_grid(f64::from(y), 0) != 0 {
                self.push(b"\" y=\"")?;
                self.number(f64::from(y), 0)?;
            }
            self.push(b"\"/>")?;
        }
        self.push(b"</g>\n")
    }

    /// Text as text: real characters, placed one by one, so it can be selected and searched, in the named font if the reader has it. Same local coordinates as [`Self::glyph_run`], y up.
    pub fn text_run(&mut self, matrix: &Matrix, style: &TextStyle<'_>, chars: impl Iterator<Item = (char, f32, f32)> + Clone) -> io::Result<()> {
        self.close_defs()?;
        self.push(b"<text transform=\"")?;
        // Text is drawn y down, and the run's local coordinates are y up.
        let flip = Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 0.0).then(matrix);
        self.matrix(&flip)?;
        self.push(b"\" font-size=\"1000\" font-family=\"")?;
        self.family(style.family, style.generic)?;
        self.push(b"\"")?;
        if style.bold {
            self.push(b" font-weight=\"700\"")?;
        }
        if style.italic {
            self.push(b" font-style=\"italic\"")?;
        }
        match style.fill {
            Some((color, opacity)) => self.paint(b" fill=\"", b" fill-opacity=\"", Paint::Solid(color, opacity))?,
            None => self.push(b" fill-opacity=\"0\"")?,
        }
        self.push(b" xml:space=\"preserve\" x=\"")?;
        let kept = |&(ch, _, _): &(char, f32, f32)| keep_in_text(ch);
        for (i, (_, x, _)) in chars.clone().filter(kept).enumerate() {
            if i > 0 {
                self.push(b" ")?;
            }
            self.number(f64::from(x), 0)?;
        }
        self.push(b"\" y=\"")?;
        let mut first = None;
        let same_line = chars.clone().filter(kept).all(|(_, _, y)| fixed_grid(f64::from(y), 0) == *first.get_or_insert(fixed_grid(f64::from(y), 0)));
        if same_line {
            self.number(-f64::from(chars.clone().filter(kept).map_or_zero()), 0)?;
        } else {
            for (i, (_, _, y)) in chars.clone().filter(kept).enumerate() {
                if i > 0 {
                    self.push(b" ")?;
                }
                self.number(-f64::from(y), 0)?;
            }
        }
        self.push(b"\">")?;
        for (ch, _, _) in chars {
            self.text_char(ch)?;
        }
        self.push(b"</text>\n")
    }

    /// One character of text, or nothing if it's one that mustn't be kept.
    fn text_char(&mut self, ch: char) -> io::Result<()> {
        if !keep_in_text(ch) {
            self.stats.chars_dropped += 1;
            return Ok(());
        }
        match ch {
            '&' => self.push(b"&amp;"),
            '<' => self.push(b"&lt;"),
            '>' => self.push(b"&gt;"),
            '"' => self.push(b"&quot;"),
            '\'' => self.push(b"&apos;"),
            // A bare one would read as a line end or be folded, and these are written as numbers so nothing changes them.
            '\t' => self.push(b"&#9;"),
            '\n' => self.push(b"&#10;"),
            '\r' => self.push(b"&#13;"),
            _ => {
                let mut utf8 = [0; 4];
                self.push(ch.encode_utf8(&mut utf8).as_bytes())
            }
        }
    }

    fn family(&mut self, family: &[u8], generic: GenericFamily) -> io::Result<()> {
        let cleaned = clean_family(family);
        if !cleaned.is_empty() {
            self.push(b"'")?;
            self.push(&cleaned)?;
            self.push(b"',")?;
        }
        self.push(match generic {
            GenericFamily::Serif => b"serif",
            GenericFamily::SansSerif => b"sans-serif",
            GenericFamily::Monospace => b"monospace",
        })
    }

    /// Defines a linear gradient, for [`Paint::Gradient`]. Points are in the coordinates [`Self::begin`] set up.
    pub fn linear_gradient(&mut self, id: u32, from: (f64, f64), to: (f64, f64), stops: &[GradientStop]) -> io::Result<()> {
        self.open_defs()?;
        self.id(b"<linearGradient id=\"p", id)?;
        self.push(b"\" gradientUnits=\"userSpaceOnUse\" x1=\"")?;
        self.number(from.0, COORD_DIGITS)?;
        self.push(b"\" y1=\"")?;
        self.number(from.1, COORD_DIGITS)?;
        self.push(b"\" x2=\"")?;
        self.number(to.0, COORD_DIGITS)?;
        self.push(b"\" y2=\"")?;
        self.number(to.1, COORD_DIGITS)?;
        self.push(b"\">")?;
        self.stops(stops)?;
        self.push(b"</linearGradient>")
    }

    /// Defines a radial gradient, for [`Paint::Gradient`].
    pub fn radial_gradient(&mut self, id: u32, center: (f64, f64), radius: f64, stops: &[GradientStop]) -> io::Result<()> {
        self.open_defs()?;
        self.id(b"<radialGradient id=\"p", id)?;
        self.push(b"\" gradientUnits=\"userSpaceOnUse\" cx=\"")?;
        self.number(center.0, COORD_DIGITS)?;
        self.push(b"\" cy=\"")?;
        self.number(center.1, COORD_DIGITS)?;
        self.push(b"\" r=\"")?;
        self.number(radius.max(0.0), COORD_DIGITS)?;
        self.push(b"\">")?;
        self.stops(stops)?;
        self.push(b"</radialGradient>")
    }

    fn stops(&mut self, stops: &[GradientStop]) -> io::Result<()> {
        for stop in stops {
            self.push(b"<stop offset=\"")?;
            self.number(stop.offset.clamp(0.0, 1.0), 4)?;
            self.push(b"\" stop-color=\"")?;
            self.color(stop.color)?;
            self.push(b"\"")?;
            self.opacity(b" stop-opacity=\"", stop.opacity)?;
            self.push(b"/>")?;
        }
        Ok(())
    }

    /// Starts an image over the unit square of `matrix`, PDF's way round: its top row runs along y = 1. Bytes follow through [`Self::image_bytes`], with nothing else until [`Self::image_end`]. `passive` lets the pointer through it, for a picture standing in for part of a page, so selectable text under it can still be selected.
    pub fn image_begin(&mut self, data: ImageData, matrix: &Matrix, opacity: u8, passive: bool) -> io::Result<()> {
        self.close_defs()?;
        self.push(b"<image width=\"1\" height=\"1\" preserveAspectRatio=\"none\" transform=\"")?;
        // SVG's image runs top down from y = 0, and PDF's from y = 1 at the top.
        let flip = Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0).then(matrix);
        self.matrix(&flip)?;
        self.push(b"\"")?;
        self.opacity(b" opacity=\"", opacity)?;
        if passive {
            self.push(b" pointer-events=\"none\"")?;
        }
        self.push(b" xlink:href=\"data:image/")?;
        self.push(match data {
            ImageData::Pixels { .. } => b"png",
            ImageData::Jpeg => b"jpeg",
        })?;
        self.push(b";base64,")?;
        let sink = self.sink.take().ok_or_else(err_state)?;
        self.image = match data {
            ImageData::Pixels { width, height, channels } => OpenImage::Png(PngWriter::new(Base64::new(sink), width, height, channels)?),
            ImageData::Jpeg => OpenImage::Jpeg(Base64::new(sink)),
        };
        Ok(())
    }

    /// The next bytes of the image. Whole rows aren't needed: pieces can end anywhere.
    pub fn image_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.image {
            OpenImage::None => Err(err_state()),
            OpenImage::Png(png) => png.write_all(bytes),
            OpenImage::Jpeg(jpeg) => jpeg.write_all(bytes),
        }
    }

    /// Ends the image. A PNG short of rows is an error, because its bytes would be a broken file.
    pub fn image_end(&mut self) -> io::Result<()> {
        let sink = match std::mem::replace(&mut self.image, OpenImage::None) {
            OpenImage::None => return Err(err_state()),
            OpenImage::Png(png) => png.finish()?.finish()?,
            OpenImage::Jpeg(jpeg) => jpeg.finish()?,
        };
        self.sink = Some(sink);
        self.push(b"\"/>\n")
    }

    /// Ends the document and hands back what it was written to, flushed. Without this, the file isn't valid.
    pub fn finish(mut self) -> io::Result<W> {
        self.set_clip(0)?;
        while self.depth > 0 {
            self.group_end()?;
        }
        self.push(b"</g>\n</svg>\n")?;
        let mut sink = self.sink.take().ok_or_else(err_state)?;
        sink.flush()?;
        Ok(sink.out)
    }
}

/// The first item's `y`, for a run all on one line.
trait FirstY {
    fn map_or_zero(self) -> f32;
}

impl<I: Iterator<Item = (char, f32, f32)>> FirstY for I {
    fn map_or_zero(mut self) -> f32 {
        self.next().map_or(0.0, |(_, _, y)| y)
    }
}

/// Whether a character can go into text. XML 1.0 can't hold control characters, surrogates or the last two of each plane's block; bidi overrides and tag characters would change how the text reads without being seen.
pub fn keep_in_text(ch: char) -> bool {
    let code = ch as u32;
    let legal_xml = matches!(code, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x1_0000..=0x10_FFFF);
    legal_xml && !matches!(code, 0x202A..=0x202E | 0x2066..=0x2069 | 0x200E | 0x200F | 0x61C | 0xE_0000..=0xE_007F | 0x80..=0x9F)
}

/// Letters, digits, spaces and a little punctuation from a font's name, without a subset's `ABCDEF+` prefix. Nothing else reaches the attribute.
pub fn clean_family(family: &[u8]) -> Vec<u8> {
    let name = match family.iter().position(|&b| b == b'+') {
        Some(6) if family[..6].iter().all(u8::is_ascii_uppercase) => &family[7..],
        _ => family,
    };
    let mut cleaned: Vec<u8> = name.iter().copied().filter(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'_' | b'.')).take(64).collect();
    while cleaned.last() == Some(&b' ') {
        cleaned.pop();
    }
    cleaned
}

/// Writes a path's commands, each relative to where the rounded path has got to.
struct PathPen {
    digits: u32,
    /// The current point, on the grid.
    at: (i64, i64),
    start: (i64, i64),
    started: bool,
    last_command: u8,
    last_was_number: bool,
    text: [u8; fixed::MAX_LEN],
}

impl PathPen {
    fn new(digits: u32) -> Self {
        Self { digits, at: (0, 0), start: (0, 0), started: false, last_command: 0, last_was_number: false, text: [0; fixed::MAX_LEN] }
    }

    fn grid(&self, point: (f64, f64)) -> (i64, i64) {
        (fixed_grid(point.0, self.digits), fixed_grid(point.1, self.digits))
    }

    fn command(&mut self, command: u8, out: &mut impl Write) -> io::Result<()> {
        if self.last_command != command {
            out.write_all(&[command])?;
            self.last_command = command;
            self.last_was_number = false;
        }
        Ok(())
    }

    fn number(&mut self, value: i64, out: &mut impl Write) -> io::Result<()> {
        let len = fixed::write(&mut self.text, value, self.digits);
        if self.last_was_number && self.text[0] != b'-' {
            out.write_all(b" ")?;
        }
        out.write_all(&self.text[..len])?;
        self.last_was_number = true;
        Ok(())
    }

    fn move_to(&mut self, point: (f64, f64), out: &mut impl Write) -> io::Result<()> {
        let grid = self.grid(point);
        // `M` so a fresh subpath doesn't depend on anything before it. After a first move, a second in a row replaces it in PDF just the same.
        out.write_all(b"M")?;
        self.last_command = b'M';
        self.last_was_number = false;
        self.number(grid.0, out)?;
        self.number(grid.1, out)?;
        // Coordinates following an `M` would mean lines from there, so the next command is always written out.
        self.last_command = 0;
        self.at = grid;
        self.start = grid;
        self.started = true;
        Ok(())
    }

    fn ensure_started(&mut self, point: (f64, f64), out: &mut impl Write) -> io::Result<bool> {
        if self.started {
            return Ok(true);
        }
        // A path must start with a move, so a broken one gets one.
        self.move_to(point, out)?;
        Ok(false)
    }

    fn line(&mut self, point: (f64, f64), out: &mut impl Write) -> io::Result<()> {
        if !self.ensure_started(point, out)? {
            return Ok(());
        }
        let grid = self.grid(point);
        let (dx, dy) = (grid.0 - self.at.0, grid.1 - self.at.1);
        if dy == 0 {
            self.command(b'h', out)?;
            self.number(dx, out)?;
        } else if dx == 0 {
            self.command(b'v', out)?;
            self.number(dy, out)?;
        } else {
            self.command(b'l', out)?;
            self.number(dx, out)?;
            self.number(dy, out)?;
        }
        self.at = grid;
        Ok(())
    }

    fn curve(&mut self, one: (f64, f64), two: (f64, f64), end: (f64, f64), out: &mut impl Write) -> io::Result<()> {
        self.ensure_started(one, out)?;
        let (one, two, end) = (self.grid(one), self.grid(two), self.grid(end));
        self.command(b'c', out)?;
        for value in [one.0 - self.at.0, one.1 - self.at.1, two.0 - self.at.0, two.1 - self.at.1, end.0 - self.at.0, end.1 - self.at.1] {
            self.number(value, out)?;
        }
        self.at = end;
        Ok(())
    }

    fn close(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.started {
            out.write_all(b"z")?;
            self.last_command = b'z';
            self.last_was_number = false;
            self.at = self.start;
        }
        Ok(())
    }
}
