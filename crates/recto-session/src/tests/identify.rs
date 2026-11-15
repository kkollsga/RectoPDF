//! Reading what an edited tab's pages are while its file is the version the edits are about (`crate::identify`), on real files: what's asked, what's
//! read, and what's refused.

use super::*;
use crate::identify::{self, IdentifyJob};

std::thread_local! {
    /// How many bytes a job may read on this thread, when a test says ([`crate::identify::MAX_JOB_BYTES`]).
    pub(crate) static JOB_BYTES: Cell<Option<u64>> = const { Cell::new(None) };
    /// What a test does to the file just before its open (`true`) and just after (`false`), on this thread.
    static AROUND_OPEN: std::cell::RefCell<Option<Box<dyn Fn(bool)>>> = const { std::cell::RefCell::new(None) };
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
    let reader = recto_core::page_id::open_for_identity(&path).unwrap();
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
        assert_eq!((found.pages.len(), found.gave_up), (1, false), "round {round}");
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

/// Failure caught (rules 1 and 6): a page with no identity (past a cap, up to a second of reading) is read again after every edit, so an agent's edits
/// keep the thread busy on a hostile file; or reading goes on past it. The job stops at it and gives up: the three pages before it are taken in, the
/// pages not read are named no more, the list says some pages can't be followed, and nothing of this version is asked again, whatever is edited next.
#[test]
fn a_page_with_no_identity_gives_up_on_this_version() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-identify-capped-");
    let path = dir.join("capped.pdf");
    std::fs::write(&path, ten_with_a_capped_page()).unwrap();
    let mut s = Session::new(Real);
    let id = s.open(&path).unwrap();
    s.edit_pages(id, |e| e.rotate(0, 10, 1)).unwrap();
    let job = identify::work(&s).remove(0);
    let found = identify::read(&job).unwrap();
    assert!(found.gave_up);
    assert_eq!(found.pages.iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert!(identify::identified(&mut s, &found));
    let ids = crate::edit::identity::identities(s.lists().get(id).unwrap()).unwrap();
    assert!(ids.over_cap());
    assert_eq!(ids.pages().len(), 3, "only what was read stays named");
    assert!(!identify::has_work(&s), "nothing asked again");
    s.edit_pages(id, |e| e.rotate(7, 1, 1)).unwrap();
    assert!(!identify::has_work(&s), "nor after another edit, on this version");
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
