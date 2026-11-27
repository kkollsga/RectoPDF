//! Hostile lines from the block helper, and a hostile page to map them onto. The input is a page's text, a zero byte, then lines of JSON, one candidate each.
//!
//! What must hold: no panic or hang; every candidate is stored or counted as dropped; what's stored is inside the page, in whole points and in order, overlaps nothing, is at most 512 blocks and
//! seals into a record that opens as the same blocks.

use recto_index::blocks::map::{Candidate, MAX_LINE_BYTES, map_page};
use recto_index::blocks::{MAX_BLOCKS, MAX_LEVEL, codec};
use recto_index::corpus::CorpusKey;
use recto_index::seal::CorpusSeal;
use recto_platform::find_key::FindKey;
use std::sync::OnceLock;

/// Most characters of page the target reads: a page is mapped, not stored, so a small one finds what a big one would.
const MAX_PAGE: usize = 4096;
const MAX_LINES: usize = 600;

fn seal() -> &'static CorpusSeal {
    static SEAL: OnceLock<CorpusSeal> = OnceLock::new();
    SEAL.get_or_init(|| {
        let key = FindKey::in_memory().expect("a key");
        CorpusSeal::create(&key, CorpusKey::from_hex(&"b8".repeat(32)).expect("a corpus key"))
            .expect("a seal")
            .0
    })
}

pub fn check(data: &[u8]) {
    let (page, lines) = data.split_at(data.iter().position(|byte| *byte == 0).unwrap_or(data.len()));
    let page: Vec<char> = String::from_utf8_lossy(page).chars().take(MAX_PAGE).collect();
    let candidates: Vec<Candidate> = lines.split(|byte| *byte == b'\n').take(MAX_LINES).filter_map(Candidate::from_line).collect();
    let mapped = map_page(2, &page, &candidates);
    assert_eq!(
        (mapped.counts.stored_total() + mapped.counts.dropped_total()) as usize,
        candidates.len(),
        "every candidate is stored or counted"
    );
    assert_eq!(mapped.counts.stored_total() as usize, mapped.blocks.blocks.len());
    assert!(mapped.blocks.blocks.len() <= MAX_BLOCKS);
    let mut spans = Vec::new();
    for block in &mapped.blocks.blocks {
        assert!(block.start < block.end && block.end as usize <= page.len() && block.level <= MAX_LEVEL);
        assert!(block.bbox[0] <= block.bbox[2] && block.bbox[1] <= block.bbox[3]);
        spans.push((block.start, block.end));
    }
    spans.sort_unstable();
    assert!(spans.windows(2).all(|pair| pair[0].1 <= pair[1].0), "mapped blocks overlap");
    let mut chunk = Vec::new();
    codec::seal_page(seal(), &mapped.blocks, &mut chunk).expect("what is mapped is what a record holds");
    assert_eq!(codec::open_page(seal(), 2, &mut chunk), Ok(mapped.blocks));
}

pub fn seeds() -> Vec<Vec<u8>> {
    let page = "Right column words\nLeft column words\nAnnual Report\nAnnual Report";
    let lines = [
        r#"{"kind":"para","level":0,"box":[10,20,300,60],"text":"Left column words","continues":false}"#,
        r#"{"kind":"heading","level":1,"box":[10,700,300,720],"text":"Right column words"}"#,
        r#"{"kind":"table","box":[0,0,1,1],"text":"x"}"#,
        r#"{"kind":"code","box":[null,"a",1e999,-1],"text":"Annual Report","continues":true}"#,
        r#"{"kind":"list_item","level":99,"text":"ﬁ ligature"}"#,
        "not json",
    ];
    let mut whole = page.as_bytes().to_vec();
    whole.push(0);
    whole.extend_from_slice(lines.join("\n").as_bytes());
    let mut long = vec![b'a'; 3000];
    long.push(0);
    long.extend_from_slice(br#"{"kind":"para","text":""#);
    long.extend_from_slice(&vec![b'a'; 2000]);
    long.extend_from_slice(b"\"}");
    vec![whole, long, vec![0], Vec::new(), {
        let mut over = vec![0u8];
        over.extend(std::iter::repeat_n(b' ', MAX_LINE_BYTES.min(8)));
        over
    }]
}
