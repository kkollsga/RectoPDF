use super::*;

/// Failure: a file with no contents of its own gets no offer, or the offer, the list while it's made and the generated list are told apart only by guessing. And a generated list must never take the place of the file's own once the reader goes back to them, nor follow a closed tab.
#[test]
fn generated_contents_are_offered_labelled_and_never_replace_the_files_own() {
    let mut s = session(40);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    assert_eq!(s.contents_verdict(id), Some(ContentsVerdict::Missing));
    fn note(s: &Session<FakeLoader>) -> Option<(String, Option<ContentsAction>)> {
        s.sidebar()
            .rows
            .iter()
            .find(|row| row.kind == RowKind::Note)
            .map(|row| (row.text.to_string(), row.action))
    }
    assert_eq!(note(&s), Some(("Generate contents".into(), Some(ContentsAction::Generate))));
    s.tabs_mut().choose_generated(id, true);
    s.set_generated_view(id, GeneratedView::Making);
    assert_eq!(note(&s).map(|(_, action)| action), Some(Some(ContentsAction::Stop)), "the reader can stop them");
    let lines = [
        TocLine {
            title: "1 Introduction",
            page: 0,
            parent: NONE,
            depth: 0,
        },
        TocLine {
            title: "2 Method",
            page: 5,
            parent: NONE,
            depth: 0,
        },
    ];
    s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
    let model = s.sidebar();
    assert_eq!(
        model
            .rows
            .iter()
            .filter(|row| row.kind == RowKind::Contents)
            .map(|row| row.text)
            .collect::<Vec<_>>(),
        ["1 Introduction", "2 Method"]
    );
    assert_eq!(
        note(&s),
        Some(("Generated contents".into(), None)),
        "labelled, and with no contents of its own there's nothing to go back to"
    );
    assert_eq!(model.rail_filled, 1, "the rail counts entries, not the note");
    assert!(s.to_record().tabs[0].generated_contents, "the choice is kept");
    s.tabs_mut().choose_generated(id, false);
    assert_eq!(note(&s), Some(("Show generated contents".into(), Some(ContentsAction::ShowGenerated))));
    assert!(
        s.sidebar().rows.iter().all(|row| row.kind != RowKind::Contents),
        "the file's own, which are none"
    );
    // A file changed underneath: the old version's list goes, as an agent's `contents --generated` would find none for the new one.
    s.tabs_mut().choose_generated(id, true);
    s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
    s.apply_file_status(
        id,
        FileStatus::Changed(FileStamp {
            size: 8,
            mtime_ns: 8,
            device: 1,
            inode: 2,
        }),
    );
    let before = s.tabs().own_stamp(id);
    s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
    s.apply_file_status(
        id,
        FileStatus::Changed(FileStamp {
            size: 9,
            mtime_ns: 9,
            device: 1,
            inode: 2,
        }),
    );
    assert!(s.generated().view(id).is_none(), "an old version's list stayed");
    // A look that started before the change comes back after it: about the old version, so it's dropped.
    assert!(before.is_some() && s.tabs().own_stamp(id) != before);
    s.set_generated_view_for(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))), before);
    assert!(s.generated().view(id).is_none(), "a late answer about the old version was shown");
    s.set_generated_view_for(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))), s.tabs().own_stamp(id));
    assert!(s.generated().view(id).is_some(), "an answer about this version is shown");

    s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
    s.sleeper().go_dormant(id);
    assert!(s.generated().view(id).is_none(), "a dormant tab held its list");
    // Another tab shown: this one's list leaves memory.
    s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
    let other = s.open(Path::new("/d/b.pdf")).unwrap();
    s.tap(other).unwrap();
    assert!(s.generated().view(id).is_none(), "a tab off screen held its list");
    s.close(id).unwrap();
    assert!(s.generated().view(id).is_none());
}

/// Failure: a section's chevron doesn't fold it, or opens one following didn't, or the fold is lost across a restart, or a fold on a list survives switching to the other list, where its entry number means something else. Owner, 2026-11-07: "there should be a collapse toggle button for sections with sub chapters on the left side of the headers".
#[test]
fn contents_sections_fold_and_their_folds_come_back() {
    struct Outlined;
    impl DocLoader for Outlined {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            let lines = [
                TocLine {
                    title: "One",
                    page: 0,
                    parent: NONE,
                    depth: 0,
                },
                TocLine {
                    title: "One a",
                    page: 1,
                    parent: 0,
                    depth: 1,
                },
                TocLine {
                    title: "Two",
                    page: 5,
                    parent: NONE,
                    depth: 0,
                },
                TocLine {
                    title: "Two a",
                    page: 6,
                    parent: 2,
                    depth: 1,
                },
            ];
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::from_lines(&lines, false),
                renderer_only: false,
            })
        }
    }
    fn shown(s: &Session<Outlined>) -> Vec<(String, bool, bool)> {
        s.sidebar()
            .rows
            .iter()
            .filter(|row| row.kind == RowKind::Contents)
            .map(|row| (row.text.to_string(), row.expandable, row.expanded))
            .collect()
    }
    let mut s = Session::new(Outlined);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    // On page 1, in "One": it's open by following, and "Two" isn't.
    assert_eq!(
        shown(&s),
        [("One".into(), true, true), ("One a".into(), false, false), ("Two".into(), true, false)]
    );
    s.toggle_contents_section(id, 0);
    s.toggle_contents_section(id, 2);
    assert_eq!(
        shown(&s),
        [("One".into(), true, false), ("Two".into(), true, true), ("Two a".into(), false, false)]
    );
    s.toggle_contents_section(id, 1);
    assert_eq!(shown(&s).len(), 3, "an entry with nothing under it doesn't fold");
    let record = s.to_record();
    assert_eq!(record.tabs[0].contents_folds, [(0, false), (2, true)]);
    let mut again = Session::new(Outlined);
    again.restore(&record, HashMap::new()).unwrap();
    again.wake_active();
    assert_eq!(shown(&again), shown(&s), "the folds come back");
    // Another version of the file: its entries aren't these, so the folds go.
    again.apply_file_status(
        again.tabs().active().unwrap(),
        FileStatus::Changed(FileStamp {
            size: 9,
            mtime_ns: 9,
            device: 1,
            inode: 2,
        }),
    );
    assert!(
        again.to_record().tabs[0].contents_folds.is_empty(),
        "folds by entry number stayed for another version"
    );
    // Back to what following does is no fold at all.
    s.toggle_contents_section(id, 0);
    s.toggle_contents_section(id, 2);
    assert!(s.to_record().tabs[0].contents_folds.is_empty());
    s.toggle_contents_section(id, 2);
    s.tabs_mut().choose_generated(id, true);
    assert!(s.to_record().tabs[0].contents_folds.is_empty(), "the other list's entries aren't these");
}

/// Failure: a thousand tabs that each had generated contents asked for hold every list in memory.
#[test]
fn generated_lists_held_are_capped() {
    let mut s = session(40);
    let ids: Vec<TabId> = (0..20).map(|n| s.open(Path::new(&format!("/d/{n}.pdf"))).unwrap()).collect();
    for id in &ids {
        s.set_generated_view(*id, GeneratedView::Making);
    }
    assert!(s.generated.len() <= MAX_GENERATED_VIEWS);
    assert!(s.generated().view(*ids.last().unwrap()).is_some());
}

/// Failure: contents that are fine get an offer, so the sidebar nags about a good file.
#[test]
fn fine_contents_get_no_note() {
    assert_eq!(contents_note(ContentsVerdict::Fine, None, false, 12), None);
    assert!(contents_note(ContentsVerdict::LooksBad(recto_core::verdict::BadReason::TooFew), None, false, 1).is_some());
}

/// **Failure: stopping generated contents leaves the tab and what's known of its generated contents out of step:** the tab still choosing generated ones
/// while nothing is being made, so the note says nothing and the file's own aren't shown, or the making forgotten while the tab still asks for them.
#[test]
fn stopping_generated_contents_shows_the_files_own_and_offers_them_again() {
    let mut s = session(40);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.tabs_mut().choose_generated(id, true);
    s.set_generated_view(id, GeneratedView::Making);
    s.stop_generated_contents(id);
    assert_eq!(s.generated().view(id), None, "where they stood is forgotten");
    assert!(!s.tabs().tab(id).unwrap().generated_contents, "the file's own are shown");
    let note = s.sidebar().rows.iter().find(|row| row.kind == RowKind::Note).map(|row| row.action);
    assert_eq!(note, Some(Some(ContentsAction::Generate)), "and offered again");
}

/// **Failure (R3.5b): a tab whose list is asleep, its wake having failed, has its contents read as if it had no edits**: each entry shows the file's page
/// number though the edits moved its page, the section being read is taken from the file's page at the reader's position, and following opens it. While
/// the order isn't known, no entry has a number, none is the current one, and following opens nothing.
#[test]
fn a_list_asleep_lends_the_contents_no_page_numbers() {
    struct Outlined;
    impl DocLoader for Outlined {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            let line = |title, page, parent, depth| TocLine { title, page, parent, depth };
            let lines = [line("One", 0, NONE, 0), line("Two", 5, NONE, 0), line("Two a", 6, 1, 1)];
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::from_lines(&lines, false),
                renderer_only: false,
            })
        }
    }
    let mut s = Session::new(Outlined);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(id, |e| e.move_pages(6, 1, 0)).unwrap();
    s.go_to(id, 6).unwrap();
    assert!(s.lists.put_to_sleep(id));
    let rows: Vec<(String, Option<u32>, bool, bool)> = s
        .sidebar()
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Contents)
        .map(|row| (row.text.to_string(), row.page, row.current, row.expanded))
        .collect();
    assert_eq!(rows, [("One".into(), None, false, false), ("Two".into(), None, false, false)]);
}

/// Failure: folding the section that holds the entry being read leaves no row marked, so the sidebar shows no place in the contents (finding 2 of the contents
/// fixes). The row of the nearest section that shows is current, opening it marks the entry itself again, and a top-level entry with nothing folded is as it was.
#[test]
fn a_folded_section_holding_the_current_entry_is_current_itself() {
    struct Outlined;
    impl DocLoader for Outlined {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            let line = |title, page, parent, depth| TocLine { title, page, parent, depth };
            let lines = [
                line("One", 0, NONE, 0),
                line("One a", 1, 0, 1),
                line("One a i", 2, 1, 2),
                line("Two", 5, NONE, 0),
            ];
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::from_lines(&lines, false),
                renderer_only: false,
            })
        }
    }
    fn current(s: &Session<Outlined>) -> (Vec<String>, usize) {
        let model = s.sidebar();
        let rows = model.rows.iter().filter(|row| row.kind == RowKind::Contents);
        (rows.filter(|row| row.current).map(|row| row.text.to_string()).collect(), model.rail_filled)
    }
    let mut s = Session::new(Outlined);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    assert_eq!(current(&s), (vec!["One".to_string()], 1), "top level, nothing folded");
    s.go_to(id, 2).unwrap();
    assert_eq!(current(&s), (vec!["One a i".to_string()], 3));
    s.toggle_contents_section(id, 1);
    assert_eq!(current(&s), (vec!["One a".to_string()], 2), "the nearest section that shows");
    s.toggle_contents_section(id, 0);
    assert_eq!(current(&s), (vec!["One".to_string()], 1), "the outermost fold wins");
    s.toggle_contents_section(id, 0);
    s.toggle_contents_section(id, 1);
    assert_eq!(current(&s), (vec!["One a i".to_string()], 3), "opened again, the entry itself");
}

/// Failure (finding 3 of the contents fixes): the current entry is chosen by page, so at the top of a page that holds several entries the last of them
/// (further down the page) is marked and the contents scroll past the ones above it. The last entry at or above the reader's place is the current one: on
/// an earlier page, or on this page and no further down than the reader. The same for a file's own contents and for generated ones, which share the one
/// function; and entries with no place keep the last of their page.
#[test]
fn the_current_entry_is_the_last_at_or_above_the_place_own_and_generated() {
    struct Placed;
    fn toc(tops: &[u16]) -> Toc {
        let line = |title, page| TocLine {
            title,
            page,
            parent: NONE,
            depth: 0,
        };
        Toc::from_lines(&[line("Before", 0), line("A", 1), line("B", 1), line("C", 1), line("After", 2)], false).with_tops(tops)
    }
    impl DocLoader for Placed {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: toc(&[0, 0, 300, 600, 0]),
                renderer_only: false,
            })
        }
    }
    fn current(s: &Session<Placed>) -> Vec<String> {
        s.sidebar()
            .rows
            .iter()
            .filter(|row| row.kind == RowKind::Contents && row.current)
            .map(|row| row.text.to_string())
            .collect()
    }
    let mut s = Session::new(Placed);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    for (within, expected) in [(0.0, "A"), (0.29, "A"), (0.30, "B"), (0.45, "B"), (0.61, "C"), (0.99, "C")] {
        s.jump(id, 1, within).unwrap();
        assert_eq!(current(&s), [expected], "{within} of the way down page 2");
    }
    s.jump(id, 2, 0.0).unwrap();
    assert_eq!(current(&s), ["After"], "the next page's entry once the reader's there");
    // Generated ones: the same function, with the places the helper found.
    s.tabs_mut().choose_generated(id, true);
    s.set_generated_view(id, GeneratedView::Ready(Arc::new(toc(&[0, 0, 500, 900, 0]))));
    for (within, expected) in [(0.0, "A"), (0.51, "B"), (0.95, "C")] {
        s.jump(id, 1, within).unwrap();
        assert_eq!(current(&s), [expected], "generated, {within} of the way down");
    }
    // No places: the last of the page wherever the reader is on it, as it always was.
    s.set_generated_view(id, GeneratedView::Ready(Arc::new(toc(&[]))));
    s.jump(id, 1, 0.0).unwrap();
    assert_eq!(current(&s), ["C"]);
}

/// A session on a file whose page 2 holds three entries (at 0, 300 and 600 thousandths) and page 3 one, which is on page 2.
fn reading_page_two() -> (Session<ThreeToAPage>, TabId) {
    let mut s = Session::new(ThreeToAPage);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.jump(id, 1, 0.0).unwrap();
    // The page view has taken the jump and arrived: what it says from here is the reader's own.
    s.mail_mut().take_jump(id);
    (s, id)
}

struct ThreeToAPage;
impl DocLoader for ThreeToAPage {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        let line = |title, page| TocLine {
            title,
            page,
            parent: NONE,
            depth: 0,
        };
        let toc = Toc::from_lines(&[line("A", 1), line("B", 1), line("C", 1), line("D", 2)], false).with_tops(&[0, 300, 600, 0]);
        Ok(DocFacts {
            page_count: 10,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc,
            renderer_only: false,
        })
    }
}

fn current_of<L: DocLoader>(s: &Session<L>) -> Vec<String> {
    s.sidebar()
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Contents && row.current)
        .map(|row| row.text.to_string())
        .collect()
}

/// The page view's read of a tab on page 2 (stamped `stamp`): the top of its view is `top` of the way down page `anchor` (counted from 0), and the page
/// number says page 2 (the eye line's, a third of the view further down).
fn view_reads(s: &mut Session<ThreeToAPage>, id: TabId, stamp: u64, anchor: u32, top: f64) {
    let place = PlaceRecord {
        page: 1,
        anchor_page: anchor,
        within: top,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(id, place, stamp));
}

/// Failure (the review of finding 3): current by place updates only on a jump, so reading down a page by scroll leaves the mark on the first entry to the
/// foot of the page. The view reports places, not jumps, and each read moves the mark to the entry that has reached the top of the view. The band says where
/// the mark next changes, and goes on following it.
#[test]
fn reading_down_a_page_by_scroll_marks_a_then_b_then_c_as_each_reaches_the_top() {
    let (mut s, id) = reading_page_two();
    assert_eq!(current_of(&s), ["A"]);
    view_reads(&mut s, id, 1, 1, 0.20);
    assert_eq!(current_of(&s), ["A"], "B hasn't reached the top");
    let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
    assert_eq!((band.page, band.lo, band.hi), (1, 0, 300 - recto_core::TOP_SLACK));
    view_reads(&mut s, id, 2, 1, 0.31);
    assert_eq!(current_of(&s), ["B"]);
    let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
    assert_eq!((band.lo, band.hi), (300 - recto_core::TOP_SLACK, 600 - recto_core::TOP_SLACK));
    view_reads(&mut s, id, 3, 1, 0.70);
    assert_eq!(current_of(&s), ["C"]);
    assert_eq!(
        reading_band(&s, s.tabs().tab(id).unwrap()).map(|band| band.hi),
        Some(recto_core::TOP_SCALE + 1),
        "no entry below C on the page"
    );
    view_reads(&mut s, id, 4, 1, 0.0);
    assert_eq!(current_of(&s), ["A"], "back up the page");
}

/// Failure: the line is the page the place's anchor is on, but at the foot of a page the page number says the next while the top of the view is still on this
/// one, so the mark is taken from the page number's page, and an entry far up the next page is current while the reader's top line is in the one before.
#[test]
fn at_the_foot_of_a_page_the_top_of_the_view_is_still_on_it() {
    let (mut s, id) = reading_page_two();
    s.go_to(id, 2).unwrap();
    // The page number says page 3 (D's), the top of the view is 0.9 of the way down page 2.
    let place = PlaceRecord {
        page: 2,
        anchor_page: 1,
        within: 0.9,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(id, place, 1));
    assert_eq!(current_of(&s), ["C"], "D is on the page below the top line");
    let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
    assert_eq!(band.page, 1, "the band is on the page at the top of the view");
}

/// Failure: the entry jumped to stops being the current one when the view reports back, or a place told about another page than the tab's (an older read, a
/// move since) is taken for where the reader is. After a click with the next entry one line below it, the clicked one stays; a jump sent is where the reader
/// is whatever the last read said; and on another page an old place is none.
#[test]
fn the_entry_jumped_to_stays_current_when_the_view_reports_back() {
    struct Close;
    impl DocLoader for Close {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            let line = |title, page| TocLine {
                title,
                page,
                parent: NONE,
                depth: 0,
            };
            let toc = Toc::from_lines(&[line("A", 1), line("B", 1), line("C", 2)], false).with_tops(&[300, 310, 0]);
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc,
                renderer_only: false,
            })
        }
    }
    let mut s = Session::new(Close);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.jump(id, 1, 0.300).unwrap();
    let names = |s: &Session<Close>| -> Vec<String> {
        s.sidebar()
            .rows
            .iter()
            .filter(|row| row.kind == RowKind::Contents && row.current)
            .map(|row| row.text.to_string())
            .collect()
    };
    assert_eq!(names(&s), ["A"], "the jump sent");
    s.mail_mut().take_jump(id);
    let place = |within: f64| PlaceRecord {
        page: 1,
        anchor_page: 1,
        within,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(id, place(0.300), 1));
    assert_eq!(names(&s), ["A"], "the view arrived at A's top: B, a line below, hasn't reached it");
    assert!(s.view_told_of(id, place(0.310), 2));
    assert_eq!(names(&s), ["B"], "and when B reaches the top, it's B");
    s.jump(id, 1, 0.300).unwrap();
    assert_eq!(names(&s), ["A"], "a jump sent is where the reader is, whatever the last read said");
    s.mail_mut().take_jump(id);
    s.go_to(id, 2).unwrap();
    assert_eq!(names(&s), ["C"], "on another page, the old place is none: the top of the page it moved to");
}

/// Failure: a page with an unsaved turn is read by tops that are in the file's frame, so the wrong entry is current there, or the band (and so every
/// scroll step's wake) comes from tops that mean nothing. Every entry reads as at the page's top, and the band is the whole page.
#[test]
fn a_page_with_an_unsaved_turn_is_unplaced_and_has_no_band_to_leave() {
    let (mut s, id) = reading_page_two();
    view_reads(&mut s, id, 1, 1, 0.0);
    assert_eq!(current_of(&s), ["A"], "placed: the top of the view is at A");
    s.edit_pages(id, |edits| edits.rotate(1, 1, 1)).unwrap();
    assert_eq!(current_of(&s), ["C"], "unplaced: the last of the page wherever the view is");
    let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
    assert_eq!((band.lo, band.hi), (0, recto_core::TOP_SCALE + 1));
    assert!(
        s.sidebar().rows.iter().filter(|row| row.kind == RowKind::Contents).all(|row| row.top == 0),
        "and a click on one goes to the page's top"
    );
}

/// Page 2 holding A and B, at the places given, and page 3 C.
struct AtTops(&'static [u16]);
impl DocLoader for AtTops {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        let line = |title, page| TocLine {
            title,
            page,
            parent: NONE,
            depth: 0,
        };
        Ok(DocFacts {
            page_count: 10,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::from_lines(&[line("A", 1), line("B", 1), line("C", 2)], false).with_tops(self.0),
            renderer_only: false,
        })
    }
}

/// Failure (S5 of the second review): the session turned the view's place into thousandths in `f32` and the page view in `f64`, which round some halves
/// apart: 0.5005 of a page is 501 in `f32` and 500 in `f64`. With B's top at 506, the session marked B and sent B's band, from 501, while the page view,
/// reading 500, was outside it and woke the core, which sent the same band: a wake every pass while the view rested there. One rounding for both: the
/// band the session sends holds the top of the view as the page view reads it.
#[test]
fn the_session_and_the_page_view_round_a_place_alike() {
    let mut s = Session::new(AtTops(&[0, 506]));
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.jump(id, 1, 0.0).unwrap();
    s.mail_mut().take_jump(id);
    let within = 0.5005;
    let place = PlaceRecord {
        page: 1,
        anchor_page: 1,
        within,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(id, place, 1));
    // As the page view reads it, in `f64`.
    let seen = (within * 1000.0).round() as u16;
    assert_eq!(seen, 500);
    let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
    assert!(band.lo <= seen && seen < band.hi, "the page view at {seen} is outside the band {band:?}");
    assert_eq!(current_of(&s), ["A"], "B, at 506, is 6 below the top of the view");
}

/// Failure (S1 of the second review): the session took the current entry from the place's top, which can't come within a view-height of the document's
/// end, and not from the line the page view says with it, so the last entries were never current by scrolling, whatever the page view said. The line is
/// the page view's word: it decides the mark and the band, and a line that moves with the place where it was is nothing to write (the record's place
/// is as it was, and no line of the log of places is asked for).
#[test]
fn the_line_the_page_view_says_finds_the_current_entry_and_is_never_written() {
    let (mut s, id) = reading_page_two();
    let place = PlaceRecord {
        page: 1,
        anchor_page: 1,
        within: 0.1,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(id, ViewSaid { place, line: (1, 650) }, 1));
    assert_eq!(current_of(&s), ["C"], "the line is below C's top, the top of the view above B's");
    let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
    assert_eq!((band.page, band.lo), (1, 600 - recto_core::TOP_SLACK), "and the band is C's");
    let delta = s.take_place_delta(1);
    assert_eq!(delta.places, [(id.0, 1, Some(place))], "the place as the page view said it, no line in it");
    assert!(s.view_told_of(id, ViewSaid { place, line: (1, 100) }, 2));
    assert_eq!(current_of(&s), ["A"]);
    assert!(s.take_place_delta(2).places.is_empty(), "only the line moved: nothing to write");
}

/// The page view as it reads after a jump has landed, or after the person's scroll: the reader on `page` (counted from 0), the top of the view `within`
/// down page `anchor`, and the line the page view says with it.
fn view_says(s: &mut Session<impl DocLoader>, id: TabId, stamp: u64, page: u32, anchor: (u32, f64), line: (u32, u16)) {
    let place = PlaceRecord {
        page,
        anchor_page: anchor.0,
        within: anchor.1,
        x: 0.0,
        y: 0.0,
        document_height: 1.0,
        zoom: 1.0,
        page_width: 0.0,
        page_height: 0.0,
    };
    assert!(s.view_told_of(id, ViewSaid { place, line }, stamp));
}

/// A click on a contents row: a jump to the entry's page and place, naming the entry, sent to the page view by the look with the place clock at `stamp`.
fn click_entry(s: &mut Session<impl DocLoader>, id: TabId, page: u32, within: f64, entry: u32, stamp: u64) -> trail::Stop {
    s.jump(id, page, within).unwrap();
    s.mail_mut().jump_is_to_entry(id, entry);
    let to = s.mail().waiting_jump(id).expect("a jump waiting");
    s.mail_mut().jump_sent(id, to, stamp);
    to
}

/// Failure (S3 of the second review): the entry clicked is found again by its place when the view reports back, so with another entry at the same place
/// (ties go to the later) or a few thousandths below it (inside the slack for a pixel's rounding) the other one is marked, not the one clicked. The
/// clicked entry stays current until the person scrolls; one step by hand and the place decides again.
#[test]
fn the_entry_clicked_stays_current_until_a_scroll_by_hand_with_another_at_or_just_below_its_place() {
    for (tops, case) in [(&[300, 300][..], "at the same place"), (&[300, 303][..], "3 thousandths below")] {
        let mut s = Session::new(AtTops(tops));
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        let to = click_entry(&mut s, id, 1, 0.3, 0, 5);
        assert_eq!(current_of(&s), ["A"], "{case}: on its way");
        view_says(&mut s, id, 6, 1, (1, f64::from(to.within)), (1, 300));
        assert_eq!(current_of(&s), ["A"], "{case}: the view landed where the click sent it");
        let band = reading_band(&s, s.tabs().tab(id).unwrap()).expect("a band");
        assert_eq!((band.page, band.lo, band.hi), (1, 300, 301), "{case}: the person's first step leaves it");
        view_says(&mut s, id, 7, 1, (1, 0.31), (1, 310));
        assert_eq!(current_of(&s), ["B"], "{case}: one step down by hand, and the place decides");
    }
}

/// Failure (S3): in an outline whose destinations say no place (`/Fit`), every entry of a page reads as at its top and the last of them is current, so
/// clicking the first of three entries on a page marks the third. The clicked one is marked; scrolling, the last of the page is, as before.
#[test]
fn clicking_the_first_of_three_unplaced_entries_on_a_page_marks_it() {
    struct ThreeUnplaced;
    impl DocLoader for ThreeUnplaced {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            let line = |title, page| TocLine {
                title,
                page,
                parent: NONE,
                depth: 0,
            };
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::from_lines(&[line("A", 1), line("B", 1), line("C", 1)], false),
                renderer_only: false,
            })
        }
    }
    let mut s = Session::new(ThreeUnplaced);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    click_entry(&mut s, id, 1, 0.0, 0, 5);
    view_says(&mut s, id, 6, 1, (1, 0.0), (1, 0));
    assert_eq!(current_of(&s), ["A"], "the one clicked");
    view_says(&mut s, id, 7, 1, (1, 0.2), (1, 200));
    assert_eq!(current_of(&s), ["C"], "scrolled: the last of the page, as with no places");
}

/// Failure (S3): an entry in the last view-height of the document can't be brought to the top of the view; the page view stops at the end and says the
/// last page, the line is the end of the document, and the last entry is marked, not the one clicked. It stays marked through the view's report and the
/// page it says, until a scroll by hand; and Back to the place it left lands by place, without the entry.
#[test]
fn an_entry_clicked_near_the_end_stays_marked_until_a_scroll_by_hand_and_back_doesnt_revive_it() {
    let (mut s, id) = reading_page_two();
    let to = click_entry(&mut s, id, 1, 0.3, 1, 5);
    assert_eq!(s.here(id), Some(trail::Stop::new(1, 0.3)), "the trail's place is the jump's, entry or none");
    // Arrived where the view stops: the page view says the last page (counted from 0, 9) and the end of the document as its line, the top of its
    // view where the jump put it.
    s.go_to(id, 9).unwrap();
    view_says(&mut s, id, 6, 9, (1, f64::from(to.within)), (9, 1000));
    assert_eq!(current_of(&s), ["B"], "the one clicked, not D at the end");
    // Scrolled up a step by hand.
    view_says(&mut s, id, 7, 9, (1, 0.29), (9, 990));
    assert_eq!(current_of(&s), ["D"], "the place decides again");
    // Back and forth: Back lands at the stop the click made, by place.
    let mut s = Session::new(AtTops(&[300, 300]));
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    let to = click_entry(&mut s, id, 1, 0.3, 0, 5);
    view_says(&mut s, id, 6, 1, (1, f64::from(to.within)), (1, 300));
    assert_eq!(current_of(&s), ["A"], "A clicked, B at its place");
    s.jump(id, 2, 0.0).unwrap();
    assert!(s.step_as(id, true, Author::Owner).unwrap(), "back");
    assert_eq!(current_of(&s), ["B"], "Back goes by place: the later of two at one place");
}
