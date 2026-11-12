use super::*;

fn with_owner_and_agent_edits(s: &mut Session<FakeLoader>) -> (TabId, TabId) {
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.delete(4, 1)).unwrap();
    s.edit_pages(a, |e| e.move_pages(0, 1, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.rotate(2, 1, 3)).unwrap();
    assert!(s.undo_edit(a));
    assert!(s.undo_edit(a));
    s.go_to(a, 2).unwrap();
    s.tap(a).unwrap();
    s.tap(a).unwrap();
    (a, b)
}

/// Failure: a record from a build with focus view, written with it on, comes back with the sidebar showing, or the next record still says focus, so it can't be turned off. Owner, 2026-11-08: focus view became the sidebar's button.
#[test]
fn an_older_records_focus_view_comes_back_as_a_hidden_sidebar() {
    let old: record::ViewRecord = serde_json::from_str(r#"{"paper":null,"focus":true,"agent_icon_hidden":false}"#).unwrap();
    let mut saved = journaled(1).to_record();
    saved.view = old;
    let mut back = journaled(1);
    back.restore(&saved, HashMap::new()).unwrap();
    assert!(back.view().sidebar_hidden);
    let written = serde_json::to_string(&back.to_record().view).unwrap();
    assert!(!written.contains("focus") && written.contains("\"sidebar_hidden\":true"), "{written}");
    back.set_view(ViewSettings {
        sidebar_hidden: false,
        ..back.view()
    });
    let mut again = journaled(1);
    again.restore(&back.to_record(), HashMap::new()).unwrap();
    assert!(!again.view().sidebar_hidden, "shown again stays shown");
}

/// Failure: anything about the session is different after going through its record and journals: a tab, its order, the active one, a place, a fold, unsaved edits, the steps undone, or who made them. Compares the whole of both.
#[test]
fn a_session_comes_back_whole_from_its_record_and_journals() {
    let mut s = journaled(5);
    let (a, _b) = with_owner_and_agent_edits(&mut s);
    s.note_place(
        a,
        PlaceRecord {
            page: 2,
            anchor_page: 2,
            within: 0.25,
            x: 3.5,
            y: 1234.5,
            document_height: 9000.0,
            zoom: 1.5625,
            page_width: 612.0,
            page_height: 792.0,
        },
    );
    s.set_find(true, "ran", "typed but not run");
    s.set_edit_mode(true);
    s.set_view_rotation(a, 1);
    assert!(s.set_spread(a, record::Spread::TwoUpCover));
    assert!(!s.set_spread(a, record::Spread::TwoUpCover), "the same again changes nothing");
    s.set_selection(
        a,
        Some(Selection {
            from_page: 1,
            from_char: 2,
            to_page: 2,
            to_char: 9,
        }),
    );
    s.set_view(ViewSettings {
        paper: Some(Paper::Warm),
        sidebar_hidden: true,
        agent_icon_hidden: true,
    });
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let bytes = record::encode_record(&s.to_record(), 7);
    let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
    let mut back = journaled(5);
    back.restore(&read.record, back_from(&files)).unwrap();
    assert_eq!(back.to_record(), s.to_record());
    assert_eq!(back.edits_record(a), s.edits_record(a));
    assert_eq!(back.view(), s.view());
    assert_eq!(back.spread_of(a), Some(record::Spread::TwoUpCover), "a tab's layout comes back with it");
    assert_eq!(back.active(), s.active());
    assert_eq!(back.tier(a), Tier::Hot, "it's the active one");
    assert!(back.warm_tabs().next().is_none(), "and nothing's been opened");
    assert_eq!(back.page_edits(a).unwrap().slots(), s.page_edits(a).unwrap().slots());
    // Redo works, after a restart, and whose each step is holds for what's then allowed.
    assert!(back.page_edits(a).unwrap().can_redo());
    assert!(back.has_undone_owner_steps(a), "the owner's move is undone, and still theirs");
    assert!(back.redo_edit(a));
    assert!(!back.has_undone_owner_steps(a));
    assert!(back.redo_edit(a));
    assert!(s.redo_edit(a) && s.redo_edit(a));
    assert_eq!(back.page_edits(a).unwrap().slots(), s.page_edits(a).unwrap().slots());
    // What comes next goes on the same journal, with its count following on.
    back.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    write_journal(&mut back, &mut files);
    let mut third = journaled(5);
    third.restore(&back.to_record(), back_from(&files)).unwrap();
    assert_eq!(third.edits_record(a), s.edits_record(a));
}

/// Failure: restoring a thousand tabs reads a file for each, or holds their contents, or a restored tab costs what an opened one doesn't. Every one of them comes back cold with no load at all.
#[test]
fn restoring_opens_no_file() {
    let mut s = session(5);
    let ids: Vec<TabId> = (0..1000).map(|at| s.open(Path::new(&format!("/d/{at}.pdf"))).unwrap()).collect();
    s.tap(ids[500]).unwrap();
    let record = s.to_record();
    let mut back = session(5);
    back.restore(&record, HashMap::new()).unwrap();
    assert_eq!(back.loader.loads.get(), 0, "nothing read");
    assert_eq!(back.tabs().len(), 1000);
    assert_eq!(back.active(), Some(ids[500]));
    assert!(ids.iter().filter(|id| **id != ids[500]).all(|id| back.tier(*id) == Tier::Cold));
    assert_eq!(back.to_record(), record);
    assert_eq!(back.warm_tabs().count(), 0);
}

/// Failure: a journal that can't be read costs the tab silently. The tab comes back and says so, with no edits it can't vouch for.
#[test]
fn a_tab_whose_journal_cant_be_read_says_so() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let mut back = journaled(5);
    back.restore(&s.to_record(), HashMap::new()).unwrap();
    assert_eq!(back.tab_notice(a).unwrap().code(), "edits_unreadable");
    assert!(!back.is_edited(a));
    let mut torn = back_from(&files);
    torn.values_mut().next().unwrap().lost_lines = true;
    let mut partly = journaled(5);
    partly.restore(&s.to_record(), torn).unwrap();
    assert_eq!(partly.tab_notice(a).unwrap().code(), "edits_unreadable");
    assert!(partly.is_edited(a), "what could be read is here");
}

/// Failure: restoring turns a hostile record into something that panics later: a page past the end, or an active tab that isn't there. Or a record's tabs with no journal cost it a tab.
#[test]
fn a_restored_page_is_held_to_the_document() {
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let mut saved = s.to_record();
    saved.tabs[0].page = u32::MAX;
    let mut back = session(5);
    back.restore(&saved, HashMap::new()).unwrap();
    assert_eq!(back.tab(a).unwrap().page, 4);
    saved.active = Some(999);
    assert!(back.restore(&saved, HashMap::new()).is_err());
}
