//! Page identity over a folder of PDFs, or one PDF: one line per page on stdout, `file TAB page TAB digest-hex|none TAB bytes` (tabs, as a file's name may
//! hold spaces). Run before and after a change to `page_id` and compare the output:
//! `cargo run --release -p recto-core --example page_id_corpus -- FOLDER > out.txt`. Files are taken in name order, folders below included.
//!
//! What each file cost goes to stderr, one line per file: `file TAB elapsed-ms TAB peak-MiB`, the peak being the process's own highest physical footprint
//! so far (what Activity Monitor shows). Run on one file, that is the file's own: `page_id_corpus FILE > /dev/null`. A machine's load changes these, so
//! they are read as an order of size and never compared to the byte; stdout is the part to compare.

use recto_core::page_id::{self, Caps};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn pdfs(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            pdfs(&path, found);
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")) {
            found.push(path);
        }
    }
}

/// The process's highest physical footprint so far, in MiB.
fn peak_mib() -> f64 {
    recto_platform::isolate::process_memory(std::process::id()).map_or(0.0, |memory| memory.peak_footprint_bytes as f64 / 1_048_576.0)
}

/// One file's lines on stdout.
fn identify(file: &Path, name: &str) {
    let Some(reader) = recto_platform::file_stamp(file)
        .ok()
        .and_then(|stamp| page_id::open_version_for_identity(file, stamp))
    else {
        println!("{name}\t-\tunopened\t0");
        return;
    };
    let Ok(map) = reader.page_map() else {
        println!("{name}\t-\tnomap\t0");
        return;
    };
    for (page, entry) in map.iter().enumerate() {
        let read = page_id::read_page(&reader, entry, &Caps::STANDARD);
        let digest = read
            .identity
            .map_or_else(|| "none".to_owned(), |id| id.digest.iter().map(|byte| format!("{byte:02x}")).collect());
        println!("{name}\t{page}\t{digest}\t{}", read.bytes);
    }
}

fn main() {
    let Some(folder) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: page_id_corpus FOLDER|FILE");
        std::process::exit(2);
    };
    let mut files = Vec::new();
    if folder.is_file() {
        files.push(folder.clone());
    } else {
        pdfs(&folder, &mut files);
    }
    files.sort();
    for file in files {
        let name = if folder.is_file() {
            file.file_name().unwrap_or_default().to_string_lossy().into_owned()
        } else {
            file.strip_prefix(&folder).unwrap_or(&file).display().to_string()
        };
        let started = Instant::now();
        identify(&file, &name);
        eprintln!("{name}\t{:.1}\t{:.1}", started.elapsed().as_secs_f64() * 1000.0, peak_mib());
    }
}
