//! **Zoom:** one path and one set of limits for every way of zooming, a menu, a key, the panel, ⌘-scroll or `recto view --zoom`, and what a typed zoom level means.

use super::{Op, Reader};

/// The zoom limits. Past these a pinch stops.
pub const MIN_ZOOM: f64 = 0.25;
pub const MAX_ZOOM: f64 = 8.0;

/// What's asked of the zoom, from anywhere: a menu, a key, the panel, or `recto view --zoom`. All the same path and limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    In,
    Out,
    /// The page fills the width. Magnification 1.
    Fit,
    /// A point of the page is a point on the screen.
    Actual,
    To(f64),
    /// A share of the page's real size, where 100 is [`Zoom::Actual`]: what the zoom level in the top left says and takes.
    Percent(f64),
}

/// The zoom level as the reader is told it: a share of the page's real size, so 100% is a point of the page to a point on the screen, as in any Mac app that shows a zoom. A magnification of 1 is the fitted width, whatever that is.
pub fn zoom_percent(magnification: f64, fit: f64) -> f64 {
    let percent = magnification * fit * 100.0;
    if percent.is_finite() { percent } else { 100.0 }
}

/// What a zoom level typed by the reader asks for, or nothing if it isn't one. `150`, `150%` and ` 150 % ` are all 150%. It's held to the limits when it's applied, like any zoom.
///
/// **`.` and `,` are read the same on any Mac,** as a number shows which it means: `1.5` and `1,5` are both one and a half, whatever the Mac's decimal sign (a Mac set to `,`, as Norwegian is, once read `1.5` as 15). With both in it, the last is the decimal sign, as in `1.250,5` and `1,250.5`. One that comes more than once groups thousands, as in `1.000.000`. Only a lone one with exactly three digits after it and one to three before can be either, `1,000` or `1.000`, and there `decimal`, the Mac's own decimal sign, says which. Spaces and `'` group thousands too.
pub fn typed_zoom(text: &str, decimal: char) -> Option<Zoom> {
    let text = text.trim();
    let number = text.strip_suffix('%').unwrap_or(text).trim();
    // A number, and nothing a hostile paste could make long: past this it isn't a zoom level anyone typed.
    if number.is_empty() || number.chars().count() > 16 {
        return None;
    }
    let marks: String = number.chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\'')).collect();
    if !marks.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b',') {
        return None;
    }
    let point = decimal_point(&marks, decimal)?;
    // A mark that groups has three digits after it, up to the next mark: `1.2,5` isn't a number anyone means.
    let whole = &marks[..point.unwrap_or(marks.len())];
    if whole.split(['.', ',']).skip(1).any(|group| group.len() != 3) || (whole.contains(['.', ',']) && whole.starts_with(['.', ','])) {
        return None;
    }
    let plain: String = marks
        .char_indices()
        .filter_map(|(at, c)| match c {
            '.' | ',' => (Some(at) == point).then_some('.'),
            digit => Some(digit),
        })
        .collect();
    if plain.is_empty() || plain == "." || plain.len() > 12 {
        return None;
    }
    plain.parse::<f64>().ok().filter(|value| value.is_finite() && *value > 0.0).map(Zoom::Percent)
}

/// Where the decimal sign is in a number of digits, `.` and `,`, by [`typed_zoom`]'s rule: `Ok(None)` for a whole number, and `None` for marks no number has, such as a decimal sign twice beside the other mark.
fn decimal_point(marks: &str, local: char) -> Option<Option<usize>> {
    let Some(last) = marks.rfind(['.', ',']) else { return Some(None) };
    let mark = marks.as_bytes()[last] as char;
    let other = if mark == '.' { ',' } else { '.' };
    let times = marks.matches(mark).count();
    if marks.contains(other) {
        // `1.250,5`: the last is the decimal sign, and the other only groups, before it.
        return (times == 1 && marks.rfind(other) < Some(last)).then_some(Some(last));
    }
    if times > 1 {
        return Some(None);
    }
    let (before, after) = (last, marks.len() - last - 1);
    let either = after == 3 && (1..=3).contains(&before);
    let local = if local == ',' { ',' } else { '.' };
    Some((!either || mark == local).then_some(last))
}

/// How much one step of zoom is.
pub const ZOOM_STEP: f64 = 1.25;

/// How much a point of ⌘-scroll zooms, as a power: gentle enough that a flick of two fingers goes a step or two.
const ZOOM_PER_SCROLL_POINT: f64 = 0.006;

/// The zoom after this much ⌘-scroll, in points, upwards being in. Held to the limits, as a pinch is.
pub fn zoom_after_scroll(current: f64, points: f64) -> f64 {
    if !current.is_finite() || !points.is_finite() {
        return current.clamp(MIN_ZOOM, MAX_ZOOM);
    }
    (current * (points * ZOOM_PER_SCROLL_POINT).exp()).clamp(MIN_ZOOM, MAX_ZOOM)
}

impl Reader {
    /// Zooms, wherever it's asked from. The host keeps the middle of the view where it is, and tiles stretch until new ones come, as with any zoom.
    pub fn zoom(&mut self, zoom: Zoom) -> Option<f64> {
        let doc = self.hot.and_then(|tab| self.docs.get_mut(&tab))?;
        let value = match zoom {
            Zoom::In => doc.magnification * ZOOM_STEP,
            Zoom::Out => doc.magnification / ZOOM_STEP,
            Zoom::Fit => 1.0,
            Zoom::Actual => 1.0 / doc.layout.fit().max(f64::MIN_POSITIVE),
            Zoom::To(value) => value,
            Zoom::Percent(percent) => percent / 100.0 / doc.layout.fit().max(f64::MIN_POSITIVE),
        };
        let value = if value.is_finite() {
            value.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            doc.magnification
        };
        doc.magnification = value;
        self.host.ops.push_back(Op::Magnify { value });
        Some(value)
    }

    /// The tab showing's fit, if one is.
    pub fn fit(&self) -> Option<f64> {
        self.doc().map(|doc| doc.layout.fit())
    }

    /// The tab showing's magnification, if one is.
    pub fn magnification(&self) -> Option<f64> {
        self.doc().map(|doc| doc.magnification)
    }
}
