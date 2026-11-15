//! Unsaved edits following their pages into another version of their file (`crate::version_change`), on real files: the match read off the lock,
//! taken in under it, and what the tab says. The pure rules are the table in `edit/rebase/table.rs`; this is that the session does what they say.

use super::*;
use crate::edit::matcher::WorkCaps;
use crate::version_change;

std::thread_local! {
    /// A follow's one budget on this thread, when a test sets it small (`crate::version_change::work_caps`).
    pub(crate) static WORK_CAPS: Cell<Option<WorkCaps>> = const { Cell::new(None) };
    /// How many bytes a step of a follow's reading may read on this thread, when a test says (`crate::version_change::step_bytes`).
    pub(crate) static STEP_BYTES: Cell<Option<u64>> = const { Cell::new(None) };
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

/// `bytes` with an `/ID` pair in its trailer, as a writer that keeps the document's `/ID[0]` across saves gives it (the trailer is after the cross
/// reference, so nothing moves).
fn with_id(bytes: &[u8], first: &str, second: &str) -> Vec<u8> {
    let at = bytes.windows(7).rposition(|window| window == b"trailer").expect("a trailer");
    let (head, tail) = bytes.split_at(at);
    let tail = String::from_utf8_lossy(tail).replacen(" >>", &format!(" /ID [({first}) ({second})] >>"), 1);
    [head, tail.as_bytes()].concat()
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
        Self::turned_page_9_of(&ten("Mine"))
    }

    /// [`Self::turned_page_9`], with `doc.pdf` this.
    fn turned_page_9_of(bytes: &[u8]) -> Self {
        let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-version-change-");
        let path = dir.join("doc.pdf");
        std::fs::write(&path, bytes).unwrap();
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

/// Failure caught: the person left a tab whose edits followed by opening another PDF, and the notice stayed, with the sidecar still saying "not yet
/// told", so every restart said it again. Left by an open: told, as by a tap.
#[test]
fn opening_another_file_tells_a_follow_as_a_tap_does() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    assert!(bed.follow()[0].is_some());
    assert_eq!(bed.notice(), Some("edits_followed"));
    let third = bed.path.with_file_name("third.pdf");
    std::fs::write(&third, ten("Third")).unwrap();
    bed.s.open(&third).unwrap();
    assert_eq!(bed.notice(), None, "left: said");
    assert!(
        !crate::edit::identity::followed_untold(bed.s.lists().get(bed.a).unwrap()),
        "and told, so the sidecar says so"
    );
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

/// Failure caught (rule 6, review F2): a scan of a long new version not cut at the follow's one budget, or one cut short that takes a digest seen
/// once for the only one. With the budget at ten pages and the new version eleven pages long, renumbered by its cover, nothing is matched by digest and
/// the list is held; with room for the scan and the pass by object number before it (which counts against the same budget), it follows. The limit plus
/// one exactly is the table's (`edit/rebase/table.rs`), where what each page costs is known.
#[test]
fn a_scan_cut_at_its_cap_matches_nothing_by_digest() {
    for (cap, follows) in [(10, false), (11 + 6, true)] {
        let mut bed = Bed::turned_page_9();
        bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
        WORK_CAPS.with(|caps| {
            caps.set(Some(WorkCaps {
                pages: cap,
                ..WorkCaps::STANDARD
            }));
        });
        let followed = bed.follow();
        WORK_CAPS.with(|caps| caps.set(None));
        assert_eq!(followed[0].is_some(), follows, "cap {cap}");
    }
}

/// Failure caught: pages another program appended can't be reached under the edits (the changed-file work's note), and a re-save that keeps the
/// object numbers is matched by them without a scan. The turn follows, and the new pages are the document's.
#[test]
fn pages_appended_by_another_program_are_reached_once_the_edits_follow() {
    let mut bed = Bed::turned_page_9_of(&with_id(&ten("Mine"), "mine", "one"));
    // The same document saved again, its `/ID[0]` kept (only then is an object number evidence, review M1).
    bed.changed_and_read(&with_id(&document("Mine", &(1..=12).collect::<Vec<_>>(), false), "mine", "two"));
    // No scan: page 9 and the pages beside it are found by their object numbers, which the new version keeps, and five pages read is all the
    // budget allows, where a scan of the new version would read twelve.
    WORK_CAPS.with(|caps| {
        caps.set(Some(WorkCaps {
            pages: 5,
            ..WorkCaps::STANDARD
        }));
    });
    let followed = bed.follow();
    WORK_CAPS.with(|caps| caps.set(None));
    assert_eq!(followed[0].map(|followed| followed.pages), Some(12));
    assert_eq!(bed.turned(), vec![8]);
    assert_eq!(bed.s.pages().count(bed.a), 12);
}

/// Puts `bytes` in the file's place by a rename, with the time the file had put back: the same size and time, another inode.
fn renamed_in_with_time_kept(path: &Path, bytes: &[u8]) {
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    let new = path.with_extension("new");
    std::fs::write(&new, bytes).unwrap();
    std::fs::File::options().write(true).open(&new).unwrap().set_modified(modified).unwrap();
    std::fs::rename(&new, path).unwrap();
}

/// Failure (the changed-file work's review, 2026-11-13, left for the matcher): a different file of the same size, time and page count renamed into
/// place was taken for the version the edits were made on, as a look goes by size and time: the tab took the new file's number as the edits' own, and
/// showed the turn on the other file's page 9 (and a save's own check of the number then passed). Another inode under unsaved edits is another version:
/// with page 9 not in it the edits are held and said; with the same pages (a copy put back, as `cp -p` makes), they follow.
#[test]
fn another_file_renamed_in_with_the_same_size_and_time_is_another_version() {
    let mut bed = Bed::turned_page_9();
    let made_on = file_stamp(&bed.path).unwrap();
    let mine = std::fs::read(&bed.path).unwrap();
    let theirs = ten("Ours");
    assert_eq!(theirs.len(), mine.len(), "the same size");
    renamed_in_with_time_kept(&bed.path, &theirs);
    let now = file_stamp(&bed.path).unwrap();
    assert!(
        now.same_contents(made_on) && now.inode != made_on.inode,
        "size and time tell nothing; the inode does"
    );
    let look = record::look_at_file(&bed.path, bed.s.tabs().own_stamp(bed.a));
    bed.s.apply_file_status(bed.a, look);
    assert_eq!(bed.s.notices().fit(bed.a), EditsFit::FileChanged, "not taken for the edits' version");
    assert_eq!(bed.s.tabs().own_stamp(bed.a), Some(made_on));
    bed.read_again();
    assert_eq!(bed.follow(), vec![None], "page 9 isn't in it, so the edits are held");
    assert_eq!(bed.notice(), Some("file_changed"));
    // The edits' own pages put back by another copy: another inode again, and every page found.
    renamed_in_with_time_kept(&bed.path, &mine);
    let look = record::look_at_file(&bed.path, bed.s.tabs().own_stamp(bed.a));
    bed.s.apply_file_status(bed.a, look);
    bed.read_again();
    assert_eq!(bed.follow().len(), 1);
    assert_eq!(bed.s.notices().fit(bed.a), EditsFit::Fit);
    assert_eq!(bed.turned(), vec![8]);
}

/// Failure caught (review S1): the new version is checked by its path before and after the read, but opened by its path too, so another file renamed in
/// for the moment of the open alone (with the version found back in place after it) is what's matched, and the edits follow onto pages of a file that
/// was never there for them. With the edits' own first version swapped in for the open alone, nothing is matched: what's opened is checked by its own
/// descriptor against the version found.
#[test]
fn a_file_swapped_in_for_the_open_of_the_match_alone_is_never_matched() {
    let mut bed = Bed::turned_page_9();
    let mine = std::fs::read(&bed.path).unwrap();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    let jobs = version_change::work(&mut bed.s);
    let dir = bed.path.parent().unwrap().to_path_buf();
    let other = dir.join("swap.pdf");
    std::fs::write(&other, &mine).unwrap();
    let (kept, at, theirs) = (dir.join("doc.keep"), bed.path.clone(), other.clone());
    super::identify::AROUND_OPEN.with(|seam| {
        *seam.borrow_mut() = Some(Box::new(move |before| {
            if before {
                std::fs::rename(&at, &kept).unwrap();
                std::fs::rename(&theirs, &at).unwrap();
            } else {
                std::fs::rename(&at, &theirs).unwrap();
                std::fs::rename(&kept, &at).unwrap();
            }
        }));
    });
    let matched = version_change::read(&jobs[0]);
    super::identify::AROUND_OPEN.with(|seam| *seam.borrow_mut() = None);
    assert_eq!(file_stamp(&bed.path).unwrap(), jobs[0].to, "the path looks as it did, before and after");
    assert_eq!(matched, None, "another file's pages were matched for the version found");
}

/// Failure caught (review S2): a Discard after the edits followed leaves the tab saying "Your unsaved edits were moved onto the same pages of the new
/// version", with none left, and `edits` (which says `followed` while the notice stands) still telling an agent they followed. Discarded: no notice,
/// no list, the file as it is.
#[test]
fn a_discard_after_the_edits_followed_lets_the_notice_go() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    assert!(bed.follow()[0].is_some());
    assert_eq!(bed.notice(), Some("edits_followed"));
    bed.s.discard_edits(bed.a);
    assert_eq!(bed.notice(), None, "nothing was moved that's still here");
    assert_eq!(bed.s.tabs().tab_notice(bed.a), None);
    assert!(bed.s.lists().get(bed.a).is_none_or(|list| !list.is_edited()));
}

/// Failure caught (review F2): a follow read in one piece, holding the page-identity thread for as long as the new version takes to read, however long
/// that is; or one that, taken a step at a time, matches otherwise than in one piece. With a step of one byte (each step one page), the reading takes
/// many steps and comes to the same match.
#[test]
fn a_follow_is_read_a_step_at_a_time_and_comes_to_the_same_match() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    let job = version_change::work(&mut bed.s).remove(0);
    let whole = version_change::read(&job).expect("read whole");
    STEP_BYTES.with(|bytes| bytes.set(Some(1)));
    let mut following = version_change::start(job.clone()).expect("opened");
    let mut steps = 0;
    let matched = loop {
        steps += 1;
        match version_change::read_part(&mut following) {
            version_change::Read::More => {}
            version_change::Read::Matched(matched) => break *matched,
            version_change::Read::NotRead => panic!("not read"),
        }
    };
    STEP_BYTES.with(|bytes| bytes.set(None));
    assert!(steps > 11, "a page a step, over the pass by object number and the scan: {steps} steps");
    assert_eq!(matched, whole);
}

/// Failure caught: a follow being read across steps goes on after the person edits the list, or the file changes again, and its match of the list as it
/// was is taken in. Between steps it's asked whether it's still wanted: after an edit it isn't, and the next job is about the list as it is.
#[test]
fn a_follow_being_read_is_dropped_when_the_list_is_edited_between_its_steps() {
    let mut bed = Bed::turned_page_9();
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    let job = version_change::work(&mut bed.s).remove(0);
    assert!(version_change::current(&bed.s, &job));
    bed.s.edit_pages(bed.a, |e| e.rotate(2, 1, 1)).unwrap();
    assert!(!version_change::current(&bed.s, &job), "an edit since: not this job's any more");
}

/// Failure caught (review: "with no unsaved edits behaviour is exactly `main`'s"): a tab with only history (its edit undone, nothing unsaved) and its
/// file put back by a rename with the same bytes, size and time, another inode: `main` takes it for the same version and keeps the history (redo
/// still brings the edit back) and says nothing; slice 2's inode rule, meant for unsaved edits only, took it for another version, let the history go
/// and said the file changed.
#[test]
fn with_only_history_a_file_renamed_in_with_the_same_size_and_time_is_main_s_same_file() {
    let mut bed = Bed::turned_page_9();
    let mine = std::fs::read(&bed.path).unwrap();
    assert!(bed.s.undo_edit(bed.a));
    let list = bed.s.lists().get(bed.a).unwrap();
    assert!(!list.is_edited() && list.can_redo());
    renamed_in_with_time_kept(&bed.path, &mine);
    // Read again before any look has taken the new file's number in, as going back to the tab does.
    bed.read_again();
    assert!(bed.s.lists().get(bed.a).is_some_and(PageEdits::can_redo), "the history kept, as on main");
    assert_eq!(bed.notice(), None);
}

/// Failure caught (the owner's all-or-nothing for 0.1.1): a list of several edits of which one can't follow (its page changed) followed in part, the
/// others put on the new version, or the held list not exactly what it is without the matcher. Held whole: the list's record and history as they
/// were, the tab saying the file changed in today's words, its own version and length the edits' version's, nothing to try again.
#[test]
fn a_list_where_one_edit_of_several_cant_follow_is_held_whole_as_before() {
    let mut bed = Bed::turned_page_9();
    bed.s.edit_pages(bed.a, |e| e.rotate(2, 1, 1)).unwrap();
    bed.s.edit_pages(bed.a, |e| e.move_pages(0, 1, 5)).unwrap();
    bed.identify();
    let (record_before, stamp_before, pages_before) = (
        bed.s.lists().edits_record(bed.a).unwrap(),
        bed.s.tabs().own_stamp(bed.a),
        bed.s.pages().count(bed.a),
    );
    let can_undo = bed.s.lists().get(bed.a).unwrap().can_undo();
    let mut theirs: Vec<recto_testpdf::Page> = (1..=10)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")])))
        .collect();
    theirs[2] = recto_testpdf::Page::new(recto_testpdf::text_lines(&["Their page 3"]));
    bed.changed_and_read(&recto_testpdf::pages(&theirs));
    assert_eq!(bed.follow(), vec![None], "page 3 changed: held, though pages 1 and 9 are there");
    assert_eq!(bed.s.lists().edits_record(bed.a).as_ref(), Some(&record_before), "the list as it was");
    assert_eq!(bed.s.lists().get(bed.a).unwrap().can_undo(), can_undo, "and its history");
    assert_eq!(bed.s.tabs().own_stamp(bed.a), stamp_before, "the edits' own version");
    assert_eq!(bed.s.pages().count(bed.a), pages_before);
    assert_eq!(bed.s.notices().fit(bed.a), EditsFit::FileChanged);
    assert_eq!(bed.notice(), Some("file_changed"));
    assert_eq!(bed.s.notices().said(bed.a).unwrap().message, FileNotice::Changed.message(true), "today's words");
    assert!(!version_change::wanted(&bed.s));
}

/// Failure caught: a list past its cap of named pages (an edit on a page not named) followed, the unnamed page's edit then guessed at. With the cap set
/// small, a list naming more than it holds is held whole when the file changes, and nothing of the new version is read for it.
#[test]
fn a_list_past_its_cap_of_named_pages_is_held() {
    crate::edit::identity::tests::CAP.with(|cap| cap.set(Some(4)));
    let mut bed = Bed::turned_page_9();
    bed.s.edit_pages(bed.a, |e| e.rotate(2, 1, 1)).unwrap();
    bed.identify();
    let over = crate::edit::identity::identities(bed.s.lists().get(bed.a).unwrap()).is_some_and(crate::edit::identity::Identities::over_cap);
    bed.changed_and_read(&document("Mine", &(1..=10).collect::<Vec<_>>(), true));
    let jobs = version_change::work(&mut bed.s);
    crate::edit::identity::tests::CAP.with(|cap| cap.set(None));
    assert!(over, "past the cap");
    assert!(jobs.is_empty(), "nothing read for it");
    assert_eq!(bed.notice(), Some("file_changed"));
    assert_eq!(bed.turned(), vec![2, 8], "held as it was");
}
