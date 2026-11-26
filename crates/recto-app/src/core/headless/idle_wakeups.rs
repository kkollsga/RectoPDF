//! **An idle session thread wakes at most once a second** (SIGNOFF, `docs/SIGNOFF.md`): its only clock-driven wake is the soonest of the state's five deadlines.

use crate::testkit::headless::Disk;
use std::time::Duration;

/// Failure: a deadline that is recomputed short with nothing due (a tick of a few milliseconds, a past deadline that never clears, a cool-down that never ends) wakes the session thread many times a second at rest, each a wake-up and a bit of CPU for an app nobody is using, times every open app. After a file is opened, a place changed, at rest the session has no deadline, or one a second or more away (the record's write, a minute after a place changes), and the wait it parks in ran out no time while it did. 2026-11-26: a loaded Mac's 6 wake-ups a second in the real app were looked for here and are not this thread's (it read 0 timeouts in the real app's rest windows).
#[test]
fn a_resting_session_has_no_deadline_under_a_second() {
    let disk = Disk::new();
    let pdf = disk.pdf("a.pdf", 5);
    let app = disk.builder().open(pdf).start();
    app.settle();
    let at_rest = |what: &str| {
        let left = {
            let inner = app.core();
            crate::core::session::next_deadline(&inner.saver, &inner.on_disk, &inner.life, &inner.tools)
        };
        assert!(
            left.is_none_or(|left| left >= Duration::from_secs(1)),
            "{what}: the session thread would wake for a deadline {left:?} away"
        );
    };
    at_rest("after the file opened");
    app.person(r#"{"cmd":"goto","page":3}"#);
    app.settle();
    at_rest("after a place changed");
    // The thread parked on that deadline, never woken by it: the wait that ran out is counted only when the clock does it.
    assert_eq!(app.core().core.timeouts(), 0, "no wait ran out while the clock stood still");
}
