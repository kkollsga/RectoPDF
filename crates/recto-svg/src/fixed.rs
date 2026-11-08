//! Numbers as text, through integers. Going by way of a whole number on a fixed grid means no form of a float's own text can reach the output: no `1e-7`, no `NaN`, no `inf`.

/// The longest a number can be: a sign, 19 digits and a point.
pub(crate) const MAX_LEN: usize = 21;
/// The most any coordinate says. A shape a thousand kilometres away is on no page, and a reader's own arithmetic stays sound.
pub const MAX_COORD: f64 = 1e9;

/// `value` as a whole number of `10^-digits`. Not finite is 0, and past [`MAX_COORD`] stops there.
pub fn fixed_grid(value: f64, digits: u32) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    (value.clamp(-MAX_COORD, MAX_COORD) * 10f64.powi(digits as i32)).round() as i64
}

/// Writes `value / 10^digits` with as few characters as it takes: `-.5`, `3`, `0`. Returns the length.
pub(crate) fn write(out: &mut [u8; MAX_LEN], value: i64, digits: u32) -> usize {
    let mut len = 0;
    if value < 0 {
        out[0] = b'-';
        len = 1;
    }
    let mut magnitude = value.unsigned_abs();
    let mut text = [0u8; 20];
    let mut count = 0;
    while magnitude > 0 {
        text[count] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        count += 1;
    }
    if count == 0 {
        out[0] = b'0';
        return 1;
    }
    // `text` runs from the last digit to the first. Trailing zeros of the fraction go.
    let mut trim = 0;
    while trim < digits as usize && trim < count && text[trim] == b'0' {
        trim += 1;
    }
    let fraction_digits = (digits as usize).saturating_sub(trim);
    let whole_digits = count.saturating_sub(digits as usize);
    for i in (count.max(digits as usize) - whole_digits..count).rev() {
        out[len] = text[i];
        len += 1;
    }
    if fraction_digits > 0 {
        out[len] = b'.';
        len += 1;
        for i in (trim..digits as usize).rev() {
            out[len] = if i < count { text[i] } else { b'0' };
            len += 1;
        }
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: f64, digits: u32) -> String {
        let mut out = [0; MAX_LEN];
        let len = write(&mut out, fixed_grid(value, digits), digits);
        String::from_utf8(out[..len].to_vec()).unwrap()
    }

    /// Failure: a number comes out in scientific notation, or as `NaN` or `inf`, which SVG readers refuse or misread, and hostile input decides where that goes.
    #[test]
    fn no_number_is_ever_scientific_or_not_a_number() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1e300,
            -1e300,
            1e-300,
            1.5e-7,
            f64::MIN_POSITIVE,
            123_456_789.125,
        ] {
            for digits in [0, 3, 8] {
                let written = text(value, digits);
                assert!(
                    written.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-'),
                    "{value} became {written}"
                );
                assert!(written.parse::<f64>().is_ok(), "{written}");
            }
        }
    }

    /// Failure: a number reads back as something else, so a shape is drawn in the wrong place.
    #[test]
    fn numbers_read_back_within_their_grid_and_as_short_as_they_can_be() {
        for (value, digits, want) in [
            (0.0, 3, "0"),
            (1.0, 3, "1"),
            (-0.5, 3, "-.5"),
            (0.5, 3, ".5"),
            (12.345, 3, "12.345"),
            (12.3, 3, "12.3"),
            (100.0, 3, "100"),
            (0.001, 3, ".001"),
            (-0.04, 3, "-.04"),
            (5.0, 0, "5"),
            (1234.0, 0, "1234"),
        ] {
            assert_eq!(text(value, digits), want);
        }
        let mut seed = 0x1234_5678_9abc_def0u64;
        for _ in 0..50_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let value = ((seed >> 20) as i64 - (1i64 << 43)) as f64 / 1e6;
            for digits in [0u32, 1, 3, 4, 8] {
                let read: f64 = text(value, digits).parse().unwrap();
                let tolerance = 0.51 * 10f64.powi(-(digits as i32)) + value.abs() * 1e-12;
                assert!((read - value).abs() <= tolerance, "{value} at {digits}: {read}");
            }
        }
    }
}
