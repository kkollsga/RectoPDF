//! S0 of 8.3 (`docs/design/blocks.md` §7): what distillPDF's page output costs when a `blocks` job runs it, and how many blocks map onto the page's text.
//!
//! ```text
//! cargo build --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target --examples
//! cargo run --release --manifest-path crates/recto-headings/Cargo.toml --target-dir target --example blocks_sample -- [--scan full|typography] [--no-map] FILE
//! ```
//!
//! `Block::text` carries inline markup (`<a href>`, `<b>`), so it goes through the helper's own `plain` first, as heading titles do.
//! One file a run, so the process's peak footprint (`phys_footprint`, from `recto_platform::isolate`, run with `MallocLargeCache=0` as the helper is) is that file's. It scans, then streams page by page, as `find_headings` does, keeping nothing but counts.
//! The mapping pass reads each page's characters through the render worker (`DocRenderer::page_characters`, as the corpus does; the worker's own memory isn't in this process's peak) and checks, for each block with letters, that its letters (the heading matcher's `letters`) are one unbroken run in the page's letters: `strict` as the design says, after the previous mapped block's end, and `anywhere` on the page. Hidden text isn't checked here.
//! The record size is an estimate: 1 byte for kind, level and the continues bit, 3 bytes each for start and end (a page has at most 2^21 characters), 8 bytes for the box in whole points, per mapped block, plus 8 bytes a page for a count and a digest.

use distillpdf::model::BlockKind;
use distillpdf::pages::DocStats;
use distillpdf::{Engine, PdfDocument};
use recto_render_service::{DocRenderer, Policy, ServiceConfig};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

const MAX_COMPARISONS: usize = 20_000_000;
const BYTES_PER_BLOCK: u64 = 1 + 3 + 3 + 8;
const BYTES_PER_PAGE: u64 = 8;

fn letters(text: &str, out: &mut Vec<char>) {
    for c in text.chars() {
        let spelled: &str = match c {
            '\u{FB00}' => "ff",
            '\u{FB01}' => "fi",
            '\u{FB02}' => "fl",
            '\u{FB03}' => "ffi",
            '\u{FB04}' => "ffl",
            '\u{FB05}' | '\u{FB06}' => "st",
            _ => {
                if c.is_alphanumeric() {
                    out.extend(c.to_lowercase());
                }
                continue;
            }
        };
        out.extend(spelled.chars());
    }
}

fn find(hay: &[char], key: &[char], from: usize, budget: &mut usize) -> Option<usize> {
    if key.is_empty() || hay.len() < key.len() || from > hay.len() - key.len() {
        return None;
    }
    for start in from..=hay.len() - key.len() {
        if hay[start] != key[0] {
            continue;
        }
        if *budget < key.len() {
            return None;
        }
        *budget -= key.len();
        if hay[start..start + key.len()] == *key {
            return Some(start);
        }
    }
    None
}

#[derive(Default)]
struct Tally {
    blocks: u64,
    with_letters: u64,
    strict: u64,
    anywhere: u64,
}

fn kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Heading => "heading",
        BlockKind::Para => "para",
        BlockKind::ListItem => "list_item",
        BlockKind::Table => "table",
        BlockKind::Figure => "figure",
        BlockKind::Caption => "caption",
        BlockKind::Footnote => "footnote",
        BlockKind::Code => "code",
        BlockKind::Header => "header",
        BlockKind::DestAnchors => "dest_anchors",
    }
}

fn cpu_seconds() -> f64 {
    let out = Command::new("ps").args(["-o", "cputime=", "-p", &std::process::id().to_string()]).output().ok();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let (min, sec) = text.split_once(':').unwrap_or(("0", "0"));
    min.parse::<f64>().unwrap_or(0.0) * 60.0 + sec.parse::<f64>().unwrap_or(0.0)
}

fn main() {
    let mut scan = "full".to_string();
    let mut map = true;
    let mut file = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scan" => scan = args.next().unwrap(),
            "--no-map" => map = false,
            _ => file = Some(arg),
        }
    }
    let file = file.expect("a file");
    let path = Path::new(&file);
    let started = Instant::now();
    let document = PdfDocument::open_with_engine(&file, Engine::LazyStrict).expect("open");
    let mut on_page = |_: distillpdf::pages::PageProgress| {};
    let stats: DocStats = if scan == "typography" {
        document.scan_typography(None, &mut on_page)
    } else {
        document.scan_document(None, &mut on_page)
    }
    .expect("scan");
    let scanned = started.elapsed().as_secs_f64();
    let mut stream = document.page_stream(stats).expect("stream");
    let pages = stream.page_count();
    let mut renderer = if map {
        match ServiceConfig::discover().and_then(|config| DocRenderer::new(path, None, config, Policy::default())) {
            Ok(renderer) => Some(renderer),
            Err(error) => {
                eprintln!("no render worker: {error:?}");
                None
            }
        }
    } else {
        None
    };
    let mut tally: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut failed_pages = 0u32;
    let mut unmapped_pages = 0u32;
    let mut max_blocks = 0usize;
    let mut max_letters = 0usize;
    let mut mapped_blocks = 0u64;
    let mut map_seconds = 0.0f64;
    let (mut hay, mut key) = (Vec::new(), Vec::new());
    let mut debug: u32 = std::env::var("BLOCKS_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for number in 1..=pages {
        let Ok(page) = stream.page(number) else {
            failed_pages += 1;
            continue;
        };
        max_blocks = max_blocks.max(page.blocks.len());
        let mut page_letters: Option<Vec<char>> = None;
        if let Some(renderer) = renderer.as_mut() {
            let at = Instant::now();
            match renderer.page_characters(number - 1, false, false) {
                Ok(read) => {
                    hay.clear();
                    letters(&read.chars.iter().map(|c| c.ch()).collect::<String>(), &mut hay);
                    page_letters = Some(hay.clone());
                }
                Err(error) => {
                    if unmapped_pages == 0 {
                        eprintln!("page text failed: {error:?}");
                    }
                    unmapped_pages += 1
                }
            }
            map_seconds += at.elapsed().as_secs_f64();
        }
        let (mut cursor, mut budget) = (0usize, MAX_COMPARISONS);
        for block in &page.blocks {
            let entry = tally.entry(kind_name(block.kind)).or_default();
            entry.blocks += 1;
            key.clear();
            letters(&recto_headings::plain(&block.text), &mut key);
            max_letters = max_letters.max(key.len());
            if key.is_empty() {
                continue;
            }
            entry.with_letters += 1;
            let Some(hay) = page_letters.as_ref() else { continue };
            if let Some(start) = find(hay, &key, cursor, &mut budget) {
                entry.strict += 1;
                entry.anywhere += 1;
                cursor = start + key.len();
                mapped_blocks += 1;
            } else if find(hay, &key, 0, &mut budget).is_some() {
                entry.anywhere += 1;
            } else if debug > 0 && block.kind == BlockKind::Para {
                debug -= 1;
                // How far the block's letters go before the page's text stops agreeing: the longest prefix found anywhere.
                let longest = (1..=key.len()).take_while(|n| find(hay, &key[..*n], 0, &mut budget).is_some()).last().unwrap_or(0);
                let cut: String = key[longest.saturating_sub(10)..(longest + 25).min(key.len())].iter().collect();
                println!("MISS page {number} letters {} prefix_found {longest} around: {cut}", key.len());
            }
        }
    }
    let wall = started.elapsed().as_secs_f64();
    let peak = recto_platform::isolate::process_memory(std::process::id()).map(|m| m.peak_footprint_bytes).unwrap_or(0);
    let total: u64 = tally.values().map(|t| t.blocks).sum();
    let name = path.file_name().unwrap().to_string_lossy();
    println!(
        "FILE {name} scan={scan} map={} pages={pages} failed_pages={failed_pages} unmapped_pages={unmapped_pages} blocks={total} max_blocks_page={max_blocks} max_letters_block={max_letters}",
        renderer.is_some()
    );
    println!(
        "TIME wall={wall:.2}s scan={scanned:.2}s stream+map={:.2}s of which text reads={map_seconds:.2}s cpu={:.2}s (this process; the render worker's is not in it) peak_footprint={:.1}MiB",
        wall - scanned,
        cpu_seconds(),
        peak as f64 / 1048576.0
    );
    for (kind, t) in &tally {
        println!("KIND {kind} blocks={} with_letters={} strict={} anywhere={}", t.blocks, t.with_letters, t.strict, t.anywhere);
    }
    let record = mapped_blocks * BYTES_PER_BLOCK + u64::from(pages) * BYTES_PER_PAGE;
    println!(
        "RECORD mapped_blocks={mapped_blocks} bytes={record} per_page={:.0}",
        record as f64 / f64::from(pages.max(1))
    );
}
