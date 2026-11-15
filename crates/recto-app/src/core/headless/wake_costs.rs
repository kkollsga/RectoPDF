//! **What waking an idle tab costs, measured** (owner, 2026-11-15: no picture of a page is kept): from a person's tap to the tab showing, to its pages on
//! their paper, to the first frame with something on every page in view, and to sharp; for a tab with a sleep file (its facts) and one without (the first
//! wake after the update, and every wake of a document opened with its password), on a page of text and a slow page, its worker gone first as after a
//! restart or a long idle. Asserts nothing about time and runs
//! only when asked, on a release build:
//!
//! `tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-app -p recto-render-service --features recto-app/testkit --run-ignored only --no-capture -E 'test(=core::headless::wake_costs::waking_an_idle_tab)'`

use crate::testkit::headless::{Disk, HeadlessApp, applied_as_the_host, host_turns};
use std::time::{Duration, Instant};

fn view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

fn tab_named(app: &HeadlessApp, name: &str) -> u64 {
    app.session_json()["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .and_then(|tab| tab["id"].as_u64())
        .expect("the tab")
}

/// Milliseconds from `began`, to a tenth.
fn ms(began: Instant) -> f64 {
    (began.elapsed().as_secs_f64() * 10_000.0).round() / 10.0
}

/// One wake of `name`, asleep, by the person's tap, the host taking every turn: when the tab was showing, its pages on paper, its first frame with
/// something on every page in view, and sharp, in milliseconds from the tap.
fn one_wake(app: &HeadlessApp, name: &str, keep_file: bool) -> [f64; 4] {
    let engine = app.core().engine.clone();
    let tab = tab_named(app, name);
    let other = tab_named(app, "other.pdf");
    let mut view = view();
    person_taps(app, other);
    host_turns(app, &mut view, true);
    app.recto(recto_agent::Level::Full, &["session", "sleep", "--tab", &format!("#{tab}")])
        .expect("asleep");
    host_turns(app, &mut view, true);
    if !keep_file {
        for path in sleep_files(app) {
            let _ = std::fs::remove_file(path);
        }
    }
    // Its worker gone, as after a restart or once it's been idle: the wake starts one, as the first wake after the update does.
    for pid in engine.workers().pids() {
        let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
    let gone = Instant::now();
    while !engine.workers().pids().is_empty() && gone.elapsed() < Duration::from_secs(5) {
        let _ = engine.drain();
        engine.applied();
        std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
    }
    // Asleep in the page view too, not only in the session: the page view lets go of a tab gone cold when it's next told the warm tabs, and a tap
    // before that shows it from what it still holds, with no wake to record (found 2026-11-15: the "wake that never ended", 1 in about 40, was this:
    // the tab drawn 2 ms after the tap from its held tiles, and `last_wake` still the other tab's).
    let held_since = Instant::now();
    while engine.waking().holds(u32::try_from(tab).expect("a tab id")) {
        assert!(
            held_since.elapsed() < Duration::from_secs(10),
            "the page view still holds tab {tab} after it went to sleep"
        );
        host_turns(app, &mut view, false);
        std::thread::sleep(Duration::from_millis(1)); // ratchet: poll
    }
    let began = Instant::now();
    person_taps(app, tab);
    let (mut showing, mut paper, mut first, mut sharp, mut tiled) = (None, None, None, None, None);
    while sharp.is_none() && began.elapsed() < Duration::from_secs(20) {
        let mut ops = engine.drain();
        applied_as_the_host(&ops);
        engine.applied();
        while let Some(op) = ops.pop() {
            if let recto_reader::Op::ScrollTo { y, seq } = op {
                view.y = y;
                view.scrolled_to = seq;
                ops = engine.set_viewport(view);
                applied_as_the_host(&ops);
                engine.applied();
            }
        }
        let report = engine.report().0;
        if showing.is_none()
            && app
                .core()
                .host
                .session
                .try_with_session(|session| session.tabs().active().map(|id| u64::from(id.0)))
                .flatten()
                == Some(tab)
        {
            showing = Some(ms(began));
        }
        if paper.is_none() && showing.is_some() && report["tiles"]["page_layers"].as_u64().unwrap_or(0) > 0 {
            paper = Some(ms(began));
        }
        // Something drawn on a layer, by the page view's own count rather than the wake's record, which ends after `WAKE_FOLLOWED_FOR`: late is told from
        // never.
        if tiled.is_none() && showing.is_some() && report["tiles"]["attached"].as_u64().unwrap_or(0) > 0 {
            tiled = Some(ms(began));
        }
        let wake = &report["last_wake"];
        if wake["tab"] == tab {
            if first.is_none() && wake["first_frame_us"].is_u64() {
                first = Some(ms(began));
            }
            if wake["sharp_us"].is_u64() {
                sharp = Some(ms(began));
            }
        }
        std::thread::sleep(Duration::from_micros(200)); // ratchet: poll
    }
    if sharp.is_none() {
        // What didn't happen, said as the record has it: the wake, the page view's jobs and workers, what the session thread was doing.
        let report = engine.report().0;
        let state = app.record_json();
        println!(
            "a wake that didn't end: showing {showing:?} paper {paper:?} first {first:?} something on a layer {tiled:?}\n  workers {:?} (killed before the tap)\n  the page view: {}\n  the record: {}",
            engine.workers().pids(),
            report,
            state
        );
    }
    [showing, paper, first, sharp].map(|at| at.unwrap_or(f64::NAN))
}

/// The person's tap on a tab, as the window sends it.
fn person_taps(app: &HeadlessApp, tab: u64) {
    let words = ["tap".to_string(), format!("#{tab}")];
    app.person(&recto_agent::table::parse_cli(&words).expect("a command").wire());
}

fn sleep_files(app: &HeadlessApp) -> Vec<std::path::PathBuf> {
    let folder = app.disk().paths().root.join("session").join("journals");
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(recto_session::record::is_sleep_name)
        })
        .collect()
}

#[test]
#[ignore = "a measurement, run on a release build when asked"]
fn waking_an_idle_tab() {
    let load = || {
        std::process::Command::new("uptime")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    };
    println!("load before: {}", load());
    // `RECTO_WAKE_ONLY=slow-facts` (a kind and `facts` or `pdf`) runs one route, and `RECTO_WAKE_RUNS` how many times: to look into one route.
    let only = std::env::var("RECTO_WAKE_ONLY").ok();
    let runs_each: usize = std::env::var("RECTO_WAKE_RUNS").ok().and_then(|runs| runs.parse().ok()).unwrap_or(5);
    for (kind, page) in [("text", recto_testpdf::busy_page()), ("slow", recto_testpdf::slow_page(40_000))] {
        for keep_file in [true, false] {
            let route = format!("{kind}-{}", if keep_file { "facts" } else { "pdf" });
            if only.as_ref().is_some_and(|only| *only != route) {
                continue;
            }
            let mut runs = Vec::new();
            for _ in 0..runs_each {
                let disk = Disk::new();
                let name = format!("{kind}.pdf");
                let doc = disk.file(
                    &name,
                    &recto_testpdf::pages(&(0..12).map(|_| recto_testpdf::Page::new(page.clone())).collect::<Vec<_>>()),
                );
                let other = disk.pdf("other.pdf", 2);
                let app = disk.start();
                app.core().engine.waking().window_up();
                for file in [&doc, &other] {
                    app.open(file);
                    let mut view = view();
                    applied_as_the_host(&app.core().engine.set_viewport(view));
                    host_turns(&app, &mut view, true);
                }
                runs.push(one_wake(&app, &name, keep_file));
                drop(app.quit());
            }
            let column = |at: usize| {
                let mut all: Vec<f64> = runs.iter().map(|run| run[at]).collect();
                all.sort_by(f64::total_cmp);
                all
            };
            println!(
                "{kind} page, {}: showing {:?} ms; on paper {:?} ms; first frame {:?} ms; sharp {:?} ms",
                if keep_file {
                    "from its sleep file's facts"
                } else {
                    "no sleep file, from its PDF"
                },
                column(0),
                column(1),
                column(2),
                column(3)
            );
        }
    }
    println!("load after: {}", load());
}

/// **What a sleep file of a real document comes to,** asserting nothing: the first 25 PDFs of the folder `RECTO_SLEEP_SIZES_DIR` names (2026-11-15:
/// distillPDF's `benchmarking/bench100`), each opened and drawn, then put to sleep: its file's bytes, sorted, with the average. Runs when asked:
///
/// `tools/sysguard/heavy.sh cargo nextest run --locked -p recto-app -p recto-render-service --features recto-app/testkit --run-ignored only --no-capture -E 'test(=core::headless::wake_costs::sleep_file_sizes_of_real_documents)'`
#[test]
#[ignore = "a measurement, run when asked"]
fn sleep_file_sizes_of_real_documents() {
    let Ok(folder) = std::env::var("RECTO_SLEEP_SIZES_DIR") else {
        println!("set RECTO_SLEEP_SIZES_DIR to a folder of real PDFs");
        return;
    };
    let Ok(entries) = std::fs::read_dir(&folder) else {
        println!("no folder of real documents at {folder}");
        return;
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "pdf"))
        .collect();
    files.sort();
    let mut sizes = Vec::new();
    for file in files.into_iter().take(25) {
        let disk = Disk::new();
        let name = file.file_name().and_then(|name| name.to_str()).unwrap_or("doc.pdf").to_string();
        let doc = disk.file(&name, &std::fs::read(&file).expect("a real document"));
        let other = disk.pdf("other.pdf", 2);
        let app = disk.start();
        for opened in [&doc, &other] {
            app.open(opened);
            let mut view = view();
            applied_as_the_host(&app.core().engine.set_viewport(view));
            host_turns(&app, &mut view, true);
        }
        let _ = app.recto(
            recto_agent::Level::Full,
            &["session", "sleep", "--tab", &format!("#{}", tab_named(&app, &name))],
        );
        let mut view = view();
        host_turns(&app, &mut view, true);
        sizes.extend(sleep_files(&app).iter().filter_map(|path| std::fs::metadata(path).ok()).map(|meta| meta.len()));
        drop(app.quit());
    }
    sizes.sort_unstable();
    let average = sizes.iter().sum::<u64>() / u64::try_from(sizes.len().max(1)).unwrap_or(1);
    println!("sleep files of {} real documents (bytes, sorted): {sizes:?}; average {average}", sizes.len());
}
