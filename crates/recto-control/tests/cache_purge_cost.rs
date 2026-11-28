//! What the clear-all survey and the purge of the four cache stores cost, in their own binary because the counting allocator would be every test's in one it shared.
//! `purge_holds_one_tree_at_a_time` is a test of the normal suite; `costs` is the measurement behind D2e's report, ignored: `cargo test --release -p recto-control
//! --test cache_purge_cost -- --ignored --nocapture --test-threads=1`.
#![allow(unsafe_code)]

use recto_control::removal_store::{CacheRoots, CacheTarget, CachesQuiet, Pins, VolumeNow, purge_caches, survey_caches, survey_caches_to_clear};
use recto_core::removal::{
    ArtifactId, DESIGN_LIMITS, Effect, FileVersion, Kinds, LEDGER_LIMITS, Ledger, Limits, Link, Pin, PinKind, Request, Store, Survey, Surveyed, TargetMode,
    lineage, plan,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::num::NonZeroU16;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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

/// `keys` corpus folders as the app makes them (`v4/app` with its files and lock; every fifth has an agent folder of `agent_pages` pages), `notes` version notes and
/// `kept` contents files; `files` more files in each app folder, to make a tree of a size.
fn fixture(keys: u64, agent_pages: u64, files: u64, notes: u64, kept: u64) -> Fixture {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-purge-cost-");
    recto_platform::local::ensure_private_dir(&dir).unwrap();
    let (corpus, contents) = (dir.join("corpus.noindex"), dir.join("contents.noindex"));
    std::fs::create_dir_all(corpus.join("by-file")).unwrap();
    std::fs::create_dir_all(&contents).unwrap();
    for n in 1..=keys {
        let format = corpus.join(key(n)).join("v4");
        std::fs::create_dir_all(format.join("app")).unwrap();
        for file in ["text.pack", "pages.bin", "ready.sealed", "build.lock", "used"] {
            std::fs::write(format.join("app").join(file), b"x").unwrap();
        }
        for file in 0..files {
            std::fs::write(format.join("app").join(format!("extra{file}")), b"x").unwrap();
        }
        if n % 5 == 0 {
            let pages = format.join("agent").join("pages");
            std::fs::create_dir_all(&pages).unwrap();
            std::fs::write(format.join("agent").join("manifest.json"), b"{}").unwrap();
            for page in 0..agent_pages {
                std::fs::write(pages.join(format!("{page:04}.md")), b"x").unwrap();
            }
        }
    }
    for n in 1..=notes {
        std::fs::write(corpus.join("by-file").join(key(1_000_000 + n)), key(n % keys.max(1) + 1)).unwrap();
    }
    for n in 1..=kept {
        std::fs::write(contents.join(format!("{:016x}.{}.json", n, key(n))), b"{}").unwrap();
    }
    let mine = dir.join("mine.pdf");
    std::fs::write(&mine, b"%PDF-1.4").unwrap();
    Fixture { dir, mine }
}

fn lineage_of(f: &Fixture) -> (Ledger, FileVersion) {
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
    (ledger, version)
}

fn limits() -> Limits {
    Limits {
        max_artifacts: 16_384,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

/// The clear-all survey of the fixture with every other store marked complete, and its pins.
fn survey_to_clear(f: &Fixture) -> (Surveyed, Pins) {
    let (ledger, version) = lineage_of(f);
    let lineage = lineage(&ledger, version, &f.mine, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&Limits {
        max_artifacts: DESIGN_LIMITS.max_artifacts,
        ..limits()
    });
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        if ![Store::CorpusText, Store::CorpusAgentPages, Store::CorpusAliases, Store::GeneratedContents].contains(&store) {
            survey.mark_complete(store);
        }
    }
    survey.history(&lineage);
    let mut pins = Pins::default();
    survey_caches_to_clear(&f.corpus(), &f.contents(), &mut survey, &mut pins).unwrap();
    (survey.finish().unwrap(), pins)
}

/// D2d's per-document survey of the same tree.
fn survey_per_document(f: &Fixture) -> usize {
    let (ledger, version) = lineage_of(f);
    let lineage = lineage(&ledger, version, &f.mine, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits());
    let target = CacheTarget {
        key: None,
        device_inode: recto_platform::file_device_and_inode(&f.mine),
    };
    survey_caches(&f.corpus(), &f.contents(), &lineage, &target, TargetMode::Save, &mut survey).unwrap();
    survey.finish().unwrap().inventory().artifacts().len()
}

/// Every cache step of the plan, in its order, with the pins as the barrier would record them.
fn steps(s: &Surveyed, pins: &Pins) -> Vec<(Store, Vec<(recto_core::removal::ArtifactKey, Pin)>)> {
    let p = plan(
        &Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), TargetMode::Save, false),
        &s.inventory(),
        &DESIGN_LIMITS,
    )
    .expect("a plan");
    let wanted = |ids: &[ArtifactId]| {
        ids.iter()
            .map(|id| {
                let (_, key) = s.key(*id).unwrap();
                let pinned = pins.get(&key).expect("pinned");
                let kind = if pinned.kind == recto_platform::folder::Kind::Folder {
                    PinKind::Folder
                } else {
                    PinKind::File
                };
                (
                    key,
                    Pin {
                        root: 0,
                        inode: pinned.ino,
                        kind,
                    },
                )
            })
            .collect()
    };
    p.steps()
        .iter()
        .filter_map(|step| match step.effect() {
            Effect::Purge(store) | Effect::Rewrite(store) => Some((store, wanted(p.artifacts(step)))),
            _ => None,
        })
        .filter(|(store, _)| {
            matches!(
                store,
                Store::CorpusText | Store::CorpusAgentPages | Store::CorpusAliases | Store::GeneratedContents
            )
        })
        .collect()
}

/// Purges every step. Returns the number of steps.
fn purge_all(f: &Fixture, steps: &[(Store, Vec<(recto_core::removal::ArtifactKey, Pin)>)]) -> usize {
    purge_steps(f, steps).len()
}

/// Purges each step in order and gives what each did.
fn purge_steps(f: &Fixture, steps: &[(Store, Vec<(recto_core::removal::ArtifactKey, Pin)>)]) -> Vec<recto_control::removal_store::Purged> {
    let roots_corpus = f.corpus();
    let roots_contents = f.contents();
    let roots = CacheRoots {
        corpus: &roots_corpus,
        contents: &roots_contents,
    };
    let volume = VolumeNow {
        root: 0,
        dev: std::fs::metadata(f.dir.path()).unwrap().dev(),
    };
    let quiet = CachesQuiet::assumed_for_tests();
    steps
        .iter()
        .map(|(store, wanted)| {
            let result = purge_caches(*store, wanted, &roots, volume, &quiet);
            assert!(result.is_ok(), "{store:?}: {result:?}");
            result.unwrap()
        })
        .collect()
}

/// The most live bytes above what was held before, over one run of `work`.
fn peak_of<T>(work: impl FnOnce() -> T) -> (T, isize) {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let out = work();
    (out, PEAK.load(Ordering::Relaxed) - before)
}

/// Failure: the purge keeps what it walks (every corpus's tree held until the step ends, a tree per folder, a listing per file), so its memory grows with the
/// cache against rule 1. With each corpus a tree of 65 entries, 20 corpora against 200: the peak grows by under 1 KiB for each extra corpus (about 0.2 KiB: a
/// name in the listing and its match), where holding each tree would add over 6 KiB. The brief's "within 1.2x" can't hold for a listing that is one entry a
/// corpus, so the property measured is the growth per corpus.
#[test]
fn purge_holds_one_tree_at_a_time() {
    let peak_for = |keys: u64| {
        let f = fixture(keys, 0, 60, 0, 0);
        let (s, pins) = survey_to_clear(&f);
        let steps = steps(&s, &pins);
        drop((s, pins));
        let (_, peak) = peak_of(|| purge_all(&f, &steps));
        assert!(!f.corpus().join(key(1)).exists(), "the corpora went");
        peak
    };
    let (small, large) = (peak_for(20), peak_for(200));
    println!(
        "PURGE_COST peak live bytes: 20 corpora {small}, 200 corpora {large}, {} per extra corpus",
        (large - small) / 180
    );
    assert!((large - small) / 180 < 1024, "{small} against {large}");
}

/// The most descriptors the process held over its baseline, seen by a sampler thread (so a floor), while `work` ran.
fn descriptors_over_baseline<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let (stop, high) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let baseline = std::fs::read_dir("/dev/fd").unwrap().count();
    let sampler = {
        let (stop, high) = (stop.clone(), high.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                high.fetch_max(std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0), Ordering::Relaxed);
                std::thread::sleep(Duration::from_micros(50)); // ratchet: poll
            }
        })
    };
    let out = work();
    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    (out, high.load(Ordering::Relaxed).saturating_sub(baseline))
}

/// Failure: the agent-pages step, or the older build's `corpus` taken child by child, holds more descriptors than the bound of 8 (a listing, a lock or a
/// folder kept open past its use), as the corpus path was held to. The agent step runs first on 40 corpora (the fixture gives an agent folder of 50 pages to
/// every fifth, so 8 are taken), then the corpus step with an older `corpus` of 10 children beside the root. Each step really takes its folders (none already
/// gone) and is within 8 descriptors of its baseline. A sampler sees them, so the number is a floor.
#[test]
fn the_agent_pages_and_older_corpus_paths_hold_at_most_eight_descriptors() {
    let f = fixture(40, 50, 0, 0, 0);
    let old = f.dir.join("corpus");
    for child in 0..10 {
        let folder = old.join(format!("child{child}"));
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("sub").join("file"), b"x").unwrap();
        std::fs::write(old.join(format!("loose{child}")), b"x").unwrap();
    }
    let (s, pins) = survey_to_clear(&f);
    let steps = steps(&s, &pins);
    drop((s, pins));
    let of = |store| steps.iter().filter(|(s, _)| *s == store).cloned().collect::<Vec<_>>();
    let (agents, corpora) = (of(Store::CorpusAgentPages), of(Store::CorpusText));
    let (taken, agent_descriptors) = descriptors_over_baseline(|| purge_steps(&f, &agents));
    assert_eq!(
        taken.iter().map(|p| (p.removed, p.already_gone)).collect::<Vec<_>>(),
        vec![(8, 0)],
        "the agent folders were really taken"
    );
    let (taken, corpus_descriptors) = descriptors_over_baseline(|| purge_steps(&f, &corpora));
    assert_eq!(taken.iter().map(|p| p.already_gone).sum::<u32>(), 0);
    assert!(!old.exists(), "the older corpus went with the step");
    println!("PURGE_COST descriptors over baseline: agent pages {agent_descriptors}, corpora with the older corpus {corpus_descriptors}");
    assert!(agent_descriptors <= 8, "agent pages: {agent_descriptors} more than before");
    assert!(corpus_descriptors <= 8, "corpora and the older corpus: {corpus_descriptors} more than before");
}

/// The measurement behind the report. Median of five runs of each, on fixtures of 100 and 1,000 corpora (a fifth with an agent folder of 50 pages), 3,000 notes and
/// 512 contents files: the clear-all survey with its pins, D2d's per-document survey of the same tree, every plan step purged in order (time, peak live bytes,
/// most descriptors seen by a sampler), and the app's own clear (`corpus::clear_all` and `generated::clear_all`) on the same tree. Not an assertion.
#[test]
#[ignore = "a measurement, not a test"]
fn costs() {
    fn median<T: Ord + Copy>(mut values: Vec<T>) -> T {
        values.sort();
        values[values.len() / 2]
    }
    for corpora in [100u64, 1_000] {
        let mut survey_times = Vec::new();
        let mut survey_peaks = Vec::new();
        let mut per_doc_times = Vec::new();
        let mut purge_times = Vec::new();
        let mut purge_peaks = Vec::new();
        let mut descriptors = Vec::new();
        let mut own_times = Vec::new();
        let (mut artifacts, mut found, mut steps_count) = (0, 0, 0);
        for _ in 0..5 {
            let f = fixture(corpora, 50, 0, 3_000, 512);
            let started = Instant::now();
            artifacts = survey_per_document(&f);
            per_doc_times.push(started.elapsed());
            let started = Instant::now();
            let ((s, pins), peak) = peak_of(|| survey_to_clear(&f));
            survey_times.push(started.elapsed());
            survey_peaks.push(peak);
            found = s.inventory().artifacts().len();
            let steps = steps(&s, &pins);
            let (stop, high) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
            let baseline = std::fs::read_dir("/dev/fd").unwrap().count();
            let sampler = {
                let (stop, high) = (stop.clone(), high.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        high.fetch_max(std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0), Ordering::Relaxed);
                        std::thread::sleep(Duration::from_micros(50)); // ratchet: poll
                    }
                })
            };
            let started = Instant::now();
            let (count, peak) = peak_of(|| purge_all(&f, &steps));
            purge_times.push(started.elapsed());
            purge_peaks.push(peak);
            stop.store(true, Ordering::Relaxed);
            sampler.join().unwrap();
            descriptors.push(high.load(Ordering::Relaxed).saturating_sub(baseline));
            steps_count = count;
            let own = fixture(corpora, 50, 0, 3_000, 512);
            let started = Instant::now();
            recto_index::corpus::clear_all(&own.corpus()).unwrap();
            recto_index::generated::clear_all(&own.contents());
            own_times.push(started.elapsed());
        }
        println!(
            "N={corpora}: {found} artifacts to clear in {steps_count} steps ({:.2} a corpus), clear-all survey median {:?} peak {:.2} MiB | per-document survey {:?} ({artifacts} artifacts) | purge median {:?} peak {:.2} MiB, most descriptors over baseline {} | the app's own clear {:?}",
            (found as f64 / corpora as f64),
            median(survey_times),
            median(survey_peaks) as f64 / 1_048_576.0,
            median(per_doc_times),
            median(purge_times),
            median(purge_peaks) as f64 / 1_048_576.0,
            median(descriptors),
            median(own_times),
        );
    }
}

use std::sync::Arc;
