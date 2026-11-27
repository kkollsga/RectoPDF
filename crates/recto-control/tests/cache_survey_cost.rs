//! What the caches survey costs, in its own binary because its counting allocator would be every test's in one it shared. `held_bytes_follow_the_entries_not_the_
//! caches` is a test of the normal suite; `costs` is the measurement behind D2d's report, ignored: `cargo test --release -p recto-control --test
//! cache_survey_cost -- --ignored --nocapture --test-threads=1`.
#![allow(unsafe_code)]

use recto_control::removal_store::{CacheTarget, survey_caches};
use recto_core::removal::{FileVersion, LEDGER_LIMITS, Ledger, Limits, Link, Store, Survey, TargetMode, lineage};
use std::alloc::{GlobalAlloc, Layout, System};
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

struct Counting;

/// Live bytes of the whole process, and the most it has been since the last reset. One measuring test at a time (`--test-threads=1` for the measurement).
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged and only adds to atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed) + layout.size() as isize;
        PEAK.fetch_max(now, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn temp() -> recto_testpdf::TempDir {
    recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-cache-cost-")
}

fn key(n: u64) -> String {
    format!("{n:064x}")
}

struct Fixture {
    dir: recto_testpdf::TempDir,
    mine: PathBuf,
}

impl Fixture {
    fn corpus(&self) -> PathBuf {
        self.dir.join("corpus.noindex")
    }

    fn contents(&self) -> PathBuf {
        self.dir.join("contents.noindex")
    }
}

/// `keys` corpus folders (`v4/app` with three files; every fifth also has `v4/agent`), `notes` version notes, `kept` contents files, and `strays` more names in the root.
fn fixture(keys: u64, notes: u64, kept: u64, strays: u64) -> Fixture {
    let dir = temp();
    let (corpus, contents) = (dir.join("corpus.noindex"), dir.join("contents.noindex"));
    std::fs::create_dir_all(corpus.join("by-file")).unwrap();
    std::fs::create_dir_all(&contents).unwrap();
    for n in 1..=keys {
        let format = corpus.join(key(n)).join("v4");
        std::fs::create_dir_all(format.join("app")).unwrap();
        for file in ["text.pack", "ids.bin", "ready.sealed"] {
            std::fs::write(format.join("app").join(file), b"x").unwrap();
        }
        if n % 5 == 0 {
            std::fs::create_dir(format.join("agent")).unwrap();
        }
    }
    for n in 1..=notes {
        std::fs::write(corpus.join("by-file").join(key(1_000_000 + n)), key(n % keys.max(1) + 1)).unwrap();
    }
    for n in 1..=kept {
        std::fs::write(contents.join(format!("{:016x}.{}.json", n, key(n))), b"{}").unwrap();
    }
    for n in 0..strays {
        std::fs::write(corpus.join(format!("stray{n}")), b"x").unwrap();
    }
    let mine = dir.join("mine.pdf");
    std::fs::write(&mine, b"%PDF-1.4").unwrap();
    Fixture { dir, mine }
}

/// One survey of the fixture as the document `mine`, and what it found.
fn survey(f: &Fixture) -> usize {
    let version = FileVersion {
        volume: None,
        inode: 7,
        size: 8,
        mtime_ns: 1,
    };
    let ledger = Ledger {
        links: vec![Link::At { version, path: f.mine.clone() }],
        destinations: Vec::new(),
        path_bytes: 0,
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    };
    let lineage = lineage(&ledger, version, &f.mine, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&Limits {
        max_artifacts: 16_384,
        max_batch: NonZeroU16::new(4).unwrap(),
    });
    let target = CacheTarget {
        key: None,
        device_inode: recto_platform::file_device_and_inode(&f.mine),
    };
    survey_caches(&f.corpus(), &f.contents(), &lineage, &target, TargetMode::Save, &mut survey).unwrap();
    let found = survey.finish().unwrap();
    Store::ALL.into_iter().filter(|s| found.inventory().complete().contains(*s)).count()
}

/// The most live bytes above what was held before, over one run of `work`.
fn peak_of<T>(work: impl FnOnce() -> T) -> (T, isize) {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let out = work();
    (out, PEAK.load(Ordering::Relaxed) - before)
}

/// Failure: the survey keeps what it reads (a name list that is never dropped, a file's content kept, a path per entry), so its memory grows with the cache
/// rather than with its caps, against rule 1. 2,000 names in the root and 2,000 version notes: the most held at any moment stays under 1 MiB (about 0.4 MiB
/// as measured), where keeping each entry's own path and its note's text as well would be several times that.
#[test]
fn held_bytes_follow_the_entries_not_the_caches() {
    let f = fixture(0, 2_000, 0, 2_000);
    let (marked, peak) = peak_of(|| survey(&f));
    println!("peak live bytes over 2,000 names and 2,000 notes: {peak}");
    assert_eq!(marked, 4, "the four stores are complete");
    assert!(peak < 1 << 20, "held {peak} bytes");
}

/// The measurement behind the report: the survey of 1,000 corpora with 3,000 notes and 512 contents files, and of a root of 8,192 names. Median of five runs,
/// and the peak of live bytes.
#[test]
#[ignore = "a measurement, not a test"]
fn costs() {
    let report = |what: &str, f: &Fixture| {
        let mut times = Vec::new();
        let mut peaks = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let (_, peak) = peak_of(|| survey(f));
            times.push(started.elapsed());
            peaks.push(peak);
        }
        times.sort();
        peaks.sort();
        println!(
            "{what}: median {:?}, peak live {} bytes ({:.2} MiB)",
            times[2],
            peaks[2],
            peaks[2] as f64 / 1_048_576.0
        );
    };
    report(
        "1,000 corpora, 200 with an agent's folder, 3,000 notes, 512 contents",
        &fixture(1_000, 3_000, 512, 0),
    );
    report("8,192 names in the root", &fixture(0, 0, 0, 8_192 - 1));
}
