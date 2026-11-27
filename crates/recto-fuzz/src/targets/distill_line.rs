//! Hostile page lines from the block helper, and a hostile page to map them onto. The input is a page's text, a zero byte, then lines of JSON, one page each
//! (`{"page":N,"blocks":[{"kind","level","box","text","continues"}]}`).
//!
//! What must hold: no panic or hang; every block is stored or counted as dropped (the unreadable too); what's stored is inside the page, in whole points and in order, overlaps nothing, is at
//! most 512 blocks, and seals into a record that opens as the same blocks. And the rule itself, checked from the page and the candidate again: each stored block's letters are its candidate's
//! letters, exactly, and a block the rule's second step placed was the one place on the page that overlapped no block stored before it.

use recto_core::match_letters;
use recto_index::blocks::map::{PageLines, Step, map_line};
use recto_index::blocks::{MAX_BLOCKS, MAX_LEVEL, codec};
use recto_index::corpus::CorpusKey;
use recto_index::seal::CorpusSeal;
use recto_platform::find_key::FindKey;
use std::sync::OnceLock;

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

pub fn check(data: &[u8]) {
    let (page, lines) = data.split_at(data.iter().position(|byte| *byte == 0).unwrap_or(data.len()));
    let page: Vec<char> = String::from_utf8_lossy(page).chars().take(MAX_PAGE).collect();
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
    vec![whole, long, vec![0], Vec::new()]
}
