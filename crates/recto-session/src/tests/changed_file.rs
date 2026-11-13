//! A tab's file changed on disk by another program while it has unsaved edits (SPEC: "Unsaved edits stay, and nothing is written to the file").
//! Until the page matcher exists (owner, 2026-11-13), an edit names its page by its number in the version it was made on, so what has to hold here is
//! only that the edits and their journal are never lost because a file changed, and that nothing claims what isn't so.

use super::*;
use std::rc::Rc;

/// A loader whose file is as long as the test says, and which can have the file replaced on disk in the middle of reading it: after the stamp was
/// taken and before the read, as a save by another app can land.
struct Shifting {
    pages: Rc<Cell<u32>>,
    /// Bytes written over the file at the next load, before it's read.
    replace_with: Rc<std::cell::RefCell<Option<Vec<u8>>>>,
}

impl DocLoader for Shifting {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        if let Some(bytes) = self.replace_with.borrow_mut().take() {
            replace_file(path, &bytes);
        }
        if !path.exists() {
            return Err(OpenError::NotFound);
        }
        Ok(DocFacts {
            page_count: self.pages.get(),
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// Another program's save: a new file renamed over the old, so it has a new number, size and time.
fn replace_file(path: &Path, bytes: &[u8]) {
    let new = path.with_extension("new");
    std::fs::write(&new, bytes).unwrap();
    std::fs::rename(&new, path).unwrap();
}

struct Bed {
    dir: recto_testpdf::TempDir,
    pages: Rc<Cell<u32>>,
    replace_with: Rc<std::cell::RefCell<Option<Vec<u8>>>>,
    s: Session<Shifting>,
    files: HashMap<String, Vec<u8>>,
}

impl Bed {
    /// A journaled session with `a.pdf`, `pages` long, open and active, and a second tab to go to.
    fn new(pages: u32) -> (Self, TabId, TabId) {
        let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-changed-file-");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), b"the version the edits are made on").unwrap();
        std::fs::write(dir.join("b.pdf"), b"another file").unwrap();
        let pages = Rc::new(Cell::new(pages));
        let replace_with = Rc::new(std::cell::RefCell::new(None));
        let mut s = Session::new(Shifting {
            pages: Rc::clone(&pages),
            replace_with: Rc::clone(&replace_with),
        });
        s.set_journaling(true);
        let a = s.open(&dir.join("a.pdf")).unwrap();
        let b = s.open(&dir.join("b.pdf")).unwrap();
        s.tap(a).unwrap();
        let bed = Self {
            dir,
            pages,
            replace_with,
            s,
            files: HashMap::new(),
        };
        (bed, a, b)
    }

    fn path(&self) -> PathBuf {
        self.dir.join("a.pdf")
    }

    fn stamp_now(&self) -> FileStamp {
        file_stamp(&self.path()).unwrap()
    }

    /// The journal thread's work, done, into memory.
    fn write_journal(&mut self) {
        write_journal(&mut self.s, &mut self.files);
    }

    /// Goes to `b` and lets go of every tab's contents, so going back to `a` reads its file again.
    fn make_cold(&mut self, a: TabId, b: TabId) {
        self.s.tap(b).unwrap();
        self.s.warm.clear_contents();
        assert_eq!(self.s.tier(a), Tier::Cold);
    }

    /// What has to hold whichever way the file changed: the edits are all there, their journal stays on disk, the tab says the file changed, and no
    /// words say anything was kept somewhere it wasn't.
    fn assert_edits_kept_and_said(&mut self, a: TabId, as_they_were: &record::EditsRecord, journal: &str) {
        assert_eq!(self.s.lists().edits_record(a).as_ref(), Some(as_they_were), "the edits are all there");
        assert!(self.s.lists().is_edited(a));
        self.write_journal();
        assert!(
            self.files.contains_key(journal),
            "the journal is still on disk: {:?}",
            self.files.keys().collect::<Vec<_>>()
        );
        assert_eq!(self.s.tabs().tab_notice(a).map(FileNotice::code), Some("file_changed"));
        let words = self.s.notices().said(a).map(|said| said.message).unwrap();
        assert!(!words.contains("kept in the state folder"), "nothing claims a copy was kept elsewhere: {words}");
        assert_eq!(self.s.close(a), Err(SessionError::UnsavedChanges), "and closing would lose them, so it asks");
    }

    /// A save from here can't be about the file at the path: its stamp isn't the file's, or it has none.
    fn assert_save_cant_take_the_new_file(&mut self, a: TabId) {
        let snapshot = self.s.begin_save(a, false).unwrap().unwrap();
        assert_ne!(
            snapshot.stamp,
            Some(self.stamp_now()),
            "a save would be from the file the edits weren't made on"
        );
        self.s.end_save(a);
    }
}

/// Failure (B1, route 1): the file is replaced between the stamp being taken and the file being read, so the read is of the new file at the old file's stamp.
/// Its other length was taken to mean the list was never about it: the list and its journal were deleted, and the tab said what couldn't be read "has been
/// kept in the state folder", which nothing had kept.
#[test]
fn a_file_replaced_mid_read_doesnt_take_the_edits_with_it() {
    let (mut bed, a, b) = Bed::new(10);
    bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
    bed.write_journal();
    let journal = bed.files.keys().next().unwrap().clone();
    let as_they_were = bed.s.lists().edits_record(a).unwrap();
    let made_on = bed.stamp_now();
    bed.make_cold(a, b);
    // Going back reads the file, and another app's save lands between the stamp and the read: five pages now.
    bed.pages.set(5);
    *bed.replace_with.borrow_mut() = Some(b"a shorter version, saved by another app".to_vec());
    bed.s.tap(a).unwrap();
    assert!(bed.replace_with.borrow().is_none(), "the file was replaced mid-read");
    assert_ne!(bed.stamp_now(), made_on);
    bed.assert_edits_kept_and_said(a, &as_they_were, &journal);
    assert_eq!(bed.s.tabs().own_stamp(a), Some(made_on), "the edits still name the version they were made on");
    bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a)));
    assert_eq!(
        bed.s.tabs().tab_notice(a).map(FileNotice::code),
        Some("file_changed"),
        "a look says what the notice says"
    );
    bed.assert_save_cant_take_the_new_file(a);
}

/// Failure (B1, route 2): a clean tab hears its file changed (its stamp moves to the new file, its length is still the old one's until it's read), an
/// edit lands before the read does (an agent editing a background tab), and the list is made on the old length under the new stamp. When the read
/// lands, the list's length isn't the file's at its own stamp, and the list and its journal were deleted.
#[test]
fn an_edit_made_before_a_changed_file_is_read_again_is_kept() {
    let (mut bed, a, b) = Bed::new(10);
    replace_file(&bed.path(), b"a shorter version, saved by another app");
    bed.pages.set(5);
    let now = bed.stamp_now();
    bed.s.apply_file_status(a, FileStatus::Changed(now));
    assert_eq!(bed.s.pages().count(a), 10, "not read yet");
    bed.s.edit_pages_as(a, Author::Agent, |e| e.rotate(8, 1, 1)).unwrap();
    bed.write_journal();
    let journal = bed.files.keys().next().unwrap().clone();
    let as_they_were = bed.s.lists().edits_record(a).unwrap();
    // The read lands.
    bed.make_cold(a, b);
    bed.s.tap(a).unwrap();
    bed.assert_edits_kept_and_said(a, &as_they_were, &journal);
    bed.assert_save_cant_take_the_new_file(a);
    // Looking again doesn't make the edits about the new file, and is nothing new.
    assert!(!bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a))));
    assert_eq!(bed.s.tabs().tab_notice(a).map(FileNotice::code), Some("file_changed"), "it goes on saying so");
    bed.assert_save_cant_take_the_new_file(a);
}

/// Failure (B5): every look at an edited tab whose file had changed took the change in again, as the edits keep the old stamp and every look finds the file
/// isn't that: generated contents forgotten, the folds reset, the sleep file retired, and the app reading the file's structure again, at every return to the
/// front and every show. A look that finds what's already known is nothing new, and says so, so the app neither forgets nor reads anything for it.
#[test]
fn a_look_that_finds_what_is_already_known_is_nothing_new() {
    let (mut bed, a, _b) = Bed::new(10);
    bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
    let made_on = bed.stamp_now();
    replace_file(&bed.path(), b"a shorter version, saved by another app");
    let look = |bed: &Bed| record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a));
    assert!(bed.s.apply_file_status(a, look(&bed)), "the first look is news");
    assert_eq!(bed.s.tabs().tab_notice(a).map(FileNotice::code), Some("file_changed"));
    bed.s.tab_mut(a).unwrap().folds = Some(vec![(1, false)].into_boxed_slice());
    for _ in 0..3 {
        assert!(!bed.s.apply_file_status(a, look(&bed)), "the same again isn't");
    }
    assert!(bed.s.tabs().tab(a).unwrap().folds.is_some(), "folds made since are kept");
    assert!(!bed.s.apply_file_status(a, FileStatus::NotLooked));
    assert!(bed.s.lists().is_edited(a));
    // Gone is news, and so is coming back as the other version again.
    assert!(bed.s.apply_file_status(a, FileStatus::Missing));
    assert!(bed.s.apply_file_status(a, look(&bed)));
    // Back at the version the edits were made on is news too, and the notice goes.
    assert!(bed.s.apply_file_status(a, FileStatus::Same(made_on)));
    assert_eq!(bed.s.tabs().tab_notice(a), None);
}

/// Pinned for the page matcher (2026-11-13): a tab's list has gone to disk with the tab dormant, the file changes while it sleeps, and the tab is looked at,
/// woken and read again. The list comes back whole from its journal, the journal stays, and the tab says its file changed: a list asleep is no less kept.
#[test]
fn a_list_asleep_when_its_file_changes_wakes_whole() {
    let (mut bed, a, b) = Bed::new(10);
    let journals = Arc::new(std::sync::Mutex::new(HashMap::new()));
    bed.s
        .journals_mut()
        .set_history(Arc::new(super::sleep_tests::MemorySource(Arc::clone(&journals))));
    bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
    bed.s.tap(b).unwrap();
    bed.write_journal();
    journals.lock().unwrap().clone_from(&bed.files);
    let journal = bed.files.keys().next().unwrap().clone();
    let as_they_were = bed.s.lists().edits_record(a).unwrap();
    assert!(bed.s.go_dormant(a));
    assert!(bed.s.lists().is_asleep(a));
    replace_file(&bed.path(), b"a shorter version, saved by another app");
    bed.pages.set(5);
    assert!(bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a))));
    bed.s.warm.clear_contents();
    bed.s.tap(a).unwrap();
    assert!(!bed.s.lists().is_asleep(a));
    bed.assert_edits_kept_and_said(a, &as_they_were, &journal);
    bed.assert_save_cant_take_the_new_file(a);
}

/// Bed's `an_edit_made_before_a_changed_file_is_read_again_is_kept` ordering, which leaves `a` edited with no stamp: its file is five pages under a list of ten.
fn dropped_stamp() -> (Bed, TabId, TabId) {
    let (mut bed, a, b) = Bed::new(10);
    replace_file(&bed.path(), b"a shorter version, saved by another app");
    bed.pages.set(5);
    let now = bed.stamp_now();
    bed.s.apply_file_status(a, FileStatus::Changed(now));
    bed.s.edit_pages_as(a, Author::Agent, |e| e.rotate(8, 1, 1)).unwrap();
    bed.make_cold(a, b);
    bed.s.tap(a).unwrap();
    assert_eq!(bed.s.tabs().own_stamp(a), None, "the ordering drops the stamp");
    (bed, a, b)
}

/// What has to hold while a tab's edits are about another version than its file: the tab says so, nothing reads the file as its pages, and no sleep file is
/// made of it.
fn assert_known_not_to_fit(bed: &Bed, a: TabId, what: &str) {
    assert_ne!(bed.s.notices().fit(a), EditsFit::Fit, "{what}");
    assert_eq!(bed.s.notices().said(a).map(|said| said.code), Some("file_changed"), "{what}: the tab says so");
    assert!(!bed.s.page_source(a, 2).unwrap().is_current(), "{what}: its pages aren't read from this file");
    assert!(bed.s.sleep_facts(a).is_none(), "{what}: no sleep file of it");
}

/// Failure (F1, case 1): that a tab's edits didn't fit its file lived only in its notice, which other notices overwrite. With no stamp, the file deleted
/// (Missing over the notice), put back (nothing new, so Missing stayed) and read again from cold (which clears Missing) left no word of it: no notice, Find no
/// longer refused, and its pages taken to be this file's but for the stamp.
#[test]
fn edits_with_no_stamp_still_dont_fit_after_the_file_goes_and_comes_back() {
    let (mut bed, a, b) = dropped_stamp();
    assert_known_not_to_fit(&bed, a, "after the read");
    let aside = bed.path().with_extension("aside");
    std::fs::rename(bed.path(), &aside).unwrap();
    assert!(bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a))));
    assert_eq!(bed.s.notices().said(a).map(|said| said.code), Some("file_missing"), "gone is said first");
    std::fs::rename(&aside, bed.path()).unwrap();
    assert!(
        bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a))),
        "back is news"
    );
    assert_known_not_to_fit(&bed, a, "back");
    bed.make_cold(a, b);
    bed.s.tap(a).unwrap();
    assert_known_not_to_fit(&bed, a, "read again from cold");
}

/// Failure (F1, case 2): with the stamp of the version the edits were made on kept and the other version read in (its contents warm), history rolling off put
/// its notice over "changed", and leaving the tab cleared that, so a sleep file was written named for the edits' version holding the other's contents and
/// pictures, which would wake as that version once it was put back.
#[test]
fn edits_on_another_version_stay_known_under_other_notices_and_make_no_sleep_file() {
    let (mut bed, a, b) = Bed::new(10);
    bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
    let made_on = bed.stamp_now();
    replace_file(&bed.path(), b"another version, saved by another app");
    assert!(bed.s.apply_file_status(a, record::look_at_file(&bed.path(), Some(made_on))));
    bed.s.forget_read(a);
    bed.make_cold(a, b);
    bed.s.tap(a).unwrap();
    assert_eq!(bed.s.tabs().own_stamp(a), Some(made_on), "the edits' version is kept");
    assert_eq!(bed.s.tier(a), Tier::Hot, "the other version's contents are held");
    bed.s.tabs_mut().tab_mut(a).unwrap().file.set_notice(Some(FileNotice::HistoryRolledOff));
    assert_known_not_to_fit(&bed, a, "under history's notice");
    bed.s.tap(b).unwrap();
    assert_eq!(bed.s.tier(a), Tier::Warm);
    assert_known_not_to_fit(&bed, a, "left");
    assert!(bed.s.take_left_warm().iter().all(|facts| facts.id != a));
}

/// Failure (F7): a list with only undo history (nothing unsaved) went through the block for edits that don't fit: the tab then said "Your unsaved edits are
/// kept" when there were none, and history about another version stayed, so redo could bring back edits numbered by the version that's gone. On `main` before
/// that, such a list was let go with "What couldn't has been kept in the state folder", which nothing had kept. The history goes, and the tab says what's so:
/// the file changed, where a look shows it was replaced, or else only that undo can't go back as far.
#[test]
fn only_history_about_another_length_goes_and_the_tab_says_whats_so() {
    // Replaced mid-read: the file did change.
    let (mut bed, a, b) = Bed::new(10);
    bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
    assert!(bed.s.undo_edit(a));
    assert!(!bed.s.lists().is_edited(a) && bed.s.lists().get(a).is_some(), "only history");
    bed.make_cold(a, b);
    bed.pages.set(5);
    *bed.replace_with.borrow_mut() = Some(b"a shorter version, saved by another app".to_vec());
    bed.s.tap(a).unwrap();
    assert_only_history_gone(&bed, a, "file_changed");
    // A journal of only history naming another length than the file has, which didn't change.
    let (mut bed, a, b) = Bed::new(5);
    let mut list = PageEdits::new(12);
    list.journal_from_now();
    list.rotate(11, 1, 1).unwrap();
    assert!(list.undo());
    bed.s.take_up_history(a, "journal-00000000000000ab.log", list, 1);
    assert!(
        !bed.s.lists().is_edited(a) && bed.s.lists().undo_and_redo(a).1,
        "only history, with a step to redo"
    );
    bed.make_cold(a, b);
    bed.s.tap(a).unwrap();
    assert_only_history_gone(&bed, a, "history_rolled_off");
}

/// Nothing of a history about another version is left to redo, nothing says unsaved edits were kept, and the tab is the file it read.
fn assert_only_history_gone(bed: &Bed, a: TabId, said: &str) {
    assert_eq!(bed.s.notices().fit(a), EditsFit::Fit, "nothing unsaved to fit");
    assert!(bed.s.lists().get(a).is_none(), "the history's gone");
    assert_eq!(bed.s.lists().undo_and_redo(a), (false, false), "nothing of it can be redone");
    let words = bed.s.notices().said(a).unwrap();
    assert_eq!(words.code, said, "{words:?}");
    assert!(
        !words.message.contains("unsaved edits") && !words.message.contains("kept in the state folder"),
        "{words:?}"
    );
    assert_eq!(bed.s.pages().count(a), 5, "the tab is the file it read");
}

/// Failure (review round 1): what's allowed on a tab whose edits don't fit its file was decided by its notice, which every other notice overwrites, so a file
/// gone and back, history rolling off or a password asked for again let its pages be read as this file's, a sleep file be made of it, or a save be tried onto it.
/// Whatever the notice says over it, nothing that reads the fact changes: what Find, `search` and the agent's save read too, through the same `fit`.
#[test]
fn no_notice_over_edits_that_dont_fit_changes_what_is_allowed() {
    let kept = || {
        let (mut bed, a, b) = Bed::new(10);
        bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
        replace_file(&bed.path(), b"another version, saved by another app");
        bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a)));
        (bed, a, b)
    };
    let notices = [
        None,
        Some(FileNotice::Moved {
            from: PathField::Text("/d/before.pdf".into()),
        }),
        Some(FileNotice::Changed),
        Some(FileNotice::Missing),
        Some(FileNotice::NeedsPassword),
        Some(FileNotice::EditsUnreadable),
        Some(FileNotice::EditsNotMerged),
        Some(FileNotice::HistoryRolledOff),
    ];
    for (what, (mut bed, a, _b)) in [("kept stamp", kept()), ("no stamp", dropped_stamp())] {
        let fit = bed.s.notices().fit(a);
        assert!(!fit.fits(), "{what}");
        for notice in notices.clone() {
            bed.s.tabs_mut().tab_mut(a).unwrap().file.set_notice(notice.clone());
            assert_eq!(bed.s.notices().fit(a), fit, "{what}, under {notice:?}");
            assert!(!bed.s.page_source(a, 2).unwrap().is_current(), "{what}, under {notice:?}");
            assert!(bed.s.sleep_facts(a).is_none(), "{what}, under {notice:?}");
            assert_eq!(bed.s.begin_save(a, false).unwrap().unwrap().stamp, None, "{what}, under {notice:?}");
            bed.s.end_save(a);
        }
    }
}

/// Failure (review round 1): a reader decided what's allowed by comparing a notice's code string (`notice == Some("file_changed")`,
/// `notice.code() == "file_changed"`), which another notice overwrites. No code outside tests compares a tab's notice to a code: what's allowed is
/// [`Notices::fit`]'s, and what's said is [`Notices::said`]'s.
#[test]
fn nothing_decides_by_a_notice_code_string() {
    let codes = [
        "\"file_changed\"",
        "\"file_missing\"",
        "\"file_moved\"",
        "\"password_again\"",
        "\"edits_unreadable\"",
        "\"edits_not_merged\"",
        "\"history_rolled_off\"",
        "\"edits_dont_fit\"",
    ];
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = Vec::new();
    let mut folders: Vec<PathBuf> = ["recto-session", "recto-agent", "recto-app", "recto-ffi", "recto-control"]
        .iter()
        .map(|name| crates.join(name).join("src"))
        .collect();
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), "tests" | "headless" | "testkit") {
                    folders.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") || name == "tests.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let production = source.split("#[cfg(test)]").next().unwrap_or_default();
            for (at, line) in production.lines().enumerate() {
                if line.contains("notice") && codes.iter().any(|code| line.contains(code)) {
                    found.push(format!("{}:{}: {}", path.display(), at + 1, line.trim()));
                }
            }
        }
    }
    assert!(found.is_empty(), "decided by a notice's code: {found:#?}");
}

/// Failure (F2): a second change under edits that don't fit (v2, then v3) was taken for nothing new, as the tab already said its file changed, so what was held
/// of v2 (its contents and labels, the contents' verdict and folds, generated contents) stayed on while the renderer drew v3: "nothing of the old file is
/// shown as the new" (SPEC). Each version not yet taken in is news, and what's held of the last goes; a look at the same one again does no work. With the stamp
/// kept and with it gone alike.
#[test]
fn a_second_change_under_edits_that_dont_fit_is_news_and_a_third_look_isnt() {
    let kept = || {
        let (mut bed, a, b) = Bed::new(10);
        bed.s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
        replace_file(&bed.path(), b"v2, saved by another app");
        assert!(bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.tabs().own_stamp(a))));
        (bed, a, b)
    };
    for (what, (mut bed, a, b)) in [("kept stamp", kept()), ("no stamp", dropped_stamp())] {
        let stamp = bed.s.tabs().own_stamp(a);
        let look = |bed: &Bed| record::look_at_file(&bed.path(), stamp);
        // v2 read in, as the app does on news: what's held of it is v2's, and a fold is made on its contents.
        bed.s.forget_read(a);
        bed.make_cold(a, b);
        bed.s.tap(a).unwrap();
        bed.s.tabs_mut().tab_mut(a).unwrap().folds = Some(vec![(1, false)].into_boxed_slice());
        assert!(!bed.s.apply_file_status(a, look(&bed)), "{what}: v2 again is nothing new");
        assert!(bed.s.tabs().tab(a).unwrap().folds.is_some(), "{what}: so its fold stays");
        replace_file(&bed.path(), b"v3, saved by another app again, longer");
        assert!(bed.s.apply_file_status(a, look(&bed)), "{what}: v3 is news");
        assert!(bed.s.tabs().tab(a).unwrap().folds.is_none(), "{what}: v2's folds go with it");
        assert_eq!(bed.s.tabs().own_stamp(a), stamp, "{what}: and the edits aren't taken to be about v3");
        assert!(!bed.s.notices().fit(a).fits(), "{what}");
        assert!(!bed.s.apply_file_status(a, look(&bed)), "{what}: a third look at v3 does nothing");
    }
}
