//! The stored blocks: what a build writes beside the text and what a reader trusts of it. Each test names the failure it catches.

use super::*;
use crate::blocks::{Block, Kind};
use crate::corpus::{BuildOutcome, DocMeta, FedPage, FeedError, PageFeed, build, clear, evict};
use recto_proto::CharRecord;

fn tk() -> &'static FindKey {
    static KEY: std::sync::OnceLock<FindKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| FindKey::for_tests(9))
}

const KEY: CorpusKey = CorpusKey([77; 32]);

/// A folder of its own, private and removed at the end.
struct Temp(recto_testpdf::TempDir);

impl Temp {
    fn new() -> Self {
        let path = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-blocks-");
        local::ensure_private_dir(&path).unwrap();
        Self(path)
    }
}

/// Pages of a made-up document: every page has text, except those named.
struct Feed {
    pages: u32,
    no_text: Vec<u32>,
}

fn text_of(page: u32) -> String {
    format!("Page {page} holds some words of its own")
}

impl PageFeed for Feed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(self.pages)
    }

    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        let text = if self.no_text.contains(&page) { String::new() } else { text_of(page) };
        Ok(FedPage {
            chars: text.chars().map(|ch| CharRecord::new(ch as u32, 0)).collect(),
            boxes: Vec::new(),
            hidden_checked: true,
        })
    }

    fn still_the_same_file(&mut self) -> bool {
        true
    }
}

/// A finished corpus of `pages` pages under `KEY`.
fn corpus(root: &Path, pages: u32) {
    let outcome = build(
        tk(),
        root,
        KEY,
        &mut Feed { pages, no_text: Vec::new() },
        &DocMeta::default(),
        &BuildLimits::default(),
    )
    .unwrap();
    assert!(matches!(outcome, BuildOutcome::Ready { .. }), "{outcome:?}");
}

fn paths(root: &Path) -> CorpusPaths {
    CorpusPaths::new(root, KEY)
}

/// What a mapping of page `page` might say: a heading and a paragraph, and up to three more list items after them.
fn blocks_of(page: u32, chars: u32, extra: u32) -> PageBlocks {
    let block = |kind, start, end| Block {
        kind,
        level: 0,
        page,
        start,
        end,
        bbox: [1, 2, 30, 40],
        continues: false,
    };
    if chars < 30 {
        // A page with no text maps nothing.
        return PageBlocks {
            page,
            chars,
            blocks: Vec::new(),
        };
    }
    let mut blocks = vec![block(Kind::Heading, 0, 4), block(Kind::Paragraph, 5, 20)];
    for item in 0..extra.min(3) {
        blocks.push(block(Kind::ListItem, 21 + 3 * item, 22 + 3 * item));
    }
    PageBlocks { page, chars, blocks }
}

fn tally(pages: u32) -> BlocksTally {
    let mut tally = BlocksTally::default();
    tally.stored[Shape::Heading.index()] = pages;
    tally.stored[Shape::Paragraph.index()] = pages;
    tally.dropped[Shape::Table.index()] = 1;
    tally
}

fn building(root: &Path) -> BlocksBuild {
    match BlocksBuild::begin(tk(), root, KEY, &BuildLimits::default()) {
        Begin::Building(build) => *build,
        other => panic!("expected a build, got {other:?}"),
    }
}

/// Puts every page in `pages` with its real length, in order.
fn put_pages(build: &mut BlocksBuild, pages: impl IntoIterator<Item = u32>) {
    let mut chars = Vec::new();
    for page in pages {
        build.text(page, &mut chars);
        build.put(&blocks_of(page, chars.len() as u32, 0)).unwrap();
    }
}

/// Blocks made again from nothing, whatever was there.
fn made(root: &Path, pages: u32) -> BlocksMade {
    remove_blocks(root, KEY);
    let mut build = building(root);
    put_pages(&mut build, 0..pages);
    build.finish(&tally(pages)).unwrap()
}

fn stored(root: &Path) -> StoredBlocks {
    StoredBlocks::open(tk(), root, KEY).expect("holds up").expect("there are blocks")
}

/// Rewrites the header's plain bytes as `change` leaves them, sealed as a build seals them.
fn rewrite_header(root: &Path, change: impl FnOnce(&mut Vec<u8>)) {
    let paths = paths(root);
    let seal = read_seal(tk(), &paths).unwrap();
    let mut chunk = fs::read(header_path(&paths)).unwrap();
    let range = seal.open_in_place(Purpose::BlocksHeader, &mut chunk).unwrap();
    let mut plain = chunk[range].to_vec();
    change(&mut plain);
    write_sealed(&seal, &header_path(&paths), Purpose::BlocksHeader, &plain, true).unwrap();
}

type Damage = Box<dyn Fn(&mut Vec<u8>)>;

fn files(root: &Path) -> (bool, bool) {
    let paths = paths(root);
    (header_path(&paths).exists(), bin_path(&paths).exists())
}

/// Failure: a page's blocks come back other than they went in, or from another page's place (the table's offsets and lengths wrong by one page), or the header's counts aren't the build's.
#[test]
fn stored_blocks_read_back_page_by_page_as_put() {
    let temp = Temp::new();
    corpus(&temp.0, 5);
    let mut build = building(&temp.0);
    let mut chars = Vec::new();
    let mut put = Vec::new();
    for page in 0..5 {
        assert!(build.text(page, &mut chars));
        // Pages of different lengths in the file, so a record read at the wrong place has the wrong length.
        let blocks = blocks_of(page, chars.len() as u32, page % 3);
        build.put(&blocks).unwrap();
        put.push((blocks, chars.len() as u32));
    }
    let made = build.finish(&tally(5)).unwrap();
    let stored = stored(&temp.0);
    assert_eq!(*stored.made(), made);
    assert_eq!((made.pages, made.stored[Shape::Heading.index()], made.dropped[Shape::Table.index()]), (5, 5, 1));
    assert_eq!(
        made.bytes,
        fs::metadata(bin_path(&paths(&temp.0))).unwrap().len() + fs::metadata(header_path(&paths(&temp.0))).unwrap().len()
    );
    for (page, (blocks, chars)) in put.iter().enumerate() {
        assert_eq!(&stored.page(page as u32, *chars).unwrap(), blocks, "page {page}");
    }
    assert_eq!(stored.page(5, 0), Err(OpenError::Damaged), "a page past the document");
    assert_eq!(stored.page(0, 3), Err(OpenError::Stale), "a page whose text is another length now");
}

/// Failure: a build stopped part way is believed (a header written early, or a bin with no header taken as made), or it leaves files behind, or the next build
/// carries something of the one that was stopped.
#[test]
fn a_build_stopped_part_way_leaves_nothing_believed() {
    let temp = Temp::new();
    corpus(&temp.0, 5);
    {
        let mut build = building(&temp.0);
        put_pages(&mut build, 0..2);
        assert!(!files(&temp.0).0, "the header is written last");
    }
    assert_eq!(files(&temp.0), (false, false), "a dropped build leaves no file");
    // A real kill leaves the bin and no header.
    {
        let mut build = building(&temp.0);
        put_pages(&mut build, 0..2);
        let bin = fs::read(bin_path(&paths(&temp.0))).unwrap();
        drop(build);
        fs::write(bin_path(&paths(&temp.0)), bin).unwrap();
    }
    assert_eq!(files(&temp.0), (false, true));
    assert!(StoredBlocks::open(tk(), &temp.0, KEY).unwrap().is_none(), "a bin with no header is not made");
    let again = made(&temp.0, 5);
    let other = Temp::new();
    corpus(&other.0, 5);
    assert_eq!(made(&other.0, 5), again);
    let (first, second) = (stored(&temp.0), stored(&other.0));
    for page in 0..5 {
        let chars = text_of(page).chars().count() as u32;
        assert_eq!(first.page(page, chars).unwrap(), second.page(page, chars).unwrap(), "page {page}");
    }
}

/// Failure: a build that is told it's done with a page missing writes a header, and a document with fewer pages of blocks than text is answered as made.
#[test]
fn the_header_is_written_last_and_only_whole() {
    let temp = Temp::new();
    corpus(&temp.0, 4);
    let mut build = building(&temp.0);
    put_pages(&mut build, 0..3);
    let error = build.finish(&tally(3)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(files(&temp.0), (false, false), "nothing is left of a build that wasn't whole");
    let mut build = building(&temp.0);
    let mut chars = Vec::new();
    assert!(build.text(0, &mut chars));
    assert_eq!(build.put(&blocks_of(1, 10, 0)), Err(PutError::Write), "a page that isn't the next");
    assert_eq!(build.put(&blocks_of(0, chars.len() as u32, 0)), Ok(()));
    assert_eq!(build.put(&blocks_of(0, chars.len() as u32, 0)), Err(PutError::Write), "a page twice");
}

/// Failure: the blocks don't count against a document's limit, so a hostile file's text and blocks together take more than it may, or the limit is off by one on either side.
#[test]
fn blocks_count_against_max_doc_bytes_at_the_limit_and_one_over() {
    let temp = Temp::new();
    corpus(&temp.0, 3);
    let whole = made(&temp.0, 3);
    let corpus_bytes = building_summary(&temp.0);
    let at = BuildLimits {
        max_doc_bytes: corpus_bytes + whole.bytes,
        ..BuildLimits::default()
    };
    remove_blocks(&temp.0, KEY);
    let Begin::Building(mut build) = BlocksBuild::begin(tk(), &temp.0, KEY, &at) else {
        panic!()
    };
    put_pages(&mut build, 0..3);
    assert_eq!(build.finish(&tally(3)).unwrap().bytes, whole.bytes, "exactly at the limit is let through");
    remove_blocks(&temp.0, KEY);
    let over = BuildLimits {
        max_doc_bytes: at.max_doc_bytes - 1,
        ..at
    };
    let Begin::Building(mut build) = BlocksBuild::begin(tk(), &temp.0, KEY, &over) else {
        panic!()
    };
    let mut chars = Vec::new();
    for page in 0..2 {
        assert!(build.text(page, &mut chars));
        build.put(&blocks_of(page, chars.len() as u32, 0)).unwrap();
    }
    assert!(build.text(2, &mut chars));
    assert_eq!(build.put(&blocks_of(2, chars.len() as u32, 0)), Err(PutError::TooLarge));
    drop(build);
    assert_eq!(files(&temp.0), (false, false), "nothing is left of what was too large");
    assert!(StoredBlocks::open(tk(), &temp.0, KEY).unwrap().is_none());
}

fn building_summary(root: &Path) -> u64 {
    ReadyCorpus::open(tk(), root, KEY).unwrap().unwrap().summary.corpus_bytes
}

/// Failure: blocks made under other rules are taken for damage and thrown away, or used; or damage of any kind is believed: a changed byte, a table with a gap, a bin cut short or
/// longer, a record that is another page's, counts that can't be, a page of tables with something stored.
#[test]
fn another_rules_tag_is_not_damage_and_damage_is_refused() {
    let temp = Temp::new();
    corpus(&temp.0, 4);
    let whole = made(&temp.0, 4);
    let bin_before = fs::read(bin_path(&paths(&temp.0))).unwrap();
    // Another tag: nothing is used, nothing is thrown away.
    rewrite_header(&temp.0, |plain| plain[0] ^= 1);
    assert!(StoredBlocks::open(tk(), &temp.0, KEY).unwrap().is_none());
    assert_eq!(fs::read(bin_path(&paths(&temp.0))).unwrap(), bin_before);
    assert!(files(&temp.0).0, "the header stays");
    let begin = BlocksBuild::begin(tk(), &temp.0, KEY, &BuildLimits::default());
    assert!(matches!(begin, Begin::Building(_)), "{begin:?}");
    drop(begin);
    assert_eq!(files(&temp.0), (false, false), "a build of its own replaces them, header first");
    let _ = made(&temp.0, 4);
    assert_eq!(*stored(&temp.0).made(), whole, "the next build made them again");

    let damage: [(&str, Damage); 6] = [
        (
            "a gap in the table",
            Box::new(|plain| plain[HEADER_FIXED + ENTRY] = plain[HEADER_FIXED + ENTRY].wrapping_add(1)),
        ),
        (
            "a length past what a record can be",
            Box::new(|plain| plain[HEADER_FIXED + 4..HEADER_FIXED + 6].copy_from_slice(&u16::MAX.to_le_bytes())),
        ),
        ("more pages than the table has", Box::new(|plain| plain[16] += 1)),
        (
            "more pages without text than pages",
            Box::new(|plain| plain[20..24].copy_from_slice(&99u32.to_le_bytes())),
        ),
        ("a table stored", Box::new(|plain| plain[28 + 6 * 4] = 1)),
        ("a bin length that isn't the table's", Box::new(|plain| plain[HEADER_FIXED - 8] ^= 1)),
    ];
    for (what, change) in damage {
        let _ = made(&temp.0, 4);
        rewrite_header(&temp.0, change);
        assert!(StoredBlocks::open(tk(), &temp.0, KEY).is_err(), "{what} was believed");
    }
    // A changed byte of the header, and of a record (found when the page is read).
    let _ = made(&temp.0, 4);
    let mut header = fs::read(header_path(&paths(&temp.0))).unwrap();
    header[40] ^= 1;
    fs::write(header_path(&paths(&temp.0)), &header).unwrap();
    assert!(StoredBlocks::open(tk(), &temp.0, KEY).is_err(), "a changed header byte was believed");
    let _ = made(&temp.0, 4);
    let mut bin = fs::read(bin_path(&paths(&temp.0))).unwrap();
    for cut in [bin.len() - 1, 0] {
        fs::write(bin_path(&paths(&temp.0)), &bin[..cut]).unwrap();
        assert!(StoredBlocks::open(tk(), &temp.0, KEY).is_err(), "a bin cut to {cut} was believed");
    }
    bin.push(0);
    fs::write(bin_path(&paths(&temp.0)), &bin).unwrap();
    assert!(StoredBlocks::open(tk(), &temp.0, KEY).is_err(), "a longer bin was believed");
    // A record that is another page's: the pages are the same length, so the table is sound, and each opens only as its own.
    let _ = made(&temp.0, 4);
    let mut bin = fs::read(bin_path(&paths(&temp.0))).unwrap();
    let each = bin.len() / 4;
    let (first, rest) = bin.split_at_mut(each);
    first.swap_with_slice(&mut rest[..each]);
    fs::write(bin_path(&paths(&temp.0)), &bin).unwrap();
    let stored = stored(&temp.0);
    for page in 0..2 {
        assert_eq!(
            stored.page(page, text_of(page).chars().count() as u32),
            Err(OpenError::Damaged),
            "page {page} opened as another's"
        );
    }
    assert!(stored.page(2, text_of(2).chars().count() as u32).is_ok(), "an untouched page");
}

/// Failure: a clear or a sweep takes a folder while blocks are being made into it (the build then writes into nothing, or the clear says it has cleared what a build holds), or leaves it for ever after.
#[test]
fn a_clear_or_sweep_skips_a_folder_being_built_and_takes_it_after() {
    let temp = Temp::new();
    corpus(&temp.0, 2);
    let build = building(&temp.0);
    let error = clear(&temp.0, KEY).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    let sweep = BuildLimits {
        max_total_bytes: 0,
        ..BuildLimits::default()
    };
    assert_eq!(evict(&temp.0, &sweep, None).unwrap().corpora, 0);
    assert!(temp.0.join(KEY.hex()).exists());
    assert!(
        matches!(BlocksBuild::begin(tk(), &temp.0, KEY, &BuildLimits::default()), Begin::Busy),
        "a second build waits its turn"
    );
    drop(build);
    assert_eq!(evict(&temp.0, &sweep, None).unwrap().corpora, 1, "taken after the build");
    assert!(!temp.0.join(KEY.hex()).exists());
}

/// Failure: a new version of a file leaves the old version's blocks behind (they are made of the old text's offsets and say where its paragraphs are).
#[test]
fn a_new_version_of_the_file_takes_the_old_versions_blocks() {
    use crate::corpus::build_for_file;
    let temp = Temp::new();
    let root = temp.0.join("corpus");
    let file = temp.0.join("doc.pdf");
    fs::write(&file, b"one").unwrap();
    let first = CorpusKey::of_file(&file).unwrap();
    let feed = || Feed { pages: 2, no_text: Vec::new() };
    build_for_file(tk(), &file, &root, first, &mut feed(), &DocMeta::default(), &BuildLimits::default()).unwrap();
    let Begin::Building(mut build) = BlocksBuild::begin(tk(), &root, first, &BuildLimits::default()) else {
        panic!()
    };
    put_pages(&mut build, 0..2);
    build.finish(&tally(2)).unwrap();
    assert!(StoredBlocks::open(tk(), &root, first).unwrap().is_some());
    fs::write(&file, b"two!").unwrap();
    let second = CorpusKey::of_file(&file).unwrap();
    assert_ne!(first, second);
    build_for_file(tk(), &file, &root, second, &mut feed(), &DocMeta::default(), &BuildLimits::default()).unwrap();
    assert!(!root.join(first.hex()).exists(), "the old version's folder, and its blocks, are gone");
    assert!(StoredBlocks::open(tk(), &root, first).unwrap().is_none());
}

/// Failure: a document with no corpus, one this key may not touch or one that's already done is built anyway, or a page with no text is given as text.
#[test]
fn begin_says_what_it_found_and_a_page_without_text_has_none() {
    let temp = Temp::new();
    assert!(matches!(BlocksBuild::begin(tk(), &temp.0, KEY, &BuildLimits::default()), Begin::NotIndexed));
    let outcome = build(
        tk(),
        &temp.0,
        KEY,
        &mut Feed { pages: 3, no_text: vec![1] },
        &DocMeta::default(),
        &BuildLimits::default(),
    )
    .unwrap();
    assert!(matches!(outcome, BuildOutcome::Ready { .. }));
    let other = crate::seal::grant_for(tk(), CorpusKey([5; 32])).unwrap();
    assert!(
        matches!(BlocksBuild::begin(&other, &temp.0, KEY, &BuildLimits::default()), Begin::NotIndexed),
        "a key bound to another corpus"
    );
    let mut build = building(&temp.0);
    let mut chars = vec!['x'; 9];
    assert!(build.text(0, &mut chars));
    assert_eq!(chars.iter().collect::<String>(), text_of(0));
    assert!(!build.text(1, &mut chars), "a page with no text");
    assert!(chars.is_empty(), "and nothing left in the buffer");
    assert!(!build.text(9, &mut chars), "a page the document doesn't have");
    drop(build);
    let grant = crate::seal::grant_for(tk(), KEY).unwrap();
    assert!(
        matches!(BlocksBuild::begin(&grant, &temp.0, KEY, &BuildLimits::default()), Begin::Building(_)),
        "the grant for this corpus builds"
    );
    let _ = made(&temp.0, 3);
    assert!(matches!(BlocksBuild::begin(tk(), &temp.0, KEY, &BuildLimits::default()), Begin::Made(made) if made.pages == 3));
}
