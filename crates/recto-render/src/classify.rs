//! Tells a blank tile from a grey one from a colour one, so the host can skip or shrink the surface. Platform report, §2.5.

use recto_proto::TileKind;

const RGB_MASK: u32 = 0x00FF_FFFF;

/// Looks at every pixel of a BGRx tile, a row at a time, and stops as soon as the answer is colour.
/// Runs on every tile, so the common text-page case, a full scan of a grey tile, is written to vectorise.
pub fn classify_bgrx(pixels: &[u8], width: u32, height: u32, stride: u32) -> TileKind {
    let row_bytes = width as usize * 4;
    let mut rows = pixels.chunks(stride as usize).take(height as usize).map(|row| &row[..row_bytes.min(row.len())]);
    let Some(first_pixel) = rows.clone().next().and_then(|row| row.first_chunk::<4>()) else {
        return TileKind::Colour;
    };
    let first = u32::from_le_bytes(*first_pixel) & RGB_MASK;
    let (mut blank, mut grey) = (true, true);
    for row in &mut rows {
        // No early exit inside a row: a plain fold over `u32`s is what the compiler can vectorise.
        let (differs, chroma) = row.as_chunks::<4>().0.iter().fold((0u32, 0u32), |(differs, chroma), px| {
            let value = u32::from_le_bytes(*px) & RGB_MASK;
            (differs | (value ^ first), chroma | u32::from(px[0] ^ px[1]) | u32::from(px[1] ^ px[2]))
        });
        blank &= differs == 0;
        grey &= chroma == 0;
        if !grey && !blank {
            return TileKind::Colour;
        }
    }
    if blank {
        TileKind::Blank { rgb: first }
    } else if grey {
        TileKind::Grey
    } else {
        TileKind::Colour
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(width: usize, height: usize, stride: usize, fill: [u8; 4]) -> Vec<u8> {
        let mut out = vec![0xAB; stride * height];
        for row in out.chunks_mut(stride) {
            for px in row[..width * 4].as_chunks_mut::<4>().0 {
                *px = fill;
            }
        }
        out
    }

    /// Failure: a tile with one dark pixel in its last corner is called blank, so the host drops real content and shows a flat colour.
    #[test]
    fn a_blank_tile_really_is_one_colour() {
        let mut pixels = tile(64, 32, 256, [255, 255, 255, 255]);
        assert_eq!(classify_bgrx(&pixels, 64, 32, 256), TileKind::Blank { rgb: 0xFF_FFFF });
        pixels[31 * 256 + 63 * 4] = 0;
        assert_ne!(classify_bgrx(&pixels, 64, 32, 256), TileKind::Blank { rgb: 0xFF_FFFF });
    }

    /// Failure: the blank colour's channels come out swapped, so a pale blue page background turns pale red on screen.
    #[test]
    fn the_blank_colour_is_reported_as_rgb() {
        let pixels = tile(8, 8, 32, [0x30, 0x20, 0x10, 0xFF]);
        assert_eq!(classify_bgrx(&pixels, 8, 8, 32), TileKind::Blank { rgb: 0x10_2030 });
    }

    /// Failure: a tile with a sliver of colour is called grey, so the host stores it in one channel and the colour is lost.
    #[test]
    fn a_grey_tile_has_no_chroma() {
        let mut pixels = tile(16, 16, 64, [255, 255, 255, 255]);
        pixels[4..8].copy_from_slice(&[10, 10, 10, 255]);
        assert_eq!(classify_bgrx(&pixels, 16, 16, 64), TileKind::Grey);
        pixels[15 * 64 + 60..15 * 64 + 64].copy_from_slice(&[10, 10, 11, 255]);
        assert_eq!(classify_bgrx(&pixels, 16, 16, 64), TileKind::Colour);
    }

    /// Failure: the unused byte or the padding past a row's end changes the answer, so identical pages classify differently.
    #[test]
    fn only_pixels_count_not_the_unused_byte_or_padding() {
        let mut pixels = tile(8, 8, 64, [200, 200, 200, 0]);
        pixels[3] = 77;
        assert_eq!(classify_bgrx(&pixels, 8, 8, 64), TileKind::Blank { rgb: 0xC8_C8C8 }, "padding bytes are 0xAB and must be ignored");
    }

    /// Failure: an empty or short buffer panics inside the worker.
    #[test]
    fn short_buffers_dont_panic() {
        assert_eq!(classify_bgrx(&[], 8, 8, 32), TileKind::Colour);
        let _ = classify_bgrx(&[1, 2, 3], 8, 8, 32);
        let _ = classify_bgrx(&[1, 2, 3, 4, 5, 6, 7, 8, 9], 8, 8, 8);
    }
}
