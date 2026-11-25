//! What one `resolve` costs when it must read every page (a reference whose page is gone): `cargo run --release -p recto-core --example locator_scan -- FILE.pdf...`
//! prints, for each file, its pages, the pages and bytes the resolve read, its status and the wall time of the open and of the resolve. A folder as the
//! only argument names the file with the most pages in it, and the largest by size.

use recto_core::locator::{self, DocumentTags, Locator, PageRef, ScanCaps, Target};
use recto_core::page_id::PageIdentity;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn measure(path: &Path) {
    let stamp = recto_platform::file_stamp(path).expect("a file");
    let opening = Instant::now();
    let Some(reader) = recto_core::page_id::open_version_for_identity(path, stamp) else {
        println!("{}: not opened for identity", path.display());
        return;
    };
    let map = reader.page_map().expect("its pages");
    let opened = opening.elapsed();
    // A reference to page 1 of a made-up page: no page of the file has its digest, so the resolve reads them all.
    let identity = PageIdentity {
        digest: [7; 32],
        object: (1, 0),
        frame: 0,
    };
    let page = PageRef::new(1, &identity).expect("page 1");
    let gone = Locator::new(DocumentTags::new([0; 8], None), page, Target::Page);
    let started = Instant::now();
    let answer = locator::resolve(&gone, &reader, &map, &ScanCaps::STANDARD);
    println!(
        "{}: {} MiB, {} pages, opened {} ms, resolve {} ms, pages_read {}, bytes_read {}, status {:?}, looked {:?}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::fs::metadata(path).map_or(0, |meta| meta.len() >> 20),
        map.len(),
        opened.as_millis(),
        started.elapsed().as_millis(),
        answer.pages_read,
        answer.bytes_read,
        answer.status,
        answer.looked
    );
    // The whole process, as the kernel counts it: what one scan costs in memory at most.
    if let Some(memory) = recto_platform::isolate::process_memory(std::process::id()) {
        println!(
            "  footprint {:.1} MiB now, peak {:.1} MiB",
            memory.footprint_bytes as f64 / 1048576.0,
            memory.peak_footprint_bytes as f64 / 1048576.0
        );
    }
}

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if let [folder] = &args[..]
        && folder.is_dir()
    {
        let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
            .expect("the folder")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "pdf"))
            .collect();
        files.sort();
        let mut most = (0, None);
        for file in &files {
            let stamp = recto_platform::file_stamp(file).expect("a file");
            if let Some(reader) = recto_core::page_id::open_version_for_identity(file, stamp)
                && let Ok(map) = reader.page_map()
                && map.len() > most.0
            {
                most = (map.len(), Some(file.clone()));
            }
        }
        let largest = files.iter().max_by_key(|file| std::fs::metadata(file).map_or(0, |meta| meta.len()));
        for file in [most.1.as_ref(), largest].into_iter().flatten() {
            measure(file);
        }
        return;
    }
    for file in &args {
        measure(file);
    }
}
