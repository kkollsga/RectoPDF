//! References on search hits made from the identities an indexer stored beside the text, against the same references made from the file. Each test names the
//! failure it catches.

use super::*;
use crate::index::ReadOnlyCorpus;
use recto_index::corpus::{BuildLimits, DocMeta, FedPage, FeedError, FileIdentities, IdentityInfo, PageFeed};
use recto_platform::find_key::FindKey;
use std::path::PathBuf;

/// A corpus of a PDF's pages, built the way the indexer builds one: the text of each page, and (when `with_identities`) its identities read by lopdf.
struct Built {
    _folder: recto_testpdf::TempDir,
    root: PathBuf,
    key: CorpusKey,
    indexing: ReadOnlyCorpus,
}

struct Feed {
    ids: Option<FileIdentities>,
    pages: u32,
}

impl PageFeed for Feed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(self.pages)
    }
    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        let text = format!("Page {page}");
        Ok(FedPage {
            chars: text.chars().map(|c| recto_proto::CharRecord::new(c as u32, 0)).collect(),
            boxes: Vec::new(),
            hidden_checked: true,
        })
    }
    fn still_the_same_file(&mut self) -> bool {
        true
    }
    fn identities(&mut self) -> Option<IdentityInfo> {
        self.ids.as_ref().map(FileIdentities::info)
    }
    fn identity(&mut self, page: u32) -> recto_core::page_id::PageRead {
        self.ids.as_ref().expect("asked only when it answered").read(page)
    }
}

fn build(file: &TempPdf, pages: u32, with_identities: bool) -> Built {
    let folder = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-hits-ids-");
    recto_platform::local::ensure_private_dir(&folder).unwrap();
    let root = folder.join("corpus.noindex");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let ids = with_identities.then(|| FileIdentities::open(&file.0, recto_platform::file_stamp(&file.0).unwrap()).expect("lopdf opens it"));
    let find_key = FindKey::for_tests(1);
    recto_index::corpus::build(&find_key, &root, key, &mut Feed { ids, pages }, &DocMeta::default(), &BuildLimits::default()).unwrap();
    Built {
        _folder: folder,
        indexing: ReadOnlyCorpus(root.clone(), Some(find_key)),
        root,
        key,
    }
}

/// One reference to each of `pages`, made as a search with a hit on each makes them: from `corpus`'s index when it's given, from the file when not.
fn refer(file: &TempPdf, pages: &[u32], built: Option<&Built>) -> (Vec<Option<String>>, Value, OnceCell<Option<Held>>) {
    let opened = OnceCell::new();
    let tokens;
    let said;
    {
        let mut refs = HitRefs::new(Some(file.0.as_path()), &opened, far(), built.map(|built| &built.indexing as &dyn Indexing));
        let mut hits: Vec<Value> = pages
            .iter()
            .map(|&page| {
                let mut hit = hit();
                refs.mark(&mut hit, false, page, "some words", Some(&(0..5)), built.map(|built| built.key));
                hit
            })
            .collect();
        said = refs.finish(&mut Value::Array(hits.clone()));
        tokens = hits.iter_mut().map(|hit| hit["ref"].as_str().map(str::to_owned)).collect();
    }
    (tokens, said, opened)
}

fn find(root: &std::path::Path, name: &str) -> PathBuf {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.file_name().is_some_and(|file| file == name) {
                found.push(path);
            }
        }
    }
    assert_eq!(found.len(), 1, "{name}: {found:?}");
    found.remove(0)
}

fn is_stored(opened: &OnceCell<Option<Held>>) -> bool {
    matches!(opened.get(), Some(Some(Held::Stored(..))))
}

/// Failure: the stored route makes another token than the file's own (a different tag, or a page's identity from the wrong record), or it still opens the
/// file with lopdf for each search, which is the cost the stored identities are there to take away. The same pages are referred to both ways, with a hit on
/// the same pages (the tag pages and later ones), and the cell holds the stored source and not an opened file.
#[test]
fn a_search_from_identities_opens_no_file_and_makes_the_files_own_tokens() {
    let file = numbered(30);
    let built = build(&file, 30, true);
    let pages: Vec<u32> = (0..30).rev().collect();
    let (live, said_live, live_opened) = refer(&file, &pages, None);
    let (stored, said_stored, stored_opened) = refer(&file, &pages, Some(&built));
    assert!(live.iter().all(Option::is_some), "the file's own references were made");
    assert_eq!(stored, live, "the same tokens either way");
    assert_eq!(said_stored["made"], said_live["made"]);
    assert_eq!(said_stored["missing"], said_live["missing"]);
    assert!(matches!(live_opened.get(), Some(Some(Held::Live(_)))));
    assert!(is_stored(&stored_opened), "the stored source answered, and no file was opened");
}

/// Failure: an index built before identities were stored (it has neither file) is taken as damaged, thrown away or answered without references, instead of
/// being referred to by reading the file as before.
#[test]
fn an_index_without_identities_answers_and_refers_by_reading_the_file() {
    let file = numbered(5);
    let built = build(&file, 5, false);
    let (live, ..) = refer(&file, &[0, 3, 4], None);
    let (old, said, opened) = refer(&file, &[0, 3, 4], Some(&built));
    assert_eq!(old, live);
    assert_eq!(said["missing"], json!({}));
    assert!(matches!(opened.get(), Some(Some(Held::Live(_)))), "read from the file");
    let find_key = FindKey::for_tests(1);
    assert!(
        matches!(recto_index::corpus::Identities::open(&find_key, &built.root, built.key), Ok(None)),
        "nothing was made of it, or thrown away"
    );
}

/// Failure: a search with a `--scan`, a password or an added file's page gets its reference from stored identities. They have no corpus key, so the
/// file is read.
#[test]
fn a_page_not_read_from_the_index_is_referred_to_from_the_file_even_with_identities_there() {
    let file = numbered(5);
    let built = build(&file, 5, true);
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened, far(), Some(&built.indexing));
    let mut hit = hit();
    refs.mark(&mut hit, false, 2, "some words", Some(&(0..5)), None);
    assert!(hit["ref"].is_string());
    assert!(matches!(opened.get(), Some(Some(Held::Live(_)))));
}

/// Failure: the digest caps in a search are not the standard ones the identities were made under, and a stored read answers for pages read under other
/// rules. The file is read.
#[test]
fn caps_that_are_not_the_standard_digest_caps_read_the_file() {
    let file = numbered(5);
    let built = build(&file, 5, true);
    let other = ScanCaps {
        page: recto_core::page_id::Caps {
            content_streams: 1,
            ..recto_core::page_id::Caps::STANDARD
        },
        ..HIT_REF_CAPS
    };
    let opened = OnceCell::new();
    let mut refs = HitRefs::with_caps(Some(file.0.as_path()), &opened, &other, far(), Some(&built.indexing));
    let mut hit = hit();
    refs.mark(&mut hit, false, 2, "some words", Some(&(0..5)), Some(built.key));
    assert!(matches!(opened.get(), Some(Some(Held::Live(_)))));
}

/// Failure: the caps hold on one source and not the other: at the limit a full answer is referred to whole, and one page more is counted `caps`, from the
/// file and from the identities alike.
#[test]
fn hit_ref_caps_hold_over_stored_identities_as_over_the_file() {
    let file = numbered(230);
    let built = build(&file, 230, true);
    caps_hold(&file, Some(built.key), Some(&built.indexing));
}

/// Failure: a file rewritten in place with its size and modification time put back keeps references made from identities stored for the file it was. Its
/// `FileStamp` (size, time, device and inode) is the same after, which is what `finish` checked before; only the corpus key, which has the change time,
/// differs. Probed on this macOS (2026-11-26): the stamps are equal and the keys are not, asserted below, so the test fails if that stops being so.
#[test]
fn a_file_rewritten_in_place_with_its_size_and_time_restored_withdraws_every_stored_ref() {
    let file = numbered(4);
    let built = build(&file, 4, true);
    let before = recto_platform::file_stamp(&file.0).unwrap();
    let modified = std::fs::metadata(&file.0).unwrap().modified().unwrap();
    let opened = OnceCell::new();
    let mut refs = HitRefs::new(Some(file.0.as_path()), &opened, far(), Some(&built.indexing));
    let mut hits = json!([hit(), hit()]);
    for index in 0..2 {
        refs.mark(&mut hits[index], false, index as u32, "some words", Some(&(0..5)), Some(built.key));
    }
    assert!(is_stored(&opened) && hits[1]["ref"].is_string(), "made from the stored identities");
    // The same size (one byte of a content stream changed), the same file, the time put back.
    let mut bytes = std::fs::read(&file.0).unwrap();
    let at = bytes.windows(6).position(|window| window == b"Page 1").expect("the page's text");
    bytes[at + 5] = b'7';
    {
        use std::io::{Seek, Write};
        let mut open = std::fs::OpenOptions::new().write(true).open(&file.0).unwrap();
        open.seek(std::io::SeekFrom::Start(0)).unwrap();
        open.write_all(&bytes).unwrap();
        open.set_modified(modified).unwrap();
    }
    assert_eq!(recto_platform::file_stamp(&file.0).unwrap(), before, "probe: the stamp can't tell");
    assert_ne!(CorpusKey::of_file(&file.0).unwrap(), built.key, "probe: the key can");
    let said = refs.finish(&mut hits);
    assert_eq!(said["made"], 0);
    assert_eq!(said["missing"], json!({ "file_changed": 2 }));
    assert!(hits.as_array().unwrap().iter().all(|hit| hit["ref"].is_null()));
}

/// Failure: a record that doesn't open as its page's (a flipped byte, or another page's record in its place) gives another page's identity, or the
/// answer without a count; the identity files stay to fail the next search as well; or the hits after it still use what is stored.
#[test]
fn a_damaged_record_withdraws_that_hit_and_the_rest_and_takes_the_identities_away() {
    for damage in ["flip", "swap"] {
        let file = numbered(10);
        let built = build(&file, 10, true);
        let bin = find(&built.root, "ids.bin");
        let record = recto_index::corpus::RECORD_BYTES;
        let mut bytes = std::fs::read(&bin).unwrap();
        match damage {
            "flip" => bytes[5 * record + 20] ^= 1,
            _ => {
                let (left, right) = bytes.split_at_mut(6 * record);
                left[5 * record..].swap_with_slice(&mut right[..record]);
            }
        }
        std::fs::write(&bin, &bytes).unwrap();
        let (tokens, said, _) = refer(&file, &[2, 3, 5, 6, 8], Some(&built));
        assert_eq!(
            tokens.iter().map(Option::is_some).collect::<Vec<_>>(),
            [true, true, false, false, false],
            "{damage}"
        );
        assert_eq!(said["made"], 2, "{damage}");
        assert_eq!(said["missing"], json!({ "file_unreadable": 3 }), "{damage}");
        assert!(!bin.exists(), "{damage}: the identities are taken away");
        // The next search finds none, and refers from the file.
        let (again, said, opened) = refer(&file, &[5, 6], Some(&built));
        assert!(again.iter().all(Option::is_some), "{damage}: {said}");
        assert!(matches!(opened.get(), Some(Some(Held::Live(_)))), "{damage}");
    }
}

/// Failure: an `ids.bin` one record short or one long is used as far as it goes (a page past its end gets no identity or another page's) instead of being
/// refused, or it's refused without being taken away.
#[test]
fn an_identity_file_one_record_short_or_long_is_refused_and_taken_away() {
    for (name, change) in [("short", -1i64), ("long", 1)] {
        let file = numbered(6);
        let built = build(&file, 6, true);
        let bin = find(&built.root, "ids.bin");
        let record = recto_index::corpus::RECORD_BYTES as u64;
        let length = std::fs::metadata(&bin).unwrap().len();
        let open = std::fs::OpenOptions::new().write(true).open(&bin).unwrap();
        open.set_len((length as i64 + change * record as i64) as u64).unwrap();
        drop(open);
        let (tokens, said, _) = refer(&file, &[0, 5], Some(&built));
        assert!(tokens.iter().all(Option::is_none), "{name}");
        assert_eq!(said["missing"], json!({ "file_unreadable": 2 }), "{name}");
        assert!(!bin.exists(), "{name}");
    }
}
