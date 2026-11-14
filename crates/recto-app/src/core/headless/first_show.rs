//! **The window's first showing:** what its record says the decision saw, and that a page check turning true is asked again.

use crate::testkit::headless::HeadlessApp;
use serde_json::json;

/// Failure (2026-11-14, two of twenty hand-offs like CI shown by the patience or the cap, 1.5 to 2.0 s after the window went up, with nothing in the record
/// to say what held them: its checks were gathered after the show's layout, so `why: ready` sat beside `showable: false`): a show by the host's patience
/// whose record doesn't name the check that held the window, or names it from after the layout. Made here: the session's lock held, as the session thread
/// holds it for a command, so the check that only tries it says wait; the host's timer runs out and asks once more for the record, and shows. The record
/// names the session as what held it, at the timer's ask and the last wait, and keeps the checks gathered after the layout apart, under their own name.
#[test]
fn a_show_by_patience_names_the_check_that_held_it() {
    let app = HeadlessApp::start();
    app.open(&app.disk().pdf("one.pdf", 3));
    let inner = app.core();
    inner.window_closed();
    inner.window_opened();
    let ask = || inner.life.ready_to_show(&inner.host.session, &inner.engine);
    assert!(!inner.host.session.with_session(|_| ask()), "the session busy, the window waits");
    assert!(!inner.host.session.with_session(|_| ask()), "and as the host's timer runs out");
    inner.life.record_window_shown(false, 0, "patience", &inner.host.session, &inner.engine);
    let shown = inner.life.first_shown().expect("the record of the showing");
    assert_eq!(shown["why"], "patience", "{shown:#}");
    assert_eq!(shown["asks"]["count"], 2, "{shown:#}");
    for which in ["last", "last_wait"] {
        let asked = &shown["asks"][which];
        assert_eq!(asked["said"], "wait", "{which}: {shown:#}");
        assert!(
            asked["held_by"].as_array().unwrap().contains(&json!("session_busy")),
            "{which} names the session: {shown:#}"
        );
        assert_eq!(asked["checks"]["session_free"], false, "{which}: {shown:#}");
    }
    assert!(shown["asks"]["last"]["ms"].as_f64().unwrap() >= shown["asks"]["last_wait"]["ms"].as_f64().unwrap());
    assert_eq!(
        shown["after_layout"]["session_free"], true,
        "gathered after the show, with the lock let go: {shown:#}"
    );
    assert!(shown.get("when_shown").is_none(), "the old name, which read as what was decided on: {shown:#}");
}
