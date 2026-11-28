//! `get`'s own tests, over a real session with a real file open.

use super::*;
use crate::locating::read_reference;
use recto_platform::slow::{self, Seam};
use recto_session::Session;
use std::sync::Mutex;

/// Failure: a stored page is given though the index never checked it for hidden text, or dropped some of its stretches, so its hidden words show as
/// plain text. Only a checked page with no dropped stretch is enough.
#[test]
fn only_a_checked_page_with_no_dropped_stretch_is_taken_from_the_index() {
    assert!(index_is_enough(true, false));
    assert!(!index_is_enough(false, false));
    assert!(!index_is_enough(true, true));
}

fn pdf(who: &str, pages: usize) -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (1..=pages)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

fn open(bytes: &[u8]) -> (recto_testpdf::TempDir, std::path::PathBuf, Mutex<Session<crate::DrawableLoader>>) {
    let dir = recto_testpdf::TempDir::new("recto-getting-");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, bytes).unwrap();
    let mut session = Session::new(crate::DrawableLoader);
    session.open(&path).unwrap();
    (dir, path, Mutex::new(session))
}

/// What a plain `get` of page `page` asks for: the page's own reference.
fn page_ask(access: &Mutex<Session<crate::DrawableLoader>>, page: u32) -> Locator {
    let made = crate::locating::reference(access, &TabRef::Active, page, None).expect("a reference");
    read_reference(made["ref"].as_str().expect("its token")).expect("a reference")
}

fn asked<'a>(reference: &'a Locator, grain: Grain) -> Asked<'a> {
    Asked {
        tab: &TabRef::Active,
        reference,
        grain,
        form: Form::Text,
        max_chars: DEFAULT_MAX_CHARS,
        from_here: false,
        entry: None,
    }
}

/// Failure: the file is replaced by another program after `get` has read the page and before it answers, and the answer is given as the old file's words
/// (or its index key isn't looked at again): an agent is told text of a version the file no longer has. `get` held at its end check ([`Seam::Get`]) while
/// the file is replaced says `file_changed` and gives nothing; the same call with the file left alone answers.
#[test]
fn a_file_replaced_after_get_has_read_it_is_refused_not_answered() {
    let (_dir, path, access) = open(&pdf("Mine", 3));
    let reference = page_ask(&access, 2);
    let alone = get(&access, &mut Vec::new(), &asked(&reference, Grain::Page)).expect("the file is as it was");
    assert!(alone["untrusted"]["text"].as_str().is_some_and(|text| text.contains("Mine page 2")), "{alone}");

    slow::gate::hold(&[Seam::Get]);
    let outcome = std::thread::scope(|scope| {
        let call = scope.spawn(|| get(&access, &mut Vec::new(), &asked(&reference, Grain::Page)));
        let began = std::time::Instant::now();
        while slow::gate::waiting(Seam::Get) == 0 {
            assert!(began.elapsed() < std::time::Duration::from_secs(30), "get never reached its end check");
            std::thread::yield_now();
        }
        // Another program's save, of another length, under the read.
        let incoming = path.with_extension("incoming");
        std::fs::write(&incoming, pdf("Theirs", 5)).unwrap();
        std::fs::rename(&incoming, &path).unwrap();
        slow::gate::open(Seam::Get);
        call.join().expect("get's thread")
    });
    let error = outcome.expect_err("an answer about a file that is gone is refused");
    assert_eq!((error.code, error.message.as_str()), (Code::FILE_CHANGED, CHANGED_WHILE_READ));
}

/// Failure: `headings_above` is `[]` for a hit or a page of a file that has a good outline, so an agent reading a page can't tell what it is under; or it names the page's headings by the wrong rule, or
/// lists them in the wrong order or with the wrong depth. On the S5 outlined file (a chapter on each of pages 1 and 2, a section starting on page 3): page 1 is under the first chapter, page 2 under the
/// second, page 3 under the second chapter and its section, outermost first, whatever the grain (a page, or a hit in it), and the titles are under `untrusted`.
#[test]
fn a_hit_or_a_page_names_the_headings_it_is_under_from_the_files_outline() {
    let (_dir, _path, access) = open(&recto_testpdf::blocks::outlined_blocks_document());
    let titles = |said: &Value| -> Vec<(u64, String, String)> {
        said["headings_above"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|heading| {
                (
                    heading["depth"].as_u64().unwrap_or(99),
                    heading["source"].as_str().unwrap_or_default().to_string(),
                    heading["untrusted"]["title"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    };
    let (chapter, equipment, inspection) = (
        (0, "file".to_string(), "Harbour Survey Overview".to_string()),
        (0, "file".to_string(), "Equipment Register".to_string()),
        (1, "file".to_string(), "Final Inspection".to_string()),
    );
    for (page, grain, expected) in [
        (1, Grain::Page, vec![chapter.clone()]),
        (2, Grain::Page, vec![equipment.clone()]),
        (3, Grain::Page, vec![equipment.clone(), inspection.clone()]),
    ] {
        let reference = page_ask(&access, page);
        let said = get(&access, &mut Vec::new(), &asked(&reference, grain)).expect("an answer");
        assert_eq!(titles(&said), expected, "page {page}: {said}");
    }
    // A hit in page 3: the same headings.
    let made = crate::locating::reference(&access, &TabRef::Active, 3, Some((0, 5))).expect("a reference to characters");
    let reference = read_reference(made["ref"].as_str().expect("its token")).expect("a reference");
    let hit = get(&access, &mut Vec::new(), &asked(&reference, Grain::Hit)).expect("a hit");
    assert_eq!(
        (hit["unit"]["kind"].as_str(), titles(&hit)),
        (Some("hit"), vec![equipment, inspection]),
        "{hit}"
    );
}

struct Indexed {
    session: Mutex<Session<crate::DrawableLoader>>,
    indexing: crate::index::ReadOnlyCorpus,
}

impl crate::SessionAccess for Indexed {
    type Loader = crate::DrawableLoader;
    fn with<R>(&self, step: impl FnOnce(&mut Session<crate::DrawableLoader>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn crate::index::Indexing> {
        Some(&self.indexing)
    }
}

/// Failure: a hit is given the headings of its page's end though it sits in a block above where the next section starts (so it is named under a heading its words come before), or the answer doesn't say
/// which place it read at; or a hit whose page has no stored blocks is told `block`. On page 3 of the outlined file, with a stored block at the very top of the page (above the "Final Inspection" entry): the hit
/// in it is under the chapter alone and says `block`; a hit with no stored block, and a page, are under both and say `page_end`.
#[test]
fn a_hit_in_a_stored_block_is_read_at_the_blocks_place_and_otherwise_at_the_foot_of_the_page() {
    use recto_index::blocks::{Block, Kind, PageBlocks};
    use recto_index::corpus::{BlocksBuild, BlocksTally, BuildLimits, CorpusKey, build_text_corpus};
    use recto_platform::find_key::FindKey;
    let bytes = recto_testpdf::blocks::outlined_blocks_document();
    let (dir, path, session) = open(&bytes);
    let texts: Vec<String> = (0..3).map(|page| crate::render::page_text(&path, page).expect("a page's text")).collect();
    let key = CorpusKey::of_file(&path).unwrap();
    let root = recto_testpdf::TempDir::new_in(&dir, "recto-getting-corpus-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let find_key = FindKey::for_tests(5);
    build_text_corpus(&find_key, &root, (key, Some(&path)), &texts.iter().cloned().map(Some).collect::<Vec<_>>()).unwrap();
    let mut build = match BlocksBuild::begin(&find_key, &root, key, &BuildLimits::default()) {
        recto_index::corpus::Begin::Building(build) => *build,
        other => panic!("a build, got {other:?}"),
    };
    let mut chars = Vec::new();
    for page in 0..3u32 {
        build.text(page, &mut chars);
        let length = chars.len() as u32;
        // Page 3 (index 2) has one block over its first 20 characters, at the very top of the page: above the section its outline starts there.
        let blocks = if page == 2 {
            vec![Block {
                kind: Kind::Paragraph,
                level: 0,
                page,
                start: 0,
                end: 20,
                bbox: [0, 791, 100, 792],
                continues: false,
            }]
        } else {
            Vec::new()
        };
        build.put(&PageBlocks { page, chars: length, blocks }).unwrap();
    }
    build.finish(&BlocksTally::default()).unwrap();
    let access = Indexed {
        session,
        indexing: crate::index::ReadOnlyCorpus(root.to_path_buf(), Some(FindKey::for_tests(5))),
    };
    let hit_at = |page: u32, chars: (u32, u32)| {
        let made = crate::locating::reference(&access, &TabRef::Active, page, Some(chars)).expect("a reference to characters");
        let reference = read_reference(made["ref"].as_str().expect("its token")).expect("a reference");
        get(&access, &mut Vec::new(), &asked(&reference, Grain::Hit)).expect("a hit")
    };
    let names = |said: &Value| -> Vec<String> {
        said["headings_above"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["untrusted"]["title"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let in_block = hit_at(3, (2, 8));
    assert_eq!(
        (in_block["headings_at"].as_str(), names(&in_block)),
        (Some("block"), vec!["Equipment Register".to_string()]),
        "{in_block}"
    );
    let outside = hit_at(3, (30, 36));
    assert_eq!(
        (outside["headings_at"].as_str(), names(&outside)),
        (Some("page_end"), vec!["Equipment Register".to_string(), "Final Inspection".to_string()]),
        "{outside}"
    );
    let reference = page_ask(&access.session, 3);
    let page = get(&access, &mut Vec::new(), &asked(&reference, Grain::Page)).expect("a page");
    assert_eq!((page["headings_at"].as_str(), names(&page).len()), (Some("page_end"), 2), "{page}");
}

#[derive(Clone)]
struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl recto_session::DocLoader for Counting {
    fn load(&self, path: &std::path::Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::DrawableLoader.load(path)
    }
}

/// Failure: every `get` of a hit or a page reads the whole file again for its outline, though the tab holds it: a cost on the commonest `get` that grows with the file. After the tab is open, asking
/// for the headings of a page and of a hit loads the file no more times.
#[test]
fn the_headings_of_a_hit_or_a_page_use_the_outline_the_tab_holds() {
    let dir = recto_testpdf::TempDir::new("recto-getting-held-");
    let path = dir.join("doc.pdf");
    std::fs::write(&path, recto_testpdf::blocks::outlined_blocks_document()).unwrap();
    let loads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut session = Session::new(Counting(loads.clone()));
    session.open(&path).unwrap();
    let access = Mutex::new(session);
    let opened = loads.load(std::sync::atomic::Ordering::SeqCst);
    let made = crate::locating::reference(&access, &TabRef::Active, 3, None).expect("a reference");
    let reference = read_reference(made["ref"].as_str().expect("its token")).expect("a reference");
    for grain in [Grain::Page, Grain::Hit] {
        let said = get(&access, &mut Vec::new(), &asked(&reference, grain)).expect("an answer");
        assert_eq!(said["headings_above"].as_array().map(Vec::len), Some(2), "{said}");
    }
    assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), opened, "no load for the outline");
}
