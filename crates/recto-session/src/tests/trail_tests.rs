use super::*;

/// Failure: Back after following a link doesn't return to where the reader was, to the depth on the page; or the trail isn't in the record, so it's gone after a quit or an update; or a closed tab's trail stays in memory.
#[test]
fn back_and_forward_come_back_after_a_restart() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.go_to(id, 2).unwrap();
    s.view_told_of(
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
        1,
    );
    s.jump(id, 9, 0.25).unwrap();
    assert_eq!(s.tabs().tab(id).unwrap().page, 9);
    assert_eq!(s.take_jump(id), Some(trail::Stop::new(9, 0.25)), "the page view's told where to land");
    assert_eq!(s.take_jump(id), None, "once");
    assert_eq!(s.tabs().can_go_back_and_forward(id), (true, false));
    let record = s.to_record();
    let mut again = Session::new(Labelled);
    again.restore(&record, HashMap::new()).unwrap();
    assert_eq!(again.tabs().trail_of(id), s.tabs().trail_of(id), "the trail comes back as it was");
    assert!(again.step_as(id, true, Author::Owner).unwrap());
    assert_eq!(
        again.take_jump(id),
        Some(trail::Stop::new(2, 0.4)),
        "back to the page and the depth it was left at"
    );
    assert_eq!(again.tabs().tab(id).unwrap().page, 2);
    assert_eq!(again.tabs().can_go_back_and_forward(id), (false, true));
    assert!(again.step_as(id, false, Author::Owner).unwrap());
    assert_eq!(again.tabs().tab(id).unwrap().page, 9);
    assert!(!again.step_as(id, false, Author::Owner).unwrap(), "nowhere further");
    again.close(id).unwrap();
    assert!(again.mail.no_jumps());
}

/// Failure: a tab's trail makes a cold tab cost more than its budget of 4 KiB, as nothing counts it.
#[test]
fn a_cold_tab_with_a_full_trail_stays_in_budget() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    for round in 0..(trail::MAX_TRAIL as u32 * 3) {
        s.jump(id, round % 12, 0.0).unwrap();
    }
    s.step_as(id, true, Author::Owner).unwrap();
    for _ in 0..trail::MAX_TRAIL {
        s.step_as(id, true, Author::Owner).unwrap();
    }
    let tab = s.tabs().tab(id).unwrap();
    let cost = std::mem::size_of::<TabRecord>()
        + tab.name.len()
        + tab.file.file_name.len()
        + tab
            .reading
            .trail
            .as_ref()
            .map_or(0, |trail| std::mem::size_of::<trail::Trail>() + trail.heap_bytes());
    assert!(cost <= 4096, "{cost} bytes");
    assert!(
        tab.reading
            .trail
            .as_ref()
            .is_some_and(|trail| !trail.back().is_empty() || !trail.forward().is_empty())
    );
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
    let back: Vec<u32> = s.tabs().trail_of(id).unwrap().back().iter().map(|stop| stop.page).collect();
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
    let persons = s.tabs().trail_of(id).unwrap().back().len();
    for round in 0..200u32 {
        s.jump_as(id, round % 11, 0.0, Author::Agent).unwrap();
    }
    let trail = s.tabs().trail_of(id).unwrap();
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
    assert_eq!(s.tabs().tab(id).unwrap().page, 9);
    assert_eq!(s.sidebar().place, "10 of 10");
    let mut empty = session(0);
    let id = empty.open(Path::new("/d/empty.pdf")).unwrap();
    empty.go_to(id, 3).unwrap();
    assert_eq!(empty.tabs().tab(id).unwrap().page, 0);
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
    s.view_told_of(a, place, 1);
    assert_eq!(s.take_place_delta(2).places.len(), 1);
    s.view_told_of(a, place, 2);
    assert!(s.take_place_delta(3).places.is_empty(), "it didn't move");
    assert_eq!(s.tabs().place_to_show(a), Some(place));
    s.go_to(a, 4).unwrap();
    assert_eq!(s.tabs().place_to_show(a), None, "it's for page 1, and the reader's on page 5");
    assert_eq!(s.take_place_delta(4).places.len(), 1);
}

/// Failure: a step straight after a jump the page view hasn't taken marks where the page view last said it was, not where the session sent the reader, so
/// Forward after Back lands at the old place on the page; or a move since that sent no jump of its own (Find's step) is passed over for the jump before it.
#[test]
fn a_jump_not_yet_taken_is_where_the_reader_is() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.jump(id, 4, 0.3).unwrap();
    s.take_jump(id);
    s.jump(id, 4, 0.8).unwrap();
    assert_eq!(s.here(id), Some(trail::Stop::new(4, 0.8)), "the jump waiting");
    assert!(s.step_as(id, true, Author::Owner).unwrap());
    assert!(s.step_as(id, false, Author::Owner).unwrap());
    assert_eq!(s.take_jump(id), Some(trail::Stop::new(4, 0.8)), "Forward comes back to it");
    s.jump(id, 6, 0.5).unwrap();
    s.go_to(id, 7).unwrap();
    assert_eq!(s.here(id), Some(trail::Stop::new(7, 0.0)), "a move since, with no jump of its own");
}

fn on_page_5(within: f64) -> PlaceRecord {
    PlaceRecord {
        page: 4,
        anchor_page: 4,
        within,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    }
}

/// A session with a book open on page 5, and a jump 0.8 of the way down it sent the page view by the look, the send having come to 10 on the place clock.
fn sent_on_page_5() -> (Session<Labelled>, TabId) {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.jump(id, 4, 0.8).unwrap();
    let jump = s.waiting_jump(id).expect("a jump waiting");
    s.jump_sent(id, jump, 10);
    assert_eq!(s.waiting_jump(id), None, "sent, it no longer waits");
    (s, id)
}

/// Failure: which read of the page view is newer goes by when it was taken in and not when it was made (the place clock's stamp), so a read made before
/// the look sent a jump, taken in after, has the reader where they were before the jump (Back keeps the old place for Forward); a read made after the
/// person's own scroll since is passed over for the jump; or an older read still landing undoes a newer one already taken in.
#[test]
fn the_newer_read_of_the_view_wins_over_a_jump_sent_and_an_older_read() {
    let (mut s, id) = sent_on_page_5();
    assert!(s.view_told_of(id, on_page_5(0.3), 9), "newer than anything taken in yet, so taken");
    assert_eq!(
        s.here(id),
        Some(trail::Stop::new(4, 0.8)),
        "but made before the send: the jump's still where the reader is"
    );
    assert!(s.view_told_of(id, on_page_5(0.6), 12));
    assert_eq!(s.here(id), Some(trail::Stop::new(4, 0.6)), "made after the send: the person's scroll since");
    assert!(!s.view_told_of(id, on_page_5(0.3), 11), "older than the read taken in");
    assert_eq!(s.here(id), Some(trail::Stop::new(4, 0.6)), "an older read doesn't undo it");
    assert!(s.view_told_of(id, on_page_5(0.65), 12), "as new as the last: taken");
    assert_eq!(s.here(id), Some(trail::Stop::new(4, 0.65)));
}

/// Failure: a read made after the send and taken in before the look says it sent the jump (a command's take-in between the send and the look's second
/// lock on the session) doesn't tell of the jump, so the jump is kept as where the reader is past the person's scroll.
#[test]
fn a_read_after_the_send_taken_in_before_the_look_says_so_tells_of_it() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.jump(id, 4, 0.8).unwrap();
    let jump = s.waiting_jump(id).unwrap();
    assert!(s.view_told_of(id, on_page_5(0.7), 20));
    s.jump_sent(id, jump, 15);
    assert_eq!(s.here(id), Some(trail::Stop::new(4, 0.7)), "the read at 20 came after the send at 15");
}

/// Failure: the look takes out of waiting a newer jump that took the place of the one it sent while it had the session let go (an agent's `goto` between
/// the look's two locks), so the newer jump is never sent; or it's passed over for the one sent.
#[test]
fn a_newer_jump_waiting_stays_waiting_when_the_look_says_it_sent_the_old_one() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    s.jump(id, 4, 0.8).unwrap();
    let peeked = s.waiting_jump(id).unwrap();
    s.jump(id, 4, 0.2).unwrap();
    s.jump_sent(id, peeked, 10);
    assert_eq!(s.waiting_jump(id), Some(trail::Stop::new(4, 0.2)), "the newer jump waits for the next look");
    assert_eq!(s.here(id), Some(trail::Stop::new(4, 0.2)), "and comes first");
}

/// Failure: a jump sent is kept as where the reader is past a move to another page since, with no read of the view to tell of it; or a closed tab's
/// word stays in the session. (An edit, a save and dormancy: `forgetting.rs`.)
#[test]
fn a_jump_sent_goes_with_a_move_and_a_close() {
    let (mut s, id) = sent_on_page_5();
    assert!(s.view_told_of(id, on_page_5(0.6), 5), "made before the send");
    s.go_to(id, 9).unwrap();
    s.go_to(id, 4).unwrap();
    assert_eq!(
        s.here(id),
        Some(trail::Stop::new(4, 0.6)),
        "moved to another page and back since, with no jump of its own"
    );
    s.close(id).unwrap();
    assert!(s.mail.no_jumps(), "a closed tab's word goes with it");
}
