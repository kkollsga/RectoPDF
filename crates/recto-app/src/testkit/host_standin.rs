//! The stand-in host's report of where the view is: one implementation for every test that sends the page view a viewport.

use super::headless::{HeadlessApp, WAIT_LIMIT, applied_as_the_host};

/// Says where the view is, as the real host does (`PageArea.swift`): it arrives at every scroll it was sent before it says where it is, and never claims
/// to have arrived at a scroll it didn't apply. Failure caught: a stand-in that read the view's old place, sent it with "arrived at every scroll", and
/// dropped what `set_viewport` handed back cleared a jump the session pass had just queued as arrived, and the page view read the wrong page ("2 of 30"
/// after `goto 28`) whenever the session pass beat the stand-in to it. In order: the session thread has taken what was queued; the scrolls drained are
/// arrived at (the last one's place and number); the viewport is sent, and each scroll it hands back is arrived at and the viewport sent again, until
/// it hands back none. `view` ends where the host is.
pub fn report_where_the_host_is(app: &HeadlessApp, view: &mut recto_reader::Viewport) {
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took what was queued");
    let engine = &app.core().engine;
    let report = engine.report().0;
    let at = |key: &str| report[key].as_f64().unwrap_or(0.0);
    view.x = at("scroll_x");
    view.y = at("scroll_y");
    let arrive = |view: &mut recto_reader::Viewport, ops: &[recto_reader::Op]| {
        let last = ops.iter().rev().find_map(|op| {
            if let recto_reader::Op::ScrollTo { y, seq } = op {
                Some((*y, *seq))
            } else {
                None
            }
        });
        if let Some((y, seq)) = last {
            view.y = y;
            view.scrolled_to = seq;
        }
        last.is_some()
    };
    let queued = engine.drain();
    arrive(view, &queued);
    applied_as_the_host(&queued);
    engine.applied();
    let mut ops = engine.set_viewport(*view);
    let began = std::time::Instant::now();
    while arrive(view, &ops) {
        applied_as_the_host(&ops);
        ops = engine.set_viewport(*view);
        assert!(began.elapsed() < WAIT_LIMIT, "the view kept being sent to a new place: {}", engine.report().0);
    }
    applied_as_the_host(&ops);
}
