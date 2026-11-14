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
/// moved by a jump. Withheld while it follows; after sync is off, only if it moved while it followed, and until its next jump (D-1); the protected
/// tab is withheld throughout, and a comparison of two plain tabs withholds nothing. Only the person's jump lets it go; an agent's keeps it (D-A).
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
    assert!(
        compare::place_withheld(&s, plain),
        "the person's scroll lands where following left it, plus how far (D-1)"
    );
    s.jump_as(plain, 2, 0.0, Author::Agent).unwrap();
    assert!(compare::place_withheld(&s, plain), "an agent's jump moves it and keeps it withheld (D-A)");
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    assert!(!compare::place_withheld(&s, plain), "moved by the person's jump");
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

/// Failure (security review of S2's round, the same-page oracle): a jump to the page a tab left by following is already on keeps its place withheld,
/// while one to any other page lets it go, so whether `where` then gives the page says whether the jump went where following left it. The person's jump
/// sets the place, whether or not the page changes; an agent's sets it on neither (D-A).
#[test]
fn a_move_to_the_page_a_tab_is_on_lets_its_place_go_as_any_move_does() {
    let (mut s, plain, _) = left_by_following();
    s.jump_as(plain, 7, 0.0, Author::Owner).unwrap();
    assert!(!compare::place_withheld(&s, plain), "the person's goto to the same page set the place");
}

/// Failure (the third security review; the coordinator's D-A): an agent's jump on a tab whose place is withheld lets it go, so what follows (the page
/// view telling its old place again, the draws in the gap before the view arrives, how many of the page's tiles are drawn again) can be set against the
/// agent's own guess, again and again. An agent's jump of every kind the session has (to a page, the same page, past the end, and down a page) moves the
/// tab and leaves its place withheld; the person's lets it go.
#[test]
fn an_agents_jump_never_lets_a_withheld_place_go() {
    for (page, within) in [(2, 0.0), (7, 0.0), (99, 0.0), (4, 0.5)] {
        let (mut s, plain, _) = left_by_following();
        s.jump_as(plain, page, within, Author::Agent).unwrap();
        assert_eq!(s.tabs().tab(plain).unwrap().page, page.min(9), "the tab moved");
        assert!(compare::place_withheld(&s, plain), "an agent's jump to {page} kept it withheld");
        assert!(s.mail().waiting_jump(plain).is_some(), "and the page view is sent there");
    }
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
    let drawn = |s: &mut Session<FakeLoader>, page: u32| {
        s.mail_mut().note_drawn(DrawNote {
            tab: plain,
            page,
            failure: None,
        })
    };
    drawn(&mut s, 0);
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 7).unwrap();
    assert_eq!(s.mail().draw_note(plain, 0), None, "a note from before it followed");
    drawn(&mut s, 7);
    s.set_compare_synced(false);
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    assert_eq!(s.mail().draw_note(plain, 7), None, "a note from while it followed");
}

/// Failure (security review of S2's round, S2): the person's `goto` on a tab left by following lets its place go and marks where it was for Back, and
/// `back` then lands there with its place its own, so `where` says where following left it; `forward` the same. A stop made from a withheld place is
/// marked; the person's Back onto one lands where it always did, with the place withheld again; a stop made from a place of the tab's own isn't.
#[test]
fn a_stop_made_from_a_withheld_place_is_marked_and_withholds_it_again() {
    let (mut s, plain, _) = left_by_following();
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    assert!(
        compare::trail_step_withheld(&s, plain, true),
        "the stop Back would go to was made from the followed place"
    );
    assert!(!compare::trail_step_withheld(&s, plain, false), "nothing forward");
    assert!(s.step_as(plain, true, Author::Owner).unwrap());
    assert_eq!(s.tabs().tab(plain).unwrap().page, 7, "the person's Back goes where it always went");
    assert!(compare::place_withheld(&s, plain), "and the place it lands on is withheld again");
    assert!(
        !compare::trail_step_withheld(&s, plain, false),
        "Forward goes to page 3, made from a place of its own"
    );
}

/// A place of the page view's on `page`, `within` the way down it.
fn place_on(page: u32, within: f64) -> record::PlaceRecord {
    record::PlaceRecord {
        page,
        anchor_page: page,
        within,
        x: 0.0,
        y: 540.0,
        document_height: 9000.0,
        zoom: 1.5,
        page_width: 0.0,
        page_height: 0.0,
    }
}

/// Failure (the second security review of S2's round, S-1): the comparison's own checks ask only whether a pane was opened with a password, so a plain
/// tab synced beside one whose place is withheld (left by following a protected one) follows it unwithheld and says where following left it; and that
/// tab, following the third as the person scrolls it, has its own mark let go. Withholding carries from tab to tab through syncing: a tab that follows
/// one whose place is withheld has its place withheld, the comparison counts as withheld (no lengths, no starts), and neither loses it by following.
#[test]
fn withholding_a_place_carries_through_syncing_to_a_third_tab() {
    let (mut s, plain, _) = left_by_following();
    let other = s.open(&PathBuf::from("/d/other.pdf")).unwrap();
    s.compare(&[plain, other], Some(other)).unwrap();
    assert!(
        !compare::comparison_withheld(&s) || compare::place_withheld(&s, plain),
        "separate: only the followed tab"
    );
    assert!(!compare::place_withheld(&s, other), "separate scroll follows nothing");
    s.set_compare_synced(true);
    assert!(compare::place_withheld(&s, other), "it follows a tab whose place is withheld");
    assert!(
        compare::comparison_withheld(&s),
        "where synced scroll started each pane says where following left the first"
    );
    // The person scrolls the third tab, and the first follows it.
    s.go_to(other, 3).unwrap();
    s.go_to(plain, 9).unwrap();
    s.set_compare_synced(false);
    assert!(
        compare::place_withheld(&s, plain),
        "following the third, whose place followed it, kept its mark"
    );
    assert!(compare::place_withheld(&s, other), "the third moved while it followed");
}

/// Failure (the second security review, D-1): the person's own scroll, page keys, Back or Forward in a tab left by following lets its place go, so where
/// it lands, a function of where following left it, is told; or a place the page view tells of after a zoom or a change of layout does. Only an absolute
/// move lets a withheld place go: a jump (an agent's or the person's `goto`, a contents row, a link, a Find hit chosen).
#[test]
fn only_an_absolute_move_lets_a_withheld_place_go() {
    let (mut s, plain, _) = left_by_following();
    s.go_to(plain, 8).unwrap();
    assert!(
        compare::place_withheld(&s, plain),
        "the person's scroll to another page lands where following left it, plus how far"
    );
    assert!(s.view_told_of(plain, place_on(8, 0.4), 50));
    assert!(compare::place_withheld(&s, plain), "a place the page view tells of, after a zoom or a layout");
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    s.jump_as(plain, 4, 0.0, Author::Owner).unwrap();
    assert!(!compare::place_withheld(&s, plain), "the person's jumps set the place");
    assert!(s.step_as(plain, true, Author::Owner).unwrap());
    assert!(!compare::place_withheld(&s, plain), "Back onto a stop of the tab's own");
    // Back, from a place withheld again by following, onto a stop made before it: where it lands is the trail's, but D-1 keeps it withheld.
    let mut s = session(10);
    let [plain, locked] = ["plain", "locked"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
    s.tabs_mut().note_password_used(locked);
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    s.jump_as(plain, 4, 0.0, Author::Owner).unwrap();
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 7).unwrap();
    s.set_compare_synced(false);
    assert!(s.step_as(plain, true, Author::Owner).unwrap());
    assert_eq!(s.tabs().tab(plain).unwrap().page, 2);
    assert!(compare::place_withheld(&s, plain), "the person's Back from a withheld place keeps it withheld");
}

/// Failure (the second security review, S-2): the person's `goto p` on a tab left by following on page q lets the flag go and keeps the page view's
/// place from before, so `session show`'s `zoom` and `within_page` are given only when p is q, and an agent learns whether the person went where
/// following left them, and how far down it the protected document was. Letting a withheld place go drops what was derived from it: the place the page
/// view said, the draw notes; what an agent then reads of the tab is the same for every p. In a pane and in the background.
#[test]
fn letting_a_withheld_place_go_drops_what_was_derived_from_it() {
    for in_pane in [true, false] {
        let told = |p: u32| {
            let mut s = session(10);
            let [plain, locked, other] = ["plain", "locked", "other"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
            s.tabs_mut().note_password_used(locked);
            s.compare(&[locked, plain], Some(locked)).unwrap();
            s.set_compare_synced(true);
            s.go_to(plain, 7).unwrap();
            assert!(s.view_told_of(plain, place_on(7, 0.6), 50));
            s.set_compare_synced(false);
            if !in_pane {
                s.tap(other).unwrap();
            }
            assert!(compare::place_withheld(&s, plain));
            s.jump_as(plain, p, 0.0, Author::Owner).unwrap();
            let tab = s.tabs().tab(plain).unwrap();
            (
                crate::place_shown(tab).map(|place| (place.zoom, place.within)),
                s.mail().waiting_jump(plain).map(|stop| stop.within),
            )
        };
        assert_eq!(told(7), told(2), "in a pane: {in_pane}");
    }
}

/// Failure (the third security review, D-B): after the person's own jump lets a withheld place go, a page view that still holds the tab where following
/// left it (warm in the background, the person having gone to another tab before the jump was sent) tells that place again, and it's taken in: on the
/// page the person went to, an agent reads its zoom and how far down it the protected document was, only when that page is where following left the
/// tab. Nothing a view says before the jump is sent is taken in, nor a read stamped before the send once it is; the view's word at the jump is.
#[test]
fn a_views_word_from_before_the_persons_release_is_not_taken_in() {
    let shown = |p: u32| {
        let (mut s, plain, _) = left_by_following();
        assert!(s.view_told_of(plain, place_on(7, 0.6), 50));
        s.jump_as(plain, p, 0.0, Author::Owner).unwrap();
        let before_send = s.view_told_of(plain, place_on(7, 0.6), 60);
        let kept = crate::place_shown(s.tabs().tab(plain).unwrap()).map(|place| (place.zoom, place.within));
        let to = s.mail_mut().take_jump(plain).expect("the person's jump");
        s.mail_mut().jump_sent(plain, to, 100);
        let read_before = s.view_told_of(plain, place_on(7, 0.6), 90);
        let at_the_jump = s.view_told_of(plain, place_on(p, 0.0), 100);
        (before_send, kept, read_before, at_the_jump)
    };
    assert_eq!(shown(7), (false, None, false, true), "to the page following left it on");
    assert_eq!(shown(7), shown(2), "told alike whatever page the person went to");
}

/// A plain tab with `earlier` jumps made in it first (from page 0 up, one a page), then synced beside a protected one and moved to page 7 by following,
/// then separate. Gives the session and the plain tab.
fn followed_after_jumps(earlier: u32) -> (Session<FakeLoader>, TabId) {
    let mut s = session(200);
    let [plain, locked] = ["plain", "locked"].map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap());
    s.tabs_mut().note_password_used(locked);
    for page in 0..earlier {
        s.jump_as(plain, 100 + page, 0.0, Author::Owner).unwrap();
    }
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 7).unwrap();
    s.set_compare_synced(false);
    (s, plain)
}

/// Failure (the second security review, D-2): a jump from a withheld place to the place it's on leaves a stop there, to keep the trail from saying
/// whether the jump went where following left the tab, and the person's Back then lands where they already are: Back does nothing, or is offered with
/// nowhere to go. The person's Back after such a jump moves, or isn't offered, exactly as when the place was never withheld.
#[test]
fn the_persons_back_after_a_jump_to_where_a_withheld_place_is_goes_as_ever() {
    for earlier in [0, 1, crate::trail::MAX_TRAIL as u32] {
        let persons = |withheld: bool| {
            let (mut s, plain) = if withheld {
                followed_after_jumps(earlier)
            } else {
                let mut s = session(200);
                let plain = s.open(&PathBuf::from("/d/plain.pdf")).unwrap();
                for page in 0..earlier {
                    s.jump_as(plain, 100 + page, 0.0, Author::Owner).unwrap();
                }
                s.go_to(plain, 7).unwrap();
                (s, plain)
            };
            s.jump_as(plain, 7, 0.0, Author::Owner).unwrap();
            let offered = s.tabs().can_go_back_and_forward(plain);
            let went = s.step_as(plain, true, Author::Owner).unwrap();
            let page = s.tabs().tab(plain).unwrap().page;
            let forward = s.step_as(plain, false, Author::Owner).unwrap();
            (offered, went, page, forward, s.tabs().tab(plain).unwrap().page)
        };
        assert_eq!(persons(true), persons(false), "with {earlier} stops before");
    }
}

/// Failure (the second security review, D-2, and the third's D-A): an agent tells a jump to where following left a tab from one elsewhere by what it's
/// told after it: the place, whether Back is there to go to, refused, or where it lands, after its goto, after the person's own Back, and after the
/// person's own jump, which lets the place go. Twins whose agent jumped to the page following left the tab on and to another are told the same: with no
/// stops before, one, and a trail at its cap. What an agent is told is the place and the trail's facts only while the place isn't withheld, as every
/// command that gives either asks [`compare::place_withheld`] first (`recto_agent::place`).
#[test]
fn an_agents_jump_to_where_following_left_a_tab_is_told_as_one_elsewhere() {
    for earlier in [0, 1, crate::trail::MAX_TRAIL as u32] {
        let told = |p: u32| {
            let (mut s, plain) = followed_after_jumps(earlier);
            s.jump_as(plain, p, 0.0, Author::Agent).unwrap();
            let agent = |s: &Session<FakeLoader>| {
                (!compare::place_withheld(s, plain)).then(|| {
                    (
                        s.tabs().tab(plain).unwrap().page,
                        compare::trail_step_withheld(s, plain, true),
                        compare::trail_step_withheld(s, plain, false),
                        s.tabs().can_go_back_and_forward(plain),
                    )
                })
            };
            let after_goto = agent(&s);
            let _ = s.step_as(plain, true, Author::Owner);
            let after_back = agent(&s);
            s.jump_as(plain, 20, 0.0, Author::Owner).unwrap();
            (after_goto, after_back, agent(&s))
        };
        assert_eq!(told(7), told(2), "with {earlier} stops before");
    }
}

/// Failure (the second security review, S-4): a restart forgets that a tab's place is withheld and which stops were made from a withheld place, and an
/// agent at Full can cause one (`update install --now`), then reads where following left the tab. The record keeps both, and leaves them out of its
/// bytes when there are none, so every record without them is as it was.
#[test]
fn the_record_keeps_a_withheld_place_and_its_trails_marks() {
    let (mut s, plain) = followed_after_jumps(0);
    let locked = s.tabs().iter().map(|tab| tab.id).find(|id| *id != plain).unwrap();
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    s.compare(&[locked, plain], Some(locked)).unwrap();
    s.set_compare_synced(true);
    s.go_to(plain, 5).unwrap();
    s.end_compare();
    let marks = |s: &Session<FakeLoader>| {
        let trail = s.tabs().tab(plain).unwrap().reading.trail.clone().unwrap_or_default();
        (compare::place_withheld(s, plain), trail.back().to_vec(), trail.forward().to_vec())
    };
    assert!(marks(&s).0 && marks(&s).1.iter().all(|stop| stop.from_withheld), "{:?}", marks(&s));
    let bytes = record::encode_record(&s.to_record(), 3);
    let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
    let mut back = session(200);
    back.restore(&read.record, HashMap::new()).unwrap();
    assert_eq!(marks(&back), marks(&s), "kept across a restart");
    let mut plain_only = session(200);
    let id = plain_only.open(&PathBuf::from("/d/plain.pdf")).unwrap();
    plain_only.jump_as(id, 3, 0.0, Author::Owner).unwrap();
    let text = String::from_utf8_lossy(&record::encode_record(&plain_only.to_record(), 3)).into_owned();
    assert!(
        !text.contains("withheld") && !text.contains("mark_only"),
        "nothing written when nothing is withheld: {text}"
    );
}

/// Failure (the third review, D-C; the fourth's item 4): the record's list of tabs whose place is withheld, read from a file that's hostile, panics, grows
/// by what it claims, marks a tab it doesn't name, gives back a place it named, or costs the person their session: an id that names no tab, one named
/// twice, a list one past [`crate::MAX_TABS`], and a list of the wrong type. What names a tab withholds that tab and no other after a restore, as the ids
/// are the tabs' own; a list that can't be read (past the cap, or the wrong type) withholds every tab's place, as "something was withheld and which can't
/// be told" (fail closed), and the rest of the record is read as usual, so the tabs and the active one come back; a record with nothing withheld has no
/// such key.
#[test]
fn the_records_list_of_withheld_places_is_read_as_hostile() {
    let (mut s, plain) = followed_after_jumps(0);
    let other = s.open(&PathBuf::from("/d/other.pdf")).unwrap();
    let body = |s: &Session<FakeLoader>| serde_json::to_value(s.to_record()).unwrap();
    let mut written = body(&s);
    assert_eq!(written["withheld_places"], serde_json::json!([plain.0]), "the one tab, by its id");
    let read = |body: &serde_json::Value| {
        let bytes = record::seal(record::MAIN_KIND, 3, &serde_json::to_vec(body).unwrap());
        record::decode_record(&bytes, record::CURRENT_VERSION).map(|read| {
            let mut back = session(200);
            back.restore(&read.record, HashMap::new()).unwrap();
            [plain, other].map(|id| compare::place_withheld(&back, id))
        })
    };
    written["withheld_places"] = serde_json::json!([plain.0, 999_999, plain.0]);
    assert_eq!(read(&written), Ok([true, false]), "an id of no tab is nothing, and one twice is one");
    written["withheld_places"] = serde_json::json!(vec![plain.0; crate::MAX_TABS + 1]);
    assert_eq!(read(&written), Ok([true, true]), "one past the cap withholds every tab's place");
    for wrong in [
        serde_json::json!("1"),
        serde_json::json!([-1]),
        serde_json::json!([plain.0, {}]),
        serde_json::json!([plain.0, [1, 2]]),
        serde_json::json!([4_294_967_296_u64]),
        serde_json::json!({"1": true}),
        serde_json::json!(null),
        serde_json::json!(true),
    ] {
        written["withheld_places"] = wrong.clone();
        assert_eq!(read(&written), Ok([true, true]), "{wrong} withholds every tab's place");
    }
    // The rest of the record is read as usual: the tabs and the one being read.
    let bytes = record::seal(record::MAIN_KIND, 3, &serde_json::to_vec(&written).unwrap());
    let back = record::decode_record(&bytes, record::CURRENT_VERSION).expect("the record reads");
    assert_eq!(back.record.tabs.len(), s.tabs().all().len());
    assert_eq!(back.record.active, s.tabs().active().map(|id| id.0));
    // Written again, it says each tab by its id, so a restart after it still withholds them all; and a record read with the list unreadable, written as it
    // was read (as a compaction does), reads back the same.
    let mut every = session(200);
    every.restore(&back.record, HashMap::new()).unwrap();
    let sorted = |mut ids: Vec<u32>| {
        ids.sort_unstable();
        ids
    };
    let written_ids: Vec<u32> = serde_json::from_value(body(&every)["withheld_places"].clone()).expect("a list of ids");
    assert_eq!(sorted(written_ids), sorted(every.tabs().all().iter().map(|tab| tab.id.0).collect()));
    let again = record::decode_record(&record::encode_record(&back.record, 4), record::CURRENT_VERSION).expect("written as read");
    assert_eq!(again.record.withheld_places, back.record.withheld_places);
    let plain_only = session(10);
    assert!(body(&plain_only).get("withheld_places").is_none(), "nothing withheld, no key");
}

/// Failure (the second review, S-3): a draw asked of the page view while it was where a withheld place had it, and finished after the person's `goto`
/// let that place go, is noted and announced, so `wait page-drawn` and a watcher's `page_drawn` name the pages around where following left the tab. From the
/// release every draw of the tab is dropped until the jump's send, then those asked before the send; a tab never withheld has every draw told.
#[test]
fn draws_asked_before_a_withheld_place_was_let_go_are_dropped() {
    let (mut s, plain, locked) = left_by_following();
    let note = |page| DrawNote {
        tab: plain,
        page,
        failure: None,
    };
    s.jump_as(plain, 2, 0.0, Author::Owner).unwrap();
    assert!(!s.mail_mut().note_drawn_asked(note(8), 100), "drawn before the jump was sent");
    let to = s.mail_mut().take_jump(plain).expect("the goto's jump");
    s.mail_mut().jump_sent(plain, to, 200);
    assert!(!s.mail_mut().note_drawn_asked(note(6), 199), "asked before the send");
    assert_eq!(s.mail().draw_note(plain, 6), None);
    assert!(s.mail_mut().note_drawn_asked(note(2), 200), "asked of the view at the jump");
    assert!(s.mail().draw_note(plain, 2).is_some());
    let other = s.open(&PathBuf::from("/d/other.pdf")).unwrap();
    let plain_note = DrawNote {
        tab: other,
        page: 0,
        failure: None,
    };
    assert!(s.mail_mut().note_drawn_asked(plain_note, 1), "a tab never withheld");
    let _ = locked;
}
