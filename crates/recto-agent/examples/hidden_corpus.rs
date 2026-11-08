//! Which pages of a folder of real PDFs the hidden-text check flags, and what it marks on each, for review by eye. Read-only. Run on a release build:
//!
//! `cargo run --release -p recto-agent --example hidden_corpus -- <target-dir> <folder>`
//!
//! Prints a line per flagged page: file, page, the characters with no ink, the characters on the page, and the first hidden runs, which are document text, so this is for looking at, not for acting on.

use recto_index::hidden_report;
use recto_render_service::{DocRenderer, HeavySlot, Policy, ServiceConfig};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = PathBuf::from(&args[1]);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&args[2])
        .expect("the folder can be read")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
        .collect();
    files.sort();
    let (mut pages, mut flagged, mut no_ink, mut unchecked) = (0u32, 0u32, 0u64, 0u32);
    for path in &files {
        let config = ServiceConfig {
            worker_exe: target.join("release/recto-render-service"),
            pdfium_lib: target.join("pdfium/libpdfium.dylib"),
            heavy_slot: HeavySlot::new(),
            extra_env: Vec::new(),
            replace_args: None,
        };
        let Ok(mut doc) = DocRenderer::new(path, None, config, Policy::default()) else {
            continue;
        };
        let Ok(count) = doc.page_count() else { continue };
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        for page in 0..count {
            let Ok(text) = doc.page_characters(page, true, true) else { continue };
            pages += 1;
            unchecked += u32::from(!text.hidden_checked);
            let report = hidden_report(&text.chars, text.hidden_checked);
            no_ink += u64::from(report.chars);
            if report.flagged {
                flagged += 1;
                let runs: Vec<String> = report.runs.iter().map(|run| run.chars().take(40).collect()).collect();
                println!("{name}\t{}\t{}\t{}\t{}", page + 1, report.chars, text.chars.len(), runs.join(" | "));
            }
        }
    }
    println!(
        "# {} files, {pages} pages, {flagged} flagged, {no_ink} characters with no ink, {unchecked} not checked",
        files.len()
    );
}
