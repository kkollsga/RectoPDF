//! **The newer read of the page views wins, by when it was made, not by who read** (`core/places.rs`): a jump the session thread's look sends is where
//! the reader is until a read made after the send is taken in, by the look, a command or the saver; and a read made before another already taken in is
//! skipped. The orderings here are each one a real window meets: a scroll within a page queues no look, the views are read with no lock on the session,
//! and the session thread, an agent's command and the saver take them in from threads of their own.

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use recto_control::host::Host;

/// An app with a 30-page file open.
fn thirty_pages() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("thirty.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    app
}

fn agent(app: &HeadlessApp, line: &str) -> serde_json::Value {
    app.agent(Level::Control, line).unwrap_or_else(|error| panic!("{line}: {error:?}"))
}

/// How far down page 5 the last stop of a tab's trail is, back or forward, with the trail said for a failure.
fn last_stop_on_page_5(app: &HeadlessApp, back: bool) -> (f32, String) {
    app.core().host.session.with_session(|s| {
        let trail = s.tabs().active().and_then(|id| s.tabs().trail_of(id)).cloned().unwrap_or_default();
        let stops = if back { trail.back() } else { trail.forward() };
        let last = stops.last().expect("a stop");
        assert_eq!(last.page, 4, "on page 5: {trail:?}");
        (last.within, format!("{trail:?}"))
    })
}

/// Where the session has the reader in the tab being read, how far down page 5, which is what a step would mark.
fn here_on_page_5(app: &HeadlessApp) -> f64 {
    let here = app
        .core()
        .host
        .session
        .with_session(|s| s.tabs().active().and_then(|id| s.here(id)))
        .expect("a place");
    assert_eq!(here.page, 4, "on page 5: {here:?}");
    f64::from(here.within)
}

/// A page's height in the page view, about: the document's over its 30 pages.
fn page_height(app: &HeadlessApp) -> f64 {
    let inner = app.core();
    let (_, places) = inner.panes.reading_engine(&inner.engine).places();
    places.first().map_or(0.0, |(_, place)| place.document_height) / 30.0
}

/// Where the page view says the reader is, `(page, within)`.
fn views_place(app: &HeadlessApp) -> (u32, f64) {
    let inner = app.core();
    inner.panes.reading_engine(&inner.engine).reading_anchor().expect("a place")
}

/// On page 5 0.3 of the way down, arrived and taken in; then `goto 5 --within 0.9` sent the page view by exactly one look, the command's own, and the
/// view arrived there, which no look takes in, as no look comes for a scroll that stays on its page. Where a link on the page showing, Back to a place
/// on it, or Find's jump leaves the window.
fn a_jump_sent_and_arrived_at(app: &HeadlessApp) {
    agent(app, r#"{"cmd":"goto","page":5,"within":0.3}"#);
    app.settle();
    app.person_scrolls(0.0);
    app.settle();
    let held = app.hold_session_thread();
    agent(app, r#"{"cmd":"goto","page":5,"within":0.9}"#);
    let looks = app.looks_done();
    drop(held);
    app.wait_until("the one look that sends the jump", || app.looks_done() > looks);
    let (page, within) = views_place(app);
    assert!(page == 4 && (within - 0.9).abs() < 0.005, "sent to 0.9 of page 5: {page} {within}");
    // The host arrives where it was sent.
    app.person_scrolls(0.0);
    assert_eq!(app.looks_done(), looks + 1, "nothing queued a look since the one that sent it");
}

fn seen_looks(inner: &crate::core::Inner) -> u64 {
    inner.serving.place_seams.looks_done()
}

/// Failure (review of the sent-jump branch, S1): a jump the look sent is kept as where the reader is past the person's own scroll on the same page, as no
/// look comes for that scroll and only a look forgot a sent jump: from 0.3 of page 5, a jump to 0.9 there, arrived at, the person scrolling up to about 0.4,
/// then ⌘[ (or an agent's `back`), kept 0.9 for Forward where the reader was at 0.4. No settle between the scroll and the step, as the window has none: the
/// test that settled there hid it.
#[test]
fn back_after_the_persons_scroll_past_a_sent_jump_marks_where_they_scrolled() {
    for by_agent in [false, true] {
        let app = thirty_pages();
        a_jump_sent_and_arrived_at(&app);
        let looks = app.looks_done();
        app.person_scrolls(-0.5 * page_height(&app));
        let (page, within) = views_place(&app);
        assert!(page == 4 && (within - 0.4).abs() < 0.05, "scrolled up page 5 to about 0.4: {page} {within}");
        if by_agent {
            let inner = std::sync::Arc::clone(app.core());
            app.when_the_next_take_places_has_read(move || {
                assert_eq!(seen_looks(&inner), looks, "no look came between the scroll and `back`'s read");
            });
            agent(&app, r#"{"cmd":"back"}"#);
        } else {
            assert_eq!(app.looks_done(), looks, "no look came between the scroll and ⌘[");
            app.act(UiAction::Menu { id: "go.back".into() });
        }
        app.settle();
        let (marked, trail) = last_stop_on_page_5(&app, false);
        assert!(
            (f64::from(marked) - within).abs() < 0.01,
            "Back (by an agent: {by_agent}) marked {marked} for Forward, where the person scrolled to {within} after the jump to 0.9: {trail}"
        );
    }
}

/// Failure (S1, Find): an agent's `find --next --jump` to a hit on page 5, after a jump there was sent and the person scrolled up the page with no look
/// since, marks Back at the jump's 0.9 where the person was at about 0.4: Find's command takes the views in, and only a look forgot a sent jump.
#[test]
fn finds_jump_after_the_persons_scroll_past_a_sent_jump_marks_where_they_scrolled() {
    let app = thirty_pages();
    agent(&app, r#"{"cmd":"find","query":"pdf page 5"}"#);
    app.settle();
    a_jump_sent_and_arrived_at(&app);
    let looks = app.looks_done();
    app.person_scrolls(-0.5 * page_height(&app));
    let (_, within) = views_place(&app);
    assert_eq!(app.looks_done(), looks, "no look since the scroll");
    let went = agent(&app, r#"{"cmd":"find","next":true,"jump":true}"#);
    assert_eq!(went["went_to"]["page"].as_u64(), Some(5), "the hit on page 5: {went}");
    app.settle();
    let (marked, trail) = last_stop_on_page_5(&app, true);
    assert!(
        (f64::from(marked) - within).abs() < 0.01,
        "Find's jump marked {marked} for Back, where the person scrolled to {within} after the jump to 0.9: {trail}"
    );
}

/// Failure (S1, synced scroll): in a comparison with synced scroll, the person's scroll in the other pane takes the pane read along on its page after a
/// jump was sent it, with no look; an agent's `back` then marks the jump's 0.9 for Forward where the pane had followed to: the command takes the views in,
/// and only a look forgot a sent jump.
///
/// Failure (one run of a full check, 2026-11-13, at a load of 16 to 22): the follow is sent to a's page view "now or soon", and soon when a's own thread
/// holds its lock at that instant, as it does for moments throughout a drawing job; it's taken by that thread only at the job's end. A read of a's places
/// in between, as `back` takes them in to mark where the reader is, said 0.9, where the person's scroll in b had already sent a on: the mark lost the
/// person's newer move for an older read. Forced with a's page view held busy (`Engine::qa().hold_busy`), so the follow is left pending until after `back`.
#[test]
fn back_after_synced_scroll_past_a_sent_jump_marks_where_the_pane_followed() {
    for busy in [false, true] {
        let disk = Disk::new();
        let (a, b) = (disk.pdf("a.pdf", 30), disk.pdf("b.pdf", 30));
        let app = disk.start();
        app.open(&b);
        app.open(&a);
        let (b_tab, a_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
        // Both on page 5 as synced scroll begins, so it keeps them on the same page.
        agent(&app, &format!(r##"{{"cmd":"goto","tab":"#{b_tab}","page":5,"within":0.5}}"##));
        agent(&app, &format!(r##"{{"cmd":"goto","tab":"#{a_tab}","page":5,"within":0.3}}"##));
        agent(&app, &format!(r##"{{"cmd":"compare","toggle":"#{b_tab}"}}"##));
        agent(&app, r#"{"cmd":"compare","sync":"on"}"#);
        agent(&app, &format!(r##"{{"cmd":"compare","reading":"#{a_tab}"}}"##));
        app.settle();
        let inner = std::sync::Arc::clone(app.core());
        let pane_of = |tab: u32| {
            (0..inner.panes.count())
                .find(|pane| inner.panes.tab_of(*pane).map(|id| id.0) == Some(tab))
                .expect("in a pane")
        };
        let (a_pane, b_pane) = (pane_of(a_tab), pane_of(b_tab));
        assert_eq!(inner.panes.reading(), a_pane, "a is read");
        a_jump_sent_and_arrived_at(&app);
        // The person's own scroll in b's pane, a little way down its page 5, as the host tells of it; a's pane follows on its page 5, to as far down it.
        let b_engine = inner.panes.engine(&inner.engine, b_pane).expect("b's page view");
        let a_engine = inner.panes.engine(&inner.engine, a_pane).expect("a's page view");
        let b_place = b_engine.places().1.into_iter().find(|(tab, _)| *tab == b_tab).expect("b's place").1;
        let looks = app.looks_done();
        let scroll = |y: f64, seq: u64| recto_reader::Viewport {
            y,
            magnification: b_place.magnification,
            backing: 2.0,
            scrolled_to: seq,
            ..recto_reader::Viewport::default()
        };
        drop(inner.panes.viewport(&inner.engine, b_pane, false, scroll(b_place.y, u64::MAX - 1)));
        a_engine.qa().hold_busy(busy);
        drop(
            inner
                .panes
                .viewport(&inner.engine, b_pane, true, scroll(b_place.y + 0.1 * page_height(&app), u64::MAX - 1)),
        );
        let (page, followed) = b_engine.reading_anchor().expect("b's place");
        assert!(
            page == 4 && (followed - 0.9).abs() > 0.05,
            "b's scroll stays on its page 5, away from 0.9: {page} {followed}"
        );
        // Only the busy round is held to it: a free round that meets a's own thread at work leaves the follow pending too, which is the same ordering and no failure.
        assert!(
            !busy || a_engine.qa().follow_is_pending(),
            "the follow is left pending for a's own thread in the busy round"
        );
        // The window would make b's pane the one read, by a call of its own still to come: the agent's `back` here is on a, ahead of it.
        inner.panes.focus(a_pane);
        if !busy {
            let (page, at) = views_place(&app);
            assert!(
                page == 4 && (at - followed).abs() < 0.01,
                "a's pane followed b's on page 5 to {followed}: {page} {at}"
            );
        }
        assert_eq!(app.looks_done(), looks, "no look since the scroll");
        agent(&app, r#"{"cmd":"back"}"#);
        a_engine.qa().hold_busy(false);
        app.settle();
        let (marked, trail) = last_stop_on_page_5(&app, false);
        assert!(
            (f64::from(marked) - followed).abs() < 0.01,
            "Back marked {marked} for Forward, where a's pane was following b's to {followed} after the jump to 0.9 (the follow left pending for a's \
             page view's own thread: {busy}): {trail}"
        );
    }
}

/// Failure (review round 1 of the page view follow-ups): in a comparison with synced scroll, the look sends Back's jump to the pane read, and before that
/// jump gets to the pane's page view the person's scroll in the other pane sends this pane a follow, asked after the jump and applied first. Who got the
/// page view's lock decided: the older jump landed after the follow and undid the person's newer move. Now the follow's place stands in the view, and the
/// session, which sent the jump, ends where the view is: the page the pane followed to, said once, nothing sending Back's jump again, and the trail as Back
/// left it, its Forward stop where Back marked the reader and a Forward going there. Forced: the jump is held in the page view between being asked and
/// taking the lock (`Engine::qa().hold_asks`) while the person scrolls. Twice: the follow takes the pane to page 7, and the follow keeps it on page 20, where
/// it was before Back, which only the view's telling of its place after the jump it didn't take can say to the session.
#[test]
fn back_overtaken_by_synced_scroll_ends_where_the_pane_followed_and_forward_goes_where_back_left() {
    back_overtaken_by_synced_scroll(2.4, 6);
    back_overtaken_by_synced_scroll(15.3, 19);
}

/// [`back_overtaken_by_synced_scroll_ends_where_the_pane_followed_and_forward_goes_where_back_left`], with b scrolled `pages_down` from 0.5 of its page 5,
/// which takes a to `followed_to` (from 0).
fn back_overtaken_by_synced_scroll(pages_down: f64, followed_to: u32) {
    use recto_reader::engine::Ask;
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 30), disk.pdf("b.pdf", 30));
    let app = disk.start();
    app.open(&b);
    app.open(&a);
    let (b_tab, a_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
    agent(&app, &format!(r##"{{"cmd":"goto","tab":"#{b_tab}","page":5,"within":0.5}}"##));
    agent(&app, &format!(r##"{{"cmd":"goto","tab":"#{a_tab}","page":5,"within":0.3}}"##));
    agent(&app, &format!(r##"{{"cmd":"compare","toggle":"#{b_tab}"}}"##));
    agent(&app, r#"{"cmd":"compare","sync":"on"}"#);
    agent(&app, &format!(r##"{{"cmd":"compare","reading":"#{a_tab}"}}"##));
    app.settle();
    let inner = std::sync::Arc::clone(app.core());
    let pane_of = |tab: u32| {
        (0..inner.panes.count())
            .find(|pane| inner.panes.tab_of(*pane).map(|id| id.0) == Some(tab))
            .expect("in a pane")
    };
    let (a_pane, b_pane) = (pane_of(a_tab), pane_of(b_tab));
    // a's trail: a jump to page 20 from 0.3 of page 5, arrived at.
    agent(&app, r#"{"cmd":"goto","page":20}"#);
    app.settle();
    app.person_scrolls(0.0);
    app.settle();
    assert_eq!(views_place(&app), (19, 0.0), "a went to page 20");
    let b_engine = inner.panes.engine(&inner.engine, b_pane).expect("b's page view");
    let a_engine = inner.panes.engine(&inner.engine, a_pane).expect("a's page view");
    let b_place = b_engine.places().1.into_iter().find(|(tab, _)| *tab == b_tab).expect("b's place").1;
    let scroll = |y: f64| recto_reader::Viewport {
        y,
        magnification: b_place.magnification,
        backing: 2.0,
        scrolled_to: u64::MAX - 1,
        ..recto_reader::Viewport::default()
    };
    drop(inner.panes.viewport(&inner.engine, b_pane, false, scroll(b_place.y)));
    // b's place and its page's height as its view now has it.
    let b_place = b_engine.places().1.into_iter().find(|(tab, _)| *tab == b_tab).expect("b's place").1;
    let b_page_height = b_place.document_height / 30.0;
    let a_id = recto_session::TabId(a_tab);
    app.hear_events();
    a_engine.qa().hold_asks(Ask::Jump, true);
    std::thread::scope(|scope| {
        let back = std::thread::Builder::new()
            .name("test-back".into())
            .spawn_scoped(scope, || agent(&app, r#"{"cmd":"back"}"#))
            .expect("a thread for the agent's back");
        assert!(a_engine.qa().wait_for_held_asks(1), "the look sent Back's jump to a's page view");
        // The person's own scroll in b, from 0.5 of its page 5 down: a follows, to the same page from its own page 5.
        drop(
            inner
                .panes
                .viewport(&inner.engine, b_pane, true, scroll(b_place.y + pages_down * b_page_height)),
        );
        inner.panes.focus(a_pane);
        a_engine.qa().hold_asks(Ask::Jump, false);
        back.join().unwrap();
    });
    app.settle();
    let (page, followed) = views_place(&app);
    assert_eq!(
        page, followed_to,
        "a's pane followed b's, and Back's older jump didn't undo it: {page} {followed}"
    );
    let (session_page, here, waiting) = inner
        .host
        .session
        .with_session(|s| (s.tabs().tab(a_id).map(|tab| tab.page), s.here(a_id), s.mail().waiting_jump(a_id)));
    assert_eq!(session_page, Some(followed_to), "the session has the page the view is on");
    assert!(
        here.is_some_and(|here| here.page == followed_to && (f64::from(here.within) - followed).abs() < 0.01),
        "the session has the reader where the view is: {here:?}, the view at {followed}"
    );
    assert_eq!(waiting, None, "no jump waits to be sent again");
    let moves: Vec<_> = app
        .heard_events()
        .into_iter()
        .filter(|event| matches!(event, recto_agent::Event::PageChanged { tab, .. } if *tab == a_id))
        .collect();
    assert!(
        matches!(moves.as_slice(), [recto_agent::Event::PageChanged { page: Some(page), .. }] if *page == followed_to + 1),
        "the view's page said once, as the reader's move: {moves:?}"
    );
    // Nothing sends Back's jump again: the view arrives where it was last sent, which is where it followed to.
    app.person_scrolls(0.0);
    app.settle();
    let (again, at) = views_place(&app);
    assert!(again == followed_to && (at - followed).abs() < 0.01, "no jump sent after: {again} {at}");
    assert!(app.heard_events().is_empty(), "nothing more said");
    // The trail is as Back left it: Forward is where Back marked the reader, page 20's top, and goes there.
    let forward = inner
        .host
        .session
        .with_session(|s| s.tabs().trail_of(a_id).and_then(|trail| trail.forward().last().copied()));
    assert!(
        forward.is_some_and(|stop| stop.page == 19 && stop.within == 0.0),
        "Forward is page 20's top, where Back marked the reader: {forward:?}"
    );
    agent(&app, r#"{"cmd":"forward"}"#);
    app.settle();
    app.person_scrolls(0.0);
    app.settle();
    assert_eq!(views_place(&app), (19, 0.0), "Forward goes to page 20");
    assert_eq!(inner.host.session.with_session(|s| s.tabs().tab(a_id).map(|tab| tab.page)), Some(19));
}

/// Failure: an agent's `back`, `forward` or `goto` takes in where the page views are and then marks where the reader is, under two locks on the session,
/// and the session thread's look can come between: it sends the jump waiting on the page showing to the page view, and the mark, finding none waiting, takes
/// the view's place from before that jump. So `goto 5 --within 0.8` from 0.3 of the way down page 5 then `back` kept 0.3 for Forward, `back` to 0.3 then
/// `forward` kept 0.8 for Back, and `goto 5 --within 0.8` then `goto 20` kept nothing new for Back. Forced each time: exactly one look comes between the
/// take-in and the step, with a second queued and held off.
#[test]
fn a_look_between_taking_places_and_the_step_loses_no_jump() {
    // What's settled first, the jump left waiting on the page showing, the step, which of the trail's ends its mark is on and how far down page 5.
    let cases = [
        (
            vec![r#"{"cmd":"goto","page":5,"within":0.3}"#],
            r#"{"cmd":"goto","page":5,"within":0.8}"#,
            r#"{"cmd":"back"}"#,
            false,
            0.8,
        ),
        (
            vec![r#"{"cmd":"goto","page":5,"within":0.3}"#, r#"{"cmd":"goto","page":5,"within":0.8}"#],
            r#"{"cmd":"back"}"#,
            r#"{"cmd":"forward"}"#,
            true,
            0.3,
        ),
        (
            vec![r#"{"cmd":"goto","page":5,"within":0.3}"#],
            r#"{"cmd":"goto","page":5,"within":0.8}"#,
            r#"{"cmd":"goto","page":20}"#,
            true,
            0.8,
        ),
    ];
    let mut lost = Vec::new();
    for (before, waiting, step, back, within) in cases {
        let app = thirty_pages();
        for line in before {
            agent(&app, line);
            app.settle();
        }
        let held = app.hold_session_thread_aside();
        agent(&app, waiting);
        // A stray call queued behind the command's own look, whose look would tell of the jump and hide the ordering: held off by the seam.
        app.send(crate::queue::CoreCall::Reconcile);
        let between = app.look_after_the_next_take_places(held);
        agent(&app, step);
        drop(between);
        app.settle();
        let (marked, trail) = last_stop_on_page_5(&app, back);
        if (marked - within).abs() >= 0.01 {
            lost.push(format!(
                "{step} after {waiting} marked {marked} where the jump sent the reader to {within}: {trail}"
            ));
        }
    }
    assert!(lost.is_empty(), "with the look between the take-in and the step: {lost:#?}");
}

/// Failure: a command's read of where the page views are, taken with no lock on the session, is taken in after the session thread's look has taken in
/// their word since, so the person's own scroll in between is undone by the place from before it: Back marked 0.3 of the way down page 5 for Forward, where
/// the person had scrolled up to 0.2 while it read. Forced: the scroll and the look come between the command's read and its take-in.
#[test]
fn a_commands_read_older_than_a_looks_doesnt_undo_the_persons_scroll() {
    let app = thirty_pages();
    agent(&app, r#"{"cmd":"goto","page":5,"within":0.3}"#);
    app.settle();
    // The view arrives where it was sent, and the session hears it there.
    app.person_scrolls(0.0);
    app.settle();
    let scroll = app.person_scroll_ahead(-0.1 * page_height(&app));
    let inner = std::sync::Arc::clone(app.core());
    app.when_the_next_take_places_has_read(move || {
        drop(inner.panes.viewport(&inner.engine, inner.panes.reading(), true, scroll));
        assert!(inner.core.flush(crate::testkit::headless::WAIT_LIMIT), "the look at the scroll");
    });
    agent(&app, r#"{"cmd":"back"}"#);
    app.settle();
    assert_eq!(views_place(&app).0, 0, "Back went to page 1");
    let (marked, trail) = last_stop_on_page_5(&app, false);
    assert!(
        (f64::from(marked) - 0.2).abs() < 0.02,
        "Back marked {marked} for Forward, where the person had scrolled up to about 0.2 while it read the views at 0.3: {trail}"
    );
}

/// Failure (review F1): the session thread's look reads the views before it takes the session's lock, and a command that reads and takes in between is
/// newer; the look's older read taken in after it undoes the person's scroll the command saw. With the looks counted instead of the reads stamped, the
/// command's newer read was the one dropped, and Back marked 0.3 for Forward where the person had scrolled up to 0.2. Forced: the scroll and the command
/// come between the look's read and its take-in, the command as Back (its mark) and as the take-in alone (what the session then has).
#[test]
fn a_looks_read_older_than_a_commands_doesnt_undo_the_persons_scroll() {
    for stepping in [true, false] {
        let app = thirty_pages();
        agent(&app, r#"{"cmd":"goto","page":5,"within":0.3}"#);
        app.settle();
        app.person_scrolls(0.0);
        app.settle();
        let scroll = app.person_scroll_ahead(-0.1 * page_height(&app));
        let inner = std::sync::Arc::clone(app.core());
        app.when_the_next_look_has_read(move || {
            drop(inner.panes.viewport(&inner.engine, inner.panes.reading(), true, scroll));
            if stepping {
                let back = recto_agent::parse(r#"{"cmd":"back"}"#).expect("back");
                inner
                    .host
                    .run(recto_agent::Actor::Agent(Level::Control), &back, &mut Vec::new())
                    .expect("back runs");
            } else {
                inner.take_places();
            }
        });
        let looks = app.looks_done();
        app.send(crate::queue::CoreCall::Reconcile);
        app.wait_until("the look", || app.looks_done() > looks);
        if stepping {
            app.settle();
            let (marked, trail) = last_stop_on_page_5(&app, false);
            assert!(
                (f64::from(marked) - 0.2).abs() < 0.02,
                "Back marked {marked} for Forward, where the person had scrolled up to about 0.2 before it read: {trail}"
            );
        } else {
            let here = here_on_page_5(&app);
            assert!(
                (here - 0.2).abs() < 0.02,
                "the look's read at 0.3 undid the command's newer one at about 0.2: {here}"
            );
        }
    }
}

/// Failure (review F1, two threads): the saver's read of the views, older than a command's read taken in since, is taken in after it and undoes the person's
/// scroll the command saw: the session has the reader at 0.3 of page 5 where they'd scrolled up to 0.2. The saver takes the views in by the same call as
/// a command (`Inner::take_places`), made here on the test's thread; the scroll and the command's read and take-in come between its read and its take-in.
#[test]
fn the_savers_read_older_than_a_commands_doesnt_undo_the_persons_scroll() {
    let app = thirty_pages();
    agent(&app, r#"{"cmd":"goto","page":5,"within":0.3}"#);
    app.settle();
    app.person_scrolls(0.0);
    app.settle();
    let scroll = app.person_scroll_ahead(-0.1 * page_height(&app));
    let inner = std::sync::Arc::clone(app.core());
    app.when_the_next_take_places_has_read(move || {
        drop(inner.panes.viewport(&inner.engine, inner.panes.reading(), true, scroll));
        // A command's own read and take-in, on another thread than the saver's in the app.
        inner.take_places();
    });
    let looks = app.looks_done();
    app.core().take_places();
    assert_eq!(app.looks_done(), looks, "no look in all this");
    let here = here_on_page_5(&app);
    assert!(
        (here - 0.2).abs() < 0.02,
        "the saver's read at 0.3 undid the command's newer one at about 0.2: {here}"
    );
}

/// Failure (review F2): a jump the look meant for a tab whose file is still being looked at isn't sent (nothing goes on that pane's page until the file's
/// known) and was recorded as sent all the same, so it was neither waiting nor on its way: lost, and the tab came up at its page's top or its old place.
/// It waits for the next look, which sends it once the file's been looked at.
#[test]
fn a_jump_for_a_tab_held_for_its_file_waits_for_the_next_look() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 30), disk.pdf("b.pdf", 30));
    let app = disk.start();
    app.open(&b);
    app.open(&a);
    let b_tab = recto_session::TabId(app.tabs()[0].tab);
    let inner = std::sync::Arc::clone(app.core());
    // Held as when its file's being looked at, before it's shown in a pane.
    *crate::lock(&inner.life.holding_tab) = Some(b_tab);
    agent(&app, &format!(r##"{{"cmd":"goto","tab":"#{}","page":5,"within":0.8}}"##, b_tab.0));
    agent(&app, &format!(r##"{{"cmd":"compare","toggle":"#{}"}}"##, b_tab.0));
    app.settle();
    let (waiting, here) = inner.host.session.with_session(|s| (s.mail().waiting_jump(b_tab), s.here(b_tab)));
    let stop = waiting.expect("the jump still waiting while the tab's held");
    assert_eq!((stop.page, here.map(|here| here.page)), (4, Some(4)), "on page 5");
    assert!((stop.within - 0.8).abs() < 0.01, "0.8 of the way down: {stop:?}");
    *crate::lock(&inner.life.holding_tab) = None;
    app.send(crate::queue::CoreCall::Reconcile);
    app.settle();
    let pane = (0..inner.panes.count())
        .find(|pane| inner.panes.tab_of(*pane) == Some(b_tab))
        .expect("b in a pane");
    let (page, within) = inner
        .panes
        .engine(&inner.engine, pane)
        .expect("its page view")
        .reading_anchor()
        .expect("a place");
    assert!(page == 4 && (within - 0.8).abs() < 0.01, "sent once the hold's over: {page} {within}");
    assert_eq!(inner.host.session.with_session(|s| s.mail().waiting_jump(b_tab)), None, "and no longer waiting");
}

/// Failure (review F3): the seam that puts one look between a command's take-in and its step is set and never used, as when the command run next takes no
/// places in, and the test it's in passes having forced nothing. It says so, by name, when it's let go of.
#[test]
#[should_panic(expected = "look_after_the_next_take_places")]
fn the_one_look_between_says_so_when_no_command_took_the_places_in() {
    let app = thirty_pages();
    let held = app.hold_session_thread_aside();
    let between = app.look_after_the_next_take_places(held);
    agent(&app, r#"{"cmd":"where"}"#);
    drop(between);
}
