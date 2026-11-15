//! Which pages a list's edits name for the page matcher, what's kept of them, and that it comes back from the journal as it went in.

use super::super::PageEdits;
use super::*;

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

/// Failure caught (B1): what was read of the pages goes in the journal (which an app from before then can't read), or never reaches the sidecar, or
/// reaches it twice; or a read of pages not named (an older wanting) is taken in. Found pages wait for the sidecar once, in the order read; the journal
/// gets nothing of them; once written they're no longer waiting, and only pages read after that wait next time.
#[test]
fn what_was_read_waits_for_the_sidecar_once_and_never_for_the_journal() {
    let mut edits = PageEdits::new(20);
    edits.journal_from_now();
    edits.rotate(9, 1, 1).unwrap();
    let ops_before = edits.journal_waiting().map(|(_, ops)| ops.to_vec()).unwrap();
    assert!(found(&mut edits, &[(9, known(9)), (10, known(10)), (2, known(2))], None));
    assert_eq!(
        edits.journal_waiting().map(|(_, ops)| ops.to_vec()).unwrap(),
        ops_before,
        "nothing for the journal"
    );
    assert_eq!(identities(&edits).unwrap().known(2), None, "page 2 was never named");
    assert!(!found(&mut edits, &[(9, known(1))], None), "a page read already stays as it was");
    let waiting = sidecar_waiting(&edits).unwrap();
    assert!(!waiting.whole);
    assert_eq!(waiting.unwritten, 2);
    assert_eq!(waiting.bytes, sidecar::entries(&[(9, known(9)), (10, known(10))], None));
    assert!(!sidecar_caught_up(&edits));
    // A page read while that write was on its way waits on after it.
    assert!(found(&mut edits, &[(8, known(8))], None));
    sidecar_written(&mut edits, &waiting, sidecar::Done::Written);
    let next = sidecar_waiting(&edits).unwrap();
    assert_eq!(next.bytes, sidecar::entries(&[(8, known(8))], None));
    sidecar_written(&mut edits, &next, sidecar::Done::Written);
    assert!(sidecar_waiting(&edits).is_none());
    assert!(sidecar_caught_up(&edits));
}

/// Failure caught: a sidecar on disk that isn't this list's (another version's, or a launch's that went) is appended to, or the list never writes
/// again; or the file's `/ID` pair is written more than once. Asked for whole, the next write holds every page known and the pair, and after it only
/// what's new.
#[test]
fn a_sidecar_asked_for_whole_is_written_whole_once() {
    let document = DocumentIds {
        first: [1; 16],
        second: [2; 16],
    };
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    found(&mut edits, &[(9, known(9))], Some(document));
    let first = sidecar_waiting(&edits).unwrap();
    assert!(first.document);
    sidecar_written(&mut edits, &first, sidecar::Done::WantsWhole);
    let whole = sidecar_waiting(&edits).unwrap();
    assert!(whole.whole);
    assert_eq!(whole.bytes, sidecar::entries(&[(9, known(9))], Some(document)));
    sidecar_written(&mut edits, &whole, sidecar::Done::Written);
    assert!(sidecar_waiting(&edits).is_none(), "the pair and the page, once");
}

/// Failure caught: slice 2's follow to a new version keeps what was read of the old one, or writes the new version's pages onto the old version's
/// sidecar. [`followed`] makes the list's pages the new version's, read, and asks for the sidecar whole; a page past the new file isn't kept.
#[test]
fn a_followed_list_rewrites_its_sidecar_whole_for_the_new_version() {
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    found(&mut edits, &[(9, known(9))], None);
    followed(&mut edits, &[(4, known(4)), (30, known(30))], None);
    let ids = identities(&edits).unwrap();
    assert_eq!(ids.pages(), &[(4, Some(known(4)))]);
    let waiting = sidecar_waiting(&edits).unwrap();
    assert!(waiting.whole);
    assert_eq!(waiting.bytes, sidecar::entries(&[(4, known(4))], None));
}

/// Failure caught (B1): a list restored from its journal doesn't get what its sidecar holds, so a restart loses what its pages are; or it writes it all
/// again; or a hostile sidecar's page past the file's end is named. Pages are named and known, past the end left out, and nothing waits to be written.
#[test]
fn a_restored_list_takes_in_its_sidecar_and_writes_nothing_again() {
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    restored(&mut edits, &[(9, known(9)), (15, known(15)), (25, known(25))], None, false);
    let ids = identities(&edits).unwrap();
    assert_eq!(ids.known(9), Some(known(9)));
    assert_eq!(ids.known(15), Some(known(15)), "named by the sidecar alone: a page of the journal's base");
    assert!(ids.pages().iter().all(|(page, _)| *page < 20));
    assert_eq!(wanted(&edits), vec![7, 8, 10, 11]);
    assert!(sidecar_caught_up(&edits));
}

/// Failure caught (rules 1 and 6): after a page had no identity, an edit names new pages of the same version and they're read (each up to its caps),
/// so an agent's edits keep the reader busy; or a save, a new version, stays closed. A give-up names nothing more until the save, after which edits
/// name pages again.
#[test]
fn a_give_up_holds_until_the_version_changes() {
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    found(&mut edits, &[(9, known(9))], None);
    give_up(&mut edits);
    assert_eq!(identities(&edits).unwrap().pages(), &[(9, Some(known(9)))]);
    edits.rotate(2, 1, 1).unwrap();
    assert!(wanted(&edits).is_empty(), "nothing named of this version");
    edits.rebase_on_saved(20, None, None).unwrap();
    edits.rotate(4, 1, 1).unwrap();
    assert!(wanted(&edits).contains(&4), "the saved file is another version");
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

/// Failure caught (for slice 2): that a list followed its file and the person wasn't told yet is lost at a restart, so they're never told; or telling
/// them doesn't reach the sidecar. Set or cleared, the sidecar is written whole with the flag in its header; restored, it says it again.
#[test]
fn whether_a_follow_was_told_goes_in_the_sidecars_header() {
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    found(&mut edits, &[(9, known(9))], None);
    let first = sidecar_waiting(&edits).unwrap();
    sidecar_written(&mut edits, &first, sidecar::Done::Written);
    set_followed_untold(&mut edits, true);
    assert!(followed_untold(&edits));
    let waiting = sidecar_waiting(&edits).expect("written again");
    assert!(waiting.whole);
    sidecar_written(&mut edits, &waiting, sidecar::Done::Written);
    set_followed_untold(&mut edits, true);
    assert!(sidecar_waiting(&edits).is_none(), "no change, no write");
    let mut back = PageEdits::new(20);
    restored(&mut back, &[(9, known(9))], None, true);
    assert!(followed_untold(&back));
    assert!(sidecar_caught_up(&back));
}

/// Failure caught (second review, F2): a page tried and found to have no identity is named again by the next edit and read again, or counts as the
/// list being past its cap (so slice 2 follows none of its pages); or a list closed by its budget with every page read says it's past its cap, or one
/// closed says nothing when an edit then names a page it can't read. Tried: named no more and not again, the list not past its cap; closed with all
/// read: not past it either, until an edit names a page it won't read.
#[test]
fn a_page_tried_is_itself_alone_and_a_closed_list_says_what_it_loses() {
    let mut edits = PageEdits::new(20);
    edits.rotate(9, 1, 1).unwrap();
    assert_eq!(named(&edits), vec![7, 8, 9, 10, 11]);
    assert!(tried(&mut edits, &[10]));
    assert_eq!(named(&edits), vec![7, 8, 9, 11]);
    assert!(!identities(&edits).unwrap().over_cap());
    edits.rotate(10, 1, 1).unwrap();
    assert!(!named(&edits).contains(&10), "10 isn't named again: {:?}", named(&edits));
    let all: Vec<(u32, Known)> = named(&edits).into_iter().map(|page| (page, known(page as u8))).collect();
    found(&mut edits, &all, None);
    give_up(&mut edits);
    assert!(!identities(&edits).unwrap().over_cap(), "every page named was read");
    assert_eq!(budget_left(&edits), 0);
    edits.rotate(10, 1, 1).unwrap();
    assert!(!identities(&edits).unwrap().over_cap(), "pages it has, or tried");
    edits.rotate(16, 1, 1).unwrap();
    assert!(identities(&edits).unwrap().over_cap(), "a page it won't read");
    assert!(wanted(&edits).is_empty());
}
