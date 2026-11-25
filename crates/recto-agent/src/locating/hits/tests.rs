//! The references on search hits, on a real file made by `recto_testpdf` and page texts made up for the hits: what `chars` and `ref` are, what is counted
//! when there is none, and what a search may spend. Each test names the failure it catches.

use super::*;
use recto_core::locator::{Target, TextMatch};
use recto_testpdf::{Page, TempPdf};
use serde_json::{Value, json};

fn numbered(count: usize) -> TempPdf {
    let pages: Vec<Page> = (0..count)
        .map(|index| Page::new(format!("BT /F1 12 Tf 72 700 Td (Page {index}) Tj ET")))
        .collect();
    TempPdf::new("hits.pdf", &recto_testpdf::pages(&pages))
}

fn hit() -> Value {
    json!({ "page": 1 })
}

fn made(hit: &Value) -> Locator {
    locator::decode(hit["ref"].as_str().expect("a token")).expect("a reference")
}

fn characters_of(hit: &Value) -> (CharacterRange, [u64; 2]) {
    let Target::Characters(range) = made(hit).target() else {
        panic!("not characters: {hit}")
    };
    (range, [hit["chars"][0].as_u64().expect("start"), hit["chars"][1].as_u64().expect("end")])
}

fn missing(refs: HitRefs<'_>, hits: &mut Value) -> (u64, Value) {
    let said = refs.finish(hits);
    (said["made"].as_u64().expect("made"), said["missing"].clone())
}

/// Failure: the offsets are bytes (the sharp s and the ligature are two and three bytes), so `recto text`'s characters never match, or the token's
/// check is over other text than the characters it names. The words are found again by the core's own check over the same text.
#[test]
fn a_hits_chars_are_unicode_characters_of_the_page_text_and_its_ref_checks_same() {
    let file = numbered(3);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let text = "Straße ﬁnal café";
    let start = text.find('ﬁ').expect("the ligature");
    let mut hit = hit();
    refs.mark(&mut hit, false, 2, text, Some(&(start..start + "ﬁnal".len())));
    let (range, chars) = characters_of(&hit);
    assert_eq!(chars, [7, 11], "characters, not bytes: {hit}");
    assert_eq!(range.offsets(), (7, 11));
    assert_eq!(locator::check_characters(range, text), TextMatch::Same);
    assert_eq!(made(&hit).page().number(), 3, "the file's page 2, counted from 1");
    assert_eq!(refs.made, 1);
}

/// Failure: a phrase hit's byte range runs on to the next word, so its reference names the space after the words as well.
#[test]
fn a_phrase_hits_range_that_runs_on_to_the_next_word_is_trimmed_to_its_words() {
    let file = numbered(1);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let text = "Mine page 3";
    let mut hit = hit();
    refs.mark(&mut hit, false, 0, text, Some(&(0..10)));
    assert_eq!(hit["chars"], json!([0, 9]));
    assert_eq!(characters_of(&hit).0.text_check(), locator::text_check("Mine page"));
}

/// Failure: a range off a character boundary, an empty one, or one past what a reference can hold gets a reference anyway (or a panic on the slice), or
/// is not counted under its own code, or opens the file for nothing.
#[test]
fn a_range_off_characters_empty_or_past_the_cap_gets_no_ref_is_counted_and_opens_nothing() {
    let file = numbered(1);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let mut hits = json!([hit(), hit(), hit(), hit(), hit()]);
    let big = "x".repeat(locator::MAX_CHARACTER as usize + 1);
    for (text, range) in [("é", 1..2), ("abc", 3..3), ("a    ", 1..5), ("é", 0..9), (big.as_str(), 0..big.len())] {
        refs.mark(&mut hits[0], false, 0, text, Some(&range));
    }
    // Every one marked the first hit, the last overwriting the others: only the counts and the file are looked at.
    assert_eq!(hits[0]["ref"], Value::Null);
    let (made, missing) = missing(refs, &mut hits);
    assert_eq!(made, 0);
    assert_eq!(missing, json!({ "empty": 2, "not_on_characters": 2, "past_character_cap": 1 }));
    assert!(opened.get().is_none(), "no file was opened for a hit refused by its range");
}

/// Failure: a hit on an added PDF's page is referred to a page of the tab's own file, or another spelling is, on a page of an added file, given a
/// reference; or another spelling on the file's own page gets characters (it has no range) or none at all.
#[test]
fn a_hit_on_an_added_files_page_gets_no_ref_and_another_spelling_gets_its_pages() {
    let file = numbered(3);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let (mut added, mut added_other, mut other) = (hit(), hit(), hit());
    refs.mark(&mut added, true, 1, "words", Some(&(0..5)));
    refs.mark(&mut added_other, true, 1, "words", None);
    refs.mark(&mut other, false, 1, "words", None);
    assert_eq!((added["ref"].clone(), added["chars"].clone()), (Value::Null, Value::Null));
    assert_eq!(added_other["ref"], Value::Null);
    assert_eq!(other["chars"], Value::Null, "a page reference has no characters");
    assert_eq!(made(&other).target(), Target::Page);
    assert_eq!(made(&other).page().number(), 2);
    let mut hits = json!([added, added_other, other]);
    assert_eq!(missing(refs, &mut hits), (1, json!({ "added_file": 2 })));
}

/// Failure: HIT_REF_CAPS is short of a full answer's 200 pages and the tags, so a search with 200 hits is left without references; or the small cap is off by one (one tag page and three pages are read, the next page is counted under `caps`).
#[test]
fn hit_ref_caps_at_their_limit_refer_and_one_more_page_is_counted_caps() {
    assert_eq!(HIT_REF_CAPS.pages, locator::TAG_PAGES as u32 + crate::search::MAX_LIMIT);
    let file = numbered(230);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let mut hits = Vec::new();
    for page in 16..16 + crate::search::MAX_LIMIT {
        let mut hit = hit();
        refs.mark(&mut hit, false, page, "words", Some(&(0..5)));
        hits.push(hit);
    }
    // The tags took one read here (the first page has an identity); the limit is for a file whose first pages have none.
    assert_eq!(missing(refs, &mut Value::Array(hits)), (200, json!({})), "a full answer is referred to whole");

    let small = ScanCaps { pages: 4, ..HIT_REF_CAPS };
    let opened = OnceCell::new();
    let mut refs = HitRefs::with_caps(Some(file.0.as_path()), &opened, &small);
    let mut three = Value::Array(
        (20..24)
            .map(|page| {
                let mut hit = hit();
                refs.mark(&mut hit, false, page, "words", Some(&(0..5)));
                hit
            })
            .collect(),
    );
    assert_eq!(missing(refs, &mut three), (3, json!({ "caps": 1 })));
}

fn replace_with_another_file(file: &TempPdf) {
    let other = file.folder().join("other.pdf");
    std::fs::write(&other, recto_testpdf::pages(&[Page::new("BT /F1 12 Tf 72 700 Td (Changed) Tj ET")])).expect("write");
    std::fs::rename(&other, &file.0).expect("rename over the file");
}

/// Failure: another program replaces the file during a search, and the hits keep references made from pages of a version that's gone (or some do and
/// some don't), or the answer still counts them as made. Also when it changes before the first hit, when the open fails and `finish` says what
/// happened.
#[test]
fn a_file_changed_during_the_search_withdraws_every_ref() {
    let file = numbered(4);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let mut hits = json!([hit(), hit(), hit()]);
    for (index, page) in [0, 1, 2].into_iter().enumerate() {
        refs.mark(&mut hits[index], false, page, "words", Some(&(0..5)));
    }
    assert!(hits[2]["ref"].is_string(), "before the change they have references");
    replace_with_another_file(&file);
    let said = refs.finish(&mut hits);
    assert_eq!(said["made"], 0);
    assert_eq!(said["missing"], json!({ "file_changed": 3 }));
    assert_eq!(said["note"], crate::locating::HIT_REFS_WITHDRAWN);
    assert!(hits.as_array().expect("hits").iter().all(|hit| hit["ref"].is_null() && hit["chars"].is_null()));

    let file = numbered(4);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    replace_with_another_file(&file);
    let mut hits = json!([hit(), hit()]);
    for index in 0..2 {
        refs.mark(&mut hits[index], false, 0, "words", Some(&(0..5)));
    }
    assert_eq!(
        missing(refs, &mut hits),
        (0, json!({ "file_changed": 2 })),
        "a change before the first hit is a change"
    );
}

/// Failure: a page of two million characters with 200 hits is walked from its start for each hit (200 times two million), which stalls a search on
/// one hostile page; or its page is read once for each hit. Counts characters stepped over, not seconds.
#[test]
fn two_hundred_hits_on_a_two_million_character_page_walk_it_once() {
    let file = numbered(2);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let text = format!("x{}", " ".repeat(9_999)).repeat(200);
    let mut hits = Vec::new();
    for index in 0..200 {
        let mut hit = hit();
        // A phrase hit's range runs on to the next word: the spaces after the `x` are in it.
        refs.mark(&mut hit, false, 0, &text, Some(&(index * 10_000..(index + 1) * 10_000)));
        assert_eq!(hit["chars"], json!([index * 10_000, index * 10_000 + 1]));
        hits.push(hit);
    }
    assert!(refs.walked <= text.chars().count() as u64, "walked {} of {}", refs.walked, text.chars().count());
    assert_eq!(
        refs.referrer.as_ref().map(|referrer| referrer.spent().0),
        Some(1),
        "the page and its tags, one read"
    );
    assert_eq!(missing(refs, &mut Value::Array(hits)), (200, json!({})));
}

/// Failure: a search that finds nothing pays for opening the file for references (13 ms on a 97 MiB report) and says something it has no hits for.
#[test]
fn a_search_that_finds_nothing_opens_nothing_for_references() {
    let file = numbered(2);
    let opened = OnceCell::new();
    let refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let said = refs.finish(&mut json!([]));
    assert_eq!((said["made"].clone(), said["missing"].clone()), (json!(0), json!({})));
    assert!(opened.get().is_none());
}

/// Failure: a file lopdf can't open (or no file at all) gives a panic or an unexplained null, or its hits are not counted under their own code.
#[test]
fn an_unreadable_file_gives_every_hit_no_ref_counted_file_unreadable() {
    let folder = recto_testpdf::TempDir::new("recto-hits-");
    let garbage = folder.join("garbage.pdf");
    std::fs::write(&garbage, b"not a pdf").expect("write");
    for path in [Some(garbage.as_path()), Some(folder.join("missing.pdf").as_path()), None] {
        let opened = OnceCell::new();
        let mut refs = HitRefs::new(path, &opened);
        let mut hits = json!([hit(), hit()]);
        for index in 0..2 {
            refs.mark(&mut hits[index], false, 0, "words", Some(&(0..5)));
        }
        assert_eq!(missing(refs, &mut hits), (0, json!({ "file_unreadable": 2 })), "{path:?}");
        assert_eq!(hits[0]["ref"], Value::Null);
    }
}

/// Failure: a search's log entry carries a hit's token (which is hostile-sized and names a document's words).
#[test]
fn a_searchs_log_entry_never_carries_a_hits_reference() {
    let file = numbered(1);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened);
    let mut hits = json!([hit()]);
    refs.mark(&mut hits[0], false, 0, "hunter2", Some(&(0..7)));
    assert!(hits[0]["ref"].as_str().is_some_and(|token| token.starts_with("r1.")));
    let references = refs.finish(&mut hits);
    let answer = json!({ "hits": hits, "references": references, "coverage": { "failed_page_reasons": { "needs_password": 1 }, "busy_pages": [4] } });
    let command = crate::Command::from(crate::commands::text::Search {
        tab: "active".into(),
        query: "hunter2".into(),
        pages: None,
        limit: None,
        scan: false,
        regex: false,
        rank: false,
        context: None,
    });
    let mut details = command.log_details();
    crate::search::log_how_it_went(&mut details, &command, &Ok(answer));
    assert!(!details.to_string().contains("r1."), "{details}");
}

/// Failure: references go on being read after the search's own deadline has passed (a 30 s search that stops reading pages still opens the file and reads
/// a page for each hit). A deadline already passed gives no reference and opens no file: every hit is counted under `caps`.
#[test]
fn references_stop_at_the_searchs_deadline_and_are_counted_caps() {
    let file = numbered(2);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened).with_deadline(std::time::Instant::now());
    let mut hits = json!([hit(), hit(), hit()]);
    for index in 0..3 {
        refs.mark(&mut hits[index], false, 0, "words", Some(&(0..5)));
    }
    assert_eq!(missing(refs, &mut hits), (0, json!({ "caps": 3 })));
    assert!(opened.get().is_none(), "no file was opened after the deadline");
}
