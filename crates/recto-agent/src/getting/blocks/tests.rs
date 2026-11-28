//! The glue's own tests: `Source`, the pages of a real file, a real corpus and real stored blocks, driving the builder.

use super::*;
use crate::TabRef;
use recto_core::{DocFacts, OpenError, Toc, TocLine};
use recto_index::corpus::{BlocksBuild, BlocksTally, BuildLimits, build_text_corpus};
use recto_platform::find_key::FindKey;
use recto_session::{DocLoader, Session};
use std::path::Path;
use std::sync::Mutex;

const PAGES: u32 = 70;
/// The page, from 0, that has no characters at all: the 65th.
const BLANK: u32 = 64;

#[derive(Clone)]
struct Pdf;

impl DocLoader for Pdf {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: PAGES,
            byte_len: 8,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

struct World(Mutex<Session<Pdf>>);

impl SessionAccess for World {
    type Loader = Pdf;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Pdf>) -> R) -> R {
        self.0.with(step)
    }
}

/// Failure (review round 2): the builder's blank-page rule isn't connected to the corpus, so a blank page 65 still ends the answer as `truncated` with no `more`. A 70-page file whose 65th page has no
/// characters in its real index, with real stored blocks, read through the real `Source`: the chapter's answer is cut at the page cap and `more` is at page 66. A page that failed to index isn't passed
/// over: it is not `has_text == false`.
#[test]
fn a_blank_page_after_the_cap_is_passed_over_by_the_real_source() {
    let pages: Vec<recto_testpdf::Page> = (0..PAGES)
        .map(|page| {
            recto_testpdf::Page::new(if page == BLANK {
                Vec::new()
            } else {
                recto_testpdf::text_lines(&[&format!("Words of page {page}")])
            })
        })
        .collect();
    let file = recto_testpdf::TempPdf::new("long.pdf", &recto_testpdf::pages(&pages));
    let key = CorpusKey::of_file(&file.0).unwrap();
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-agent-source-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let find_key = FindKey::for_tests(5);
    let texts: Vec<Option<String>> = (0..PAGES).map(|page| (page != BLANK).then(|| format!("Words of page {page}"))).collect();
    build_text_corpus(&find_key, &root, (key, Some(&file.0)), &texts).unwrap();
    let mut build = match BlocksBuild::begin(&find_key, &root, key, &BuildLimits::default()) {
        recto_index::corpus::Begin::Building(build) => *build,
        other => panic!("a build, got {other:?}"),
    };
    let mut chars = Vec::new();
    for page in 0..PAGES {
        build.text(page, &mut chars);
        let blocks = if chars.is_empty() {
            Vec::new()
        } else {
            vec![Block {
                kind: recto_index::blocks::Kind::Paragraph,
                level: 0,
                page,
                start: 0,
                end: chars.len() as u32,
                bbox: [0, 100, 100, 200],
                continues: false,
            }]
        };
        build
            .put(&recto_index::blocks::PageBlocks {
                page,
                chars: chars.len() as u32,
                blocks,
            })
            .unwrap();
    }
    build.finish(&BlocksTally::default()).unwrap();

    let world = World(Mutex::new(Session::new(Pdf)));
    world.with(|session| session.open(&file.0).unwrap());
    let tab = crate::reading::tab_file(&world, &TabRef::Active).unwrap();
    let stamp = recto_platform::file_stamp(&file.0).unwrap();
    let opened = Opened::open(&file.0, stamp).expect("the file opens");
    let corpus = ReadyCorpus::open(&find_key, &root, key).unwrap().unwrap();
    let stored = StoredBlocks::open_matching(&find_key, &root, key, &corpus.summary).unwrap().unwrap();
    let mut events = Vec::new();
    let mut source = Source {
        access: &world,
        events: &mut events,
        id: TabId(tab.id()),
        file: &tab,
        corpus,
        stored,
        opened: &opened,
        live_spent: Duration::ZERO,
        anchor: None,
        key,
        checked: true,
        partial: false,
        live: false,
        buffer: Vec::new(),
    };
    assert!(!source.has_text(BLANK), "the index recorded no characters on it");
    assert!(source.has_text(BLANK - 1) && source.has_text(BLANK + 1));

    let entries = Toc::from_lines(
        &[TocLine {
            title: "t",
            page: 0,
            parent: recto_core::NONE,
            depth: 0,
        }],
        false,
    );
    let sections = Sections::new(entries.entries());
    let ask = Ask {
        grain: Grain::Chapter,
        page: 0,
        start: None,
        from_here: false,
        entry: None,
        max_chars: 65_536,
        sections: Some(&sections),
    };
    let Ok(Outcome::Unit(answer)) = unit::build(&mut source, &ask) else {
        panic!("a unit")
    };
    assert!(answer.truncated);
    assert_eq!(answer.refs.more.map(|more| more.page), Some(BLANK + 1), "{:?}", answer.refs);
    assert!(
        answer.given.iter().all(|piece| piece.page < BLANK),
        "nothing of the pages past the cap is given"
    );
}

/// Three pages, a chapter on the first whose nested sections are on the second and the third (both depth 1): the contents a section is cut by.
#[derive(Clone)]
struct Nested;

impl DocLoader for Nested {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        let line = |page, depth| TocLine {
            title: "t",
            page,
            parent: recto_core::NONE,
            depth,
        };
        Ok(DocFacts {
            page_count: 3,
            byte_len: 8,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::from_lines(&[line(0, 0), line(1, 1), line(2, 1)], false),
            renderer_only: false,
        })
    }
}

struct Nobody;

impl crate::blocks::Blocking for Nobody {
    fn want(&self, _ask: crate::blocks::BlocksAsk) {}
    fn state(&self, _key: CorpusKey) -> crate::generated::GenState {
        crate::generated::GenState::Idle
    }
    fn cancel_files(&self, _paths: Option<&[std::path::PathBuf]>) {}
    fn making(&self, _paths: Option<&[std::path::PathBuf]>) -> bool {
        false
    }
}

struct Nested3 {
    session: Mutex<Session<Nested>>,
    indexing: crate::index::ReadOnlyCorpus,
}

impl SessionAccess for Nested3 {
    type Loader = Nested;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Nested>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn crate::index::Indexing> {
        Some(&self.indexing)
    }
    fn blocking(&self) -> Option<&dyn crate::blocks::Blocking> {
        Some(&Nobody)
    }
}

/// Failure (round 3): a section's `more` doesn't say which entry it was of, or `--entry` never reaches the builder, so going on from inside a nested section gives that nested section and the rest of the
/// one the `more` came from is lost. Through the glue's `answer` over a real file, index and blocks: a cut section answer carries `refs.more_entry`, `--from-here --entry N` on a page inside a nested
/// entry continues entry N, and without the entry it is the nested one.
#[test]
fn a_cut_section_names_its_entry_and_going_on_with_it_continues_that_section() {
    let texts = ["Chapter heading and its words", "Nested one words here", "Nested two words here"];
    let pages: Vec<recto_testpdf::Page> = texts.iter().map(|text| recto_testpdf::Page::new(recto_testpdf::text_lines(&[text]))).collect();
    let file = recto_testpdf::TempPdf::new("nested.pdf", &recto_testpdf::pages(&pages));
    let key = CorpusKey::of_file(&file.0).unwrap();
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-agent-entry-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let find_key = FindKey::for_tests(5);
    build_text_corpus(&find_key, &root, (key, Some(&file.0)), &texts.map(|text| Some(text.to_string()))).unwrap();
    let mut build = match BlocksBuild::begin(&find_key, &root, key, &BuildLimits::default()) {
        recto_index::corpus::Begin::Building(build) => *build,
        other => panic!("a build, got {other:?}"),
    };
    let mut chars = Vec::new();
    for page in 0..3u32 {
        build.text(page, &mut chars);
        let block = Block {
            kind: recto_index::blocks::Kind::Paragraph,
            level: 0,
            page,
            start: 0,
            end: chars.len() as u32,
            bbox: [0, 100, 100, 200],
            continues: false,
        };
        build
            .put(&recto_index::blocks::PageBlocks {
                page,
                chars: chars.len() as u32,
                blocks: vec![block],
            })
            .unwrap();
    }
    build.finish(&BlocksTally::default()).unwrap();
    let world = Nested3 {
        session: Mutex::new(Session::new(Nested)),
        indexing: crate::index::ReadOnlyCorpus(root.to_path_buf(), Some(FindKey::for_tests(5))),
    };
    world.with(|session| session.open(&file.0).unwrap());
    let tab = crate::reading::tab_file(&world, &TabRef::Active).unwrap();
    let stamp = recto_platform::file_stamp(&file.0).unwrap();
    let get = |page: u32, chars: Option<(u32, u32)>, from_here: bool, entry: Option<u32>, max_chars: u32| {
        let range = chars.map(|(start, end)| locator::characters(texts[page as usize], start, end).unwrap());
        let found = crate::locating::Located::for_a_test(TabId(tab.id()), page, stamp, range);
        let asked = super::super::Asked {
            tab: &TabRef::Active,
            reference: &locator::decode("r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000").unwrap(),
            grain: AskedGrain::Section,
            form: Form::Text,
            max_chars,
            from_here,
            entry,
        };
        match answer(&world, &mut Vec::new(), &asked, &found, (TabId(tab.id()), &tab)).expect("an answer") {
            Reached::Answer(data) => *data,
            Reached::Widen { why, .. } => panic!("widened: {why}"),
        }
    };
    // The chapter, cut inside its own first page: it names entry 0 for going on.
    let first = get(0, None, false, None, 10);
    assert_eq!(
        (first["refs"]["more_entry"].as_u64(), first["truncated"].as_bool()),
        (Some(0), Some(true)),
        "{first}"
    );
    // Going on from the nested section's own page: with the entry it is the chapter's rest, through the second nested one; without it, the nested section alone.
    let with = get(1, Some((0, 6)), true, Some(0), 10_000);
    let without = get(1, Some((0, 6)), true, None, 10_000);
    let text = |said: &Value| said["untrusted"]["text"].as_str().unwrap_or_default().to_string();
    assert!(text(&with).contains("Nested two"), "the chapter goes on through the second nested one: {with}");
    assert!(!text(&without).contains("Nested two"), "the nested section alone: {without}");
    assert_eq!(
        (with["unit"]["pages"].as_u64(), without["unit"]["pages"].as_u64()),
        (Some(3), Some(1)),
        "{with} {without}"
    );
}
