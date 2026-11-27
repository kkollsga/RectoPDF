//! Hostile stored blocks: a page's record as the corpus's files may hold it. Half the inputs are a chunk as it lies on disk (sealed or not, damaged or not),
//! half are plain bytes sealed under the right key and purpose, so the reading past the seal meets hostile counts, kinds, flags and ranges too.
//!
//! What must hold: no panic; a record that opens holds what the build could have written (at most 512 blocks, levels in range, no empty or overlapping block, the page it
//! was opened for) and writes back as exactly the record it was read from.

use recto_index::blocks::{MAX_BLOCKS, MAX_LEVEL, MAX_PAGE_CHARS, PageBlocks, codec};
use recto_index::corpus::CorpusKey;
use recto_index::seal::{CorpusSeal, Purpose};
use recto_platform::find_key::FindKey;
use std::sync::OnceLock;

const PAGE: u32 = 5;

fn seal() -> &'static CorpusSeal {
    static SEAL: OnceLock<CorpusSeal> = OnceLock::new();
    SEAL.get_or_init(|| {
        let key = FindKey::in_memory().expect("a key");
        CorpusSeal::create(&key, CorpusKey::from_hex(&"b7".repeat(32)).expect("a corpus key"))
            .expect("a seal")
            .0
    })
}

fn holds(page: &PageBlocks) {
    assert!(page.page == PAGE && page.blocks.len() <= MAX_BLOCKS);
    let mut spans: Vec<(u32, u32)> = page.blocks.iter().map(|b| (b.start, b.end)).collect();
    for block in &page.blocks {
        assert!(block.level <= MAX_LEVEL && block.start < block.end && block.end <= MAX_PAGE_CHARS && block.page == PAGE);
    }
    spans.sort_unstable();
    assert!(spans.windows(2).all(|pair| pair[0].1 <= pair[1].0), "blocks overlap");
}

pub fn check(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else { return };
    let mut chunk = if mode & 1 == 0 {
        rest.to_vec()
    } else {
        let mut out = Vec::new();
        if seal().seal(Purpose::Blocks(PAGE), &mut out, |out| out.extend_from_slice(rest)).is_err() {
            return;
        }
        out
    };
    let before = chunk.clone();
    if let Ok(page) = codec::open_page(seal(), PAGE, &mut chunk) {
        holds(&page);
        let mut again = Vec::new();
        codec::seal_page(seal(), &page, &mut again).expect("what opened writes back");
        assert_eq!(codec::record_len(page.blocks.len()), again.len());
        assert_eq!(again.len(), before.len(), "a record that opens is exactly as long as it was written");
        let reopened = codec::open_page(seal(), PAGE, &mut again).expect("what was written opens");
        assert_eq!(reopened, page);
    }
}

/// A made record's plain bytes, for the seeds: `blocks` blocks of the given kind.
pub fn plain(kinds: &[u8]) -> Vec<u8> {
    let mut plain = vec![codec::FORMAT];
    plain.extend_from_slice(&PAGE.to_le_bytes());
    plain.extend_from_slice(&(kinds.len() as u16).to_le_bytes());
    for (at, kind) in kinds.iter().enumerate() {
        let mut block = [0u8; 19];
        block[0] = *kind;
        block[1] = 1;
        block[3..7].copy_from_slice(&(at as u32 * 10).to_le_bytes());
        block[7..11].copy_from_slice(&(at as u32 * 10 + 8).to_le_bytes());
        plain.extend_from_slice(&block);
    }
    plain
}

pub fn seeds() -> Vec<Vec<u8>> {
    let mut sealed = Vec::new();
    seal()
        .seal(Purpose::Blocks(PAGE), &mut sealed, |out| out.extend_from_slice(&plain(&[1, 2, 3, 5])))
        .expect("seals");
    let with = |mode: u8, bytes: &[u8]| {
        let mut seed = vec![mode];
        seed.extend_from_slice(bytes);
        seed
    };
    vec![
        with(1, &plain(&[1, 2, 3, 4, 5, 6])),
        with(1, &plain(&[])),
        with(1, &plain(&[9])),
        with(1, &plain(&[1, 1])[..30]),
        with(0, &sealed),
        with(0, &sealed[..sealed.len() - 1]),
    ]
}
