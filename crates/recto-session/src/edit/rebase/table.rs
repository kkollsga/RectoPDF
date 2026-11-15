//! **The page matcher's table** (slice 2's main test): each way another program can change a file, crossed with each kind of edit, says whether the
//! list follows its pages into the new version or is held whole, and where it follows, that every edit is on the page with the same content it was
//! made on (the owner's rule, 2026-11-13: never put on another page). No files: a file is a list of pages, each its content's number, its object
//! number and its frame, and a page's identity is made from those as `recto-page/1` would see them (equal content, equal digest).
//!
//! The base file has ten pages, content 0 to 9, objects 10 to 19; every edit is about page 4 (content 4), and a move puts page 4 after page 7.

use super::super::identity::{self, Known};
use super::super::marks::{FileKey, FileMark, MarkChange, MarkPage, MarkRect};
use super::super::matcher::{Found, ScanCaps, match_pages};
use super::super::{PageEdits, PageSlot};
use super::*;

/// A page of a made file: its content's number, its object number, its frame's tag, and whether it has an identity at all (a page that reached a cap
/// of the digest's has none).
#[derive(Clone, Copy, Debug)]
struct Page {
    content: u8,
    object: u32,
    frame: u32,
    identified: bool,
}

fn page(content: u8, object: u32) -> Page {
    Page {
        content,
        object,
        frame: 0,
        identified: true,
    }
}

fn known(page: Page) -> Known {
    Known {
        digest: [page.content; 16],
        object: (page.object, 0),
        frame: page.frame,
    }
}

/// The file the edits were made on.
fn base() -> Vec<Page> {
    (0..10).map(|at| page(at, 10 + u32::from(at))).collect()
}

/// The base's pages by content, with objects as another program that renumbers gives them.
fn renumbered(contents: &[u8]) -> Vec<Page> {
    contents.iter().enumerate().map(|(at, content)| page(*content, 500 + at as u32)).collect()
}

/// The base's pages by content, keeping the base's object numbers (a program that keeps them, as our own appends do); a page added gets a new one.
fn kept(contents: &[u8]) -> Vec<Page> {
    let mut seen = std::collections::HashSet::new();
    contents
        .iter()
        .enumerate()
        .map(|(at, content)| {
            let object = if seen.insert(*content) && *content < 10 {
                10 + u32::from(*content)
            } else {
                900 + at as u32
            };
            page(*content, object)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Turn,
    Delete,
    Move,
    Insert,
    Highlight,
    FileHighlightOut,
}

const KINDS: [Kind; 6] = [Kind::Turn, Kind::Delete, Kind::Move, Kind::Insert, Kind::Highlight, Kind::FileHighlightOut];

const RECT: MarkRect = MarkRect {
    x: 10.0,
    y: 10.0,
    width: 100.0,
    height: 12.0,
};

fn file_key() -> FileKey {
    FileKey {
        source: 0,
        page: 4,
        index: 0,
        rect: [10.0, 700.0, 110.0, 712.0],
    }
}

/// A list with one edit of this kind on page 4.
fn edited(kind: Kind) -> PageEdits {
    let mut edits = PageEdits::new(10);
    match kind {
        Kind::Turn => edits.rotate(4, 1, 1).unwrap(),
        Kind::Delete => edits.delete(4, 1).unwrap(),
        Kind::Move => edits.move_pages(4, 1, 7).unwrap(),
        Kind::Insert => edits.insert_pdf_noting(5, std::path::Path::new("/d/other.pdf"), 2, None, false).unwrap(),
        Kind::Highlight => edits
            .mark(MarkChange::Add {
                pages: vec![MarkPage {
                    source: 0,
                    page: 4,
                    rects: vec![RECT],
                }],
            })
            .unwrap(),
        Kind::FileHighlightOut => edits
            .mark(MarkChange::RemoveFile {
                files: vec![FileMark {
                    key: file_key(),
                    rects: vec![RECT].into_boxed_slice(),
                }],
                ids: Vec::new(),
            })
            .unwrap(),
    }
    edits
}

/// What's read of the pages a list names, from the file it was made on, as the page-identity thread reads them; a page `unread` isn't.
fn identify(edits: &mut PageEdits, file: &[Page], unread: &[u32]) {
    let pages: Vec<(u32, Known)> = identity::wanted(edits)
        .into_iter()
        .filter(|page| !unread.contains(page))
        .filter(|page| file[*page as usize].identified)
        .map(|page| (page, known(file[page as usize])))
        .collect();
    identity::found(edits, &pages, None);
}

/// The match and the rebase, as the session runs them, of a list onto `new`; the file's own highlight is on the same place of its page there unless
/// `annots` says what that page has.
fn follow(edits: &PageEdits, new: &[Page], caps: ScanCaps, annots: Option<Vec<(u32, Option<[f32; 4]>)>>) -> (Vec<(u32, Found)>, Outcome) {
    follow_as(edits, new, caps, annots, false)
}

/// [`follow`], saying whether the new version has the `/ID[0]` the edits' version had (a re-save keeps it; another document hasn't it).
fn follow_as(
    edits: &PageEdits,
    new: &[Page],
    caps: ScanCaps,
    annots: Option<Vec<(u32, Option<[f32; 4]>)>>,
    same_document: bool,
) -> (Vec<(u32, Found)>, Outcome) {
    let named = identity::identities(edits).map(|ids| ids.pages().to_vec()).unwrap_or_default();
    let wanted: Vec<u32> = named.iter().map(|(page, _)| *page).collect();
    let new_ids: Vec<(Option<Known>, u64)> = new.iter().map(|page| (page.identified.then(|| known(*page)), 1)).collect();
    let found = match_pages(&named, &wanted, &new_ids, caps, same_document);
    let on_page = annots.unwrap_or_else(|| vec![(file_key().index, Some(file_key().rect))]);
    let refound: Vec<(FileKey, Option<FileKey>)> = edits
        .marks()
        .hidden()
        .iter()
        .map(|key| {
            let at = found.iter().find(|(old, _)| *old == key.page).and_then(|(_, found)| found.page());
            (*key, at.and_then(|(new, _)| refind(*key, new, &on_page)))
        })
        .collect();
    let outcome = rebase(edits, &found, u32::try_from(new.len()).unwrap(), &refound);
    (found, outcome)
}

fn content_of(new: &[Page], slot: &PageSlot) -> Option<u8> {
    (slot.source == 0).then(|| new[slot.page as usize].content)
}

/// **The owner's rule, checked of a list that followed:** each edit is on a page with the content it was made on, and nothing else of the new version
/// is touched.
fn check_on_their_pages(kind: Kind, new: &[Page], onto: &Onto, row: &str) {
    let contents: Vec<Option<u8>> = onto.slots.iter().map(|slot| content_of(new, slot)).collect();
    let count = |content: u8, of: &[Option<u8>]| of.iter().filter(|at| **at == Some(content)).count();
    let in_new: Vec<Option<u8>> = new.iter().map(|page| Some(page.content)).collect();
    let turned: Vec<&PageSlot> = onto.slots.iter().filter(|slot| slot.quarter_turns != 0).collect();
    match kind {
        Kind::Turn => {
            assert_eq!(turned.len(), 1, "{row}: one page turned");
            assert_eq!(content_of(new, turned[0]), Some(4), "{row}: the turn is on page 4's content");
        }
        Kind::Delete => assert_eq!(count(4, &contents) + 1, count(4, &in_new), "{row}: page 4's content taken out once"),
        Kind::Move => {
            let at = contents
                .iter()
                .position(|content| *content == Some(7))
                .unwrap_or_else(|| panic!("{row}: page 7 is there"));
            assert_eq!(
                contents.get(at + 1).copied().flatten(),
                Some(4),
                "{row}: page 4 right after page 7, where the person put it"
            );
            assert_eq!(count(4, &contents), count(4, &in_new), "{row}: page 4 moved, not copied");
        }
        Kind::Insert => {
            let at = contents
                .iter()
                .position(|content| *content == Some(4))
                .unwrap_or_else(|| panic!("{row}: page 4 is there"));
            assert!(
                onto.slots[at + 1..at + 3].iter().all(|slot| slot.source == 1),
                "{row}: the added pages right after page 4"
            );
        }
        Kind::Highlight => {
            let pages: Vec<u32> = onto.marks.values().copied().collect();
            assert_eq!(pages.len(), 1, "{row}");
            assert_eq!(new[pages[0] as usize].content, 4, "{row}: the highlight is on page 4's content");
        }
        Kind::FileHighlightOut => {
            assert_eq!(onto.hidden.len(), 1, "{row}");
            assert_eq!(
                new[onto.hidden[0].page as usize].content, 4,
                "{row}: the file's highlight taken out of page 4's content"
            );
        }
    }
    if kind != Kind::Turn {
        assert!(turned.is_empty(), "{row}: nothing turned that wasn't");
    }
    // Every page of the new version is there but what the person deleted, once each, and every page the person added.
    let expected_own = new.len() - usize::from(kind == Kind::Delete);
    assert_eq!(
        onto.slots.iter().filter(|slot| slot.source == 0).count(),
        expected_own,
        "{row}: the new version's pages are there"
    );
    let mut own: Vec<u32> = onto.slots.iter().filter(|slot| slot.source == 0).map(|slot| slot.page).collect();
    own.sort_unstable();
    own.dedup();
    assert_eq!(own.len(), expected_own, "{row}: no page of the new version twice");
}

/// One row: what another program did, the new file, the caps, and whether each kind follows (in [`KINDS`]'s order).
struct Row {
    name: &'static str,
    new: Vec<Page>,
    caps: ScanCaps,
    follows: [bool; 6],
    /// Pages of the base the page-identity thread hadn't read when the file changed.
    unread: &'static [u32],
    /// What page 4 of the new version has for the file's highlights, if not the one taken out, unchanged.
    annots: Option<Vec<(u32, Option<[f32; 4]>)>>,
    /// The new version has the edits' version's `/ID[0]` (a re-save that keeps it); by default it hasn't, so a page found once needs a neighbour.
    same_document: bool,
}

const ALL: [bool; 6] = [true; 6];
const NONE: [bool; 6] = [false; 6];

fn row(name: &'static str, new: Vec<Page>, follows: [bool; 6]) -> Row {
    Row {
        name,
        new,
        caps: ScanCaps::STANDARD,
        follows,
        unread: &[],
        annots: None,
        same_document: false,
    }
}

/// A row whose new version keeps the edits' version's `/ID[0]`.
fn resaved(name: &'static str, new: Vec<Page>, follows: [bool; 6]) -> Row {
    Row {
        same_document: true,
        ..row(name, new, follows)
    }
}

fn rows() -> Vec<Row> {
    let ten: Vec<u8> = (0..10).collect();
    let mut cropped = renumbered(&ten);
    cropped[4].frame = 1;
    let mut unidentifiable_now = renumbered(&ten);
    unidentifiable_now[4].identified = false;
    let mut duplicated_far = ten.clone();
    duplicated_far.push(4);
    let mut cut_short = row(
        "a scan cut at its page cap: page 4 seen once before the cut isn't taken as the only one",
        renumbered(&duplicated_far),
        NONE,
    );
    cut_short.caps.pages = 10;
    let mut at_the_cap = row("a scan at exactly its page cap: complete, so it's found", renumbered(&ten), ALL);
    at_the_cap.caps.pages = 10;
    let mut bytes_cut = row("a scan cut at its byte cap plus one", renumbered(&ten), NONE);
    bytes_cut.caps.bytes = 9;
    let mut kept_past_cap = row("caps far too small, but every page found by its object number: no scan needed", kept(&ten), ALL);
    kept_past_cap.caps = ScanCaps { pages: 1, bytes: 1 };
    let mut unread = row("page 4 not read before the file changed", renumbered(&ten), NONE);
    unread.unread = &[4];
    // An insert after page 4 and a move of page 4 are placed by page 4 and page 7; an edit that names page 4 alone (a deletion) still needs it found.
    let mut base_unidentifiable = row(
        "page 4 reached a cap of the digest's when it was read, so it has no identity",
        renumbered(&ten),
        NONE,
    );
    base_unidentifiable.unread = &[4];
    let mut annot_moved = row(
        "the file's highlight on page 4 is at another place in /Annots, with its /Rect the only one like it",
        renumbered(&ten),
        ALL,
    );
    annot_moved.annots = Some(vec![(0, Some([1.0, 1.0, 2.0, 2.0])), (3, Some(file_key().rect))]);
    let mut annot_gone = row(
        "the file's highlight on page 4 is gone from it",
        renumbered(&ten),
        [true, true, true, true, true, false],
    );
    annot_gone.annots = Some(vec![(0, Some([1.0, 1.0, 2.0, 2.0]))]);
    let mut annot_twice = row(
        "two highlights on page 4 have its /Rect, at other places",
        renumbered(&ten),
        [true, true, true, true, true, false],
    );
    annot_twice.annots = Some(vec![(1, Some(file_key().rect)), (2, Some(file_key().rect))]);
    vec![
        row("the same file saved again, objects kept", kept(&ten), ALL),
        row("the same file saved again, every object renumbered", renumbered(&ten), ALL),
        row("a page inserted before page 4", renumbered(&[0, 1, 20, 2, 3, 4, 5, 6, 7, 8, 9]), ALL),
        row("a page inserted right after page 4", renumbered(&[0, 1, 2, 3, 4, 20, 5, 6, 7, 8, 9]), ALL),
        row("a page appended", renumbered(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 20]), ALL),
        row("a page none of the edits use deleted", renumbered(&[0, 1, 2, 3, 4, 5, 6, 7, 9]), ALL),
        row("pages none of the edits use reordered", renumbered(&[1, 0, 2, 3, 4, 5, 6, 7, 9, 8]), ALL),
        row(
            "page 4 duplicated far from its neighbours: the copy beside them is it",
            renumbered(&duplicated_far),
            ALL,
        ),
        row(
            "page 4 duplicated next to itself: both copies have one of its neighbours",
            renumbered(&[0, 1, 2, 3, 4, 4, 5, 6, 7, 8, 9]),
            [false, false, false, false, false, false],
        ),
        row(
            "a neighbour of page 4 deleted, page 4 unique: the neighbour tells nothing it's needed for",
            renumbered(&[0, 1, 2, 3, 4, 6, 7, 8, 9]),
            ALL,
        ),
        row(
            "page 4 duplicated, and both its neighbours, which told the copies apart, deleted",
            renumbered(&[0, 1, 2, 4, 6, 7, 8, 9, 4]),
            NONE,
        ),
        row("every page alike, renumbered: nothing tells them apart", renumbered(&[4; 10]), NONE),
        row("page 4's content changed", renumbered(&[0, 1, 2, 3, 40, 5, 6, 7, 8, 9]), NONE),
        row(
            "page 7's content changed: only the move, placed by page 7, can't follow",
            renumbered(&[0, 1, 2, 3, 4, 5, 6, 70, 8, 9]),
            [true, true, false, true, true, true],
        ),
        row("every page changed", renumbered(&[30, 31, 32, 33, 34, 35, 36, 37, 38, 39]), NONE),
        row(
            "truncated after page 6: only the move, placed by page 7, can't follow",
            renumbered(&[0, 1, 2, 3, 4, 5, 6]),
            [true, true, false, true, true, true],
        ),
        row("truncated before page 4", renumbered(&[0, 1, 2, 3]), NONE),
        row(
            "both reordered: the other program put page 7 first, away from both its neighbours, in another document: only the move can't follow",
            renumbered(&[7, 0, 1, 2, 3, 4, 5, 6, 8, 9]),
            [true, true, false, true, true, true],
        ),
        resaved(
            "both reordered in a re-save keeping /ID[0]: the other program put page 7 first; page 4 still goes right after page 7",
            renumbered(&[7, 0, 1, 2, 3, 4, 5, 6, 8, 9]),
            ALL,
        ),
        row(
            "page 4 moved to the end, away from both its neighbours, in another document: found once and nothing agrees, so held",
            renumbered(&[0, 1, 2, 3, 5, 6, 7, 8, 9, 4]),
            NONE,
        ),
        resaved(
            "the other program moved page 4 itself to the end, in a re-save keeping /ID[0]",
            renumbered(&[0, 1, 2, 3, 5, 6, 7, 8, 9, 4]),
            ALL,
        ),
        row(
            "another document, of which only page 4 (a stock page) is like ours: found once, nothing beside it agrees, so held",
            renumbered(&[30, 31, 32, 33, 4, 35, 36, 37, 38, 39]),
            NONE,
        ),
        row(
            "pages 3 to 5 reversed, in another document: page 4 is still between pages 3 and 5, the other way round, so it's found",
            renumbered(&[0, 1, 2, 5, 4, 3, 6, 7, 8, 9]),
            ALL,
        ),
        resaved(
            "a re-save keeping /ID[0], page 4 unique, both its neighbours gone: follows",
            renumbered(&[0, 1, 2, 4, 6, 7, 8, 9]),
            ALL,
        ),
        row(
            "page 4's crop changed: only a highlight, which is in its frame, can't follow",
            cropped,
            [true, true, true, true, false, true],
        ),
        row(
            "page 4 of the new version reached a digest cap: no page has its identity",
            unidentifiable_now,
            NONE,
        ),
        cut_short,
        at_the_cap,
        bytes_cut,
        kept_past_cap,
        unread,
        base_unidentifiable,
        annot_moved,
        annot_gone,
        annot_twice,
    ]
}

/// **Failure caught, row by row and edit by edit:** an edit put on another page of the new version than the one it was made on (a turn, a deletion, a
/// move's place, an insert's place, a highlight, the file's highlight taken out), a list followed where a page can't be told for sure, or one held where
/// every page is found. Each row says what another program did; held means the whole list stays as it was (all or nothing, for 0.1.1).
#[test]
fn every_edit_follows_its_page_or_the_list_is_held() {
    for row in rows() {
        for (kind, follows) in KINDS.iter().zip(row.follows) {
            let mut edits = edited(*kind);
            identify(&mut edits, &base(), row.unread);
            let (found, outcome) = follow_as(&edits, &row.new, row.caps, row.annots.clone(), row.same_document);
            let label = format!("{}, {kind:?}", row.name);
            assert_eq!(
                outcome.onto.is_some(),
                follows,
                "{label}: follows? verdicts {:?}, found {found:?}",
                outcome.edits
            );
            // The seam for slice 3: one verdict an edit, and held exactly when one of them is.
            assert_eq!(outcome.edits.iter().all(|edit| edit.held.is_none()), follows, "{label}: {:?}", outcome.edits);
            if let Some(onto) = &outcome.onto {
                check_on_their_pages(*kind, &row.new, onto, &label);
            }
        }
    }
}

/// Failure caught: two old pages that come to one new page both taken there (two edits on one page, one of them not its own).
#[test]
fn two_pages_claiming_one_are_neither_found() {
    // Pages 3 and 4 were alike; the new version, a re-save keeping /ID[0] (so a page found once would do), has one of them, with neither's
    // neighbours.
    let mut file = base();
    file[3].content = 4;
    let mut edits = PageEdits::new(10);
    edits.rotate(3, 2, 1).unwrap();
    identify(&mut edits, &file, &[]);
    let (found, outcome) = follow_as(&edits, &renumbered(&[4]), ScanCaps::STANDARD, None, true);
    assert!(outcome.onto.is_none());
    assert!(
        matches!(found.iter().find(|(old, _)| *old == 3), Some((_, Found::Claimed | Found::Ambiguous))),
        "{found:?}"
    );
}

/// Failure caught: an unsaved highlight on a page the person deleted holds the list, or is carried onto a page of the new version. It can be shown or
/// reached only by undoing the delete, which the end of the history at a followed change takes away (owner, 2026-11-15), and it was never saved: so
/// dropping it loses nothing a person can see or save, and the list follows.
#[test]
fn a_highlight_on_a_page_the_person_deleted_goes_with_the_history_and_loses_nothing_reachable() {
    let mut edits = edited(Kind::Highlight);
    edits.delete(4, 1).unwrap();
    identify(&mut edits, &base(), &[]);
    let (_, outcome) = follow(&edits, &renumbered(&[0, 1, 20, 2, 3, 4, 5, 6, 7, 8, 9]), ScanCaps::STANDARD, None);
    let onto = outcome.onto.expect("follows");
    assert!(onto.marks.is_empty(), "nothing carried onto the new version");
    apply(&mut edits, onto, None);
    assert_eq!(edits.marks().len(), 0);
    assert!(
        !edits.can_undo() && !edits.can_redo(),
        "the history ends, so the deleted page can't come back to show it"
    );
}

/// Failure caught: a list put onto the new version keeps an undo history about the old one (⌘Z would undo onto pages that aren't those, owner
/// 2026-11-15: "⌘Z doesn't go back past it"), loses the person's edits, or names pages of the old version for the next change. After [`apply`]: the
/// new pages with the turn on page 4's content, no undo or redo, still edited and the owner's, and the named pages are the new version's, read.
#[test]
fn a_list_put_onto_the_new_version_ends_its_history_there_and_names_the_new_pages() {
    let mut edits = edited(Kind::Turn);
    edits.move_pages(0, 1, 9).unwrap();
    edits.undo();
    assert!(edits.can_redo());
    identify(&mut edits, &base(), &[]);
    let new = renumbered(&[0, 1, 20, 2, 3, 4, 5, 6, 7, 8, 9]);
    let (_, outcome) = follow(&edits, &new, ScanCaps::STANDARD, None);
    let onto = outcome.onto.expect("follows");
    assert_eq!(onto.positions[4], Some(5), "the page being read at position 4 is at 5 now");
    apply(&mut edits, onto, None);
    assert_eq!(edits.file_pages(), 11);
    assert_eq!(edits.len(), 11);
    assert_eq!(edits.slots()[5].quarter_turns, 1);
    assert!(!edits.can_undo() && !edits.can_redo(), "the history ends at the change");
    assert!(edits.is_edited() && edits.holds_owner_edits(), "the person's edits are still here, and theirs");
    let ids = identity::identities(&edits).expect("named");
    let page_5 = ids.known(5).expect("page 5 of the new version is named and read");
    assert_eq!(page_5.digest, [4; 16]);
    assert_eq!(page_5.object, (505, 0), "what it is in the new version");
    assert!(
        ids.known(2).is_none() && ids.pages().iter().any(|(page, _)| *page == 2),
        "the page another program added beside them named, to be read"
    );
}

/// Failure caught: a page the person moved to the front, with a page the other program put first: the person's placement isn't kept relative to the
/// page it was next to (owner, 2026-11-15: "the person's placement wins, relative to the page theirs was next to").
#[test]
fn a_page_moved_to_the_front_stays_before_the_page_it_was_next_to() {
    let mut edits = PageEdits::new(10);
    edits.move_pages(6, 1, 0).unwrap();
    identify(&mut edits, &base(), &[]);
    let new = renumbered(&[20, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    let (_, outcome) = follow(&edits, &new, ScanCaps::STANDARD, None);
    let onto = outcome.onto.expect("follows");
    let contents: Vec<u8> = onto.slots.iter().map(|slot| new[slot.page as usize].content).collect();
    assert_eq!(contents, vec![20, 6, 0, 1, 2, 3, 4, 5, 7, 8, 9]);
}

/// Failure caught: a list whose pages are all added from other PDFs (every page of the file deleted) placed somewhere in the new version with nothing
/// to place it by. It's held.
#[test]
fn a_list_with_no_page_of_its_own_left_is_held() {
    let mut edits = PageEdits::new(2);
    edits.insert_pdf_noting(2, std::path::Path::new("/d/other.pdf"), 1, None, false).unwrap();
    edits.delete(0, 2).unwrap();
    identify(&mut edits, &base()[..2], &[]);
    let (_, outcome) = follow(&edits, &renumbered(&[0, 1, 5]), ScanCaps::STANDARD, None);
    assert!(outcome.onto.is_none());
    assert!(outcome.edits.iter().any(|edit| edit.held == Some(Held::NoAnchor)), "{:?}", outcome.edits);
}

/// Failure caught: the person's own pages still in order are taken for moved (or the other way round), so an untouched page is placed where the
/// person never put it. The longest run in order is kept, the same each time.
#[test]
fn the_pages_in_order_are_the_longest_run() {
    assert_eq!(in_order(&[0, 1, 2, 3]), vec![true; 4]);
    assert_eq!(in_order(&[1, 2, 3, 0]), vec![true, true, true, false]);
    assert_eq!(in_order(&[3, 0, 1, 2]), vec![false, true, true, true]);
    assert_eq!(in_order(&[0, 2, 1, 3]).iter().filter(|kept| **kept).count(), 3);
    assert!(in_order(&[]).is_empty());
}

/// Failure caught: more alike pages named than [`super::super::matcher::MAX_ALIKE`] are each compared with every copy (a hostile file of blank pages
/// costs the scan its square), or one of them is matched. Past the cap none is looked for, and the list is held.
#[test]
fn more_alike_pages_than_the_cap_are_none_of_them_looked_for() {
    let alike = super::super::matcher::MAX_ALIKE + 1;
    let named: Vec<(u32, Option<Known>)> = (0..alike as u32)
        .map(|page| (page, Some(known(Page { object: page, ..page_of_4() }))))
        .collect();
    let new: Vec<(Option<Known>, u64)> = (0..alike as u32)
        .map(|at| {
            (
                Some(known(Page {
                    object: 1000 + at,
                    ..page_of_4()
                })),
                1,
            )
        })
        .collect();
    let found = match_pages(&named, &[0], &new, ScanCaps::STANDARD, false);
    assert!(found.iter().all(|(_, found)| *found == Found::NotFound), "{found:?}");
}

fn page_of_4() -> Page {
    page(4, 0)
}

/// **Failure caught (review F1):** a place in the list (the reading position, a trail's stop) far from the edits mapped to the nearest edited page:
/// reading page 150 with a turn on page 4 taken to page 6. Written by hand from the rule: its own place if known; else the nearest known before it plus
/// the distance, kept short of the next known place; with none before, the nearest after it less the distance; within the list.
#[test]
fn a_place_keeps_its_page_by_the_nearest_known_and_the_distance_from_it() {
    // Positions 3 to 5 known (a turn on page 4 and its neighbours), one page inserted before them by another program; the list is 201 long now.
    let mut positions = vec![None; 200];
    positions[3] = Some(4);
    positions[4] = Some(5);
    positions[5] = Some(6);
    assert_eq!(moved_position(&positions, 150, 201), 151, "far past the edits: shifted by the one inserted before it");
    assert_eq!(moved_position(&positions, 4, 201), 5, "known: its own place");
    assert_eq!(moved_position(&positions, 1, 201), 2, "before every known place: the nearest after it less the distance");
    assert_eq!(moved_position(&positions, 199, 150), 149, "within the list");
    assert_eq!(moved_position(&positions, 900, 201), 200, "past the old list: its last place, kept in the new one");
    // Kept short of the next known place: pages taken out between two known ones.
    let mut squeezed = vec![None; 10];
    squeezed[2] = Some(2);
    squeezed[8] = Some(4);
    assert_eq!(moved_position(&squeezed, 6, 9), 3, "short of the next known place, not past it");
    assert_eq!(moved_position(&[None, None], 1, 2), 1, "nothing known: where it was");
}
