use super::*;

/// Failure: after a move, the place stays at the same number and shows a different page, which no reader would do. It stays on the page, and if that page goes, it lands on the nearest and says so once.
#[test]
fn the_place_follows_the_page_being_read() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.go_to(id, 3).unwrap();
    s.edit_pages(id, |e| e.move_pages(3, 1, 0)).unwrap();
    assert_eq!(s.tab(id).unwrap().page, 0, "page 4 moved to the front, and the reader with it");
    assert!(!s.take_place_lost());
    s.edit_pages(id, |e| e.move_pages(1, 1, 3)).unwrap();
    assert_eq!(s.tab(id).unwrap().page, 0, "another page moved, so nothing changes");
    s.edit_pages(id, |e| e.delete(0, 1)).unwrap();
    assert!(s.take_place_lost(), "the page being read was deleted");
    assert!(!s.take_place_lost(), "it's only said once");
    assert_eq!(s.tab(id).unwrap().page, 0, "the nearest there is");
    s.go_to(id, 3).unwrap();
    s.edit_pages(id, |e| e.delete(3, 1)).unwrap();
    assert!(s.take_place_lost());
    assert_eq!(s.tab(id).unwrap().page, 2, "the last page, since that was the end");
    let before = s.tab(id).unwrap().page;
    assert!(s.undo_edit(id));
    assert_eq!(s.tab(id).unwrap().page, before, "undo keeps the page in view while one comes back behind it");
}

/// Failure: a tab back at its file through four quarter turns still reads as edited, and the sidebar's dot and the refusal to close stay with it. Its steps can still be undone, because the list isn't dropped while it has any. And a `page move` to where it already is must leave no list behind at all.
#[test]
fn a_tab_back_at_its_file_isnt_edited() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(id, |e| e.move_pages(2, 1, 2)).unwrap();
    assert!(s.page_edits(id).is_none() && !s.sidebar().edited, "nothing happened");
    for _ in 0..4 {
        s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
    }
    assert!(!s.is_edited(id) && !s.sidebar().edited);
    assert!(s.page_edits(id).unwrap().can_undo(), "it can still step back through them");
    assert!(s.undo_edit(id));
    assert!(s.is_edited(id));
    assert!(s.redo_edit(id));
    assert!(s.close(id).is_ok(), "it matches the file, and nothing's been undone");
}

/// Failure: a refused edit leaves an 8-byte-a-page list behind forever, over the budget for an idle tab, and the tab's length stops following its file.
#[test]
fn a_refused_first_edit_leaves_no_list() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    assert_eq!(s.edit_pages(id, |e| e.rotate(999, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
    assert!(s.page_edits(id).is_none(), "a tab with no edits must cost nothing here");
    s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
    assert_eq!(s.edit_pages(id, |e| e.rotate(999, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
    assert!(s.page_edits(id).is_some(), "a real edit stays");
    assert!(s.undo_edit(id));
    assert!(s.page_edits(id).is_some(), "it can be redone");
    assert!(s.redo_edit(id));
    s.discard_edits(id);
    assert!(s.page_edits(id).is_none());
}

/// Failure: a file replaced behind an edited tab, or behind an added PDF, is read at the old page numbers as if nothing happened. A tab with no edits has nothing to get wrong, and reads the file as it is.
#[test]
fn a_page_knows_whether_its_file_is_the_one_it_was_numbered_from() {
    let dir = recto_testpdf::TempDir::new("recto-stamp-");
    std::fs::create_dir_all(&dir).unwrap();
    let (own, added) = (dir.join("own.pdf"), dir.join("added.pdf"));
    std::fs::write(&own, b"one").unwrap();
    std::fs::write(&added, b"two").unwrap();
    let mut s = session(3);
    let id = s.open(&own).unwrap();
    assert_eq!(s.page_source(id, 0).unwrap().expected, None, "no edits, so no claim");
    s.insert_pdf(id, 0, &added).unwrap();
    assert!(s.page_source(id, 0).unwrap().added);
    assert!(s.page_source(id, 0).unwrap().is_current());
    assert!(s.page_source(id, 3).unwrap().is_current());
    assert!(s.own_file_is_current(id));
    std::fs::write(&added, b"two, longer").unwrap();
    assert!(!s.page_source(id, 0).unwrap().is_current(), "the added file changed");
    assert!(s.page_source(id, 3).unwrap().is_current(), "the tab's own file hasn't");
    assert_eq!(
        s.insert_pdf(id, 0, &added),
        Err(SessionError::Edit(EditError::SourceChanged)),
        "two versions can't share page numbers"
    );
    std::fs::remove_file(&added).unwrap();
    assert!(!s.page_source(id, 0).unwrap().is_current(), "the added file's gone");
    std::fs::write(&own, b"one, longer").unwrap();
    assert!(!s.page_source(id, 3).unwrap().is_current());
    assert!(!s.own_file_is_current(id));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure: a tab with added pages wakes from cold and lands back inside the file's own length, so the reader jumps away from the page they were on.
#[test]
fn waking_an_edited_tab_keeps_its_place_in_the_edited_length() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.insert_pdf(a, 5, Path::new("/d/added.pdf")).unwrap();
    s.go_to(a, 8).unwrap();
    for i in 0..MAX_WARM + 1 {
        s.open(Path::new(&format!("/d/other{i}.pdf"))).unwrap();
    }
    assert_eq!(s.tier(a), Tier::Cold);
    s.tap(a).unwrap();
    assert_eq!(s.tab(a).unwrap().page, 8);
}

/// Failure: after a move or a delete, reading "page 2" reads the file's second page rather than the one now in that place.
#[test]
fn a_pages_source_follows_edits() {
    let mut s = session(3);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let three = s.page_source(a, 2).unwrap();
    assert_eq!(
        (three.path.as_path(), three.page, three.quarter_turns, three.added),
        (Path::new("/d/a.pdf"), 2, 0, false)
    );
    assert_eq!(s.page_source(a, 3), None);
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let first = s.page_source(a, 0).unwrap();
    assert_eq!(
        (first.path.as_path(), first.page, first.quarter_turns, first.added),
        (Path::new("/d/a.pdf"), 1, 1, false)
    );
    s.insert_pdf(a, 0, Path::new("/d/b.pdf")).unwrap();
    let second = s.page_source(a, 1).unwrap();
    assert_eq!((second.path.as_path(), second.page, second.added), (Path::new("/d/b.pdf"), 1, true));
}

struct RendererOnlyLoader;

impl DocLoader for RendererOnlyLoader {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 4,
            byte_len: 1,
            encrypted: false,
            version: "".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: true,
        })
    }
}

/// Failure: a file only the renderer can read gets edited or added to another, though nothing could ever save the result. One writer: it must be readable and drawable, never editable.
#[test]
fn a_renderer_only_file_can_be_opened_but_not_edited_or_added() {
    let mut s = Session::new(RendererOnlyLoader);
    let id = s.open(Path::new("/d/damaged.pdf")).unwrap();
    assert_eq!(s.pages_in(id), 4);
    assert!(s.tab(id).unwrap().renderer_only);
    assert_eq!(s.edit_pages(id, |e| e.rotate(0, 1, 1)), Err(SessionError::RendererOnly));
    assert!(!s.is_edited(id));
    let mut other = session(3);
    let target = other.open(Path::new("/d/fine.pdf")).unwrap();
    let facts = RendererOnlyLoader.load(Path::new("/d/damaged.pdf")).unwrap();
    assert_eq!(
        other.insert_pdf_with_facts(target, Author::Owner, 0, Path::new("/d/damaged.pdf"), &facts, None),
        Err(SessionError::RendererOnly)
    );
    assert!(!other.is_edited(target));
}

/// Failure: undone steps of an agent's own hold up the app's close, or read as the owner's. They're an agent's to lose.
#[test]
fn an_agents_undone_steps_dont_stop_a_close() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages_as(id, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(s.undo_edit(id));
    assert!(!s.has_undone_owner_steps(id) && !s.sidebar().undone_owner_steps);
    assert!(s.close(id).is_ok());
}

/// Failure: every open tab pays for page edits, so idle tabs cost more once one PDF is edited.
#[test]
fn only_edited_tabs_have_an_edit_list() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.open(Path::new("/d/b.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    assert_eq!(s.lists.in_memory(), 1);
}

/// Failure: a dropped PDF that can't be read changes the document anyway.
#[test]
fn a_dropped_pdf_that_cant_be_read_changes_nothing() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    assert_eq!(s.insert_pdf(id, 0, Path::new("/d/missing.pdf")), Err(SessionError::Open(OpenError::NotFound)));
    assert!(!s.is_edited(id));
    assert_eq!(s.pages_in(id), 5);
}

/// Failure: the panel's Rotate outside edit mode turns one page, or takes a step a page, so one undo doesn't take it back; or in edit mode it turns every page. Owner, 2026-11-07: "When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment)".
#[test]
fn the_panels_rotate_turns_every_page_as_one_step() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    assert!(s.can_edit_pages(a));
    s.rotate_from_panel(a, 1).unwrap();
    assert!(s.page_edits(a).unwrap().slots().iter().all(|slot| slot.quarter_turns == 1), "every page");
    assert!(s.is_edited(a), "it's an unsaved edit, for Save to commit");
    assert!(s.undo_edit_as(a, Author::Owner).unwrap());
    assert!(
        s.page_edits(a).unwrap().slots().iter().all(|slot| slot.quarter_turns == 0),
        "one undo takes it all back"
    );
    assert!(!s.undo_and_redo(a).0, "it was one step");
    s.set_edit_mode(true);
    s.go_to(a, 3).unwrap();
    s.rotate_from_panel(a, -1).unwrap();
    let turns: Vec<u8> = s.page_edits(a).unwrap().slots().iter().map(|slot| slot.quarter_turns).collect();
    assert_eq!(turns, [0, 0, 0, 3, 0], "edit mode turns the page being read");
}

/// Failure (review B9): a file claiming 50 million pages makes a list of that many at its first edit, 400 MiB. It's refused.
#[test]
fn a_huge_claim_of_pages_makes_no_list() {
    let mut s = session(edit::MAX_PAGES as u32 + 1);
    let id = s.open(Path::new("/d/huge.pdf")).unwrap();
    assert_eq!(s.edit_pages(id, |e| e.rotate(0, 1, 1)), Err(SessionError::Edit(EditError::TooManyPages)));
    assert!(s.page_edits(id).is_none());
}
