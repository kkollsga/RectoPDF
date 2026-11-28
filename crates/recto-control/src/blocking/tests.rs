//! What a blocks job's end comes to: whether a header is written. Each test names the failure it catches.

use super::*;
use crate::structure::read_bytes;
use recto_index::blocks::map::Shape;
use recto_index::corpus::{StoredBlocks, build_text_corpus, remove_blocks};
use recto_platform::find_key::FindKey;
use recto_testpdf::blocks::BLOCKS_TEXT;

fn key() -> CorpusKey {
    CorpusKey::from_hex(&"2a".repeat(32)).unwrap()
}

fn find_key() -> FindKey {
    FindKey::for_tests(7)
}

struct Folder(recto_testpdf::TempDir);

/// A finished corpus of the blocks document's three pages, as the index keeps them, in a folder of its own.
fn corpus() -> Folder {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-blocking-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let texts: Vec<Option<String>> = BLOCKS_TEXT.iter().map(|lines| Some(lines.join("\n"))).collect();
    build_text_corpus(&find_key(), &root, (key(), None), &texts).unwrap();
    Folder(root)
}

fn build(folder: &Folder) -> BlocksBuild {
    match BlocksBuild::begin(&find_key(), &folder.0, key(), &BuildLimits::default()) {
        Begin::Building(build) => *build,
        other => panic!("{other:?}"),
    }
}

fn line(kind: &str, text: &str) -> String {
    format!(
        r#"{{"kind":"{kind}","level":0,"box":[10,20,300,60],"text":{},"continues":false}}"#,
        serde_json::to_string(text).unwrap()
    )
}

/// A helper's whole good answer for the blocks document: a heading on the first page, a heading and a list item on the second, a paragraph on the third.
fn said() -> Vec<u8> {
    let page = |page: u32, blocks: &[String]| format!(r#"{{"page":{page},"blocks":[{}]}}"#, blocks.join(","));
    let lines = [
        r#"{"state":"hello","walled":true}"#.to_string(),
        page(0, &[line("heading", BLOCKS_TEXT[0][0])]),
        page(1, &[line("heading", BLOCKS_TEXT[1][0]), line("list_item", BLOCKS_TEXT[1][1])]),
        page(2, &[line("para", BLOCKS_TEXT[2][0])]),
        r#"{"state":"ready","pages":3,"pages_failed":0,"blocks":4,"cut":0,"distilled_ms":10,"helper_peak_bytes":5000000}"#.to_string(),
    ];
    format!("{}\n", lines.join("\n")).into_bytes()
}

/// The end of a job that was answered whole.
fn answered(folder: &Folder) -> Ended<BlocksAnswer<Store>> {
    let (answer, end, bad) = read_bytes(&said(), BlocksAnswer::new(Store(build(folder)), 3));
    assert!(!bad && end.is_some(), "the answer was believed");
    Ended::Answered(answer)
}

fn stored(folder: &Folder) -> Option<StoredBlocks> {
    StoredBlocks::open(&find_key(), &folder.0, key()).unwrap()
}

/// Failure: blocks are made whole (the header written, and the file believed to have them) for a file that was swapped by a rename under the helper (which only a look at its path shows), for a job the reader
/// stopped, or for one the helper didn't answer to its end; or a good answer isn't made whole. Written only when the answer was believed to its end, the file is the one asked for and no stop was asked; every other end leaves no file.
#[test]
fn a_changed_file_or_a_cancel_writes_nothing_and_a_good_answer_writes_the_header() {
    let folder = corpus();
    let still = || true;
    let mut report = Value::Null;
    let stop = AtomicBool::new(false);

    let outcome = outcome_of(answered(&folder), || false, &stop, &mut report);
    assert!(matches!(outcome, Outcome::Changed), "{outcome:?}");
    assert!(stored(&folder).is_none(), "a file that changed gets none");
    assert!(!folder.0.join(key().hex()).join("v4/app/blocks.bin").exists(), "and leaves no bin");

    let outcome = outcome_of(answered(&folder), still, &AtomicBool::new(true), &mut report);
    assert!(matches!(outcome, Outcome::Changed), "{outcome:?}");
    assert!(stored(&folder).is_none(), "a stopped job gets none");

    for ended in [
        Ended::Refused("couldnt_read".to_string()),
        Ended::Changed,
        Ended::Failed("over_memory".to_string()),
    ] {
        let outcome = outcome_of(ended, still, &stop, &mut report);
        assert!(!matches!(outcome, Outcome::Ready(_)), "{outcome:?}");
    }
    assert!(stored(&folder).is_none());

    let outcome = outcome_of(answered(&folder), still, &stop, &mut report);
    let Outcome::Ready(made) = outcome else { panic!("{outcome:?}") };
    let found = stored(&folder).expect("the header is written");
    assert_eq!(*found.made(), made);
    assert!(made.stored[Shape::Heading.index()] > 0 && made.stored.iter().sum::<u32>() >= 3, "{made:?}");
    assert_eq!(made.stored[Shape::Table.index()], 0);
    assert_eq!(report["pages"], 3);
    assert_eq!(report["bytes"], made.bytes);
    remove_blocks(&folder.0, key());
}

/// Failure: a store that can't take a page (too large, or a write that failed) goes on to be believed, so a document's blocks are made with pages missing; or says the wrong code, which the queue then
/// remembers (a limit is that version's own) or pauses.
#[test]
fn a_store_that_cant_take_a_page_stops_the_job_with_its_code() {
    let folder = corpus();
    let tight = BuildLimits {
        max_doc_bytes: 1,
        ..BuildLimits::default()
    };
    let Begin::Building(build) = BlocksBuild::begin(&find_key(), &folder.0, key(), &tight) else {
        panic!()
    };
    let (answer, _, bad) = read_bytes(&said(), BlocksAnswer::new(Store(*build), 3));
    assert!(bad, "the answer stopped, so it was not believed to its end");
    assert_eq!(crate::structure::Answer::stopped_for(&answer), Some("too_large"));
    assert!(stored(&folder).is_none());
}
