use super::*;
use crate::testkit::{FakeListener, Heard, QuietEngine};
use recto_agent::commands::finding::FindAsk;
use recto_reader::EngineConfig;
use std::sync::Arc;

/// A session with no tabs, a page view and the panes, and the field: what a [`FindHandle`] borrows.
struct Rig {
    session: Arc<SessionHost<DrawableLoader>>,
    field: Arc<Mutex<Field>>,
    frozen: AtomicBool,
    panes: Panes,
    engine: Engine,
}

impl Rig {
    fn new() -> Self {
        Self {
            session: Arc::new(SessionHost::default()),
            field: Arc::new(Mutex::new(Field::default())),
            frozen: AtomicBool::new(false),
            panes: Panes::new(Arc::new(QuietEngine), EngineConfig::default()),
            engine: Engine::new(EngineConfig::default(), Arc::new(QuietEngine)),
        }
    }

    fn with<R>(&self, listener: &FakeListener, step: impl FnOnce(&FindHandle<'_, FakeListener>) -> R) -> R {
        step(&FindHandle {
            session: &self.session,
            field: &self.field,
            frozen: &self.frozen,
            panes: &self.panes,
            engine: &self.engine,
            listener,
        })
    }
}

/// Failure: Find's list grows with its hits and pushes the tabs and the contents off the sidebar (owner, 2026-11-07: "max height within scroll so it doesn't push everything of screen"), or takes room while it's closed.
#[test]
fn the_list_never_takes_more_than_its_share() {
    assert_eq!(
        heights(800.0, false, 500, 1),
        Heights {
            block: 0.0,
            status: 0.0,
            list: 0.0
        }
    );
    let few = heights(800.0, true, 3, 1);
    assert_eq!(few.list, 3.0 * ROW_HEIGHT, "only what it needs");
    let many = heights(800.0, true, 2000, 1);
    assert!(many.list <= 800.0 * MOST_OF_ROOM && many.list > 0.0, "{many:?}");
    assert_eq!(many.list % ROW_HEIGHT, 0.0, "whole rows");
    let none = heights(800.0, true, 0, 1);
    assert_eq!(none.block, FIELD_HEIGHT + STATUS_HEIGHT + GAP);
    let tiny = heights(20.0, true, 50, 1);
    assert!(tiny.block <= 20.0 && tiny.list >= 0.0, "{tiny:?}");
    assert_eq!(heights(f64::NAN, true, 5, 1).block, 0.0);
}

/// Failure (the first worker's quiet run of 2026-11-13, `06-*-c-find-typed`, and the coordinator's call): why Find can't search is a sentence, and on one line
/// the sidebar cut it to "Find can't search this file: it changed on dis…", which tells the person nothing; or, letting it wrap, an ordinary status moves
/// the list, or the extra lines push the tabs and the contents down (owner, 2026-11-07: "max height within scroll so it doesn't push everything of screen").
/// A one-line status is today's height at every width; the refusal takes more lines at the default width and at the narrowest, at most three; and its extra
/// lines come out of the list's share, so the list and those lines together stay inside it.
#[test]
fn a_long_status_wraps_and_takes_its_lines_from_the_list() {
    use recto_session::record::{SIDEBAR_WIDTH, SIDEBAR_WIDTHS};
    let refusal = "Find can't search this file: it changed on disk after its unsaved edits were made.";
    for width in [SIDEBAR_WIDTHS.0, SIDEBAR_WIDTH, SIDEBAR_WIDTHS.1] {
        for ordinary in ["194 matches on 24 pages", "Not found", "Searching… page 12 of 1,024", ""] {
            assert_eq!(status_lines(ordinary, width), 1, "{ordinary:?} at {width}");
        }
    }
    let at_default = status_lines(refusal, SIDEBAR_WIDTH);
    assert!((2..=MOST_STATUS_LINES).contains(&at_default), "{at_default} lines at the default width");
    assert!(status_lines(refusal, SIDEBAR_WIDTHS.0) >= at_default, "no fewer where it's narrower");
    assert!(status_lines(refusal, SIDEBAR_WIDTHS.0) <= MOST_STATUS_LINES);
    assert_eq!(status_lines(&"word ".repeat(400), SIDEBAR_WIDTHS.0), MOST_STATUS_LINES, "capped");
    assert_eq!(
        status_lines(&"x".repeat(400), SIDEBAR_WIDTHS.0),
        MOST_STATUS_LINES,
        "one long word breaks, capped"
    );
    // A one-line status is today's block, and a long one takes its lines from the list, not the window.
    let one = heights(800.0, true, 2000, 1);
    assert_eq!(one.status, STATUS_HEIGHT);
    let three = heights(800.0, true, 2000, 3);
    assert_eq!(three.status, STATUS_HEIGHT + 2.0 * STATUS_LINE);
    assert!(
        three.list + 2.0 * STATUS_LINE <= 800.0 * MOST_OF_ROOM,
        "the extra lines inside the list's share: {three:?}"
    );
    assert!(three.list < one.list && three.list % ROW_HEIGHT == 0.0, "{three:?}");
    let few = heights(800.0, true, 2, 3);
    assert_eq!(
        (few.list, few.block),
        (2.0 * ROW_HEIGHT, FIELD_HEIGHT + few.status + 2.0 * ROW_HEIGHT + GAP),
        "a short list keeps its rows"
    );
}

/// Failure: the Finder's news is held back by a hand-off's freeze like a person's typing, so a list that was filling stays as it was for good once the freeze lifts; or an agent's reveal that moves the page gets through the freeze and moves the page after what comes back was written. (The person's own Find is their `find`, held as any person's command is.)
#[test]
fn only_the_finders_news_gets_through_a_freeze() {
    assert!(!FindCall::News.is_user_action());
    let hit = find::Hit {
        page: 0,
        start: 0,
        end: 1,
        before: "".into(),
        matched: "a".into(),
        after: "".into(),
        hidden: false,
    };
    let reveal = FindCall::Effect(recto_agent::Effect::Reveal {
        tab: recto_session::TabId(1),
        hit,
    });
    assert!(reveal.is_user_action(), "{reveal:?}");
}

/// Failure: a draft or an open that came from the record (an agent's `recto find`, a restore) isn't mirrored for the UI thread until something else changes, as Find's view isn't in what `reconcile` compares; or every look tells the host again with nothing new, so the window pulls for nothing on every change.
#[test]
fn a_change_from_the_record_tells_the_host_once() {
    let rig = Rig::new();
    let listener = FakeListener::default();
    rig.session.with_session(|session| session.viewing_mut().set_find(true, "", "from an agent"));
    rig.with(&listener, |find| find.keep_up());
    assert_eq!(listener.take(), vec![Heard::ModelChanged]);
    let view = rig.with(&listener, |find| find.view());
    assert!(view.open && view.text == "from an agent", "{view:?}");
    rig.with(&listener, |find| find.keep_up());
    assert!(listener.heard().is_empty(), "nothing new, nothing said");
}

/// Failure: the host is told of a change while Find still holds its field's lock, so a host that pulls at once (as Listener allows) waits on the session thread, which is waiting on the host: the promise in `Listener`'s doc. A change from the record, and the Finder's news.
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
    rig.session.with_session(|session| session.viewing_mut().set_find(true, "", "word"));
    rig.with(&listener, |find| {
        find.keep_up();
        // The Finder's news, with something new in the hits, as it has when it comes: the host's told only of a change.
        rig.session.finder().with_results(&mut |results| {
            let (generation, progress) = (results.generation, results.progress);
            results.add(generation, Vec::new(), &progress);
        });
        find.act(FindCall::News, &mut events);
    });
    assert_eq!(listener.count(&Heard::ModelChanged), 2, "{:?}", listener.heard());
    assert!(is_open(&rig.field));
}

/// Failure (found by R3.7's control golden, 2026-11-13): Find tells the host the model changed when nothing it draws of Find did, so the window pulls and
/// draws for nothing, against "an announcement is made when something changed, and only then": ⌘F with Find already open, ⌘G with nothing to find, each
/// told once; and ⌘G with no query, which opens Find, told twice. The person's `find` as the session thread follows it: the command's change to the record,
/// then what the window does with how it went ([`FindHandle::answered`]), then Find kept up with the session, as every call on that thread ends.
#[test]
fn find_tells_the_host_only_what_changed_what_it_draws() {
    let rig = Rig::new();
    let listener = FakeListener::default();
    let ran = |ask: FindAsk| {
        rig.with(&listener, |find| {
            find.answered(&ask, false);
            find.keep_up();
        });
        listener.take().len()
    };
    // ⌘G with no query and Find closed opens it: one change, told once.
    rig.session.with_session(|session| session.viewing_mut().set_find(true, "", ""));
    let before = rig.with(&listener, |find| find.view());
    assert_eq!(ran(FindAsk::Step { forward: true, jump: false }), 1, "⌘G that opens Find");
    assert_ne!(rig.with(&listener, |find| find.view()), before, "it did change what's drawn");
    // ⌘F with Find open: nothing changed, nothing told.
    let open = rig.with(&listener, |find| find.view());
    assert_eq!(ran(FindAsk::Open), 0, "⌘F with Find open");
    // ⌘G and ⇧⌘G with nothing to find: nothing changed, nothing told.
    assert_eq!(ran(FindAsk::Step { forward: true, jump: false }), 0, "⌘G with nothing to find");
    assert_eq!(ran(FindAsk::Step { forward: false, jump: false }), 0, "⇧⌘G with nothing to find");
    assert_eq!(rig.with(&listener, |find| find.view()), open, "and what's drawn is as it was");
    // A query too long says so under the field: a change, told.
    assert_eq!(
        rig.with(&listener, |find| {
            find.answered(&FindAsk::Open, true);
            listener.take().len()
        }),
        1,
        "a note is a change"
    );
}
