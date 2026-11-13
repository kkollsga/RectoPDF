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
        assert_eq!(self.s.edits_record(a).as_ref(), Some(as_they_were), "the edits are all there");
        assert!(self.s.is_edited(a));
        self.write_journal();
        assert!(
            self.files.contains_key(journal),
            "the journal is still on disk: {:?}",
            self.files.keys().collect::<Vec<_>>()
        );
        assert_eq!(self.s.tab_notice(a).map(FileNotice::code), Some("file_changed"));
        let words = self.s.tab_notice_message(a).unwrap();
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
    let as_they_were = bed.s.edits_record(a).unwrap();
    let made_on = bed.stamp_now();
    bed.make_cold(a, b);
    // Going back reads the file, and another app's save lands between the stamp and the read: five pages now.
    bed.pages.set(5);
    *bed.replace_with.borrow_mut() = Some(b"a shorter version, saved by another app".to_vec());
    bed.s.tap(a).unwrap();
    assert!(bed.replace_with.borrow().is_none(), "the file was replaced mid-read");
    assert_ne!(bed.stamp_now(), made_on);
    bed.assert_edits_kept_and_said(a, &as_they_were, &journal);
    // From here the session can't tell which version the read was of, so the edits name none, and a look at the file doesn't make them its own.
    assert_eq!(bed.s.own_stamp(a), None);
    bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.own_stamp(a)));
    assert_eq!(bed.s.tab_notice(a).map(FileNotice::code), Some("file_changed"), "it goes on saying so");
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
    assert_eq!(bed.s.pages_in(a), 10, "not read yet");
    bed.s.edit_pages_as(a, Author::Agent, |e| e.rotate(8, 1, 1)).unwrap();
    bed.write_journal();
    let journal = bed.files.keys().next().unwrap().clone();
    let as_they_were = bed.s.edits_record(a).unwrap();
    // The read lands.
    bed.make_cold(a, b);
    bed.s.tap(a).unwrap();
    bed.assert_edits_kept_and_said(a, &as_they_were, &journal);
    bed.assert_save_cant_take_the_new_file(a);
    // Looking again doesn't make the edits about the new file.
    bed.s.apply_file_status(a, record::look_at_file(&bed.path(), bed.s.own_stamp(a)));
    assert_eq!(bed.s.tab_notice(a).map(FileNotice::code), Some("file_changed"), "it goes on saying so");
    bed.assert_save_cant_take_the_new_file(a);
}
