//! The stored identities: what a build writes beside the text and what a reader trusts of it. Each test names the failure it catches.

use super::*;
use crate::corpus::{BuildOutcome, CorpusKey, DiskState, DocMeta, FailReason, FedPage, FeedError, ReadyCorpus, build, disk_state};
use recto_core::locator::{Referrer, ScanCaps};
use recto_proto::CharRecord;
use recto_testpdf::{Object, Page, TempPdf};
use std::os::unix::fs::symlink;

fn tk() -> &'static FindKey {
    static KEY: std::sync::OnceLock<FindKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| FindKey::for_tests(9))
}

/// A folder of its own, private and removed at the end.
struct Temp(recto_testpdf::TempDir);

impl Temp {
    fn new() -> Self {
        let path = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-ids-");
        local::ensure_private_dir(&path).unwrap();
        Self(path)
    }
}

/// Other rules for the rest of the test, on this thread.
struct RulesTag;

impl RulesTag {
    fn set(tag: [u8; 16]) -> Self {
        super::RULES_TAG_OVERRIDE.with(|cell| cell.set(Some(tag)));
        Self
    }
}

impl Drop for RulesTag {
    fn drop(&mut self) {
        super::RULES_TAG_OVERRIDE.with(|cell| cell.set(None));
    }
}

const KEY: CorpusKey = CorpusKey([77; 32]);

/// The read a page of the made-up document has: no identity on every fifth page, and what it was charged.
fn made_up(page: u32) -> PageRead {
    PageRead {
        identity: (page % 5 != 4).then(|| PageIdentity {
            digest: [(page as u8).wrapping_add(1); 32],
            object: (page + 10, 0),
            frame: page * 4,
        }),
        bytes: 100 + u64::from(page),
    }
}

/// A feed of `pages` text pages, with identities for `map` pages of a made-up map, or from a real file.
struct Feed {
    pages: u32,
    map: Option<u32>,
    real: Option<FileIdentities>,
    ids: Option<DocumentIds>,
    /// Identity reads, in order.
    asked: Vec<u32>,
    /// Pauses the build at this page, as a kill would.
    stop_at: Option<u32>,
    /// Page `n` fails once for the moment, so the build's own retry reads it again.
    fail_once: Option<u32>,
    failed: bool,
    changed: bool,
    /// What the last look at the file saw: whether the header was there, and how many identity reads had been made.
    last_look: Option<(bool, usize)>,
    watch: Option<CorpusPaths>,
    /// A copy of `ids.bin` as the retry began.
    at_retry: Option<Vec<u8>>,
    /// Each identity read takes this long (spent working, not sleeping).
    spin: Option<std::time::Duration>,
}

impl Feed {
    fn made_up(pages: u32) -> Self {
        Self {
            pages,
            map: Some(pages),
            real: None,
            ids: None,
            asked: Vec::new(),
            stop_at: None,
            fail_once: None,
            failed: false,
            changed: false,
            last_look: None,
            watch: None,
            at_retry: None,
            spin: None,
        }
    }

    fn without_identities(pages: u32) -> Self {
        Self {
            map: None,
            ..Self::made_up(pages)
        }
    }
}

impl PageFeed for Feed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Ok(self.pages)
    }

    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        if self.stop_at == Some(page) {
            return Err(FeedError::Cant("stopped_here"));
        }
        if self.fail_once == Some(page) && !self.failed {
            self.failed = true;
            return Err(FeedError::Page(FailReason::Deadline));
        }
        let chars = format!("Page {page}").chars().map(|ch| CharRecord::new(ch as u32, 0)).collect();
        Ok(FedPage {
            chars,
            boxes: Vec::new(),
            hidden_checked: true,
        })
    }

    fn still_the_same_file(&mut self) -> bool {
        self.last_look = self.watch.as_ref().map(|paths| (paths.ids_header().exists(), self.asked.len()));
        !self.changed
    }

    fn forget_failures(&mut self) {
        if let Some(paths) = &self.watch {
            self.at_retry = fs::read(paths.ids_file()).ok();
        }
    }

    fn identities(&mut self) -> Option<IdentityInfo> {
        match &self.real {
            Some(real) => Some(real.info()),
            None => self.map.map(|pages| IdentityInfo { pages, ids: self.ids }),
        }
    }

    fn identity(&mut self, page: u32) -> PageRead {
        self.asked.push(page);
        if let Some(spin) = self.spin {
            let start = std::time::Instant::now();
            while start.elapsed() < spin {
                std::hint::spin_loop();
            }
        }
        match &self.real {
            Some(real) => real.read(page),
            None => made_up(page),
        }
    }
}

fn built(root: &Path, feed: &mut Feed, limits: &BuildLimits) -> BuildOutcome {
    build(tk(), root, KEY, feed, &DocMeta::default(), limits).unwrap()
}

fn paths(root: &Path) -> CorpusPaths {
    CorpusPaths::new(root, KEY)
}

fn stored(root: &Path) -> Identities {
    Identities::open(tk(), root, KEY).expect("holds up").expect("there are identities")
}

fn read_of(source: &impl PageSource, page: u32) -> SourceRead {
    source.read(page, &Caps::STANDARD)
}

fn every_read(source: &impl PageSource) -> Vec<SourceRead> {
    (0..source.len() as u32).map(|page| read_of(source, page)).collect()
}

fn sum_of_made_up(pages: u32) -> u64 {
    (0..pages).map(|page| made_up(page).bytes).sum()
}

/// A document with a page of text, an image, a form, a page with nothing on it and a page that names more things to draw than a page may (no identity), and a trailer with an `/ID` pair.
fn fixture() -> Vec<u8> {
    let first = recto_testpdf::first_extra_object(5);
    let scan_cap = Caps::STANDARD.xobjects;
    let resources = format!("/XObject << /Im {first} 0 R /Fm {} 0 R /X {} 0 R >>", first + 1, first + 2);
    let pages = [
        Page::new("BT /F1 12 Tf 72 700 Td (Some text) Tj ET"),
        Page::new("q 100 0 0 100 10 10 cm /Im Do Q"),
        Page::new("q /Fm Do Q"),
        Page::new(""),
        Page::new((0..scan_cap + 1).map(|name| format!("/N{name} Do ")).collect::<String>()),
    ];
    let bytes = recto_testpdf::pages_with(
        &pages,
        &resources,
        vec![
            Object::Stream {
                dict: "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray".into(),
                data: vec![0x80],
            },
            Object::Stream {
                dict: "/Type /XObject /Subtype /Form /BBox [0 0 100 100]".into(),
                data: b"0 0 50 50 re f".to_vec(),
            },
            Object::Stream {
                dict: format!("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << {resources} >>"),
                data: b"q /X Do Q q 1 0 0 1 5 0 cm /X Do Q".to_vec(),
            },
        ],
    );
    // The trailer comes after the cross-reference table, so a longer one moves no offset.
    let (old, new): (&[u8], &[u8]) = (
        b"/Root 1 0 R >>",
        b"/Root 1 0 R /ID [<00112233445566778899aabbccddeeff> <ffeeddccbbaa99887766554433221100>] >>",
    );
    let at = bytes.windows(old.len()).rposition(|window| window == old).expect("the trailer");
    [&bytes[..at], new, &bytes[at + old.len()..]].concat()
}

/// Failure: what a build stores is not what `read_page` says of the page (a field dropped or swapped, the bytes it was charged lost, a page with no identity
/// stored as one, the `/ID` pair lost), or a reference made from the stored reads differs from the one made from the file. Real lopdf, on a page of text, an
/// image, a form, an empty page and a page past a cap.
#[test]
fn stored_identities_equal_read_page_for_every_page() {
    let temp = Temp::new();
    let pdf = TempPdf::new("doc.pdf", &fixture());
    let stamp = recto_platform::file_stamp(&pdf.0).unwrap();
    let real = FileIdentities::open(&pdf.0, stamp).expect("opens");
    let live = page_id::open_version_for_identity(&pdf.0, stamp).expect("opens");
    let map = live.page_map().unwrap();
    let mut feed = Feed {
        real: Some(real),
        ..Feed::made_up(5)
    };
    assert!(matches!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::Ready { .. }));
    let ids = stored(&temp.0);
    assert_eq!(ids.len(), map.len());
    let mut some = 0;
    for (page, entry) in map.iter().enumerate() {
        let expected = page_id::read_page(&live, entry, &Caps::STANDARD);
        some += usize::from(expected.identity.is_some());
        assert_eq!(read_of(&ids, page as u32), SourceRead::Read(expected), "page {page}");
    }
    assert!(
        some > 0 && some < map.len(),
        "the fixture has pages with an identity and a page without: {some} of {}",
        map.len()
    );
    assert_eq!(ids.document_ids(), page_id::document_ids(&live));
    assert!(ids.document_ids().is_some());
    let caps = ScanCaps::STANDARD;
    let (mut from_stored, mut from_file) = (Referrer::over(&ids, &caps).expect("standard caps"), Referrer::new(&live, &map, &caps));
    for page in 0..=map.len() as u32 {
        assert_eq!(from_stored.page(page), from_file.page(page), "a reference to page {page}");
    }
    assert_eq!(from_stored.spent(), from_file.spent());
}

/// Failure: a corpus built with no identities (every corpus before this, or a feed that can't read them) is read as damaged, or thrown away, or leaves a
/// header that names records that aren't there. It answers its text, and says there are no identities.
#[test]
fn a_corpus_without_identities_still_answers_and_has_none() {
    let temp = Temp::new();
    let mut feed = Feed::without_identities(3);
    assert!(matches!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::Ready { .. }));
    assert!(ReadyCorpus::open(tk(), &temp.0, KEY).unwrap().is_some());
    assert!(!paths(&temp.0).ids_file().exists() && !paths(&temp.0).ids_header().exists());
    assert!(matches!(Identities::open(tk(), &temp.0, KEY), Ok(None)));
}

/// Failure: the header is written before the last look at the file (a file that changes in between has identities of its old self believed), an identity is
/// read after that look, or a page is missed. At the last look every identity is read and no header is there.
#[test]
fn the_header_comes_after_the_last_look_at_the_file_and_every_read_before_it() {
    let temp = Temp::new();
    // The map is longer than the renderer's count, so the 30 pages past it are read by `complete()` alone: a `complete()` moved after the last look shows here.
    let mut feed = Feed::made_up(100);
    feed.map = Some(130);
    feed.watch = Some(paths(&temp.0));
    assert!(matches!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::Ready { .. }));
    assert_eq!(feed.last_look, Some((false, 130)), "no header yet, and all 130 reads made");
    assert_eq!(feed.asked, (0..130).collect::<Vec<_>>(), "each page once, in order");
    assert!(paths(&temp.0).ids_header().exists());
}

/// Failure: a file that changed during the build keeps identities of a version it no longer is (or the text's but not theirs): neither file is left.
#[test]
fn a_file_changed_during_a_build_leaves_no_identity_files() {
    let temp = Temp::new();
    let mut feed = Feed::made_up(5);
    feed.changed = true;
    assert_eq!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::FileChanged);
    assert!(!paths(&temp.0).ids_file().exists() && !paths(&temp.0).ids_header().exists());
}

/// Failure: the map is longer or shorter than the renderer's page count and the records follow the renderer (a reference counts lopdf's pages), so a page
/// past the renderer's count has no record or a record is made for a page lopdf doesn't have.
#[test]
fn the_records_follow_the_page_map_not_the_renderers_count() {
    for (pages, map) in [(3, 5), (5, 3)] {
        let temp = Temp::new();
        let mut feed = Feed::made_up(pages);
        feed.map = Some(map);
        assert!(matches!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::Ready { .. }));
        let ids = stored(&temp.0);
        assert_eq!(ids.len(), map as usize, "text {pages}, map {map}");
        assert_eq!(read_of(&ids, map - 1), SourceRead::Read(made_up(map - 1)));
        assert_eq!(read_of(&ids, map), SourceRead::Absent);
    }
}

/// Failure: a map past what a corpus holds gets identities anyway, so a hostile file makes a build write 200,000 records and more.
#[test]
fn a_map_past_the_page_cap_has_no_identities() {
    let temp = Temp::new();
    let mut feed = Feed::made_up(3);
    feed.map = Some(MAX_PAGES + 1);
    assert!(matches!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::Ready { .. }));
    assert!(feed.asked.is_empty(), "nothing was read for it");
    assert!(matches!(Identities::open(tk(), &temp.0, KEY), Ok(None)));
}

/// Failure: the identity limit is off by one: a build charged exactly the limit loses its identities, or one charged a byte more keeps them. And what's left
/// when they are given up: no header, no records, and the corpus is as ready as ever.
#[test]
fn identity_bytes_at_the_limit_store_and_one_less_does_not() {
    let sum = sum_of_made_up(7);
    for (limit, stores) in [(sum, true), (sum - 1, false)] {
        let temp = Temp::new();
        let mut feed = Feed::made_up(7);
        let limits = BuildLimits {
            identity_bytes: limit,
            ..BuildLimits::default()
        };
        assert!(matches!(built(&temp.0, &mut feed, &limits), BuildOutcome::Ready { .. }), "limit {limit}");
        assert_eq!(Identities::open(tk(), &temp.0, KEY).map(|found| found.is_some()), Ok(stores), "limit {limit}");
        if !stores {
            assert!(!paths(&temp.0).ids_file().exists() && !paths(&temp.0).ids_header().exists());
            assert!(ReadyCorpus::open(tk(), &temp.0, KEY).unwrap().is_some());
            assert_eq!(feed.asked.len() as u64, 7, "the last page is read before the sum is known to be over");
        }
    }
}

/// Failure: the records and the header aren't counted toward what a document may take (a hostile file with 200,000 pages gets 15.8 MiB past its limit), or
/// are counted off by one. The least limit that builds without identities, plus the records and the header, builds with them, and one byte less is `too_large`.
#[test]
fn max_doc_bytes_counts_the_identities_at_the_limit_and_one_less() {
    let least = |ids: bool| {
        let (mut low, mut high) = (0u64, 1 << 20);
        while low < high {
            let middle = (low + high) / 2;
            let temp = Temp::new();
            let mut feed = if ids { Feed::made_up(6) } else { Feed::without_identities(6) };
            let limits = BuildLimits {
                max_doc_bytes: middle,
                ..BuildLimits::default()
            };
            match built(&temp.0, &mut feed, &limits) {
                BuildOutcome::Ready { .. } => high = middle,
                BuildOutcome::Refused(code) => {
                    assert_eq!(code, "too_large");
                    low = middle + 1;
                }
                other => panic!("{other:?}"),
            }
        }
        low
    };
    assert_eq!(least(true), least(false) + 6 * RECORD_BYTES as u64 + HEADER_BYTES as u64);
}

/// Failure: a page whose text failed for the moment is read again and the retry touches the identities (reads them again, or rewrites the file), where the
/// identity doesn't depend on the renderer. The records are the same bytes when the retry began as when it ended, and no identity is read twice.
#[test]
fn a_retry_of_failed_pages_leaves_the_identities_alone() {
    let temp = Temp::new();
    let mut feed = Feed::made_up(4);
    feed.fail_once = Some(2);
    feed.watch = Some(paths(&temp.0));
    let BuildOutcome::Ready { summary, .. } = built(&temp.0, &mut feed, &BuildLimits::default()) else {
        panic!("built")
    };
    assert_eq!(summary.failed_pages, 0, "the retry read the page");
    let at_retry = feed.at_retry.take().expect("the retry began");
    assert_eq!(fs::read(paths(&temp.0).ids_file()).unwrap(), at_retry, "the same bytes after the retry");
    assert_eq!(feed.asked, vec![0, 1, 2, 3], "no identity read again");
    assert_eq!(every_read(&stored(&temp.0)).len(), 4);
}

fn all_reads(root: &Path) -> Vec<SourceRead> {
    every_read(&stored(root))
}

/// Failure: a build killed while it writes `ids.bin` goes on from a record that isn't whole or a record with no text after it, and the corpus has a record
/// that is not its page's, or is short. It ends with the identities an uninterrupted build has, whatever the kill left.
#[test]
fn a_build_resumed_after_a_kill_ends_with_the_identities_of_an_uninterrupted_one() {
    let whole = Temp::new();
    assert!(matches!(
        built(&whole.0, &mut Feed::made_up(8), &BuildLimits::default()),
        BuildOutcome::Ready { .. }
    ));
    let expected = all_reads(&whole.0);
    // What a kill after three pages can leave in `ids.bin`: the three records and a piece of a fourth, or the fourth whole with no text after it, or only two.
    type Cut = fn(&mut Vec<u8>);
    let cuts: [(&str, Cut); 3] = [
        ("a record cut in half", |bytes| bytes.extend_from_slice(&[0x55; RECORD_BYTES / 2])),
        ("a whole record with no text after it", |bytes| bytes.extend_from_slice(&[0x55; RECORD_BYTES])),
        ("two records of the three", |bytes| bytes.truncate(2 * RECORD_BYTES)),
    ];
    for (what, damage) in cuts {
        let temp = Temp::new();
        let mut feed = Feed::made_up(8);
        feed.stop_at = Some(3);
        assert!(matches!(built(&temp.0, &mut feed, &BuildLimits::default()), BuildOutcome::Paused(_)), "{what}");
        rewrite_records(&temp.0, |bytes| {
            assert_eq!(bytes.len(), 3 * RECORD_BYTES, "three pages were done");
            damage(bytes);
        });
        let mut again = Feed::made_up(8);
        assert!(
            matches!(built(&temp.0, &mut again, &BuildLimits::default()), BuildOutcome::Ready { .. }),
            "{what}"
        );
        assert_eq!(all_reads(&temp.0), expected, "{what}");
    }
}

/// Failure: a resumed build whose records hold up reads them all again (a kill costs every identity read), or trusts a file it didn't check.
#[test]
fn a_resume_keeps_the_records_that_hold_up() {
    let temp = Temp::new();
    let mut feed = Feed::made_up(8);
    feed.stop_at = Some(5);
    built(&temp.0, &mut feed, &BuildLimits::default());
    let mut again = Feed::made_up(8);
    assert!(matches!(built(&temp.0, &mut again, &BuildLimits::default()), BuildOutcome::Ready { .. }));
    assert_eq!(again.asked, vec![5, 6, 7], "the five before the kill were kept");
}

/// Failure: a build paused under one set of rules and resumed under another keeps the first one's records, and the header then vouches for them under the
/// second's tag (a stale "no identity" answers as current). The records of another rules tag are read again, and the finished corpus holds this build's.
#[test]
fn a_resume_under_other_rules_reads_every_record_again() {
    let temp = Temp::new();
    let mut feed = Feed::made_up(8);
    feed.stop_at = Some(5);
    built(&temp.0, &mut feed, &BuildLimits::default());
    let _other_rules = RulesTag::set([9; 16]);
    let mut again = Feed::made_up(8);
    assert!(matches!(built(&temp.0, &mut again, &BuildLimits::default()), BuildOutcome::Ready { .. }));
    assert_eq!(again.asked, (0..8).collect::<Vec<_>>(), "none of the five before the pause was kept");
    assert_eq!(every_read(&stored(&temp.0)).len(), 8);
}

fn rewrite_records(root: &Path, change: impl FnOnce(&mut Vec<u8>)) {
    let file = paths(root).ids_file();
    let mut bytes = fs::read(&file).unwrap();
    change(&mut bytes);
    fs::write(&file, &bytes).unwrap();
}

/// Failure: a record moved to another page's slot, or one with a byte flipped, answers for a page (another page's identity is believed), and one record's
/// damage costs the others their answers.
#[test]
fn a_record_in_the_wrong_slot_or_with_a_flipped_byte_answers_for_no_page() {
    let temp = Temp::new();
    built(&temp.0, &mut Feed::made_up(6), &BuildLimits::default());
    let good = all_reads(&temp.0);
    rewrite_records(&temp.0, |bytes| {
        let (a, b) = (RECORD_BYTES, 2 * RECORD_BYTES);
        let first = bytes[a..b].to_vec();
        bytes.copy_within(b..b + RECORD_BYTES, a);
        bytes[b..b + RECORD_BYTES].copy_from_slice(&first);
        bytes[4 * RECORD_BYTES + 20] ^= 1;
    });
    let after = all_reads(&temp.0);
    for page in [1, 2, 4] {
        assert_eq!(after[page], SourceRead::Unreadable, "page {page}");
    }
    for page in [0, 3, 5] {
        assert_eq!(after[page], good[page], "page {page} is as it was");
    }
}

/// Failure: a file one record short or one long, or a header that names another count, is believed. A map length that's off is damage, never another page's
/// identity. Nothing is believed of a header that doesn't open.
#[test]
fn a_file_a_record_short_or_long_and_a_damaged_header_are_damage() {
    for (what, change) in [
        (
            "one record short",
            (|bytes: &mut Vec<u8>| bytes.truncate(bytes.len() - RECORD_BYTES)) as fn(&mut Vec<u8>),
        ),
        ("one record long", |bytes| bytes.extend_from_slice(&[0; RECORD_BYTES])),
        ("a byte of a record cut off", |bytes| {
            bytes.pop();
        }),
    ] {
        let temp = Temp::new();
        built(&temp.0, &mut Feed::made_up(6), &BuildLimits::default());
        rewrite_records(&temp.0, change);
        assert!(matches!(Identities::open(tk(), &temp.0, KEY), Err(Damaged)), "{what}");
    }
    let temp = Temp::new();
    built(&temp.0, &mut Feed::made_up(6), &BuildLimits::default());
    let header = paths(&temp.0).ids_header();
    let mut bytes = fs::read(&header).unwrap();
    bytes[20] ^= 1;
    fs::write(&header, &bytes).unwrap();
    assert!(matches!(Identities::open(tk(), &temp.0, KEY), Err(Damaged)), "a flipped byte in the header");
    remove_identities(&temp.0, KEY);
    assert!(matches!(Identities::open(tk(), &temp.0, KEY), Ok(None)), "taken away, it's an older corpus");
    assert!(ReadyCorpus::open(tk(), &temp.0, KEY).unwrap().is_some(), "and the text is still its own");
}

/// Failure: identities made under other rules (another digest, other caps, another number of tag pages) answer. A header with another rules tag is not
/// trusted, and is not damage: nothing is thrown away.
#[test]
fn a_header_with_another_rules_tag_is_not_trusted_and_is_not_damage() {
    let temp = Temp::new();
    built(&temp.0, &mut Feed::made_up(4), &BuildLimits::default());
    let seal = read_seal(tk(), &paths(&temp.0)).expect("the corpus's seal");
    let mut plain = vec![0; HEADER_PLAIN];
    plain[..16].copy_from_slice(&stored_rules_tag());
    plain[0] ^= 0xff;
    plain[16..20].copy_from_slice(&4u32.to_le_bytes());
    crate::corpus::sealed::write_sealed(&seal, &paths(&temp.0).ids_header(), Purpose::Identities, &plain, true).unwrap();
    assert!(matches!(Identities::open(tk(), &temp.0, KEY), Ok(None)));
    assert!(paths(&temp.0).ids_file().exists(), "nothing was thrown away for it");
}

/// Failure: a link or a named pipe put where `ids.bin` or `ids.sealed` goes is followed or waited on, so a search reads something else as a document's
/// identities or a helper waits for ever; or a build writes through a link and changes a file somewhere else.
#[test]
fn a_link_or_a_pipe_for_an_identity_file_is_refused_with_no_wait_and_nothing_it_points_at_changes() {
    for (name, of_file) in [("ids.bin", true), ("ids.sealed", false)] {
        let temp = Temp::new();
        built(&temp.0, &mut Feed::made_up(4), &BuildLimits::default());
        let place = if of_file { paths(&temp.0).ids_file() } else { paths(&temp.0).ids_header() };
        let outside = temp.0.join(format!("outside-{name}"));
        fs::write(&outside, b"keep me").unwrap();
        let swap = |make: &dyn Fn(&Path)| {
            fs::remove_file(&place).unwrap();
            make(&place);
        };
        swap(&|place| symlink(&outside, place).unwrap());
        let root = temp.0.to_path_buf();
        assert!(
            within_20_seconds(name, move || matches!(Identities::open(tk(), &root, KEY), Err(Damaged))),
            "a link at {name}"
        );
        swap(&|place| assert!(std::process::Command::new("/usr/bin/mkfifo").arg(place).status().unwrap().success()));
        let root = temp.0.to_path_buf();
        assert!(
            within_20_seconds(name, move || matches!(Identities::open(tk(), &root, KEY), Err(Damaged))),
            "a pipe at {name}"
        );
        // A build over what's there: not followed, not waited on, and the identities are whole again after.
        fs::remove_file(paths(&temp.0).ready).unwrap();
        let root = temp.0.to_path_buf();
        assert!(
            within_20_seconds(name, move || matches!(
                built(&root, &mut Feed::made_up(4), &BuildLimits::default()),
                BuildOutcome::Ready { .. }
            )),
            "{name}"
        );
        assert_eq!(fs::read(&outside).unwrap(), b"keep me", "{name}: what the link pointed at");
        assert_eq!(every_read(&stored(&temp.0)).len(), 4);
    }
}

/// `work` on a thread, so a regression to an open that blocks on a pipe fails here in 20 s and says so, where nextest would wait out 300 s.
fn within_20_seconds<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(work());
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(20))
        .unwrap_or_else(|_| panic!("{what}: blocked on a pipe for 20 s"))
}

/// Failure: the corpus's state says Ready on a build that wrote identities, but `disk_state` or a later build reads the new files as something else.
#[test]
fn the_identity_files_do_not_change_what_a_corpus_says_of_itself() {
    let (with, without) = (Temp::new(), Temp::new());
    built(&with.0, &mut Feed::made_up(4), &BuildLimits::default());
    built(&without.0, &mut Feed::without_identities(4), &BuildLimits::default());
    let (DiskState::Ready(a), DiskState::Ready(b)) = (disk_state(tk(), &with.0, KEY), disk_state(tk(), &without.0, KEY)) else {
        panic!("both ready")
    };
    assert_eq!((a.pages, a.table_bytes, a.pack_bytes), (b.pages, b.table_bytes, b.pack_bytes));
    assert!(a.corpus_bytes >= b.corpus_bytes + 4 * RECORD_BYTES as u64 + HEADER_BYTES as u64);
    assert!(matches!(
        build(tk(), &with.0, KEY, &mut Feed::made_up(4), &DocMeta::default(), &BuildLimits::default()).unwrap(),
        BuildOutcome::AlreadyReady(_)
    ));
}

/// Failure: a build killed after its header but before `ready.sealed`, started again with identities it can't keep (over the limit), leaves the old header
/// naming records that are gone, so the corpus is read as damaged where it only has none. The header goes before the records change.
#[test]
fn a_new_build_takes_the_old_header_away_before_it_changes_the_records() {
    let temp = Temp::new();
    built(&temp.0, &mut Feed::made_up(4), &BuildLimits::default());
    fs::remove_file(paths(&temp.0).ready).unwrap();
    let limits = BuildLimits {
        identity_bytes: 0,
        ..BuildLimits::default()
    };
    assert!(matches!(built(&temp.0, &mut Feed::made_up(4), &limits), BuildOutcome::Ready { .. }));
    assert!(matches!(Identities::open(tk(), &temp.0, KEY), Ok(None)));
}
