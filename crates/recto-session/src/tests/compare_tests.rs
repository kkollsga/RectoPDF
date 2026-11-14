use super::*;

/// Failure: ⌘-click doesn't build a comparison the way Finder's does: a second tab doesn't join, one in it doesn't leave, the last one left isn't alone, a fourth gets in, or a plain click doesn't return to one. And the pane being read isn't the one clicked.
#[test]
fn command_click_builds_a_comparison_and_a_plain_click_ends_it() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.tap(ids[0]).unwrap();
    assert_eq!(s.toggle_compare(ids[0]), Ok(compare::CompareChange::Nothing), "the only tab showing");
    assert_eq!(s.panes(), vec![ids[0]]);
    assert_eq!(s.toggle_compare(ids[1]), Ok(compare::CompareChange::Added));
    assert_eq!(s.panes(), vec![ids[0], ids[1]]);
    assert_eq!(s.tabs().active(), Some(ids[1]), "the one clicked is read");
    assert_eq!(s.toggle_compare(ids[2]), Ok(compare::CompareChange::Added));
    assert_eq!(s.toggle_compare(ids[3]), Err(compare::CompareError::TooMany));
    assert_eq!(s.panes().len(), compare::MAX_PANES);
    assert!(s.focus_pane(ids[0]));
    assert_eq!(s.tabs().active(), Some(ids[0]));
    assert!(!s.focus_pane(ids[3]), "not a pane");
    assert_eq!(s.toggle_compare(ids[0]), Ok(compare::CompareChange::Removed));
    assert_eq!(s.panes(), vec![ids[1], ids[2]]);
    assert_eq!(s.tabs().active(), Some(ids[1]), "reading moves to a pane that's left");
    assert_eq!(s.toggle_compare(ids[2]), Ok(compare::CompareChange::Ended));
    assert_eq!(s.panes(), vec![ids[1]]);
    assert!(s.comparison().is_none());
    s.compare(&[ids[0], ids[3]], None).unwrap();
    s.tap(ids[2]).unwrap();
    assert!(s.comparison().is_none(), "a plain click returns to one");
    assert_eq!(s.panes(), vec![ids[2]]);
    assert!(s.tabs().tab(ids[2]).unwrap().toc_open, "and doesn't fold its contents");
    s.compare(&[ids[0], ids[1]], None).unwrap();
    s.open(&PathBuf::from("/d/e.pdf")).unwrap();
    assert!(s.comparison().is_none(), "opening another PDF shows it alone");
    assert_eq!(s.panes().len(), 1);
}

/// Failure: a tab on screen in a comparison pane goes dormant, by time, budget or pressure, and its pane goes blank or its edits leave memory under it. Panes are on screen, and only what's off screen goes.
#[test]
fn panes_on_screen_dont_go_dormant() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.compare(&[ids[0], ids[1]], Some(ids[0])).unwrap();
    assert!(!s.dormant().may_go(ids[1]), "a pane not being read is still on screen");
    assert!(!s.dormant().oldest_first().contains(&ids[1]));
    assert!(s.dormant().oldest_first().contains(&ids[2]), "the tab off screen can");
    assert!(!s.sleeper().go_dormant(ids[1]));
}

/// Failure: closing a pane's tab leaves a pane for a tab that's gone, or a comparison of one, or the reader on a tab that's off screen rather than a pane that's left.
#[test]
fn closing_a_panes_tab_takes_its_pane() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.compare(&[ids[1], ids[2], ids[3]], Some(ids[2])).unwrap();
    s.close(ids[2]).unwrap();
    assert_eq!(s.panes(), vec![ids[1], ids[3]]);
    assert_eq!(s.tabs().active(), Some(ids[1]), "to a pane, not the next tab along");
    s.close(ids[1]).unwrap();
    assert!(s.comparison().is_none(), "one left is alone");
    assert_eq!(s.panes(), vec![ids[3]]);
    s.compare(&[ids[0], ids[3]], Some(ids[0])).unwrap();
    s.close(ids[3]).unwrap();
    assert_eq!(s.panes(), vec![ids[0]]);
}

/// Failure: a comparison doesn't come back after a restart exactly: its tabs, their order, synced scroll and each pane's start; or a record naming a tab that isn't there, or of one, or of four, gets in.
#[test]
fn a_comparison_comes_back_from_the_record_exactly() {
    let mut s = session(40);
    let ids: Vec<TabId> = ["a", "b", "c"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.go_to(ids[2], 7).unwrap();
    s.go_to(ids[0], 3).unwrap();
    s.compare(&[ids[2], ids[0]], Some(ids[0])).unwrap();
    assert!(s.set_compare_synced(true).is_some());
    let bytes = record::encode_record(&s.to_record(), 3);
    let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
    let mut back = session(40);
    back.restore(&read.record, HashMap::new()).unwrap();
    assert_eq!(back.comparison(), s.comparison());
    assert_eq!(back.comparison().unwrap().sync_from, vec![7, 3]);
    assert_eq!(back.tabs().active(), Some(ids[0]));
    assert_eq!(back.to_record(), s.to_record());
    for tabs in [vec![ids[0].0], vec![ids[0].0, 99], vec![ids[0].0, ids[0].0], vec![1, 2, 3, 3]] {
        let mut bad = s.to_record();
        bad.comparison = Some(record::ComparisonRecord {
            tabs,
            synced: false,
            sync_from: Vec::new(),
        });
        assert!(record::validate_record(&bad).is_err(), "{:?}", bad.comparison);
    }
    let mut uneven = s.to_record();
    uneven.comparison.as_mut().unwrap().sync_from = vec![1];
    assert!(record::validate_record(&uneven).is_err(), "a start for each pane, or none");
    // An older record has none, and reads.
    let mut older = s.to_record();
    older.comparison = None;
    let json = serde_json::to_string(&older).unwrap();
    assert!(!json.contains("comparison"), "nothing written for an older app to trip on");
}

/// Failure (review 1): a comparison with cold tabs leaves more live documents than SPEC §2 allows, as only the pane being read was made warm and the tabs warm before stayed; or making them warm reads their files with the session held. Each pane is one of the warm tabs, the rest go cold, and what's read ahead is all that's read.
#[test]
fn a_comparison_of_cold_tabs_is_the_live_documents_and_reads_nothing_held() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    assert_eq!(s.tabs().active(), Some(ids[4]));
    let panes = [ids[0], ids[1], ids[2]];
    let cold = s.wake_for_compare(&panes);
    assert_eq!(
        cold.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![ids[0], ids[1]],
        "the warm one needs no read"
    );
    for (id, _) in cold {
        let facts = s.loader().load(Path::new("/d/x.pdf"));
        s.sleeper().give_woken(id, facts, None);
    }
    let loads = s.loader().loads.get();
    s.compare(&panes, None).unwrap();
    assert_eq!(s.loader().loads.get(), loads, "nothing read with the session held");
    let mut warm: Vec<TabId> = s.warm().ids().collect();
    warm.sort();
    assert_eq!(warm, panes.to_vec(), "the panes are the live documents, and only they");
    assert!(warm.len() <= MAX_WARM);
}

/// Failure (review 1): the pane read next, when the one being read leaves by closing or a ⌘-click, is read from disk with the session held, as nobody read it ahead. Both say which file to read first.
#[test]
fn the_pane_read_next_is_read_ahead() {
    let mut s = session(10);
    let ids: Vec<TabId> = ["a", "b", "c"]
        .iter()
        .map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap())
        .collect();
    s.compare(&[ids[0], ids[1], ids[2]], Some(ids[1])).unwrap();
    // Come back from a record, every pane cold.
    let record = s.to_record();
    let mut back = session(10);
    back.restore(&record, HashMap::new()).unwrap();
    assert_eq!(back.tabs().active(), Some(ids[1]));
    back.wake_active();
    assert_eq!(
        back.wake_for_close(ids[1]).map(|(id, _)| id),
        Some(ids[0]),
        "closing the read pane wakes the first left, not the next tab along"
    );
    assert_eq!(
        back.wake_for_compare_click(ids[1]).map(|(id, _)| id),
        Some(ids[0]),
        "and so does ⌘-clicking it out"
    );
    assert_eq!(back.wake_for_compare_click(ids[2]), None, "a pane that isn't read leaving wakes nothing");
    let loads = back.loader().loads.get();
    let (next, _) = back.wake_for_compare_click(ids[1]).unwrap();
    let facts = back.loader().load(Path::new("/d/a.pdf"));
    back.sleeper().give_woken(next, facts, None);
    let read_ahead = back.loader().loads.get();
    back.toggle_compare(ids[1]).unwrap();
    assert_eq!(back.loader().loads.get(), read_ahead, "nothing read with the session held");
    assert!(read_ahead > loads);
    assert_eq!(back.tabs().active(), Some(ids[0]));
}

/// Failure (security review of phase R, S2): a tab's place says where a protected one is and goes on saying it: while it follows one in a synced
/// comparison it isn't withheld, or it's let go as soon as sync is off though it's still on the page following left it on, or it stays withheld after it's
/// moved otherwise. Withheld while it follows; after sync is off, only if it moved while it followed, and until its next move made otherwise; the protected
/// tab is withheld throughout, and a comparison of two plain tabs withholds nothing.
#[test]
fn a_tab_that_follows_a_withheld_one_has_its_place_withheld_until_moved_otherwise() {
    let mut s = session(10);
    let [plain, other, locked] = ["plain", "other", "locked"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
    s.tabs_mut().note_password_used(locked);
    s.compare(&[plain, other], None).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 4).unwrap();
    assert!(
        !compare::place_withheld(&s, plain) && !compare::place_withheld(&s, other),
        "two plain tabs synced"
    );
    s.compare(&[locked, plain], Some(locked)).unwrap();
    assert!(compare::place_withheld(&s, plain), "it follows the protected one");
    s.set_compare_synced(false);
    assert!(!compare::place_withheld(&s, plain), "it didn't move while it followed");
    s.set_compare_synced(true);
    s.go_to(plain, 7).unwrap();
    s.set_compare_synced(false);
    assert!(compare::place_withheld(&s, plain), "left where following put it");
    s.end_compare();
    assert!(compare::place_withheld(&s, plain), "and after the comparison ends");
    assert!(compare::place_withheld(&s, locked), "the protected tab, throughout");
    s.go_to(plain, 2).unwrap();
    assert!(!compare::place_withheld(&s, plain), "moved otherwise");
}

/// A plain tab `plain` left by following a protected one in a synced comparison: synced with `locked`, moved to page 7 while it followed, then synced
/// scroll turned off. Gives the session and (plain, locked).
fn left_by_following() -> (Session<FakeLoader>, TabId, TabId) {
    let mut s = session(10);
    let [plain, locked] = ["plain", "locked"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
    s.tabs_mut().note_password_used(locked);
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 7).unwrap();
    s.set_compare_synced(false);
    assert!(compare::place_withheld(&s, plain), "left where following put it");
    (s, plain, locked)
}

/// Failure (security review of S2's round, the same-page oracle): an agent's `goto` to the page a tab left by following is already on keeps its place
/// withheld, while one to any other page lets it go, so whether `where` then gives the page says whether the agent guessed where following left it. A
/// move that isn't a follow sets the place, whether or not the page changes.
#[test]
fn a_move_to_the_page_a_tab_is_on_lets_its_place_go_as_any_move_does() {
    let (mut s, plain, _) = left_by_following();
    s.jump_as(plain, 7, 0.0, Author::Agent).unwrap();
    assert!(!compare::place_withheld(&s, plain), "a goto to the same page set the place");
}

/// Failure (S2's round): a follow that stays on its page moves how far down it the tab is (`within_page`), and that isn't counted as placed by following,
/// so after synced scroll is off the tab says how far down the protected one went. A place the page view tells of while the tab follows is withheld too.
#[test]
fn a_follow_within_a_page_withholds_the_place_too() {
    let mut s = session(10);
    let [plain, locked] = ["plain", "locked"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
    s.tabs_mut().note_password_used(locked);
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    let place = record::PlaceRecord {
        page: 0,
        anchor_page: 0,
        within: 0.6,
        x: 0.0,
        y: 540.0,
        document_height: 9000.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(plain, place, 1));
    s.set_compare_synced(false);
    assert!(compare::place_withheld(&s, plain), "moved down its page while it followed");
}

/// Failure (S2's round): a page-drawn note made while a tab's place was its own answers `wait page-drawn` after following has moved it, or one made while
/// it followed answers after its place is its own again, so which pages were drawn says where following took it. A tab's notes go whenever its place
/// starts or stops being withheld.
#[test]
fn a_tabs_draw_notes_go_when_its_place_is_withheld_or_let_go() {
    let mut s = session(10);
    let [plain, locked] = ["plain", "locked"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
    s.tabs_mut().note_password_used(locked);
    let drawn = |s: &mut Session<FakeLoader>, page: u32| s.mail_mut().note_drawn(DrawNote { tab: plain, page, failure: None });
    drawn(&mut s, 0);
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 7).unwrap();
    assert_eq!(s.mail().draw_note(plain, 0), None, "a note from before it followed");
    drawn(&mut s, 7);
    s.set_compare_synced(false);
    s.jump_as(plain, 2, 0.0, Author::Agent).unwrap();
    assert_eq!(s.mail().draw_note(plain, 7), None, "a note from while it followed");
}

/// Failure (security review of S2's round, S2): an agent's `goto` on a tab left by following lets its place go and marks where it was for Back, and
/// `back` then lands there with its place its own, so `where` says where following left it; `forward` the same. A stop made from a withheld place is
/// marked; the person's Back onto one lands where it always did, with the place withheld again; a stop made from a place of the tab's own isn't.
#[test]
fn a_stop_made_from_a_withheld_place_is_marked_and_withholds_it_again() {
    let (mut s, plain, _) = left_by_following();
    s.jump_as(plain, 2, 0.0, Author::Agent).unwrap();
    assert!(compare::trail_step_withheld(&s, plain, true), "the stop Back would go to was made from the followed place");
    assert!(!compare::trail_step_withheld(&s, plain, false), "nothing forward");
    assert!(s.step_as(plain, true, Author::Owner).unwrap());
    assert_eq!(s.tabs().tab(plain).unwrap().page, 7, "the person's Back goes where it always went");
    assert!(compare::place_withheld(&s, plain), "and the place it lands on is withheld again");
    assert!(!compare::trail_step_withheld(&s, plain, false), "Forward goes to page 3, made from a place of its own");
}
