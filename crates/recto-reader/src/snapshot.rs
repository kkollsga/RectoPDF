//! **The snapshot tool** (SPEC section 6, Extraction): with it on, a press, drag and release on a page draws a box and takes it, and a click without a drag takes the image under the pointer. This is the whole rule, with no toolkit: where the box is, when a press is a click, and what a release asks for. The host only draws the box it's told, and the clipboard is filled off the UI thread by `recto_agent::extract`.
//!
//! Points are on a page as it's shown, from its top left, in the page's own coordinates, which are its layer's: what `recto snapshot --rect` and `recto select --at` take, so the app and an agent go by the same numbers.

use recto_index::select::{PagePoint, Rect};

/// How far a press may move, in points on the page, and still be a click. A hand that means to click moves a point or two, and a box this small would hold nothing.
pub const CLICK_SLOP: f32 = 3.0;

/// What a release asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Taken {
    /// The image under this point, if there is one.
    Click(PagePoint),
    /// This box on this page.
    Box { page: u32, rect: Rect },
    /// Nothing: a release with no press, or off any page.
    Nothing,
}

/// The tool's state: on or off, and the box being drawn. Two points, so 24 bytes and a flag, and none of it is per page or per tab.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnapshotTool {
    on: bool,
    press: Option<PagePoint>,
    now: Option<PagePoint>,
    /// The size of the pressed page, as it's shown, which the box is held to.
    page_size: (f32, f32),
}

impl SnapshotTool {
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Turns it on or off. Off drops any box being drawn.
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.press = None;
            self.now = None;
        }
    }

    /// The mouse went down on a page of this size.
    pub fn press(&mut self, point: PagePoint, page_size: (f32, f32)) {
        if !self.on {
            return;
        }
        self.press = Some(point);
        self.now = Some(point);
        self.page_size = page_size;
    }

    /// The page the button went down on, which a drag's points are given on: the host converts a point over another page onto this one (`Engine::point_on_given_page`), so a box stays on one page.
    pub fn pressed_page(&self) -> Option<u32> {
        self.press.map(|press| press.page)
    }

    /// The mouse moved with the button down, to a point given on the pressed page, past its edges if it's off it, and held to them here. A point on any other page is a mistake of the caller's and is ignored, rather than read in the wrong page's coordinates.
    pub fn drag(&mut self, point: PagePoint) {
        if self.on && self.press.is_some_and(|press| press.page == point.page) {
            self.now = Some(point);
        }
    }

    /// The box as it's being drawn, on its page and held to it, once it's more than a click. What the host draws.
    pub fn shown_box(&self) -> Option<(u32, Rect)> {
        let (press, now) = (self.press?, self.now?);
        if self.is_click(press, now) {
            return None;
        }
        let (width, height) = self.page_size;
        let (left, right) = (press.x.min(now.x).max(0.0), press.x.max(now.x).min(width));
        let (top, bottom) = (press.y.min(now.y).max(0.0), press.y.max(now.y).min(height));
        (right > left && bottom > top && [left, right, top, bottom].iter().all(|edge| edge.is_finite())).then_some((press.page, Rect { x: left, y: top, width: right - left, height: bottom - top }))
    }

    fn is_click(&self, press: PagePoint, now: PagePoint) -> bool {
        (now.x - press.x).abs() <= CLICK_SLOP && (now.y - press.y).abs() <= CLICK_SLOP
    }

    /// The button came up: what to take. The box goes either way.
    pub fn release(&mut self) -> Taken {
        let taken = match (self.press, self.now) {
            (Some(press), Some(now)) if self.is_click(press, now) => {
                let (width, height) = self.page_size;
                if press.x >= 0.0 && press.y >= 0.0 && press.x <= width && press.y <= height { Taken::Click(press) } else { Taken::Nothing }
            }
            (Some(_), Some(_)) => self.shown_box().map_or(Taken::Nothing, |(page, rect)| Taken::Box { page, rect }),
            _ => Taken::Nothing,
        };
        self.press = None;
        self.now = None;
        taken
    }

    /// Whether a box is being drawn, for the host's cursor and QA.
    pub fn is_drawing(&self) -> bool {
        self.press.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(page: u32, x: f32, y: f32) -> PagePoint {
        PagePoint { page, x, y }
    }

    /// Failure: a box is drawn backwards and comes out with a negative size, or runs past its page, or a hand's small slip on a click draws a box of nothing.
    #[test]
    fn a_drag_draws_a_box_held_to_its_page_and_a_click_is_a_click() {
        let mut tool = SnapshotTool::default();
        tool.set_on(true);
        tool.press(at(2, 300.0, 400.0), (612.0, 792.0));
        tool.drag(at(2, 301.0, 402.0));
        assert_eq!(tool.shown_box(), None, "still a click");
        tool.drag(at(2, 100.0, 900.0));
        assert_eq!(tool.shown_box(), Some((2, Rect { x: 100.0, y: 400.0, width: 200.0, height: 392.0 })), "backwards, and held to the page");
        assert_eq!(tool.release(), Taken::Box { page: 2, rect: Rect { x: 100.0, y: 400.0, width: 200.0, height: 392.0 } });
        assert_eq!(tool.shown_box(), None, "the box goes on release");
        tool.press(at(0, 50.0, 60.0), (612.0, 792.0));
        tool.drag(at(0, 52.0, 58.0));
        assert_eq!(tool.release(), Taken::Click(at(0, 50.0, 60.0)));
    }

    /// Failure: the tool acts while it's off, taking presses that are the reader selecting text, or turning it off leaves a box drawn. Or a click in the margin beside a page asks for an image there.
    #[test]
    fn off_does_nothing_and_a_click_off_the_page_takes_nothing() {
        let mut tool = SnapshotTool::default();
        tool.press(at(0, 10.0, 10.0), (612.0, 792.0));
        assert!(!tool.is_drawing());
        assert_eq!(tool.release(), Taken::Nothing);
        tool.set_on(true);
        tool.press(at(0, 10.0, 10.0), (612.0, 792.0));
        tool.drag(at(0, 200.0, 200.0));
        tool.set_on(false);
        assert_eq!((tool.shown_box(), tool.release()), (None, Taken::Nothing));
        tool.set_on(true);
        tool.press(at(0, -20.0, 10.0), (612.0, 792.0));
        assert_eq!(tool.release(), Taken::Nothing);
        tool.press(at(0, f32::NAN, 10.0), (612.0, 792.0));
        tool.drag(at(0, 100.0, 100.0));
        assert_eq!(tool.release(), Taken::Nothing, "nothing that isn't a number gets through");
    }
}
