use super::*;

/// Failure: a field added carelessly raises the cost of every remembered draw.
#[test]
fn draw_note_stays_small() {
    assert!(std::mem::size_of::<DrawNote>() <= 32);
}

/// Failure: wait says a page is drawn when a different page now has that number, because notes are by position and outlive edits.
#[test]
fn draw_notes_go_when_the_pages_move() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    let note = DrawNote {
        tab: id,
        page: 0,
        failure: None,
    };
    s.note_drawn(note);
    s.edit_pages(id, |e| e.move_pages(0, 1, 2)).unwrap();
    assert_eq!(s.draw_note(id, 0), None, "after an edit");
    s.note_drawn(note);
    assert!(s.undo_edit(id));
    assert_eq!(s.draw_note(id, 0), None, "after an undo");
    s.note_drawn(note);
    assert!(s.redo_edit(id));
    assert_eq!(s.draw_note(id, 0), None, "after a redo");
    s.note_drawn(note);
    s.discard_edits(id);
    assert_eq!(s.draw_note(id, 0), None, "after a discard");
    s.note_drawn(note);
    assert_eq!(s.edit_pages(id, |e| e.rotate(99, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
    assert_eq!(s.draw_note(id, 0), Some(note), "a refused edit changes nothing");
}

/// Failure: the draw notes grow with a long session, or an old result survives its tab, so a waiter is told a closed tab's page is drawn.
#[test]
fn draw_notes_are_capped_and_go_with_their_tab() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    for page in 0..(MAX_DRAWS_KEPT as u32 * 2) {
        s.note_drawn(DrawNote { tab: a, page, failure: None });
    }
    assert_eq!(s.draws.len(), MAX_DRAWS_KEPT);
    assert!(s.draw_note(a, 0).is_none());
    let last = MAX_DRAWS_KEPT as u32 * 2 - 1;
    assert!(s.draw_note(a, last).is_some());
    s.close(a).unwrap();
    assert!(s.draw_note(a, last).is_none());
}
