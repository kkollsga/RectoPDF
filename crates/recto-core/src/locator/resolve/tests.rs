//! The resolver's tests, on small PDFs made by `recto_testpdf`: what a reference finds after another program reordered, removed or duplicated
//! pages, what a cap does to an answer, and what a target becomes on a page found. Each test names the failure it catches.

use super::*;
use crate::locator::{CharacterRange, PageRef, Spot};
use crate::page_id::{self, Caps, open_bytes_for_identity};
use recto_testpdf::Page;
use std::sync::Arc;

fn letter(name: &str) -> Page {
    Page::new(format!("BT /F1 12 Tf 72 700 Td ({name}) Tj ET"))
}

fn pdf(names: &[&str]) -> Vec<u8> {
    recto_testpdf::pages(&names.iter().map(|name| letter(name)).collect::<Vec<_>>())
}

fn open(bytes: &[u8]) -> (IndexedReader, PageMap) {
    let reader = open_bytes_for_identity(Arc::from(bytes)).expect("it opens");
    let map = reader.page_map().expect("its pages");
    (reader, map)
}

const STANDARD: ScanCaps = ScanCaps::STANDARD;

fn reference_of(names: &[&str], page: u32) -> Locator {
    let (reader, map) = open(&pdf(names));
    reference(&reader, &map, page, &STANDARD).expect("a reference")
}

fn resolve_in(locator: &Locator, bytes: &[u8], caps: &ScanCaps) -> Resolution {
    let (reader, map) = open(bytes);
    resolve(locator, &reader, &map, caps)
}

fn with_target(locator: &Locator, target: Target) -> Locator {
    Locator::new(locator.document(), locator.page(), target)
}

/// Failure: a page another program moved is called gone, or is taken for the page now at its old number.
#[test]
fn a_page_reordered_by_another_program_is_moved_to_its_new_place() {
    let locator = reference_of(&["A", "B", "C"], 2);
    let now = resolve_in(&locator, &pdf(&["C", "A", "B"]), &STANDARD);
    assert_eq!(now.status, Status::Moved { page: 0 });
    assert_eq!(now.looked, Looked::All);
    assert_eq!(now.frame_same, Some(true));
}

/// Failure: a page is called gone after reading only some of the file, or while a read page's neighbour is mistaken for it.
#[test]
fn a_page_removed_is_gone_only_after_every_page_was_read() {
    let locator = reference_of(&["A", "B", "C"], 2);
    let now = resolve_in(&locator, &pdf(&["A", "B", "D", "E"]), &STANDARD);
    assert_eq!(now.status, Status::Gone);
    assert_eq!(now.looked, Looked::All);
    assert_eq!(now.pages_read, 4, "every page, each once");
    assert_eq!((now.frame_same, now.target), (None, None));
}

/// Failure: of two identical pages the nearer one to the hint is offered as the answer.
#[test]
fn two_identical_candidates_are_ambiguous_never_the_nearest() {
    let locator = reference_of(&["A", "B", "A"], 0);
    let now = resolve_in(&locator, &pdf(&["B", "A", "A"]), &STANDARD);
    assert_eq!(
        now.status,
        Status::Ambiguous {
            pages: vec![1, 2],
            complete: true
        }
    );
    assert_eq!(now.target, None, "nothing is placed on a page that wasn't chosen");
}

/// Failure: more matches than the list holds are dropped without saying so, or the list grows past its cap (ten pages match; eight are listed).
#[test]
fn candidates_are_capped_and_the_answer_says_the_list_is_not_whole() {
    let locator = reference_of(&["A", "B"], 0);
    let now = resolve_in(&locator, &pdf(&["B", "A", "A", "A", "A", "A", "A", "A", "A", "A", "A"]), &STANDARD);
    let Status::Ambiguous { pages, complete } = now.status else {
        panic!("{:?}", now.status)
    };
    assert_eq!(pages, (1..=MAX_CANDIDATES as u32).collect::<Vec<_>>());
    assert!(!complete);
}

/// Failure: an unchanged hinted page costs a scan of the file, or is answered from the wrong page.
#[test]
fn an_unchanged_hint_is_found_without_a_scan() {
    let bytes = pdf(&["A", "B", "C", "D", "E"]);
    let locator = reference_of(&["A", "B", "C", "D", "E"], 3);
    let now = resolve_in(&locator, &bytes, &STANDARD);
    assert_eq!(now.status, Status::Found { page: 3 });
    assert_eq!(now.looked, Looked::Hint);
    assert_eq!(now.document, DocumentMatch::Same);
    assert_eq!(now.pages_read, 2, "the first page for the document tag, and the hinted page");
}

/// Failure: the hinted page, read for the hint, is read a second time by the scan when it is past the first [`TAG_PAGES`] pages, so a 20-page file
/// costs 21 reads.
#[test]
fn a_hint_past_the_tag_pages_is_read_once_by_the_scan() {
    let names: Vec<String> = (0..20).map(|i| format!("P{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (reader, map) = open(&pdf(&refs));
    let locator = reference(&reader, &map, 18, &STANDARD).expect("a reference");
    let mut changed = refs.clone();
    changed.swap(18, 5);
    let now = resolve_in(&locator, &pdf(&changed), &STANDARD);
    assert_eq!(now.status, Status::Moved { page: 5 });
    assert_eq!(now.pages_read, 20, "every page, each once, the hinted one included");
}

/// Failure: a scan keeps reading after a page with no identity made the answer `Incomplete` with a candidate already held, spending reads that can
/// no longer change the status.
#[test]
fn a_scan_stops_once_the_answer_can_only_be_incomplete_and_holds_a_candidate() {
    let locator = reference_of(&["A", "B", "C"], 0);
    let mut big = letter("Z");
    big.content.extend(std::iter::repeat_n(b' ', 400));
    let bytes = recto_testpdf::pages(&[letter("X"), letter("A"), big, letter("D"), letter("E"), letter("F")]);
    let mut caps = STANDARD;
    caps.page.content_bytes = 200;
    let now = resolve_in(&locator, &bytes, &caps);
    assert_eq!(now.status, Status::Incomplete { pages: vec![1] });
    assert_eq!(now.looked, Looked::Part);
    assert_eq!(
        now.pages_read, 3,
        "X, the candidate A, and the page without identity; the last three are not read"
    );
}

/// Failure: pages that reached a cap (no identity) count as "no match", and a page is called gone that was never read.
#[test]
fn a_page_with_no_identity_makes_a_scan_incomplete_never_gone() {
    let locator = reference_of(&["A", "B", "C"], 2);
    let mut caps = STANDARD;
    caps.page.content_streams = 0;
    let now = resolve_in(&locator, &pdf(&["D", "E", "F"]), &caps);
    assert_eq!(now.status, Status::Incomplete { pages: vec![] });
    assert_eq!(now.looked, Looked::Part);
    assert_eq!(now.document, DocumentMatch::Unknown);
}

/// Failure: a limit on pages read truncates an answer to "gone", or is off by one at its limit.
#[test]
fn the_page_cap_at_its_limit_completes_and_one_less_is_incomplete() {
    let locator = reference_of(&["A", "B", "C"], 2);
    let now = pdf(&["D", "E", "F"]);
    let at = ScanCaps { pages: 3, ..STANDARD };
    assert_eq!(resolve_in(&locator, &now, &at).status, Status::Gone);
    let under = ScanCaps { pages: 2, ..STANDARD };
    let short = resolve_in(&locator, &now, &under);
    assert_eq!(short.status, Status::Incomplete { pages: vec![] });
    assert_eq!((short.looked, short.pages_read), (Looked::Part, 2));
    let moved = pdf(&["C", "A", "B"]);
    assert_eq!(resolve_in(&locator, &moved, &at).status, Status::Moved { page: 0 });
    let found_before_the_cap = resolve_in(&locator, &moved, &under);
    assert_eq!(
        found_before_the_cap.status,
        Status::Incomplete { pages: vec![0] },
        "one candidate, never called moved"
    );
}

/// Failure: a byte budget is ignored, or a scan it stopped says "gone".
#[test]
fn a_byte_cap_of_one_stops_after_one_page_and_says_incomplete() {
    let locator = reference_of(&["A", "B", "C"], 2);
    let caps = ScanCaps { bytes: 1, ..STANDARD };
    let now = resolve_in(&locator, &pdf(&["D", "E", "F"]), &caps);
    assert_eq!(now.status, Status::Incomplete { pages: vec![] });
    assert_eq!(now.pages_read, 1);
    assert!(now.bytes_read >= 1);
}

/// Failure: the byte cap is off by one at its limit. The cap is checked before each read: with C the cost of the first two pages, a cap of exactly C
/// stops after two (incomplete, the third unread), and a cap of C + 1 reads the third as well (all three, `Gone`).
#[test]
fn a_byte_cap_equal_to_what_was_read_stops_and_one_more_byte_reads_on() {
    let locator = reference_of(&["A", "B", "C"], 2);
    let now = pdf(&["D", "E", "F"]);
    let all = resolve_in(&locator, &now, &ScanCaps { bytes: u64::MAX, ..STANDARD });
    assert_eq!((all.status, all.pages_read), (Status::Gone, 3));
    let (reader, map) = open(&now);
    let before_last: u64 = (0..2)
        .map(|page| page_id::read_page(&reader, map.get(page).expect("page"), &STANDARD.page).bytes)
        .sum();
    let at = resolve_in(
        &locator,
        &now,
        &ScanCaps {
            bytes: before_last,
            ..STANDARD
        },
    );
    assert_eq!(at.status, Status::Incomplete { pages: vec![] });
    assert_eq!((at.looked, at.pages_read), (Looked::Part, 2));
    let over = resolve_in(
        &locator,
        &now,
        &ScanCaps {
            bytes: before_last + 1,
            ..STANDARD
        },
    );
    assert_eq!((over.status, over.looked, over.pages_read), (Status::Gone, Looked::All, 3));
}

/// Failure: someone makes a scan charge a shared image once, so the byte cap stops bounding the work. The charge matches the work done: a shared
/// image is decoded and hashed on each page that draws it, so it is charged on each. One page draws an image and is scanned alone; two pages draw it and are scanned together: the two cost at least
/// twice the one, and the one cost more than the image.
#[test]
fn a_scan_charges_an_image_each_page_draws_it() {
    let side = 64usize;
    let drawing = |names: &[&str]| {
        let image = recto_testpdf::Object::Stream {
            dict: format!("/Type /XObject /Subtype /Image /Width {side} /Height {side} /ColorSpace /DeviceGray /BitsPerComponent 8"),
            data: (0..side * side).map(|i| (i % 251) as u8).collect(),
        };
        let pages: Vec<Page> = names
            .iter()
            .map(|name| Page::new(format!("q 100 0 0 100 72 500 cm /Im1 Do Q BT /F1 12 Tf 72 700 Td ({name}) Tj ET")))
            .collect();
        let number = recto_testpdf::first_extra_object(pages.len());
        recto_testpdf::pages_with(&pages, &format!("/XObject << /Im1 {number} 0 R >>"), vec![image])
    };
    let locator = reference_of(&["X", "Y"], 0);
    let alone = resolve_in(&locator, &drawing(&["A"]), &STANDARD);
    let together = resolve_in(&locator, &drawing(&["A", "B"]), &STANDARD);
    assert_eq!((alone.status, together.status), (Status::Gone, Status::Gone));
    assert!(
        alone.bytes_read > (side * side) as u64,
        "a page's charge includes the image it draws: {}",
        alone.bytes_read
    );
    assert!(
        together.bytes_read >= 2 * alone.bytes_read,
        "the second page was charged the image again: {} for two pages, {} for one",
        together.bytes_read,
        alone.bytes_read
    );
}

/// Failure: a hint past the end of the file indexes out of the page map, or hides a page that is there.
#[test]
fn a_hint_past_the_end_never_indexes_out() {
    let (reader, map) = open(&pdf(&["A", "B", "C"]));
    let identity = page_id::identify_page(&reader, map.get(1).unwrap(), &Caps::STANDARD).unwrap();
    let page = PageRef::new(MAX_PAGE, &identity).unwrap();
    let locator = Locator::new(DocumentTags::new([0; 8], None), page, Target::Page);
    let now = resolve(&locator, &reader, &map, &STANDARD);
    assert_eq!(now.status, Status::Moved { page: 1 });
}

/// Failure: the same drawing under a changed crop silently places a rectangle on a different frame.
#[test]
fn a_changed_crop_is_found_with_frame_changed_and_its_spot_unplaced() {
    let locator = reference_of(&["A", "B", "C"], 1);
    let spot = with_target(&locator, Target::Spot(Spot::area(72, 100, 100, 50).unwrap()));
    let mut cropped = letter("B");
    cropped.page_extra = "/CropBox [0 0 300 400]".into();
    let bytes = recto_testpdf::pages(&[letter("A"), cropped, letter("C")]);
    let now = resolve_in(&spot, &bytes, &STANDARD);
    assert_eq!(now.status, Status::Found { page: 1 });
    assert_eq!(now.frame_same, Some(false));
    assert_eq!(now.target, Some(Placed::SpotFrameChanged));
    assert_eq!(
        resolve_in(&locator, &bytes, &STANDARD).target,
        Some(Placed::Page),
        "the page itself is still found"
    );
}

/// Failure: a spot is placed outside its page, or on a page of the same frame it isn't drawn for.
#[test]
fn a_spot_is_placed_in_shown_points_only_on_the_same_frame() {
    let locator = reference_of(&["A", "B"], 0);
    let bytes = pdf(&["A", "B"]);
    let inside = with_target(&locator, Target::Spot(Spot::area(72, 350, 200, 90).unwrap()));
    assert_eq!(resolve_in(&inside, &bytes, &STANDARD).target, Some(Placed::Spot([72.0, 350.0, 272.0, 440.0])));
    let point = with_target(&locator, Target::Spot(Spot::point(10, 20).unwrap()));
    assert_eq!(resolve_in(&point, &bytes, &STANDARD).target, Some(Placed::Spot([10.0, 20.0, 10.0, 20.0])));
    let outside = with_target(&locator, Target::Spot(Spot::point(700, 100).unwrap()));
    assert_eq!(resolve_in(&outside, &bytes, &STANDARD).target, Some(Placed::SpotOutsidePage));
}

/// Failure: a character range is reported as checked against the page's text, which nothing here reads.
#[test]
fn a_character_target_is_never_called_checked() {
    let locator = reference_of(&["A", "B"], 1);
    let range = with_target(&locator, Target::Characters(CharacterRange::new(3, 9, [7; 8]).unwrap()));
    let now = resolve_in(&range, &pdf(&["A", "B"]), &STANDARD);
    assert_eq!(now.status, Status::Found { page: 1 });
    assert_eq!(now.target, Some(Placed::CharactersNotChecked { start: 3, end: 9 }));
}

/// Failure: a reference carries a second hash of the page, so it never matches what the journal and the matcher keep.
#[test]
fn the_reference_carries_the_shared_page_identity() {
    let (reader, map) = open(&pdf(&["A", "B", "C"]));
    let identity = page_id::identify_page(&reader, map.get(2).unwrap(), &Caps::STANDARD).unwrap();
    let locator = reference(&reader, &map, 2, &STANDARD).unwrap();
    assert_eq!(locator.page().number(), 3);
    assert_eq!(locator.page().page_check(), identity.digest[..16]);
    assert_eq!(locator.page().frame_tag(), identity.frame);
    assert_eq!(locator.target(), Target::Page);
    assert_eq!(reference(&reader, &map, 3, &STANDARD), Err(Unreferenceable::NoSuchPage));
    assert_eq!(reference(&reader, &map, MAX_PAGE, &STANDARD), Err(Unreferenceable::NoSuchPage));
    let mut no_streams = STANDARD;
    no_streams.page.content_streams = 0;
    assert_eq!(reference(&reader, &map, 0, &no_streams), Err(Unreferenceable::NoIdentity));
}

/// Failure: a reference to the same page differs between opens of an unchanged file, so a saved one can never be found again.
#[test]
fn a_reference_is_the_same_on_every_read_of_an_unchanged_file() {
    let first = reference_of(&["A", "B", "C"], 1);
    let second = reference_of(&["A", "B", "C"], 1);
    assert_eq!(first, second);
    assert_eq!(crate::locator::encode(&first), crate::locator::encode(&second));
}

fn with_id(bytes: &[u8], first: &str, second: &str) -> Vec<u8> {
    let marker = b"/Root 1 0 R";
    let at = bytes.windows(marker.len()).position(|window| window == marker).expect("a trailer") + marker.len();
    let mut out = bytes[..at].to_vec();
    out.extend(format!(" /ID [<{first}> <{second}>]").as_bytes());
    out.extend(&bytes[at..]);
    out
}

/// Failure: the `/ID` tag is a second hash of the file's `/ID`, or it is left out, so a re-saved file's pages can't be told from another document's.
#[test]
fn the_id_tag_is_the_shared_document_id_prefix() {
    let bytes = with_id(&pdf(&["A", "B"]), "0102030405060708090a0b0c0d0e0f10", "1112131415161718191a1b1c1d1e1f20");
    let (reader, map) = open(&bytes);
    let ids = page_id::document_ids(&reader).expect("two ids");
    let tags = document_tags(&reader, &map, &STANDARD).expect("tags");
    assert_eq!(tags.id(), Some(ids.first[..8].try_into().unwrap()));
    let locator = reference(&reader, &map, 1, &STANDARD).unwrap();
    assert_eq!(locator.document(), tags);
    assert_eq!(document_tags(&open(&pdf(&["A"])).0, &open(&pdf(&["A"])).1, &STANDARD).unwrap().id(), None);
}

/// Failure: the document tags decide the status: another document's pages are refused though they match, or the same document's page is called found
/// though it is gone.
#[test]
fn another_document_says_different_and_pages_still_decide() {
    let locator = reference_of(&["A", "B", "C"], 1);
    let other = resolve_in(&locator, &pdf(&["X", "B", "Y"]), &STANDARD);
    assert_eq!(other.document, DocumentMatch::Different);
    assert_eq!(other.status, Status::Found { page: 1 });

    let ids = ("0102030405060708090a0b0c0d0e0f10", "1112131415161718191a1b1c1d1e1f20");
    let (reader, map) = open(&with_id(&pdf(&["A", "B", "C"]), ids.0, ids.1));
    let locator = reference(&reader, &map, 1, &STANDARD).unwrap();
    let rebuilt = with_id(&pdf(&["X", "Y", "Z"]), ids.0, "ffffffffffffffffffffffffffffffff");
    let same = resolve_in(&locator, &rebuilt, &STANDARD);
    assert_eq!(same.document, DocumentMatch::Same, "the /ID[0] tag is the same");
    assert_eq!(same.status, Status::Gone, "and its pages are not");
}
