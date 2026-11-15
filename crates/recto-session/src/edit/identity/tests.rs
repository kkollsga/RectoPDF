//! Which pages a list's edits name for the page matcher, what's kept of them, and that it comes back from the journal as it went in.

use super::super::PageEdits;
use super::*;
use crate::record::{CURRENT_VERSION, JOURNAL_VERSION, JournalHeader, journal_start, parse_journal};

std::thread_local! {
    /// The cap for this thread's test, when it sets it small ([`super::cap`]).
    pub(super) static CAP: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn named(edits: &PageEdits) -> Vec<u32> {
    identities(edits)
        .map(|ids| ids.pages().iter().map(|(page, _)| *page).collect())
        .unwrap_or_default()
}

fn known(seed: u8) -> Known {
    Known {
        digest: [seed; 16],
        object: (u32::from(seed) + 3, 0),
        frame: u32::from(seed),
    }
}

/// Failure caught: a step's page isn't named, so its identity is never read and the edit can't follow it when the file changes (set aside for nothing);
/// or the pages it sits between aren't, so a move or an insert can't be placed again, nor a blank page told from another. A turn names its page and the
/// pages either side of it in the document and in the file; nothing is named for a list with no step.
#[test]
fn a_turn_names_its_page_and_the_pages_beside_it() {
    let mut edits = PageEdits::new(20);
    assert!(identities(&edits).is_none(), "nothing for a list with no step");
    edits.rotate(9, 1, 1).unwrap();
    assert_eq!(named(&edits), vec![7, 8, 9, 10, 11]);
    assert_eq!(wanted(&edits), vec![7, 8, 9, 10, 11], "none read yet");
}

/// Failure caught: a move names only the pages it moved, so the place it put them (between two pages) can't be found in another version, or the pages it
/// left behind are never named. Moving page 15 to the front names 15, its old neighbours and the new first pages, each with its file neighbours.
#[test]
fn a_move_names_where_it_came_from_and_where_it_went() {
    let mut edits = PageEdits::new(20);
    edits.move_pages(15, 1, 0).unwrap();
    for page in [13, 14, 15, 16, 17, 0, 1] {
        assert!(named(&edits).contains(&page), "page {page} in {:?}", named(&edits));
    }
}

/// Failure caught: a deletion's pages, or an inserted file's place, aren't named. Deleting 5 to 7 names them and the pages beside; adding another PDF's
/// pages after page 12 names the pages either side of where they went (12 and 13), not the added file's pages, which have their own file.
#[test]
fn a_delete_and_an_insert_name_their_pages_and_their_place() {
    let mut edits = PageEdits::new(20);
    edits.delete(5, 3).unwrap();
    for page in 3..=9 {
        assert!(named(&edits).contains(&page), "page {page} in {:?}", named(&edits));
    }
    let mut edits = PageEdits::new(20);
    edits.insert_pdf_noting(13, std::path::Path::new("/d/other.pdf"), 4, None, false).unwrap();
    assert_eq!(named(&edits), vec![11, 12, 13, 14]);
}

/// Failure caught: a highlight's page isn't named, so a highlight made on a page can't follow it.
#[test]
fn a_highlight_names_its_page() {
    let mut edits = PageEdits::new(20);
    edits
        .mark(crate::edit::marks::MarkChange::Add {
            pages: vec![crate::edit::marks::MarkPage {
                source: 0,
                page: 4,
                rects: vec![crate::edit::marks::MarkRect {
                    x: 1.0,
                    y: 1.0,
                    width: 10.0,
                    height: 10.0,
                }],
            }],
        })
        .unwrap();
    assert_eq!(named(&edits), vec![3, 4, 5]);
}

/// Failure caught: a list names pages without end (a turn of 200,000 pages, or an agent's long run of steps), which costs memory per edited tab beyond
/// any bound; or the cap drops pages named before it. With the cap set small: at the cap plus one the newest page isn't named, the list says it's over,
/// and what was named first is kept.
#[test]
fn the_pages_named_are_capped_and_the_first_kept() {
    CAP.with(|cap| cap.set(Some(5)));
    let mut edits = PageEdits::new(100);
    edits.rotate(1, 1, 1).unwrap();
    assert_eq!(named(&edits), vec![0, 1, 2, 3]);
    assert!(!identities(&edits).unwrap().over_cap());
    edits.rotate(50, 1, 1).unwrap();
    let after = named(&edits);
    assert_eq!(after.len(), 5, "{after:?}");
    assert!(after.starts_with(&[0, 1, 2, 3]), "{after:?}");
    assert!(identities(&edits).unwrap().over_cap());
    // A turn of the whole document names no more.
    edits.rotate(0, 100, 1).unwrap();
    assert_eq!(named(&edits).len(), 5);
    CAP.with(|cap| cap.set(None));
}

/// Failure caught: what was read of the pages doesn't reach the journal, or doesn't come back from it, so after a restart the edits can't follow their
/// pages; or a read of pages not named (an older wanting) is taken in. Found pages go in once, are noted for the journal, and come back by a replay of
/// its lines and from its base.
#[test]
fn what_was_read_comes_back_from_the_journal_and_its_base() {
    let mut edits = PageEdits::new(20);
    edits.journal_from_now();
    edits.rotate(9, 1, 1).unwrap();
    assert!(found(&mut edits, &[(9, known(9)), (10, known(10)), (2, known(2))], None));
    assert_eq!(identities(&edits).unwrap().known(9), Some(known(9)));
    assert_eq!(identities(&edits).unwrap().known(2), None, "page 2 was never named");
    assert_eq!(wanted(&edits), vec![7, 8, 11]);
    assert!(!found(&mut edits, &[(9, known(1))], None), "a page read already stays as it was");
    let ops = edits.journal_waiting().map(|(_, ops)| ops.to_vec()).unwrap();
    assert!(ops_have_identities(&ops));
    let (replayed, applied) = PageEdits::replay(20, None, &ops).unwrap();
    assert_eq!(applied, ops.len());
    assert_eq!(identities(&replayed), identities(&edits));
    let back = PageEdits::from_record(&edits.to_record()).unwrap();
    assert_eq!(identities(&back), identities(&edits));
}

/// Failure caught (rule 6): a hostile journal's identities are believed: pages past the file's end, out of order or repeated, or a digest that isn't
/// one. A base with any of that has none of it taken; a line's bad page is left unread; the edits themselves come back either way.
#[test]
fn a_hostile_journals_identities_are_left_out_and_the_edits_kept() {
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    found(&mut edits, &[(9, known(9))], None);
    let mut record = edits.to_record();
    for bad in [
        |ids: &mut IdentitiesRecord| ids.pages[0].page = 99,
        |ids: &mut IdentitiesRecord| ids.pages.swap(0, 1),
        |ids: &mut IdentitiesRecord| ids.pages[1].page = ids.pages[0].page,
    ] {
        let mut forged = record.clone();
        bad(forged.identities.as_mut().unwrap());
        let back = PageEdits::from_record(&forged).unwrap();
        assert!(identities(&back).is_none());
        assert_eq!(back.slots(), edits.slots(), "the edits are kept");
    }
    let ids = record.identities.as_mut().unwrap();
    let at = ids.pages.iter().position(|page| page.page == 9).unwrap();
    ids.pages[at].digest = Some("not hex at all, and not 32 long".into());
    let back = PageEdits::from_record(&record).unwrap();
    assert_eq!(identities(&back).unwrap().known(9), None, "a digest that isn't one is unread");
}

/// Failure caught: after a Save the list keeps naming pages by the file as it was, whose numbers say nothing of the saved one, so a later change would
/// be matched against the wrong pages; or it forgets them, so an edit made while the save was landing can't follow its page; or it keeps what was read
/// of them, though the save wrote a turn into a page's frame. Each page named goes to its place in the saved file, unread; a page the save took out goes.
#[test]
fn a_save_moves_the_named_pages_to_the_saved_file() {
    let mut edits = PageEdits::new(20);
    edits.delete(0, 2).unwrap();
    edits.rotate(9, 1, 1).unwrap();
    // Pages 0 and 1 are gone; file page 11 is at position 9.
    assert!(named(&edits).contains(&11));
    found(&mut edits, &[(11, known(11))], None);
    edits.rebase_on_saved(18, None, None).unwrap();
    let after = named(&edits);
    assert!(after.contains(&9), "file page 11 is page 9 of the saved file: {after:?}");
    assert!(!after.contains(&19) && after.iter().all(|page| *page < 18), "{after:?}");
    assert_eq!(identities(&edits).unwrap().known(9), None, "to be read again in the saved file");
}

/// Failure caught: a journal's base with what was read of pages locks an app from before out of the person's other edits, though that app reads past a
/// field it doesn't know and loses only the identities: the base says the version it did, and this app reads the identities back from it. (A line of
/// them, which that app can't read past, makes the journal say [`JOURNAL_VERSION`]: `recto-control`'s record store, which appends it, holds that.)
#[test]
fn a_journals_base_with_identities_is_read_by_an_app_from_before() {
    let header = JournalHeader::new(1, std::path::Path::new("/d/a.pdf"), None, 20, 0);
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    found(&mut edits, &[(9, known(9))], None);
    let journal = journal_start(&header, Some(&edits.to_record()));
    let old = parse_journal(&journal, CURRENT_VERSION).expect("an app from before reads it");
    assert!(old.base.is_some());
    let parsed = parse_journal(&journal, JOURNAL_VERSION).unwrap();
    let (back, _) = PageEdits::replay(20, parsed.base.as_ref(), &parsed.ops).unwrap();
    assert_eq!(identities(&back).unwrap().known(9), Some(known(9)));
}
