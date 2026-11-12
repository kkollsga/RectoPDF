use super::*;
use crate::testkit::{FakeListener, Heard, QuietEngine};
use recto_reader::EngineConfig;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// A session with no tabs, a page view and the panes, and the field: what a [`FindHandle`] borrows.
struct Rig {
    session: Arc<SessionHost<DrawableLoader>>,
    field: Arc<Mutex<Field>>,
    frozen: AtomicBool,
    panes: Panes,
    engine: Engine,
    views_set: AtomicUsize,
}

impl Rig {
    fn new() -> Self {
        Self {
            session: Arc::new(SessionHost::default()),
            field: Arc::new(Mutex::new(Field::default())),
            frozen: AtomicBool::new(false),
            panes: Panes::new(Arc::new(QuietEngine), EngineConfig::default()),
            engine: Engine::new(EngineConfig::default(), Arc::new(QuietEngine)),
            views_set: AtomicUsize::new(0),
        }
    }

    fn with<R>(&self, listener: &FakeListener, step: impl FnOnce(&FindHandle<'_, FakeListener>) -> R) -> R {
        let set_view = |events: &mut Vec<Event>, change: fn(&mut ViewSettings)| {
            self.views_set.fetch_add(1, Ordering::SeqCst);
            let view = self.session.with_session(|session| {
                let mut view = session.view();
                change(&mut view);
                session.set_view(view);
                view
            });
            events.push(recto_agent::view_changed(view));
        };
        step(&FindHandle {
            session: &self.session,
            field: &self.field,
            frozen: &self.frozen,
            panes: &self.panes,
            engine: &self.engine,
            listener,
            set_view: &set_view,
        })
    }
}

/// Failure: Find's list grows with its hits and pushes the tabs and the contents off the sidebar (owner, 2026-11-07: "max height within scroll so it doesn't push everything of screen"), or takes room while it's closed.
#[test]
fn the_list_never_takes_more_than_its_share() {
    assert_eq!(heights(800.0, false, 500), Heights { block: 0.0, list: 0.0 });
    let few = heights(800.0, true, 3);
    assert_eq!(few.list, 3.0 * ROW_HEIGHT, "only what it needs");
    let many = heights(800.0, true, 2000);
    assert!(many.list <= 800.0 * MOST_OF_ROOM && many.list > 0.0, "{many:?}");
    assert_eq!(many.list % ROW_HEIGHT, 0.0, "whole rows");
    let none = heights(800.0, true, 0);
    assert_eq!(none.block, FIELD_HEIGHT + STATUS_HEIGHT + GAP);
    let tiny = heights(20.0, true, 50);
    assert!(tiny.block <= 20.0 && tiny.list >= 0.0, "{tiny:?}");
    assert_eq!(heights(f64::NAN, true, 5).block, 0.0);
}

/// Failure: the Finder's news is held back by a hand-off's freeze like a person's typing, so a list that was filling stays as it was for good once the freeze lifts; or a person's step, or an agent's reveal that moves the page, gets through the freeze and moves the page after what comes back was written.
#[test]
fn only_the_finders_news_gets_through_a_freeze() {
    assert!(!FindCall::News.is_user_action());
    for call in [
        FindCall::Typed("a".into()),
        FindCall::Open,
        FindCall::Close,
        FindCall::Step(true, Going::Jump),
        FindCall::Choose(0),
    ] {
        assert!(call.is_user_action(), "{call:?}");
    }
}

/// Failure: ⌘F with the sidebar hidden opens Find where it can't be seen, as Find lives in the sidebar; or it changes the view some way other than the app's one way, so watchers don't hear the view changed; or the host isn't told to pull the open field.
#[test]
fn opening_find_with_the_sidebar_hidden_shows_the_sidebar() {
    let rig = Rig::new();
    rig.session.with_session(|session| {
        let mut view = session.view();
        view.sidebar_hidden = true;
        session.set_view(view);
    });
    let listener = FakeListener::default();
    let mut events = Vec::new();
    rig.with(&listener, |find| find.act(FindCall::Open, &mut events));
    assert!(!rig.session.with_session(|session| session.view().sidebar_hidden));
    assert_eq!(rig.views_set.load(Ordering::SeqCst), 1, "through the app's set_view");
    assert!(events.iter().any(|event| matches!(event, Event::FindOpened { .. })), "{events:?}");
    assert!(is_open(&rig.field), "the UI thread's mirror is open");
    assert!(listener.count(Heard::ModelChanged) >= 1);
    // With the sidebar showing, opening again leaves the view alone.
    rig.with(&listener, |find| find.act(FindCall::Open, &mut events));
    assert_eq!(rig.views_set.load(Ordering::SeqCst), 1);
}

/// Failure: a query too long to find is searched anyway, or the line under the field doesn't say why nothing's found, or what was typed is lost from the field.
#[test]
fn a_query_too_long_is_said_and_not_searched() {
    let rig = Rig::new();
    let listener = FakeListener::default();
    let text = "x".repeat(find::MAX_QUERY_BYTES + 1);
    let mut events = Vec::new();
    rig.with(&listener, |find| find.act(FindCall::Typed(text.clone()), &mut events));
    let view = rig.with(&listener, |find| find.view());
    assert!(view.open);
    assert_eq!(view.status, find::too_long().message, "the line says why");
    assert_eq!(view.text, text, "the field keeps what was typed");
    assert!(!view.running && view.row_count == 0, "nothing searched: {view:?}");
    assert!(rig.session.with_session(|session| session.find().query.is_empty()));
}

/// Failure: a draft or an open that came from the record (an agent's `recto find`, a restore) isn't mirrored for the UI thread until something else changes, as Find's view isn't in what `reconcile` compares; or every look tells the host again with nothing new, so the window pulls for nothing on every change.
#[test]
fn a_change_from_the_record_tells_the_host_once() {
    let rig = Rig::new();
    let listener = FakeListener::default();
    rig.session.with_session(|session| session.set_find(true, "", "from an agent"));
    rig.with(&listener, |find| find.keep_up());
    assert_eq!(listener.take(), vec![Heard::ModelChanged]);
    let view = rig.with(&listener, |find| find.view());
    assert!(view.open && view.text == "from an agent", "{view:?}");
    rig.with(&listener, |find| find.keep_up());
    assert!(listener.heard().is_empty(), "nothing new, nothing said");
}

/// Failure: the host is told of a change while Find still holds its field's lock, so a host that pulls at once (as Listener allows) waits on the session thread, which is waiting on the host: the promise in `Listener`'s doc.
#[test]
fn the_host_is_told_with_the_field_let_go() {
    let rig = Rig::new();
    let field = Arc::clone(&rig.field);
    let session = Arc::clone(&rig.session);
    let listener = FakeListener::checking(move |_| {
        assert!(field.try_lock().is_ok(), "the field's lock is held during the call");
        session.with_session(|_| ());
    });
    let mut events = Vec::new();
    rig.with(&listener, |find| {
        find.act(FindCall::Typed("word".into()), &mut events);
        find.act(FindCall::Step(true, Going::Step), &mut events);
        find.act(FindCall::Close, &mut events);
    });
    assert!(listener.count(Heard::ModelChanged) >= 3, "{:?}", listener.heard());
    assert!(events.iter().any(|event| matches!(event, Event::FindClosed)), "{events:?}");
    assert!(!is_open(&rig.field));
}
