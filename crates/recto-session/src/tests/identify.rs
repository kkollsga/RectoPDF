//! Reading what an edited tab's pages are while its file is the version the edits are about (`crate::identify`), on real files: what's asked, what's
//! read, and what's refused.

use super::*;
use crate::identify::{self, IdentifyJob};

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
