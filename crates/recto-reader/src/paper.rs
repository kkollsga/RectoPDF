//! Dark paper, as a change to a tile's pixels (owner, 2026-11-06: dark mode offers dark or white paper).
//!
//! It's done to the pixels rather than by a filter on the layer because a filter makes the compositor draw every frame through an offscreen pass, and because a QA shot wouldn't see it: `CALayer.render(in:)` leaves
//! filters out. A tile redrawn for another paper is just another tile.
//!
//! **The rule,** from `docs/concepts/reader-dark.png`: lightness turns over, so white paper goes dark and black text goes light, and each pixel keeps its colour's distance from grey, so blue text is lighter blue with no halo round its letters. Images on a page of text aren't turned over: the renderer says where they are, and they stay as they were, a little dimmed.
//!
//! **Each image is judged by its own pixels** (owner, 2026-11-07: "We need to detect scanned images (with predominently white bg, and flip and match grey tone paper color", and "similar with images with white bg"). One with a mostly white background, a scanned page, an embedded letter, a chart, turns over with the page, its background brought to true white first so it lands on the paper's tone, with no bright box or hard edge. One without stays as it is on a page of text.
//!
//! **A page that's mostly a picture of that second kind isn't turned over at all** (owner, 2026-11-07, answering "should pages that are mostly a picture be dimmed instead of flipped? I recommend yes": "I agree"). A map or a full-page photograph keeps its true colours, dimmed by [`PICTURE_PAGE_DIM`] so it doesn't glare. **When a page's images can't be told apart, it's judged whole, and what's uncertain is dimmed,** as an image turned over is a negative, and a page dimmed is only brighter than it might have been. There's one [`PageLook`] a page, so its tiles, preview and lower-resolution version can't disagree. **Its limits:** a photo inside a scan turns over with it, which is accepted. A chart drawn as vectors on a page of text turns over with the text, since pixels can't say what they belong to.

/// Which paper a tile was drawn for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaperKind {
    #[default]
    White,
    Dark,
}

use recto_render_service::TileKind;

/// Dark paper's colour, `0xRRGGBB`, from the concept.
pub const DARK_PAPER: u32 = 0x1E_20_23;
/// What black text becomes.
const DARK_INK: [i32; 3] = [0xE4, 0xE5, 0xE7];
/// How bright an image on a page of text stays, in 256ths.
const IMAGE_DIM: u32 = 218;
/// How bright a page that's mostly a picture stays, in 256ths. White at this gives off under half the light of white, beside a dark window, and black text on it, in a map's legend say, stays at about 9.7:1 against WCAG AA's 4.5, by a test. Chosen by looking at the USGS map's `heavy-dark` and `low-res-dark`: 192 still glared, and darker loses the hillshade.
pub const PICTURE_PAGE_DIM: u32 = 176;

/// How a highlight goes over a page: which of the system's two selection colours, and how it's laid over the pixels so text stays readable under it. On a light page it multiplies: white takes the colour and ink stays ink. On a dark page it lightens, each channel the brighter of the two: paper takes the colour and light text stays as it is. A highlight laid over with plain transparency either hides the text or barely shows, and these two come out as the system's own colour behind the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighlightLook {
    /// The system's colour for a dark page, which isn't always a dark window.
    pub dark_colour: bool,
    /// Lighten rather than multiply.
    pub lighten: bool,
}

impl PaperKind {
    /// How a highlight goes over this paper. Warm paper is white paper, with its tint laid over everything, the highlight included.
    pub fn highlight(self) -> HighlightLook {
        match self {
            Self::White => HighlightLook { dark_colour: false, lighten: false },
            Self::Dark => HighlightLook { dark_colour: true, lighten: true },
        }
    }

    pub fn rgb(self) -> u32 {
        match self {
            Self::White => 0xFF_FF_FF,
            Self::Dark => DARK_PAPER,
        }
    }
}

const fn paper_channels() -> [i32; 3] {
    [((DARK_PAPER >> 16) & 0xFF) as i32, ((DARK_PAPER >> 8) & 0xFF) as i32, (DARK_PAPER & 0xFF) as i32]
}

/// Turns one colour over, as a blank tile's single colour needs.
pub fn dark_rgb(rgb: u32) -> u32 {
    let mut pixel = [(rgb & 0xFF) as u8, ((rgb >> 8) & 0xFF) as u8, ((rgb >> 16) & 0xFF) as u8, 0xFF];
    turn_pixel(&mut pixel);
    (u32::from(pixel[2]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[0])
}

#[inline]
fn turn_pixel(pixel: &mut [u8; 4]) {
    let [paper_r, paper_g, paper_b] = paper_channels();
    let (b, g, r) = (i32::from(pixel[0]), i32::from(pixel[1]), i32::from(pixel[2]));
    // Rec. 601 weights, in 256ths: 255 is paper, 0 is ink.
    let luma = (77 * r + 150 * g + 29 * b) >> 8;
    let over = |paper: i32, ink: i32, value: i32| (((paper * luma + ink * (255 - luma)) / 255) + value - luma).clamp(0, 255) as u8;
    pixel[0] = over(paper_b, DARK_INK[2], b);
    pixel[1] = over(paper_g, DARK_INK[1], g);
    pixel[2] = over(paper_r, DARK_INK[0], r);
}

#[inline]
fn dim_pixel(pixel: &mut [u8; 4]) {
    for channel in &mut pixel[..3] {
        *channel = ((u32::from(*channel) * IMAGE_DIM) >> 8) as u8;
    }
}

/// How much of a page, added up over the images that stay pictures, makes it mostly a picture: more than half, which is what the words say. Measured over `bench100`: USGS map sheets 0.59 to 1.0, with their legends and titles in vectors around the map, and figures on arXiv pages of text 0.16 to 0.26. A USGS sheet at 0.40 turns over and keeps its map.
pub const MOSTLY_PICTURE: f64 = 0.5;
/// A pixel this light in every channel, with little colour to it, may be paper. Low enough for a scan's yellowed or grey paper, and for paper seen through a small picture's blurred text.
const PAPER_LIGHTEST_CHANNEL: u8 = 170;
/// How far a pixel's channels can be apart and it still be paper. Aged paper's yellow is about this, and a photo's sky and clouds mostly aren't.
const PAPER_MOST_SPREAD: u8 = 48;
/// A pixel's channels this far apart is colour, whatever else it is.
const COLOUR_LEAST_SPREAD: u8 = 56;
/// A pixel this light in every channel and this neutral is plain white, as a chart's or a clean document's background is. Tan or pale-green land on a map isn't.
const WHITE_LIGHTEST_CHANNEL: u8 = 232;
const WHITE_MOST_SPREAD: u8 = 14;
/// **What's white-backed, measured over 557 images in `bench100`, the fixtures and made scans: see `tests/look_survey.rs`.** The first kind: this much plain white, and no more colour than this. Charts, diagrams, screenshots of documents and clean scans are at 0.45 and up. The USGS map sheet is 0.21 white though 0.72 is light, and a heatmap with a white margin has 0.28 colour.
const WHITE_LEAST: f64 = 0.45;
const WHITE_MOST_COLOUR: f64 = 0.20;
/// The second kind, a scan on paper that's gone yellow or grey: no plain white at all, so nearly all of it must be paper, with next to no colour. A red stamp and a blue signature are about 0.02. Maps on tan are 0.04 and up, or under this much paper.
const SCAN_LEAST_PAPER: f64 = 0.75;
const SCAN_MOST_COLOUR: f64 = 0.03;
/// Judging a whole page, with its images not known, white margins count for the page: a map sheet is as white as a chart. So nearly none of it can be colour.
const WHOLE_PAGE_MOST_COLOUR: f64 = 0.05;
/// An image this small in a picture of the page has too few pixels to say. It's left alone.
const LEAST_PIXELS_TO_JUDGE: usize = 64;
/// Brought up to white, a pixel this close to it is white: a scan's noise doesn't speckle the paper.
const NEAR_WHITE_SNAP: u32 = 12;

/// What's found looking at an image's pixels in a small picture of its page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageSample {
    /// The part of it that's paper.
    pub paper: f64,
    /// The part of it that's colour.
    pub colour: f64,
    /// The part of it that's plain white, or as near as makes no difference.
    pub white: f64,
    /// The colour its paper is, as `[red, green, blue]`.
    pub background: [u8; 3],
    pub pixels: usize,
}

/// Looks at a rectangle of BGRA pixels.
pub fn sample_pixels(pixels: &[u8], stride: usize, left: usize, top: usize, right: usize, bottom: usize) -> ImageSample {
    let (mut paper, mut colour, mut white, mut count) = (0usize, 0usize, 0usize, 0usize);
    let mut sums = [0u64; 3];
    if stride > 0 {
        for row in pixels.chunks(stride).skip(top).take(bottom.saturating_sub(top)) {
            let Some(row) = row.get(left.saturating_mul(4)..right.saturating_mul(4).min(row.len())) else { continue };
            for pixel in row.as_chunks::<4>().0 {
                let (b, g, r) = (pixel[0], pixel[1], pixel[2]);
                let (low, high) = (r.min(g).min(b), r.max(g).max(b));
                count += 1;
                if low >= WHITE_LIGHTEST_CHANNEL && high - low <= WHITE_MOST_SPREAD {
                    white += 1;
                }
                if low >= PAPER_LIGHTEST_CHANNEL && high - low <= PAPER_MOST_SPREAD {
                    paper += 1;
                    sums[0] += u64::from(r);
                    sums[1] += u64::from(g);
                    sums[2] += u64::from(b);
                } else if high - low >= COLOUR_LEAST_SPREAD {
                    colour += 1;
                }
            }
        }
    }
    let share = |part: usize| if count == 0 { 0.0 } else { part as f64 / count as f64 };
    let average = |sum: u64| if paper == 0 { 255 } else { u8::try_from(sum / paper as u64).unwrap_or(255) };
    ImageSample { paper: share(paper), colour: share(colour), white: share(white), background: [average(sums[0]), average(sums[1]), average(sums[2])], pixels: count }
}

impl ImageSample {
    /// Whether it's an image with a white background, that turns over with the page. Uncertain is no.
    pub fn has_white_background(self) -> bool {
        self.pixels >= LEAST_PIXELS_TO_JUDGE && ((self.white >= WHITE_LEAST && self.colour <= WHITE_MOST_COLOUR) || (self.paper >= SCAN_LEAST_PAPER && self.colour <= SCAN_MOST_COLOUR))
    }

    /// The same, for a whole page with nothing known of its images. Stricter, and uncertain is still no.
    fn whole_page_is_white_backed(self) -> bool {
        self.has_white_background() && self.colour <= WHOLE_PAGE_MOST_COLOUR
    }

    /// What brings its paper to true white, in 256ths of each channel.
    fn gains(self) -> [u16; 3] {
        self.background.map(|channel| u16::try_from(255 * 256 / u32::from(channel.max(1))).unwrap_or(256))
    }
}

/// An image that turns over, and what brings its paper to white first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Whitened {
    pub sides: [u16; 4],
    pub gains: [u16; 3],
}

/// The one decision about a page on dark paper.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageLook {
    /// True colours, dimmed, all of it.
    pub dim: bool,
    /// Images left as they are, on a page that turns over.
    pub keep: Vec<[u16; 4]>,
    /// Images that turn over with it.
    pub whiten: Vec<Whitened>,
}

const WHOLE_PAGE: [u16; 4] = [0, 0, u16::MAX, u16::MAX];

/// The parts of a page these cover. Added up, so images that overlap count twice, and a page can't come to more than all of it.
fn share_of_page(boxes: impl Iterator<Item = [u16; 4]>) -> f64 {
    boxes.map(|sides| f64::from(sides[2].saturating_sub(sides[0])) * f64::from(sides[3].saturating_sub(sides[1])) / (65_535.0 * 65_535.0)).sum::<f64>().min(1.0)
}

impl PageLook {
    /// Nothing known about the page, and no way to find out: dimmed.
    pub fn unknown() -> Self {
        Self { dim: true, ..Self::default() }
    }

    /// A page with no images.
    pub fn text() -> Self {
        Self::default()
    }

    /// Decides from a small picture of the whole page, `width` by `height` of BGRA pixels as drawn for white paper. `boxes` is where its images are, if that's known, and with none known the page is judged whole.
    pub fn decide(boxes: Option<&[[u16; 4]]>, pixels: &[u8], stride: usize, width: u32, height: u32) -> Self {
        let sample = |sides: [u16; 4]| {
            let place = |part: u16, whole: u32| (u64::from(part) * u64::from(whole) / 65_535) as usize;
            sample_pixels(pixels, stride, place(sides[0], width), place(sides[1], height), place(sides[2], width).min(width as usize), place(sides[3], height).min(height as usize))
        };
        let Some(boxes) = boxes else {
            let whole = sample(WHOLE_PAGE);
            return if whole.whole_page_is_white_backed() { Self { dim: false, keep: Vec::new(), whiten: vec![Whitened { sides: WHOLE_PAGE, gains: whole.gains() }] } } else { Self::unknown() };
        };
        let (mut keep, mut whiten) = (Vec::new(), Vec::new());
        for &sides in boxes {
            let found = sample(sides);
            if found.has_white_background() {
                whiten.push(Whitened { sides, gains: found.gains() });
            } else {
                keep.push(sides);
            }
        }
        if share_of_page(keep.iter().copied()) >= MOSTLY_PICTURE {
            return Self { dim: true, ..Self::default() };
        }
        Self { dim: false, keep, whiten }
    }

    /// What it comes to in a rectangle of the page, whose size in device pixels at the rectangle's scale is `page`.
    pub fn in_tile(&self, page: Option<(u32, u32)>, rect: recto_proto::TileRect) -> DarkTile {
        let Some(page) = page else { return DarkTile::Dim };
        if self.dim {
            return DarkTile::Dim;
        }
        let whiten = self.whiten.iter().filter_map(|image| Some((keep_in(image.sides, page, rect)?, image.gains))).collect();
        DarkTile::TurnOver(self.keep.iter().filter_map(|sides| keep_in(*sides, page, rect)).collect(), whiten)
    }
}

/// A rectangle of a tile, in its pixels, to leave as a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keep {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// What dark paper does to a tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DarkTile {
    /// Turned over, but for pictures that stay, with the parts that are white-backed images brought to white first.
    TurnOver(Vec<Keep>, Vec<(Keep, [u16; 3])>),
    /// True colours, dimmed, as the page is a picture or it isn't known.
    Dim,
}

/// Where a part of a page falls in a rectangle of it. Rounds outward, so an image's edge isn't turned over.
fn keep_in(sides: [u16; 4], page: (u32, u32), rect: recto_proto::TileRect) -> Option<Keep> {
    let place = |part: u16, whole: u32, up: bool| -> u32 {
        let exact = u64::from(part) * u64::from(whole);
        (if up { exact.div_ceil(65_535) } else { exact / 65_535 }) as u32
    };
    let (left, top) = (place(sides[0], page.0, false), place(sides[1], page.1, false));
    let (right, bottom) = (place(sides[2], page.0, true), place(sides[3], page.1, true));
    let keep = Keep { left: left.max(rect.x) - rect.x, top: top.max(rect.y) - rect.y, right: right.min(rect.x + rect.width).saturating_sub(rect.x), bottom: bottom.min(rect.y + rect.height).saturating_sub(rect.y) };
    (keep.right > keep.left && keep.bottom > keep.top).then_some(keep)
}

/// Brings a pixel of a white-backed image up to white, so paper that's cream, grey or noisy all lands on the same tone, and what's dark stays dark.
#[inline]
fn whiten_pixel(pixel: &mut [u8; 4], gains: [u16; 3]) {
    let up = |value: u8, gain: u16| ((u32::from(value) * u32::from(gain)) >> 8).min(255);
    let (b, g, r) = (up(pixel[0], gains[2]), up(pixel[1], gains[1]), up(pixel[2], gains[0]));
    if b.min(g).min(r) >= 255 - NEAR_WHITE_SNAP {
        pixel[..3].fill(255);
    } else {
        pixel[0] = b as u8;
        pixel[1] = g as u8;
        pixel[2] = r as u8;
    }
}

/// One colour, dimmed as a picture page is.
pub fn dim_rgb(rgb: u32) -> u32 {
    let channel = |shift: u32| (((rgb >> shift) & 0xFF) * PICTURE_PAGE_DIM) >> 8;
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Dims a whole tile, in place: a picture page's. A multiply per channel, which costs less than turning a colour tile over. Bytes from a worker are only multiplied here.
pub fn dim_page(pixels: &mut [u8], width: u32, height: u32, stride: u32) {
    let (row_bytes, stride) = (width as usize * 4, stride as usize);
    if stride < row_bytes || stride == 0 {
        return;
    }
    for row in pixels.chunks_mut(stride).take(height as usize) {
        if let Some(row) = row.get_mut(..row_bytes) {
            for pixel in row.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = ((u32::from(*channel) * PICTURE_PAGE_DIM) >> 8) as u8;
                }
            }
        }
    }
}

/// Whether a tile's red, green and blue are equal, which lets [`turn_over`] use a table. A tile of one colour only is if that colour's grey.
pub fn is_grey(kind: TileKind) -> bool {
    match kind {
        TileKind::Grey => true,
        TileKind::Blank { rgb } => (rgb >> 16) & 0xFF == rgb & 0xFF && (rgb >> 8) & 0xFF == rgb & 0xFF,
        TileKind::Colour => false,
    }
}

/// Turns a tile's pixels over, in place, leaving `keeps` as pictures. Bytes come from a worker that may be hostile, and this only does arithmetic on them. `grey` says red, green and blue are equal, as the renderer found, which is the common text tile: the answer depends on one byte, so
/// a table does it. A compromised worker can say so falsely, and all that gets is a wrong picture, which it can draw anyway.
pub fn turn_over(pixels: &mut [u8], width: u32, height: u32, stride: u32, grey: bool, keeps: &[Keep], whiten: &[(Keep, [u16; 3])]) {
    let (row_bytes, stride) = (width as usize * 4, stride as usize);
    if stride < row_bytes || stride == 0 {
        return;
    }
    if grey && keeps.is_empty() && whiten.is_empty() {
        let mut table = [[0u8; 3]; 256];
        for (value, entry) in table.iter_mut().enumerate() {
            let mut pixel = [value as u8, value as u8, value as u8, 0xFF];
            turn_pixel(&mut pixel);
            *entry = [pixel[0], pixel[1], pixel[2]];
        }
        for row in pixels.chunks_mut(stride).take(height as usize) {
            if let Some(row) = row.get_mut(..row_bytes) {
                for pixel in row.as_chunks_mut::<4>().0 {
                    let [b, g, r] = table[usize::from(pixel[0])];
                    pixel[0] = b;
                    pixel[1] = g;
                    pixel[2] = r;
                }
            }
        }
        return;
    }
    for (y, row) in pixels.chunks_mut(stride).take(height as usize).enumerate() {
        let Some(row) = row.get_mut(..row_bytes) else { continue };
        let here: Vec<Keep> = keeps.iter().filter(|keep| (keep.top as usize..keep.bottom as usize).contains(&y)).copied().collect();
        let to_whiten: Vec<(Keep, [u16; 3])> = whiten.iter().filter(|(place, _)| (place.top as usize..place.bottom as usize).contains(&y)).copied().collect();
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if here.iter().any(|keep| (keep.left as usize..keep.right as usize).contains(&x)) {
                dim_pixel(pixel);
                continue;
            }
            if let Some((_, gains)) = to_whiten.iter().find(|(place, _)| (place.left as usize..place.right as usize).contains(&x)) {
                whiten_pixel(pixel, *gains);
            }
            turn_pixel(pixel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the compositor does to one channel under a highlight, in 0 to 1.
    fn blended(look: HighlightLook, under: f64, over: f64) -> f64 {
        if look.lighten { under.max(over) } else { under * over }
    }

    fn contrast(one: [f64; 3], two: [f64; 3]) -> f64 {
        let linear = |c: f64| if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let luminance = |rgb: [f64; 3]| 0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2]);
        let (a, b) = (luminance(one), luminance(two));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Failure: on some paper, text under a highlight can't be read, or the highlight doesn't show against the page. The system's own colours as macOS has them by default in each appearance, blue and graphite, on what each paper shows: white paper, warm paper, which is white under a tint, and dark.
    #[test]
    fn a_highlight_shows_and_leaves_text_readable_on_every_paper() {
        let rgb = |value: u32| [f64::from((value >> 16) & 0xFF) / 255.0, f64::from((value >> 8) & 0xFF) / 255.0, f64::from(value & 0xFF) / 255.0];
        let warm_tint = [0.96, 0.93, 0.86];
        type Case = (&'static str, PaperKind, u32, [u32; 2], Option<[f64; 3]>);
        let cases: [Case; 3] = [("white", PaperKind::White, 0xFF_FF_FF, [0xB3_D7_FF, 0xD9_D9_D9], None), ("warm", PaperKind::White, 0xFF_FF_FF, [0xB3_D7_FF, 0xD9_D9_D9], Some(warm_tint)), ("dark", PaperKind::Dark, DARK_PAPER, [0x3F_63_8B, 0x46_46_46], None)];
        for (name, kind, paper, colours, tint) in cases {
            let look = kind.highlight();
            assert_eq!(look.dark_colour, kind == PaperKind::Dark);
            let ink = if kind == PaperKind::Dark { [f64::from(DARK_INK[0]) / 255.0, f64::from(DARK_INK[1]) / 255.0, f64::from(DARK_INK[2]) / 255.0] } else { [0.0; 3] };
            let paper = rgb(paper);
            for colour in colours {
                let over = rgb(colour);
                let with = |under: [f64; 3]| {
                    let mixed = [blended(look, under[0], over[0]), blended(look, under[1], over[1]), blended(look, under[2], over[2])];
                    tint.map_or(mixed, |tint| [mixed[0] * tint[0], mixed[1] * tint[1], mixed[2] * tint[2]])
                };
                let shown_paper = tint.map_or(paper, |tint| [paper[0] * tint[0], paper[1] * tint[1], paper[2] * tint[2]]);
                assert!(contrast(with(ink), with(paper)) >= 4.5, "{name}, {colour:06x}: text is {:.1}:1 under it", contrast(with(ink), with(paper)));
                assert!(contrast(with(paper), shown_paper) >= 1.2, "{name}, {colour:06x}: it barely shows, {:.2}:1", contrast(with(paper), shown_paper));
            }
        }
    }

    fn luma(rgb: u32) -> u32 {
        (((rgb >> 16) & 0xFF) * 77 + ((rgb >> 8) & 0xFF) * 150 + (rgb & 0xFF) * 29) >> 8
    }

    /// Failure: dark paper isn't dark, or text on it isn't readable. Asserts the property, which is contrast, and not the exact colours.
    #[test]
    fn paper_goes_dark_and_ink_goes_light() {
        assert_eq!(dark_rgb(0xFF_FF_FF), DARK_PAPER, "white paper becomes the paper the page sits on, or tiles would show");
        let ink = luma(dark_rgb(0));
        assert!(ink > luma(DARK_PAPER) * 4, "{ink}");
    }

    /// Failure: a blue link becomes unreadable on dark paper, stays dark with a halo round its letters, or changes colour. On RFC 9000 this looked like blue letters in a light outline, because the letters' middles stayed dark while their edges went light. The steps from paper to ink must get steadily lighter, and stay blue.
    #[test]
    fn coloured_text_turns_over_and_keeps_its_colour() {
        let link = dark_rgb(0x00_00_EE);
        assert!(luma(link) > luma(DARK_PAPER) * 3, "{link:06x}");
        assert!(link & 0xFF > (link >> 16) & 0xFF, "still blue: {link:06x}");
        let mut last = luma(DARK_PAPER);
        for step in (0..=255u32).rev() {
            let edge = (step << 16) | (step << 8) | (step + (0xEE * (255 - step)) / 255).min(255);
            let now = luma(dark_rgb(edge));
            assert!(now + 2 >= last, "at {edge:06x}: {now}, after {last}");
            last = now;
        }
    }

    /// Failure: an image turns into its negative on dark paper, as the hillshade on the USGS map did. It stays itself, a little dimmed, while the page around it turns over.
    #[test]
    fn what_is_kept_stays_a_picture() {
        let mut pixels = vec![0xFFu8; 4 * 4 * 2];
        turn_over(&mut pixels, 4, 2, 16, true, &[Keep { left: 1, top: 0, right: 3, bottom: 1 }], &[]);
        let at = |x: usize, y: usize| pixels[(y * 16) + x * 4];
        assert!(at(1, 0) > 200 && at(2, 0) > 200, "inside: {}", at(1, 0));
        assert_eq!(u32::from(at(0, 0)), DARK_PAPER & 0xFF, "beside it");
        assert_eq!(u32::from(at(1, 1)), DARK_PAPER & 0xFF, "below it");
    }

    /// Failure: an image's place misses the tile it's in, or a box from a hostile worker writes outside the tile. Holds the parts of a page to its pixels, at the tile's own origin.
    #[test]
    fn a_box_lands_where_it_should_in_a_tile() {
        let rect = recto_proto::TileRect { x: 512, y: 0, width: 512, height: 512 };
        let half = keep_in([0, 0, 32_768, 32_768], (2048, 2048), rect);
        assert_eq!(half, Some(Keep { left: 0, top: 0, right: 512, bottom: 512 }), "to the middle of the page, past this tile");
        assert_eq!(keep_in([60_000, 60_000, 65_535, 65_535], (2048, 2048), rect), None, "elsewhere on the page");
        assert_eq!(keep_in([65_535, 65_535, 0, 0], (2048, 2048), rect), None, "inside out");
        let edge_to_edge = keep_in([0, 0, 40_000, 40_000], (u32::MAX, u32::MAX), rect).unwrap();
        assert!(edge_to_edge.right <= 512 && edge_to_edge.bottom <= 512);
    }

    /// A small picture of a page, BGRA, from what's at each place.
    fn picture(width: u32, height: u32, at: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = at(x, y);
                pixels.extend_from_slice(&[b, g, r, 0xFF]);
            }
        }
        pixels
    }

    fn scan(paper: [u8; 3]) -> Vec<u8> {
        picture(100, 100, |_, y| if y % 5 == 0 { [40, 40, 40] } else { paper })
    }

    /// Failure: a scanned page or embedded letter shows as a bright block with a hard edge against dark paper, as the owner saw. Off-white, yellowed or with a stamp on it, it turns over. Failure the other way: a map or a photo goes negative, as the USGS map's band went mint. It's dimmed, keeping its colours. Owner, 2026-11-07, for both.
    #[test]
    fn a_page_of_white_background_turns_over_and_a_picture_page_is_dimmed() {
        let whole = Some(&[[0u16, 0, 65_535, 65_535]][..]);
        for paper in [[255, 255, 255], [240, 232, 206], [226, 226, 226]] {
            let look = PageLook::decide(whole, &scan(paper), 400, 100, 100);
            assert!(!look.dim && look.keep.is_empty() && look.whiten.len() == 1, "{paper:?}: {look:?}");
        }
        let stamped = picture(100, 100, |x, y| if x > 85 && y > 85 { [200, 30, 30] } else if y % 5 == 0 { [40, 40, 40] } else { [250, 250, 250] });
        assert!(!PageLook::decide(whole, &stamped, 400, 100, 100).dim, "a red stamp in the corner is still a scan");
        let photo = picture(100, 100, |x, y| [(x * 2) as u8 + 40, (y * 2) as u8 + 20, 90]);
        assert!(PageLook::decide(whole, &photo, 400, 100, 100).dim, "a photo");
        let map_with_margins = picture(100, 100, |x, y| if !(15..=85).contains(&x) || !(10..=90).contains(&y) { [255, 255, 255] } else { [90 + (x as u8), 140, 70] });
        assert!(PageLook::decide(None, &map_with_margins, 400, 100, 100).dim, "white margins don't make a map a scan");
        let many = picture(100, 100, |x, y| if (x / 10 + y / 10) % 2 == 0 { [200, 40, 40] } else { [255, 255, 255] });
        assert!(PageLook::decide(whole, &many, 400, 100, 100).dim, "a coloured image half of it white");
    }

    /// Failure: a small picture on a page of text goes negative, or a white-backed one stays a bright box. Each is judged by itself, and it makes the page dim only if what stays adds up to most of the page.
    #[test]
    fn each_image_is_judged_by_itself() {
        let page = picture(100, 100, |x, y| if x < 40 && y < 40 { [80, 160, 60] } else { [255, 255, 255] });
        let boxes = [[0u16, 0, 26_214, 26_214], [39_321, 39_321, 65_535, 65_535]];
        let look = PageLook::decide(Some(&boxes), &page, 400, 100, 100);
        assert!(!look.dim, "{look:?}");
        assert_eq!(look.keep, vec![boxes[0]], "the coloured one stays");
        assert_eq!(look.whiten.iter().map(|image| image.sides).collect::<Vec<_>>(), vec![boxes[1]], "the white one turns over");
        assert_eq!(PageLook::decide(Some(&[]), &page, 400, 100, 100), PageLook::text());
        let tiny = PageLook::decide(Some(&[[0, 0, 300, 300]]), &page, 400, 100, 100);
        assert_eq!(tiny.keep.len(), 1, "too small to say, so it's left alone: {tiny:?}");
    }

    /// Failure: a scan's off-white paper lands on a muddy tone beside a true text page, or its noise speckles, and a tile crossing the image's edge shows a seam. Whitened first, its paper is the paper's colour exactly, with the same result as white paper turned over, and its ink stays light.
    #[test]
    fn a_scans_paper_lands_on_the_papers_tone() {
        let look = PageLook::decide(Some(&[[0, 0, 65_535, 65_535]]), &scan([236, 226, 196]), 400, 100, 100);
        let rect = recto_proto::TileRect { x: 0, y: 0, width: 3, height: 1 };
        let DarkTile::TurnOver(keeps, whiten) = look.in_tile(Some((100, 100)), rect) else { panic!("{look:?}") };
        // Its paper, a little noisier than the average, and its ink, as blue-green-red.
        let mut pixels = [196, 226, 236, 0xFF, 190, 222, 233, 0xFF, 40, 40, 40, 0xFF];
        turn_over(&mut pixels, 3, 1, 12, false, &keeps, &whiten);
        let rgb = |pixel: &[u8]| (u32::from(pixel[2]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[0]);
        assert_eq!(rgb(&pixels[0..3]), DARK_PAPER, "its paper");
        assert_eq!(rgb(&pixels[4..7]), DARK_PAPER, "and the noise in it");
        assert!(luma(rgb(&pixels[8..11])) > luma(DARK_PAPER) * 3, "its ink reads light");
    }

    /// Failure: where the images are isn't known, for a page whose renderer can't be asked or an answer that failed, and "unknown" reads as "none", so a photo comes out as a negative and stays cached as one. With no picture to judge it's dimmed, and with one it's judged whole, the same as a sharp page.
    #[test]
    fn what_isnt_known_is_never_turned_over() {
        let rect = recto_proto::TileRect { x: 0, y: 0, width: 2, height: 1 };
        assert_eq!(PageLook::unknown().in_tile(Some((2048, 2048)), rect), DarkTile::Dim);
        assert_eq!(PageLook::text().in_tile(None, rect), DarkTile::Dim, "the page's size wasn't known");
        let whole = PageLook::decide(None, &scan([250, 250, 250]), 400, 100, 100);
        assert!(!whole.dim, "the low-resolution path, judged whole: {whole:?}");
    }

    /// Failure: a picture page's colours change, or it's left as bright as it was, or black text on it goes unreadable. Each channel falls by the same share, and black stays black.
    #[test]
    fn dimming_keeps_a_pictures_colours_and_takes_the_glare_off() {
        let mut pixels = [0x20, 0x40, 0xC0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0xFF];
        dim_page(&mut pixels, 3, 1, 12);
        assert!(pixels[2] > pixels[1] && pixels[1] > pixels[0], "still red: {pixels:?}");
        assert_eq!(u32::from(pixels[6]), (0xFF * PICTURE_PAGE_DIM) >> 8);
        assert!(pixels[6] < 0xE0, "the glare's off");
        assert_eq!(&pixels[8..11], &[0, 0, 0], "black stays black");
        assert_eq!(dim_rgb(0xFF_FF_FF) & 0xFF, u32::from(pixels[6]), "one colour and many agree");
        // WCAG's ratio of black on the dimmed white, with sRGB's curve. AA for body text is 4.5.
        let linear = (f64::from(pixels[6]) / 255.0 + 0.055) / 1.055;
        let contrast = (linear.powf(2.4) + 0.05) / 0.05;
        assert!(contrast >= 4.5, "{contrast}");
    }

    /// Failure: a preview of one colour goes through the table for grey, which reads one channel only, so solid red comes out near white rather than turned over as red. Only a grey one may.
    #[test]
    fn one_colour_only_counts_as_grey_if_it_is() {
        assert!(is_grey(TileKind::Grey));
        assert!(is_grey(TileKind::Blank { rgb: 0x77_77_77 }));
        assert!(!is_grey(TileKind::Blank { rgb: 0xFF_00_00 }));
        assert!(!is_grey(TileKind::Colour));
        let mut pixel = [0x00, 0x00, 0xFF, 0xFF];
        turn_over(&mut pixel, 1, 1, 4, is_grey(TileKind::Blank { rgb: 0xFF_00_00 }), &[], &[]);
        let turned = (u32::from(pixel[2]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[0]);
        assert_eq!(turned, dark_rgb(0xFF_00_00));
        assert!(pixel[2] > pixel[0] && pixel[2] > pixel[1], "still red: {pixel:?}");
    }

    /// Failure: the table the grey path uses disagrees with the path for colour, so a grey tile beside a colour tile shows a seam.
    #[test]
    fn grey_tiles_and_colour_tiles_agree() {
        let mut one: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v, 0xFF]).collect();
        let mut two = one.clone();
        turn_over(&mut one, 256, 1, 1024, true, &[], &[]);
        turn_over(&mut two, 256, 1, 1024, false, &[], &[]);
        assert_eq!(one, two);
    }

    /// Failure: a hostile stride panics or writes outside a row.
    #[test]
    fn a_bad_stride_changes_nothing_or_stays_in_bounds() {
        let mut pixels = vec![9u8; 64];
        turn_over(&mut pixels, 100, 100, 4, false, &[], &[]);
        assert_eq!(pixels, vec![9u8; 64], "a stride narrower than the row");
        turn_over(&mut pixels, 8, 100, 0, true, &[], &[]);
        turn_over(&mut pixels, 4, 100, 16, true, &[], &[]);
    }

    /// Measures, and isn't a check: `cargo test --release -p recto-reader -- --ignored dark_paper_costs --nocapture`. What a 512 px colour tile costs each way.
    #[test]
    #[ignore = "a measurement"]
    fn dark_paper_costs() {
        let tile = |seed: u8| (0..512 * 512 * 4).map(|index| (index as u8).wrapping_mul(seed)).collect::<Vec<u8>>();
        let time = |name: &str, step: &dyn Fn(&mut [u8])| {
            let mut pixels = tile(31);
            let mut samples: Vec<u128> = (0..200)
                .map(|_| {
                    let started = std::time::Instant::now();
                    step(&mut pixels);
                    started.elapsed().as_micros()
                })
                .collect();
            samples.sort_unstable();
            println!("{name}: p50 {} us, p95 {} us", samples[100], samples[190]);
        };
        time("turn over", &|pixels| turn_over(pixels, 512, 512, 2048, false, &[], &[]));
        time("whiten and turn over", &|pixels| turn_over(pixels, 512, 512, 2048, false, &[], &[(Keep { left: 0, top: 0, right: 512, bottom: 512 }, [270, 280, 300])]));
        let page = tile(31);
        time("judge a page picture", &|_| {
            let _ = PageLook::decide(Some(&[[0, 0, 65_535, 65_535]]), &page, 2048, 512, 512);
        });
        time("dim page", &|pixels| dim_page(pixels, 512, 512, 2048));
    }
}
