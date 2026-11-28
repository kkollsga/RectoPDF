//! **A comparison's panes after a quit or a hand-off:** every tab side by side comes up on its paper from its sleep file before anything's drawn, not
//! only the tab being read.

use super::sleeping::window_view;
use crate::testkit::headless::{Disk, HeadlessApp, applied_as_the_host, host_turns};
use recto_platform::slow::Seam;

fn full(app: &HeadlessApp, words: &[&str]) -> serde_json::Value {
    app.recto(recto_agent::Level::Full, words)
        .unwrap_or_else(|error| panic!("{words:?}: {error:?}"))
}

/// Failure (2026-11-28, `restore update::an_update_in_the_middle_of_things_changes_nothing` under load: the window up at the cap with pane 0 blank,
/// `page_layers` 0, its wake `had: "nothing"`): a quit's or a hand-off's flush wrote the sleep file of the active tab only, so in a comparison the other
/// pane's tab, warm all along, had none, and the next launch laid its pages on their paper only once a worker said their sizes. With two tabs side by side
/// (the one read in pane 1), a quit, and the next launch's workers held so nothing's drawn and no size learned: each pane wakes from its facts, with its
/// pages in view on their paper.
#[test]
fn every_pane_of_a_comparison_comes_up_on_its_paper_after_a_quit() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 12));
    let app = disk.start();
    let mut view = window_view();
    applied_as_the_host(&app.core().engine.set_viewport(view));
    for doc in [&a, &b] {
        app.open(doc);
        host_turns(&app, &mut view, true);
    }
    full(&app, &["compare", "--tabs", "#1,#2"]);
    host_turns(&app, &mut view, true);
    let session = app.session_json();
    assert_eq!(session["comparison"]["tabs"], serde_json::json!([1, 2]), "{}", session["comparison"]);
    assert_eq!(session["active"], 2, "the tab read is in pane 1, so pane 0's isn't the active one");
    let (_, disk) = app.quit();

    let app = disk.builder().hold(&[Seam::Worker]).start();
    let inner = app.core();
    for pane in 0..2 {
        let engine = inner.panes.engine(&inner.engine, pane).unwrap_or_else(|| panic!("pane {pane}'s page view"));
        applied_as_the_host(&engine.set_viewport(view));
    }
    host_turns(&app, &mut view, false);
    for (pane, tab) in [(0, 1), (1, 2)] {
        let engine = inner.panes.engine(&inner.engine, pane).expect("its page view");
        let report = engine.report().0;
        let wake = &report["last_wake"];
        assert_eq!(wake["tab"], tab, "pane {pane}: {wake}");
        assert_eq!(
            wake["had"],
            "facts",
            "pane {pane}'s tab came up with nothing from a sleep file: {wake} {}",
            app.record_json()["sleep"]
        );
        assert!(
            report["tiles"]["page_layers"].as_u64().unwrap() > 0,
            "pane {pane} blank with nothing drawn: {}",
            report["tiles"]
        );
    }
    recto_platform::slow::gate::open(Seam::Worker);
    drop(app.quit());
}
