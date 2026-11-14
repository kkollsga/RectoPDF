//! **The person's scroll, as the host tells of it** ([`HeadlessApp::person_scrolls`]): a test's core has no host to scroll its page view, so a jump it's
//! sent is never arrived at unless a test says so.

use super::headless::HeadlessApp;

/// The page being read's view as [`HeadlessApp::person_scrolls`] has it: where it's scrolled to and the last scroll it was sent that it's arrived at, and
/// the last scroll it was sent, from the layer changes taken, each `(y, seq)`.
#[derive(Default)]
pub(super) struct ScrolledView {
    at: (f64, u64),
    sent: Option<(f64, u64)>,
}

impl HeadlessApp {
    pub(super) fn note_scrolls_sent(&self, ops: &[recto_reader::Op]) {
        let sent = ops.iter().rev().find_map(|op| match op {
            recto_reader::Op::ScrollTo { y, seq } => Some((*y, *seq)),
            _ => None,
        });
        if sent.is_some() {
            crate::lock(&self.view).sent = sent;
        }
    }

    /// **The person scrolls the page being read `dy` points down** (up if less than 0), as the host tells of its view on the UI thread, by the window's one
    /// way ([`crate::panes::Panes::viewport`]): the view first arrives where it was last sent, as a scroll the host's been sent lands, and then moves by the
    /// reader's hand. The view is 0 high, as nothing in a test's core says otherwise. Doesn't settle.
    pub fn person_scrolls(&self, dy: f64) {
        let inner = self.core();
        let scroll = self.person_scroll_ahead(dy);
        super::headless::applied_as_the_host(&inner.panes.viewport(&inner.engine, inner.panes.reading(), true, scroll).0);
    }

    /// [`Self::person_scrolls`], with the view's arrival where it was last sent told of now and the reader's own scroll by `dy` given back, for the test to
    /// tell of when it chooses, as the host would: `Panes::viewport(engine, pane, true, it)`.
    #[must_use]
    pub fn person_scroll_ahead(&self, dy: f64) -> recto_reader::Viewport {
        let inner = self.core();
        let reading = inner.panes.reading_engine(&inner.engine);
        let drained = reading.drain();
        self.note_scrolls_sent(&drained);
        super::headless::applied_as_the_host(&drained);
        reading.applied();
        let mut view = crate::lock(&self.view);
        let viewport = |(y, seq): (f64, u64)| recto_reader::Viewport {
            y,
            magnification: 1.0,
            backing: 2.0,
            scrolled_to: seq,
            ..recto_reader::Viewport::default()
        };
        if let Some(sent) = view.sent.take() {
            view.at = sent;
            super::headless::applied_as_the_host(&inner.panes.viewport(&inner.engine, inner.panes.reading(), false, viewport(sent)).0);
        }
        view.at.0 += dy;
        viewport(view.at)
    }
}
