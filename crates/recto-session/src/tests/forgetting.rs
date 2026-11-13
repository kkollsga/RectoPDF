//! What the session lets go of a tab, by every route that lets go: closing it, its going dormant, forgetting what was read of it, and a restore. R3.2 turns each of these into calls on `Session`'s components, and these hold that no component is left holding what it held before.

use super::*;

/// Every part of the session that keeps something of a tab, by the name of what it keeps, and whether it holds anything of this one. A part added to the session, or moved into a component, is listed here.
fn holding(s: &Session<FakeLoader>, id: TabId) -> Vec<&'static str> {
    let parts = [
        ("tabs", s.tabs.iter().any(|tab| tab.id == id)),
        ("active", s.active == Some(id)),
        ("warm", s.warm.contains(id)),
        ("woken", s.warm.holds_read_of(id)),
        ("edits", s.edits.contains_key(&id)),
        ("asleep", s.asleep.contains_key(&id)),
        ("wake_failed", s.wake_failed.contains(&id)),
        ("draws", s.mail.holds_draw_of(id)),
        ("journals", s.journals.contains_key(&id)),
        ("fresh_journals", s.fresh_journals.iter().any(|(tab, _, _)| *tab == id)),
        ("departing", s.departing.iter().any(|append| append.tab == id)),
        ("saving", s.saves.is_saving(id)),
        ("abandoned_saves", s.saves.abandoned_has(id)),
        ("comparison", s.comparison.as_ref().is_some_and(|comparison| comparison.contains(id))),
        ("generated", s.generated.view(id).is_some()),
        ("saved_over", s.saves.remembers(id)),
        ("jumps", s.mail.holds_jump_of(id)),
    ];
    parts
        .into_iter()
        .chain(s.dormancy.holding(id))
        .filter(|(_, holds)| *holds)
        .map(|(name, _)| name)
        .collect()
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
    s.note_drawn(DrawNote {
        tab: a,
        page: 0,
        failure: None,
    });
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
    s.note_drawn(DrawNote {
        tab: a,
        page: 0,
        failure: None,
    });
    s.set_generated_view(a, ready());
    assert_eq!(s.active(), Some(b));
    assert!(holding(&s, a).contains(&"warm"));
    assert!(s.go_dormant(a));
    assert_eq!(holding(&s, a), ["tabs"]);
}

/// Failure: what was read of a tab is kept once its file has turned out to be another, so the next look shows the old file's contents.
#[test]
fn forgetting_what_was_read_of_a_tab_forgets_all_of_it() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.set_generated_view(a, ready());
    s.forget_read(a);
    assert_eq!(holding(&s, a), ["tabs", "active"]);
    assert!(s.tab(a).unwrap().from_sleep.is_none());
}

/// Failure: a restore keeps something of the tabs of the session it replaces, so a restored tab with the same number, another file, takes it as its own.
#[test]
fn a_restore_keeps_nothing_of_the_tabs_it_replaces() {
    let mut s = journaled(10);
    let (a, b) = busy(&mut s);
    s.hold(a);
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
        s.give_woken(*id, Err(OpenError::NotFound), None);
    }
    assert_eq!(s.warm.reads_held(), MAX_READ_AHEAD);
    assert!(!s.holds_read_of(cold[0]), "the oldest read went");
    assert!(cold[1..].iter().all(|id| s.holds_read_of(*id)), "the newest are kept");
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
