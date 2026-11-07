//! The hidden-text check's pixel test: after a page is drawn, does a character's box have any ink in it? From the spike's prototype
//! (`docs/research/2026-11-06-renderer-spike.md` §10.4): 14 of 14 hidden techniques caught in the fixture, and 0.15% of 13.4 M real
//! characters flagged, with padding. It can't see hidden text beside visible text, because the box then has ink.

/// The channel range that counts as ink. From the prototype.
pub const INK_RANGE: i32 = 16;
/// How far a box grows before the test, in pixels at the check's scale. Padded by 2 px the prototype flagged 152 pages of 5,689
/// rather than 248, by forgiving thin glyphs.
pub const INK_PADDING_PX: i32 = 2;

/// Whether a device rectangle holds ink in a BGRx bitmap: the widest range of any one channel's values there is at least [`INK_RANGE`].
/// A rectangle wholly off the bitmap has no ink. Never reads outside `pixels`.
pub fn box_has_ink(pixels: &[u8], width: i32, height: i32, stride: usize, (x0, y0, x1, y1): (i32, i32, i32, i32)) -> bool {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    // Off the bitmap on any side: nothing to pad into, and padding mustn't pull in the page's edge pixels.
    if x1 <= 0 || y1 <= 0 || x0 >= width || y0 >= height {
        return false;
    }
    // These numbers come from a hostile file's boxes, so they may be at the very ends of the range.
    let (x0, x1) = (x0.saturating_sub(INK_PADDING_PX).max(0), x1.saturating_add(INK_PADDING_PX).min(width));
    let (y0, y1) = (y0.saturating_sub(INK_PADDING_PX).max(0), y1.saturating_add(INK_PADDING_PX).min(height));
    let (mut low, mut high) = (255i32, 0i32);
    for y in y0..y1 {
        let start = y as usize * stride + x0 as usize * 4;
        let end = y as usize * stride + x1 as usize * 4;
        let Some(row) = pixels.get(start..end) else { return false };
        for px in row.as_chunks::<4>().0 {
            for &channel in &px[..3] {
                low = low.min(i32::from(channel));
                high = high.max(i32::from(channel));
            }
        }
        if high - low >= INK_RANGE {
            return true;
        }
    }
    false
}

/// How close a glyph's own colour may be to the colour most of its box is drawn in, per channel, for it to be taken as drawn in the colour of what's under it.
pub const SAME_AS_GROUND: i32 = 24;

/// Whether a glyph drawn in `rgb` can't be told from what's around it: the colour that most of its padded box is drawn in is the glyph's own. That's white text over a white page with other text under it, which [`box_has_ink`] passes because the box has ink, though not the glyph's. A glyph in ordinary text covers well under half its box, so the box's main colour is its background. BGRx pixels; never reads outside `pixels`, and at most a box's worth.
pub fn glyph_is_ground_coloured(pixels: &[u8], width: i32, height: i32, stride: usize, (x0, y0, x1, y1): (i32, i32, i32, i32), rgb: (u8, u8, u8)) -> bool {
    let (x0, x1) = (x0.min(x1).saturating_sub(INK_PADDING_PX).max(0), x0.max(x1).saturating_add(INK_PADDING_PX).min(width));
    let (y0, y1) = (y0.min(y1).saturating_sub(INK_PADDING_PX).max(0), y0.max(y1).saturating_add(INK_PADDING_PX).min(height));
    if x1 <= x0 || y1 <= y0 {
        return false;
    }
    // Colours to 4 bits a channel: anti-aliasing and dithering land in the same bin.
    let mut bins = [0u32; 4096];
    let mut total = 0u32;
    for y in y0..y1 {
        let start = y as usize * stride + x0 as usize * 4;
        let end = y as usize * stride + x1 as usize * 4;
        let Some(row) = pixels.get(start..end) else { return false };
        for px in row.as_chunks::<4>().0 {
            let (b, g, r) = (usize::from(px[0] >> 4), usize::from(px[1] >> 4), usize::from(px[2] >> 4));
            bins[(r << 8) | (g << 4) | b] += 1;
            total += 1;
        }
    }
    let Some((bin, count)) = bins.iter().enumerate().max_by_key(|(_, count)| **count) else { return false };
    if *count * 2 < total {
        return false;
    }
    let centre = |part: usize| ((part as i32) << 4) + 8;
    let (r, g, b) = (centre(bin >> 8), centre((bin >> 4) & 15), centre(bin & 15));
    (r - i32::from(rgb.0)).abs() <= SAME_AS_GROUND && (g - i32::from(rgb.1)).abs() <= SAME_AS_GROUND && (b - i32::from(rgb.2)).abs() <= SAME_AS_GROUND
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Vec<u8> {
        let mut pixels = vec![255u8; 40 * 40 * 4];
        // One dark pixel at (20, 20).
        pixels[(20 * 40 + 20) * 4..(20 * 40 + 20) * 4 + 3].copy_from_slice(&[0, 0, 0]);
        pixels
    }

    /// Failure: real visible text is flagged as hidden, so an agent is told a normal page hides something.
    #[test]
    fn a_box_over_ink_has_ink() {
        assert!(box_has_ink(&page(), 40, 40, 160, (18, 18, 23, 23)));
    }

    /// Failure: white-on-white text counts as visible, so hidden instructions reach an agent unflagged.
    #[test]
    fn a_box_over_nothing_has_no_ink() {
        assert!(!box_has_ink(&page(), 40, 40, 160, (2, 2, 10, 10)));
    }

    /// Failure: white text drawn over a paragraph of black text counts as seen, because its box has the paragraph's ink, so a heading no one can see reaches the contents and `recto text` doesn't flag it. And ordinary black text on white, or white text on a dark box, is taken for hidden.
    #[test]
    fn a_glyph_in_its_grounds_colour_is_hidden_whatever_else_is_in_its_box() {
        // A white box with a few black pixels in it, as body text under a heading's box.
        let mut pixels = vec![255u8; 40 * 40 * 4];
        for x in 5..15 {
            pixels[(20 * 40 + x) * 4..(20 * 40 + x) * 4 + 3].copy_from_slice(&[0, 0, 0]);
        }
        let whole = (0, 0, 40, 40);
        assert!(box_has_ink(&pixels, 40, 40, 160, whole), "the box has ink");
        assert!(glyph_is_ground_coloured(&pixels, 40, 40, 160, whole, (255, 255, 255)), "a white glyph there can't be seen");
        assert!(!glyph_is_ground_coloured(&pixels, 40, 40, 160, whole, (0, 0, 0)), "a black one can");
        let dark = vec![20u8; 40 * 40 * 4];
        assert!(!glyph_is_ground_coloured(&dark, 40, 40, 160, whole, (255, 255, 255)), "white on a dark box can be seen");
        assert!(!glyph_is_ground_coloured(&dark, 40, 40, 160, (60, 60, 80, 80), (0, 0, 0)), "off the bitmap is nothing");
    }

    /// Failure: thin glyphs like `·` read as hidden, because their box lands a pixel beside the ink. Padding forgives that.
    #[test]
    fn padding_forgives_a_box_just_beside_the_ink() {
        assert!(box_has_ink(&page(), 40, 40, 160, (21, 21, 22, 22)));
    }

    /// Failure: off-page text counts as visible, or a box past the edge reads out of bounds and kills the worker.
    #[test]
    fn a_box_off_the_page_has_no_ink_and_reads_nothing_outside() {
        for (x0, y0, x1, y1) in [(1000, 0, 1010, 10), (-50, -50, -40, -40), (0, 500, 10, 510), (i32::MAX - 1, 0, i32::MAX, 10)] {
            assert!(!box_has_ink(&page(), 40, 40, 160, (x0, y0, x1, y1)));
        }
        assert!(box_has_ink(&page(), 40, 40, 160, (-100, -100, 1000, 1000)), "a huge box is clipped, not refused");
    }

    /// Failure: a hostile file's box reaches the ends of the integer range, and the padding overflows, panicking in a checked build and reading the wrong
    /// pixels in a release one.
    #[test]
    fn boxes_at_the_ends_of_the_range_dont_overflow() {
        assert!(box_has_ink(&page(), 40, 40, 160, (i32::MIN, i32::MIN, i32::MAX, i32::MAX)));
        assert!(!box_has_ink(&page(), 40, 40, 160, (i32::MIN, i32::MIN, i32::MIN + 1, i32::MIN + 1)));
        assert!(!box_has_ink(&page(), 40, 40, 160, (i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX)));
    }

    /// Failure: a bitmap shorter than its claimed size panics the worker.
    #[test]
    fn a_short_bitmap_doesnt_panic() {
        assert!(!box_has_ink(&[0; 8], 40, 40, 160, (0, 0, 10, 10)));
    }
}
