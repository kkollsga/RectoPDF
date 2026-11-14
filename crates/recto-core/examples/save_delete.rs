//! **Measures a delete saved by the rewrite:** time, the whole process's peak `phys_footprint`, and the bytes it wrote to disk by the kernel's own counter.
//! Works on a copy in a work folder, so the file given is never written.
//!
//! `cargo run --release -p recto-core --example save_delete -- <file.pdf | --pages N> <work folder> <page to delete, 1-based>`
//!
//! A file of one page, such as the USGS map, gets a page added from a small file first, by an appended save that isn't measured, and that page is what's
//! deleted: the rewrite then copies every byte of the map that stays. `--pages N` makes a document of N pages of text.

use recto_core::save::{AddedFile, PageRef, SaveRequest, Target};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn counters() -> (u64, u64, u64) {
    let pid = std::process::id();
    let written = recto_platform::local::process_counters(pid).map_or(0, |counters| counters.disk_bytes_written);
    let memory = recto_platform::isolate::process_memory(pid);
    (
        written,
        memory.as_ref().map_or(0, |memory| memory.footprint_bytes),
        memory.map_or(0, |memory| memory.peak_footprint_bytes),
    )
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (source, work, delete) = match args.as_slice() {
        [flag, count, work, delete] if flag == "--pages" => (
            Err(count.parse::<usize>().expect("a page count")),
            PathBuf::from(work),
            delete.parse::<u32>().expect("a page"),
        ),
        [file, work, delete] => (Ok(PathBuf::from(file)), PathBuf::from(work), delete.parse::<u32>().expect("a page")),
        _ => {
            eprintln!("usage: save_delete <file.pdf | --pages N> <work folder> <page to delete, 1-based>");
            std::process::exit(2);
        }
    };
    std::fs::create_dir_all(&work).unwrap();
    let copy = work.join("measured.pdf");
    let _ = std::fs::remove_file(&copy);
    match &source {
        Ok(file) => {
            std::fs::copy(file, &copy).unwrap();
        }
        Err(count) => {
            let pages: Vec<recto_testpdf::Page> = (0..*count)
                .map(|at| {
                    recto_testpdf::Page::new(recto_testpdf::text_lines(&[
                        &format!("Page {at} of a long document"),
                        "With a second line of ordinary words on it.",
                    ]))
                })
                .collect();
            std::fs::write(&copy, recto_testpdf::pages(&pages)).unwrap();
        }
    }
    let mut pages = recto_core::inspect(&copy).expect("opens").page_count;
    if pages == 1 {
        // Something to delete beside the one page, added by an append, which isn't what's measured.
        let small = work.join("small.pdf");
        std::fs::write(
            &small,
            recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["A small page to delete"]))]),
        )
        .unwrap();
        let added = [AddedFile {
            path: &small,
            page_count: 1,
            stamp: recto_platform::file_stamp(&small).ok(),
        }];
        let order = [
            PageRef {
                source: 0,
                page: 0,
                quarter_turns: 0,
            },
            PageRef {
                source: 1,
                page: 0,
                quarter_turns: 0,
            },
        ];
        let request = SaveRequest {
            path: &copy,
            stamp: recto_platform::file_stamp(&copy).ok(),
            file_pages: 1,
            pages: &order,
            added: &added,
            highlights: &[],
        };
        recto_core::save::save(&request, Target::Same, None, &mut recto_core::save::NoHooks).expect("the page is added");
        pages = 2;
    }
    assert!(delete >= 1 && delete <= pages, "page {delete} of {pages}");
    let order: Vec<PageRef> = (0..pages)
        .filter(|page| *page != delete - 1)
        .map(|page| PageRef {
            source: 0,
            page,
            quarter_turns: 0,
        })
        .collect();
    let request = SaveRequest {
        path: &copy,
        stamp: recto_platform::file_stamp(&copy).ok(),
        file_pages: pages,
        pages: &order,
        added: &[],
        highlights: &[],
    };
    let (written_before, footprint_before, peak_before) = counters();
    let started = Instant::now();
    let outcome = recto_core::save::save(&request, Target::Same, None, &mut recto_core::save::NoHooks).expect("the delete saves");
    let took = started.elapsed();
    let (written_after, _, peak_after) = counters();
    println!(
        "{} pages, page {delete} deleted by {}: {:.1} MiB -> {:.1} MiB, {:.0} ms; wrote {:.1} MiB to disk; footprint {:.1} MiB before, peak {:.1} MiB (peak before the save {:.1} MiB); objects missing {}",
        pages,
        outcome.writer.name(),
        mib(outcome.previous_len),
        mib(outcome.new_len),
        took.as_secs_f64() * 1000.0,
        mib(written_after.saturating_sub(written_before)),
        mib(footprint_before),
        mib(peak_after),
        mib(peak_before),
        outcome.objects_missing,
    );
    // `KEEP=1` leaves the saved file in the work folder, to compare with the original in other readers.
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_file(&copy);
    }
    let _ = std::fs::remove_file(Path::new(&work).join("small.pdf"));
}
