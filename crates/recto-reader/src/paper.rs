//! Dark paper, as a change to a tile's pixels (owner, 2026-11-06: dark mode offers dark or white paper).
//!
//! It's done to the pixels rather than by a filter on the layer because a filter makes the compositor draw every frame through an offscreen pass, and because a QA shot wouldn't see it: `CALayer.render(in:)` leaves
//! filters out. A tile redrawn for another paper is just another tile.
//!
//! **The rule,** from `docs/concepts/reader-dark.png`: lightness turns over, so white paper goes dark and black text goes light, and each pixel keeps its colour's distance from grey, so blue text is lighter blue with no halo round its letters. Images on a page of text aren't turned over: the renderer says where they are, and they stay as they were, a little dimmed.
//!
//! **Each image is judged by its own pixels** (owner, 2026-11-07: "We need to detect scanned images (with predominently white bg, and flip and match grey tone paper color", and "similar with images with white bg"). One with a mostly white background, a scanned page, an embedded letter, a chart, turns over with the page, its background brought to true white first so it lands on the paper's tone, with no bright box or hard edge. One without stays as it is on a page of text.
//!
//! **A page whose own background is already dark is never turned over, whatever pictures it has** (owner, 2026-11-13: "I agree with your recommendation on dark images"). It's shown as it is, dimmed by [`PICTURE_PAGE_DIM`] as a picture page is ([`PageLook::dark_page`]), as the owner was shown it on a navy page: a black page dimmed is the same black, and a navy one a deeper navy, with its white text taken off glaring. Turned over, a navy page came out light blue, a black cover a white page and a dark slide deck light, the opposite of what dark appearance is for. **This is asked first**, of every page, with images or none, and before the rules below, which decide only for a page whose background isn't dark, as they always have. In the code:
//! - **The background** is everything on the page but the pictures that are kept (review of 2026-11-13, B1): its paper, and its white-backed pictures, a chart, a scan, read as they are, which is mostly white, as that's what makes them white-backed. Or the whole page when its images aren't known, or it has none. A white-backed picture taken out of it let a dark band or footer beside it be most of what was left, and dimmed a page of white chart to grey.
//! - **It's dark** by [`Lightness::is_mostly_dark`], beside [`ImageSample::is_mostly_white`]: at least [`DARK_LEAST`], half, of it darker than [`DARK_MOST_LUMA`], 128 of 255 in the luma [`turn_pixel`] turns over, with enough of it to judge, [`LEAST_BACKGROUND`] of the page as for white, **and at least half of its ring dark too, the outer [`RING_WIDTH`], a twentieth, of each side** (second review of 2026-11-14): a dark page is dark to its edges, and a white page with something dark on it has white margins, or white down a side. So a black, navy, dark green or dark grey slide is dark, and so is one with a white text box or a light table over a third of it, or a photo under half of it. **What's true of a white page with a dark part:** a dark header band, sidebar or footer isn't dark, whatever's beside it, a photo or white-backed pictures, as its ring is mostly white: a sidebar a third wide beside a photo over 45% is on the line (its ring 0.48 to 0.51 dark at the look picture's size, as the photo takes the white side out of it), and a little wider is dark. A dark figure drawn in vectors over half a white page, a chart, a code listing, isn't dark either, its ring its white margins, and turns over with its page. A scan with a black scanner border has a dark ring but under half of it dark, and turns over. Cream, a pale tint, a yellowed scan, light grey or a middle tone over 128 aren't dark, and turn over as before. A page half dark and half light is on the line: a little more dark than light, to its edges, is dark.
//! - **Middle tones** under 128, a deep orange, a mid blue, a mid grey, count as dark: turned over they come out lighter than they were, and dimmed reads better (the sheets of 2026-11-13, at `DARK_MOST_LUMA`).
//! - **It's read from a grid** of about 128 points along the picture's longest side ([`sample_lightness`]), not every pixel: the same grid at every size of picture, so a page's verdict doesn't depend on which picture it's decided from, and about 12,700 looks on a letter page.
//! - **A large drawing in vectors on a white page** isn't asked about separately: it turns over with its page, as built, unless it runs off the page's edges and half of the page is dark.
//!
//! **A page that's mostly a picture turns over only if its background is mostly white** (owner, 2026-11-07, answering "should pages that are mostly a picture be dimmed instead of flipped? I recommend yes": "I agree"; then 2026-11-13, shown a photo over 53% of a white page with text dimmed whole to light grey: "The image should flip if the background is mostly white"). Which pages the background decides for is the coordinator's reading, the conservative one, which the owner may change. In the code:
//! - **Mostly a picture** is [`MOSTLY_PICTURE`] of the page or more, covered by the pictures that are kept and large enough to judge, together: where they overlap, as a photo and its drop shadow do, it counts once. It's the same measure as the background's, the part of the picture of the page outside every kept picture's box. **Under it, a page whose background isn't dark turns over whatever its background,** as it always has, keeping its pictures: a slide on a light coloured background with a logo, a report page with a coloured band beside a photo, a page on cream. A picture under [`LEAST_PIXELS_TO_JUDGE`] in the picture the look is decided from, a spacer, a box with no width, is left alone where it is and is no picture for this.
//! - **The background** is the page outside the boxes of the pictures that are kept: its own paper, and any white-backed pictures on it, as for the dark test.
//! - **It's mostly white** when it's mostly white by [`ImageSample::is_mostly_white`], the one definition, which the test for a white-backed image is built on too: at least [`WHITE_LEAST`] of it plain white, each channel at least [`WHITE_LIGHTEST_CHANNEL`] and at most [`WHITE_MOST_SPREAD`] apart, or at least [`SCAN_LEAST_PAPER`] of it paper-like, as a scan's yellowed or grey paper is. It's the background the dark test reads, the page's paper and its white-backed pictures read as they are (second review of 2026-11-14: read as only the paper outside every image's box, a white-backed chart over 30% beside a photo over 55% left a dark title band over 15% as all of it, and the page was dimmed whole, its chart grey-white). An image must also have little colour, as a photo with a white half is a photo; the page's background needn't, as its kept pictures are taken out of it, and what colour is left is the page's own, its headings, coloured bands and table fills, which turn over with it on a page with no picture, and a white-backed chart's bars.
//! - **Mostly a picture on a mostly white background: the page turns over** however large its picture: paper dark, text light, and a photo on it keeps its own colours, a little dimmed, as under half. A picture whose own background is mostly white turns over with it (2026-11-07, unchanged).
//! - **Mostly a picture with too little background to judge: the page is a picture,** and isn't turned over. A full-page photograph or a map edge to edge, or a picture with all but a margin under [`LEAST_BACKGROUND`] of the page, a twentieth, which is a margin of about 3 mm round a letter page, keeps its true colours dimmed by [`PICTURE_PAGE_DIM`] so it doesn't glare.
//! - **Mostly a picture on a coloured background isn't turned over either:** the whole page is dimmed, true colours, the same as a page that's a picture, since turning it over would turn a page that isn't white round it.
//! - **Several pictures** are one background between them: each is judged by itself, what they cover together is their share, and the background is what's outside the kept ones.
//! - **A page with no picture kept** turns over as before, whatever its background but a dark one: a page of text, a large drawing in vectors, which has no picture to keep, and a page of white-backed pictures.
//!
//! **What the pictures cover is measured in two steps, each an approximation.** The worker gives each image's box, and on a page past 64 images gives images whose boxes overlap or share an edge as one picture, the box round them (`recto-render`'s `places.rs`), so a picture cut in strips, a map with insets laid over it, or a mosaic of photos edge to edge is one picture there, and its box can take in paper between its images: a step at 64. Here, each of those boxes is judged by itself, and the page's share of picture is what the kept ones cover together in the picture the look is decided from, an overlap counted once, the same measure the background is the rest of. So the worker's merging decides what a picture is, and the union decides how much of the page the pictures are.
//!
//! **When a page's images can't be told apart, it's judged whole, and what's uncertain is dimmed,** as an image turned over is a negative, and a page dimmed is only brighter than it might have been. That's unchanged: the page with a photo over 53% of it, judged whole, is dimmed, as its photo is too much colour for a page of white. A page is judged whole only when its images can't be had: a page whose drawing failed for time or a crash, an asking that was stopped, or a page with more pictures apart than an answer holds, too many to tell apart; a page drawn at a lower resolution is asked as any page is (`recto-render-service`, `image_boxes`). A look refused a worker isn't decided at all, and so isn't remembered, nor is one whose picture failed for a reason that may pass, which is tried again at the page's next job (`engine/look.rs`). There's one [`PageLook`] a page while its renderer lives, so its tiles, preview, lower-resolution version and the picture it sleeps as can't disagree. **Across a renderer dropped and made again, as a tab sleeps and wakes, the look is decided again,** from whichever picture of the whole page comes first: the preview, if the job drew one; a tile that's the whole page, for a page with no images (zoomed out past where a preview is drawn); or a small picture drawn for it, the look picture, 512 pixels long, for a page with images, and 128 for one with none, as all that's asked of it is whether it's dark (`engine/look.rs`). What's decided can differ only in part: whether the page turns over or is dimmed doesn't hang on which picture it's decided from (0 of 1,291 pages differed between pictures 1,144, 512 and 128 pixels tall, `tests/look_survey.rs`, `sizes`), but whether one image on it turns over with the page or is kept can (7 of 292 pages with images between the look picture and the largest preview, 28 at a lower resolution's quarter), so such an image can change across a sleep and a wake. **Its limits:** a photo inside a scan turns over with it, which is accepted. A chart drawn as vectors on a page of text turns over with the text, since pixels can't say what they belong to.

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
            Self::White => HighlightLook {
                dark_colour: false,
                lighten: false,
            },
            Self::Dark => HighlightLook {
                dark_colour: true,
                lighten: true,
            },
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
    [
        ((DARK_PAPER >> 16) & 0xFF) as i32,
        ((DARK_PAPER >> 8) & 0xFF) as i32,
        (DARK_PAPER & 0xFF) as i32,
    ]
}

/// **How turned-over text keeps its weight** (owner, 2026-11-07: "The font in the future competitiveness is not super readible. I think its a bit to skinny"). On white paper the renderer's text is what Preview draws: PDFium draws glyphs through Core Graphics on a Mac, and at the same scale their ink is the same to four places. Turned over in sRGB's encoding, though, a letter's anti-aliased edge gives off much less light above dark paper than it took from white, so light text came out thinner than black, and thinner than macOS draws its own light text: in three system fonts at the reader's scale, 0.70 to 0.78 of the light per letter against 0.84 to 0.88 for Core Text's own light-on-dark. Paper-ness goes through this power first, so an edge gives off more: at 1.8 the turned-over text gives off 0.98 to 1.00 of what Core Text's own does. White and black stay where they were, so paper and ink are unchanged.
const TURNED_LIGHTNESS_POWER: f64 = 1.8;
/// **The curve is for text and lines** (owner, 2026-11-08: coloured areas in flipped images keep the gentler curve, so charts keep richer colours). A pixel whose channels are this close is grey, as black text's edges, a scan's text and rules are, and takes all of the curve. This far apart is colour, as a chart's bars and fills are, and turns straight, as before the curve. Between, the curve fades, so a coloured letter's edge has no step. Figure 8's bars are 96 to 219 apart.
const CURVE_GREY_SPREAD: i32 = 16;
const CURVE_COLOUR_SPREAD: i32 = 64;

/// How much of the curve a pixel takes, in 256ths, for each spread of its channels. A table, as a division a pixel doubled a colour tile's cost.
const CURVE_WEIGHT: [i32; 256] = {
    let mut table = [0i32; 256];
    let mut spread = 0;
    while spread < 256 {
        let weight = (CURVE_COLOUR_SPREAD - spread as i32) * 256 / (CURVE_COLOUR_SPREAD - CURVE_GREY_SPREAD);
        table[spread] = if weight < 0 {
            0
        } else if weight > 256 {
            256
        } else {
            weight
        };
        spread += 1;
    }
    table
};

/// Paper-ness after [`TURNED_LIGHTNESS_POWER`], for each value of it. Made once.
static TURNED_LIGHTNESS: std::sync::LazyLock<[u8; 256]> =
    std::sync::LazyLock::new(|| std::array::from_fn(|value| (255.0 * (value as f64 / 255.0).powf(TURNED_LIGHTNESS_POWER)).round().clamp(0.0, 255.0) as u8));

/// Turns one colour over, as a blank tile's single colour needs.
pub fn dark_rgb(rgb: u32) -> u32 {
    let mut pixel = [(rgb & 0xFF) as u8, ((rgb >> 8) & 0xFF) as u8, ((rgb >> 16) & 0xFF) as u8, 0xFF];
    turn_pixel(&mut pixel, &TURNED_LIGHTNESS);
    (u32::from(pixel[2]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[0])
}

/// **A colour's lightness, once:** Rec. 601's weights in 256ths, 0 black and 255 white, at most 255 as the weights add up to 256. What [`turn_pixel`] turns over, and what [`sample_lightness`] calls dark under [`DARK_MOST_LUMA`].
#[inline]
fn luma(r: u8, g: u8, b: u8) -> i32 {
    (77 * i32::from(r) + 150 * i32::from(g) + 29 * i32::from(b)) >> 8
}

#[inline]
fn turn_pixel(pixel: &mut [u8; 4], lightness: &[u8; 256]) {
    let [paper_r, paper_g, paper_b] = paper_channels();
    let (b, g, r) = (i32::from(pixel[0]), i32::from(pixel[1]), i32::from(pixel[2]));
    let luma = luma(pixel[2], pixel[1], pixel[0]);
    // Text and lines are near grey and take the curve; colour, as a chart's fills, turns straight, with the curve fading out between.
    let spread = r.max(g).max(b) - r.min(g).min(b);
    let paperness = if spread >= CURVE_COLOUR_SPREAD {
        luma
    } else {
        luma + (((i32::from(lightness[(luma as usize) & 0xFF]) - luma) * CURVE_WEIGHT[(spread as usize) & 0xFF]) >> 8)
    };
    let over = |paper: i32, ink: i32, value: i32| (((paper * paperness + ink * (255 - paperness)) / 255) + value - luma).clamp(0, 255) as u8;
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

/// How much of a page, covered by the pictures kept on it that are large enough to judge, an overlap counted once, makes its background decide: half, which is what "mostly a picture" says (owner, 2026-11-07). Under it a page turns over whatever its background, as it always has. Measured over `bench100`: USGS map sheets 0.59 to 1.0, with their legends and titles in vectors around the map, and figures on arXiv pages of text 0.16 to 0.26.
pub const MOSTLY_PICTURE: f64 = 0.5;
/// The least background, the part of a page outside the pictures kept on it, that can be judged at all: a twentieth of the page, which round a letter page is a margin of about 3 mm. With less, the page is a picture edge to edge and is dimmed, whatever colour that sliver is: a printer's margin round a full-page photograph isn't a page of white. A photo over 90% of a white page, a margin of about 6 mm on each side, is judged, and turns over.
pub const LEAST_BACKGROUND: f64 = 0.05;
/// **A pixel is dark** when its lightness, the Rec. 601 luma [`turn_pixel`] turns over (0 black, 255 white), is under this: darker than the middle, where turning a colour over makes it lighter than it was. Chosen by looking at pages of grey, blue, orange, red and green at 60, 90, 115, 135, 155 and 175, each turned over and dimmed in the real app (2026-11-13): to 115 a dimmed page reads better in every hue, as turned over it goes light; at 175 turned over does, as it comes out darker than dimmed; at 135 and 155 they're alike, but for a grey at 135, which turned over is light text on light grey, as a grey takes text's curve. Higher, at 150, a white page whose coloured band and sidebar are most of what's outside its photo (`a_page_with_a_coloured_band_and_sidebar_and_a_small_photo_turns_over`) counts as dark, and stops turning over.
pub const DARK_MOST_LUMA: i32 = 128;
/// **A background is dark** when at least this much of it is dark pixels: most of it, so a white page with a dark band or sidebar isn't, and a dark slide with a white text box is.
pub const DARK_LEAST: f64 = 0.5;
/// **And its edges are dark: the ring,** the outer [`RING_WIDTH`] of each side of the page, kept pictures left out, at least [`DARK_LEAST`] of it dark too, as "mostly" means for the background (second review of 2026-11-14, the reviewer's proposal, taken on the survey). A page that's dark is dark to its edges, a slide's or a cover's colour running off it, whatever's laid on it, a photo, a white text box, a table; a white page with something dark on it has white margins, or white down at least one side. Most of the background dark alone couldn't tell them apart beside a kept photo or over a dark figure: a white page with a photo over 45% and a dark sidebar over 30% has 0.57 of what the photo leaves dark, and a white page with a dark chart or code listing drawn in vectors over half of it is mostly dark, and both were dimmed whole, white margins grey with dark text. Their rings are 0.41 dark (the sidebar's side and its share of the top and the foot) and white, and they turn over; a navy slide with a photo and a white box has a navy ring, and stays dark. A scan with a black scanner border has a dark ring, but under half its background is dark, so it turns over, as it did. The share of the page dark it replaces, 0.35, told the sidebar page from a navy page with a photo over 39% by 0.035 (review of 2026-11-13).
pub const RING_WIDTH: f64 = 0.05;
/// How many pixels along a picture's longest side a background's darkness is read from: every pixel at most this many apart, so whether a page is dark is read from about the same grid, about 128 by 99 on a letter page, whatever the size of the picture it's decided from (the largest preview's 1,144, the look picture's 512, its lower resolution's 128), and costs the same at each.
const DARK_GRID: usize = 128;
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
/// **The third kind, a chart or diagram whose colour is flat** (owner, 2026-11-07: "images with a white background should be dark, as a chart with a white background was left light"). Its bars, slices and fills are a few colours each, so most of its colour falls in a handful of [`COLOUR_BINS`]: [`FLAT_LEAST_CLUSTERED`] of it in the [`FLAT_TOP_BINS`] fullest. A photo, a rendering or a heatmap shades from one colour to the next and spreads over many. Over `bench100`'s 4,149 images that stayed and part B of the EU competitiveness report, every one past this with a white or plain background was a chart, a table, a diagram, a logo or a page of a document, and the photos, renderings and heatmaps with as much white were under 0.80. With this, the background can be white, as in the first kind, or one plain light colour, [`PLAIN_LEAST`] of it within [`PLAIN_NEAR`] of its average, as a chart on beige is. A map on tan isn't: its land is shaded.
const FLAT_MOST_COLOUR: f64 = 0.45;
const FLAT_LEAST_CLUSTERED: f64 = 0.80;
const FLAT_TOP_BINS: usize = 4;
/// Colour is binned by the top 3 bits of each channel, so a JPEG's noise round a bar's colour stays in its bin.
const COLOUR_BINS: usize = 512;
const PLAIN_LEAST: f64 = 0.60;
/// See [`is_shading`].
const SHADE_LEAST_SHARE: f64 = 0.08;
const SHADE_LEAST_LEVELS: usize = 4;
const PLAIN_NEAR: u8 = 10;
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
    /// The part of its colour in its [`FLAT_TOP_BINS`] fullest bins: near 1 for a chart's flat fills, low for a photo. 1 with no colour.
    pub clustered: f64,
    /// The part of it within [`PLAIN_NEAR`] of its paper's average colour, in every channel. Only looked for when it decides something, and 0 otherwise.
    pub plain: f64,
    /// Its colour is one colour shaded through several lightnesses, as a photographed or rendered object's is, and not flat fills. See [`is_shading`].
    pub shaded: bool,
}

/// A rectangle of a picture in its pixels, `[left, top, right, bottom]`, right and bottom not in it.
type PixelBox = [usize; 4];

/// Calls `run` with each stretch `[from, to)` of row `y` between `left` and `right` that none of `holes` covers. `holes` are sorted by their left side, so one walk along the row does it, with nothing made. The places come from a worker, so a hole inside out covers nothing, and the stretches come in order and never overlap, whatever the holes: what's walked is at most the row.
fn gaps_in_row(holes: &[PixelBox], y: usize, left: usize, right: usize, mut run: impl FnMut(usize, usize)) {
    let mut at = left;
    for hole in holes.iter().filter(|hole| hole[0] < hole[2] && (hole[1]..hole[3]).contains(&y)) {
        if at >= right {
            return;
        }
        if hole[0] > at {
            run(at, hole[0].min(right));
        }
        at = at.max(hole[2]);
    }
    if at < right {
        run(at, right);
    }
}

/// Looks at a rectangle of BGRA pixels. One pass, and a second over the same rectangle only for an image the third kind's plain background would decide.
pub fn sample_pixels(pixels: &[u8], stride: usize, left: usize, top: usize, right: usize, bottom: usize) -> ImageSample {
    sample_outside(pixels, stride, [left, top, right, bottom], &[])
}

/// Calls `look` with each stretch of the BGRA pixels of `within` that none of `holes` covers, row by row. Nothing's kept between calls, so a second pass walks the rows again rather than holding a slice for each stretch, which with 64 holes came to a mebibyte for the largest preview.
fn for_each_stretch(pixels: &[u8], stride: usize, within: PixelBox, holes: &[PixelBox], mut look: impl FnMut(&[u8])) {
    let [left, top, right, bottom] = within;
    if stride == 0 {
        return;
    }
    for (y, row) in pixels.chunks(stride).enumerate().skip(top).take(bottom.saturating_sub(top)) {
        gaps_in_row(holes, y, left, right, |from, to| {
            if let Some(stretch) = row.get(from.saturating_mul(4)..to.saturating_mul(4).min(row.len())) {
                look(stretch);
            }
        });
    }
}

/// The same, for the pixels of `within` that none of `holes` covers, as a page's paper outside its images is. `holes` must be sorted by their left side.
fn sample_outside(pixels: &[u8], stride: usize, within: PixelBox, holes: &[PixelBox]) -> ImageSample {
    let (mut paper, mut colour, mut white, mut count) = (0usize, 0usize, 0usize, 0usize);
    let mut sums = [0u64; 3];
    let mut bins = [0u32; COLOUR_BINS];
    for_each_stretch(pixels, stride, within, holes, |stretch| {
        for pixel in stretch.as_chunks::<4>().0 {
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
                bins[(usize::from(r >> 5) << 6) | (usize::from(g >> 5) << 3) | usize::from(b >> 5)] += 1;
            }
        }
    });
    let share = |part: usize| if count == 0 { 0.0 } else { part as f64 / count as f64 };
    let average = |sum: u64| if paper == 0 { 255 } else { u8::try_from(sum / paper as u64).unwrap_or(255) };
    let background = [average(sums[0]), average(sums[1]), average(sums[2])];
    // The fullest bins, as `(count, bin)`, fullest first.
    let mut fullest = [(0u32, 0usize); FLAT_TOP_BINS];
    for (index, &bin) in bins.iter().enumerate() {
        if bin > fullest[FLAT_TOP_BINS - 1].0 {
            fullest[FLAT_TOP_BINS - 1] = (bin, index);
            fullest.sort_unstable_by_key(|(count, _)| std::cmp::Reverse(*count));
        }
    }
    let clustered = if colour == 0 {
        1.0
    } else {
        fullest.iter().map(|(bin, _)| f64::from(*bin)).sum::<f64>() / colour as f64
    };
    let shaded = colour > 0 && is_shading(&fullest, colour);
    let mut found = ImageSample {
        paper: share(paper),
        colour: share(colour),
        white: share(white),
        background,
        pixels: count,
        clustered,
        plain: 0.0,
        shaded,
    };
    if found.white < WHITE_LEAST
        && found.paper >= SCAN_LEAST_PAPER
        && found.colour > SCAN_MOST_COLOUR
        && found.colour <= FLAT_MOST_COLOUR
        && found.clustered >= FLAT_LEAST_CLUSTERED
    {
        let near = |value: u8, to: u8| value.abs_diff(to) <= PLAIN_NEAR;
        let mut plain = 0usize;
        for_each_stretch(pixels, stride, within, holes, |stretch| {
            plain += stretch
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| near(pixel[2], background[0]) && near(pixel[1], background[1]) && near(pixel[0], background[2]))
                .count();
        });
        found.plain = share(plain);
    }
    found
}

/// Whether an image's fullest colour bins are one colour shaded, as a ball or a product photographed on white is: of the bins that hold at least [`SHADE_LEAST_SHARE`] of its colour, the ones joined to the fullest through neighbours run through [`SHADE_LEAST_LEVELS`] lightnesses or more. A chart's flat fill has its edges' and noise's in a bin or two beside it: of the 41 images the chart rule added in bench100 and part B, 40 span at most 3, and the 41st is a gradient strip, rightly dimmed; shaded balls in red, blue and green span 4.
fn is_shading(fullest: &[(u32, usize); FLAT_TOP_BINS], colour: usize) -> bool {
    let cell = |bin: usize| [(bin >> 6) as i32, ((bin >> 3) & 7) as i32, (bin & 7) as i32];
    let counted: Vec<usize> = fullest
        .iter()
        .filter(|(count, _)| f64::from(*count) >= SHADE_LEAST_SHARE * colour as f64)
        .map(|(_, bin)| *bin)
        .collect();
    let Some(&first) = counted.first() else { return false };
    // The bins joined to the fullest through neighbours, and the lightnesses they span.
    let mut joined = vec![first];
    let mut at = 0;
    while at < joined.len() {
        let here = cell(joined[at]);
        for &other in &counted {
            let there = cell(other);
            if !joined.contains(&other) && (0..3).all(|channel| (here[channel] - there[channel]).abs() <= 1) {
                joined.push(other);
            }
        }
        at += 1;
    }
    let mut levels: Vec<i32> = joined.iter().map(|bin| cell(*bin).iter().sum()).collect();
    levels.sort_unstable();
    levels.dedup();
    levels.len() >= SHADE_LEAST_LEVELS
}

impl ImageSample {
    /// **What "mostly white" means, once:** at least [`WHITE_LEAST`] of it plain white, or at least [`SCAN_LEAST_PAPER`] of it paper-like. A page's background is mostly white by this alone (owner, 2026-11-13); an image is white-backed by it with little colour besides, in [`Self::has_white_background`]. What "dark" means is beside it, [`Lightness::is_mostly_dark`].
    pub fn is_mostly_white(self) -> bool {
        self.pixels >= LEAST_PIXELS_TO_JUDGE && (self.is_plain_white() || self.is_paper())
    }

    fn is_plain_white(self) -> bool {
        self.white >= WHITE_LEAST
    }

    fn is_paper(self) -> bool {
        self.paper >= SCAN_LEAST_PAPER
    }

    /// Whether it's an image with a white background, that turns over with the page: mostly white, with no more colour than its kind allows. Uncertain is no.
    pub fn has_white_background(self) -> bool {
        let flat = self.colour <= FLAT_MOST_COLOUR
            && self.clustered >= FLAT_LEAST_CLUSTERED
            && !self.shaded
            && (self.is_plain_white() || (self.is_paper() && self.plain >= PLAIN_LEAST));
        self.is_mostly_white() && ((self.is_plain_white() && self.colour <= WHITE_MOST_COLOUR) || (self.is_paper() && self.colour <= SCAN_MOST_COLOUR) || flat)
    }

    /// The same, for a whole page with nothing known of its images. Stricter, and uncertain is still no.
    fn whole_page_is_white_backed(self) -> bool {
        self.has_white_background() && self.colour <= WHOLE_PAGE_MOST_COLOUR
    }

    /// What brings its paper to true white, in 256ths of each channel.
    fn gains(self) -> [u16; 3] {
        self.background
            .map(|channel| u16::try_from(255 * 256 / u32::from(channel.max(1))).unwrap_or(256))
    }
}

/// How dark a page's background is: what's found at the points of a grid [`DARK_GRID`] along the picture's longest side that none of the holes given covers, the boxes of the page's kept pictures, and of those, the ones in its ring ([`RING_WIDTH`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lightness {
    /// The part of the points looked at that are dark, by [`DARK_MOST_LUMA`].
    pub dark: f64,
    /// How many points were looked at.
    pub points: usize,
    /// The part of the grid they are: how much of the page is background.
    pub of_page: f64,
    /// The part of the points in the ring that are dark.
    pub ring: f64,
    /// How many points of the ring were looked at.
    pub ring_points: usize,
}

impl Lightness {
    /// **What "dark" means, once:** at least [`DARK_LEAST`] of the background dark, with enough of it to judge, [`LEAST_BACKGROUND`] of the page, as for white, and at least [`DARK_LEAST`] of its ring. A ring all under kept pictures says nothing, and the background decides alone.
    pub fn is_mostly_dark(self) -> bool {
        self.points >= LEAST_PIXELS_TO_JUDGE
            && self.of_page >= LEAST_BACKGROUND
            && self.dark >= DARK_LEAST
            && (self.ring_points < LEAST_PIXELS_TO_JUDGE || self.ring >= DARK_LEAST)
    }
}

/// Reads how dark a picture of a page `width` by `height` is outside `holes` (sorted by left side), at one pixel in each `step` by `step` square, the step making about [`DARK_GRID`] points along the longest side. Which pixel of its square is scattered, row by row and column by column, so the grid doesn't fall in step with lines of text, which every `step`th row would: on a page with a line every 6 pixels and a step of 2 it counted each line twice over. One look and a compare a point, about 12,700 points on a letter page at any size, so it costs about the same from the largest preview as from the look picture. The bytes and the size come from a worker: what isn't there isn't looked at.
pub fn sample_lightness(pixels: &[u8], stride: usize, width: u32, height: u32, holes: &[PixelBox]) -> Lightness {
    let (width, height) = (width as usize, height as usize);
    if stride == 0 || width == 0 || height == 0 {
        return Lightness::default();
    }
    let step = (width.max(height) / DARK_GRID).max(1);
    // How far in from each side the ring goes, in pixels, at least one.
    let ring_across = ((width as f64 * RING_WIDTH).ceil() as usize).max(1);
    let ring_down = ((height as f64 * RING_WIDTH).ceil() as usize).max(1);
    // Where in its stretch of `step` the point of stretch `at` is: the same every time, and not in step with anything on a page.
    let scatter = |at: usize| (((at as u64).wrapping_mul(2_654_435_761) >> 16) % step as u64) as usize;
    let (mut dark, mut points, mut grid, mut ring_dark, mut ring_points) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for band in 0..height.div_ceil(step) {
        let y = band * step + scatter(band);
        let Some(row) = pixels.get(y.saturating_mul(stride)..).filter(|_| y < height) else {
            continue;
        };
        let row = &row[..row.len().min(stride)];
        let offset = scatter(y);
        grid += width.saturating_sub(offset).div_ceil(step);
        let ring_row = y < ring_down || y >= height.saturating_sub(ring_down);
        gaps_in_row(holes, y, 0, width, |from, to| {
            let mut x = from + (offset + step - from % step) % step;
            while x < to {
                let Some(&[b, g, r]) = row.get(x * 4..x * 4 + 3) else { return };
                let is_dark = luma(r, g, b) < DARK_MOST_LUMA;
                points += 1;
                dark += usize::from(is_dark);
                if ring_row || x < ring_across || x >= width.saturating_sub(ring_across) {
                    ring_points += 1;
                    ring_dark += usize::from(is_dark);
                }
                x += step;
            }
        });
    }
    let share = |part: usize, whole: usize| if whole == 0 { 0.0 } else { part as f64 / whole as f64 };
    Lightness {
        dark: share(dark, points),
        points,
        of_page: share(points, grid),
        ring: share(ring_dark, ring_points),
        ring_points,
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

/// Where a part of a page, `[left, top, right, bottom]` in 65535ths, falls in `width` by `height` pixels: each side rounded down, or with the right and bottom rounded up when `outward`, so an image's anti-aliased edge goes with the image. Never past the size given, as no side is past 65,535.
fn in_pixels(sides: [u16; 4], width: u32, height: u32, outward: bool) -> [u64; 4] {
    let place = |part: u16, whole: u32, up: bool| {
        let exact = u64::from(part) * u64::from(whole);
        if up { exact.div_ceil(65_535) } else { exact / 65_535 }
    };
    [
        place(sides[0], width, false),
        place(sides[1], height, false),
        place(sides[2], width, outward),
        place(sides[3], height, outward),
    ]
}

/// Where parts of a page fall in a picture of it `width` by `height`, rounded outward, so an image's anti-aliased edge isn't taken for the page's paper. Sorted by left side, as [`gaps_in_row`] wants.
fn pixel_boxes(boxes: impl Iterator<Item = [u16; 4]>, width: u32, height: u32) -> Vec<PixelBox> {
    let mut found: Vec<PixelBox> = boxes.map(|sides| in_pixels(sides, width, height, true).map(|side| side as usize)).collect();
    found.sort_unstable_by_key(|hole| hole[0]);
    found
}

/// The part of a picture `width` by `height` that none of `holes` covers. Images that overlap count once.
fn share_outside(holes: &[PixelBox], width: u32, height: u32) -> f64 {
    let (width, height) = (width as usize, height as usize);
    if width == 0 || height == 0 {
        return 0.0;
    }
    let mut outside = 0usize;
    for y in 0..height {
        gaps_in_row(holes, y, 0, width, |from, to| outside += to - from);
    }
    outside as f64 / (width * height) as f64
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

    /// **What a page gets whose picture can't be had** (no size, a draw that failed for good, or one that kept failing for a reason that may pass): one with no images a page of text's look, as before 2026-11-13, as nothing on it can come out a negative and most such pages are text, so a dark slide among them is the one that turns over; one with images, or whose images aren't known, [`Self::unknown`], as an image turned over is a negative and a page dimmed is only brighter than it might have been.
    pub fn undecided(image_less: bool) -> Self {
        if image_less { Self::text() } else { Self::unknown() }
    }

    /// A page whose own background is dark: shown as it is, dimmed as a picture page is, never turned over.
    pub fn dark_page() -> Self {
        Self { dim: true, ..Self::default() }
    }

    /// Decides from a small picture of the whole page, `width` by `height` of BGRA pixels as drawn for white paper. `boxes` is where its images are, if that's known, and with none known the page is judged whole. With its images known, each is judged by itself first. Then a page whose own background is dark, everything but its kept pictures, is [`Self::dark_page`], whatever else is on it (owner, 2026-11-13). Otherwise a page whose kept pictures cover [`MOSTLY_PICTURE`] of it or more between them turns over only if its background is mostly white (owner, 2026-11-13).
    pub fn decide(boxes: Option<&[[u16; 4]]>, pixels: &[u8], stride: usize, width: u32, height: u32) -> Self {
        let sample = |sides: [u16; 4]| {
            let [left, top, right, bottom] = in_pixels(sides, width, height, false).map(|side| side as usize);
            sample_pixels(pixels, stride, left, top, right, bottom)
        };
        let Some(boxes) = boxes else {
            // Judged whole, the whole page is its background.
            if sample_lightness(pixels, stride, width, height, &[]).is_mostly_dark() {
                return Self::dark_page();
            }
            let whole = sample(WHOLE_PAGE);
            return if whole.whole_page_is_white_backed() {
                Self {
                    dim: false,
                    keep: Vec::new(),
                    whiten: vec![Whitened {
                        sides: WHOLE_PAGE,
                        gains: whole.gains(),
                    }],
                }
            } else {
                Self::unknown()
            };
        };
        let (mut keep, mut whiten, mut pictures) = (Vec::new(), Vec::new(), Vec::new());
        for &sides in boxes {
            let found = sample(sides);
            if found.has_white_background() {
                whiten.push(Whitened { sides, gains: found.gains() });
            } else {
                keep.push(sides);
                // Too small to judge is left alone where it is, and is no picture for what the page does.
                if found.pixels >= LEAST_PIXELS_TO_JUDGE {
                    pictures.push(sides);
                }
            }
        }
        // How much of the page is picture, and how much is background, are one measure: the part of the picture outside every kept picture's box, where pictures that overlap, as a photo and its drop shadow do, count once.
        let pictured = pixel_boxes(pictures.iter().copied(), width, height);
        // A dark background first: such a page is never turned over, whatever pictures it has, nor how large. Its background is everything but its kept pictures, its white-backed pictures with its paper, as light as they are (review of 2026-11-13, B1).
        if sample_lightness(pixels, stride, width, height, &pictured).is_mostly_dark() {
            return Self::dark_page();
        }
        let outside = share_outside(&pictured, width, height);
        if 1.0 - outside < MOSTLY_PICTURE || Self::background_is_white(outside, &pictured, pixels, stride, width, height) {
            return Self { dim: false, keep, whiten };
        }
        // Most of the page a picture, edge to edge or on a background that isn't white: true colours, dimmed.
        Self { dim: true, ..Self::default() }
    }

    /// Whether a page's background, the part of it outside the pictures that stay as they are, their boxes `pictured`, `outside` of it, is mostly white, as the module's doc says: enough of it to judge, and mostly white by [`ImageSample::is_mostly_white`]. It's the background the dark test reads, the page's paper and its white-backed pictures as they are (second review of 2026-11-14: read as only the paper outside every box, a white-backed chart over 30% beside a photo over 55% left a dark title band over 15% as all of it, and the page was dimmed whole, its chart grey-white).
    fn background_is_white(outside: f64, pictured: &[PixelBox], pixels: &[u8], stride: usize, width: u32, height: u32) -> bool {
        outside >= LEAST_BACKGROUND && sample_outside(pixels, stride, [0, 0, width as usize, height as usize], pictured).is_mostly_white()
    }

    /// What it comes to in a rectangle of the page, whose size in device pixels at the rectangle's scale is `page`.
    pub fn in_tile(&self, page: Option<(u32, u32)>, rect: recto_proto::TileRect) -> DarkTile {
        let Some(page) = page else { return DarkTile::Dim };
        if self.dim {
            return DarkTile::Dim;
        }
        let whiten = self
            .whiten
            .iter()
            .filter_map(|image| Some((keep_in(image.sides, page, rect)?, image.gains)))
            .collect();
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
    let [left, top, right, bottom] = in_pixels(sides, page.0, page.1, true).map(|side| side as u32);
    let keep = Keep {
        left: left.max(rect.x) - rect.x,
        top: top.max(rect.y) - rect.y,
        right: right.min(rect.x + rect.width).saturating_sub(rect.x),
        bottom: bottom.min(rect.y + rect.height).saturating_sub(rect.y),
    };
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
    let lightness = &*TURNED_LIGHTNESS;
    if grey && keeps.is_empty() && whiten.is_empty() {
        let mut table = [[0u8; 3]; 256];
        for (value, entry) in table.iter_mut().enumerate() {
            let mut pixel = [value as u8, value as u8, value as u8, 0xFF];
            turn_pixel(&mut pixel, lightness);
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
    // The last colour turned and what it came to: a page's tile is mostly runs of one colour, its paper, so most pixels are a look and a copy.
    let mut last: ([u8; 3], [u8; 3]) = ([0xFF; 3], {
        let mut white = [0xFF, 0xFF, 0xFF, 0xFF];
        turn_pixel(&mut white, lightness);
        [white[0], white[1], white[2]]
    });
    for (y, row) in pixels.chunks_mut(stride).take(height as usize).enumerate() {
        let Some(row) = row.get_mut(..row_bytes) else { continue };
        let here: Vec<Keep> = keeps
            .iter()
            .filter(|keep| (keep.top as usize..keep.bottom as usize).contains(&y))
            .copied()
            .collect();
        let to_whiten: Vec<(Keep, [u16; 3])> = whiten
            .iter()
            .filter(|(place, _)| (place.top as usize..place.bottom as usize).contains(&y))
            .copied()
            .collect();
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if here.iter().any(|keep| (keep.left as usize..keep.right as usize).contains(&x)) {
                dim_pixel(pixel);
                continue;
            }
            if let Some((_, gains)) = to_whiten.iter().find(|(place, _)| (place.left as usize..place.right as usize).contains(&x)) {
                whiten_pixel(pixel, *gains);
            }
            let colour = [pixel[0], pixel[1], pixel[2]];
            if colour == last.0 {
                pixel[..3].copy_from_slice(&last.1);
            } else {
                turn_pixel(pixel, lightness);
                last = (colour, [pixel[0], pixel[1], pixel[2]]);
            }
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
        let rgb = |value: u32| {
            [
                f64::from((value >> 16) & 0xFF) / 255.0,
                f64::from((value >> 8) & 0xFF) / 255.0,
                f64::from(value & 0xFF) / 255.0,
            ]
        };
        let warm_tint = [0.96, 0.93, 0.86];
        type Case = (&'static str, PaperKind, u32, [u32; 2], Option<[f64; 3]>);
        let cases: [Case; 3] = [
            ("white", PaperKind::White, 0xFF_FF_FF, [0xB3_D7_FF, 0xD9_D9_D9], None),
            ("warm", PaperKind::White, 0xFF_FF_FF, [0xB3_D7_FF, 0xD9_D9_D9], Some(warm_tint)),
            ("dark", PaperKind::Dark, DARK_PAPER, [0x3F_63_8B, 0x46_46_46], None),
        ];
        for (name, kind, paper, colours, tint) in cases {
            let look = kind.highlight();
            assert_eq!(look.dark_colour, kind == PaperKind::Dark);
            let ink = if kind == PaperKind::Dark {
                [f64::from(DARK_INK[0]) / 255.0, f64::from(DARK_INK[1]) / 255.0, f64::from(DARK_INK[2]) / 255.0]
            } else {
                [0.0; 3]
            };
            let paper = rgb(paper);
            for colour in colours {
                let over = rgb(colour);
                let with = |under: [f64; 3]| {
                    let mixed = [
                        blended(look, under[0], over[0]),
                        blended(look, under[1], over[1]),
                        blended(look, under[2], over[2]),
                    ];
                    tint.map_or(mixed, |tint| [mixed[0] * tint[0], mixed[1] * tint[1], mixed[2] * tint[2]])
                };
                let shown_paper = tint.map_or(paper, |tint| [paper[0] * tint[0], paper[1] * tint[1], paper[2] * tint[2]]);
                assert!(
                    contrast(with(ink), with(paper)) >= 4.5,
                    "{name}, {colour:06x}: text is {:.1}:1 under it",
                    contrast(with(ink), with(paper))
                );
                assert!(
                    contrast(with(paper), shown_paper) >= 1.2,
                    "{name}, {colour:06x}: it barely shows, {:.2}:1",
                    contrast(with(paper), shown_paper)
                );
            }
        }
    }

    /// The lightness of a colour `0xRRGGBB`, by the one definition.
    fn luma(rgb: u32) -> u32 {
        super::luma((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8) as u32
    }

    /// Failure: dark paper isn't dark, or text on it isn't readable. Asserts the property, which is contrast, and not the exact colours.
    #[test]
    fn paper_goes_dark_and_ink_goes_light() {
        assert_eq!(
            dark_rgb(0xFF_FF_FF),
            DARK_PAPER,
            "white paper becomes the paper the page sits on, or tiles would show"
        );
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

    /// Failure: text on dark paper is thinner than the same text on white, as the owner found the EU competitiveness report's light sans (2026-11-07: "I think its a bit to skinny"). Turning lightness over in sRGB's encoding leaves a letter's anti-aliased edge giving off far less light above dark paper, in linear light, than it took from white paper, and the letter looks thinner, where macOS draws its own light text heavier. Over every grey an edge can be, what's turned over must give off at least 0.7 of the light the edge took from white: a straight turn gives 0.50.
    #[test]
    fn text_keeps_its_weight_on_dark_paper() {
        let linear = |value: u32| {
            let channel = f64::from(value) / 255.0;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        let light = |rgb: u32| 0.2126 * linear((rgb >> 16) & 0xFF) + 0.7152 * linear((rgb >> 8) & 0xFF) + 0.0722 * linear(rgb & 0xFF);
        let (paper, ink) = (light(DARK_PAPER), light(dark_rgb(0)));
        let (mut taken, mut given) = (0.0, 0.0);
        for grey in 0..=255u32 {
            taken += 1.0 - linear(grey);
            given += (light(dark_rgb(grey * 0x01_01_01)) - paper) / (ink - paper);
        }
        assert!(
            given / taken >= 0.7,
            "turned-over edges give off {:.2} of what they took from white",
            given / taken
        );
    }

    /// Failure: a chart's colours fade on dark paper, as figure 8's deep blue bars went pale lavender once lightness was curved for text (owner, 2026-11-08: the curve is for text and lines; coloured areas keep the gentler one). A colour turns over as it did before the curve, a grey takes the curve, and between them the curve fades in, so an anti-aliased edge has no step.
    #[test]
    fn colours_keep_the_straight_turn_and_greys_take_the_curve() {
        let straight = |rgb: u32| {
            let (r, g, b) = (((rgb >> 16) & 0xFF) as i32, ((rgb >> 8) & 0xFF) as i32, (rgb & 0xFF) as i32);
            let luma = (77 * r + 150 * g + 29 * b) >> 8;
            let [pr, pg, pb] = paper_channels();
            let over = |paper: i32, ink: i32, value: i32| (((paper * luma + ink * (255 - luma)) / 255) + value - luma).clamp(0, 255) as u32;
            (over(pr, DARK_INK[0], r) << 16) | (over(pg, DARK_INK[1], g) << 8) | over(pb, DARK_INK[2], b)
        };
        for colour in [0x15_20_F0, 0x4A_8A_C6, 0xC0_30_30, 0x20_80_30] {
            assert_eq!(dark_rgb(colour), straight(colour), "{colour:06x} keeps its colour");
        }
        assert!(luma(dark_rgb(0x80_80_80)) > luma(straight(0x80_80_80)) + 20, "a grey takes the curve");
        let mut last = dark_rgb(0x80_80_80);
        for step in 1..=64u32 {
            let edge = (0x80 << 16) | (0x80 << 8) | (0x80 + step * 2).min(255);
            let now = dark_rgb(edge);
            assert!(luma(now).abs_diff(luma(last)) <= 6, "a step at {edge:06x}: {:06x} after {:06x}", now, last);
            last = now;
        }
    }

    /// Failure: an image turns into its negative on dark paper, as the hillshade on the USGS map did. It stays itself, a little dimmed, while the page around it turns over.
    #[test]
    fn what_is_kept_stays_a_picture() {
        let mut pixels = vec![0xFFu8; 4 * 4 * 2];
        turn_over(
            &mut pixels,
            4,
            2,
            16,
            true,
            &[Keep {
                left: 1,
                top: 0,
                right: 3,
                bottom: 1,
            }],
            &[],
        );
        let at = |x: usize, y: usize| pixels[(y * 16) + x * 4];
        assert!(at(1, 0) > 200 && at(2, 0) > 200, "inside: {}", at(1, 0));
        assert_eq!(u32::from(at(0, 0)), DARK_PAPER & 0xFF, "beside it");
        assert_eq!(u32::from(at(1, 1)), DARK_PAPER & 0xFF, "below it");
    }

    /// Failure: an image's place misses the tile it's in, or a box from a hostile worker writes outside the tile. Holds the parts of a page to its pixels, at the tile's own origin.
    #[test]
    fn a_box_lands_where_it_should_in_a_tile() {
        let rect = recto_proto::TileRect {
            x: 512,
            y: 0,
            width: 512,
            height: 512,
        };
        let half = keep_in([0, 0, 32_768, 32_768], (2048, 2048), rect);
        assert_eq!(
            half,
            Some(Keep {
                left: 0,
                top: 0,
                right: 512,
                bottom: 512
            }),
            "to the middle of the page, past this tile"
        );
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
        let stamped = picture(100, 100, |x, y| {
            if x > 85 && y > 85 {
                [200, 30, 30]
            } else if y % 5 == 0 {
                [40, 40, 40]
            } else {
                [250, 250, 250]
            }
        });
        assert!(
            !PageLook::decide(whole, &stamped, 400, 100, 100).dim,
            "a red stamp in the corner is still a scan"
        );
        let photo = picture(100, 100, |x, y| [(x * 2) as u8 + 40, (y * 2) as u8 + 20, 90]);
        assert!(PageLook::decide(whole, &photo, 400, 100, 100).dim, "a photo");
        let map_with_margins = picture(100, 100, |x, y| {
            if !(15..=85).contains(&x) || !(10..=90).contains(&y) {
                [255, 255, 255]
            } else {
                [90 + (x as u8), 140, 70]
            }
        });
        assert!(
            PageLook::decide(None, &map_with_margins, 400, 100, 100).dim,
            "white margins don't make a map a scan"
        );
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
        assert_eq!(
            look.whiten.iter().map(|image| image.sides).collect::<Vec<_>>(),
            vec![boxes[1]],
            "the white one turns over"
        );
        assert_eq!(PageLook::decide(Some(&[]), &page, 400, 100, 100), PageLook::text());
        let tiny = PageLook::decide(Some(&[[0, 0, 300, 300]]), &page, 400, 100, 100);
        assert_eq!(tiny.keep.len(), 1, "too small to say, so it's left alone: {tiny:?}");
    }

    /// Failure: a bar chart on white stays a light box on dark paper because its bars are a quarter of it, as the owner saw in the EU competitiveness report's figure 8 (2026-11-07: "images with a white background should be dark"), or one on beige does. Failure the other way: a photo beside as much white, or a map on shaded tan, goes negative. Flat fills turn over, shading stays.
    #[test]
    fn a_chart_turns_over_and_a_photo_with_as_much_white_does_not() {
        let whole = Some(&[[0u16, 0, 65_535, 65_535]][..]);
        let bars = |paper: [u8; 3], tall: u32| {
            picture(100, 100, move |x, y| match x {
                10..25 if y >= 95 - tall => [21, 32, 240],
                35..50 if y >= 95 - tall / 2 => [74, 138, 198],
                60..75 if y >= 95 - tall => [21, 32, 240],
                _ if y == 96 => [80, 80, 80],
                _ => paper,
            })
        };
        let chart = bars([255, 255, 255], 60);
        let found = sample_pixels(&chart, 400, 0, 0, 100, 100);
        assert!(
            found.colour > WHITE_MOST_COLOUR,
            "more colour than the first kind allows, as figure 8 has: {found:?}"
        );
        let look = PageLook::decide(whole, &chart, 400, 100, 100);
        assert!(!look.dim && look.whiten.len() == 1, "a chart on white: {look:?}");
        let look = PageLook::decide(whole, &bars([234, 232, 227], 45), 400, 100, 100);
        assert!(!look.dim && look.whiten.len() == 1, "a chart on beige: {look:?}");
        let photo = picture(100, 100, |x, y| {
            if x < 50 {
                [255, 255, 255]
            } else {
                [((x - 50) * 4) as u8 + 40, (y * 2) as u8 + 20, 90]
            }
        });
        let found = sample_pixels(&photo, 400, 0, 0, 100, 100);
        assert!(
            found.white >= WHITE_LEAST && found.colour <= FLAT_MOST_COLOUR,
            "as white as a chart, and no more colour: {found:?}"
        );
        assert!(PageLook::decide(whole, &photo, 400, 100, 100).dim, "a photo beside white");
        let tan_map = picture(100, 100, |x, y| {
            if (x + y) % 9 == 0 {
                [40, 90, 200]
            } else {
                [200 + ((x * 7 + y * 3) % 50) as u8, 190 + ((x * 3 + y * 5) % 40) as u8, 160]
            }
        });
        assert!(PageLook::decide(whole, &tan_map, 400, 100, 100).dim, "a map on shaded tan");
    }

    /// Failure: a one-colour object photographed or rendered on white, shaded from light to dark, turns negative on dark paper because its colour falls in a few neighbouring bins, as a chart's flat fills do (review of e3fc6e0). Shading runs through several lightnesses of one colour with none much fuller than the rest; a flat fill has one bin that holds most of it.
    #[test]
    fn a_shaded_object_on_white_stays_true() {
        let whole = Some(&[[0u16, 0, 65_535, 65_535]][..]);
        for colour in [[200u32, 30, 30], [30, 60, 200], [40, 160, 60]] {
            // A ball lit from the top left, shaded to a quarter, with a little noise.
            let ball = picture(140, 140, |x, y| {
                let (dx, dy) = (x as f32 - 70.0, y as f32 - 70.0);
                if dx * dx + dy * dy >= 50.0 * 50.0 {
                    return [255, 255, 255];
                }
                let light = (1.0 - ((x as f32 - 55.0).powi(2) + (y as f32 - 55.0).powi(2)).sqrt() / 75.0).clamp(0.25, 1.0);
                let noise = ((x * 7 + y * 13) % 9) as f32 - 4.0;
                colour.map(|channel| (channel as f32 * light + noise).clamp(0.0, 255.0) as u8)
            });
            let found = sample_pixels(&ball, 560, 0, 0, 140, 140);
            assert!(
                found.white >= WHITE_LEAST && found.colour > WHITE_MOST_COLOUR && found.colour <= FLAT_MOST_COLOUR,
                "as white as a chart, with more colour than the first kind allows: {found:?}"
            );
            assert!(PageLook::decide(whole, &ball, 560, 140, 140).dim, "{colour:?}: {found:?}");
        }
    }

    /// Failure: a scan's off-white paper lands on a muddy tone beside a true text page, or its noise speckles, and a tile crossing the image's edge shows a seam. Whitened first, its paper is the paper's colour exactly, with the same result as white paper turned over, and its ink stays light.
    #[test]
    fn a_scans_paper_lands_on_the_papers_tone() {
        let look = PageLook::decide(Some(&[[0, 0, 65_535, 65_535]]), &scan([236, 226, 196]), 400, 100, 100);
        let rect = recto_proto::TileRect {
            x: 0,
            y: 0,
            width: 3,
            height: 1,
        };
        let DarkTile::TurnOver(keeps, whiten) = look.in_tile(Some((100, 100)), rect) else {
            panic!("{look:?}")
        };
        // Its paper, a little noisier than the average, and its ink, as blue-green-red.
        let mut pixels = [196, 226, 236, 0xFF, 190, 222, 233, 0xFF, 40, 40, 40, 0xFF];
        turn_over(&mut pixels, 3, 1, 12, false, &keeps, &whiten);
        let rgb = |pixel: &[u8]| (u32::from(pixel[2]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[0]);
        assert_eq!(rgb(&pixels[0..3]), DARK_PAPER, "its paper");
        assert_eq!(rgb(&pixels[4..7]), DARK_PAPER, "and the noise in it");
        assert!(luma(rgb(&pixels[8..11])) > luma(DARK_PAPER) * 3, "its ink reads light");
    }

    /// A small picture of a letter page, as the look is decided from, 200 by 260.
    const PAGE: (u32, u32) = (200, 260);
    const PAGE_STRIDE: usize = PAGE.0 as usize * 4;

    /// What's on a made page, from the top: a photo, which is shaded colour and no white, a chart, which is white with a few flat blue bars, a band of colour drawn on the page, shaded through many blues and purples, as a cover's or a heading's band is, or a photo's drop shadow, an image of dark grey.
    #[derive(Clone, Copy)]
    enum Part {
        Photo,
        Chart,
        Band,
        Shadow,
        /// A flat near-black fill, as a dark figure, band or slide is.
        Dark,
        /// A flat near-white fill, as a text box or a table is.
        Light,
        /// A scan of a page of text: off-white paper with a line of dark grey text every few rows, as a white-backed image is.
        Scan,
    }

    /// A made page: `parts` on a `background`, with a line of `text` every few rows across its middle where nothing else is, inside a margin a twentieth of the page deep at the top and the foot, as a page's text is. Places are parts of the page, `[left, top, right, bottom]`.
    fn made_page(parts: &[([f64; 4], Part)], background: [u8; 3], text: [u8; 3]) -> Vec<u8> {
        let (width, height) = (f64::from(PAGE.0), f64::from(PAGE.1));
        picture(PAGE.0, PAGE.1, |x, y| {
            let (fx, fy) = (f64::from(x), f64::from(y));
            for (place, part) in parts {
                let [left, top, right, bottom] = *place;
                if fx >= left * width && fx < right * width && fy >= top * height && fy < bottom * height {
                    return match part {
                        Part::Photo if y % 40 < 22 => [70 + (x % 60) as u8, 130 + (y % 50) as u8, 220 - (x % 40) as u8],
                        Part::Photo => [30 + (x % 20) as u8, 90 + (x * 3 % 70) as u8, 40],
                        Part::Band => [30 + (x * 7 % 160) as u8, 60 + (y * 5 % 120) as u8, 200],
                        Part::Shadow => [40, 40, 45],
                        Part::Dark => [20, 24, 32],
                        Part::Light => [250, 250, 250],
                        Part::Scan if y % 7 < 2 && x % 9 != 0 => [60, 60, 62],
                        Part::Scan => [246, 244, 239],
                        Part::Chart => {
                            let (inside_x, inside_y) = ((fx - left * width) as u32, (fy - top * height) / ((bottom - top) * height));
                            if (inside_x / 12) % 3 == 0 && inside_y > 0.6 {
                                [21, 32, 240]
                            } else {
                                [255, 255, 255]
                            }
                        }
                    };
                }
            }
            if y % 6 == 0 && (20..180).contains(&x) && (14..246).contains(&y) { text } else { background }
        })
    }

    /// A place on a page as the renderer gives it, in 65,535ths.
    fn sides(place: [f64; 4]) -> [u16; 4] {
        place.map(|side| (side * 65_535.0).round() as u16)
    }

    /// The page as dark paper shows it, by its look.
    fn shown(look: &PageLook, mut pixels: Vec<u8>) -> Vec<u8> {
        let rect = recto_proto::TileRect {
            x: 0,
            y: 0,
            width: PAGE.0,
            height: PAGE.1,
        };
        match look.in_tile(Some(PAGE), rect) {
            DarkTile::Dim => dim_page(&mut pixels, PAGE.0, PAGE.1, PAGE_STRIDE as u32),
            DarkTile::TurnOver(keeps, whiten) => turn_over(&mut pixels, PAGE.0, PAGE.1, PAGE_STRIDE as u32, false, &keeps, &whiten),
        }
        pixels
    }

    /// A pixel of a made page, as `0xRRGGBB`.
    fn pixel_at(pixels: &[u8], x: usize, y: usize) -> u32 {
        let at = y * PAGE_STRIDE + x * 4;
        (u32::from(pixels[at + 2]) << 16) | (u32::from(pixels[at + 1]) << 8) | u32::from(pixels[at])
    }

    const WHITE: [u8; 3] = [255, 255, 255];
    const INK: [u8; 3] = [50, 50, 50];
    /// Colours a page's background can be that aren't dark, lightness about 180, with dark text.
    const LIGHT_ORANGE: [u8; 3] = [250, 170, 60];
    const SKY_BLUE: [u8; 3] = [140, 190, 230];
    const PALE_GREEN: [u8; 3] = [150, 210, 150];
    const NAVY: [u8; 3] = [30, 60, 110];
    const LIGHT_TEXT: [u8; 3] = [235, 235, 235];

    /// A photo centred on a page, covering `share` of it.
    fn centred_photo(share: f64) -> [f64; 4] {
        let width = if share > 0.8 { 0.95 } else { 0.85 };
        let height = share / width;
        [(1.0 - width) / 2.0, (1.0 - height) / 2.0, (1.0 + width) / 2.0, (1.0 + height) / 2.0]
    }

    /// Failure: on dark paper a photo over more than half of a white page with text dims the whole page to light grey with black text, as the owner was shown at 53% (2026-11-13: "The image should flip if the background is mostly white"). However large the photo, while the page round it is white the paper goes dark, the text light, and the photo keeps its colours, only a little dimmed. 39% is how it was before, kept; 53%, 70% and 90% were dimmed whole by the rule before.
    #[test]
    fn a_photo_on_a_white_page_turns_the_page_over_however_large_it_is() {
        for share in [0.39, 0.53, 0.70, 0.90] {
            let photo = centred_photo(share);
            let pixels = made_page(&[(photo, Part::Photo)], WHITE, INK);
            let boxes = [sides(photo)];
            let look = PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(!look.dim && look.keep == boxes && look.whiten.is_empty(), "{share}: {look:?}");
            let after = shown(&look, pixels.clone());
            // The top left corner is paper at every share, row 18 a line of text above the photo but at 90%, and the middle is the photo.
            assert!(
                luma(pixel_at(&after, 1, 1)) < 60,
                "{share}: the paper goes dark: {:06x}",
                pixel_at(&after, 1, 1)
            );
            if share < 0.9 {
                assert!(
                    luma(pixel_at(&after, 100, 18)) > 150,
                    "{share}: the text goes light: {:06x}",
                    pixel_at(&after, 100, 18)
                );
            }
            for (x, y) in [(100, 130), (60, 100), (150, 160)] {
                let (before, now) = (pixel_at(&pixels, x, y), pixel_at(&after, x, y));
                let channels = |rgb: u32| [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF];
                let strongest = |rgb: u32| (0..3).max_by_key(|&channel| channels(rgb)[channel]);
                assert!(
                    strongest(now) == strongest(before) && luma(now) * 10 >= luma(before) * 7 && luma(now) <= luma(before),
                    "{share}: the photo keeps its colours at ({x}, {y}): {before:06x} came out {now:06x}"
                );
            }
        }
    }

    /// Failure: a page of text with a photo over half of it, where the background decides, is dimmed whole because the page has colour of its own, shaded bands over a third of what's outside the photo, as NASA's and the World Bank's report pages with a small logo were when the page's paper was first judged by the test for a white-backed image, which allows a fifth of colour. With the photo taken out, what's left is the page, and a page with no picture turns over with its colour, so this one does too.
    #[test]
    fn a_white_page_with_colour_of_its_own_and_a_photo_turns_over() {
        let photo = [0.3, 0.22, 0.98, 0.98];
        let pixels = made_page(
            &[(photo, Part::Photo), ([0.0, 0.0, 1.0, 0.12], Part::Band), ([0.02, 0.3, 0.28, 0.5], Part::Band)],
            WHITE,
            INK,
        );
        let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep == [sides(photo)], "{look:?}");
    }

    /// Failure: a full-page photograph, or one with only a printer's thin margin of white round it, turns over on dark paper, a sliver of margin deciding for a page that's a picture. Too little background to judge is a picture: true colours, dimmed (owner, 2026-11-07, what's left of "mostly a picture"). The rule before dimmed these too; this holds the new rule from reaching them. The thin margin is what [`LEAST_BACKGROUND`] carries, and was seen failing with it at 0; the full bleed passes without it, as with no paper outside the photo and no white-backed picture beside it there's no background to call white.
    #[test]
    fn a_page_that_is_a_picture_edge_to_edge_is_dimmed() {
        for (photo, name) in [([0.0, 0.0, 1.0, 1.0], "full bleed"), ([0.01, 0.01, 0.99, 0.99], "a margin too thin to judge")] {
            let pixels = made_page(&[(photo, Part::Photo)], WHITE, INK);
            let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(look.dim, "{name}: {look:?}");
            let after = shown(&look, pixels.clone());
            let (before, now) = (pixel_at(&pixels, 100, 130), pixel_at(&after, 100, 130));
            assert!(
                luma(now) < luma(before) && luma(now) * 10 >= luma(before) * 6,
                "{name}: dimmed, not turned: {before:06x} to {now:06x}"
            );
        }
    }

    /// Failure, either way round on a page whose background is a colour that isn't dark. A photo over half of it turns over with the page, so the page round a picture that's most of it is turned over though it isn't white: that page is dimmed whole, true colours (owner, 2026-11-13: the background decides once a picture is most of the page). Or a photo under half of it dims the whole page, which turns over beside its neighbours without one, as it did before 2026-11-13: under [`MOSTLY_PICTURE`] the background isn't asked. A dark background is never turned over: `a_page_whose_background_is_dark_is_never_turned_over`.
    #[test]
    fn on_a_coloured_background_a_photo_under_half_turns_over_and_one_over_half_is_dimmed() {
        for background in [LIGHT_ORANGE, SKY_BLUE, PALE_GREEN] {
            let photo = centred_photo(0.30);
            let pixels = made_page(&[(photo, Part::Photo)], background, INK);
            let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(!look.dim && look.keep == [sides(photo)], "{background:?} at 30%: {look:?}");
            let photo = centred_photo(0.60);
            let pixels = made_page(&[(photo, Part::Photo)], background, INK);
            let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(look.dim, "{background:?} at 60%: {look:?}");
        }
    }

    /// Failure: a slide of a light colour with a photo over 30% of it and the photo's drop shadow, an image of its own under it in nearly the same box, is dimmed whole while its neighbour with the same photo and no shadow turns over (second review of 2026-11-13, on a navy slide, which since the same day isn't turned over at all). The two boxes overlap all but a sliver, so they cover about 32% of the page between them; added up, they came to 60%, and the background decided. How much of a page is picture counts an overlap once, as the background outside the pictures does. Seen failing with the boxes' areas added up.
    #[test]
    fn a_photo_with_a_drop_shadow_turns_over_like_the_photo_alone() {
        let photo = centred_photo(0.30);
        let shadow = [photo[0] + 0.015, photo[1] + 0.012, photo[2] + 0.015, photo[3] + 0.012];
        let alone = made_page(&[(photo, Part::Photo)], SKY_BLUE, INK);
        let alone = PageLook::decide(Some(&[sides(photo)]), &alone, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!alone.dim, "{alone:?}");
        let pixels = made_page(&[(photo, Part::Photo), (shadow, Part::Shadow)], SKY_BLUE, INK);
        let boxes = [sides(shadow), sides(photo)];
        let look = PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep == boxes, "with its shadow: {look:?}, alone: {alone:?}");
    }

    /// Failure: two photos one above the other on a page of a light colour, over 30% of it each and 60% between them, turn over with their page, so the page round pictures that are most of it is turned over though it isn't white, as the background is asked only of a page that's mostly a picture. Their boxes don't overlap, so their share is both of them: half or more, the background decides, and a page not white is dimmed whole.
    #[test]
    fn two_photos_over_half_a_coloured_page_between_them_dim_it() {
        let (top, bottom) = ([0.05, 0.05, 0.95, 0.383], [0.05, 0.6, 0.95, 0.933]);
        let pixels = made_page(&[(top, Part::Photo), (bottom, Part::Photo)], SKY_BLUE, INK);
        let look = PageLook::decide(Some(&[sides(top), sides(bottom)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(look.dim, "{look:?}");
    }

    /// Failure: a report page with a coloured header band and sidebar beside a photo over 30% of it is dimmed whole to light grey between neighbours that turn over, as the background test did when any picture made it decide (review of 2026-11-13). Under [`MOSTLY_PICTURE`] the page turns over as before, its band and sidebar with it, and keeps its photo.
    #[test]
    fn a_page_with_a_coloured_band_and_sidebar_and_a_small_photo_turns_over() {
        let photo = [0.35, 0.35, 0.95, 0.85];
        let pixels = made_page(
            &[(photo, Part::Photo), ([0.0, 0.0, 1.0, 0.2], Part::Band), ([0.0, 0.2, 0.25, 1.0], Part::Band)],
            WHITE,
            INK,
        );
        let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep == [sides(photo)], "{look:?}");
    }

    /// Failure: in a dark slide deck, a slide with a small logo looks one way and its text-only neighbour another, so the deck alternates page by page (review of 2026-11-13, when the slide with a logo was dimmed and its neighbour turned light), or the deck turns light, which is what dark appearance is there to stop (owner, 2026-11-13: a page already dark is never turned over). Both stay dark, as they are, dimmed.
    #[test]
    fn a_dark_page_with_a_small_logo_looks_like_its_text_only_neighbour() {
        let logo = [0.05, 0.04, 0.2, 0.12];
        let with_logo = made_page(&[(logo, Part::Photo)], [25, 25, 28], [235, 235, 235]);
        let look = PageLook::decide(Some(&[sides(logo)]), &with_logo, PAGE_STRIDE, PAGE.0, PAGE.1);
        let neighbour = made_page(&[], [25, 25, 28], [235, 235, 235]);
        let text_only = PageLook::decide(Some(&[]), &neighbour, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert_eq!((&look, &text_only), (&PageLook::dark_page(), &PageLook::dark_page()));
    }

    /// Failure: images too small to say anything about, spacers of a few pixels, count as pictures kept, so sixty of them above a photo over 45% of a page of a light colour take it past half and dim it whole (review of 2026-11-13, on a navy page, which since the same day isn't turned over at all). They're no picture for this decision: the page comes out as it would without them, and a spacer is left alone where it is. Seen failing with every kept box counted.
    #[test]
    fn a_box_too_small_to_judge_changes_nothing() {
        let photo = centred_photo(0.45);
        let pixels = made_page(&[(photo, Part::Photo)], SKY_BLUE, INK);
        let one_pixel = [6_000u16, 6_000, 6_000 + 65_535 / PAGE.0 as u16, 6_000 + 65_535 / PAGE.1 as u16];
        let look = PageLook::decide(Some(&[sides(photo), one_pixel]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep == [sides(photo), one_pixel], "one spacer: {look:?}");
        // Sixty spacers of 7 by 7 pixels above the photo, each too small to judge: counted, they'd take the photo's 45% past half.
        let mut boxes = vec![sides(photo)];
        for at in 0..60u32 {
            let (x, y) = (20 * (at % 10) + 5, 8 * (at / 10) + 4);
            boxes.push([x * 65_535 / PAGE.0, y * 65_535 / PAGE.1, (x + 7) * 65_535 / PAGE.0, (y + 7) * 65_535 / PAGE.1].map(|side| side as u16));
        }
        let covered = 1.0 - share_outside(&pixel_boxes(boxes.iter().copied(), PAGE.0, PAGE.1), PAGE.0, PAGE.1);
        assert!(covered >= MOSTLY_PICTURE, "{covered}");
        let look = PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim, "sixty spacers: {look:?}");
    }

    /// Failure: a page on pale cream paper, too warm to count as paper by the scan's test, with a photo over 30% of it, is dimmed whole while its text-only neighbours turn over (review of 2026-11-13). Under [`MOSTLY_PICTURE`] it turns over as before.
    #[test]
    fn a_cream_page_with_a_photo_under_half_turns_over() {
        let cream = [253, 240, 200];
        let photo = centred_photo(0.30);
        let pixels = made_page(&[(photo, Part::Photo)], cream, INK);
        let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep == [sides(photo)], "{look:?}");
    }

    /// Failure: a picture with a white background, a chart or a scan, stays a bright box on dark paper, or keeps a photo beside it from turning its page over, as if it weren't background. It's judged by itself and turns over with the page (owner, 2026-11-07: "similar with images with white bg"): at 60% alone, and beside a photo over 55% of the page, where the chart is most of what's left, with a margin of white round them and with no paper outside the two at all. Beside the photo, the page was dimmed whole by the rule before.
    #[test]
    fn a_white_backed_picture_turns_over_with_its_page() {
        let chart = [0.1, 0.2, 0.9, 0.95];
        let pixels = made_page(&[(chart, Part::Chart)], WHITE, INK);
        let look = PageLook::decide(Some(&[sides(chart)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep.is_empty() && look.whiten.len() == 1, "a chart over 60%: {look:?}");
        for (photo, chart, name) in [
            ([0.05, 0.05, 0.95, 0.66], [0.05, 0.68, 0.95, 0.97], "with a margin"),
            ([0.0, 0.0, 1.0, 0.55], [0.0, 0.55, 1.0, 1.0], "with no paper outside them"),
        ] {
            let pixels = made_page(&[(photo, Part::Photo), (chart, Part::Chart)], WHITE, INK);
            let boxes = [sides(photo), sides(chart)];
            let look = PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(
                !look.dim && look.keep == [boxes[0]] && look.whiten.iter().map(|image| image.sides).eq([boxes[1]]),
                "{name}: {look:?}"
            );
        }
    }

    /// Failure: where the images are isn't known, for a page whose renderer can't be asked or an answer that failed, and "unknown" reads as "none", so a photo comes out as a negative and stays cached as one. With no picture to judge it's dimmed, and with one it's judged whole: a scan turns over, and a photo over 53% of a white page is dimmed, as it was before the background decided (2026-11-13), since its places aren't known to leave out.
    #[test]
    fn what_isnt_known_is_never_turned_over() {
        let rect = recto_proto::TileRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        assert_eq!(PageLook::unknown().in_tile(Some((2048, 2048)), rect), DarkTile::Dim);
        assert_eq!(PageLook::text().in_tile(None, rect), DarkTile::Dim, "the page's size wasn't known");
        let whole = PageLook::decide(None, &scan([250, 250, 250]), 400, 100, 100);
        assert!(!whole.dim, "the low-resolution path, judged whole: {whole:?}");
        let photo_page = made_page(&[(centred_photo(0.53), Part::Photo)], WHITE, INK);
        assert!(
            PageLook::decide(None, &photo_page, PAGE_STRIDE, PAGE.0, PAGE.1).dim,
            "a photo over 53% of a white page, judged whole, as before 2026-11-13"
        );
    }

    /// Failure: a catalogue page of 72 photos on white, too many images for a worker to tell apart, turns over round one bright block, as when the worker answered with one box round them all, kept as a picture over 90% of a page whose margins are white. A worker now answers that it can't tell, and a page whose images aren't known is judged whole: its photos are too much colour for a page of white, so it's dimmed, as before 2026-11-13.
    #[test]
    fn a_page_of_too_many_photos_to_tell_apart_is_dimmed_whole() {
        let photos: Vec<([f64; 4], Part)> = (0..72)
            .map(|at| {
                let (column, row) = (f64::from(at % 8), f64::from(at / 8));
                ([0.06 + column * 0.11, 0.05 + row * 0.1, 0.15 + column * 0.11, 0.13 + row * 0.1], Part::Photo)
            })
            .collect();
        let pixels = made_page(&photos, WHITE, INK);
        let look = PageLook::decide(None, &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(look.dim, "{look:?}");
    }

    /// Failure: a mosaic of 72 photos laid edge to edge on a white page, which the worker, past 64 images, gives as one picture, the box round them, is dimmed whole or turned into a negative. It's one picture kept, with its photos' colours, on a page that turns over round it, which is right: its margins are white. Photos with gaps between them, which the worker doesn't merge, are judged one by one, or, past what an answer holds, the page is judged whole and dimmed (`a_page_of_too_many_photos_to_tell_apart_is_dimmed_whole`).
    #[test]
    fn a_mosaic_of_photos_edge_to_edge_is_one_picture_on_a_page_that_turns_over() {
        let mosaic: Vec<([f64; 4], Part)> = (0..72)
            .map(|at| {
                let (column, row) = (f64::from(at % 8), f64::from(at / 8));
                ([0.06 + column * 0.11, 0.05 + row * 0.1, 0.17 + column * 0.11, 0.15 + row * 0.1], Part::Photo)
            })
            .collect();
        let pixels = made_page(&mosaic, WHITE, INK);
        let whole = [sides([0.06, 0.05, 0.94, 0.95])];
        let look = PageLook::decide(Some(&whole), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(!look.dim && look.keep == whole && look.whiten.is_empty(), "{look:?}");
        let after = shown(&look, pixels.clone());
        assert!(luma(pixel_at(&after, 2, 2)) < 60, "the margin goes dark: {:06x}", pixel_at(&after, 2, 2));
        let (before, now) = (pixel_at(&pixels, 100, 130), pixel_at(&after, 100, 130));
        assert!(
            luma(now) <= luma(before) && luma(now) * 10 >= luma(before) * 7,
            "a photo keeps its colours: {before:06x} to {now:06x}"
        );
    }

    /// Failure: on dark paper a page whose own background is already dark is turned over, so a navy page comes out light blue, a black cover a white page and a dark slide deck light: the opposite of what dark appearance is for (owner, 2026-11-13: "I agree with your recommendation on dark images", a page already dark is never turned over, whatever pictures it has). Black, navy, dark green, dark grey, and a deep orange in the middle of the range, under [`DARK_MOST_LUMA`]: with no images, a small logo, a photo over 30% and over 60% of it, and judged whole with its images not known, each is shown as it is, dimmed, and none comes out lighter than it was.
    #[test]
    fn a_page_whose_background_is_dark_is_never_turned_over() {
        let logo = [0.05, 0.04, 0.2, 0.12];
        for background in [[0, 0, 0], NAVY, [22, 70, 40], [60, 60, 60], [200, 90, 40]] {
            let cases: [(&str, &[[f64; 4]]); 4] = [
                ("no images", &[]),
                ("a logo", &[logo]),
                ("a photo over 30%", &[centred_photo(0.30)]),
                ("a photo over 60%", &[centred_photo(0.60)]),
            ];
            for (name, places) in cases {
                let parts: Vec<([f64; 4], Part)> = places.iter().map(|place| (*place, Part::Photo)).collect();
                let pixels = made_page(&parts, background, LIGHT_TEXT);
                let boxes: Vec<[u16; 4]> = places.iter().map(|place| sides(*place)).collect();
                for (judged, look) in [
                    ("with its images", PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1)),
                    ("whole", PageLook::decide(None, &pixels, PAGE_STRIDE, PAGE.0, PAGE.1)),
                ] {
                    assert_eq!(look, PageLook::dark_page(), "{background:?}, {name}, judged {judged}");
                    let after = shown(&look, pixels.clone());
                    for (x, y) in [(1, 1), (100, 18), (100, 130)] {
                        assert!(
                            luma(pixel_at(&after, x, y)) <= luma(pixel_at(&pixels, x, y)),
                            "{background:?}, {name}: lighter at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    /// Failure: a white page with a dark header band, a dark sidebar, or a dark figure across it, as an image or drawn in vectors, is taken for a dark page and dimmed whole to grey, where it turned over before: its background is white. And a dark page with a white text box or a light table on it is taken for a light one and turned light: its background is dark. What decides is most of the background, at least [`DARK_LEAST`] of it.
    #[test]
    fn a_page_is_dark_by_most_of_its_background() {
        // Each a dark part on a white page with text, and whether it's an image the worker gives a box for.
        let not_dark = [
            ("a dark header band", [0.0, 0.0, 1.0, 0.2], false),
            ("a dark sidebar", [0.0, 0.0, 0.25, 1.0], false),
            ("a dark figure in vectors over 33%", [0.05, 0.25, 0.95, 0.62], false),
            ("a dark figure as an image over 60%", [0.05, 0.15, 0.95, 0.82], true),
        ];
        for (name, place, image) in not_dark {
            let pixels = made_page(&[(place, Part::Dark)], WHITE, INK);
            let boxes: Vec<[u16; 4]> = if image { vec![sides(place)] } else { Vec::new() };
            let look = PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(!look.dim && look.keep == boxes, "a white page with {name}: {look:?}");
        }
        for (name, place) in [
            ("a white text box over 30%", [0.1, 0.35, 0.9, 0.73]),
            ("a light table over 35%", [0.1, 0.3, 0.9, 0.74]),
        ] {
            let pixels = made_page(&[(place, Part::Light)], NAVY, LIGHT_TEXT);
            assert_eq!(
                PageLook::decide(Some(&[]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1),
                PageLook::dark_page(),
                "a navy page with {name}"
            );
        }
        // Half and half: the line is half the background, so a page 40% dark turns over and one 60% dark stays, by the background's own tone, with no text to count either way (with it the 40% page was 0.48 dark).
        for (share, dark) in [(0.4, false), (0.6, true)] {
            let pixels = made_page(&[([0.0, 0.0, 1.0, share], Part::Dark)], WHITE, WHITE);
            let look = PageLook::decide(Some(&[]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert_eq!(look.dim, dark, "a page {share} dark: {look:?}");
        }
    }

    /// Failure (review of 2026-11-13, B1): a white-backed picture, a chart or a scan, is cut out of the background the dark test reads, so a dark part beside it is all that's left and decides the page: it's dimmed whole, its picture grey-white, the owner's complaint of 2026-11-07 back again. A white-backed picture is light background, read as it is. With the ring (second review, 2026-11-14), a dark band or footer along one side beside it leaves a ring mostly white, and can't show this: the six band cases this test had, and a scan with a dark footer, passed with every box cut out. A dark frame round the picture can: its ring is dark, and with the picture read under half of its background is, so it turns over, the picture with it; cut out, the frame was all of it. The other side: a navy page with a white-backed chart over 40% is dark, navy most of its background and all of its ring, and stays as it is, dimmed with its chart (owner, 2026-11-13: "whatever pictures it has"). Seen failing with every image's box cut out of the background (the frames), and with the background's line at 0.7 (the navy page).
    #[test]
    fn a_white_backed_picture_is_light_background_for_the_dark_test() {
        for (picture, part) in [([0.08, 0.08, 0.92, 0.92], Part::Chart), ([0.06, 0.06, 0.94, 0.94], Part::Scan)] {
            let pixels = made_page(&[(picture, part)], [20, 24, 32], LIGHT_TEXT);
            let look = PageLook::decide(Some(&[sides(picture)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(
                !look.dim && look.keep.is_empty() && look.whiten.iter().map(|image| image.sides).eq([sides(picture)]),
                "a white-backed picture {picture:?} in a dark frame: {look:?}"
            );
        }
        let chart = [0.1, 0.3, 0.9, 0.8];
        let pixels = made_page(&[(chart, Part::Chart)], NAVY, LIGHT_TEXT);
        assert_eq!(
            PageLook::decide(Some(&[sides(chart)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1),
            PageLook::dark_page(),
            "a navy page with a white-backed chart over 40%"
        );
    }

    /// Failure (review of 2026-11-13, B1, and the second review of 2026-11-14): a white page with a dark sidebar a third wide or under, beside a photo over 45% of it and a column of text, or with text alone, is dimmed whole, as the sidebar is more than half of what the photo leaves, where before 2026-11-13 it turned over keeping its photo. Its ring isn't mostly dark: the sidebar's side is, and its share of the top and the foot, and the rest is white or the photo's. Seen failing with the ring's line at 0.3.
    #[test]
    fn a_white_page_with_a_dark_sidebar_turns_over() {
        for side in [0.25, 0.3] {
            let photo = [side + 0.02, 0.1, 1.0, 0.76];
            let pixels = made_page(&[([0.0, 0.0, side, 1.0], Part::Dark), (photo, Part::Photo)], WHITE, INK);
            let look = PageLook::decide(Some(&[sides(photo)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
            assert!(!look.dim && look.keep == [sides(photo)] && look.whiten.is_empty(), "a sidebar {side} wide beside a photo: {look:?}");
            let pixels = made_page(&[([0.0, 0.0, side, 1.0], Part::Dark)], WHITE, INK);
            assert_eq!(PageLook::decide(Some(&[]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1), PageLook::text(), "a sidebar {side} wide");
        }
    }

    /// Failure (second review of 2026-11-14): a white page with a dark figure drawn in vectors over half of it, a dark chart, a code listing, an inverted diagram, is mostly dark and is dimmed whole, its white margins grey with dark text round a dark box, the owner's complaint of 2026-11-07 in a new place. Its ring is its white margins, and it turns over with its page, as a page of text with a figure on it does. Seen failing with the ring's line at 0.
    #[test]
    fn a_white_page_with_a_dark_figure_over_half_of_it_turns_over() {
        for place in [[0.08, 0.19, 0.92, 0.81], [0.06, 0.1, 0.94, 0.88], [0.1, 0.12, 0.9, 0.95]] {
            let pixels = made_page(&[(place, Part::Dark)], WHITE, INK);
            let found = sample_lightness(&pixels, PAGE_STRIDE, PAGE.0, PAGE.1, &[]);
            assert!(found.dark >= DARK_LEAST, "the made page isn't mostly dark: {found:?}");
            assert_eq!(PageLook::decide(Some(&[]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1), PageLook::text(), "a dark figure {place:?}: {found:?}");
        }
    }

    /// Failure (second review of 2026-11-14): a navy page with a photo under half of it, the shape where only the dark rule decides, as under [`MOSTLY_PICTURE`] a page that isn't dark turns over whatever its background, comes out light blue; or one with a white text box beside the photo, or with the photo running off one side. Its ring is navy where the photo doesn't cover it, and it stays as it is, dimmed. Seen failing with the ring's line past 1.
    #[test]
    fn a_navy_page_with_a_photo_under_half_of_it_stays_dark() {
        let cases: [(&str, &[([f64; 4], Part)]); 3] = [
            ("a photo over 45%", &[(centred_photo(0.45), Part::Photo)]),
            ("a photo and a white text box", &[([0.55, 0.2, 0.95, 0.75], Part::Photo), ([0.05, 0.2, 0.5, 0.75], Part::Light)]),
            ("a photo off its right side over 45%", &[([0.45, 0.1, 1.0, 0.92], Part::Photo)]),
        ];
        for (name, parts) in cases {
            let pixels = made_page(parts, NAVY, LIGHT_TEXT);
            let boxes: Vec<[u16; 4]> = parts.iter().filter(|(_, part)| matches!(part, Part::Photo)).map(|(place, _)| sides(*place)).collect();
            assert_eq!(PageLook::decide(Some(&boxes), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1), PageLook::dark_page(), "a navy page with {name}");
        }
    }

    /// Failure (second review of 2026-11-14): the dark test and the white test read different backgrounds, the white test only the paper outside every box, white-backed pictures cut out too. A white page with a photo over 55% of it, a white-backed chart over 30% and a dark title band over 15% isn't dark, 15 of its 45, but the white test saw only the band, and the page was dimmed whole, its chart grey-white, the owner's complaint of 2026-11-07 from the second test. One background for both, everything but the kept pictures, the chart read as it is: mostly white, so the page turns over, keeping its photo and turning its chart with it. Seen failing with the white test reading the paper outside every box.
    #[test]
    fn the_white_test_reads_the_same_background_as_the_dark_test() {
        let (band, photo, chart) = ([0.0, 0.0, 1.0, 0.15], [0.0, 0.15, 1.0, 0.7], [0.0, 0.7, 1.0, 1.0]);
        let pixels = made_page(&[(band, Part::Dark), (photo, Part::Photo), (chart, Part::Chart)], WHITE, INK);
        let look = PageLook::decide(Some(&[sides(photo), sides(chart)]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1);
        assert!(
            !look.dim && look.keep == [sides(photo)] && look.whiten.iter().map(|image| image.sides).eq([sides(chart)]),
            "{look:?}"
        );
    }

    /// Measures, and isn't a check: the darkness of the made pages the dark rule was chosen from, as `decide` reads it, the background outside the kept pictures and its ring. `cargo test -p recto-reader --lib -- --ignored made_pages_darkness --nocapture`.
    #[test]
    #[ignore = "a measurement"]
    fn made_pages_darkness() {
        let measure = |name: &str, parts: &[([f64; 4], Part)], kept: &[[f64; 4]], background: [u8; 3], text: [u8; 3]| {
            let pixels = made_page(parts, background, text);
            let holes = pixel_boxes(kept.iter().map(|place| sides(*place)), PAGE.0, PAGE.1);
            let found = sample_lightness(&pixels, PAGE_STRIDE, PAGE.0, PAGE.1, &holes);
            println!(
                "{name}: background {:.3} of the page, {:.3} of it dark, dark {:.3} of the page, ring {:.3} dark of {} points",
                found.of_page,
                found.dark,
                found.dark * found.of_page,
                found.ring,
                found.ring_points
            );
        };
        for share in [0.30, 0.39, 0.45, 0.53, 0.60, 0.70] {
            let photo = centred_photo(share);
            for (name, background) in [
                ("black", [0, 0, 0]),
                ("navy", NAVY),
                ("dark green", [22, 70, 40]),
                ("dark grey", [60, 60, 60]),
                ("deep orange", [200, 90, 40]),
            ] {
                measure(
                    &format!("{name} page, light text, photo {share}"),
                    &[(photo, Part::Photo)],
                    &[photo],
                    background,
                    LIGHT_TEXT,
                );
            }
        }
        let photo = [0.32, 0.1, 1.0, 0.76];
        measure(
            "white page, photo 45%, dark sidebar 30%",
            &[([0.0, 0.0, 0.3, 1.0], Part::Dark), (photo, Part::Photo)],
            &[photo],
            WHITE,
            INK,
        );
        for side in [0.25, 0.35, 0.4] {
            let photo = [side + 0.02, 0.1, 1.0, 0.76];
            measure(
                &format!("white page, photo, dark sidebar {side}"),
                &[([0.0, 0.0, side, 1.0], Part::Dark), (photo, Part::Photo)],
                &[photo],
                WHITE,
                INK,
            );
        }
    }

    /// Failure: a page that isn't dark comes out otherwise than it did before 2026-11-13, a page of text dimmed, or a pale page left bright: white, cream, a pale tint, a yellowed scan's tone, light grey, and a middle tone above [`DARK_MOST_LUMA`], each with no images, turn over exactly as a page of text did, [`PageLook::text`]. A grey a little under the line is dark, and one a little over it isn't.
    #[test]
    fn a_light_page_turns_over_as_a_page_of_text_did() {
        for background in [
            WHITE,
            [253, 246, 227],
            [225, 236, 250],
            [236, 226, 196],
            [210, 210, 210],
            [190, 190, 190],
            LIGHT_ORANGE,
            SKY_BLUE,
            PALE_GREEN,
            [136, 136, 136],
        ] {
            let pixels = made_page(&[], background, INK);
            assert_eq!(
                PageLook::decide(Some(&[]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1),
                PageLook::text(),
                "{background:?}"
            );
        }
        let pixels = made_page(&[], [120, 120, 120], LIGHT_TEXT);
        assert_eq!(
            PageLook::decide(Some(&[]), &pixels, PAGE_STRIDE, PAGE.0, PAGE.1),
            PageLook::dark_page(),
            "a grey of 120"
        );
    }

    /// Failure (rule 6): a picture's size or stride from a worker that doesn't match its bytes, or image boxes inside out or off the picture, make reading a background's darkness panic or read past the bytes. It reads only what's there, and says it read nothing where there's nothing.
    #[test]
    fn reading_darkness_stays_inside_the_bytes() {
        let pixels = vec![0u8; 16 * 16 * 4];
        assert_eq!(sample_lightness(&pixels, 0, 16, 16, &[]), Lightness::default(), "no stride");
        assert_eq!(sample_lightness(&pixels, 64, 0, 16, &[]), Lightness::default(), "no width");
        let past = sample_lightness(&pixels, 64, 100_000, 100_000, &[]);
        assert!(past.points <= 16 * 16, "a size past the bytes: {past:?}");
        let narrow = sample_lightness(&pixels, 8, 16, 16, &[]);
        assert!(narrow.points <= pixels.len() / 4, "a stride narrower than the rows: {narrow:?}");
        let holes = [[12, 0, 3, 16], [0, 20, 16, 2], [usize::MAX - 1, 0, usize::MAX, 16]];
        let found = sample_lightness(&pixels, 64, 16, 16, &holes);
        assert!(
            found.points == 16 * 16 && found.dark == 1.0,
            "holes inside out or off the picture cover nothing: {found:?}"
        );
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

    /// The stretches of a row of `[left, right)` that `gaps_in_row` gives for `holes`, sorted as it wants them, checked to be in order, apart, inside the row and clear of every hole.
    fn gaps(holes: &[PixelBox], left: usize, right: usize) -> Vec<(usize, usize)> {
        let mut holes = holes.to_vec();
        holes.sort_unstable_by_key(|hole| hole[0]);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        gaps_in_row(&holes, 0, left, right, |from, to| runs.push((from, to)));
        let mut at = left;
        for &(from, to) in &runs {
            assert!(from >= at && from < to && to <= right, "{runs:?} for {holes:?} in {left}..{right}");
            assert!(
                holes.iter().all(|hole| hole[0] >= hole[2] || to <= hole[0] || from >= hole[2]),
                "{runs:?} cross a hole of {holes:?}"
            );
            at = to;
        }
        runs
    }

    /// Failure (rule 6): boxes from a hostile worker, inside out, overlapping, nested or out of order, make the row's walk give stretches that overlap, so the page's paper is counted more than once, its share comes to more than the page, and a picture of 64 crafted holes is walked tens of times over (review of 2026-11-13). Each stretch comes once, in order, inside the row and outside every real hole, so the walk never looks at more than the row's pixels.
    #[test]
    fn the_gaps_in_a_row_never_overlap_whatever_the_holes() {
        let hole = |left: usize, right: usize| [left, 0, right, 1];
        assert_eq!(gaps(&[hole(10, 50), hole(20, 30)], 0, 100), [(0, 10), (50, 100)], "nested");
        assert_eq!(gaps(&[hole(10, 40), hole(30, 60)], 0, 100), [(0, 10), (60, 100)], "overlapping");
        assert_eq!(gaps(&[hole(70, 80), hole(10, 20)], 0, 100), [(0, 10), (20, 70), (80, 100)], "out of order");
        assert_eq!(gaps(&[hole(60, 20)], 0, 100), [(0, 100)], "inside out covers nothing");
        assert_eq!(
            gaps(&[hole(10, 5), hole(3, 8), hole(12, 4)], 0, 100),
            [(0, 3), (8, 100)],
            "inside out among others"
        );
        assert_eq!(gaps(&[hole(90, 500)], 0, 100), [(0, 90)], "past the right edge");
        assert_eq!(gaps(&[hole(150, 200)], 0, 100), [(0, 100)], "beyond it");
        assert_eq!(gaps(&[hole(10, 20)], 50, 40), [], "a row inside out");
        assert_eq!(gaps(&[hole(1, 2)], 0, 1), [(0, 1)], "a hole that starts at the row's end");
        assert_eq!(gaps(&[hole(0, 1)], 0, 1), [], "one that covers the row");
        // 64 holes, crafted so that each starts where the walk is and ends before it: the review's case.
        let crafted: Vec<PixelBox> = (0..64).map(|at| hole(at + 1, 0)).collect();
        assert_eq!(gaps(&crafted, 0, 100), [(0, 100)]);
        let every_other: Vec<PixelBox> = (0..64).map(|at| hole(at * 2, at * 2 + 1)).collect();
        assert_eq!(gaps(&every_other, 0, 200).iter().map(|(from, to)| to - from).sum::<usize>(), 200 - 64);
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
        // A colour tile as a page has them: white, grey text, and a fifth of it a chart's flat colour. Turned from a fresh copy each time, as a tile is turned once.
        let page_like: Vec<u8> = (0..512 * 512)
            .flat_map(|at: usize| {
                let (x, y) = (at % 512, at / 512);
                if x > 300 && y > 300 {
                    [240u8, 32, 21, 255]
                } else if (x / 3 + y / 7) % 5 == 0 {
                    let v = ((x * 37 + y * 11) % 256) as u8;
                    [v, v, v, 255]
                } else {
                    [255, 255, 255, 255]
                }
            })
            .collect();
        let mut samples: Vec<u128> = (0..200)
            .map(|_| {
                let mut pixels = page_like.clone();
                let started = std::time::Instant::now();
                turn_over(&mut pixels, 512, 512, 2048, false, &[], &[]);
                started.elapsed().as_micros()
            })
            .collect();
        samples.sort_unstable();
        println!("turn over a page-like colour tile: p50 {} us, p95 {} us", samples[100], samples[190]);
        time("whiten and turn over", &|pixels| {
            turn_over(
                pixels,
                512,
                512,
                2048,
                false,
                &[],
                &[(
                    Keep {
                        left: 0,
                        top: 0,
                        right: 512,
                        bottom: 512,
                    },
                    [270, 280, 300],
                )],
            )
        });
        let page = tile(31);
        time("judge a page picture", &|_| {
            let _ = PageLook::decide(Some(&[[0, 0, 65_535, 65_535]]), &page, 2048, 512, 512);
        });
        time("dim page", &|pixels| dim_page(pixels, 512, 512, 2048));
        // The decision for a letter page with lines of text and a photo over 53% of it, which on 2026-11-13 came to look at the page's paper outside the photo as well: at the size of the picture a look is drawn for, a 512 px tile's size, and the largest preview.
        for (width, height) in [(396u32, 512u32), (512, 512), (884, 1144)] {
            let photo = [4_915u16, 13_107, 60_620, 52_428];
            let page: Vec<u8> = (0..(width * height) as usize)
                .flat_map(|at| {
                    let (x, y) = ((at % width as usize) as u32, (at / width as usize) as u32);
                    let inside = x * 65_535 / width >= 4_915 && x * 65_535 / width < 60_620 && y * 65_535 / height >= 13_107 && y * 65_535 / height < 52_428;
                    if inside {
                        [40, (90 + x % 70) as u8, (70 + y % 120) as u8, 255]
                    } else if y % 7 == 0 && x % 11 != 0 {
                        [50, 50, 50, 255]
                    } else {
                        [255, 255, 255, 255]
                    }
                })
                .collect();
            time(&format!("decide a letter page with a 53% photo, {width} by {height}"), &|_| {
                let _ = PageLook::decide(Some(&[photo]), &page, width as usize * 4, width, height);
            });
        }
        // The decision for a page with no images, which since 2026-11-13 reads its background's darkness: a white page of text and a black one, at the look picture's size and the largest preview's, p50 and p99 of 2,000.
        for (width, height) in [(396u32, 512u32), (884, 1144)] {
            for (name, paper, ink) in [("white", 255u8, 40u8), ("black", 0, 230)] {
                let page: Vec<u8> = (0..(width * height) as usize)
                    .flat_map(|at| {
                        let (x, y) = (at % width as usize, at / width as usize);
                        let value = if y % 7 == 0 && x % 11 != 0 { ink } else { paper };
                        [value, value, value, 255]
                    })
                    .collect();
                let mut samples: Vec<u128> = (0..2_000)
                    .map(|_| {
                        let started = std::time::Instant::now();
                        std::hint::black_box(PageLook::decide(Some(&[]), std::hint::black_box(&page), width as usize * 4, width, height));
                        started.elapsed().as_nanos()
                    })
                    .collect();
                samples.sort_unstable();
                println!(
                    "decide a {name} page with no images, {width} by {height}: p50 {:.1} us, p99 {:.1} us",
                    samples[1_000] as f64 / 1000.0,
                    samples[1_980] as f64 / 1000.0
                );
            }
        }
    }
}
