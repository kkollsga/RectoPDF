//! A confined session's tabs: a file outside its folders never becomes one, by any route.

use super::*;
use recto_platform::run_folders::RunFolders;

/// A session confined to one temp folder, `run`, with `outside` beside it. Both real folders, as the rule goes by where paths really lead.
fn confined() -> (Session<FakeLoader>, recto_testpdf::TempDir) {
    let dir = recto_testpdf::TempDir::new("recto-confined-");
    std::fs::create_dir(dir.join("run")).unwrap();
    std::fs::create_dir(dir.join("outside")).unwrap();
    let s = session(3);
    s.confinement().confine(RunFolders::of(&[&dir.join("run")]));
    (s, dir)
}

/// Failure: a confined session (a quiet run's) opens a file outside its folders, by its own `open` or by facts read elsewhere and handed in, so a quiet
/// run started at Full reads the user's PDFs; or a file inside is refused. The read itself isn't made: the loader never sees the outside path.
#[test]
fn a_confined_session_opens_only_its_own_files() {
    let (mut s, dir) = confined();
    let outside = dir.join("outside/secret.pdf");
    assert_eq!(s.open(&outside), Err(SessionError::OutsideQuietRun));
    assert_eq!(s.loader().loads.get(), 0, "the outside file was read before it was refused");
    let facts = s.loader().load(&dir.join("run/a.pdf")).unwrap();
    assert_eq!(s.open_with_facts(&outside, facts.clone(), None, None), Err(SessionError::OutsideQuietRun));
    assert!(s.tabs().is_empty());
    assert!(s.open(&dir.join("run/a.pdf")).is_ok());
    assert!(s.open_with_facts(&dir.join("run/b.pdf"), facts, None, None).is_ok());
    assert_eq!(s.tabs().len(), 2);
    assert_eq!(SessionError::OutsideQuietRun.code(), "outside_quiet_run");
}

/// Failure: another PDF's pages are added to a confined session's tab from a file outside its folders, so its text is read through the tab.
#[test]
fn a_confined_session_adds_no_pages_from_outside() {
    let (mut s, dir) = confined();
    let tab = s.open(&dir.join("run/a.pdf")).unwrap();
    assert_eq!(s.insert_pdf(tab, 0, &dir.join("outside/b.pdf")), Err(SessionError::OutsideQuietRun));
    let facts = s.loader().load(&dir.join("run/a.pdf")).unwrap();
    assert_eq!(
        s.insert_pdf_with_facts(tab, Author::Agent, 0, &dir.join("outside/b.pdf"), &facts, None),
        Err(SessionError::OutsideQuietRun)
    );
    assert_eq!(s.pages_in(tab), 3, "nothing was added");
    assert!(s.insert_pdf(tab, 0, &dir.join("run/c.pdf")).is_ok());
}

/// Failure: a quiet run's record that names a file outside its folders brings that tab back, cold, to be read when it's tapped or asked about. The tabs
/// inside come back as they were.
#[test]
fn a_restored_record_naming_an_outside_file_doesnt_bring_it_back() {
    let dir = recto_testpdf::TempDir::new("recto-confined-");
    std::fs::create_dir(dir.join("run")).unwrap();
    let mut free = session(3);
    free.open(&dir.join("run/a.pdf")).unwrap();
    free.open(&dir.join("outside.pdf")).unwrap();
    free.open(&dir.join("run/b.pdf")).unwrap();
    let saved = free.to_record();
    let mut back = session(3);
    back.confinement().confine(RunFolders::of(&[&dir.join("run")]));
    back.restore(&saved, HashMap::new()).unwrap();
    let names: Vec<_> = back.tabs().iter().map(|tab| tab.name.to_string()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(back.tab_for_path(&dir.join("outside.pdf")).is_none());
}

/// Failure: a confined session's tab follows its file when it's found moved out of the run's folders, so the tab then reads and saves a file outside
/// them. It says its file is missing instead.
#[test]
fn a_confined_tab_doesnt_follow_its_file_out() {
    let (mut s, dir) = confined();
    let tab = s.open(&dir.join("run/a.pdf")).unwrap();
    let now = FileStamp {
        size: 1,
        mtime_ns: 1,
        device: 1,
        inode: 9,
    };
    s.apply_file_status(tab, FileStatus::Moved(dir.join("outside/a.pdf"), now));
    assert_eq!(s.path_of(tab), Some(dir.join("run/a.pdf")));
    assert_eq!(s.tab_notice(tab).map(FileNotice::code), Some("file_missing"));
    // Inside, it follows as ever.
    s.apply_file_status(tab, FileStatus::Moved(dir.join("run/moved.pdf"), now));
    assert_eq!(s.path_of(tab), Some(dir.join("run/moved.pdf")));
}

/// Failure: a second confinement widens the first, so whatever makes a session can be talked into a wider one later; or a session that was never
/// confined refuses anything.
#[test]
fn a_confinement_is_for_good_and_a_free_session_is_free() {
    let (mut s, dir) = confined();
    s.confinement().confine(RunFolders::of(&[dir.path()]));
    assert_eq!(s.open(&dir.join("outside/x.pdf")), Err(SessionError::OutsideQuietRun));
    let mut free = session(3);
    assert!(free.confinement().folders().is_none());
    assert!(free.open(Path::new("/d/anywhere.pdf")).is_ok());
}

/// Failure (review of the branch, F3): a confined tab's folder is swapped for a link out of the run's folders after the tab was opened, and everything
/// that reads or writes the tab's file later (a cold tab waking, a page's source for Find and exports, a save in place) goes where the link leads, as
/// the rule was asked only at the open. Asked again where the session hands the path out: none of them gets a path, a save is refused, and a look at
/// the file says it's missing. Put back, all is as it was.
#[cfg(unix)]
#[test]
fn a_folder_swapped_for_a_link_out_after_the_open_leads_nowhere() {
    let (mut s, dir) = confined();
    std::fs::create_dir(dir.join("run/docs")).unwrap();
    let tab = s.open(&dir.join("run/docs/a.pdf")).unwrap();
    s.open(&dir.join("run/b.pdf")).unwrap();
    assert!(s.path_of(tab).is_some() && s.page_source(tab, 0).is_some());
    std::fs::rename(dir.join("run/docs"), dir.join("run/docs-real")).unwrap();
    std::os::unix::fs::symlink(dir.join("outside"), dir.join("run/docs")).unwrap();
    assert_eq!(s.path_of(tab), None, "the tab's file is handed out through the link");
    assert!(s.page_source(tab, 0).is_none(), "a page's source is read through the link");
    assert!(s.wake_for_tap(tab).is_none(), "a wake reads through the link");
    assert_eq!(
        s.begin_save(tab, true).err(),
        Some(SessionError::OutsideQuietRun),
        "a save writes through the link"
    );
    let now = FileStamp {
        size: 1,
        mtime_ns: 1,
        device: 1,
        inode: 1,
    };
    s.apply_file_status(tab, FileStatus::Same(now));
    assert_eq!(s.tab_notice(tab).map(FileNotice::code), Some("file_missing"));
    // The tab is still the tab it was, by its name.
    assert!(s.tab_for_path(&dir.join("run/docs/a.pdf")).is_some());
    std::fs::remove_file(dir.join("run/docs")).unwrap();
    std::fs::rename(dir.join("run/docs-real"), dir.join("run/docs")).unwrap();
    assert!(s.path_of(tab).is_some() && s.page_source(tab, 0).is_some());
}

/// A loader that notes the path each read was made by.
struct Noting(std::cell::RefCell<Vec<PathBuf>>);

impl DocLoader for Noting {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        self.0.borrow_mut().push(path.to_path_buf());
        Ok(DocFacts {
            page_count: 1,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Failure (review of the branch, F3): a confined session checks where a file's name really leads and then reads by the name, so what the name leads to
/// by the time of the read (a folder swapped for a link in between) is what's read. The read is by the path the rule found, and the tab keeps the name it
/// was opened by; a free session reads by the name, asking nothing of the disk.
#[cfg(unix)]
#[test]
fn the_first_read_is_by_the_path_that_was_checked() {
    let dir = recto_testpdf::TempDir::new("recto-confined-");
    std::fs::create_dir(dir.join("run")).unwrap();
    std::fs::write(dir.join("run/real.pdf"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("run/real.pdf"), dir.join("run/link.pdf")).unwrap();
    let mut s = Session::new(Noting(std::cell::RefCell::default()));
    s.confinement().confine(RunFolders::of(&[&dir.join("run")]));
    let tab = s.open(&dir.join("run/link.pdf")).unwrap();
    let real = dir.join("run/real.pdf").canonicalize().unwrap();
    assert_eq!(s.loader().0.borrow().as_slice(), [real], "read by the name, not where the rule looked");
    assert_eq!(s.path_of(tab), Some(dir.join("run/link.pdf")), "the tab keeps its name");
    let mut free = Session::new(Noting(std::cell::RefCell::default()));
    free.open(Path::new("/d/as/named.pdf")).unwrap();
    assert_eq!(free.loader().0.borrow().as_slice(), [PathBuf::from("/d/as/named.pdf")]);
}

/// Failure (review of the branch, the replayed history): a record and journal in a quiet run's state folder name a PDF outside its folders as one whose
/// pages were inserted, and restoring them brings those pages back, so the outside file is read when they're drawn, searched or saved; the session's
/// wall saw only the tabs' own files. Restored into a confined session the tab comes back with its edits unreadable and no page from outside, and a closed
/// PDF's history naming one isn't taken up.
#[test]
fn a_replayed_history_brings_no_pages_from_outside() {
    let dir = recto_testpdf::TempDir::new("recto-confined-");
    std::fs::create_dir(dir.join("run")).unwrap();
    let mut free = journaled(3);
    let tab = free.open(&dir.join("run/a.pdf")).unwrap();
    free.insert_pdf(tab, 0, &dir.join("outside.pdf")).unwrap();
    let mut files = HashMap::new();
    write_journal(&mut free, &mut files);
    let mut back = journaled(3);
    back.confinement().confine(RunFolders::of(&[&dir.join("run")]));
    back.restore(&free.to_record(), back_from(&files)).unwrap();
    assert!(back.page_edits(tab).is_none(), "the list naming the outside file came back");
    assert_eq!(back.tab_notice(tab).map(FileNotice::code), Some("edits_unreadable"));
    assert!(
        (0..back.pages_in(tab)).all(|page| back.page_source(tab, page).is_none_or(|source| source.path == dir.join("run/a.pdf"))),
        "a page comes from outside"
    );
    // The same list, as a closed PDF's history taken up on its open.
    let restored = back_from(&files).into_values().next().unwrap();
    let mut opening = session(3);
    opening.confinement().confine(RunFolders::of(&[&dir.join("run")]));
    let facts = opening.loader().load(&dir.join("run/a.pdf")).unwrap();
    let taken = TakenHistory { name: "a".into(), restored };
    let reopened = opening.open_with_facts(&dir.join("run/a.pdf"), facts, None, Some(taken)).unwrap();
    assert!(opening.page_edits(reopened).is_none(), "the history naming the outside file was taken up");
}
