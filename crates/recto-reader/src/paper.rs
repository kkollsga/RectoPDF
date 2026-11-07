//! Dark paper, as a change to a tile's pixels (owner, 2026-11-06: dark mode offers dark or white paper).
//!
//! It's done to the pixels rather than by a filter on the layer because a filter makes the compositor draw every frame through an offscreen pass, and because a QA shot wouldn't see it: `CALayer.render(in:)` leaves
//! filters out. A tile redrawn for another paper is just another tile.
//!
//! **The rule,** from `docs/concepts/reader-dark.png`: what has no colour turns over, so white paper goes dark and black text goes light. What has colour stays that colour, a little dimmed. The steps between come from how
//! much colour a pixel has, so the edge of a coloured line doesn't get a halo. **Its limit:** the pixels don't say what they belong to, so a black and white photograph turns over too. Telling images apart needs their
//! rectangles from the renderer, which it doesn't report yet.

/// Which paper a tile was drawn for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaperKind {
    #[default]
    White,
    Dark,
}

/// Dark paper's colour, `0xRRGGBB`, from the concept.
pub const DARK_PAPER: u32 = 0x1E_20_23;
/// What black text becomes.
const DARK_INK: [u32; 3] = [0xE4, 0xE5, 0xE7];
/// How bright what keeps its colour stays, in 256ths.
const COLOUR_DIM: u32 = 218;
/// A pixel this far from grey, of 255, counts as fully coloured.
const FULL_COLOUR: u32 = 48;

impl PaperKind {
    pub fn rgb(self) -> u32 {
        match self {
            Self::White => 0xFF_FF_FF,
            Self::Dark => DARK_PAPER,
        }
    }
}

fn paper_channels() -> [u32; 3] {
    [(DARK_PAPER >> 16) & 0xFF, (DARK_PAPER >> 8) & 0xFF, DARK_PAPER & 0xFF]
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
    let (b, g, r) = (u32::from(pixel[0]), u32::from(pixel[1]), u32::from(pixel[2]));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    // Over: 255 is paper, 0 is ink. Rec. 601 weights, in 256ths.
    let luma = (77 * r + 150 * g + 29 * b) >> 8;
    let over = |paper: u32, ink: u32| (paper * luma + ink * (255 - luma)) / 255;
    let (over_r, over_g, over_b) = (over(paper_r, DARK_INK[0]), over(paper_g, DARK_INK[1]), over(paper_b, DARK_INK[2]));
    let colour = ((max - min).min(FULL_COLOUR) * 256) / FULL_COLOUR;
    let keep = |value: u32, turned: u32| (((value * COLOUR_DIM) >> 8) * colour + turned * (256 - colour)) >> 8;
    pixel[0] = keep(b, over_b) as u8;
    pixel[1] = keep(g, over_g) as u8;
    pixel[2] = keep(r, over_r) as u8;
}

/// Turns a tile's pixels over, in place. Bytes come from a worker that may be hostile, and this only does arithmetic on them. `grey` says red, green and blue are equal, as the renderer found, which is the common text tile: the
/// answer depends on one byte, so a table does it. But a compromised worker can say so falsely, and all that gets is a wrong picture, which it can draw anyway.
pub fn turn_over(pixels: &mut [u8], width: u32, height: u32, stride: u32, grey: bool) {
    let (row_bytes, stride) = (width as usize * 4, stride as usize);
    if stride < row_bytes || stride == 0 {
        return;
    }
    if grey {
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
    for row in pixels.chunks_mut(stride).take(height as usize) {
        if let Some(row) = row.get_mut(..row_bytes) {
            for pixel in row.as_chunks_mut::<4>().0 {
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

    /// Failure: a chart's blues become oranges, or its light blues go dark, because colour was turned over too. In the concept coloured figures keep their colours.
    #[test]
    fn colour_keeps_its_hue_and_brightness_order() {
        let (light, dark) = (dark_rgb(0xA0_D0_F0), dark_rgb(0x20_60_A0));
        assert!(luma(light) > luma(dark), "{light:06x} {dark:06x}");
        for rgb in [light, dark] {
            assert!(rgb & 0xFF > (rgb >> 16) & 0xFF, "still blue: {rgb:06x}");
        }
    }

    /// Failure: the table the grey path uses disagrees with the path for colour, so a grey tile beside a colour tile shows a seam.
    #[test]
    fn grey_tiles_and_colour_tiles_agree() {
        let mut one: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v, 0xFF]).collect();
        let mut two = one.clone();
        turn_over(&mut one, 256, 1, 1024, true);
        turn_over(&mut two, 256, 1, 1024, false);
        assert_eq!(one, two);
    }

    /// Failure: a hostile stride panics or writes outside a row.
    #[test]
    fn a_bad_stride_changes_nothing_or_stays_in_bounds() {
        let mut pixels = vec![9u8; 64];
        turn_over(&mut pixels, 100, 100, 4, false);
        assert_eq!(pixels, vec![9u8; 64], "a stride narrower than the row");
        turn_over(&mut pixels, 8, 100, 0, true);
        turn_over(&mut pixels, 4, 100, 16, true);
    }
}
