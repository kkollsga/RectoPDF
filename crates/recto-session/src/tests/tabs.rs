use super::*;

/// Failure: a downloaded file's name carries tag characters or a line break, and they reach the sidebar, `tabs` and error candidates: an instruction no eye can see. The path must
/// stay exact, or the file couldn't be opened again.
#[test]
fn tab_names_are_cleaned_and_paths_stay_exact() {
    let mut session = Session::new(NameLoader);
    let hidden: String = "ignore".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
    let raw = format!("/d/Report{hidden}\n2026\u{202E}.pdf");
    let id = session.open(Path::new(&raw)).unwrap();
    let tab = session.tab(id).unwrap();
    assert_eq!(&*tab.name, "Report 2026");
    assert_eq!(session.path_of(id).unwrap(), PathBuf::from(&raw));
    assert_eq!(session.sidebar().rows[0].text, "Report 2026");
    let invisible = session.open(Path::new("/d/\u{200B}.pdf")).unwrap();
    assert_eq!(&*session.tab(invisible).unwrap().name, "Untitled", "a name of nothing still needs a row");
}

struct NameLoader;

impl DocLoader for NameLoader {
    fn load(&self, _path: &Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
        Ok(recto_core::DocFacts {
            page_count: 1,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: recto_core::Toc::default(),
            renderer_only: false,
        })
    }
}

/// Failure: a field added carelessly, or a component that brings padding with it (R3.2 groups the fields), raises the cost of every idle tab. Held at its size today, 224 bytes, of which 2 are padding.
#[test]
fn tab_record_stays_small() {
    assert!(std::mem::size_of::<TabRecord>() <= 224, "{} bytes", std::mem::size_of::<TabRecord>());
}

/// Failure: moving `Session`'s methods into its components (R3.5) brings state along with them, a cache or a copy kept beside a component, and the session grows. Held at its size today, 1,128 bytes with no tab.
#[test]
fn the_session_stays_its_size() {
    let size = std::mem::size_of::<Session<DiskLoader>>();
    assert!(size <= 1128, "{size} bytes");
}

/// Failure: tapping the active PDF switches tabs or does nothing, instead of folding its contents.
#[test]
fn tapping_the_active_pdf_toggles_contents() {
    let mut s = session(10);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    assert!(s.tab(id).unwrap().toc_open);
    s.tap(id).unwrap();
    assert!(!s.tab(id).unwrap().toc_open);
    assert_eq!(s.active(), Some(id));
    s.tap(id).unwrap();
    assert!(s.tab(id).unwrap().toc_open);
}

/// Failure: tapping another PDF folds the current one instead of switching, or forgets its folded state.
#[test]
fn tapping_another_pdf_switches_and_keeps_each_fold_state() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.tap(a).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.tap(a).unwrap();
    assert_eq!(s.active(), Some(a));
    assert!(!s.tab(a).unwrap().toc_open);
    assert!(s.tab(b).unwrap().toc_open);
}

/// Failure: opening the same file twice makes a second tab rather than going to the first.
#[test]
fn opening_a_file_twice_reuses_its_tab() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.open(Path::new("/d/b.pdf")).unwrap();
    assert_eq!(s.open(Path::new("/d/a.pdf")).unwrap(), a);
    assert_eq!(s.tabs().len(), 2);
    assert_eq!(s.active(), Some(a));
}

/// Failure: closing the active tab leaves nothing active while others are open, or leaves its contents in memory.
#[test]
fn closing_the_active_tab_activates_a_neighbour() {
    let mut s = session(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.close(b).unwrap();
    assert_eq!(s.active(), Some(a));
    assert!(!s.warm.contains(b));
    s.close(a).unwrap();
    assert_eq!(s.active(), None);
    assert_eq!(s.sidebar().place, "");
}

/// Failure: closing a tab loses page edits without warning.
#[test]
fn closing_refuses_while_there_are_unsaved_edits() {
    let mut s = session(5);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
    assert_eq!(s.close(id), Err(SessionError::UnsavedChanges));
    assert!(s.sidebar().edited);
    assert!(s.undo_edit(id));
    assert!(!s.sidebar().edited);
    // Failure: an agent's `undo` takes the owner's edit off the page, so the dot goes and this close goes through with no prompt, and the edit is gone for good.
    assert!(s.sidebar().undone_owner_steps, "the app must be able to show it");
    assert_eq!(
        s.close(id),
        Err(SessionError::UnsavedChanges),
        "the owner's edit can still be redone, so closing would lose it"
    );
    s.discard_edits(id);
    assert!(!s.sidebar().undone_owner_steps);
    assert!(s.close(id).is_ok());
}

/// Failure (decision 26): a link to an open PDF, or a file moved and opened by its new name, makes a second tab. A copy is another file, and a PDF added as pages isn't a tab. Each case on real files, as what's the same is the file on disk.
#[test]
fn a_pdf_can_only_be_opened_once() {
    let dir = recto_testpdf::TempDir::new("recto-once-");
    std::fs::create_dir_all(&dir).unwrap();
    let (file, link, hard, copy, moved) = (
        dir.join("a.pdf"),
        dir.join("link.pdf"),
        dir.join("hard.pdf"),
        dir.join("copy.pdf"),
        dir.join("moved.pdf"),
    );
    std::fs::write(&file, b"one").unwrap();
    std::os::unix::fs::symlink(&file, &link).unwrap();
    std::fs::hard_link(&file, &hard).unwrap();
    std::fs::write(&copy, b"one").unwrap();
    let mut s = session(5);
    let a = s.open(&file).unwrap();
    let other = s.open(&copy).unwrap();
    assert_ne!(a, other, "a copy is another file");
    assert_eq!(s.open(&link).unwrap(), a, "a link is the same PDF");
    assert_eq!(s.open(&hard).unwrap(), a, "so is a hard link");
    assert_eq!(s.active(), Some(a), "and it goes there");
    assert_eq!(s.path_of(a).unwrap(), file, "still there under its name, so that stays");
    assert_eq!(s.tabs().len(), 2);
    // From facts read off the session, as the app's route has them.
    s.tap(other).unwrap();
    let stamp = file_stamp(&link).ok();
    let facts = FakeLoader { pages: 5, loads: Cell::new(0) }.load(&link).unwrap();
    assert_eq!(s.open_with_facts(&link, facts, stamp, None).unwrap(), a);
    assert_eq!(s.tabs().len(), 2);
    // Moved, and every other name gone: the tab goes with it.
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_file(&hard).unwrap();
    std::fs::rename(&file, &moved).unwrap();
    assert_eq!(s.open(&moved).unwrap(), a);
    assert_eq!(s.path_of(a).unwrap(), moved);
    assert_eq!(&*s.tab(a).unwrap().name, "moved");
    // As pages of another document, it's allowed.
    let stamp = file_stamp(&moved).ok();
    let facts = FakeLoader { pages: 5, loads: Cell::new(0) }.load(&moved).unwrap();
    s.insert_pdf_with_facts(other, Author::Owner, 0, &moved, &facts, stamp).unwrap();
    assert_eq!(s.pages_in(other), 10);
    assert_eq!(s.tabs().len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure (final review N2): a different PDF with the inode a tab remembers took the tab over: after a move and a rewrite in place, or on another volume under the same device number. The tab's path became the other file, with edits for 6 pages on one of 3 and no notice. The number alone isn't the file: it's a different one, and gets its own tab, and the edits stay. A file changed in place at the tab's own path is still that tab. And (final review) a file replaced under a tab, with a new inode, is reached through a link all the same.
#[test]
fn the_same_inode_isnt_enough_to_be_the_same_pdf() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-hijack-");
    std::fs::create_dir_all(&dir).unwrap();
    let (y, z) = (dir.join("y.pdf"), dir.join("z.pdf"));
    std::fs::write(&y, b"six pages").unwrap();
    let mut s = session(6);
    let tab = s.open(&y).unwrap();
    s.edit_pages(tab, |e| e.delete(0, 1)).unwrap();
    // Moved, and written over in place: the same inode, and not what the edits are about.
    std::fs::rename(&y, &z).unwrap();
    std::fs::write(&z, b"three, and longer than it was").unwrap();
    let opened = s.open(&z).unwrap();
    assert_ne!(opened, tab, "it's another PDF");
    assert_eq!(s.path_of(tab).unwrap(), y, "and the tab with edits stays on what they're about");
    assert!(s.is_edited(tab));
    assert_eq!(s.pages_in(opened), 6, "nothing of the edits on it");
    assert!(!s.is_edited(opened));
    // Changed in place under its own name, by a link: that's still the tab.
    let (own, link) = (dir.join("own.pdf"), dir.join("link.pdf"));
    std::fs::write(&own, b"before").unwrap();
    std::os::unix::fs::symlink(&own, &link).unwrap();
    let mut s = session(6);
    let tab = s.open(&own).unwrap();
    std::fs::write(&own, b"after, and longer").unwrap();
    assert_eq!(s.open(&link).unwrap(), tab, "the same file, changed");
    assert_eq!(s.tabs().len(), 1);
    // Replaced under the tab with a new inode, as a save from another app does.
    let fresh = dir.join("fresh.pdf");
    std::fs::write(&fresh, b"replaced").unwrap();
    std::fs::rename(&fresh, &own).unwrap();
    assert_eq!(s.open(&link).unwrap(), tab, "the link leads to the tab's own path");
    assert_eq!(s.tabs().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure (review T17): the next tab number wraps round, and a tab comes up with a number already in use.
#[test]
fn tab_numbers_dont_wrap() {
    let mut s = session(5);
    s.tabs.set_next_id(MAX_TAB_ID);
    assert_eq!(s.open(Path::new("/d/a.pdf")), Err(SessionError::TooManyTabs));
}

/// Failure: a folder is kept with no tab in it to let it go, so it stays for the rest of the session: an open refused because the tab numbers have run out, or a move to a path that names no file. Each refused file in another folder grows the folders kept, without a cap.
#[test]
fn no_folder_is_kept_without_a_tab() {
    let mut s = session(5);
    s.tabs.set_next_id(MAX_TAB_ID);
    for path in ["/d/a.pdf", "/e/b.pdf", "/f/c.pdf"] {
        assert_eq!(s.open(Path::new(path)), Err(SessionError::TooManyTabs));
    }
    assert_eq!(s.tabs.dirs_kept(), 0, "an open refused keeps no folder");

    let mut s = session(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.relocate(a, Path::new("/elsewhere/.."));
    assert_eq!(s.path_of(a), Some(PathBuf::from("/d/a.pdf")), "a move to no file doesn't move it");
    assert_eq!(s.tabs.dirs_kept(), 1, "and keeps no other folder");
}

/// Failure: a protected PDF just says it needs a password, with nothing to say why it's asking again, or a password is written down. The words say it isn't kept, and nothing in a record's fields could hold one.
#[test]
fn a_protected_pdf_asks_again() {
    struct Protected;
    impl DocLoader for Protected {
        fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
            Err(OpenError::PasswordNeeded)
        }
    }
    let mut record = session(5);
    let a = record.open(Path::new("/d/a.pdf")).unwrap();
    let saved = record.to_record();
    let text = serde_json::to_string(&saved).unwrap();
    assert!(!text.contains("password"), "{text}");
    let mut back = Session::new(Protected);
    back.restore(&saved, HashMap::new()).unwrap();
    back.wake_active();
    assert_eq!(back.tab(a).unwrap().problem, Some(OpenError::PasswordNeeded));
    assert!(back.tab_notice_message(a).unwrap().contains("password"));
}

struct Locked;

impl DocLoader for Locked {
    fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 3,
            byte_len: 1,
            encrypted: true,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Failure (review O1): `encrypted` is taken for "opened with a password", so a file anyone may open is kept from agents, or one the person typed a password for isn't; or the mark's lost across a quit, so what the record kept of it (its length, its place) reaches an agent before the password's typed again; or a tab whose password went missing goes on showing what was read with it instead of asking (review O5).
#[test]
fn a_tab_opened_with_a_password_is_marked_kept_and_asks_again_when_its_key_is_gone() {
    let mut s = Session::new(Locked);
    let anyone = s.open(Path::new("/d/anyone.pdf")).unwrap();
    assert!(!s.withheld_from_agents(anyone), "encrypted alone isn't protected from agents");
    let locked = s
        .open_unlocked(Path::new("/d/locked.pdf"), Locked.load(Path::new("/d/locked.pdf")).unwrap(), None)
        .unwrap();
    assert!(s.withheld_from_agents(locked));
    let record = s.to_record();
    assert_eq!(record.tabs.iter().map(|tab| tab.password).collect::<Vec<_>>(), [false, true]);
    let mut again = Session::new(Locked);
    again.restore(&record, HashMap::new()).unwrap();
    assert!(
        again.withheld_from_agents(TabId(locked.0)) && !again.withheld_from_agents(TabId(anyone.0)),
        "kept across a quit"
    );
    assert!(!s.needs_password(locked));
    s.needs_password_again(locked);
    assert!(s.needs_password(locked) && s.withheld_from_agents(locked));
    assert!(!s.warm_tabs().any(|id| id == locked), "what was read with the password is let go");
}

/// Failure: every folder a file was ever opened from stays in the session after its last tab has closed, so opening and closing files across many folders grows it for as long as it runs; or letting a folder go breaks the path of a tab still open in another.
#[test]
fn folders_are_kept_only_while_a_tab_is_in_them() {
    let mut s = session(5);
    let kept = s.open(Path::new("/stays/kept.pdf")).unwrap();
    for n in 0..2_000 {
        let id = s.open(Path::new(&format!("/folder-{n}/a.pdf"))).unwrap();
        let beside = s.open(Path::new(&format!("/folder-{n}/b.pdf"))).unwrap();
        s.close(id).unwrap();
        assert_eq!(
            s.path_of(beside),
            Some(PathBuf::from(format!("/folder-{n}/b.pdf"))),
            "its folder stays while a tab's in it"
        );
        s.close(beside).unwrap();
    }
    assert_eq!(s.tabs.dirs_kept(), 1, "only the folder still in use");
    assert_eq!(s.path_of(kept), Some(PathBuf::from("/stays/kept.pdf")));
    assert_eq!(s.tab_for_path(Path::new("/folder-7/a.pdf")), None);
    let moved = s.open(Path::new("/before/moved.pdf")).unwrap();
    s.relocate(moved, Path::new("/after/moved.pdf"));
    assert_eq!(s.tabs.dirs_kept(), 2, "a tab moved out of its folder lets it go");
    assert_eq!(s.path_of(moved), Some(PathBuf::from("/after/moved.pdf")));
    assert_eq!(s.path_of(kept), Some(PathBuf::from("/stays/kept.pdf")));
}
