//! Page identity over a folder of PDFs, one line per page: `file page digest-hex|none bytes`. Run before and after a change to `page_id` and compare the
//! output: `cargo run --release -p recto-core --example page_id_corpus -- FOLDER > out.txt`. Files are taken in name order, folders below included.

use recto_core::page_id::{self, Caps};
use std::path::{Path, PathBuf};

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

fn main() {
    let Some(folder) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: page_id_corpus FOLDER");
        std::process::exit(2);
    };
    let mut files = Vec::new();
    pdfs(&folder, &mut files);
    files.sort();
    for file in files {
        let name = file.strip_prefix(&folder).unwrap_or(&file).display().to_string();
        let Some(reader) = recto_platform::file_stamp(&file).ok().and_then(|stamp| page_id::open_version_for_identity(&file, stamp)) else {
            println!("{name} - unopened 0");
            continue;
        };
        let Ok(map) = reader.page_map() else {
            println!("{name} - nomap 0");
            continue;
        };
        for (page, entry) in map.iter().enumerate() {
            let read = page_id::read_page(&reader, entry, &Caps::STANDARD);
            let digest = read.identity.map_or_else(|| "none".to_owned(), |id| id.digest.iter().map(|byte| format!("{byte:02x}")).collect());
            println!("{name} {page} {digest} {}", read.bytes);
        }
    }
}
