use super::*;

/// Failure: a PDF moved away after opening crashes or disappears, rather than staying open and saying why.
#[test]
fn a_tab_whose_file_vanished_stays_and_says_why() {
    let mut s = session(10);
    let gone = s.open(Path::new("/d/missing-later.pdf"));
    assert!(gone.is_err());
    let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
    // Rename the first tab's file underneath the session, then wake it from cold.
    s.tabs.tab_mut(ids[0]).unwrap().file.file_name = OsStr::new("missing.pdf").into();
    s.tap(ids[0]).unwrap();
    assert_eq!(s.tabs().tab(ids[0]).unwrap().problem, Some(OpenError::NotFound));
    assert_eq!(s.tabs().active(), Some(ids[0]));
}

/// Failure (review, against decision 23): a changed file with no edits stops saying so as soon as it's been read, as the stamp's moved on and being the same says nothing. With edits, it's about what they were made on, so being back at that is news.
#[test]
fn a_changed_file_goes_on_saying_so_when_its_read() {
    let now = FileStamp {
        size: 2,
        mtime_ns: 2,
        device: 1,
        inode: 3,
    };
    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.apply_file_status(a, FileStatus::Changed(now));
    assert_eq!(s.tabs().tab_notice(a).map(FileNotice::code), Some("file_changed"));
    s.apply_file_status(a, FileStatus::Same(now));
    assert_eq!(
        s.tabs().tab_notice(a).map(FileNotice::code),
        Some("file_changed"),
        "it's the new one, and that's what it's saying"
    );
    // Left, it's been said.
    s.open(Path::new("/d/b.pdf")).unwrap();
    s.tap(a).unwrap();
    s.tap(s.tabs()[1].id).unwrap();
    assert_eq!(s.tabs().tab_notice(a), None);
    // With edits, being the same again is the news.
    s.tap(a).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let was = s.tabs().own_stamp(a);
    s.apply_file_status(a, FileStatus::Changed(FileStamp { size: 9, ..now }));
    assert_eq!(s.tabs().tab_notice(a).map(FileNotice::code), Some("file_changed"));
    s.apply_file_status(a, FileStatus::Same(was.unwrap_or(now)));
    assert_eq!(s.tabs().tab_notice(a), None);
}

/// Failure: what became of a file isn't said, or its edits go, or the tab's still pointing at where the file isn't. Each case, with the words a reader sees and the edits kept. A moved file with edits keeps its journal's name, as that's by its identity.
#[test]
fn a_tab_says_what_became_of_its_file_and_keeps_its_edits() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let now = FileStamp {
        size: 9,
        mtime_ns: 9,
        device: 9,
        inode: 9,
    };
    s.apply_file_status(a, at("/e/moved.pdf"));
    assert_eq!(s.tabs().path_of(a).unwrap(), Path::new("/e/moved.pdf"));
    assert_eq!(&*s.tabs().tab(a).unwrap().name, "moved");
    assert!(s.notices().said(a).map(|said| said.message).unwrap().contains("moved"));
    assert!(s.lists().is_edited(a));
    s.apply_file_status(a, FileStatus::Changed(now));
    assert_eq!(s.tabs().tab_notice(a).unwrap().code(), "file_changed");
    assert_ne!(s.tabs().own_stamp(a), Some(now), "edits keep the version they were made on");
    s.apply_file_status(a, FileStatus::Missing);
    assert!(s.notices().said(a).map(|said| said.message).unwrap().contains("edits"));
    assert!(s.lists().is_edited(a));
    assert_eq!(s.sidebar().rows[0].notice, s.notices().said(a).map(|said| said.message));
    s.apply_file_status(a, FileStatus::Same(now));
    assert_eq!(s.tabs().tab_notice(a), None, "it came back");
    assert_eq!(s.tabs().own_stamp(a), Some(now));
    s.apply_file_status(a, FileStatus::NotLooked);
    assert_eq!(s.tabs().own_stamp(a), Some(now), "that says nothing");
    // Clean: changed takes the new version, and it's said once, on the way past.
    let mut clean = session(5);
    let b = clean.open(Path::new("/d/b.pdf")).unwrap();
    clean.open(Path::new("/d/c.pdf")).unwrap();
    clean.apply_file_status(b, FileStatus::Changed(now));
    assert_eq!(clean.tabs().own_stamp(b), Some(now));
    clean.tap(b).unwrap();
    assert!(clean.tabs().tab_notice(b).is_some());
    let c = clean.tabs()[1].id;
    clean.tap(c).unwrap();
    assert_eq!(clean.tabs().tab_notice(b), None, "left, so it's been said");
    // Moved onto a file another tab has: this one's the one that's gone.
    clean.apply_file_status(b, FileStatus::Moved(PathBuf::from("/d/c.pdf"), now));
    assert_eq!(clean.tabs().tab_notice(b).unwrap().code(), "file_missing");
    assert_eq!(clean.tabs().path_of(b).unwrap(), Path::new("/d/b.pdf"));
}
