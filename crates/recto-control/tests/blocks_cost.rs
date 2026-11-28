//! What reading one hostile page line of a blocks answer costs the session in live bytes (the S3 review's ask: an estimate of 6 to 10 MiB for a 512 KiB line of `[],[],…`), in its own binary because its
//! counting allocator would be every test's in one it shared.
#![allow(unsafe_code)]

use recto_control::structure::blocks::{PageStore, read_all};
use recto_index::blocks::PageBlocks;
use recto_index::blocks::map::MAX_LINE_BYTES;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only adds to atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed) + layout.size() as isize;
        PEAK.fetch_max(now, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most characters a page may have in a corpus (2,000,000): more than a block may point into (`MAX_PAGE_CHARS`, 1 Mi), which maps nothing, but the session still holds them as a buffer of 8 MB.
const PAGE_CHARS: usize = 2_000_000;

/// A store with a page of this many characters, that keeps nothing.
struct Text(usize);

impl PageStore for Text {
    fn text(&mut self, _page: u32, chars: &mut Vec<char>) -> bool {
        chars.extend(std::iter::repeat_n('a', self.0));
        true
    }
    fn put(&mut self, _page: &PageBlocks) -> Result<(), &'static str> {
        Ok(())
    }
}

/// A one-page answer whose page line is as long as a line may be, of `filler` repeated: a hello, the line, the end.
fn said(filler: &str, blocks: usize) -> Vec<u8> {
    let mut body = String::from(r#"{"page":0,"blocks":["#);
    while body.len() + filler.len() < MAX_LINE_BYTES - 4 {
        body.push_str(filler);
    }
    body.push_str("]}");
    format!("{{\"state\":\"hello\",\"walled\":true}}\n{body}\n{{\"state\":\"ready\",\"pages\":1,\"pages_failed\":0,\"blocks\":{blocks},\"cut\":0}}\n")
        .into_bytes()
}

/// The most the session holds over what it held before, reading `said` against a page of `chars` characters.
fn peak_over(said: &[u8], chars: usize) -> isize {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let _ = read_all(said, 1, Text(chars));
    PEAK.load(Ordering::Relaxed) - before
}

/// Failure: one hostile line (a page line of 512 KiB of empty blocks, or of the shortest blocks that map) takes more of the session's memory than a job may: the whole line parsed into a tree, the page's
/// characters held twice, or a buffer that grows with the line. The peak over the session's own is held under 32 MiB (measured: see `docs/engineering/blocks.md`).
#[test]
fn a_hostile_page_line_costs_the_session_little() {
    for (name, filler) in [("empty blocks", "[],"), ("short objects", r#"{"kind":"para","text":"a"},"#)] {
        let peak = peak_over(&said(filler, 0), PAGE_CHARS);
        println!("BLOCKS_COST {name}: {:.1} MiB over", peak as f64 / 1048576.0);
        assert!(peak < 32 * 1024 * 1024, "{name}: {peak} bytes over");
    }
}
