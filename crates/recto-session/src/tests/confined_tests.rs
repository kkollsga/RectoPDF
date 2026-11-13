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
