//! `get` leaves running heads and page numbers out, over a real report (a head, two lines of body and the page number at the foot of each page), a real index with its edges, and real stored blocks. Each
//! test names the failure it catches.

use super::super::*;
use crate::TabRef;
use crate::locating::read_reference;
use recto_core::{DocFacts, OpenError, Toc, TocLine};
use recto_index::blocks::Block;
use recto_index::corpus::{BlocksBuild, BlocksTally, BuildLimits, DocMeta, FailReason, FedPage, FeedError, PageFeed};
use recto_platform::find_key::FindKey;
use recto_session::{DocLoader, Session};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const PAGES: u32 = 6;
const HEAD: &str = "Annual report 2024";

/// The contents: a chapter on page 1 and a section on page 2 that runs to the next entry, on page 5.
#[derive(Clone)]
struct Outline;

impl DocLoader for Outline {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        let line = |page, depth| TocLine {
            title: "t",
            page,
            parent: recto_core::NONE,
            depth,
        };
        Ok(DocFacts {
            page_count: PAGES,
            byte_len: 8,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::from_lines(&[line(0, 0), line(1, 1), line(4, 1)], false),
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
    fn cancel_files(&self, _paths: Option<&[PathBuf]>) {}
    fn making(&self, _paths: Option<&[PathBuf]>) -> bool {
        false
    }
}

struct World {
    session: Mutex<Session<Outline>>,
    indexing: crate::index::ReadOnlyCorpus,
}

impl SessionAccess for World {
    type Loader = Outline;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Outline>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn crate::index::Indexing> {
        Some(&self.indexing)
    }
    fn blocking(&self) -> Option<&dyn crate::blocks::Blocking> {
        Some(&Nobody)
    }
}

/// What the corpus is made with.
#[derive(Clone, Copy)]
struct Made {
    /// Pages fed with their frame, so their edges are kept. Without, the index is one made before edges were.
    edges: bool,
    /// Pages 2 and 3 (1 and 2 from 0) are one paragraph that goes on.
    joined: bool,
}

/// The pages' characters from the file by the same renderer `text` uses, with their frame when `edges`.
struct Feed {
    doc: recto_render_service::DocRenderer,
    edges: bool,
}

impl PageFeed for Feed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(PAGES)
    }
    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        let read = self.doc.page_characters(page, true, true).map_err(|_| FeedError::Page(FailReason::Stopped))?;
        Ok(FedPage {
            chars: read.chars,
            boxes: read.boxes,
            hidden_checked: read.hidden_checked,
            frame: self.edges.then_some(read.frame),
        })
    }
    fn still_the_same_file(&mut self) -> bool {
        true
    }
}

fn report() -> Vec<u8> {
    let list: Vec<recto_testpdf::Page> = (1..=PAGES)
        .map(|page| {
            recto_testpdf::Page::new(
                format!(
                    "BT /F1 9 Tf 72 750 Td ({HEAD}) Tj ET\nBT /F1 11 Tf 14 TL 72 690 Td (Body of page {page}, first line.) Tj T* (Body of page {page}, second line.) Tj ET\nBT /F1 9 Tf 540 30 Td ({page}) Tj ET\n"
                )
                .into_bytes(),
            )
        })
        .collect();
    recto_testpdf::pages(&list)
}

fn body(page: u32) -> String {
    format!("Body of page {page}, first line.\nBody of page {page}, second line.")
}

struct Report {
    _folder: recto_testpdf::TempDir,
    file: recto_testpdf::TempPdf,
    world: World,
}

fn built(made: Made) -> Report {
    let file = recto_testpdf::TempPdf::new("report.pdf", &report());
    let folder = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-heads-");
    recto_platform::local::ensure_private_dir(&folder).unwrap();
    let root = folder.join("corpus.noindex");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let find_key = FindKey::for_tests(5);
    let doc = crate::render::open_for_text(&file.0).expect("the renderer");
    recto_index::corpus::build(
        &find_key,
        &root,
        key,
        &mut Feed { doc, edges: made.edges },
        &DocMeta::default(),
        &BuildLimits::default(),
    )
    .unwrap();
    let mut build = match BlocksBuild::begin(&find_key, &root, key, &BuildLimits::default()) {
        recto_index::corpus::Begin::Building(build) => *build,
        other => panic!("a build, got {other:?}"),
    };
    let mut chars = Vec::new();
    for page in 0..PAGES {
        build.text(page, &mut chars);
        let text: String = chars.iter().collect();
        let start = text.find("Body of page").expect("the body is on the page") as u32;
        let end = start + body(page + 1).chars().count() as u32;
        let goes_on = made.joined && (page == 1 || page == 2);
        let block = Block {
            kind: recto_index::blocks::Kind::Paragraph,
            level: 0,
            page,
            start,
            end,
            bbox: [0, 100, 100, 200],
            continues: goes_on && page == 1,
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
    let world = World {
        session: Mutex::new(Session::new(Outline)),
        indexing: crate::index::ReadOnlyCorpus(root, Some(find_key)),
    };
    world.with(|session| session.open(&file.0).unwrap());
    Report { _folder: folder, file, world }
}

impl Report {
    /// `get` of a reference given as its token, from where an answer said it goes on.
    fn get_token(&self, token: &str, grain: Grain, max_chars: u32, entry: Option<u32>) -> Value {
        let reference = read_reference(token).expect("a reference");
        let asked = Asked {
            tab: &TabRef::Active,
            reference: &reference,
            grain,
            form: Form::Text,
            max_chars,
            from_here: true,
            entry,
        };
        get(&self.world, &mut Vec::new(), &asked).expect("an answer")
    }

    /// Every piece of `first`'s unit and of what it goes on to, to the end: the texts, in order.
    fn all_of(&self, first: Value, grain: Grain, max_chars: u32) -> Vec<String> {
        let (mut texts, mut said) = (Vec::new(), first);
        for _ in 0..60 {
            texts.push(given(&said));
            let Some(more) = said["refs"]["more"].as_str().map(str::to_owned) else {
                return texts;
            };
            let entry = said["refs"]["more_entry"].as_u64().map(|entry| entry as u32);
            said = self.get_token(&more, grain, max_chars, entry);
        }
        panic!("it never ended: {texts:?}");
    }

    /// `get` of page `page` (from 1), optionally of characters of its text, at a grain.
    fn get(&self, page: u32, chars: Option<(u32, u32)>, grain: Grain, max_chars: u32, from_here: bool) -> Value {
        let made = crate::locating::reference(&self.world, &TabRef::Active, page, chars).expect("a reference");
        let reference = read_reference(made["ref"].as_str().expect("its token")).expect("a reference");
        let asked = Asked {
            tab: &TabRef::Active,
            reference: &reference,
            grain,
            form: Form::Text,
            max_chars,
            from_here,
            entry: None,
        };
        get(&self.world, &mut Vec::new(), &asked).expect("an answer")
    }

    /// The text of page `page`, as `recto text` reads it.
    fn page_text(&self, page: u32) -> String {
        crate::render::page_text(&self.file.0, page - 1).unwrap()
    }

    /// Where `needle` is in page `page`'s text, in characters.
    fn at(&self, page: u32, needle: &str) -> (u32, u32) {
        let text = self.page_text(page);
        let start = text.find(needle).expect("it is on the page") as u32;
        (start, start + needle.chars().count() as u32)
    }
}

fn given(said: &Value) -> String {
    said["untrusted"]["text"].as_str().unwrap_or_default().to_string()
}

fn count(said: &Value, name: &str) -> u64 {
    said["counts"][name].as_u64().unwrap_or_else(|| panic!("{name} in {said}"))
}

/// What `text` marks as chrome on page `page`, summed: the characters a page leaves out, by the rule of record.
fn marked(report: &Report, page: u32) -> u64 {
    let said = crate::render::text(&report.file.0, page).unwrap();
    said["chrome_spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|span| span[1].as_u64().unwrap() - span[0].as_u64().unwrap())
        .sum()
}

fn without_space(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn has_lone_number(text: &str) -> bool {
    text.lines().any(|line| line.trim().parse::<u32>().is_ok())
}

/// Failure: running heads and page numbers are in an agent's quoted body text, or are left out and not counted. A page and a section over a real report: no "Annual report 2024" and no line that is a page
/// number, `chrome_chars_left_out` of the page is exactly the characters `recto text 3` marks, `recto text` still has both, and the section says it left some out.
#[test]
fn get_leaves_out_running_heads_and_counts_them() {
    let report = built(Made { edges: true, joined: false });
    let page = report.get(3, None, Grain::Page, 10_000, false);
    let text = given(&page);
    assert!(text.contains(&body(3)) && !text.contains("Annual") && !has_lone_number(&text), "{text:?}");
    assert_eq!(
        count(&page, "chrome_chars_left_out"),
        marked(&report, 3),
        "the same characters `recto text` marks: {page}"
    );
    assert_eq!((count(&page, "chrome_pages"), count(&page, "chrome_unknown_pages")), (1, 0));
    let section = report.get(3, None, Grain::Section, 10_000, false);
    let text = given(&section);
    assert!(text.contains(&body(2)) && text.contains(&body(3)) && text.contains(&body(4)), "{text:?}");
    assert!(!text.contains("Annual") && !has_lone_number(&text), "{text:?}");
    assert!(count(&section, "chrome_chars_left_out") >= marked(&report, 3), "{section}");
    let read = report.page_text(3);
    assert!(read.contains(HEAD) && has_lone_number(&read), "recto text still has them: {read:?}");
}

/// Failure: an agent asking for exactly a running head gets nothing. A hit on "Annual report 2024" gives it, and so does the page it is asked of (which still leaves the page number out).
#[test]
fn the_characters_a_reference_names_are_never_left_out() {
    let report = built(Made { edges: true, joined: false });
    let at = report.at(3, HEAD);
    let hit = report.get(3, Some(at), Grain::Hit, 10_000, false);
    assert_eq!(given(&hit), HEAD, "{hit}");
    let page = report.get(3, Some(at), Grain::Page, 10_000, false);
    let text = given(&page);
    assert!(text.contains(HEAD) && text.contains(&body(3)) && !has_lone_number(&text), "{text:?}");
    let paragraph = report.get(3, Some(at), Grain::Paragraph, 10_000, false);
    assert!(given(&paragraph).contains(HEAD), "{paragraph}");
}

/// Failure: offsets are taken from the given text instead of the page's, so `more` repeats or skips text, or names a running head. A section over pages 2 to 4, and a paragraph joined over pages 2 and 3, each
/// with a small cap, followed by `more` to the end: the pieces are the pages' body characters exactly (all that isn't white space, in order), and no head or number is in them.
#[test]
fn going_on_after_a_cut_gives_every_body_character_once_with_heads_left_out() {
    let report = built(Made { edges: true, joined: false });
    let first = report.get(3, None, Grain::Section, 40, false);
    assert!(first["truncated"] == true, "{first}");
    let texts = report.all_of(first, Grain::Section, 40);
    let all = texts.concat();
    assert_eq!(without_space(&all), without_space(&format!("{}{}{}", body(2), body(3), body(4))), "{texts:?}");
    let joined = built(Made { edges: true, joined: true });
    let first = joined.get(2, None, Grain::Paragraph, 40, false);
    assert!(first["truncated"] == true, "{first}");
    let texts = joined.all_of(first, Grain::Paragraph, 40);
    assert_eq!(without_space(&texts.concat()), without_space(&format!("{}{}", body(2), body(3))), "{texts:?}");
}

/// Failure: an old index (made before edges were kept) is silently taken as having no chrome, or its page is dropped. Pages with no stored edges: the heads stay and every page given is counted as unknown.
#[test]
fn an_index_built_before_edges_keeps_heads_and_says_so() {
    let report = built(Made { edges: false, joined: false });
    let page = report.get(3, None, Grain::Page, 10_000, false);
    let text = given(&page);
    assert!(text.contains(HEAD) && has_lone_number(&text), "{text:?}");
    assert_eq!((count(&page, "chrome_chars_left_out"), count(&page, "chrome_unknown_pages")), (0, 1));
    let section = report.get(3, None, Grain::Section, 10_000, false);
    assert!(given(&section).contains(HEAD), "{section}");
    assert_eq!(count(&section, "chrome_chars_left_out"), 0);
    // The same pages are given as when the edges are known, where each of them has something left out.
    let known = built(Made { edges: true, joined: false }).get(3, None, Grain::Section, 10_000, false);
    assert_eq!(count(&section, "chrome_unknown_pages"), count(&known, "chrome_pages"), "{section}");
}

/// Failure: pages read live keep the heads that stored pages leave out. A page with no index gives the same text as the same page from the index, and counts as read from the file.
#[test]
fn a_page_read_live_leaves_out_what_text_marks() {
    let indexed = built(Made { edges: true, joined: false });
    let live = built(Made { edges: true, joined: false });
    let empty = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-heads-none-");
    recto_platform::local::ensure_private_dir(&empty).unwrap();
    let world = World {
        session: Mutex::new(Session::new(Outline)),
        indexing: crate::index::ReadOnlyCorpus(empty.to_path_buf(), Some(FindKey::for_tests(5))),
    };
    world.with(|session| session.open(&live.file.0).unwrap());
    let live = Report { world, ..live };
    let (from_index, from_file) = (indexed.get(3, None, Grain::Page, 10_000, false), live.get(3, None, Grain::Page, 10_000, false));
    assert_eq!(
        (from_index["read_from"].as_str(), from_file["read_from"].as_str()),
        (Some("index"), Some("file"))
    );
    assert_eq!(given(&from_index), given(&from_file), "{from_file}");
    assert!(!given(&from_file).contains("Annual"), "{from_file}");
    assert_eq!(count(&from_index, "chrome_chars_left_out"), count(&from_file, "chrome_chars_left_out"));
}

/// Failure: the text between a joined paragraph's pieces counts its running head and page number twice (once as left out, once as outside blocks) or not as left out. A paragraph joined over pages 2 and 3, the
/// page number after the first piece and the head before the second between them: the two line ends are the only characters outside blocks, the number and the head are the characters left out, and both pages
/// are pages with something left out.
#[test]
fn between_the_pieces_of_a_paragraph_joined_over_pages_the_head_and_number_are_left_out_not_outside() {
    let report = built(Made { edges: true, joined: true });
    let said = report.get(2, None, Grain::Paragraph, 10_000, false);
    assert_eq!(given(&said), format!("{}\n{}", body(2), body(3)), "{said}");
    assert_eq!(count(&said, "chars_outside_blocks"), 2, "the two line ends: {said}");
    assert_eq!(count(&said, "chrome_chars_left_out"), 1 + HEAD.chars().count() as u64, "{said}");
    assert_eq!((count(&said, "chrome_pages"), count(&said, "chrome_unknown_pages")), (2, 0));
}
