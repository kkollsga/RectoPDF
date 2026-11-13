use super::*;

/// Failure: the corner shows `4 of 12` for a page the book calls `iv`, or `5 (5 of 12)` where the label says nothing new; or typing `iv` or `3` in the go-to field goes to the wrong page.
#[test]
fn labels_show_in_the_corner_and_find_pages() {
    let mut s = Session::new(Labelled);
    let id = s.open(Path::new("/d/book.pdf")).unwrap();
    assert_eq!(s.pages().place_words(id, 3), "iv (4 of 12)");
    assert_eq!(s.pages().place_words(id, 4), "1 (5 of 12)");
    assert_eq!(s.pages().for_typed(id, "iv"), Some(3));
    assert_eq!(s.pages().for_typed(id, "3"), Some(6), "the page labelled 3, as Preview reads it");
    assert_eq!(s.pages().for_typed(id, "12"), Some(11), "a number no page is labelled is the page");
    assert_eq!(s.pages().for_typed(id, "13"), None);
    assert_eq!(s.pages().for_typed(id, "xyz"), None);
    // A moved page keeps the label its file gives it.
    s.edit_pages(id, |edits| edits.move_pages(3, 1, 0)).unwrap();
    assert_eq!(s.pages().label(id, 0).as_deref(), Some("iv"));
    assert_eq!(s.pages().for_typed(id, "iv"), Some(0));
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
    assert_eq!(s.pages().count(id), 7);
    assert_eq!(s.sidebar().place, "2 of 7");
    s.discard_edits(id);
    assert_eq!(s.sidebar().place, "5 of 5", "the reader stays on the page they were reading");
}

/// Failure: what a page is called or how many pages a tab has comes from something other than the tab's record, its list of edits and its contents'
/// labels, so the look needs the whole session (R3.5): built here from those three alone. A tab with no contents read shows numbers, and a moved page
/// keeps its file's label.
#[test]
fn pages_are_a_look_over_the_tab_its_list_and_its_labels() {
    let mut tabs = registry::Tabs::new();
    tabs.push(bare_tab(1, 12));
    tabs.push(bare_tab(2, 7));
    let mut warm = warm::Warm::new();
    warm.push(TabId(1), Arc::new(Labelled.load(Path::new("/d/book.pdf")).unwrap().toc), |_| {});
    let mut lists = lists::Lists::new();
    let look = |lists: &lists::Lists| {
        let pages = Pages {
            tabs: &tabs,
            lists,
            warm: &warm,
        };
        (
            pages.place_words(TabId(1), 0),
            pages.count(TabId(1)),
            pages.label(TabId(2), 0),
            pages.count(TabId(2)),
            pages.for_typed(TabId(1), "iv"),
        )
    };
    assert_eq!(look(&lists), ("i (1 of 12)".to_string(), 12, None, 7, Some(3)));
    let mut moved = PageEdits::new(12);
    moved.move_pages(3, 1, 0).unwrap();
    moved.delete(11, 1).unwrap();
    lists.insert(TabId(1), moved);
    assert_eq!(look(&lists), ("iv (1 of 11)".to_string(), 11, None, 7, Some(0)));
}
