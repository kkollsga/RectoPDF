//! Unsaved edits following their pages into another version of their file (`crate::version_change`), on real files: the match read off the lock,
//! taken in under it, and what the tab says. The pure rules are the table in `edit/rebase/table.rs`; this is that the session does what they say.

use super::*;
use crate::edit::matcher::ScanCaps;
use crate::version_change;

std::thread_local! {
    /// The scan's caps on this thread, when a test sets them small (`crate::version_change::scan_caps`).
    pub(crate) static SCAN_CAPS: Cell<Option<ScanCaps>> = const { Cell::new(None) };
}

/// A loader that reads the real file's facts, as the app's does.
struct Real;

impl DocLoader for Real {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        recto_core::inspect(path)
    }
}

/// Pages saying who made them and their number; `cover` adds a first page.
fn document(who: &str, pages: &[u32], cover: bool) -> Vec<u8> {
    let mut made: Vec<recto_testpdf::Page> = Vec::new();
    if cover {
        made.push(recto_testpdf::Page::new(recto_testpdf::text_lines(&["A cover another program added"])));
    }
    made.extend(
        pages
            .iter()
            .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")]))),
    );
    recto_testpdf::pages(&made)
}

fn ten(who: &str) -> Vec<u8> {
    document(who, &(1..=10).collect::<Vec<_>>(), false)
}

/// Another program's save: a new file renamed over the old.
fn replace_file(path: &Path, bytes: &[u8]) {
    let new = path.with_extension("new");
    std::fs::write(&new, bytes).unwrap();
    std::fs::rename(&new, path).unwrap();
}

struct Bed {
    _dir: recto_testpdf::TempDir,
    path: PathBuf,
    s: Session<Real>,
    a: TabId,
    b: TabId,
    files: HashMap<String, Vec<u8>>,
}

impl Bed {
    /// A journaled session with `doc.pdf`, ten pages of "Mine", open and active, its page 9 turned and every page that names read; and a second tab.
    fn turned_page_9() -> Self {
        let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-version-change-");
        let path = dir.join("doc.pdf");
        std::fs::write(&path, ten("Mine")).unwrap();
        std::fs::write(dir.join("other.pdf"), ten("Other")).unwrap();
        let mut s = Session::new(Real);
        s.journals_mut().set_journaling(true);
        let a = s.open(&path).unwrap();
        let b = s.open(&dir.join("other.pdf")).unwrap();
        s.tap(a).unwrap();
        s.edit_pages(a, |e| e.rotate(8, 1, 1)).unwrap();
        let mut bed = Self {
            _dir: dir,
            path,
            s,
            a,
            b,
            files: HashMap::new(),
        };
        bed.identify();
        bed
    }

    /// The page-identity thread's reading of the named pages, done.
    fn identify(&mut self) {
        for job in crate::identify::work(&self.s) {
            let found = crate::identify::read(&job).expect("the edits' version");
            crate::identify::identified(&mut self.s, &found);
        }
    }

    /// Another program puts `bytes` in place, and the tab's file is read again, as going back to it does.
    fn changed_and_read(&mut self, bytes: &[u8]) {
        replace_file(&self.path, bytes);
        let look = record::look_at_file(&self.path, self.s.tabs().own_stamp(self.a));
        self.s.apply_file_status(self.a, look);
        self.read_again();
    }

    fn read_again(&mut self) {
        self.s.tap(self.b).unwrap();
        self.s.warm.clear_contents();
        self.s.tap(self.a).unwrap();
    }

    /// The page-identity thread's follow, done: the jobs, read off the lock, taken in.
    fn follow(&mut self) -> Vec<Option<version_change::Followed>> {
        version_change::work(&mut self.s)
            .iter()
            .map(|job| version_change::read(job).and_then(|matched| version_change::followed(&mut self.s, &matched)))
            .collect()
    }

    fn notice(&self) -> Option<&'static str> {
        self.s.notices().said(self.a).map(|said| said.code)
    }

    fn turned(&self) -> Vec<u32> {
        let slots = self.s.lists().get(self.a).unwrap().slots();
        (0..slots.len()).filter(|at| slots[*at].quarter_turns != 0).map(|at| at as u32).collect()
    }
}

/// Failure caught: a page inserted before the edited one by another program, and the turn shown on the page that's at its old number now (the owner's
/// rule broken), or the edits held though every page is found. Page 9 is page 10 of the new version: the turn is on it, the list is the new version's
/// eleven pages, the edits fit the file, the tab says so in the owner's words, the journal is about the new version, and undo stops at the change.
#[test]
fn a_turn_follows_its_page_when_a_page_is_inserted_before_it() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    assert_eq!(bed.notice(), Some("file_changed"), "until the match lands, held as before");
    assert!(version_change::wanted(&bed.s));
    let followed = bed.follow();
    assert_eq!(
        followed,
        vec![Some(version_change::Followed {
            tab: bed.a,
            pages: 11,
            edits: 1
        })]
    );
    assert_eq!(bed.turned(), vec![9], "the turn is on \"Mine page 9\", page 10 now, behind the cover");
    assert_eq!(bed.s.lists().get(bed.a).unwrap().slots()[9].page, 9, "the new version's own page 10");
    assert_eq!(bed.s.notices().fit(bed.a), EditsFit::Fit);
    assert_eq!(bed.s.tabs().own_stamp(bed.a), Some(file_stamp(&bed.path).unwrap()));
    assert_eq!(bed.notice(), Some("edits_followed"));
    assert_eq!(
        bed.s.notices().said(bed.a).unwrap().message,
        "This file was changed by another program. Your unsaved edits were moved onto the same pages of the new version."
    );
    assert_eq!(bed.s.pages().count(bed.a), 11);
    let list = bed.s.lists().get(bed.a).unwrap();
    assert!(!list.can_undo() && list.is_edited() && list.holds_owner_edits());
    assert!(!version_change::wanted(&bed.s), "nothing more to do");
    // The journal is the new version's, and says the list as it is now.
    write_journal(&mut bed.s, &mut bed.files);
    let name = record::journal_file_name(bed.s.tabs().own_stamp(bed.a), &bed.path, None);
    assert!(bed.files.contains_key(&name), "{name} in {:?}", bed.files.keys().collect::<Vec<_>>());
    // A save would be onto the new version, which it fits.
    let snapshot = bed.s.begin_save(bed.a, false).unwrap().unwrap();
    assert_eq!(snapshot.stamp, Some(file_stamp(&bed.path).unwrap()));
    bed.s.end_save(bed.a);
    // Not in the record: an app from before it (0.1.0, gone back to) would take the whole record for damaged.
    let record = bed.s.to_record();
    assert_eq!(record.tabs.iter().find(|tab| tab.id == bed.a.0).map(|tab| tab.notice.clone()), Some(None));
    assert!(!serde_json::to_string(&record).unwrap().contains("edits_followed"));
    // Leaving the tab lets the news go, as with any said on the way past.
    bed.s.tap(bed.b).unwrap();
    bed.s.tap(bed.a).unwrap();
    assert_eq!(bed.notice(), None);
}

/// Failure caught: the edited page's content changed under the edits, and the turn put on whatever's at its number, or followed onto a page that
/// isn't it; or the match tried again at every look. Held exactly as before: the list as it was, the tab saying the file changed in today's words
/// (which say nothing was moved), nothing asked again for this version.
#[test]
fn edits_whose_page_changed_are_held_as_before_and_not_tried_again() {
    let mut bed = Bed::turned_page_9();
    let as_they_were = bed.s.lists().edits_record(bed.a).unwrap();
    let mut theirs: Vec<recto_testpdf::Page> = (1..=10)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")])))
        .collect();
    theirs[8] = recto_testpdf::Page::new(recto_testpdf::text_lines(&["Their page 9"]));
    bed.changed_and_read(&recto_testpdf::pages(&theirs));
    assert_eq!(bed.follow(), vec![None]);
    assert_eq!(bed.s.lists().edits_record(bed.a).as_ref(), Some(&as_they_were));
    assert_eq!(bed.s.notices().fit(bed.a), EditsFit::FileChanged);
    let words = bed.s.notices().said(bed.a).unwrap().message;
    assert!(words.contains("can't be saved onto this one") && !words.contains("moved"), "{words}");
    assert!(!version_change::wanted(&bed.s));
    bed.read_again();
    assert!(version_change::work(&mut bed.s).is_empty(), "read again, the same version isn't tried again");
}

/// Failure caught (the coordinator's rule, 2026-11-15): a protected document's pages read and matched, so its edits follow (and an agent learns its
/// pages were found) where nothing of it may be derived without the person's password. A match made before the password was used isn't taken in.
#[test]
fn a_protected_tabs_edits_never_follow() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    let jobs = version_change::work(&mut bed.s);
    let matched = version_change::read(&jobs[0]).unwrap();
    bed.s.tabs.note_password_used(bed.a);
    assert_eq!(version_change::followed(&mut bed.s, &matched), None);
    assert_eq!(bed.turned(), vec![8], "held as it was");
    assert!(version_change::work(&mut bed.s).is_empty());
}

/// Failure caught: an edit made after the match was read and before it was taken in is lost or put on the wrong page, as the match was of the list
/// before it. The match is dropped, and the next job, about the list as it is, follows.
#[test]
fn an_edit_made_while_the_match_is_read_waits_for_the_next_one() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    let jobs = version_change::work(&mut bed.s);
    let matched = version_change::read(&jobs[0]).unwrap();
    bed.s.edit_pages(bed.a, |e| e.rotate(2, 1, 1)).unwrap();
    assert_eq!(version_change::followed(&mut bed.s, &matched), None);
    // The new edit's pages are read while the edits don't fit no more than slice 1 allows: they aren't, so the new turn's page can't follow.
    assert!(version_change::wanted(&bed.s), "asked again");
    assert_eq!(
        bed.follow(),
        vec![None],
        "page 3 wasn't read while the file was the edits' version, so the list is held"
    );
    assert_eq!(bed.turned(), vec![2, 8]);
}

/// Failure caught (rule 6): a scan of a long new version not cut at its caps, or one cut short that takes a digest seen once for the only one. With
/// the page cap at ten and the new version eleven pages long, renumbered by its cover, nothing is matched by digest and the list is held; with the cap
/// at eleven it follows.
#[test]
fn a_scan_cut_at_its_cap_matches_nothing_by_digest() {
    for (cap, follows) in [(10, false), (11, true)] {
        let mut bed = Bed::turned_page_9();
        bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
        SCAN_CAPS.with(|caps| caps.set(Some(ScanCaps { pages: cap, bytes: u64::MAX })));
        let followed = bed.follow();
        SCAN_CAPS.with(|caps| caps.set(None));
        assert_eq!(followed[0].is_some(), follows, "cap {cap}");
    }
}

/// Failure caught: pages another program appended can't be reached under the edits (the changed-file work's note), and a re-save that keeps the
/// object numbers is matched by them without a scan. The turn follows, and the new pages are the document's.
#[test]
fn pages_appended_by_another_program_are_reached_once_the_edits_follow() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=12).collect::<Vec<_>>(), false));
    // Nothing may be scanned: page 9 is found by its object number, which the new version keeps.
    SCAN_CAPS.with(|caps| caps.set(Some(ScanCaps { pages: 0, bytes: 0 })));
    let followed = bed.follow();
    SCAN_CAPS.with(|caps| caps.set(None));
    assert_eq!(followed[0].map(|followed| followed.pages), Some(12));
    assert_eq!(bed.turned(), vec![8]);
    assert_eq!(bed.s.pages().count(bed.a), 12);
}
