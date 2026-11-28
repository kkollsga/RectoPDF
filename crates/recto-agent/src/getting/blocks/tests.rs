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
