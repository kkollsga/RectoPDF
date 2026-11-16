//! **A zoom as the host makes it, step by step** ([`HeadlessApp::zoom_step`]): what a pinch or ⌘-scroll does to the page area's view on the window's
//! thread, for the measurements of what a step costs there (input to frame, and the allocations a step makes). A test's core has no host, so this stands
//! in for the parts of `PageArea` and AppKit a zoom goes through, and nothing else.

use super::headless::{HeadlessApp, applied_as_the_host};
use recto_reader::{Op, Viewport};

/// The window's reading area, in points, as the headless measurements show it.
pub const FRAME: f64 = 1168.0;

/// The page area's view across and down, as the host has it: AppKit's `NSScrollView` with `CenteringClipView`.
#[derive(Clone, Copy, Debug)]
pub struct ZoomedView {
    pub x: f64,
    pub y: f64,
    pub magnification: f64,
    pub document_width: f64,
    /// The last `ScrollTo` the host applied, which it says back.
    pub scrolled_to: u64,
}

/// What one step made on the window's thread: how many times the view was told, and how many ops came back.
#[derive(Clone, Copy, Debug, Default)]
pub struct ZoomStep {
    pub calls: u32,
    pub ops: usize,
}

impl ZoomedView {
    fn view_width(&self) -> f64 {
        FRAME / self.magnification
    }

    fn viewport(&self, live: bool) -> Viewport {
        Viewport {
            x: self.x,
            y: self.y,
            width: self.view_width(),
            height: 900.0 / self.magnification,
            frame_width: FRAME,
            magnification: self.magnification,
            backing: 2.0,
            live,
            scrolled_to: self.scrolled_to,
        }
    }

    /// Held to the document across, or centred on one narrower than the view: `CenteringClipView.constrainBoundsRect`.
    fn hold(&mut self) {
        let width = self.view_width();
        self.x = if width > self.document_width {
            (self.document_width - width) / 2.0
        } else {
            self.x.clamp(0.0, self.document_width - width)
        };
    }

    /// Applies what the page view said, as `PageArea.apply` does. Says whether the view moved or the document changed size, which sets the host's
    /// `viewportDirty`: it tells the view again once the batch is applied.
    fn apply(&mut self, ops: &[Op]) -> bool {
        let mut dirty = false;
        for op in ops {
            match op {
                Op::Document { width, .. } => {
                    self.document_width = *width;
                    self.hold();
                    dirty = true;
                }
                Op::ScrollTo { y, seq } => {
                    self.y = *y;
                    self.scrolled_to = *seq;
                    dirty = true;
                }
                Op::ScrollAcross { x } => {
                    self.x = *x;
                    self.hold();
                    dirty = true;
                }
                Op::Magnify { value } => {
                    let middle = self.view_width() / 2.0 * self.magnification;
                    self.zoom_about(*value, middle);
                    dirty = true;
                }
                _ => {}
            }
        }
        applied_as_the_host(ops);
        dirty
    }

    /// `NSScrollView.setMagnification(_:centeredAt:)`: the point of the document `across` window points from the view's left edge stays there.
    fn zoom_about(&mut self, magnification: f64, across: f64) {
        let point = self.x + across / self.magnification;
        self.magnification = magnification;
        self.x = point - across / magnification;
        self.hold();
    }
}

impl HeadlessApp {
    /// Tells the page being read where the view is, applies what comes back and tells it again while that moved the view, as `PageArea.sendViewport`
    /// and `apply` do, bounded: what one change of the view costs the window's thread.
    fn tell_view(&self, view: &mut ZoomedView, live: bool) -> ZoomStep {
        let inner = self.core();
        let mut step = ZoomStep::default();
        for _ in 0..4 {
            let (ops, _) = inner.panes.viewport(&inner.engine, inner.panes.reading(), false, view.viewport(live));
            step.calls += 1;
            step.ops += ops.len();
            let dirty = view.apply(&ops);
            inner.panes.reading_engine(&inner.engine).applied();
            if !dirty {
                break;
            }
        }
        step
    }

    /// **The page being read shown in a window [`FRAME`] wide, at the fit,** with what it was sent arrived at: where a zoom starts.
    pub fn zoom_view(&self) -> ZoomedView {
        let inner = self.core();
        let mut view = ZoomedView {
            x: 0.0,
            y: 0.0,
            magnification: 1.0,
            document_width: FRAME,
            scrolled_to: u64::MAX,
        };
        let drained = inner.panes.reading_engine(&inner.engine).drain();
        view.apply(&drained);
        self.tell_view(&mut view, false);
        view
    }

    /// **One step of a zoom to `magnification`,** about the point `across` window points from the view's left edge, as a pinch's step or a ⌘-scroll's
    /// wheel makes it: AppKit moves the view, the host tells the page view (live, as a pinch or ⌘-scroll under way is), applies what comes back, and
    /// tells it again while that moved the view. `live` false is the step that ends the zoom.
    pub fn zoom_step(&self, view: &mut ZoomedView, magnification: f64, across: f64, live: bool) -> ZoomStep {
        view.zoom_about(magnification, across);
        self.tell_view(view, live)
    }
}
