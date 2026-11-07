//! Dark paper, as a change to a tile's pixels (owner, 2026-11-06: dark mode offers dark or white paper).
//!
//! It's done to the pixels rather than by a filter on the layer because a filter makes the compositor draw every frame through an offscreen pass, and because a QA shot wouldn't see it: `CALayer.render(in:)` leaves
//! filters out. A tile redrawn for another paper is just another tile.
//!
//! **The rule,** from `docs/concepts/reader-dark.png`: lightness turns over, so white paper goes dark and black text goes light, and each pixel keeps its colour's distance from grey, so blue text is lighter blue with no halo round its letters. Images on a page of text aren't turned over: the renderer says where they are, and they stay as they were, a little dimmed.
//!
//! **A page that's mostly a picture isn't turned over at all.** Owner, 2026-11-07, answering "should pages that are mostly a picture be dimmed instead of flipped? I recommend yes": "I agree". A map, a full-page photograph or a scan keeps its true colours, dimmed by [`PICTURE_PAGE_DIM`] so it doesn't glare. [`MOSTLY_PICTURE`] says what counts. **When where the images are isn't known, it's the same,** as an image turned over is a negative, and a page dimmed is only brighter than it might have been: a low-resolution page, whose renderer isn't asked, comes out the same way as a sharp one. **Its cost:** a scanned page of text stays light, and dimmed, which the owner accepted. A chart drawn as vectors on a page of text turns over with the text, where the concept kept its colours, since pixels can't say what they belong to.

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
/// How bright a page that's mostly a picture stays, in 256ths. White paper at this is what the eye gets beside a dark window, so it takes the glare off, and it leaves black text on it, in a map's legend say, at well over WCAG AA, by a test. Chosen by looking at the USGS map's `heavy-dark` and `low-res-dark`.
pub const PICTURE_PAGE_DIM: u32 = 192;

impl PaperKind {
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

/// How much of a page, added up over its images, makes it mostly a picture: more than half, which is what the words say. Measured over `bench100`: USGS map sheets 0.59 to 1.0, with their legends and titles in vectors around the map, scans and full-page backgrounds 1.0, and figures on arXiv pages of text 0.16 to 0.26. A USGS sheet at 0.40 turns over and keeps its map.
pub const MOSTLY_PICTURE: f64 = 0.5;

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
    /// Turned over, but for these pictures.
    TurnOver(Vec<Keep>),
    /// True colours, dimmed, as the page is a picture or it isn't known.
    Dim,
}

/// The part of a page its images cover. Added up, so images that overlap count twice, and a page can't come to more than all of it. A page's images past the report's cap come as one box around them all, which only ever errs towards not turning over.
fn image_share(boxes: &[[u16; 4]]) -> f64 {
    boxes.iter().map(|sides| f64::from(sides[2].saturating_sub(sides[0])) * f64::from(sides[3].saturating_sub(sides[1])) / (65_535.0 * 65_535.0)).sum::<f64>().min(1.0)
}

/// The rule for a tile of a page. `boxes` is where the images are, if that's known, and `page` the page's size in device pixels at the rectangle's scale.
pub fn dark_tile(boxes: Option<&[[u16; 4]]>, page: Option<(u32, u32)>, rect: recto_proto::TileRect) -> DarkTile {
    match (boxes, page) {
        (Some(boxes), Some(page)) if image_share(boxes) < MOSTLY_PICTURE => DarkTile::TurnOver(keeps_in(boxes, page, rect)),
        _ => DarkTile::Dim,
    }
}

/// Where a page's images fall in a rectangle of it, from the parts of the page the renderer gave. `page` is the page's size in device pixels at the rectangle's scale. Rounds outward, so an image's edge isn't turned over.
fn keeps_in(boxes: &[[u16; 4]], page: (u32, u32), rect: recto_proto::TileRect) -> Vec<Keep> {
    let place = |part: u16, whole: u32, up: bool| -> u32 {
        let exact = u64::from(part) * u64::from(whole);
        (if up { exact.div_ceil(65_535) } else { exact / 65_535 }) as u32
    };
    boxes
        .iter()
        .filter_map(|sides| {
            let (left, top) = (place(sides[0], page.0, false), place(sides[1], page.1, false));
            let (right, bottom) = (place(sides[2], page.0, true), place(sides[3], page.1, true));
            let keep = Keep { left: left.max(rect.x) - rect.x, top: top.max(rect.y) - rect.y, right: right.min(rect.x + rect.width).saturating_sub(rect.x), bottom: bottom.min(rect.y + rect.height).saturating_sub(rect.y) };
            (keep.right > keep.left && keep.bottom > keep.top).then_some(keep)
        })
        .collect()
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
pub fn turn_over(pixels: &mut [u8], width: u32, height: u32, stride: u32, grey: bool, keeps: &[Keep]) {
    let (row_bytes, stride) = (width as usize * 4, stride as usize);
    if stride < row_bytes || stride == 0 {
        return;
    }
    if grey && keeps.is_empty() {
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
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if here.iter().any(|keep| (keep.left as usize..keep.right as usize).contains(&x)) {
                dim_pixel(pixel);
            } else {
                turn_pixel(pixel);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        turn_over(&mut pixels, 4, 2, 16, true, &[Keep { left: 1, top: 0, right: 3, bottom: 1 }]);
        let at = |x: usize, y: usize| pixels[(y * 16) + x * 4];
        assert!(at(1, 0) > 200 && at(2, 0) > 200, "inside: {}", at(1, 0));
        assert_eq!(u32::from(at(0, 0)), DARK_PAPER & 0xFF, "beside it");
        assert_eq!(u32::from(at(1, 1)), DARK_PAPER & 0xFF, "below it");
    }

    /// Failure: an image's place misses the tile it's in, or a box from a hostile worker writes outside the tile. Holds the parts of a page to its pixels, at the tile's own origin.
    #[test]
    fn a_box_lands_where_it_should_in_a_tile() {
        let rect = recto_proto::TileRect { x: 512, y: 0, width: 512, height: 512 };
        let half = keeps_in(&[[0, 0, 32_768, 32_768]], (2048, 2048), rect);
        assert_eq!(half, vec![Keep { left: 0, top: 0, right: 512, bottom: 512 }], "to the middle of the page, past this tile");
        assert!(keeps_in(&[[60_000, 60_000, 65_535, 65_535]], (2048, 2048), rect).is_empty(), "elsewhere on the page");
        assert!(keeps_in(&[[65_535, 65_535, 0, 0]], (2048, 2048), rect).is_empty(), "inside out");
        let edge_to_edge = keeps_in(&[[0, 0, 40_000, 40_000]], (u32::MAX, u32::MAX), rect);
        assert!(edge_to_edge.iter().all(|keep| keep.right <= 512 && keep.bottom <= 512));
    }

    /// Failure: a map, a full-page photo or a scan goes negative on dark paper, as the USGS map's band went mint and its black lines white. Owner, 2026-11-07: pages that are mostly a picture are dimmed, not flipped. A page of text with a figure still turns over, its figure left alone.
    #[test]
    fn a_page_that_is_mostly_a_picture_is_dimmed_not_flipped() {
        let rect = recto_proto::TileRect { x: 0, y: 0, width: 512, height: 512 };
        let page = Some((2048, 2048));
        assert_eq!(dark_tile(Some(&[[0, 0, 65_535, 65_535]]), page, rect), DarkTile::Dim, "a scan or a full-page photo");
        let side = ((MOSTLY_PICTURE + 0.01).sqrt() * 65_535.0) as u16;
        assert_eq!(dark_tile(Some(&[[0, 0, side, side]]), page, rect), DarkTile::Dim, "just past the line");
        let pieces: Vec<[u16; 4]> = (0..10u16).map(|index| [0, index * 6553, 65_535, index * 6553 + 5000]).collect();
        assert_eq!(dark_tile(Some(&pieces), page, rect), DarkTile::Dim, "a picture in strips adds up");
        assert_eq!(dark_tile(Some(&[]), page, rect), DarkTile::TurnOver(Vec::new()), "a page of text");
        let DarkTile::TurnOver(figure) = dark_tile(Some(&[[0, 0, 20_000, 20_000]]), page, rect) else { panic!("a page of text with a figure turns over") };
        assert_eq!(figure.len(), 1, "and the figure's left alone");
    }

    /// Failure: where the images are isn't known, for a low-resolution page that isn't asked or an answer that failed, and "unknown" reads as "none", so a photo comes out as a negative and stays cached as one. It's dimmed, the same as a sharp picture page.
    #[test]
    fn what_isnt_known_is_never_turned_over() {
        let rect = recto_proto::TileRect { x: 0, y: 0, width: 2, height: 1 };
        assert_eq!(dark_tile(None, Some((2048, 2048)), rect), DarkTile::Dim, "the low-resolution path");
        assert_eq!(dark_tile(Some(&[]), None, rect), DarkTile::Dim, "the page's size wasn't known");
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
        turn_over(&mut pixel, 1, 1, 4, is_grey(TileKind::Blank { rgb: 0xFF_00_00 }), &[]);
        let turned = (u32::from(pixel[2]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[0]);
        assert_eq!(turned, dark_rgb(0xFF_00_00));
        assert!(pixel[2] > pixel[0] && pixel[2] > pixel[1], "still red: {pixel:?}");
    }

    /// Failure: the table the grey path uses disagrees with the path for colour, so a grey tile beside a colour tile shows a seam.
    #[test]
    fn grey_tiles_and_colour_tiles_agree() {
        let mut one: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v, 0xFF]).collect();
        let mut two = one.clone();
        turn_over(&mut one, 256, 1, 1024, true, &[]);
        turn_over(&mut two, 256, 1, 1024, false, &[]);
        assert_eq!(one, two);
    }

    /// Failure: a hostile stride panics or writes outside a row.
    #[test]
    fn a_bad_stride_changes_nothing_or_stays_in_bounds() {
        let mut pixels = vec![9u8; 64];
        turn_over(&mut pixels, 100, 100, 4, false, &[]);
        assert_eq!(pixels, vec![9u8; 64], "a stride narrower than the row");
        turn_over(&mut pixels, 8, 100, 0, true, &[]);
        turn_over(&mut pixels, 4, 100, 16, true, &[]);
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
        time("turn over", &|pixels| turn_over(pixels, 512, 512, 2048, false, &[]));
        time("dim page", &|pixels| dim_page(pixels, 512, 512, 2048));
    }
}
