//! **Where a thing on a page lands:** matrices, an area through them, and the one placement of an object by the matrices of the forms it's in, which the
//! contents walk (`contents.rs`) and the walk for a click (`image_places` in the parent) both use, so a picture in a form is placed the same way by each.
//! Part of the PDFium seam.

use super::{Api, FPDF_PAGEOBJECT, FS_MATRIX};
use recto_proto::contents::Mat;

pub(super) type M = [f64; 6];
pub(super) const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `first` then `next`.
pub(super) fn then(first: &M, next: &M) -> M {
    [
        first[0] * next[0] + first[1] * next[2],
        first[0] * next[1] + first[1] * next[3],
        first[2] * next[0] + first[3] * next[2],
        first[2] * next[1] + first[3] * next[3],
        first[4] * next[0] + first[5] * next[2] + next[4],
        first[4] * next[1] + first[5] * next[3] + next[5],
    ]
}

pub(super) fn apply(m: &M, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

pub(super) fn inverse(m: &M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    Some([
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ])
}

pub(super) fn to_f32(m: &M) -> Option<Mat> {
    let out = m.map(|value| value as f32);
    out.iter().all(|value| value.is_finite()).then_some(out)
}

/// An area, left bottom right top, in page coordinates.
#[derive(Clone, Copy, Debug)]
pub(super) struct Area {
    pub(super) left: f64,
    pub(super) bottom: f64,
    pub(super) right: f64,
    pub(super) top: f64,
}

impl Area {
    pub(super) fn through(self, m: &M) -> Self {
        let corners = [
            apply(m, self.left, self.bottom),
            apply(m, self.right, self.bottom),
            apply(m, self.left, self.top),
            apply(m, self.right, self.top),
        ];
        let mut out = Self {
            left: f64::INFINITY,
            bottom: f64::INFINITY,
            right: f64::NEG_INFINITY,
            top: f64::NEG_INFINITY,
        };
        for (x, y) in corners {
            out.left = out.left.min(x);
            out.right = out.right.max(x);
            out.bottom = out.bottom.min(y);
            out.top = out.top.max(y);
        }
        out
    }

    pub(super) fn is_finite(self) -> bool {
        [self.left, self.bottom, self.right, self.top].iter().all(|value| value.is_finite())
    }

    pub(super) fn meets(self, other: Self) -> bool {
        self.left <= other.right && other.left <= self.right && self.bottom <= other.top && other.bottom <= self.top
    }

    /// More than touching.
    pub(super) fn overlaps(self, other: Self) -> bool {
        self.left < other.right && other.left < self.right && self.bottom < other.top && other.bottom < self.top
    }

    pub(super) fn inside(self, other: Self) -> bool {
        self.left >= other.left && self.right <= other.right && self.bottom >= other.bottom && self.top <= other.top
    }
}

/// **What takes the contents of `form` to the page:** its own matrix, then `outer`, the matrix of what holds it. `None` if PDFium can't say its matrix, or
/// a number in it isn't finite.
pub(super) fn inside_form(api: &Api, form: FPDF_PAGEOBJECT, outer: &M) -> Option<M> {
    matrix_of(api, form).map(|own| then(&own, outer))
}

/// An object's own matrix, as PDFium has it, if every number in it is finite.
pub(super) fn matrix_of(api: &Api, object: FPDF_PAGEOBJECT) -> Option<M> {
    let mut m = FS_MATRIX::default();
    // SAFETY: the object is live with its page, and the out-pointer is live for the call.
    if unsafe { (api.FPDFPageObj_GetMatrix)(object, &raw mut m) } == 0 {
        return None;
    }
    let m = [f64::from(m.a), f64::from(m.b), f64::from(m.c), f64::from(m.d), f64::from(m.e), f64::from(m.f)];
    m.iter().all(|value| value.is_finite()).then_some(m)
}

/// **An object's area on the page:** its bounds, in the space of the form it's in, through `level`, what takes that space to the page. `None` if PDFium
/// can't say its bounds, or the area isn't finite (a hostile matrix's numbers past what a float holds).
pub(super) fn placed(api: &Api, object: FPDF_PAGEOBJECT, level: &M) -> Option<Area> {
    bounds_of(api, object).and_then(|bounds| on_the_page(bounds, level))
}

/// An object's bounds as PDFium gives them, left, bottom, right, top, in the space of the form it's in: `None` if it can't say.
pub(super) fn bounds_of(api: &Api, object: FPDF_PAGEOBJECT) -> Option<[f32; 4]> {
    let (mut left, mut bottom, mut right, mut top) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    // SAFETY: the object is live with its page, and the out-pointers are live for the call.
    let ok = unsafe { (api.FPDFPageObj_GetBounds)(object, &raw mut left, &raw mut bottom, &raw mut right, &raw mut top) };
    (ok != 0).then_some([left, bottom, right, top])
}

/// [`placed`]'s sum, apart from PDFium: `bounds` (left, bottom, right, top) through `level`, if it comes out finite.
fn on_the_page([left, bottom, right, top]: [f32; 4], level: &M) -> Option<Area> {
    let area = Area {
        left: f64::from(left),
        bottom: f64::from(bottom),
        right: f64::from(right),
        top: f64::from(top),
    }
    .through(level);
    area.is_finite().then_some(area)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (rule 6): a matrix a hostile file chose, of numbers that aren't finite, past what a float holds once two are multiplied, or that flattens
    /// everything to nothing, makes a placement panic or gives an area that isn't finite, which the walk for a click would then hand PDFium to map. A
    /// plain one is placed by its numbers, through two levels.
    #[test]
    fn a_placement_through_a_hostile_matrix_is_finite_or_none() {
        let bounds = [0.0, 0.0, 10.0, 20.0];
        let twice = [2.0, 0.0, 0.0, 2.0, 100.0, 400.0];
        let area = on_the_page(bounds, &then(&twice, &IDENTITY)).expect("a plain matrix");
        assert_eq!([area.left, area.bottom, area.right, area.top], [100.0, 400.0, 120.0, 440.0]);
        assert!(on_the_page(bounds, &[f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none(), "NaN");
        assert!(on_the_page(bounds, &[f64::INFINITY, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none(), "infinite");
        let huge = [f64::from(f32::MAX), 0.0, 0.0, f64::from(f32::MAX), 0.0, 0.0];
        // Nine forms down, each at the largest scale a float holds: past what a double holds.
        let deep = (0..9).fold(IDENTITY, |level, _| then(&huge, &level));
        assert!(on_the_page(bounds, &deep).is_none(), "past what a double holds");
        let flat = on_the_page(bounds, &[0.0; 6]).expect("flattened: a point");
        assert_eq!([flat.left, flat.bottom, flat.right, flat.top], [0.0; 4]);
    }
}
