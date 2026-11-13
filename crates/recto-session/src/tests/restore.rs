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
    assert!(back.viewing().view().sidebar_hidden);
    let written = serde_json::to_string(&back.to_record().view).unwrap();
    assert!(!written.contains("focus") && written.contains("\"sidebar_hidden\":true"), "{written}");
    let shown = ViewSettings {
        sidebar_hidden: false,
        ..back.viewing().view()
    };
    back.viewing_mut().set_view(shown);
    let mut again = journaled(1);
    again.restore(&back.to_record(), HashMap::new()).unwrap();
    assert!(!again.viewing().view().sidebar_hidden, "shown again stays shown");
}

/// Failure: anything about the session is different after going through its record and journals: a tab, its order, the active one, a place, a fold, unsaved edits, the steps undone, or who made them. Compares the whole of both.
#[test]
fn a_session_comes_back_whole_from_its_record_and_journals() {
    let mut s = journaled(5);
    let (a, _b) = with_owner_and_agent_edits(&mut s);
    s.view_told_of(
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
        1,
    );
    s.viewing_mut().set_find(true, "ran", "typed but not run");
    s.viewing_mut().set_edit_mode(true);
    s.tabs_mut().set_view_rotation(a, 1);
    assert!(s.tabs_mut().set_spread(a, record::Spread::TwoUpCover));
    assert!(!s.tabs_mut().set_spread(a, record::Spread::TwoUpCover), "the same again changes nothing");
    s.tabs_mut().set_selection(
        a,
        Some(Selection {
            from_page: 1,
            from_char: 2,
            to_page: 2,
            to_char: 9,
        }),
    );
    s.viewing_mut().set_view(ViewSettings {
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
    assert_eq!(back.lists().edits_record(a), s.lists().edits_record(a));
    assert_eq!(back.viewing().view(), s.viewing().view());
    assert_eq!(back.tabs().spread_of(a), Some(record::Spread::TwoUpCover), "a tab's layout comes back with it");
    assert_eq!(back.tabs().active(), s.tabs().active());
    assert_eq!(back.tier(a), Tier::Hot, "it's the active one");
    assert!(back.warm().ids().next().is_none(), "and nothing's been opened");
    assert_eq!(back.lists().get(a).unwrap().slots(), s.lists().get(a).unwrap().slots());
    // Redo works, after a restart, and whose each step is holds for what's then allowed.
    assert!(back.lists().get(a).unwrap().can_redo());
    assert!(back.lists().has_undone_owner_steps(a), "the owner's move is undone, and still theirs");
    assert!(back.redo_edit(a));
    assert!(!back.lists().has_undone_owner_steps(a));
    assert!(back.redo_edit(a));
    assert!(s.redo_edit(a) && s.redo_edit(a));
    assert_eq!(back.lists().get(a).unwrap().slots(), s.lists().get(a).unwrap().slots());
    // What comes next goes on the same journal, with its count following on.
    back.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    write_journal(&mut back, &mut files);
    let mut third = journaled(5);
    third.restore(&back.to_record(), back_from(&files)).unwrap();
    assert_eq!(third.lists().edits_record(a), s.lists().edits_record(a));
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
    assert_eq!(back.tabs().active(), Some(ids[500]));
    assert!(ids.iter().filter(|id| **id != ids[500]).all(|id| back.tier(*id) == Tier::Cold));
    assert_eq!(back.to_record(), record);
    assert_eq!(back.warm().ids().count(), 0);
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
    assert_eq!(back.tabs().tab_notice(a).unwrap().code(), "edits_unreadable");
    assert!(!back.lists().is_edited(a));
    let mut torn = back_from(&files);
    torn.values_mut().next().unwrap().lost_lines = true;
    let mut partly = journaled(5);
    partly.restore(&s.to_record(), torn).unwrap();
    assert_eq!(partly.tabs().tab_notice(a).unwrap().code(), "edits_unreadable");
    assert!(partly.lists().is_edited(a), "what could be read is here");
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
    assert_eq!(back.tabs().tab(a).unwrap().page, 4);
    saved.active = Some(999);
    assert!(back.restore(&saved, HashMap::new()).is_err());
}

/// Failure: an agent's `session restore` on a live session leaves the generated contents of a tab it replaces, and a restored tab with the same number, another file, shows them: one document's headings under another's name. Tab numbers come back from the record, so a number seen before is the usual case.
#[test]
fn a_restored_tab_never_shows_the_generated_contents_of_the_tab_it_replaced() {
    let lines = [TocLine {
        title: "Only in the old file",
        page: 0,
        parent: NONE,
        depth: 0,
    }];
    let mut other = session(10);
    let b = other.open(Path::new("/d/other.pdf")).unwrap();
    other.choose_generated(b, true);
    let record = other.to_record();
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    assert_eq!(a, b, "the restored tab has the old one's number");
    s.choose_generated(a, true);
    s.set_generated_view(a, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
    s.restore(&record, HashMap::new()).unwrap();
    // As the agent's restore does next.
    s.wake_active();
    assert_eq!(s.tabs().tab(a).unwrap().name.as_ref(), "other");
    assert_eq!(s.generated().view(a), None, "nothing's known of the restored file's generated contents");
    let shown: Vec<&str> = s
        .sidebar()
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Contents)
        .map(|row| row.text)
        .collect();
    assert!(
        !shown.contains(&"Only in the old file"),
        "the old file's headings show under the new: {shown:?}"
    );
}

/// The record of a session with the same tab numbers as `tabs` live ones, each another file, on a version the record names, the first of them active.
pub(super) fn another_session_with(tabs: usize) -> SessionRecord {
    let mut other = session(10);
    for n in 0..tabs {
        other.open(Path::new(&format!("/d/other-{n}.pdf"))).unwrap();
    }
    let mut record = other.to_record();
    for (n, tab) in record.tabs.iter_mut().enumerate() {
        tab.file = Some(replaced_stamp(n as u64).into());
    }
    record.active = record.tabs.first().map(|tab| tab.id);
    record
}

pub(super) fn replaced_stamp(n: u64) -> FileStamp {
    FileStamp {
        size: 100 + n,
        mtime_ns: 100,
        device: 1,
        inode: 100 + n,
    }
}

/// Failure: a save under way on a tab that a restore replaced holds the restored tab with its number as being saved, so its edits are refused with "being saved" until a save it has nothing to do with ends.
#[test]
fn a_restored_tab_isnt_being_saved_for_the_tab_it_replaced() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    s.edit_pages(a, |edits| edits.rotate(0, 1, 1)).unwrap();
    s.begin_save(a, false).unwrap();
    s.restore(&another_session_with(1), HashMap::new()).unwrap();
    assert!(!s.saves().is_saving(a) && !s.saves().any());
    assert_eq!(s.edit_pages(a, |edits| edits.rotate(0, 1, 1)), Ok(()));
    s.end_save(a);
    assert!(s.lists().is_edited(a), "the save that ends later takes nothing of the restored tab's");
}

/// Failure: a hold on a tab a restore replaced, by an agent's command or an export, keeps the restored tab with its number from ever going dormant once that hold has been let go of as well.
#[test]
fn a_restored_tab_isnt_held_for_the_tab_it_replaced() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    s.open(Path::new("/d/old-2.pdf")).unwrap();
    s.dormancy_mut().hold(a);
    let mut record = another_session_with(2);
    record.active = Some(record.tabs[1].id);
    s.restore(&record, HashMap::new()).unwrap();
    assert!(s.can_go_dormant(a), "nothing holds the restored tab");
    s.let_go(a);
    assert!(s.can_go_dormant(a));
}

/// Failure: a save remembered of a tab a restore replaced is believed of the restored tab with its number, so the page view carries another file's layout over as if it had just been saved.
#[test]
fn a_restored_tab_wasnt_saved_over_by_the_tab_it_replaced() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    for version in [1, 0] {
        s.edit_pages(a, |edits| edits.rotate(0, 1, 1)).unwrap();
        s.begin_save(a, false).unwrap();
        s.saved(a, replaced_stamp(version), 10, None, None);
        s.end_save(a);
    }
    assert!(s.saved_from(a).is_some());
    s.restore(&another_session_with(1), HashMap::new()).unwrap();
    assert_eq!(
        s.tabs().own_stamp(a),
        Some(replaced_stamp(0)),
        "the restored tab is on the version the save wrote"
    );
    assert_eq!(s.saved_from(a), None);
}

/// Failure: the sleep file a tab a restore replaced was woken from is taken for the restored tab with its number's first frame: another file's pictures.
#[test]
fn a_restored_tab_takes_no_sleep_file_of_the_tab_it_replaced() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    let sleep = sleep::SleepFile {
        file: replaced_stamp(9),
        file_pages: 10,
        pages: 10,
        list_seq: 0,
        renderer_only: false,
        version: None,
        toc: Toc::default(),
        usual: None,
        sizes: Vec::new(),
        pictures: Vec::new(),
    };
    s.dormancy.woke_from_file(a, sleep);
    s.restore(&another_session_with(1), HashMap::new()).unwrap();
    assert!(s.dormancy_mut().take_sleep_read(a).is_none());
}

/// Failure: a tab a restore replaced, woken from its sleep file, is still on the list of files to look at, which is then about the restored tab with its number: a look at a file for a wake that never happened to it.
#[test]
fn a_restored_tab_isnt_down_as_woken_from_a_sleep_file() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    s.dormancy.note_woken_from_file(a);
    s.restore(&another_session_with(1), HashMap::new()).unwrap();
    assert!(s.dormancy_mut().take_woken_from_file().is_empty());
}

/// Failure: a tab a restore replaced, just gone cold, has its sleep file made as the restored tab with its number: the restored file's name and version with the replaced file's contents.
#[test]
fn a_restored_tab_gets_no_sleep_file_of_the_tab_it_replaced() {
    let mut s = session(10);
    s.dormancy_mut().set_sleep_files(true);
    let ids: Vec<TabId> = (0..=MAX_WARM).map(|n| s.open(Path::new(&format!("/d/old-{n}.pdf"))).unwrap()).collect();
    assert_eq!(s.tier(ids[0]), Tier::Cold);
    s.restore(&another_session_with(MAX_WARM + 1), HashMap::new()).unwrap();
    assert!(s.take_left_warm().is_empty(), "no sleep file comes of a tab that's gone");
}

/// Failure: an edit that took away the page being read in a session a restore replaced is said of the restored one: "the page you were on was removed" about a document nobody's edited.
#[test]
fn a_restore_doesnt_say_a_place_was_lost_in_the_session_it_replaced() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/old.pdf")).unwrap();
    s.go_to(a, 4).unwrap();
    s.edit_pages(a, |edits| edits.delete(4, 1)).unwrap();
    s.restore(&another_session_with(1), HashMap::new()).unwrap();
    assert!(!s.mail_mut().take_place_lost());
}
