use super::*;

/// What a state record's store does, in memory: journals by name, read back as a launch would.
struct MemorySource(Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>);

impl HistorySource for MemorySource {
    fn history_of(&self, _stamp: Option<FileStamp>, _path: &Path) -> Option<TakenHistory> {
        None
    }
    fn read_list(&self, name: &str) -> Option<RestoredEdits> {
        let files = self.0.lock().unwrap();
        let parsed = record::parse_journal(files.get(name)?, record::CURRENT_VERSION).ok()?;
        let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).ok()?;
        Some(RestoredEdits {
            edits,
            seq: parsed.header.base_seq + applied as u64,
            lost_lines: false,
            rolled_off: false,
        })
    }
}

/// A session with a list on tab `a`, in its journal, and another tab showing.
fn with_a_list(files: &Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>) -> (Session<FakeLoader>, TabId, TabId) {
    let mut s = journaled(5);
    s.journals_mut().set_history(Arc::new(MemorySource(Arc::clone(files))));
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(s.undo_edit(a), "with something to redo as well");
    let mut plain = files.lock().unwrap().clone();
    write_journal(&mut s, &mut plain);
    *files.lock().unwrap() = plain;
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    (s, a, b)
}

/// Failure (decision 25): a list gone to disk reads as if the tab had none, so its edits look saved, or a close loses them, or an undo finds nothing, or pages come from the wrong place. Every route about a tab finds the list as it was left, and what's in memory till then is only a summary.
#[test]
fn a_dormant_tabs_list_goes_to_disk_and_comes_back_by_every_route() {
    let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let (mut s, a, _b) = with_a_list(&files);
    let as_it_was = s.lists().edits_record(a).unwrap();
    assert!(s.go_dormant(a));
    assert!(s.lists().is_asleep(a));
    assert!(s.lists().get(a).is_none(), "out of memory");
    // A look at it, with nothing read.
    assert_eq!(s.pages().count(a), 4);
    assert!(s.lists().is_edited(a));
    assert!(s.lists().holds_owner_edits());
    assert_eq!(s.to_record().tabs[0].edits.unwrap().pages, 4);
    assert_eq!(s.lists().undo_and_redo(a), (true, true), "and what can be undone, with nothing read");
    assert!(s.to_record().tabs[0].journal.is_some());
    assert_eq!(
        s.close(a),
        Err(SessionError::UnsavedChanges),
        "and a close is refused as it would be: it wakes it to see"
    );
    assert_eq!(s.list_record_anywhere(a), Some(as_it_was.clone()));
    // Each way in.
    type Route = fn(&mut Session<FakeLoader>, TabId);
    let routes: [(&str, Route); 9] = [
        ("tap", |s, a| {
            s.tap(a).unwrap();
        }),
        ("edit", |s, a| s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap()),
        ("undo", |s, a| assert!(s.undo_edit(a))),
        ("redo", |s, a| assert!(s.redo_edit(a))),
        ("agent's undo", |s, a| assert!(s.undo_edit_as(a, Author::Agent).unwrap())),
        ("discard", |s, a| s.discard_edits(a)),
        ("clear", |s, a| assert!(s.clear_history(a, ClearReason::UserAsked))),
        ("save", |s, a| {
            assert_eq!(
                s.saved(
                    a,
                    FileStamp {
                        size: 9,
                        mtime_ns: 1,
                        device: 1,
                        inode: 2
                    },
                    4,
                    None,
                    None
                ),
                1
            )
        }),
        ("wake", |s, a| assert!(s.wake_list(a))),
    ];
    for (route, go) in routes {
        let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (mut s, a, _b) = with_a_list(&files);
        let mut twin = {
            let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
            with_a_list(&files).0
        };
        assert!(s.go_dormant(a));
        go(&mut s, a);
        go(&mut twin, a);
        assert!(!s.lists().is_asleep(a), "{route}");
        assert_eq!(s.lists().edits_record(a), twin.lists().edits_record(a), "{route}");
        assert_eq!(s.lists().is_edited(a), twin.lists().is_edited(a), "{route}");
        assert_eq!(s.pages().count(a), twin.pages().count(a), "{route}");
    }
    // And what it goes on to write follows on from the journal, so a crash after that replays exactly.
    let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let (mut s, a, _b) = with_a_list(&files);
    assert!(s.go_dormant(a));
    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    let mut plain = files.lock().unwrap().clone();
    write_journal(&mut s, &mut plain);
    let parsed = record::parse_journal(plain.values().next().unwrap(), record::CURRENT_VERSION).unwrap();
    let (replayed, _) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
    assert_eq!(replayed.to_record(), s.lists().edits_record(a).unwrap());
}

/// Failure: a list goes out of memory with some of it not on disk, or its journal's gone and it's lost in silence. It stays until it's all there, and if the journal can't be read the tab says so.
#[test]
fn a_list_only_sleeps_with_all_of_it_on_disk() {
    let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let (mut s, a, _b) = with_a_list(&files);
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(s.list_to_sleep(a).is_none(), "a step hasn't gone out");
    assert!(!s.go_dormant(a));
    assert!(s.lists().get(a).is_some());
    let mut plain = files.lock().unwrap().clone();
    write_journal(&mut s, &mut plain);
    *files.lock().unwrap() = plain;
    assert!(s.go_dormant(a));
    // The journal can't be read just now. **It stays asleep, by its name, and nothing's laid on what can't be seen,** which would land on the wrong pages (review L3). The tab says so, and every look at it tries again.
    let (name, bytes) = files.lock().unwrap().drain().next().unwrap();
    assert!(!s.wake_list(a));
    assert!(s.lists().is_asleep(a));
    assert_eq!(
        s.tabs().tab(a).unwrap().file.notice.as_ref().map(|notice| notice.code()),
        Some("edits_unreadable")
    );
    assert!(s.lists().is_edited(a), "what's known of it stands");
    assert_eq!(
        s.edit_pages(a, |e| e.rotate(0, 1, 1)),
        Err(SessionError::EditsUnreadable),
        "a new edit isn't taken"
    );
    assert!(!s.undo_edit(a));
    assert_eq!(s.undo_edit_as(a, Author::Owner), Err(SessionError::EditsUnreadable));
    assert_eq!(s.close(a), Err(SessionError::UnsavedChanges));
    assert!(
        s.to_record().tabs[0].journal.as_deref() == Some(name.as_str()),
        "and its name's kept, so nothing else takes it"
    );
    // It's back.
    files.lock().unwrap().insert(name.clone(), bytes);
    assert!(s.wake_list(a));
    assert!(s.lists().get(a).is_some());
    assert_eq!(s.tabs().tab(a).unwrap().file.notice, None, "and it's stopped saying so");
    // Gone for good, and given up on by saying so.
    assert!(s.go_dormant(a));
    files.lock().unwrap().clear();
    assert!(!s.wake_list(a));
    s.discard_edits(a);
    assert!(!s.lists().is_asleep(a) && !s.lists().is_edited(a));
    assert_eq!(s.tabs().tab(a).unwrap().file.notice, None);
    assert!(s.journal_work().retired.contains(&name));
    // A session that keeps no journal keeps its lists.
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.open(Path::new("/d/b.pdf")).unwrap();
    assert!(!s.go_dormant(a));
    assert!(s.lists().get(a).is_some());
}

/// Failure (review): a sleep file's pictures match a list they're not about, as a count of edits starts again after a discard. What's compared is what's on the pages, so one order is one mark, however it was reached, and back at the file it's the mark of no list at all.
#[test]
fn a_list_is_known_by_whats_on_its_pages() {
    let mut one = PageEdits::new(5);
    assert_eq!(one.order_mark(), 0);
    one.rotate(0, 1, 1).unwrap();
    let rotated = one.order_mark();
    assert_ne!(rotated, 0);
    let mut other = PageEdits::new(5);
    other.delete(0, 1).unwrap();
    assert_ne!(other.order_mark(), rotated, "the same number of edits, and another document");
    assert!(one.undo());
    assert_eq!(one.order_mark(), 0, "back at the file");
    assert!(one.redo());
    assert_eq!(one.order_mark(), rotated, "the same as it was");
    // A move and a move back, by other steps.
    one.move_pages(1, 1, 2).unwrap();
    one.move_pages(2, 1, 1).unwrap();
    assert_eq!(one.order_mark(), rotated);
}

/// Failure (review): `session sleep` puts a tab to sleep from under whatever's working on it, as only the deadline looks at holds.
#[test]
fn nothing_sends_a_tab_to_sleep_from_under_a_hold() {
    let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let (mut s, a, b) = with_a_list(&files);
    s.dormancy_mut().hold(a);
    assert!(!s.dormant().may_go(a));
    assert!(!s.go_dormant(a));
    assert!(s.lists().get(a).is_some() && !s.lists().is_asleep(a));
    assert!(s.warm.contains(a), "its contents stay too");
    s.let_go(a);
    assert!(!s.go_dormant(b), "nor the one on screen");
    s.save_prepared(
        a,
        FileStamp {
            size: 9,
            mtime_ns: 1,
            device: 1,
            inode: 2,
        },
        None,
    );
    assert!(!s.go_dormant(a), "nor one being saved");
}

/// Failure (review): a sleep file's word on a file's length goes on being believed once the worker has said otherwise, so a phantom page takes a rotation, or it's written into the next sleep file as if it had been checked. And a file read the usual way is left alone, as two readers of a damaged file can differ.
#[test]
fn the_workers_word_on_a_files_length_goes_over_a_sleep_files() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-file-length-");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.pdf");
    std::fs::write(&path, b"three pages").unwrap();
    let stamp = file_stamp(&path).unwrap();
    let mut s = session(3);
    let a = s.open(&path).unwrap();
    assert!(!s.file_length_is(a, 2, 5), "read the usual way, that's nothing to go on");
    s.open(Path::new("/d/b.pdf")).unwrap();
    s.warm.drop_contents(a);
    // Forged: a hundred pages.
    let forged = |file_pages: u32| sleep::SleepFile {
        file: stamp,
        file_pages,
        pages: file_pages,
        list_seq: 0,
        renderer_only: false,
        version: Some(5),
        toc: Toc::default(),
        usual: None,
        sizes: Vec::new(),
        pictures: Vec::new(),
    };
    s.wake_from_sleep(a, &forged(100));
    s.tap(a).unwrap();
    assert_eq!(s.pages().count(a), 100, "believed for now");
    assert!(s.sleep_facts(a).is_none(), "and not written on, unchecked");
    assert!(s.file_length_is(a, 3, 5), "it says to read the file");
    assert!(!s.warm.contains(a));
    assert!(
        s.journal_work().retired.iter().any(|name| *name == record::sleep_file_name(stamp, &path)),
        "and the file's gone"
    );
    s.give_woken(
        a,
        Ok(DocFacts {
            page_count: 3,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        }),
        Some(stamp),
    );
    s.wake_active();
    assert_eq!(s.pages().count(a), 3);
    assert_eq!(
        s.edit_pages(a, |e| e.rotate(50, 1, 1)),
        Err(SessionError::Edit(EditError::OutOfRange)),
        "no phantom page takes a rotation"
    );
    // Right about it: nothing to do, and from here it's been checked.
    s.discard_edits(a);
    s.open(Path::new("/d/c.pdf")).unwrap();
    s.warm.drop_contents(a);
    s.wake_from_sleep(a, &forged(3));
    s.tap(a).unwrap();
    assert!(!s.file_length_is(a, 3, 5));
    assert!(s.sleep_facts(a).is_some());
    // The right length, and another version: a rewrite that kept its size, time and number. Read after all.
    s.open(Path::new("/d/d.pdf")).unwrap();
    s.warm.drop_contents(a);
    s.wake_from_sleep(a, &forged(3));
    s.tap(a).unwrap();
    assert!(s.file_length_is(a, 3, 6), "another version");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure: going dormant takes a timer per tab, or a thousand cold tabs make it look along them, or the tab on screen or one being saved goes, or a tab being used goes anyway. There's one deadline, the soonest of the few tabs that hold anything.
#[test]
fn going_dormant_has_one_deadline_and_goes_by_being_left_alone() {
    const AFTER: u32 = 300;
    let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let (mut s, a, b) = with_a_list(&files);
    s.tabs_mut().touch_at(a, 100);
    assert_eq!(s.dormant().next_at(AFTER), Some(400));
    assert!(s.dormant().due(399, AFTER).is_empty());
    assert_eq!(s.dormant().due(400, AFTER), [a]);
    assert!(!s.dormant().due(10_000, AFTER).contains(&b), "the tab on screen doesn't go");
    s.tabs_mut().touch_at(a, 200);
    assert_eq!(s.dormant().next_at(AFTER), Some(500), "being used puts it off");
    // Being worked on.
    s.dormancy_mut().hold(a);
    assert!(s.dormant().due(10_000, AFTER).is_empty());
    assert!(s.dormant().oldest_first().is_empty(), "whatever's asking");
    s.let_go(a);
    assert_eq!(s.dormant().due(10_000, AFTER), [a]);
    // A save under way.
    s.save_prepared(
        a,
        FileStamp {
            size: 9,
            mtime_ns: 1,
            device: 1,
            inode: 2,
        },
        None,
    );
    assert!(s.dormant().due(10_000, AFTER).is_empty());
    // Cold tabs aren't candidates, however many there are.
    let mut s = session(5);
    let ids: Vec<TabId> = (0..1000).map(|at| s.open(Path::new(&format!("/d/{at}.pdf"))).unwrap()).collect();
    assert!(s.dormant().candidates().count() <= MAX_WARM + 1);
    s.go_dormant(ids[998]);
    assert!(!s.warm.contains(ids[998]), "its contents go");
    assert!(s.dormant().oldest_first().len() < MAX_WARM);
}

/// Failure: a protected PDF's contents go in a sleep file, in the clear, or a tab pushed out of the warm ones goes cold with no sleep file, so it wakes the slow way.
#[test]
fn a_tab_gone_cold_has_what_a_sleep_file_needs_unless_its_protected() {
    struct Protected;
    impl DocLoader for Protected {
        fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 3,
                byte_len: 1,
                encrypted: path.ends_with("locked.pdf"),
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-sleep-facts-");
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str| {
        let path = dir.join(name);
        std::fs::write(&path, b"a file").unwrap();
        path
    };
    let mut s = Session::new(Protected);
    s.dormancy_mut().set_sleep_files(true);
    let open = s.open(&file("open.pdf")).unwrap();
    let locked = s.open(&file("locked.pdf")).unwrap();
    assert!(s.sleep_facts(open).is_some());
    assert!(s.sleep_facts(locked).is_none(), "its contents don't go on disk");
    assert!(s.sleep_to_read(locked).is_none());
    for at in 0..MAX_WARM {
        s.open(&file(&format!("{at}.pdf"))).unwrap();
    }
    let left = s.take_left_warm();
    assert_eq!(
        left.iter().map(|facts| facts.id).collect::<Vec<_>>(),
        [open],
        "the unprotected one that left: {left:?}"
    );
    assert_eq!(left[0].sleep.file_pages, 3);
    assert!(s.take_left_warm().is_empty());
    // A protected file's pages in a plain document: its facts are fine, and no picture is.
    let plain = s.open(&file("plain.pdf")).unwrap();
    assert!(s.sleep_facts(plain).unwrap().pictures_allowed);
    let locked_file = file("locked.pdf");
    let facts = s.loader().load(&locked_file).unwrap();
    s.insert_pdf_with_facts(plain, Author::Owner, 0, &locked_file, &facts, file_stamp(&locked_file).ok())
        .unwrap();
    assert!(
        !s.sleep_facts(plain).unwrap().pictures_allowed,
        "a page from a protected file means no pictures"
    );
    assert!(s.lists().get(plain).unwrap().added_sources()[0].protected);
    // Carried through its journal, so it's known after a restart.
    let record = s.lists().edits_record(plain).unwrap();
    assert!(record.added[0].protected);
    assert!(PageEdits::from_record(&record).unwrap().has_protected_pages());
    assert!(s.undo_edit(plain));
    assert!(
        s.sleep_facts(plain).unwrap().pictures_allowed,
        "with those pages gone from the document, it's fine again"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure: a list naming a file of a length it's not about is believed, as a forged journal can say a save happened, and pages past the end are drawn or written. A file read at its own stamp and another length than the list says shows the list isn't about it.
#[test]
fn a_list_of_another_length_than_its_file_isnt_believed() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-forged-list-");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.pdf");
    std::fs::write(&file, b"five pages").unwrap();
    let mut s = journaled(5);
    let a = s.open(&file).unwrap();
    let other = s.open(&dir.join("other.pdf")).unwrap();
    // As a journal could say: a list about twelve pages, on this file.
    let mut list = PageEdits::new(12);
    list.journal_from_now();
    list.rotate(11, 1, 1).unwrap();
    s.take_up_history(a, "journal-00000000000000aa.log", list, 1);
    s.warm.clear_contents();
    s.tap(a).unwrap();
    assert!(s.lists().get(a).is_none(), "it's gone");
    assert_eq!(s.pages().count(a), 5);
    assert_eq!(
        s.tabs().tab(a).unwrap().file.notice.as_ref().map(|notice| notice.code()),
        Some("edits_unreadable")
    );
    let _ = other;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure: which tabs go dormant, and when, depends on something other than the tabs, the warm ones, the lists, the holds and what's on screen, so the
/// look needs the whole session (R3.5): built here from those alone. The tab on screen and a held one never go; a warm one goes once it's been left
/// long enough, oldest touched first.
#[test]
fn going_dormant_is_a_look_over_the_tabs_the_warm_ones_and_the_holds() {
    let mut tabs = registry::Tabs::default();
    for (id, touched) in [(1, 0), (2, 50), (3, 10), (4, 0)] {
        tabs.push(bare_tab(id, 5));
        tabs.tab_mut(TabId(id)).unwrap().touched = touched;
    }
    tabs.set_active(Some(TabId(1)));
    let mut warm = warm::Warm::default();
    for id in [1, 2, 3] {
        warm.push(TabId(id), Arc::new(Toc::default()), |_| {});
    }
    let lists = lists::Lists::default();
    let mut dormancy = dormancy::Dormancy::default();
    dormancy.hold(TabId(3));
    let look = Dormant {
        tabs: &tabs,
        warm: &warm,
        lists: &lists,
        dormancy: &dormancy,
        comparison: None,
    };
    assert!(!look.may_go(TabId(1)), "on screen");
    assert!(!look.may_go(TabId(3)), "held");
    assert!(look.may_go(TabId(2)));
    assert_eq!(
        look.next_at(30),
        Some(80),
        "the warm tab left, 30 after it was touched; tab 4 holds nothing to let go"
    );
    assert_eq!(look.due(79, 30), Vec::<TabId>::new());
    assert_eq!(look.due(80, 30), vec![TabId(2)]);
    dormancy.let_go(TabId(3));
    let look = Dormant {
        tabs: &tabs,
        warm: &warm,
        lists: &lists,
        dormancy: &dormancy,
        comparison: None,
    };
    assert_eq!(look.oldest_first(), vec![TabId(3), TabId(2)]);
}

/// Failure (review of the branch, round 2, F-c): a confined session wakes a dormant tab's list from its journal, and the journal names a PDF outside the
/// run's folders as one whose pages were inserted, so those pages come back and the outside file is read when they're drawn, searched or saved; the wall
/// on a restore and a taken-up history doesn't cover a wake. Woken, the list is unreadable, as a journal that can't be read is: the tab says so, it has no
/// list and no page from outside, and nothing was read to find that out.
#[test]
fn a_woken_list_naming_an_outside_file_is_unreadable() {
    let dir = recto_testpdf::TempDir::new("recto-confined-wake-");
    std::fs::create_dir(dir.join("run")).unwrap();
    let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let mut s = journaled(3);
    s.journals_mut().set_history(Arc::new(MemorySource(Arc::clone(&files))));
    let a = s.open(&dir.join("run/a.pdf")).unwrap();
    // Written before the session was confined, as a journal in the state folder can say anything.
    s.insert_pdf(a, 0, &dir.join("outside.pdf")).unwrap();
    let mut plain = HashMap::new();
    write_journal(&mut s, &mut plain);
    *files.lock().unwrap() = plain;
    s.open(&dir.join("run/b.pdf")).unwrap();
    assert!(s.go_dormant(a), "the list goes to its journal");
    s.confinement().confine(recto_platform::run_folders::RunFolders::of(&[&dir.join("run")]));
    let loads = s.loader().loads.get();
    assert!(!s.wake_list(a), "the list naming the outside file was woken");
    assert!(s.page_edits(a).is_none(), "it's in memory");
    assert_eq!(s.tabs().tab_notice(a).map(FileNotice::code), Some("edits_unreadable"));
    assert!(
        (0..s.pages_in(a)).all(|page| s.page_source(a, page).is_none_or(|source| source.path == dir.join("run/a.pdf"))),
        "a page comes from outside"
    );
    assert_eq!(s.loader().loads.get(), loads, "a file was read to wake it");
}
