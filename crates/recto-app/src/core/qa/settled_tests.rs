//! **`qa wait-settled` and the window's first showing.** The host shows a window when its page is ready or its patience runs out, and only then gives the
//! keyboard back to what had it; so a window that's up and not yet shown isn't at rest, and a report read then isn't what a person would see.

use super::files_tests::answering_bridge;
use super::*;
use crate::testkit::headless::HeadlessApp;

/// Failure (CI run 43539671770 on `main` at 5ae2fb1d, `restore::a_hand_off_in_the_middle_of_things_loses_nothing`: `app.find.field_has_focus: true then
/// false`): `qa wait-settled` answered with the window made from the state but not yet shown, as the host waited for its page or its patience on a starved
/// runner, and the report read then had Find's field without the keyboard, which it's given as the window's shown (`record.first_shown` was still null).
/// A window up and not yet shown keeps it waiting, and says so when it runs out of time; shown, it settles; one that goes before it's shown is owed nothing.
#[test]
fn settled_waits_for_a_window_up_and_not_yet_shown() {
    let app = HeadlessApp::start();
    let qa = answering_bridge(&app);
    let settle = |ms: u64| {
        qa.run(&Qa::QaRead(QaRead {
            verb: "settled".to_owned(),
            out: None,
            timeout_ms: Some(ms),
        }))
    };
    settle(10_000).expect("the launch's window, shown by the kit's host, settles");
    let inner = app.core();
    inner.window_closed();
    inner.window_opened();
    // The rest at rest first, by the kit's settle, which has no window to wait for: what's left is the window's showing.
    app.settle();
    let refused = settle(200).expect_err("settled with the window up and not yet shown");
    assert!(refused.message.contains("window_not_yet_shown"), "it says what it waited for: {refused:?}");
    inner.life.record_window_shown(false, 0, "ready", &inner.host.session, &inner.engine);
    settle(10_000).expect("shown, it settles");
    inner.window_closed();
    inner.window_opened();
    inner.window_closed();
    settle(10_000).expect("a window closed before it was shown is owed nothing");
}
