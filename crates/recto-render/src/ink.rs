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
    // Half or more of the box within reach of the glyph's colour: a count, with no table to fill for each of a page's characters.
    let (mut close, mut total) = (0u32, 0u32);
    let near = |pixel: &[u8; 4]| (i32::from(pixel[2]) - i32::from(rgb.0)).abs() <= SAME_AS_GROUND && (i32::from(pixel[1]) - i32::from(rgb.1)).abs() <= SAME_AS_GROUND && (i32::from(pixel[0]) - i32::from(rgb.2)).abs() <= SAME_AS_GROUND;
    for y in y0..y1 {
        let start = y as usize * stride + x0 as usize * 4;
        let end = y as usize * stride + x1 as usize * 4;
        let Some(row) = pixels.get(start..end) else { return false };
        for px in row.as_chunks::<4>().0 {
            close += u32::from(near(px));
            total += 1;
        }
    }
    total > 0 && close * 2 >= total
}

/// Fewest pixels a character has to change against its ground for it to be seen.
pub const SEEN_PIXELS: u32 = 3;

/// Whether drawing a character adds nothing anyone could see: its box drawn with it (`with`) and with its text object turned off (`without`), over the same region. A pixel counts only if the character took it farther from the colour most of its box is drawn in without it: contrast it added. So white text over a paragraph, which only paints white over black strokes on a white page, adds nothing, and bold black text on white, or white text on a dark box, adds a great deal. The cheap colour test only proposes; this decides. BGRx, both buffers `stride` bytes a row; never reads outside them.
pub fn adds_nothing_seen(with: &[u8], without: &[u8], stride: usize, (width, height): (i32, i32), (x0, y0, x1, y1): (i32, i32, i32, i32)) -> bool {
    let (x0, x1) = (x0.min(x1).saturating_sub(INK_PADDING_PX).max(0), x0.max(x1).saturating_add(INK_PADDING_PX).min(width));
    let (y0, y1) = (y0.min(y1).saturating_sub(INK_PADDING_PX).max(0), y0.max(y1).saturating_add(INK_PADDING_PX).min(height));
    if x1 <= x0 || y1 <= y0 {
        return true;
    }
    let mut bins = [0u32; 4096];
    let rows = |buffer: &'_ [u8], y: i32| buffer.get(y as usize * stride + x0 as usize * 4..y as usize * stride + x1 as usize * 4).map(|row| row.as_chunks::<4>().0.to_vec());
    for y in y0..y1 {
        let Some(row) = rows(without, y) else { return false };
        for px in row {
            bins[(usize::from(px[2] >> 4) << 8) | (usize::from(px[1] >> 4) << 4) | usize::from(px[0] >> 4)] += 1;
        }
    }
    let Some((bin, _)) = bins.iter().enumerate().max_by_key(|(_, count)| **count) else { return false };
    let centre = |part: usize| ((part as i32) << 4) + 8;
    let ground = [centre(bin & 15), centre((bin >> 4) & 15), centre(bin >> 8)];
    let (mut seen, mut changed, mut all) = (0, 0u32, 0u32);
    for y in y0..y1 {
        let (Some(drawn), Some(under)) = (rows(with, y), rows(without, y)) else { return false };
        for (px, before) in drawn.iter().zip(&under) {
            all += 1;
            if (0..3).any(|channel| (i32::from(px[channel]) - i32::from(before[channel])).abs() >= INK_RANGE) {
                changed += 1;
            }
            // Farther from the ground than it was without the character, by more than anti-aliasing: contrast it added. White painted over black strokes brings them nearer the white ground, and its grey edges too.
            let from_ground = |pixel: &[u8; 4]| (0..3).map(|channel| (i32::from(pixel[channel]) - ground[channel]).abs()).max().unwrap_or(0);
            if from_ground(px) > from_ground(before) + INK_RANGE && from_ground(px) > SAME_AS_GROUND {
                seen += 1;
                if seen >= SEEN_PIXELS {
                    return false;
                }
            }
        }
    }
    // Or it changes much of its box, against whatever's under it there: a white dot in a black one, the hollow vertex of a graph, is its whole shape. White text over a paragraph changes only where it crosses the paragraph's strokes.
    changed * 100 < all * SHAPE_PERCENT
}

/// The share of its padded box a character must change, against what's under it, to be seen by its shape alone.
pub const SHAPE_PERCENT: u32 = 18;

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

    /// Failure: bold black text, which covers more than half its box, is marked hidden because its own colour is the box's main one, so a heading, the big "990" on an IRS form, or a line of display type, goes from contents, `recto text` and a copy. Drawn with and without it, it adds black to white, and that's seen; white painted over black strokes on a white page isn't.
    #[test]
    fn what_a_glyph_adds_decides_not_its_colour() {
        let white = vec![255u8; 20 * 20 * 4];
        let mut bold = white.clone();
        for px in bold.chunks_mut(4).take(300) {
            px[..3].copy_from_slice(&[0, 0, 0]);
        }
        let whole = (0, 0, 20, 20);
        assert!(!adds_nothing_seen(&bold, &white, 80, (20, 20), whole), "bold black on white is seen");
        // Black strokes of a paragraph, and the same with white painted over some of them.
        let mut paragraph = white.clone();
        for row in 0..20 {
            for x in [3usize, 4, 10, 11] {
                paragraph[(row * 20 + x) * 4..(row * 20 + x) * 4 + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let mut painted = paragraph.clone();
        for row in 5..15 {
            painted[(row * 20 + 10) * 4..(row * 20 + 10) * 4 + 3].copy_from_slice(&[255, 255, 255]);
        }
        assert!(adds_nothing_seen(&painted, &paragraph, 80, (20, 20), whole), "white over a paragraph adds nothing");
        let dark = vec![20u8; 20 * 20 * 4];
        let mut on_dark = dark.clone();
        on_dark[..40].fill(255);
        assert!(!adds_nothing_seen(&on_dark, &dark, 80, (20, 20), whole), "white on a dark box is seen");
        assert!(adds_nothing_seen(&white, &white, 80, (20, 20), whole), "nothing changed is nothing seen");
        // A white dot drawn in a black one, a graph's hollow vertex: its whole shape changes, so it's seen.
        let disk = vec![0u8; 20 * 20 * 4];
        let mut hollow = disk.clone();
        for row in 4..16 {
            for x in 4..16 {
                hollow[(row * 20 + x) * 4..(row * 20 + x) * 4 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let mut page = white.clone();
        for row in 2..18 {
            page[row * 80 + 8..row * 80 + 72].fill(0);
        }
        let mut dotted = page.clone();
        for row in 4..16 {
            dotted[row * 80 + 16..row * 80 + 64].fill(255);
        }
        assert!(!adds_nothing_seen(&dotted, &page, 80, (20, 20), whole), "a hollow vertex is seen");
        let _ = (disk, hollow);
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
