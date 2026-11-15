//! **What flicking through ten large tabs holds, measured** (the cool-down's measurement of 2026-11-13 in `docs/engineering/sleep.md`, here on the app's own
//! core rather than the real app, so it can be run on any commit in the same way): ten documents of 40 busy pages, opened, then each shown in turn and drawn
//! at a Retina window's fit, three rounds, the cool-down at its default; the process's footprint and threads after each round, and the most the leaving
//! queue held. Asserts nothing. Runs when asked, on a release build:
//!
//! `tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-app -p recto-render-service --features recto-app/testkit --run-ignored only --no-capture -E 'test(=core::headless::flick_cost::flicking_through_ten_large_tabs)'`

use crate::testkit::headless::{Disk, applied_as_the_host, host_turns};

#[test]
#[ignore = "a measurement, run on a release build when asked"]
fn flicking_through_ten_large_tabs() {
    let view = || recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    };
    let footprint = || recto_platform::footprint_bytes().unwrap_or(0);
    let threads = || recto_platform::local::thread_names(std::process::id()).map_or(0, |names| names.len());
    let mib = |bytes: u64| (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0;
    let load = || {
        std::process::Command::new("uptime")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    };
    for run in 0..3 {
        let disk = Disk::new();
        let docs: Vec<_> = (0..10)
            .map(|at| {
                let pages: Vec<recto_testpdf::Page> = (0..40).map(|_| recto_testpdf::Page::new(recto_testpdf::busy_page())).collect();
                disk.file(&format!("large{at}.pdf"), &recto_testpdf::pages(&pages))
            })
            .collect();
        let app = disk.start();
        let engine = app.core().engine.clone();
        engine.waking().window_up();
        let before = footprint();
        for doc in &docs {
            app.open(doc);
            let mut view = view();
            applied_as_the_host(&engine.set_viewport(view));
            host_turns(&app, &mut view, true);
        }
        let tabs: Vec<u64> = app.session_json()["tabs"]
            .as_array()
            .expect("tabs")
            .iter()
            .filter_map(|tab| tab["id"].as_u64())
            .collect();
        let mut rounds = Vec::new();
        for _ in 0..3 {
            for tab in &tabs {
                let words = ["tap".to_string(), format!("#{tab}")];
                app.person(&recto_agent::table::parse_cli(&words).expect("a command").wire());
                let mut view = view();
                host_turns(&app, &mut view, true);
            }
            rounds.push((mib(footprint()), threads()));
        }
        let leaving = app.record_json()["sleep"]["leaving"].clone();
        let sleep = app.record_json()["sleep"].clone();
        println!(
            "run {run}: footprint at the start {} MiB; after each round (MiB, threads) {rounds:?}; queue peak {} tabs, {} B; sleep files {} ({} B); load {}",
            mib(before),
            leaving["peak_tabs"],
            leaving["peak_bytes"],
            sleep["sleep_files_written"],
            sleep["sleep_bytes_written"],
            load()
        );
        drop(app.quit());
    }
}
