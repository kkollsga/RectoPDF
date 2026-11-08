//! What a copy of body text leaves out as chrome over a folder of real PDFs, for a person to look at: every line it would leave out, with its file, page and why. `cargo run --release -p recto-agent --example chrome_corpus -- <target-dir> FOLDER > dropped.tsv`, where the worker's built in `<target-dir>`. A line out is `file<TAB>page<TAB>top|bottom<TAB>number|repeat<TAB>text`, and the totals go to stderr.

use recto_index::select::{PageText, chrome_for};
use recto_render_service::{DocRenderer, Policy, ServiceConfig};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(args.next().expect("the folder the worker's built in"));
    let folder = PathBuf::from(args.next().expect("a folder of PDFs"));
    let config = ServiceConfig::for_worker(target.join("recto-render-service")).expect("the worker");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&folder).expect("the folder").flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "pdf")).collect();
    names.sort();
    let (mut files, mut pages_seen, mut pages_with, mut lines_out, mut numbers, mut chars_out, mut chars_all) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut micros: Vec<u64> = Vec::new();
    for path in names {
        let Ok(mut doc) = DocRenderer::new(&path, None, config.clone(), Policy::default()) else { continue };
        let Ok(count) = doc.page_count() else { continue };
        files += 1;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let texts: Vec<Option<PageText>> = (0..count).map(|page| doc.page_characters(page, true, false).ok().map(|page| PageText::build(&page.chars, &page.boxes, page.frame, false, page.copy_restricted))).collect();
        for page in 0..count {
            let Some(text) = &texts[page as usize] else { continue };
            pages_seen += 1;
            chars_all += u64::from(text.len());
            let started = Instant::now();
            let spans = chrome_for(page, text, count, |other| texts[other as usize].as_ref().map(PageText::edge_lines));
            micros.push(started.elapsed().as_micros() as u64);
            if !spans.is_empty() {
                pages_with += 1;
            }
            for (start, end) in spans {
                let line = text.edge_lines().iter().find(|line| line.start == start).copied();
                let words = text.text_of(start, end).replace(['\t', '\n'], " ");
                lines_out += 1;
                chars_out += words.chars().filter(|ch| !ch.is_whitespace()).count() as u64;
                let number = line.is_some_and(|line| line.number.is_some());
                numbers += u64::from(number);
                println!("{name}\t{}\t{}\t{}\t{:.2}\t{}", page + 1, if line.is_some_and(|line| line.top) { "top" } else { "bottom" }, if number { "number" } else { "repeat" }, line.map_or(0.0, |line| line.relative), words.trim());
            }
        }
    }
    micros.sort_unstable();
    let at = |share: f64| micros.get(((micros.len().saturating_sub(1)) as f64 * share) as usize).copied().unwrap_or(0);
    eprintln!("{files} files, {pages_seen} pages with text read, {pages_with} with chrome, {lines_out} lines left out ({numbers} page numbers), {chars_out} of {chars_all} characters; deciding a page p50 {} us, p99 {} us, most {} us", at(0.5), at(0.99), at(1.0));
}
