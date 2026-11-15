//! The page digest's decoders and `Do` scan (`recto_core::page_id`), on a stream's filters, parameters and bytes as the input says, and a journal's
//! sidecar (`recto_session::edit::identity::sidecar`) as it might be found on disk.
//!
//! What must hold: no panic, bounded time and memory, whatever the bytes; a decode reads no further than asked; the scan keeps no more names than its
//! cap; and a sidecar gives a list no page past its file's end, nor more pages than a list names.

use crate::guard::{Budget, within};
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(20),
    memory: 256 * 1024 * 1024,
};
/// Most decoded bytes read of one input.
const LIMIT: u64 = 8 << 20;

/// A stream: decoded through every general filter and predictor its bytes pick, and scanned.
pub fn check_stream(bytes: &[u8]) {
    within("page_decode", BUDGET, || {
        let (decoded, names) = recto_core::page_id::fuzz_stream(bytes, LIMIT);
        // The reader stops once it passes the limit, by at most a chunk.
        assert!(decoded.is_none_or(|n| n <= LIMIT + (64 << 10)), "read past the limit: {decoded:?}");
        assert!(names <= 2 * 256, "{names} names kept");
    });
}

/// A sidecar's bytes: read, and taken in by a list of twenty pages.
pub fn check_sidecar(bytes: &[u8]) {
    within("sidecar", BUDGET, || {
        use recto_session::edit::identity::{self, sidecar};
        let Some(read) = sidecar::decode(bytes) else { return };
        assert!(read.good <= bytes.len());
        assert!(read.pages.len() <= identity::MAX_IDENTIFIED);
        assert!(read.pages.iter().all(|(page, _)| *page < read.header.file_pages));
        assert!(read.pages.windows(2).all(|pair| pair[0].0 < pair[1].0), "ascending, each page once");
        let mut edits = recto_session::edit::PageEdits::new(20);
        identity::restored(&mut edits, &read.pages, read.document);
        if let Some(ids) = identity::identities(&edits) {
            assert!(ids.pages().len() <= identity::MAX_IDENTIFIED);
            assert!(ids.pages().iter().all(|(page, _)| *page < 20));
        }
        assert!(identity::sidecar_caught_up(&edits), "nothing taken in waits to be written again");
    });
}
