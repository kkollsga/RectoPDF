//! **A tab held by two page views at once** (the fourth security review, item 5): while a comparison's tabs shift, the session thread's look shows a
//! tab in its new pane before the pane it's leaving is given another, so for the length of that look two page views hold it. What the one it's leaving
//! says, of the place and of what it draws, is from where the tab was before the person's jump let a withheld place go, and is the session's no more.

use super::withheld_compare::left_by_following_alone_in;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_session::TabId;
use std::sync::{Arc, Mutex};

/// A plain tab `P` left by following a protected document of `pages` pages that says `word`, side by side as the second of three, `[C, P, L]`, with a
/// third plain document `C` first and the protected `L` last. Gives the app and `P`, `C`, and the page following left `P` on (from 1).
fn compared_second(pages: usize, word: &str) -> (HeadlessApp, u32, u32, u32) {
    let (app, plain_tab, locked_tab, _, _) = left_by_following_alone_in(|disk: &Disk| disk.pdf("plain.pdf", 30), pages, word);
    let third = app.disk().pdf("third.pdf", 30);
    app.open(&third);
    let tab = |name: &str| app.tabs().into_iter().find(|row| row.text == name).expect(name);
    let third_tab = tab("third").tab;
    for adding in [plain_tab, locked_tab] {
        let row = app.tabs().into_iter().find(|row| row.tab == adding).expect("its row");
        app.act(UiAction::Row { row, adding: true });
        app.settle();
    }
    let panes: Vec<Option<u32>> = (0..3).map(|pane| app.core().panes.tab_of(pane).map(|id| id.0)).collect();
    assert_eq!(
        panes,
        [Some(third_tab), Some(plain_tab), Some(locked_tab)],
        "side by side, the plain one second"
    );
    let left_on = app
        .core()
        .host
        .session
        .with_session(|s| s.tabs().tab(TabId(plain_tab)).expect("plain").page + 1);
    assert!(
        app.core()
            .host
            .session
            .with_session(|s| recto_session::compare::place_withheld(s, TabId(plain_tab))),
        "left by following"
    );
    (app, plain_tab, third_tab, left_on)
}

/// **The person's `goto to` on `P`, and their ⌘-click taking `C` out, in one look:** `P` moves from the second pane to the first, and is shown there with
/// the jump before the second pane, which still has it, is given `L`. `between` runs on the session thread at that moment. Settled, or with `held_news`
/// (a page view's news held, which settling waits for) the session thread's queue taken.
fn release_and_shift(app: &HeadlessApp, plain_tab: u32, third_tab: u32, to: u32, held_news: bool, between: impl FnOnce() + Send + 'static) {
    let held = app.hold_session_thread_aside();
    let host = &app.core().host;
    let person = |line: String| host.run_counted(recto_agent::Actor::Person, &recto_agent::parse(&line).expect("a command"), &mut Vec::new());
    person(format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":{to}}}"##)).expect("the person's goto");
    person(format!(r##"{{"cmd":"compare","toggle":"#{third_tab}"}}"##)).expect("the person's ⌘-click");
    app.core().serving.place_seams.after_the_look_shows_pane(0, between);
    held.let_go();
    if held_news {
        assert!(app.core().core.flush(crate::testkit::headless::WAIT_LIMIT), "the look taken");
    } else {
        app.settle();
    }
    assert_eq!(app.core().panes.tab_of(0).map(|id| id.0), Some(plain_tab), "the plain one first now");
}

/// Failure (the fourth security review, item 5): a read of the page views made while a look shows a tab in its new pane, with the pane it's leaving not
/// yet given another, and taken in once the look has said the jump sent (a command's or the saver's, whose reads and take-ins are apart), has the tab in
/// the pane it's leaving where it was before the person's jump, stamped after that jump's send, and that view was last given the page the jump went to
/// when it's the page following left the tab on: so that view's place is taken in, its zoom and how far down given an agent only then. Twins whose
/// protected documents differ in length, the person going in both to the page following left the first on: the place the session has of the plain tab
/// once the read is taken in is the same, none, as the jump let it go and the view it went to hasn't said where it is yet.
#[test]
fn a_view_a_tab_is_leaving_doesnt_tell_where_it_was() {
    let told = |pages: usize, word: &str, to: Option<u32>| {
        let (app, plain_tab, third_tab, left_on) = compared_second(pages, word);
        let to = to.unwrap_or(left_on);
        let kept = Arc::new(Mutex::new(None));
        let keeping = Arc::clone(&kept);
        let inner = Arc::clone(app.core());
        release_and_shift(&app, plain_tab, third_tab, to, false, move || {
            // As a command's `take_places` reads, at this moment, and takes in once the look has said the jump sent.
            let said = crate::core::places::Said::read(&inner, false);
            let given = inner.panes.given();
            let taking = Arc::clone(&inner);
            inner.serving.place_seams.after_the_look_sends(move || {
                *crate::lock(&keeping) = Some(taking.host.session.with_session(|s| {
                    said.take_in(s, &given, &[], &mut Vec::new());
                    let page = s.tabs().tab(TabId(plain_tab)).expect("plain").page;
                    let place = s.tabs().place_to_show(TabId(plain_tab));
                    (page + 1, place.map(|place| (place.anchor_page, place.within, place.zoom)))
                }));
            });
        });
        let kept = crate::lock(&kept).take().expect("taken in once the look said the jump sent");
        (format!("{kept:?}"), to, left_on)
    };
    let (five, to, left_on) = told(5, "Locked", None);
    let (nine, _, left_on_too) = told(9, "Sealed", Some(to));
    assert_ne!(left_on, left_on_too, "following left the twins on different pages");
    assert_eq!(five, nine, "the place of the view the tab was leaving taken in on the page the person went to");
    assert_eq!(
        five,
        format!("{:?}", (to, None::<(u32, f64, f64)>)),
        "nothing of a place before the view the jump went to says one"
    );
}

/// Failure (the fourth security review, items 3 and 5): a page view a tab is leaving in a comparison's shift draws it where it was at the moment the
/// tab is shown in its new pane with the person's jump (the reader zooming there), planned after that jump's send: as another view's draws they're told
/// as though the jump's view's; and one whose news is held until the jump has arrived in the new pane and drawn there is told once that draw has reset
/// the cutoff. Either way its `page_drawn` names the pages where the tab was in the view it left. What a watcher hears of the plain tab from the jump on
/// is only the pages it went to: the draws of the view it left are dropped, and the cutoff of a tab side by side at its release isn't reset.
#[test]
fn a_view_a_tab_left_tells_nothing_it_drew_there_after_the_persons_jump() {
    let (app, plain_tab, third_tab, left_on) = compared_second(5, "Locked");
    let inner = Arc::clone(app.core());
    let second = inner.panes.engine(&inner.engine, 1).expect("the second pane's page view");
    // The second pane is shown where following left the plain tab, and drawn there.
    let at = second.places().1.into_iter().find(|(id, _)| *id == plain_tab).expect("its place").1;
    let view = recto_reader::Viewport {
        x: 0.0,
        y: at.y,
        width: 292.0,
        height: 225.0,
        frame_width: 292.0,
        magnification: at.magnification,
        backing: 1.0,
        live: false,
        scrolled_to: u64::MAX - 1,
    };
    crate::testkit::headless::applied_as_the_host(&inner.panes.viewport(&inner.engine, 1, false, view).0);
    app.settle();
    let (between, pane) = (Arc::clone(&inner), second.clone());
    let pinched = recto_reader::Viewport {
        magnification: at.magnification * 1.5,
        ..view
    };
    let watching = app.agent_watches(Level::Control);
    release_and_shift(&app, plain_tab, third_tab, 20, true, move || {
        // The plain tab shown in the first pane with the jump, and still in the second: zoomed there, it's drawn again where it was, and its news is
        // queued for the session thread, which takes it after this look has said the jump sent.
        crate::testkit::headless::applied_as_the_host(&between.panes.viewport(&between.engine, 1, false, pinched).0);
        assert!(
            pane.qa().wait_resting(crate::testkit::headless::WAIT_LIMIT),
            "the second pane drew where it was"
        );
        // Zoomed again, a tile is drawn there (the pages' previews are there already), and its news is held until the first pane has arrived and drawn,
        // which, of the view the jump went to, ends the cutoff unless the tab was side by side.
        pane.qa().hold_tells(true);
        let again = recto_reader::Viewport {
            magnification: pinched.magnification * 1.5,
            ..pinched
        };
        crate::testkit::headless::applied_as_the_host(&between.panes.viewport(&between.engine, 1, false, again).0);
        assert!(
            pane.qa().wait_for_held_tell(crate::testkit::headless::WAIT_LIMIT),
            "a tile of the plain tab drawn in the second pane, its news held"
        );
    });
    // The first pane arrives where the jump sent it and draws there.
    let engine = inner.engine.clone();
    let place = engine.places().1.into_iter().find(|(id, _)| *id == plain_tab).expect("its place").1;
    let mut first = recto_reader::Viewport {
        y: place.y,
        magnification: place.magnification,
        scrolled_to: 0,
        ..view
    };
    crate::testkit::headless::host_turns(&app, &mut first, false);
    let heard = std::cell::RefCell::new((watching, Vec::new()));
    app.wait_until("the first pane drew where the jump went, and it was heard", || {
        assert!(app.core().core.flush(crate::testkit::headless::WAIT_LIMIT));
        let (watching, pages) = &mut *heard.borrow_mut();
        pages.extend(pages_drawn(watching, plain_tab));
        pages.iter().any(|page| *page >= 19)
    });
    // The second pane's held news goes now.
    second.qa().hold_tells(false);
    app.settle();
    let (mut watching, mut all) = heard.into_inner();
    all.extend(pages_drawn(&mut watching, plain_tab));
    assert!(
        all.iter().all(|page| *page >= 19),
        "a draw of where following left the plain tab (page {left_on}) heard: {all:?}"
    );
}

/// The pages of `tab` a watcher heard drawn since it last looked.
fn pages_drawn(watching: &mut crate::testkit::watching::Watching, tab: u32) -> Vec<u64> {
    watching
        .heard()
        .into_iter()
        .filter(|event| event["event"] == "page_drawn" && event["tab"] == tab)
        .filter_map(|event| event["page"].as_u64())
        .collect()
}
