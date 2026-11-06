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
    let (x0, x1) = ((x0 - INK_PADDING_PX).max(0), (x1 + INK_PADDING_PX).min(width));
    let (y0, y1) = ((y0 - INK_PADDING_PX).max(0), (y1 + INK_PADDING_PX).min(height));
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

    /// Failure: a bitmap shorter than its claimed size panics the worker.
    #[test]
    fn a_short_bitmap_doesnt_panic() {
        assert!(!box_has_ink(&[0; 8], 40, 40, 160, (0, 0, 10, 10)));
    }
}
