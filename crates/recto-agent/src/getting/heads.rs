//! **Running heads left out of `get`, the glue** (`recto_index::blocks::unit::heads` decides what is kept). A stored page's chrome is decided from its own stored edge pieces and its neighbours', by
//! `chrome::chrome_spans` through `StoredChrome`; a page read live takes the spans `text` marked it with. A page whose edges aren't stored is unknown, and is given whole and counted.

use recto_index::chrome::EdgeLine;
use recto_index::chrome::stored::StoredChrome;
use recto_index::corpus::ReadyCorpus;
use std::sync::Arc;

/// A corpus's pages' edges, asked for in the order pages are walked.
pub(super) type Ring = StoredChrome<Box<dyn FnMut(u32) -> Option<Vec<EdgeLine>>>>;

/// A page's edge buffer is given back past this many bytes.
const BIG_BUFFER: usize = 1 << 20;

/// The chrome window over a corpus: holds at most five pages' edges, and one page's bytes at a time in a buffer of its own, overwritten after each.
pub(super) fn ring(corpus: &Arc<ReadyCorpus>) -> Ring {
    let held = Arc::clone(corpus);
    let mut buffer: Vec<u8> = Vec::new();
    StoredChrome::new(
        corpus.pages(),
        Box::new(move |page| {
            let edges = held.page(page, &mut buffer).ok().and_then(|stored| stored.edges());
            zeroize::Zeroize::zeroize(&mut buffer);
            if buffer.capacity() > BIG_BUFFER {
                buffer = Vec::new();
            }
            edges
        }),
    )
}

#[cfg(test)]
mod tests;
