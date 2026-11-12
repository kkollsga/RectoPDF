use super::*;

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

/// Failure: the footer and page clamp ignore added or deleted pages, so the reader lands past the end.
#[test]
fn page_count_and_position_follow_edits() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.go_to(id, 4).unwrap();
    s.edit_pages(id, |e| e.delete(1, 3)).unwrap();
    assert_eq!(s.sidebar().place, "2 of 2");
    s.insert_pdf(id, 2, Path::new("/d/b.pdf")).unwrap();
    assert_eq!(s.pages_in(id), 7);
    assert_eq!(s.sidebar().place, "2 of 7");
    s.discard_edits(id);
    assert_eq!(s.sidebar().place, "5 of 5", "the reader stays on the page they were reading");
}
