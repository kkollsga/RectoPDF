use super::*;

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

/// Failure: a page past the end leaves the reader nowhere, or a zero-page file underflows.
#[test]
fn going_to_a_page_clamps_to_the_document() {
    let mut s = session(10);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.go_to(id, 500).unwrap();
    assert_eq!(s.tab(id).unwrap().page, 9);
    assert_eq!(s.sidebar().place, "10 of 10");
    let mut empty = session(0);
    let id = empty.open(Path::new("/d/empty.pdf")).unwrap();
    empty.go_to(id, 3).unwrap();
    assert_eq!(empty.tab(id).unwrap().page, 0);
}

/// Failure: a place for an old page is used after a `goto` moved a cold tab, so the reader lands on the wrong page. And a place that's the same doesn't count as a change, so nothing's written for it.
#[test]
fn a_place_is_only_used_on_its_own_page_and_only_written_when_it_moves() {
    let mut s = session(9);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.take_place_delta(1);
    let place = PlaceRecord {
        page: 0,
        anchor_page: 0,
        within: 0.5,
        x: 0.0,
        y: 10.0,
        document_height: 900.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    s.note_place(a, place);
    assert_eq!(s.take_place_delta(2).places.len(), 1);
    s.note_place(a, place);
    assert!(s.take_place_delta(3).places.is_empty(), "it didn't move");
    assert_eq!(s.place_to_show(a), Some(place));
    s.go_to(a, 4).unwrap();
    assert_eq!(s.place_to_show(a), None, "it's for page 1, and the reader's on page 5");
    assert_eq!(s.take_place_delta(4).places.len(), 1);
}
