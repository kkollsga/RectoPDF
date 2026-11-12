use super::*;
use recto_core::nav::{LabelRange, LabelStyle, PageLabels};

/// Twelve pages: `i` to `iv`, then `1` to `8`.
struct Labelled;

impl DocLoader for Labelled {
    fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
        let mut toc = Toc::default();
        toc.labels = PageLabels::from_ranges(
            vec![
                LabelRange {
                    start: 0,
                    first: 1,
                    style: LabelStyle::RomanLower,
                    prefix: "".into(),
                },
                LabelRange {
                    start: 4,
                    first: 1,
                    style: LabelStyle::Decimal,
                    prefix: "".into(),
                },
            ],
            false,
        );
        Ok(DocFacts {
            page_count: 12,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc,
            renderer_only: false,
        })
    }
}

/// Failure: Back after following a link doesn't return to where the reader was, to the depth on the page; or the trail isn't in the record, so it's gone after a quit or an update; or a closed tab's trail stays in memory.
#[test]
fn back_and_forward_come_back_after_a_restart() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.go_to(id, 2).unwrap();
    s.note_place(
        id,
        PlaceRecord {
            page: 2,
            anchor_page: 2,
            within: 0.4,
            x: 0.0,
            y: 0.0,
            document_height: 1.0,
            zoom: 1.0,
            page_width: 0.0,
            page_height: 0.0,
        },
    );
    s.jump(id, 9, 0.25).unwrap();
    assert_eq!(s.tab(id).unwrap().page, 9);
    assert_eq!(s.take_jump(id), Some(trail::Stop::new(9, 0.25)), "the page view's told where to land");
    assert_eq!(s.take_jump(id), None, "once");
    assert_eq!(s.can_go_back_and_forward(id), (true, false));
    let record = s.to_record();
    let mut again = Session::new(Labelled);
    again.restore(&record, HashMap::new()).unwrap();
    assert_eq!(again.trail_of(id), s.trail_of(id), "the trail comes back as it was");
    assert!(again.go_back(id).unwrap());
    assert_eq!(
        again.take_jump(id),
        Some(trail::Stop::new(2, 0.4)),
        "back to the page and the depth it was left at"
    );
    assert_eq!(again.tab(id).unwrap().page, 2);
    assert_eq!(again.can_go_back_and_forward(id), (false, true));
    assert!(again.go_forward(id).unwrap());
    assert_eq!(again.tab(id).unwrap().page, 9);
    assert!(!again.go_forward(id).unwrap(), "nowhere further");
    again.close(id).unwrap();
    assert!(again.jumps.is_empty());
}

/// Failure: the corner shows `4 of 12` for a page the book calls `iv`, or `5 (5 of 12)` where the label says nothing new; or typing `iv` or `3` in the go-to field goes to the wrong page.
#[test]
fn labels_show_in_the_corner_and_find_pages() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    assert_eq!(s.place_words(id, 3), "iv (4 of 12)");
    assert_eq!(s.place_words(id, 4), "1 (5 of 12)");
    assert_eq!(s.page_for_typed(id, "iv"), Some(3));
    assert_eq!(s.page_for_typed(id, "3"), Some(6), "the page labelled 3, as Preview reads it");
    assert_eq!(s.page_for_typed(id, "12"), Some(11), "a number no page is labelled is the page");
    assert_eq!(s.page_for_typed(id, "13"), None);
    assert_eq!(s.page_for_typed(id, "xyz"), None);
    // A moved page keeps the label its file gives it.
    s.edit_pages(id, |edits| edits.move_pages(3, 1, 0)).unwrap();
    assert_eq!(s.page_label(id, 0).as_deref(), Some("iv"));
    assert_eq!(s.page_for_typed(id, "iv"), Some(0));
    s.go_to(id, 0).unwrap();
    assert_eq!(s.sidebar().place, "iv (1 of 12)");
}

/// Failure: a tab's trail makes a cold tab cost more than its budget of 4 KiB, as nothing counts it.
#[test]
fn a_cold_tab_with_a_full_trail_stays_in_budget() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    for round in 0..(trail::MAX_TRAIL as u32 * 3) {
        s.jump(id, round % 12, 0.0).unwrap();
    }
    s.go_back(id).unwrap();
    for _ in 0..trail::MAX_TRAIL {
        s.go_back(id).unwrap();
    }
    let tab = s.tab(id).unwrap();
    let cost = std::mem::size_of::<TabRecord>()
        + tab.name.len()
        + tab.file_name.len()
        + tab.trail.as_ref().map_or(0, |trail| std::mem::size_of::<trail::Trail>() + trail.heap_bytes());
    assert!(cost <= 4096, "{cost} bytes");
    assert!(tab.trail.as_ref().is_some_and(|trail| !trail.back().is_empty() || !trail.forward().is_empty()));
}

/// Failure: after a page is deleted or moved, Back goes to the wrong page, or to where a deleted page used to be.
#[test]
fn the_trail_follows_page_edits() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.go_to(id, 2).unwrap();
    s.jump(id, 8, 0.0).unwrap();
    s.jump(id, 5, 0.0).unwrap();
    // Page 3 (position 2) moves to the front; page 9 (position 8) is deleted.
    s.edit_pages(id, |edits| edits.move_pages(2, 1, 0)).unwrap();
    s.edit_pages(id, |edits| edits.delete(8, 1)).unwrap();
    let back: Vec<u32> = s.trail_of(id).unwrap().back().iter().map(|stop| stop.page).collect();
    assert_eq!(back, [0], "the moved page's stop followed it, and the deleted page's went");
}

/// Failure: an agent's `goto` pushes the person's own places out of the trail, so after an agent has been busy Back no longer reaches where they were.
#[test]
fn an_agents_jumps_give_way_to_the_persons() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    for round in 0..10 {
        s.jump(id, (round % 2) * 5 + 1, 0.0).unwrap();
    }
    let persons = s.trail_of(id).unwrap().back().len();
    for round in 0..200u32 {
        s.jump_as(id, round % 11, 0.0, Author::Agent).unwrap();
    }
    let trail = s.trail_of(id).unwrap();
    assert_eq!(trail.back().iter().filter(|stop| !stop.by_agent).count(), persons);
    assert!(trail.back().len() <= trail::MAX_TRAIL);
    let record = s.to_record();
    assert!(record.tabs[0].back.iter().any(|stop| stop.by_agent), "who left each is kept");
}
