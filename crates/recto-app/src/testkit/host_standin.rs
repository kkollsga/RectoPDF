//! The stand-in host's report of where the view is: the shared step for a host reporting where it is. `host_turns` (`headless.rs`) and the other tests' own stand-ins still carry their own copies; moving them onto this is a tracked follow-up.

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
    let began = std::time::Instant::now();
    let queued = engine.drain();
    if queued.is_empty() {
        let first = engine.set_viewport(*view);
        apply_batch(engine, view, &first, began);
    } else {
        apply_batch(engine, view, &queued, began);
    }
}

// A batch is applied as the host's `apply` does: arrive at its last scroll, release its surfaces, send the viewport (which hands back a batch of its
// own, applied the same way), and only then say it's applied; for a moment before that everything would look settled on the old view.
fn apply_batch(engine: &recto_reader::Engine, view: &mut recto_reader::Viewport, ops: &[recto_reader::Op], began: std::time::Instant) {
    if ops.is_empty() {
        return;
    }
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
    applied_as_the_host(ops);
    assert!(began.elapsed() < WAIT_LIMIT, "the view kept being sent to a new place: {}", engine.report().0);
    let next = engine.set_viewport(*view);
    apply_batch(engine, view, &next, began);
    engine.applied();
}

/// Scrolls the view by `dy` as the person's hand does and says where it is, the same way: the host first arrives at every scroll it was sent (a jump the
/// session pass queued included, [`report_where_the_host_is`]), and only then moves from where it is, never above the top. Failure caught: a test that added to the view's old
/// `y` and sent it with "arrived at every scroll" before a queued jump was drained moved the view from the wrong place, or cleared the jump unapplied.
pub fn the_person_scrolls(app: &HeadlessApp, view: &mut recto_reader::Viewport, dy: f64) {
    report_where_the_host_is(app, view);
    view.y = (view.y + dy).max(0.0);
    let began = std::time::Instant::now();
    let engine = &app.core().engine;
    let ops = engine.set_viewport(*view);
    apply_batch(engine, view, &ops, began);
}
