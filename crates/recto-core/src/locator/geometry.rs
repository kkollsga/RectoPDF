//! Conversion at the existing shown-frame edge, never a second PDF coordinate convention.

use super::{Error, MAX_POINT, Spot};
use crate::nav::Frame;

fn size(frame: Frame) -> Result<(f32, f32), Error> {
    if frame.turns > 3 || ![frame.left, frame.bottom, frame.right, frame.top].iter().all(|edge| edge.is_finite()) {
        return Err(Error::InvalidGeometry);
    }
    frame.shown_size().ok_or(Error::InvalidGeometry)
}

fn rounded(value: f32, bound: f32) -> Result<u16, Error> {
    if !value.is_finite() || value < 0.0 || value > bound || value > f32::from(MAX_POINT) {
        return Err(Error::InvalidGeometry);
    }
    // Only already-valid input reaches rounding. An A4 foot at 841.89 pt rounds inward to 841, not outside to 842.
    Ok(value.round().min(bound.floor()) as u16)
}

impl Spot {
    /// Round a valid shown point to whole points, inward at fractional page edges (less than one point of error).
    /// Input outside the actual page is rejected before rounding; it is never brought into the page by rounding.
    pub fn from_shown_point(x: f32, y: f32, frame: Frame) -> Result<Self, Error> {
        let (width, height) = size(frame)?;
        if x > width || y > height {
            return Err(Error::InvalidGeometry);
        }
        let spot = Self::point(rounded(x, width)?, rounded(y, height)?)?;
        spot.checked_shown(frame)?;
        Ok(spot)
    }

    /// Round a positive-area shown box's edges, rejecting boxes which collapse under whole-point rounding.
    pub fn from_shown_box(rect: [f32; 4], frame: Frame) -> Result<Self, Error> {
        let (width, height) = size(frame)?;
        let [x0, y0, x1, y1] = rect;
        if !rect.iter().all(|edge| edge.is_finite()) || x1 <= x0 || y1 <= y0 || x1 > width || y1 > height {
            return Err(Error::InvalidGeometry);
        }
        let (x0, y0, x1, y1) = (rounded(x0, width)?, rounded(y0, height)?, rounded(x1, width)?, rounded(y1, height)?);
        let width = x1.checked_sub(x0).filter(|width| *width > 0).ok_or(Error::InvalidGeometry)?;
        let height = y1.checked_sub(y0).filter(|height| *height > 0).ok_or(Error::InvalidGeometry)?;
        let spot = Self::area(x0, y0, width, height)?;
        spot.checked_shown(frame)?;
        Ok(spot)
    }

    /// A PDF-space box passes through the established crop/turn conversion, then the shown-box rules above.
    pub fn from_pdf_box(rect: [f32; 4], frame: Frame) -> Result<Self, Error> {
        if rect[2] <= rect[0] || rect[3] <= rect[1] {
            return Err(Error::InvalidGeometry);
        }
        size(frame)?;
        Self::from_shown_box(frame.to_shown(rect).ok_or(Error::InvalidGeometry)?, frame)
    }

    /// Validate against the actual current frame before use. The caller must separately verify its frame tag.
    /// A point is returned as a zero-area rectangle; a box keeps its positive extent.
    pub fn checked_shown(self, frame: Frame) -> Result<[f32; 4], Error> {
        let (width, height) = size(frame)?;
        let [w, h] = self.extent.unwrap_or([0, 0]);
        let rect = [f32::from(self.x), f32::from(self.y), f32::from(self.x + w), f32::from(self.y + h)];
        if rect[2] > width || rect[3] > height {
            return Err(Error::InvalidGeometry);
        }
        Ok(rect)
    }

    /// A contents/view thousandths value uses the existing core scale and rounding, with no new fraction convention.
    pub fn from_thousandths(top: u16, frame: Frame) -> Result<Self, Error> {
        if top > crate::TOP_SCALE {
            return Err(Error::OutOfRange);
        }
        let (_, height) = size(frame)?;
        Self::from_shown_point(0.0, (crate::fraction_down(top) * f64::from(height)) as f32, frame)
    }

    /// The target's top, in the same thousandths used by contents and jumps. Whole-point quantization is intentional.
    pub fn thousandths(self, frame: Frame) -> Result<u16, Error> {
        self.checked_shown(frame)?;
        let (_, height) = size(frame)?;
        Ok(crate::thousandths(f64::from(self.y) / f64::from(height)))
    }
}
