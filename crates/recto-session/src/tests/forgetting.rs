//! What the session lets go of a tab, by every route that lets go: closing it, its going dormant, forgetting what was read of it, and a restore. R3.2 turns each of these into calls on `Session`'s components, and these hold that no component is left holding what it held before.

use super::*;

/// Every part of the session that keeps something of a tab, by the name of what it keeps, and whether it holds anything of this one: the names of those that do, sorted. A part added to the session, or moved into a component, is listed here.
fn holding(s: &Session<FakeLoader>, id: TabId) -> Vec<&'static str> {
    let parts = [
        ("warm", s.warm.contains(id)),
        ("woken", s.warm.holds_read_of(id)),
        ("draws", s.mail.holds_draw_of(id)),
        ("saving", s.saves.is_saving(id)),
        ("abandoned_saves", s.saves.abandoned_has(id)),
        ("comparison", s.comparison.as_ref().is_some_and(|comparison| comparison.contains(id))),
        ("generated", s.generated.view(id).is_some()),
        ("saved_over", s.saves.remembers(id)),
        ("jumps", s.mail.waiting_jump(id).is_some()),
        ("view_word", s.mail.holds_view_word_of(id)),
    ];
    let mut holding: Vec<&'static str> = parts
        .into_iter()
        .chain(s.dormancy.holding(id))
        .chain(s.journals.holding(id))
        .chain(s.lists.holding(id))
        .chain(s.tabs.holding(id))
        .filter(|(_, holds)| *holds)
        .map(|(name, _)| name)
        .collect();
    // By name, so a part moving into a component doesn't change what's said.
    holding.sort_unstable();
    holding
}

fn stamp(n: u64) -> FileStamp {
    FileStamp {
        size: n,
        mtime_ns: n as i64,
        device: 1,
        inode: n,
    }
}

fn ready() -> GeneratedView {
    GeneratedView::Ready(Arc::new(Toc::default()))
}

/// A tab with something in every part a tab on screen can have: drawn, generated contents, a jump on its way, side by side with another, and saved over.
fn busy(s: &mut Session<FakeLoader>) -> (TabId, TabId) {
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.activate(a);
    // Saved twice, so the second save is over a version the session knows.
    for version in [2, 3] {
        s.edit_pages(a, |edits| edits.rotate(0, 1, 1)).unwrap();
        s.begin_save(a, false).unwrap();
        s.saved(a, stamp(version), 10, None, None);
        s.end_save(a);
    }
    s.mail_mut().note_drawn(DrawNote {
        tab: a,
        page: 0,
        failure: None,
    });
    s.jump(a, 3, 0.0).unwrap();
    s.jump(a, 2, 0.5).unwrap();
    s.mail_mut().jump_sent(a, trail::Stop::new(2, 0.5), 2);
    s.jump(a, 3, 0.0).unwrap();
    s.compare(&[a, b], Some(a)).unwrap();
    // After the comparison, which shows each tab in turn: only the tab on screen keeps a generated list.
    s.set_generated_view(a, ready());
    (a, b)
}

/// Failure: closing a tab leaves something of it behind in a part of the session, so a long session grows with every tab it ever had, or a tab number seen again (a restore brings back its own) finds another tab's state. Only its journal, still to be written, may stay, and a save's note of it, which is capped and believed only while a tab is on the version it names.
#[test]
fn closing_a_tab_leaves_nothing_of_it() {
    let mut s = journaled(10);
    let (a, b) = busy(&mut s);
    let before = holding(&s, a);
    for part in [
        "tabs",
        "active",
        "warm",
        "draws",
        "generated",
        "jumps",
        "view_word",
        "comparison",
        "edits",
        "journals",
        "saved_over",
    ] {
        assert!(before.contains(&part), "the tab has something in {part} to let go: {before:?}");
    }
    s.close(a).unwrap();
    assert_eq!(holding(&s, a), ["departing", "saved_over"], "closing leaves only its journal and a save's note");
    assert!(holding(&s, b).contains(&"tabs"), "the other tab is still there");
}

/// Failure: a tab that goes dormant keeps what it was shown with, its contents, draws, generated contents or a read of its file, so a thousand tabs left alone cost what a thousand open ones do.
#[test]
fn a_tab_gone_dormant_holds_no_more_than_a_cold_one() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.mail_mut().note_drawn(DrawNote {
        tab: a,
        page: 0,
        failure: None,
    });
    s.set_generated_view(a, ready());
    assert!(s.view_told_of(a, place_on(0, 0.5), 1));
    assert_eq!(s.tabs().active(), Some(b));
    assert!(holding(&s, a).contains(&"warm"));
    assert!(holding(&s, a).contains(&"view_word"));
    assert!(s.sleeper().go_dormant(a));
    assert_eq!(holding(&s, a), ["tabs"]);
}

fn place_on(page: u32, within: f64) -> PlaceRecord {
    PlaceRecord {
        page,
        anchor_page: page,
        within,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    }
}

/// Failure: a jump the look sent the page view is kept as where the reader is past an edit, a save or the tab going dormant, with no read of the view
/// since to tell of it: the jump is about pages that are other pages now (an edit), the same pages in another file (a save), or a view that no longer
/// holds the tab (dormancy), so Back marks a place the reader never saw.
#[test]
fn a_jump_sent_goes_with_an_edit_a_save_and_dormancy() {
    for route in ["edit", "save", "dormancy"] {
        let mut s = journaled(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.activate(a);
        s.go_to(a, 4).unwrap();
        assert!(s.view_told_of(a, place_on(4, 0.6), 5), "{route}: made before the send");
        s.jump(a, 4, 0.8).unwrap();
        s.mail_mut().jump_sent(a, trail::Stop::new(4, 0.8), 10);
        assert_eq!(s.here(a), Some(trail::Stop::new(4, 0.8)), "{route}: the jump sent");
        match route {
            "edit" => s.edit_pages(a, |edits| edits.rotate(0, 1, 1)).unwrap(),
            "save" => {
                s.begin_save(a, false).unwrap();
                s.saved(a, stamp(2), 10, None, None);
                s.end_save(a);
            }
            _ => {
                s.activate(b);
                assert!(s.sleeper().go_dormant(a), "{route}: it goes dormant");
            }
        }
        assert_eq!(s.here(a), Some(trail::Stop::new(4, 0.6)), "{route}: the jump's no longer where the reader is");
    }
}

/// Failure: what was read of a tab is kept once its file has turned out to be another, so the next look shows the old file's contents.
#[test]
fn forgetting_what_was_read_of_a_tab_forgets_all_of_it() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.set_generated_view(a, ready());
    s.sleeper().forget_read(a);
    assert_eq!(holding(&s, a), ["active", "tabs"]);
    assert!(s.tabs().tab(a).unwrap().from_sleep.is_none());
}

/// Failure: a restore keeps something of the tabs of the session it replaces, so a restored tab with the same number, another file, takes it as its own.
#[test]
fn a_restore_keeps_nothing_of_the_tabs_it_replaces() {
    let mut s = journaled(10);
    let (a, b) = busy(&mut s);
    s.dormancy_mut().hold(a);
    let empty = session(10).to_record();
    s.restore(&empty, HashMap::new()).unwrap();
    for id in [a, b] {
        assert_eq!(holding(&s, id), Vec::<&str>::new(), "tab {id:?}");
    }
}

/// Failure: reads of cold tabs' files, made ahead of waking them, pile up past their cap while nothing wakes them, each with a document's contents.
#[test]
fn reads_ahead_are_held_to_their_cap_and_the_oldest_goes() {
    let mut s = session(10);
    let ids: Vec<TabId> = (0..MAX_WARM + MAX_READ_AHEAD + 1)
        .map(|n| s.open(Path::new(&format!("/d/{n}.pdf"))).unwrap())
        .collect();
    let cold = &ids[..=MAX_READ_AHEAD];
    assert!(cold.iter().all(|id| s.tier(*id) == Tier::Cold));
    for id in cold {
        s.sleeper().give_woken(*id, Err(OpenError::NotFound), None);
    }
    assert_eq!(s.warm.reads_held(), MAX_READ_AHEAD);
    assert!(!s.warm().holds_read_of(cold[0]), "the oldest read went");
    assert!(cold[1..].iter().all(|id| s.warm().holds_read_of(*id)), "the newest are kept");
}

/// Failure: tabs open past [`MAX_TABS`], the cap on everything grown from outside, as an agent opening file after file could make them.
#[test]
fn no_tab_opens_past_the_cap() {
    let mut s = session(1);
    s.open(Path::new("/d/first.pdf")).unwrap();
    let mut record = s.to_record();
    let entry = record.tabs[0].clone();
    record.tabs = (1..=MAX_TABS as u32)
        .map(|id| record::TabEntry {
            id,
            path: PathField::Text(format!("/d/{id}.pdf")),
            ..entry.clone()
        })
        .collect();
    record.next_id = MAX_TABS as u32 + 1;
    record.active = Some(1);
    s.restore(&record, HashMap::new()).unwrap();
    assert_eq!(s.tabs().len(), MAX_TABS);
    assert_eq!(s.open(Path::new("/d/one-more.pdf")), Err(SessionError::TooManyTabs));
    assert_eq!(s.open(Path::new("/d/7.pdf")), Ok(TabId(7)), "an open file still goes to its tab");
}
