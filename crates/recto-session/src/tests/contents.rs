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
    s.choose_generated(id, true);
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
    s.choose_generated(id, false);
    assert_eq!(note(&s), Some(("Show generated contents".into(), Some(ContentsAction::ShowGenerated))));
    assert!(
        s.sidebar().rows.iter().all(|row| row.kind != RowKind::Contents),
        "the file's own, which are none"
    );
    // A file changed underneath: the old version's list goes, as an agent's `contents --generated` would find none for the new one.
    s.choose_generated(id, true);
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
    s.go_dormant(id);
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
    s.choose_generated(id, true);
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
