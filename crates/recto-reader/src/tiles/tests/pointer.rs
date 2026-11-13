use super::*;

/// Failure: going to Find's hit leaves it off screen or at the very edge of the view, as it did at the foot of a page jumped to; or stepping between hits already on screen moves the page each time. Selects the hit too.
#[test]
fn finds_hit_is_selected_and_brought_a_third_down_the_view() {
    let mut reader = reader_on(10);
    settle(&mut reader);
    let range = recto_index::select::Range {
        start: recto_index::select::Place { page: 6, at: 0 },
        end: recto_index::select::Place { page: 6, at: 4 },
    };
    reader.reveal_range(range);
    assert_eq!(reader.pointer.selection_range(), Some(range), "selected");
    let to_page = reader
        .host
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .expect("its page, at once");
    reader.set_viewport(view(to_page));
    reader.take_text(1, 6, Some((page_with_line_at(40.0), true)));
    let placed = reader
        .host
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
        .expect("then the hit, once its place is known");
    let doc = reader.doc().unwrap();
    let top = doc.layout.rect(6).y + (792.0 - 50.0) * doc.layout.fit();
    let height = view(0.0).height;
    assert!(
        top >= placed + height * 0.25 && top <= placed + height * 0.4,
        "a third down: hit at {top}, view from {placed}"
    );
    // Another hit on what's showing, away from the edges, moves nothing.
    reader.set_viewport(view(placed));
    reader.host.take_ops();
    let near = recto_index::select::Range {
        start: recto_index::select::Place { page: 6, at: 5 },
        end: recto_index::select::Place { page: 6, at: 7 },
    };
    reader.reveal_range(near);
    assert!(
        !reader.host.take_ops().iter().any(|op| matches!(op, Op::ScrollTo { .. })),
        "it's in view already"
    );
}

/// The last place the view was sent to, from what's waiting for the host.
fn last_scroll(reader: &mut Reader) -> Option<f64> {
    reader
        .host
        .take_ops()
        .into_iter()
        .rev()
        .find_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None })
}

/// Failure: Find's hit waiting for its page's text takes the view back to it when the text comes, after the session has sent the view somewhere else, as
/// Back or a page typed straight after ⌘G does: the view leaves where the session has it, and tells of it as the reader's own move. The last thing asked
/// for wins, for a jump to a page's top and one a way down it alike; and the session's own jump to the hit's page, which comes after the reveal it went with, keeps it.
#[test]
fn a_jump_the_session_sends_drops_a_hit_waiting_for_its_text() {
    let hit = recto_index::select::Range {
        start: recto_index::select::Place { page: 6, at: 0 },
        end: recto_index::select::Place { page: 6, at: 4 },
    };
    for (what, within) in [("to its top", 0.0), ("a way down it", 0.3)] {
        let mut reader = reader_on(10);
        settle(&mut reader);
        reader.reveal_range(hit);
        let to_hit = last_scroll(&mut reader).expect("the hit's page, at once");
        reader.set_viewport(view(to_hit));
        reader.jump_to(1, within);
        let back = last_scroll(&mut reader).expect("page 2");
        reader.set_viewport(view(back));
        reader.take_moved_page();
        reader.take_text(1, 6, Some((page_with_line_at(40.0), true)));
        assert_eq!(last_scroll(&mut reader), None, "{what}: the text of the hit's page moves nothing");
        assert_eq!(
            reader.doc().map(|doc| doc.reading_anchor()).map(|(page, _)| page),
            Some(1),
            "{what}: still where the jump put it"
        );
        assert_eq!(reader.take_moved_page(), None, "{what}: and tells of no move");
    }
    // The session's jump to the hit's own page, as the look after ⌘G sends it, leaves the hit to be put a third down once its text is in.
    let mut reader = reader_on(10);
    settle(&mut reader);
    reader.reveal_range(hit);
    reader.jump_to(6, 0.0);
    let to_page = last_scroll(&mut reader).expect("the hit's page");
    reader.set_viewport(view(to_page));
    reader.take_text(1, 6, Some((page_with_line_at(40.0), true)));
    assert!(last_scroll(&mut reader).is_some(), "the hit, once its place is known");
}

/// Failure: pointing at a page never asks for its links, so the hand and the hint never come; or a link's answer about another tab lands on this one; or a click on a link isn't seen as one.
#[test]
fn a_page_pointed_at_has_its_links_read_and_a_click_found() {
    use recto_core::nav::{Link, LinkTarget, PageLinks};
    let mut reader = reader_on(10);
    settle(&mut reader);
    let (x, y) = reader.doc().unwrap().document_point(0, 50.0, 20.0).unwrap();
    assert_eq!(reader.pointer_link(x, y), None, "not read yet");
    let job = reader.next_job().expect("its links are asked for first");
    assert!(matches!(job.kind, JobKind::Links) && job.position == 0, "{job:?}");
    let links = PageLinks {
        links: vec![Link {
            rect: [10.0, 10.0, 100.0, 30.0],
            target: LinkTarget::Page { page: 4, within: None },
        }],
        ..PageLinks::default()
    };
    reader.finish(job, Outcome::Links(Some(Arc::new(links))));
    let hit = reader.pointer_link(x, y).expect("now it's known");
    assert_eq!(hit.target, LinkTarget::Page { page: 4, within: None });
    assert_eq!(reader.doc().unwrap().position_in_same_file(0, 4), Some(4));
    reader.selection_press(x, y, 1, false);
    reader.pointer.selection_release();
    assert_eq!(reader.pointer.take_clicked_link().map(|hit| hit.page), Some(0));
    // Another tab: what's known goes with the one that was showing.
    reader.activate(2, 10, 0, map(), &[], None, Spread::Single);
    assert_eq!(reader.pointer.link_pages_held(), 0);
}

/// Failure: a hidden-text check that takes seconds keeps the page the reader has just scrolled to blank, over and over on a hostile file, as a running text job couldn't be told to stop. Its first read, which is quick and is what the reader's hand is waiting on, isn't given up for a scroll.
#[test]
fn a_check_goes_when_the_view_wants_something_and_a_read_does_not() {
    let mut reader = reader_on(100);
    settle_to_text(&mut reader);
    reader.select_all();
    let first = settle_to_text(&mut reader).expect("what shows is read");
    assert!(matches!(first.kind, JobKind::Text { check: false }), "{first:?}");
    reader.note_started(&first, None);
    reader.set_viewport(view(40_000.0));
    assert!(reader.running_is_wanted(), "a read stays, as what's showing needs it");
    reader.work.running = None;
    // Back where it was, with everything there, and the check runs.
    reader.set_viewport(view(0.0));
    let text = Arc::new(recto_index::select::PageText::default());
    reader.finish(first, Outcome::Text { text, checked: false });
    let check = settle_to_text(&mut reader).expect("selected and showing, so it's checked");
    assert!(matches!(check.kind, JobKind::Text { check: true }), "{check:?}");
    reader.note_started(&check, None);
    assert!(reader.running_is_wanted(), "nothing else is wanted");
    // A scroll to pages with nothing drawn.
    let asked = reader.counts.cancels_asked;
    reader.set_viewport(view(40_000.0));
    assert!(!reader.running_is_wanted(), "the view wants tiles");
    assert_eq!(reader.counts.cancels_asked, asked + 1);
}

/// Failure: a check that was stopped reads as one that was tried, so the page is never checked, or a stopped read holds a place in the cache as a page that failed.
#[test]
fn a_stopped_text_job_is_asked_for_again() {
    let mut reader = reader_on(10);
    settle_to_text(&mut reader);
    reader.select_all();
    let read = settle_to_text(&mut reader).expect("what shows is read");
    let page = read.position;
    reader.finish(read, Outcome::Cancelled);
    let again = settle_to_text(&mut reader).expect("it's still wanted");
    assert_eq!(again.position, page);
    assert!(matches!(again.kind, JobKind::Text { check: false }));
    reader.finish(
        again,
        Outcome::Text {
            text: Arc::new(recto_index::select::PageText::default()),
            checked: false,
        },
    );
    let check = settle_to_text(&mut reader).expect("not tried, so asked for");
    assert!(matches!(check.kind, JobKind::Text { check: true }));
}
