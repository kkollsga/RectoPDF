//! Reading what an edited tab's pages are while its file is the version the edits are about (`crate::identify`), on real files: what's asked, what's
//! read, and what's refused.

use super::*;
use crate::identify::{self, IdentifyJob};

/// What a test does around `Opened::open`'s open: called with `true` just before it and `false` just after.
pub(crate) type AroundOpen = Box<dyn Fn(bool)>;

std::thread_local! {
    /// How many bytes a job may read on this thread, when a test says ([`crate::identify::MAX_JOB_BYTES`]).
    pub(crate) static JOB_BYTES: Cell<Option<u64>> = const { Cell::new(None) };
    /// A version's budget for a list on this thread, when a test says (`crate::edit::identity::VERSION_BYTES`).
    pub(crate) static VERSION_BYTES: Cell<Option<u64>> = const { Cell::new(None) };
    /// What a test does to the file just before its open (`true`) and just after (`false`), on this thread.
    pub(crate) static AROUND_OPEN: std::cell::RefCell<Option<AroundOpen>> = const { std::cell::RefCell::new(None) };
}

/// The seam at `Opened::open`'s open, for a test that changes what's at the path for that moment.
pub(crate) fn around_open(before: bool) {
    AROUND_OPEN.with(|seam| {
        if let Some(seam) = seam.borrow().as_ref() {
            seam(before);
        }
    });
}

/// A loader that reads the real file's facts, as the app's does.
struct Real;

impl DocLoader for Real {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        recto_core::inspect(path)
    }
}

/// `doc.pdf`, ten pages saying their numbers, open and active, in a folder of its own.
fn open_ten() -> (recto_testpdf::TempDir, PathBuf, Session<Real>, TabId) {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-identify-");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, ten_pages("Mine")).unwrap();
    let mut s = Session::new(Real);
    let id = s.open(&path).unwrap();
    (dir, path, s, id)
}

fn ten_pages(who: &str) -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (1..=10)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

fn known(s: &Session<Real>, id: TabId, page: u32) -> Option<crate::edit::identity::Known> {
    crate::edit::identity::identities(s.lists().get(id)?)?.known(page)
}

/// Failure caught: an edit's page is never read, so its edit can't follow it when the file changes; or what's read isn't the page's identity as the
/// core makes it. A turn of page 9 asks for pages 7 to 10 (with their neighbours), off the lock; what's read is taken in, page 9's digest is the core's
/// own, and nothing more is asked.
#[test]
fn an_edited_pages_identity_is_read_and_taken_in() {
    let (_dir, path, mut s, id) = open_ten();
    s.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
    assert!(identify::has_work(&s));
    let work = identify::work(&s);
    assert_eq!(work.len(), 1);
    assert_eq!((work[0].tab, &work[0].path, work[0].pages.clone()), (id, &path, vec![6, 7, 8, 9]));
    let found = identify::read(&work[0]).expect("the file is the edits' version");
    assert!(identify::identified(&mut s, &found));
    let reader = recto_core::page_id::open_version_for_identity(&path, recto_platform::file_stamp(&path).unwrap()).unwrap();
    let map = reader.page_map().unwrap();
    let core = recto_core::page_id::identify_pages(&reader, &map, &[8])[0].unwrap();
    assert_eq!(known(&s, id, 8).map(|known| known.digest), Some(core.digest[..16].try_into().unwrap()));
    assert!(!identify::has_work(&s), "nothing more to read");
    assert!(identify::work(&s).is_empty());
}

/// Failure caught: pages read from another version of the file are taken for the edits' own, so an edit would follow a page it was never on. A file
/// replaced after the work was given and before its read gives nothing; one replaced during it (the stamp checked again after) likewise; and what was
/// read of the earlier version isn't taken in once the tab knows its file changed.
#[test]
fn another_versions_pages_are_never_taken_for_the_edits_own() {
    let (_dir, path, mut s, id) = open_ten();
    s.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
    let job = identify::work(&s).remove(0);
    let found = identify::read(&job).unwrap();
    let new = path.with_extension("new");
    std::fs::write(&new, ten_pages("Theirs")).unwrap();
    std::fs::rename(&new, &path).unwrap();
    assert_eq!(identify::read(&job), None, "the file isn't the version the job is for");
    let now = recto_platform::file_stamp(&path).unwrap();
    assert!(s.apply_file_status(id, FileStatus::Changed(now)));
    assert!(!identify::identified(&mut s, &found), "the edits no longer fit this file");
    assert_eq!(known(&s, id, 8), None);
    assert!(identify::work(&s).is_empty(), "nor is anything asked of it");
}

/// Failure caught (the coordinator's rule, 2026-11-15): a protected document's content is read for its digest, which is kept in its journal on disk,
/// though nothing of such a document may be derived without the person's password. A tab opened with one is never asked for, nor taken in.
#[test]
fn a_protected_tabs_pages_are_never_read() {
    let (_dir, _path, mut s, id) = open_ten();
    s.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
    let job: IdentifyJob = identify::work(&s).remove(0);
    let found = identify::read(&job).unwrap();
    s.tabs.note_password_used(id);
    assert!(!identify::has_work(&s));
    assert!(identify::work(&s).is_empty());
    assert!(!identify::identified(&mut s, &found));
    assert_eq!(known(&s, id, 8), None);
}

/// Failure caught: a tab naming thousands of pages makes one job read them all, holding the thread from every other tab. A job reads at most
/// `MAX_PAGES_A_JOB`, and the rest are the next one's.
#[test]
fn a_job_reads_a_bounded_number_of_pages() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-identify-many-");
    let path = dir.join("many.pdf");
    let pages: Vec<recto_testpdf::Page> = (0..identify::MAX_PAGES_A_JOB + 10)
        .map(|_| recto_testpdf::Page::new(b"0 0 m".to_vec()))
        .collect();
    std::fs::write(&path, recto_testpdf::pages(&pages)).unwrap();
    let mut s = Session::new(Real);
    let id = s.open(&path).unwrap();
    s.edit_pages(id, |e| e.rotate(0, u32::try_from(identify::MAX_PAGES_A_JOB + 10).unwrap(), 1))
        .unwrap();
    let work = identify::work(&s);
    assert_eq!(work[0].pages.len(), identify::MAX_PAGES_A_JOB);
    let found = identify::read(&work[0]).unwrap();
    assert!(identify::identified(&mut s, &found));
    let next = identify::work(&s);
    assert_eq!((next[0].tab, next[0].pages.len()), (id, 10));
}

/// Failure caught (review: "a job is bounded by bytes, not by 2 seconds"): a job that reads for a while gives up the pages it didn't get to, so a big
/// scanned book on a slow disk or a loaded Mac has pages silently dropped; or a job reads without bound. With the budget set to one byte, a job reads
/// one page and hands it in; the rest stay named and unread, the next job reads the next, and nothing is given up.
#[test]
fn a_job_reads_to_its_byte_budget_and_the_next_goes_on() {
    let (_dir, _path, mut s, id) = open_ten();
    s.edit_pages(id, |e| e.rotate(0, 10, 1)).unwrap();
    JOB_BYTES.with(|bytes| bytes.set(Some(1)));
    let mut opened = None;
    for round in 1..=10 {
        let job = identify::work(&s).remove(0);
        let found = identify::read_in(&job, &mut opened).unwrap();
        assert_eq!((found.pages.len(), found.tried.len()), (1, 0), "round {round}");
        assert!(identify::identified(&mut s, &found));
    }
    JOB_BYTES.with(|bytes| bytes.set(None));
    let ids = crate::edit::identity::identities(s.lists().get(id).unwrap()).unwrap();
    assert!(!ids.over_cap());
    assert!(ids.pages().iter().all(|(_, known)| known.is_some()));
    assert!(!identify::has_work(&s), "every page read");
}

/// Ten pages saying their numbers, the fourth past a cap: its content more than 16 MiB, flated so the file stays small.
fn ten_with_a_capped_page() -> Vec<u8> {
    use recto_testpdf::Object;
    use std::io::Write;
    let mut flated = Vec::new();
    let mut encoder = flate2::write::ZlibEncoder::new(&mut flated, flate2::Compression::best());
    encoder.write_all(&vec![b' '; (16 << 20) + 1]).unwrap();
    encoder.finish().unwrap();
    let kids: String = (0..10).map(|at| format!("{} 0 R ", 3 + at)).collect();
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Object::Dict(format!("<< /Type /Pages /Kids [{kids}] /Count 10 >>")),
    ];
    objects.extend((0..10).map(|at| Object::Dict(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>", 13 + at))));
    objects.extend((0..10).map(|at| {
        if at == 3 {
            Object::Stream {
                dict: "/Filter /FlateDecode".into(),
                data: flated.clone(),
            }
        } else {
            Object::Stream {
                dict: String::new(),
                data: format!("BT /F1 12 Tf 72 712 Td (Page {}) Tj ET", at + 1).into_bytes(),
            }
        }
    }));
    recto_testpdf::build(&objects)
}

/// `capped.pdf`, ten pages, the fourth past a cap, open in a session of its own, every page turned.
fn open_capped() -> (recto_testpdf::TempDir, PathBuf, Session<Real>, TabId) {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-identify-capped-");
    let path = dir.join("capped.pdf");
    std::fs::write(&path, ten_with_a_capped_page()).unwrap();
    let mut s = Session::new(Real);
    let id = s.open(&path).unwrap();
    s.edit_pages(id, |e| e.rotate(0, 10, 1)).unwrap();
    (dir, path, s, id)
}

/// Failure caught (second review, F2): a page past a cap (a real large map, or an image over 8 MiB in a file anyone may open) turns following off for
/// its whole file version, though it was named only as a neighbour; or it's read again after every edit, so an agent's edits keep the thread busy on
/// it. A job reads past it: the nine other pages are taken in, the capped page alone is named no more and not named again by another edit, and the
/// list isn't closed or said to be past its cap.
#[test]
fn a_capped_page_marks_only_itself() {
    let (_dir, _path, mut s, id) = open_capped();
    let job = identify::work(&s).remove(0);
    let found = identify::read(&job).unwrap();
    assert_eq!(found.tried, vec![3]);
    assert_eq!(found.pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![0, 1, 2, 4, 5, 6, 7, 8, 9]);
    assert!(identify::identified(&mut s, &found));
    let ids = crate::edit::identity::identities(s.lists().get(id).unwrap()).unwrap();
    assert!(!ids.over_cap(), "the other pages are followed");
    assert_eq!(ids.pages().len(), 9, "the capped page isn't named");
    assert!(ids.pages().iter().all(|(_, known)| known.is_some()));
    assert!(!identify::has_work(&s), "nothing asked again");
    s.edit_pages(id, |e| e.rotate(3, 1, 1)).unwrap();
    assert!(!identify::has_work(&s), "nor after an edit of the capped page, on this version");
    s.edit_pages(id, |e| e.rotate(2, 1, 1)).unwrap();
    assert!(!identify::has_work(&s), "nor of its neighbour");
}

/// What reading each of the ten pages of `path` costs, as the job counts it.
fn costs(path: &Path) -> Vec<u64> {
    let stamp = recto_platform::file_stamp(path).unwrap();
    let reader = recto_core::page_id::open_version_for_identity(path, stamp).unwrap();
    let map = reader.page_map().unwrap();
    map.iter()
        .map(|entry| recto_core::page_id::read_page(&reader, entry, &recto_core::page_id::Caps::STANDARD).bytes)
        .collect()
}

/// Failure caught (second review, F2, rules 1 and 6): a file version's reading has no bound but its pages' caps, so a crafted file whose every page
/// costs just under them (one large stream that decodes to nothing, referenced from each page's `/Contents`) keeps one core reading for half an hour
/// or more with every hand-off refused as busy, and an agent at Control can name its 8,192 pages with one turn. With the version's budget set to what
/// the first three pages cost: three are read and the list is closed (the pages not read named no more, the list past its cap, nothing asked again);
/// at one byte more, a fourth is read before it closes.
#[test]
fn a_versions_reading_stops_at_its_budget() {
    for (more, read) in [(0, 3), (1, 4)] {
        let (_dir, path, mut s, id) = open_ten();
        let budget = costs(&path)[..3].iter().sum::<u64>() + more;
        s.edit_pages(id, |e| e.rotate(0, 10, 1)).unwrap();
        VERSION_BYTES.with(|bytes| bytes.set(Some(budget)));
        let mut opened = None;
        while let Some(job) = identify::work(&s).pop() {
            let found = identify::read_in(&job, &mut opened).unwrap();
            assert!(identify::identified(&mut s, &found));
        }
        VERSION_BYTES.with(|bytes| bytes.set(None));
        let ids = crate::edit::identity::identities(s.lists().get(id).unwrap()).unwrap();
        assert_eq!(
            ids.pages().iter().map(|(page, _)| *page).collect::<Vec<_>>(),
            (0..read).collect::<Vec<u32>>(),
            "budget + {more}"
        );
        assert!(ids.pages().iter().all(|(_, known)| known.is_some()));
        assert!(ids.over_cap(), "some pages can't be followed");
        assert!(!identify::has_work(&s));
        s.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
        assert!(!identify::has_work(&s), "nor after another edit, on this version");
    }
}

/// Failure caught (slice 2's review, 2026-11-15): the file's version is checked by its path before and after the read, and never on what was opened,
/// so someone who can write the folder renames another file into place for the moment of the open and the original back (the same file number, size
/// and time on both looks): the identities recorded are the other file's, which a later crafted version uses to pull an edit onto another page. With
/// another file put at the path for the open alone, nothing is read.
#[test]
fn a_file_swapped_in_for_the_open_alone_is_never_read() {
    let (dir, path, mut s, id) = open_ten();
    let other = dir.join("other.pdf");
    std::fs::write(&other, ten_pages("Theirs")).unwrap();
    s.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
    let job = identify::work(&s).remove(0);
    let (kept, at, theirs) = (dir.join("doc.keep"), path.clone(), other.clone());
    AROUND_OPEN.with(|seam| {
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
    let found = identify::read(&job);
    AROUND_OPEN.with(|seam| *seam.borrow_mut() = None);
    assert_eq!(
        recto_platform::file_stamp(&path).unwrap(),
        job.stamp,
        "the path looks as it did, before and after"
    );
    assert_eq!(found, None, "the other file's pages were read for this one's");
}

/// Failure caught (second review): what was read of a tab's pages waits for its sidecar, and the tab's version then goes unknown (a file replaced
/// mid-read, whose stamp the session drops, `tiers.rs`): the sidecar can't be written without a version, so the journals say they have work for good,
/// and the tab can never checkpoint or go dormant. A sidecar that can't be written holds nothing: no work, and the tab checkpoints.
#[test]
fn a_tab_whose_version_goes_unknown_isnt_held_by_its_sidecar() {
    let (_dir, _path, mut s, id) = open_ten();
    s.journals_mut().set_journaling(true);
    s.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
    let found = identify::read(&identify::work(&s).remove(0)).unwrap();
    assert!(identify::identified(&mut s, &found));
    // The journal's operation is written, and the sidecar isn't yet.
    let work = s.journal_desk().work();
    assert_eq!(work.pages.len(), 1, "the sidecar's entries wait");
    let written = work.appends.iter().map(|append| (append.tab, append.through, append.name.clone())).collect();
    s.journal_desk().outcome(JournalOutcome {
        written,
        ..JournalOutcome::default()
    });
    assert!(s.journal_look().has_work(), "the sidecar waits, with a version to write it for");
    s.tabs.tab_mut(id).unwrap().file.stamp = None;
    assert!(s.journal_desk().work().pages.is_empty(), "nothing to write it for");
    assert!(!s.journal_look().has_work(), "and nothing waits for it");
    assert!(s.journal_look().checkpoint_if_caught_up(id).is_some(), "the tab may go to disk");
}

/// A loader that says a file has two pages more than its page tree has, as a file whose renderer and structure reader count differently can.
struct TwoMore;

impl DocLoader for TwoMore {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        let mut facts = recto_core::inspect(path)?;
        facts.page_count += 2;
        Ok(facts)
    }
}

/// Failure caught (second review): a named page the file's page tree doesn't have stays wanted for good, so every call with journal work wakes the
/// page-identity thread, which opens the file again and builds its page map again for nothing. It's tried, as a page past a cap is: one job, and
/// nothing asked again.
#[test]
fn a_page_the_page_tree_doesnt_have_is_tried() {
    let (_dir, path, _, _) = open_ten();
    let mut s = Session::new(TwoMore);
    let id = s.open(&path).unwrap();
    s.edit_pages(id, |e| e.rotate(11, 1, 1)).unwrap();
    let job = identify::work(&s).remove(0);
    assert_eq!(job.pages, vec![9, 10, 11]);
    let found = identify::read(&job).unwrap();
    assert_eq!((found.pages.len(), found.tried.clone()), (1, vec![10, 11]));
    assert!(identify::identified(&mut s, &found));
    assert!(!identify::has_work(&s), "nothing asked again");
}
