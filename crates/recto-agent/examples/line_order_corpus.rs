//! What line-order repair does to a folder of real PDFs, compared with `pdftotext -layout`. Read-only. Run on a release build:
//!
//! `cargo run --release -p recto-agent --example line_order_corpus -- <target-dir> <folder> [changes.txt]`
//!
//! For every page it checks that no non-space character was gained or lost, and times the repair. For every page that changed, it scores the text before and after by the share of its
//! runs of eight characters, spaces taken out, that `pdftotext -layout` also has. Pairs of words would be simpler, but pdftotext splits `F′` into `F ′`, so a pair of words marks every
//! superscript put right as a mistake. `changes.txt` gets each changed page's scores and the pairs that are new.

use recto_index::{normalize, read_page};
use recto_render_service::{DocRenderer, HeavySlot, Policy, ServiceConfig};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

fn pairs(text: &str) -> Vec<(String, String)> {
    let words = words(text);
    words.windows(2).map(|pair| (pair[0].clone(), pair[1].clone())).collect()
}

fn pdftotext(path: &Path, page: u32, layout: bool) -> String {
    let page = page.to_string();
    let mut command = Command::new("pdftotext");
    if layout {
        command.arg("-layout");
    }
    let output = command.args(["-f", &page, "-l", &page]).arg(path).arg("-").output().expect("pdftotext runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn windows(text: &str) -> Vec<String> {
    let squeezed: Vec<char> = text.chars().filter(|ch| !ch.is_whitespace()).flat_map(char::to_lowercase).collect();
    squeezed.windows(8).map(|window| window.iter().collect()).collect()
}

fn window_share(text: &str, reference: &HashSet<String>) -> f64 {
    let own = windows(text);
    if own.is_empty() {
        return 1.0;
    }
    own.iter().filter(|window| reference.contains(*window)).count() as f64 / own.len() as f64
}

fn share(text_pairs: &[(String, String)], reference: &HashSet<(String, String)>) -> f64 {
    if text_pairs.is_empty() {
        return 1.0;
    }
    text_pairs.iter().filter(|pair| reference.contains(*pair)).count() as f64 / text_pairs.len() as f64
}

fn non_space(text: &str) -> HashMap<char, u32> {
    let mut counts = HashMap::new();
    for ch in text.chars().filter(|ch| !ch.is_whitespace() && *ch != '-') {
        *counts.entry(ch).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = PathBuf::from(&args[1]);
    let mut changes = args.get(3).map(|path| std::fs::File::create(path).expect("the changes file can be made"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&args[2]).expect("the folder can be read").flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "pdf")).collect();
    files.sort();
    let (mut pages, mut changed, mut runs, mut chars_lost, mut failed) = (0u32, 0u32, 0u64, 0u32, 0u32);
    let (mut better, mut same, mut worse) = (0u32, 0u32, 0u32);
    let (mut better_plain, mut worse_plain) = (0u32, 0u32);
    let (mut better_pairs, mut worse_pairs) = (0u32, 0u32);
    let mut repair_ns: Vec<u64> = Vec::new();
    let mut read_page_chars = 0u64;
    let mut most_lines = 0usize;
    for path in &files {
        let config = ServiceConfig { worker_exe: target.join("release/recto-render-service"), pdfium_lib: target.join("pdfium/libpdfium.dylib"), heavy_slot: HeavySlot::new(), extra_env: Vec::new(), replace_args: None };
        let Ok(mut doc) = DocRenderer::new(path, None, config, Policy::default()) else { continue };
        let Ok(count) = doc.page_count() else { continue };
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let (mut file_changed, mut file_pages) = (0u32, 0u32);
        for page in 0..count {
            let Ok(text) = doc.page_characters(page, true, false) else {
                failed += 1;
                continue;
            };
            pages += 1;
            file_pages += 1;
            read_page_chars += text.chars.len() as u64;
            most_lines = most_lines.max(text.chars.iter().filter(|record| record.ch() == '\n').count());
            let started = Instant::now();
            let reading = read_page(&text.chars, &text.boxes, false);
            repair_ns.push(started.elapsed().as_nanos() as u64);
            if reading.runs_moved == 0 {
                continue;
            }
            let before = normalize(&text.chars, false).text;
            let after = reading.normalized.text;
            // `-` aside, because a hyphen at a line's end is a judgement that depends on what follows it.
            if non_space(&before) != non_space(&after) {
                chars_lost += 1;
                eprintln!("CHARACTERS DIFFER: {name} p{}", page + 1);
            }
            changed += 1;
            file_changed += 1;
            runs += u64::from(reading.runs_moved);
            let layout_text = pdftotext(path, page + 1, true);
            let plain_text = pdftotext(path, page + 1, false);
            let layout: HashSet<(String, String)> = pairs(&layout_text).into_iter().collect();
            let (old, new) = (pairs(&before), pairs(&after));
            let (old_pairs, new_pairs) = (share(&old, &layout), share(&new, &layout));
            if new_pairs > old_pairs + 1e-9 {
                better_pairs += 1;
            } else if new_pairs < old_pairs - 1e-9 {
                worse_pairs += 1;
            }
            let layout_windows: HashSet<String> = windows(&layout_text).into_iter().collect();
            let plain_windows: HashSet<String> = windows(&plain_text).into_iter().collect();
            let (old_score, new_score) = (window_share(&before, &layout_windows), window_share(&after, &layout_windows));
            let (old_plain, new_plain) = (window_share(&before, &plain_windows), window_share(&after, &plain_windows));
            if new_score > old_score + 1e-9 {
                better += 1;
            } else if new_score < old_score - 1e-9 {
                worse += 1;
            } else {
                same += 1;
            }
            if new_plain > old_plain + 1e-9 {
                better_plain += 1;
            } else if new_plain < old_plain - 1e-9 {
                worse_plain += 1;
            }
            if let Some(file) = changes.as_mut() {
                let old_set: HashSet<_> = old.iter().cloned().collect();
                let fresh: Vec<String> = new.iter().filter(|pair| !old_set.contains(*pair)).take(40).map(|pair| format!("{}{} {}", if layout.contains(pair) { "" } else { "!" }, pair.0, pair.1)).collect();
                let _ = writeln!(file, "{name} p{} moved {} layout {old_score:.3}->{new_score:.3} plain {old_plain:.3}->{new_plain:.3} | {}", page + 1, reading.runs_moved, fresh.join(" | "));
            }
        }
        if file_changed > 0 {
            println!("{name}: {file_changed} of {file_pages} pages changed");
        }
    }
    repair_ns.sort_unstable();
    let at = |share: f64| repair_ns.get(((repair_ns.len().saturating_sub(1)) as f64 * share) as usize).copied().unwrap_or(0) as f64 / 1000.0;
    println!("{} files, {pages} pages read, {failed} couldn't be read, {read_page_chars} characters, at most {most_lines} line breaks on a page", files.len());
    println!("changed: {changed} pages, {runs} runs moved, {chars_lost} with different characters");
    println!("against pdftotext -layout, by runs of 8 characters: {better} better, {same} same, {worse} worse");
    println!("against pdftotext (reading order), by runs of 8 characters: {better_plain} better, {worse_plain} worse");
    println!("against pdftotext -layout, by pairs of words: {better_pairs} better, {worse_pairs} worse");
    println!("read_page, us: p50 {:.1}, p95 {:.1}, p99 {:.1}, max {:.1}", at(0.5), at(0.95), at(0.99), at(1.0));
}
