//! **Saves every PDF in a folder, as an editor would, and says how each came out.** For the corpus gate on Save: copies each file into a work folder (the
//! corpus is never written), turns page 1, moves the last page to the front, adds page 1 of the next file at the end, saves through
//! `recto_core::save`, and reads the result back: page count, the original bytes as a prefix, and the time and the process's peak footprint. Other
//! readers are the caller's to run on the files left in the work folder.
//!
//! `cargo run --release -p recto-core --example save_corpus -- <corpus folder> <work folder>`

use recto_core::save::{AddedFile, PageRef, SaveRequest, Target};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(corpus), Some(work)) = (args.next(), args.next()) else {
        eprintln!("usage: save_corpus <corpus folder> <work folder>");
        std::process::exit(2);
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")))
        .collect();
    files.sort();
    std::fs::create_dir_all(&work).unwrap();
    let (mut saved, mut refused) = (0, 0);
    for (at, original) in files.iter().enumerate() {
        let name = original.file_name().unwrap();
        let copy = Path::new(&work).join(name);
        std::fs::copy(original, &copy).unwrap();
        let other = Path::new(&work).join(format!("added-{}", name.to_string_lossy()));
        std::fs::copy(&files[(at + 1) % files.len()], &other).unwrap();
        let result = save_one(&copy, &other);
        let _ = std::fs::remove_file(&other);
        match result {
            Ok(line) => {
                saved += 1;
                println!("ok\t{}\t{line}", name.to_string_lossy());
            }
            Err(why) => {
                refused += 1;
                println!("refused\t{}\t{why}", name.to_string_lossy());
            }
        }
    }
    println!("saved {saved}, refused {refused}");
}

fn save_one(copy: &Path, other: &Path) -> Result<String, String> {
    let pages = recto_core::inspect(copy).map_err(|error| format!("doesn't open: {}", error.code()))?.page_count;
    let other_pages = recto_core::inspect(other)
        .map_err(|error| format!("added file doesn't open: {}", error.code()))?
        .page_count;
    // `SAVE_ONLY` measures the save alone: no whole-file reads around it, which would cost the harness twice the file.
    let save_only = std::env::var_os("SAVE_ONLY").is_some();
    let before = if save_only {
        Vec::new()
    } else {
        std::fs::read(copy).map_err(|_| "unreadable")?
    };
    let mut order: Vec<PageRef> = Vec::with_capacity(pages as usize + 1);
    order.push(PageRef {
        source: 0,
        page: pages - 1,
        quarter_turns: 0,
    });
    order.extend((0..pages - 1).map(|page| PageRef {
        source: 0,
        page,
        quarter_turns: u8::from(page == 0),
    }));
    order.push(PageRef {
        source: 1,
        page: 0,
        quarter_turns: 0,
    });
    let _ = other_pages;
    let added = [AddedFile {
        path: other,
        page_count: other_pages,
        stamp: recto_platform::file_stamp(other).ok(),
    }];
    let request = SaveRequest {
        path: copy,
        stamp: recto_platform::file_stamp(copy).ok(),
        file_pages: pages,
        pages: &order,
        added: &added,
        highlights: &[],
    };
    let started = Instant::now();
    let outcome = recto_core::save::save(&request, Target::Same, None, &mut recto_core::save::NoHooks).map_err(|error| error.code().to_string())?;
    let took = started.elapsed();
    let after = if save_only {
        Vec::new()
    } else {
        std::fs::read(copy).map_err(|_| "unreadable after")?
    };
    if !after.starts_with(&before) {
        return Err("ORIGINAL BYTES CHANGED".into());
    }
    let again = recto_core::inspect(copy)
        .map_err(|error| format!("SAVED FILE DOESN'T OPEN: {}", error.code()))?
        .page_count;
    if again != pages + 1 {
        return Err(format!("SAVED FILE HAS {again} PAGES, NOT {}", pages + 1));
    }
    Ok(format!(
        "{pages}+1 pages\t{} -> {} bytes\t{:.1} ms\tmissing {}",
        outcome.previous_len,
        outcome.new_len,
        took.as_secs_f64() * 1000.0,
        outcome.objects_missing
    ))
}
