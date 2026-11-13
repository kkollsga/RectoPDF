//! The sidebar's Find across a quit and a launch whose state comes in late, and whose page text comes later still.

use crate::core::find::{self, FindCall};
use crate::queue::CoreCall;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_platform::slow::Seam;

/// What Find shows: its rows, its status, the row of the hit gone to, and whether it's still looking.
fn shown(app: &HeadlessApp) -> (u32, String, Option<u32>, bool) {
    let view = find::view(app.core());
    (view.row_count, view.status, view.current_row, view.running)
}

/// The selection on the page, as the page view has it: a hit gone to is selected there.
fn selected(app: &HeadlessApp) -> Option<recto_index::select::Range> {
    app.core().panes.reading_engine(&app.core().engine).hot_selection()?.1
}

/// Failure (slow mode, 2026-11-11; the slow stage's `find_comes_back_with_its_hits_and_the_hit_gone_to`): Find came back with its hits and not the hit gone
/// to, as with the state late its hits are found again after it's taken in, and the hit was chosen again only if they were there first; or the hit went with
/// the window that went up without the state when it was made again. Decision 23: Find's field, its hits, found again as they're never kept, and the hit gone
/// to, chosen again by its place and selected on the page. The state's read and the window's remake held, and let through in the order a slow machine has them:
/// the state, the hits found again on the window up without it, then the remake. The third hit is the current one and selected on both windows, and the next
/// step goes on from it. (The kit's Find reads pages on its own thread, not through the renderer's workers, so `Seam::Worker` holds nothing of it.)
#[test]
fn the_hit_gone_to_is_chosen_again_when_its_hits_come_after_the_state() {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    // `pdf page 1` on page 1 and pages 10 to 19: eleven hits on eleven pages.
    app.send(CoreCall::Find(FindCall::Typed("pdf page 1".into())));
    app.settle();
    for _ in 0..3 {
        app.send(CoreCall::Find(FindCall::Step(
            true,
            recto_agent::find::Going::Jump(recto_session::Author::Owner),
        )));
    }
    app.settle();
    let before = shown(&app);
    assert_eq!(
        (before.0, before.2),
        (22, Some(5)),
        "eleven hits, a page row each, and the third hit's row: {before:?}"
    );
    let hit = selected(&app).expect("the hit gone to is selected on the page");
    let (_, disk) = app.quit();
    let app = disk.builder().hold(&[Seam::State, Seam::Remake]).start();
    assert_eq!(shown(&app).0, 0, "nothing found before the state's in");
    app.let_state_in();
    assert_eq!(
        shown(&app),
        before,
        "the hits, and the hit gone to chosen again, on the window up without the state"
    );
    assert_eq!(selected(&app), Some(hit), "and selected there");
    app.let_through(Seam::Remake, 1);
    app.settle();
    assert!(app.core().life.remade(), "the window made again from the state");
    assert_eq!(shown(&app), before, "still, on the window made from it");
    assert_eq!(selected(&app), Some(hit), "and selected there");
    app.send(CoreCall::Find(FindCall::Step(
        true,
        recto_agent::find::Going::Jump(recto_session::Author::Owner),
    )));
    app.settle();
    assert_eq!(shown(&app).2, Some(7), "and it goes on from there");
}

/// Failure: an agent's `recto find --next` goes to a hit and the page view doesn't select it, as ⌘G's does, because what the window does after a command was
/// worked out from its answer by the host and not said by the command (R2.3's effects). The hit gone to is selected on the page, at its characters.
#[test]
fn an_agents_step_to_a_hit_selects_it_on_the_page() {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    app.recto(recto_agent::Level::Control, &["find", "pdf page 1"]).expect("found");
    let went = app.recto(recto_agent::Level::Control, &["find", "--next"]).expect("a step");
    app.settle();
    let to = &went["went_to"];
    let range = selected(&app).expect("the hit gone to is selected on the page");
    assert_eq!(
        (u64::from(range.start.page) + 1, u64::from(range.start.at), u64::from(range.end.at)),
        (to["page"].as_u64().unwrap(), to["start"].as_u64().unwrap(), to["end"].as_u64().unwrap()),
        "at the hit's characters: {went}"
    );
}
