//! Selecting over a folder of real PDFs, for numbers and for what no made page shows: every page's text and boxes built as the reader builds them, with what must always hold checked. `cargo run --release -p recto-agent --example select_corpus -- <target-dir> FOLDER [PAGES_A_FILE]`, where the worker's built in `<target-dir>`.
//!
//! What it checks on every page: a selection of the whole page copies what `recto text` reads, less what's hidden; a click anywhere lands inside the text; every rectangle is on the page; a word and the line it was clicked on overlap. What it reports: how long a page takes to build, a hit and a page's rectangles, and what a page holds.

use recto_index::read_page;
use recto_index::select::{PAGE_END, PageText, Unit};
use recto_render_service::{DocRenderer, Policy, ServiceConfig};
use std::path::PathBuf;
use std::time::Instant;

fn percentile(values: &mut [u64], share: f64) -> u64 {
    values.sort_unstable();
    values
        .get(((values.len().saturating_sub(1)) as f64 * share).round() as usize)
        .copied()
        .unwrap_or(0)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(args.next().expect("the folder the worker's built in"));
    let folder = PathBuf::from(args.next().expect("a folder of PDFs"));
    let cap: u32 = args.next().and_then(|word| word.parse().ok()).unwrap_or(u32::MAX);
    let config = ServiceConfig::for_worker(target.join("recto-render-service")).expect("the worker");
    let (mut build, mut hit, mut rects, mut bytes, mut chars, mut lines_on_page) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut pages, mut files, mut failed, mut with_text, mut differing, mut hidden_pages, mut outside) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut names: Vec<PathBuf> = std::fs::read_dir(&folder)
        .expect("the folder")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
        .collect();
    names.sort();
    for path in names {
        let Ok(mut doc) = DocRenderer::new(&path, None, config.clone(), Policy::default()) else {
            continue;
        };
        files += 1;
        let Ok(count) = doc.page_count() else { continue };
        let count = count.min(cap);
        for page in 0..count {
            let Ok(characters) = doc.page_characters(page, true, false) else {
                failed += 1;
                continue;
            };
            pages += 1;
            let started = Instant::now();
            let text = PageText::build(&characters.chars, &characters.boxes, characters.frame, false, characters.copy_restricted);
            build.push(started.elapsed().as_micros() as u64);
            if text.is_empty() {
                continue;
            }
            with_text += 1;
            bytes.push(text.bytes() as u64);
            chars.push(u64::from(text.len()));
            let read = read_page(&characters.chars, &characters.boxes, false);
            // With no check run, nothing's flagged, so what's left out is what's off the page.
            let whole = text.text_of(0, PAGE_END);
            if whole != read.normalized.text {
                if whole.chars().count() < read.normalized.text.chars().count() {
                    hidden_pages += 1;
                } else {
                    differing += 1;
                    eprintln!(
                        "{} p{}: {} characters, against {} read",
                        path.display(),
                        page + 1,
                        whole.chars().count(),
                        read.normalized.text.chars().count()
                    );
                }
            }
            let (width, height) = text.size();
            let started = Instant::now();
            let shapes = text.rects(0, PAGE_END);
            rects.push(started.elapsed().as_micros() as u64);
            lines_on_page.push(shapes.len() as u64);
            for shape in &shapes {
                if shape.x < -0.01 || shape.y < -0.01 || shape.x + shape.width > width + 0.01 || shape.y + shape.height > height + 0.01 {
                    outside += 1;
                }
            }
            // Clicks over the whole page, in a grid.
            for row in 0..8 {
                for column in 0..8 {
                    let (x, y) = (width * (column as f32 + 0.5) / 8.0, height * (row as f32 + 0.5) / 8.0);
                    let started = Instant::now();
                    let found = text.unit_at(x, y, Unit::Word);
                    hit.push(started.elapsed().as_nanos() as u64);
                    if let Some((start, end)) = found {
                        assert!(start <= end && end <= text.len(), "{} p{}", path.display(), page + 1);
                        let (line_start, line_end) = text.unit_at(x, y, Unit::Line).expect("a word's there, so a line is");
                        // A word joined over a line end reaches past the line it was clicked on, and always onto it.
                        assert!(
                            start <= line_end && end >= line_start,
                            "{} p{}: word {start} to {end}, line {line_start} to {line_end}",
                            path.display(),
                            page + 1
                        );
                    }
                }
            }
        }
    }
    println!("{files} files, {pages} pages, {with_text} with text, {failed} failed to read");
    println!("whole-page text differing from `recto text`, other than leaving text out: {differing}. Pages leaving text out as off the page: {hidden_pages}");
    println!("rectangles outside their page: {outside}");
    println!(
        "build a page, us: p50 {} p99 {} worst {}",
        percentile(&mut build, 0.5),
        percentile(&mut build, 0.99),
        percentile(&mut build, 1.0)
    );
    println!(
        "a click, ns: p50 {} p99 {} worst {}",
        percentile(&mut hit, 0.5),
        percentile(&mut hit, 0.99),
        percentile(&mut hit, 1.0)
    );
    println!(
        "all a page's rectangles, us: p50 {} p99 {} worst {}",
        percentile(&mut rects, 0.5),
        percentile(&mut rects, 0.99),
        percentile(&mut rects, 1.0)
    );
    println!(
        "what a page holds, KiB: p50 {} p99 {} worst {}",
        percentile(&mut bytes, 0.5) / 1024,
        percentile(&mut bytes, 0.99) / 1024,
        percentile(&mut bytes, 1.0) / 1024
    );
    println!(
        "characters on a page: p50 {} p99 {} worst {}",
        percentile(&mut chars, 0.5),
        percentile(&mut chars, 0.99),
        percentile(&mut chars, 1.0)
    );
    println!(
        "rectangles on a page: p50 {} p99 {} worst {}",
        percentile(&mut lines_on_page, 0.5),
        percentile(&mut lines_on_page, 0.99),
        percentile(&mut lines_on_page, 1.0)
    );
}
