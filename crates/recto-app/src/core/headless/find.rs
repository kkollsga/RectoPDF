//! The sidebar's Find across a quit and a launch whose state comes in late, and whose page text comes later still.

use crate::core::controls::UiAction;
use crate::core::find;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_platform::slow::Seam;

/// What Find shows: its rows, its status, the row of the hit gone to, and whether it's still looking.
fn shown(app: &HeadlessApp) -> (u32, String, Option<u32>, bool) {
    let view = find::view(app.core());
    (view.row_count, view.status, view.current_row, view.running)
}

/// The selection on the page, as the page view has it: a hit gone to is selected there.
fn selected(app: &HeadlessApp) -> Option<recto_index::select::Range> {
    app.core().panes.reading_engine(&app.core().engine).selection().hot()?.1
}

/// Failure (slow mode, 2026-11-11; the slow stage's `find_comes_back_with_its_hits_and_the_hit_gone_to`): Find came back with its hits and not the hit gone
/// to, as with the state late its hits are found again after it's taken in, and the hit was chosen again only if they were there first; or the hit went with
/// the window that went up without the state when it was made again. Decision 23: Find's field, its hits, found again as they're never kept, and the hit gone
/// to, chosen again by its place and selected on the page. The state's read and the window's remake held, and let through in the order a slow machine has them:
/// the state, the hits found again on the window up without it, then the remake. The third hit is the current one and selected on both windows, and the next
/// step goes on from it. (Find reads its pages on its own thread through a worker of its own, at the worker's seam, which isn't held here.)
#[test]
fn the_hit_gone_to_is_chosen_again_when_its_hits_come_after_the_state() {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    // `pdf page 1` on page 1 and pages 10 to 19: eleven hits on eleven pages.
    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.settle();
    for _ in 0..3 {
        app.act(UiAction::FindReturn { forward: true });
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
    app.act(UiAction::FindReturn { forward: true });
    app.settle();
    assert_eq!(shown(&app).2, Some(7), "and it goes on from there");
}

/// Failure (slow mode, 2026-11-14: the hand-off test's settle ran out on "Find looking", and nothing said whether one search was slow or searches kept starting
/// again): with the worker's answers late, Find starts its search over, on the session thread's next call after a page's answer (its own news), and never
/// finishes, reading pages without end; or a wait that runs out on it says only that Find is looking. Each page's text held at the worker's seam, a session
/// call made while it's held as a late answer's news makes one, then let through, one at a time however long each is held: Find reads each page once, in one
/// run, and finishes; and while it's held, what's under way says which run it is and how far it's got.
#[test]
fn find_with_late_answers_reads_each_page_once_and_says_how_far_it_is() {
    const PAGES: u64 = 12;
    let disk = Disk::new();
    let doc = disk.pdf("late.pdf", PAGES as usize);
    let app = disk.start();
    app.open(&doc);
    app.settle();
    let finder = app.core().host.session.finder().clone();
    recto_platform::slow::gate::hold(&[Seam::Worker]);
    // Found on no page, as the hand-off test's query is: one read a page, and no page drawn to check a hit's text isn't hidden.
    app.act(UiAction::FindTyped {
        text: "not on any page".into(),
    });
    app.wait_until("Find started", || finder.looking().is_some());
    let mut run = None;
    let mut read = 0;
    loop {
        app.wait_until("a page's text held at the worker, or Find done", || {
            app.waiting(Seam::Worker) > 0 || finder.looking().is_none()
        });
        let Some((now, _)) = finder.looking() else { break };
        assert!(read < PAGES, "page {} read of a {PAGES}-page document: Find started again ({now})", read + 1);
        assert_eq!(*run.get_or_insert(now), now, "Find started again at page {read}");
        let busy = crate::settled::under_way(&crate::settled::watched!(app.core()), crate::settled::PageViews::AtRest);
        let said = format!("Find looking (run {now}, page {read} of {PAGES})");
        assert!(busy.contains(&said), "a wait running out now says how far Find is: {busy:?}, not {said:?}");
        // What a late answer's news is: a call on the session thread, which keeps Find with the document.
        app.send(crate::queue::CoreCall::Find(crate::core::find::FindCall::News));
        app.let_through(Seam::Worker, 1);
        read += 1;
    }
    recto_platform::slow::gate::open(Seam::Worker);
    app.settle();
    assert_eq!(read, PAGES, "each page read once");
    let view = find::view(app.core());
    assert!(!view.running, "Find finished: {}", view.status);
    recto_agent::find::Finding::with_results(&finder, &mut |results| {
        assert_eq!(Some(results.generation), run, "in the one run");
        assert_eq!(results.end, Some(recto_agent::find::End::Finished), "{:?}", results.progress);
        assert_eq!(u64::from(results.progress.pages_done), PAGES);
    });
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

/// Failure (the last ask wins): the person's ⌘G with Find closed is kept for its hit while the query runs, and a new query typed before it came was still
/// made when the new query's hits arrived, so Find jumped to a hit nobody asked for in what they had just typed. Typing a query ends the kept step: its
/// hits come with no hit gone to.
#[test]
fn a_step_kept_for_its_hit_is_forgotten_when_a_new_query_is_typed() {
    let app = findable_for_kept_step();
    let finder = app.core().host.session.finder().clone();
    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.settle();
    app.act(UiAction::FindClose);
    app.settle();
    recto_platform::slow::gate::hold(&[Seam::Worker]);
    app.act(UiAction::Menu { id: "find.next".into() });
    app.wait_until("Find started again for the step", || finder.looking().is_some());
    app.act(UiAction::FindTyped { text: "pdf page 2".into() });
    recto_platform::slow::gate::open(Seam::Worker);
    app.settle();
    let view = find::view(app.core());
    assert!(view.open && view.row_count > 0 && !view.running, "{view:?}");
    assert_eq!(
        view.current_row, None,
        "no hit gone to in what was typed after the step was asked for: {view:?}"
    );
}

/// Failure (review 2026-11-27): the person's ⌘G with Find closed is kept for its hit while the query runs, and when the query's last news came during a
/// hand-off's freeze nothing tried the step once the freeze lifted, so Find ended open with no hit gone to. A hand-off that didn't go ahead steps it.
#[test]
fn a_step_kept_for_its_hit_is_made_when_a_freeze_that_held_its_news_lifts() {
    let app = kept_step_under_a_freeze();
    let finder = app.core().host.session.finder().clone();
    app.wait_until("the query finished", || finder.looking().is_none() && !finder.is_running());
    app.settle();
    assert_eq!(find::view(app.core()).current_row, None, "nothing steps while frozen");
    app.core().set_frozen(false);
    app.settle();
    let view = find::view(app.core());
    assert!(view.open && !view.running, "{view:?}");
    assert_eq!(view.current_row, Some(1), "the kept step made once the freeze lifted: {view:?}");
}

/// Failure: a freeze that ends in the hand-off going ahead (never lifted) must not step the kept step: nothing is moved after what comes back was written.
#[test]
fn a_step_kept_for_its_hit_is_not_made_while_the_freeze_stays() {
    let app = kept_step_under_a_freeze();
    let finder = app.core().host.session.finder().clone();
    app.wait_until("the query finished", || finder.looking().is_none() && !finder.is_running());
    app.settle();
    assert_eq!(find::view(app.core()).current_row, None, "{:?}", find::view(app.core()));
}

/// ⌘G with Find closed, held at the worker, frozen, then the query let finish.
fn kept_step_under_a_freeze() -> HeadlessApp {
    let app = findable_for_kept_step();
    let finder = app.core().host.session.finder().clone();
    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.settle();
    app.act(UiAction::FindClose);
    app.settle();
    recto_platform::slow::gate::hold(&[Seam::Worker]);
    app.act(UiAction::Menu { id: "find.next".into() });
    app.wait_until("Find started again for the step", || finder.looking().is_some());
    app.core().set_frozen(true);
    recto_platform::slow::gate::open(Seam::Worker);
    app
}

fn findable_for_kept_step() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    app
}
