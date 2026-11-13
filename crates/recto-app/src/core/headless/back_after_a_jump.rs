//! **Back straight after a jump** (red `main`, CI run 43401013894): a jump goes to the page view later than to the session, by the session thread's look
//! at the session after the command, or by an effect it carries out, and the page view tells of where that took it later still. A step in the trail made in
//! between is the newer, and what the page view says of the older jump must not undo it. In the kit nothing scrolls the page view, so a jump it's sent is
//! never arrived at, which is the window with the person's next key pressed before the scroll lands.

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;

/// An app with a 30-page file open, `pdf page 1` on page 1 and pages 10 to 19.
fn thirty_pages() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("thirty.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    app
}

/// The page the session has the tab being read on, counted from 1, what `recto where` says. Read off the session, not by `where`: a command ends with the
/// session thread's look at the session, which takes in what the page view has told, and an assertion between two steps mustn't do a step of its own.
fn page(app: &HeadlessApp) -> u64 {
    let page = app
        .core()
        .host
        .session
        .with_session(|s| s.active().and_then(|id| s.tab(id)).map(|tab| tab.page));
    u64::from(page.expect("a tab being read")) + 1
}

fn agent(app: &HeadlessApp, line: &str) -> serde_json::Value {
    app.agent(Level::Control, line).unwrap_or_else(|error| panic!("{line}: {error:?}"))
}

/// Failure (the red `main`): an agent's `back` straight after its `find --next --jump` to a hit on page 10 leaves the tab on page 10. The jump's hit is
/// brought into view by an effect the session thread carries out after the command, and here after `back` too, in either order with the look at the
/// session the two commands end with, as a look already queued takes theirs: with the effect first, its page view's word that it went to page 10 was taken
/// at the look as the reader's own scroll, over Back's page 1; with the look first, Back's page 1 was sent and the effect then took the page view back to
/// the hit, which it told of as its own. Held so `back` comes before the effect every time.
#[test]
fn an_agents_back_before_the_window_has_shown_its_jumps_hit_stays_back() {
    for look_first in [false, true] {
        let app = thirty_pages();
        agent(&app, r#"{"cmd":"find","query":"pdf page 1"}"#);
        agent(&app, r#"{"cmd":"find","next":true}"#);
        app.settle();
        assert_eq!(page(&app), 1, "the first hit is on the page read");
        let held = app.hold_session_thread();
        if look_first {
            // Queued ahead of the effect: the commands' own looks are this one.
            app.send(crate::queue::CoreCall::Reconcile);
        }
        let went = agent(&app, r#"{"cmd":"find","next":true,"jump":true}"#);
        assert_eq!(went["went_to"]["page"].as_u64(), Some(10), "{went}");
        agent(&app, r#"{"cmd":"back"}"#);
        drop(held);
        app.settle();
        assert_eq!(
            page(&app),
            1,
            "Back from the hit on page 10, taken before the window showed the hit (the look first: {look_first}), is on page {} where it went to 1",
            page(&app)
        );
    }
}

/// Waits, bounded, for the page view to have been sent to a page (counted from 1) by the session thread's look after a command, and no further: what it
/// tells of that jump stays untaken until the session thread looks again, which in the window is the scroll landing a frame later.
fn sent_to(app: &HeadlessApp, page: u32) {
    let inner = app.core();
    let engine = inner.panes.reading_engine(&inner.engine);
    app.wait_until(&format!("the page view sent to page {page}"), || {
        engine.reading_anchor().map(|(at, _)| at + 1) == Some(page)
    });
}

/// Failure: an agent's `back` after `goto` to a place a way down page 10, sent before the page view's scroll there has landed, leaves the tab on page 10:
/// the page view's word that it went to page 10 for the `goto`, not yet taken, was taken after Back as the reader's own scroll. And the same for a plain
/// `goto`, whose jump the page view tells nothing of.
#[test]
fn an_agents_back_after_goto_stays_back() {
    let app = thirty_pages();
    for line in [r#"{"cmd":"goto","page":10,"within":0.4}"#, r#"{"cmd":"goto","page":10}"#] {
        agent(&app, line);
        sent_to(&app, 10);
        agent(&app, r#"{"cmd":"back"}"#);
        app.settle();
        assert_eq!(page(&app), 1, "Back after {line}: on page {} where it went to 1", page(&app));
    }
}

/// Failure: `back`, `forward` and `back` again, each sent before the page view's scroll for the one before has landed, end on another page than the trail
/// says: what the page view told of one step's landing a way down page 10 was taken after the next.
#[test]
fn back_then_forward_then_back_ends_back() {
    let app = thirty_pages();
    agent(&app, r#"{"cmd":"goto","page":10,"within":0.4}"#);
    sent_to(&app, 10);
    for (line, to) in [(r#"{"cmd":"back"}"#, 1), (r#"{"cmd":"forward"}"#, 10), (r#"{"cmd":"back"}"#, 1)] {
        agent(&app, line);
        sent_to(&app, to);
        assert_eq!(page(&app), u64::from(to), "{line}: on page {}", page(&app));
    }
    app.settle();
    assert_eq!(page(&app), 1, "settled: on page {}", page(&app));
}

/// Failure: the person's ⌘[ straight after a click on a link to a place a way down page 9 (as the page view hands it on), pressed before the page view's
/// scroll there has landed, leaves the tab on page 9; and the same after Return in Find then ⌘G: ⌘[ goes to the page Return left, and stays there. The
/// person's steps are queued for the session thread one after another, as two keys pressed within a frame are.
#[test]
fn the_persons_back_after_a_jump_stays_back() {
    let app = thirty_pages();
    let tab = app.core().host.session.with_session(|session| session.active()).expect("a tab");
    app.core().link_to_page(tab, 8, Some(0.4));
    app.act(UiAction::Menu { id: "go.back".into() });
    app.settle();
    assert_eq!(page(&app), 1, "⌘[ after the link to page 9: on page {}", page(&app));

    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.settle();
    // The first Return goes to the first hit, on the page read; the second to the next, on page 10.
    for _ in 0..2 {
        app.act(UiAction::FindReturn { forward: true });
        app.settle();
    }
    assert_eq!(page(&app), 10, "Return twice goes to the hit on page 10");
    app.act(UiAction::Menu { id: "find.next".into() });
    app.act(UiAction::Menu { id: "go.back".into() });
    app.settle();
    assert_eq!(page(&app), 1, "⌘[ after Return and ⌘G: back to where Return left, on page {}", page(&app));
}
