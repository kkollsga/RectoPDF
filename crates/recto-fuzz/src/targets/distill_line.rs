//! Hostile page lines from the block helper, and a hostile page to map them onto. The input is a page's text, a zero byte, then lines of JSON, one page each
//! (`{"page":N,"blocks":[{"kind","level","box","text","continues"}]}`).
//!
//! The same lines also go through the session's reader, wrapped as a helper says them (a hello, the lines, an end that adds up) into `structure::blocks::read_all` with a store in memory: what is put is in page order and once, and what is stored and dropped adds up to what was said.
//!
//! What must hold: no panic or hang; every block is stored or counted as dropped (the unreadable too); what's stored is inside the page, in whole points and in order, overlaps nothing, is at
//! most 512 blocks, and seals into a record that opens as the same blocks. And the rule itself, checked from the page and the candidate again: each stored block's letters are its candidate's
//! letters, exactly, and a block the rule's second step placed was the one place on the page that overlapped no block stored before it.

use recto_control::structure::blocks::{PageStore, read_all};
use recto_core::match_letters;
use recto_index::blocks::map::{PageLines, Step, map_line};
use recto_index::blocks::{MAX_BLOCKS, MAX_LEVEL, PageBlocks, codec};
use recto_index::corpus::CorpusKey;
use recto_index::seal::CorpusSeal;
use recto_platform::find_key::FindKey;
use std::sync::{Arc, Mutex, OnceLock};

/// Most characters of page the target reads: a page is mapped, not stored, so a small one finds what a big one would.
const MAX_PAGE: usize = 4096;
const MAX_LINES: usize = 64;
const PAGES: u32 = 8;

fn seal() -> &'static CorpusSeal {
    static SEAL: OnceLock<CorpusSeal> = OnceLock::new();
    SEAL.get_or_init(|| {
        let key = FindKey::in_memory().expect("a key");
        CorpusSeal::create(&key, CorpusKey::from_hex(&"b8".repeat(32)).expect("a corpus key"))
            .expect("a seal")
            .0
    })
}

fn letters_of(text: impl IntoIterator<Item = char>) -> Vec<char> {
    let mut out = Vec::new();
    match_letters(text, |letter, _| out.push(letter));
    out
}

/// Every page has the one page of text the target was given; what is put is kept where the target can read it whatever became of the answer.
struct Memory {
    page: Vec<char>,
    put: Arc<Mutex<Vec<PageBlocks>>>,
}

impl PageStore for Memory {
    fn text(&mut self, _page: u32, chars: &mut Vec<char>) -> bool {
        chars.extend_from_slice(&self.page);
        !self.page.is_empty()
    }

    fn put(&mut self, page: &PageBlocks) -> Result<(), &'static str> {
        self.put.lock().expect("a store").push(page.clone());
        Ok(())
    }
}

/// The lines as a helper's whole answer. The first run is every line as it came, which mostly ends as a bad answer and may stop anywhere; the second is the lines the reader takes, in order, with an end that adds up, which must be believed whole.
fn through_the_session(page: &[char], lines: &[&[u8]]) {
    let hello = br#"{"state":"hello","walled":true}"#;
    let whole = |lines: &[&[u8]], end: Option<String>| {
        let mut said = hello.to_vec();
        for line in lines {
            said.push(b'\n');
            said.extend_from_slice(line);
        }
        said.push(b'\n');
        if let Some(end) = end {
            said.extend_from_slice(end.as_bytes());
            said.push(b'\n');
        }
        said
    };
    let in_order = |put: &Mutex<Vec<PageBlocks>>| {
        let put = put.lock().expect("a store");
        assert!(
            put.iter().enumerate().all(|(at, page)| page.page as usize == at),
            "put out of order or twice: {:?}",
            put.iter().map(|page| page.page).collect::<Vec<_>>()
        );
    };
    let put = Arc::new(Mutex::new(Vec::new()));
    let _ = read_all(
        &whole(lines, None),
        PAGES,
        Memory {
            page: page.to_vec(),
            put: Arc::clone(&put),
        },
    );
    in_order(&put);
    // What the reader would take, by a reader of its own: pages in order from 0, none missing.
    let (mut reader, mut next, mut said_blocks) = (PageLines::new(PAGES), 0u32, 0u64);
    let taken: Vec<&[u8]> = lines
        .iter()
        .take_while(|line| match reader.read(line) {
            Ok(read) if read.page == next => {
                next += 1;
                said_blocks += (read.candidates.len() + read.unreadable as usize) as u64;
                true
            }
            _ => false,
        })
        .copied()
        .collect();
    let end = format!(r#"{{"state":"ready","pages":{next},"pages_failed":0,"blocks":{said_blocks},"cut":0}}"#);
    let put = Arc::new(Mutex::new(Vec::new()));
    let (_, tally, _) = read_all(
        &whole(&taken, Some(end)),
        next,
        Memory {
            page: page.to_vec(),
            put: Arc::clone(&put),
        },
    )
    .expect("lines the reader takes, and an end that adds up, are believed");
    in_order(&put);
    assert_eq!(put.lock().expect("a store").len(), taken.len(), "every page was put");
    let (stored, dropped) = (tally.stored.iter().sum::<u32>(), tally.dropped.iter().sum::<u32>() + tally.unreadable);
    assert_eq!(u64::from(stored) + u64::from(dropped), said_blocks, "every block is stored or dropped");
}

pub fn check(data: &[u8]) {
    let (page, lines) = data.split_at(data.iter().position(|byte| *byte == 0).unwrap_or(data.len()));
    // `split_at` leaves the zero byte at the front of the lines: it went in front of the first line, which was never read as a page line.
    let lines = lines.get(1..).unwrap_or_default();
    let page: Vec<char> = String::from_utf8_lossy(page).chars().take(MAX_PAGE).collect();
    through_the_session(&page, &lines.split(|byte| *byte == b'\n').take(MAX_LINES).collect::<Vec<_>>());
    let mut reader = PageLines::new(PAGES);
    for line in lines.split(|byte| *byte == b'\n').take(MAX_LINES) {
        let Ok(line) = reader.read(line) else { continue };
        let mapped = map_line(&page, &line);
        assert_eq!(
            (mapped.counts.stored_total() + mapped.counts.dropped_total()) as usize,
            line.candidates.len() + line.unreadable as usize,
            "every block is stored or counted"
        );
        assert_eq!(mapped.counts.stored_total() as usize, mapped.blocks.blocks.len());
        assert_eq!(mapped.placed.len(), mapped.blocks.blocks.len());
        assert!(mapped.blocks.blocks.len() <= MAX_BLOCKS);
        let page_letters: Vec<(char, usize)> = {
            let mut out = Vec::new();
            match_letters(page.iter().copied(), |letter, at| out.push((letter, at)));
            out
        };
        let mut spans = Vec::new();
        for (at, (block, placed)) in mapped.blocks.blocks.iter().zip(&mapped.placed).enumerate() {
            assert!(block.start < block.end && block.end as usize <= page.len() && block.level <= MAX_LEVEL);
            assert!(block.bbox[0] <= block.bbox[2] && block.bbox[1] <= block.bbox[3]);
            let wanted = letters_of(line.candidates[placed.candidate as usize].text.chars());
            assert_eq!(
                letters_of(page[block.start as usize..block.end as usize].iter().copied()),
                wanted,
                "a stored block's letters are its candidate's"
            );
            if placed.step == Step::OnlyOccurrence {
                // Every run of the letters on the page that overlaps no block stored before this one: exactly one, and it's inside this block's range.
                let before = &mapped.blocks.blocks[..at];
                let free: Vec<usize> = (0..page_letters.len().saturating_sub(wanted.len() - 1))
                    .filter(|start| page_letters[*start..*start + wanted.len()].iter().map(|(c, _)| *c).eq(wanted.iter().copied()))
                    .filter(|start| {
                        let (from, to) = (page_letters[*start].1 as u32, page_letters[*start + wanted.len() - 1].1 as u32 + 1);
                        before.iter().all(|b| to <= b.start || b.end <= from)
                    })
                    .collect();
                assert_eq!(free.len(), 1, "the second step takes the one free place");
                assert!(block.start as usize <= page_letters[free[0]].1);
            }
            spans.push((block.start, block.end));
        }
        spans.sort_unstable();
        assert!(spans.windows(2).all(|pair| pair[0].1 <= pair[1].0), "mapped blocks overlap");
        let mut chunk = Vec::new();
        codec::seal_page(seal(), &mapped.blocks, &mut chunk).expect("what is mapped is what a record holds");
        assert_eq!(codec::open_page(seal(), line.page, page.len() as u32, &mut chunk), Ok(mapped.blocks));
    }
}

pub fn seeds() -> Vec<Vec<u8>> {
    let page = "Right column words\nLeft column words.\nAnnual Report\nAnnual Report";
    let lines = [
        r#"{"page":0,"blocks":[{"kind":"para","level":0,"box":[10,20,300,60],"text":"Left column words","continues":false},{"kind":"heading","level":1,"box":[10,700,300,720],"text":"Right column words"},{"kind":"table","box":[0,0,1,1],"text":"x"},{"kind":"code","box":[null,"a",1e999,-1],"text":"Annual Report","continues":true},{"kind":"list_item","level":99,"text":"ﬁ ligature"},5]}"#,
        r#"{"page":2,"blocks":[{"kind":"para","text":"Annual Report"}]}"#,
        r#"{"page":1,"blocks":[]}"#,
        "not json",
    ];
    let mut whole = page.as_bytes().to_vec();
    whole.push(0);
    whole.extend_from_slice(lines.join("\n").as_bytes());
    let mut long = vec![b'a'; 3000];
    long.push(0);
    long.extend_from_slice(br#"{"page":0,"blocks":[{"kind":"para","text":""#);
    long.extend_from_slice(&vec![b'a'; 2000]);
    long.extend_from_slice(b"\"}]}");
    // An answer the session believes whole: three pages, in order, with a block that maps and one that doesn't.
    let mut believed = b"Annual Report\nsecond page text\nthird".to_vec();
    believed.push(0);
    believed.extend_from_slice(
        [
            r#"{"page":0,"blocks":[{"kind":"heading","level":1,"box":[10,700,300,720],"text":"Annual Report","continues":false},{"kind":"figure","box":null,"text":""}]}"#,
            r#"{"page":1,"blocks":[{"kind":"para","level":0,"box":[10,20,300,60],"text":"second page text","continues":true},{"kind":"para","text":"not on the page"}]}"#,
            r#"{"page":2,"blocks":[]}"#,
        ]
        .join("\n")
        .as_bytes(),
    );
    vec![whole, long, believed, vec![0], Vec::new()]
}
