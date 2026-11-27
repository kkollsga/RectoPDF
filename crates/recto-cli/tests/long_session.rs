//! **A long session, in its own binary** (SPEC section 2: "memory flat over a 24-hour session"). 30 tabs, going round them, editing and moving about, with tabs going dormant as they would. What matters is the shape: the whole app's footprint doesn't climb from round to round. Rounds count from `RECTO_LONG_SESSION_ROUNDS`, a few by default. Each round's numbers are printed for a report, with `--no-capture`. On a debug build these are about the shape only, so what's reported comes from a release one.

#[path = "long_session/client.rs"]
mod client;
mod common;

use common::State;
use common::app::{Run, app_path, settle_guard_arg};
use recto_agent::Level;
use recto_testpdf::{Page, pages, text_lines};
use serde_json::{Value, json};
use std::time::Duration;

const TABS: usize = 30;

/// How long a wake's report follows it (`recto_reader::tiles::wake::WAKE_FOLLOWED_FOR`, which this test can't reach).
const WAKE_FOLLOWED_FOR_MS: u64 = 5000;

/// A wake that showed a bare page, or had no first frame, with everything that says why, kept as it was read: the wake's own report, the page view right after, the app's worst stall, a labelled preceding load snapshot, and a fresh load snapshot when the problem was read.
struct BareWake {
    round: usize,
    row: usize,
    wake: Value,
    reader: Value,
    hangs_worst: Value,
    load_before: String,
    load_after: String,
    wall_ms: u128,
}

impl BareWake {
    /// All of it, for a failure.
    fn said(&self) -> String {
        format!(
            "round {}, row {}: last_wake {}\n  click to settled {} ms; load before (sample point labelled) {}, as failure was read {}\n  hangs.worst {}\n  page view after it: {}",
            self.round, self.row, self.wake, self.wall_ms, self.load_before, self.load_after, self.hangs_worst, self.reader
        )
    }
}

/// Failure: the app's memory climbs as tabs come and go, as something's held per wake or per edit, or going dormant leaves what it should take. Or a tab's edits don't survive all that, or workers pile up, or disk bytes go past what edits and pictures account for. Or a wake's report loses its first frame: a wake with none is held to a first page later than a wake's followed for, by this test's own clock, with its tab drawn since.
#[test]
fn a_long_session_stays_flat() {
    let started = std::time::Instant::now();
    let rounds: usize = std::env::var("RECTO_LONG_SESSION_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let state = State::at(Level::Full);
    let Some(app) = app_path() else { return };
    eprintln!("long-session progress: phase=start command=qa start");
    // Retain the launched Child through Quit, rather than an attached connection that can expire while the workload runs.
    let run = Run::on_executable_with_env(&state, &app, Duration::from_secs(30 * 60), &[]);
    let workload = client::Workload::new(&run);
    let ok = |args: &[&str]| workload.ok(args);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let mut ids = Vec::with_capacity(TABS);
    for at in 0..TABS {
        workload.at("opening", 0, at);
        let file = folder.join(format!("doc{at}.pdf"));
        std::fs::write(
            &file,
            pages(
                &(0..40)
                    .map(|page| {
                        Page::new(text_lines(&[
                            &format!("Document {at}, page {page}"),
                            "Some more words on the page, so it's a page of text and not a blank.",
                        ]))
                    })
                    .collect::<Vec<_>>(),
            ),
        )
        .unwrap();
        ok(&["open", file.to_str().unwrap()]);
        ok(&["qa", "wait-settled", "--timeout", &settle_guard_arg()]);
    }
    for tab in ok(&["qa", "state"])["session"]["tabs"].as_array().unwrap() {
        ids.push(tab["id"].as_u64().unwrap());
    }
    assert_eq!(ids.len(), TABS);
    // Going dormant in the time this has, not five minutes.
    ok(&["qa", "dormant-after", "2"]);
    // And with no limit on how often sleep files are written, as what's being looked at here is what a wake and a dormancy cost, and whether they build up.
    ok(&["qa", "sleep-write-gap", "0"]);
    let mut footprints = Vec::new();
    let (mut peak_workers, mut peak_worker_bytes) = (0u64, 0u64);
    let mut edits = 0u64;
    let mut wakes: Vec<(bool, u64, u64)> = Vec::new();
    let mut blank_frames = 0u64;
    let mut bare: Vec<BareWake> = Vec::new();
    let (mut unowed_bare, mut no_first_frame, mut late) = (0u64, Vec::<BareWake>::new(), Vec::<String>::new());
    for round in 0..rounds {
        let round_load = (!workload.cli_baseline()).then(|| workload.load());
        for (at, id) in ids.iter().enumerate() {
            workload.at("visiting", round, at);
            // As a click on its row does.
            let _ = id;
            let load_before = round_load
                .as_ref()
                .map_or_else(|| format!("wake start: {}", workload.load()), |load| format!("round {round} start: {load}"));
            let began = std::time::Instant::now();
            ok(&["qa", "click-row", &format!("{at}")]);
            ok(&["qa", "wait-settled", "--timeout", &settle_guard_arg()]);
            let seen = ok(&["qa", "state"]);
            let wall_ms = began.elapsed().as_millis();
            let woken = seen["reader"]["last_wake"].clone();
            if woken["tab"] == json!(id) && round > 0 {
                let from_sleep = woken["from_sleep"] == true;
                wakes.push((
                    from_sleep,
                    woken["first_frame_us"].as_u64().unwrap_or(0),
                    woken["sharp_us"].as_u64().unwrap_or(0),
                ));
                let blank = woken["blank_frames"].as_u64().unwrap_or(0);
                blank_frames += blank;
                // What a wake had to show when it began, as its report says: a bare page is only a bug where what the view showed was still held (a warm
                // tab's tiles). A wake from its facts has its pages on their paper, bare until the renderer answers, as one with nothing has.
                let owed = woken["had"] == "cache";
                if blank > 0 && !owed {
                    unowed_bare += 1;
                }
                // No first frame: its tab is drawn by now (`wait-settled`), so either its first page came after the time a wake's followed for, and
                // the report stopped while it was still bare, which is the machine's speed (seen at a load of 30 under CI's limits, 2026-11-15), or the
                // report lost a frame that came, which is a fault: under way after the view settled, ended by a move nobody made, or said to be past
                // the time when it wasn't by this test's own clock, or its view still bare.
                if woken["first_frame_us"].is_null() {
                    let drawn = seen["reader"]["tiles"]["attached"].as_u64().unwrap_or(0) > 0;
                    let followed_ms = u128::from(WAKE_FOLLOWED_FOR_MS);
                    if woken["ended"] == "no_longer_followed" && wall_ms > followed_ms && drawn {
                        late.push(format!("round {round}, row {at}: {woken}, click to settled {wall_ms} ms"));
                    } else {
                        no_first_frame.push(BareWake {
                            round,
                            row: at,
                            wake: woken.clone(),
                            reader: seen["reader"].clone(),
                            hangs_worst: seen["hangs"]["worst"].clone(),
                            load_before: load_before.clone(),
                            load_after: workload.load(),
                            wall_ms,
                        });
                    }
                }
                if blank > 0 {
                    println!("a wake with bare frames, round {round}, row {at}: {woken}");
                }
                if blank > 0 && owed {
                    bare.push(BareWake {
                        round,
                        row: at,
                        wake: woken.clone(),
                        reader: seen["reader"].clone(),
                        hangs_worst: seen["hangs"]["worst"].clone(),
                        load_before,
                        load_after: workload.load(),
                        wall_ms,
                    });
                }
            }
            ok(&["goto", &format!("{}", (round * 7 + at) % 40 + 1)]);
            if (at + round) % 3 == 0 {
                ok(&["qa", "owner-edit", &format!("rotate:{}", at % 40 + 1)]);
                edits += 1;
            }
            ok(&["qa", "wait-settled", "--timeout", &settle_guard_arg()]);
            let memory = ok(&["qa", "state"])["memory"].clone();
            peak_workers = peak_workers.max(memory["workers"].as_u64().unwrap_or(0));
            peak_worker_bytes = peak_worker_bytes.max(memory["worker_bytes"].as_u64().unwrap_or(0));
        }
        // A moment's rest for what's left alone to go, on the one deadline.
        std::thread::sleep(std::time::Duration::from_secs(3));
        let report = ok(&["qa", "state"]);
        let memory = &report["memory"];
        let footprint = memory["footprint_bytes"].as_u64().unwrap();
        footprints.push(footprint);
        println!(
            "round {round}: app {:.1} MiB, workers now {:.1} MiB ({} alive), held {}, threads {}, wrote {} bytes, dormant {}, sleep files {} ({} bytes), lists in memory {}",
            footprint as f64 / 1048576.0,
            memory["worker_bytes"].as_u64().unwrap_or(0) as f64 / 1048576.0,
            memory["workers"],
            report["reader"]["documents_held"],
            memory["threads"],
            report["record"]["wrote_bytes"],
            report["record"]["sleep"]["gone_dormant"],
            report["record"]["sleep"]["sleep_files_written"],
            report["record"]["sleep"]["sleep_bytes_written"],
            ids.len(),
        );
        assert!(report["reader"]["documents_held"].as_u64().unwrap() <= 3, "{}", report["reader"]);
    }
    println!(
        "{TABS} tabs, {rounds} rounds, {edits} edits. Workers: at most {peak_workers} alive, {:.1} MiB between them.",
        peak_worker_bytes as f64 / 1048576.0
    );
    let sleeping: Vec<&(bool, u64, u64)> = wakes.iter().filter(|wake| wake.0).collect();
    let middle = |pick: fn(&(bool, u64, u64)) -> u64, at: f64| {
        let mut all: Vec<u64> = sleeping.iter().map(|wake| pick(wake)).collect();
        all.sort_unstable();
        all.get(((all.len().max(1) - 1) as f64 * at) as usize).copied().unwrap_or(0)
    };
    println!(
        "wakes: {} of {} from a sleep file, first frame p50 {} us p95 {} us, sharp p50 {} us p95 {} us, blank frames {blank_frames}",
        sleeping.len(),
        wakes.len(),
        middle(|wake| wake.1, 0.5),
        middle(|wake| wake.1, 0.95),
        middle(|wake| wake.2, 0.5),
        middle(|wake| wake.2, 0.95)
    );
    assert!(
        sleeping.len() * 2 >= wakes.len(),
        "most wakes are from a file: {} of {}",
        sleeping.len(),
        wakes.len()
    );
    println!("wakes with a bare page from their facts or nothing, which is allowed until the renderer answers: {unowed_bare}");
    if !bare.is_empty() {
        let said: Vec<String> = bare.iter().map(BareWake::said).collect();
        panic!(
            "a wake that began with its view held in the cache showed a bare page ({} wakes; {blank_frames} bare frames in all, {unowed_bare} wakes bare from their facts or nothing). `had` says which.\n{}\nall wakes (from a file, first frame us, sharp us): {wakes:?}\nload now {}",
            bare.len(),
            said.join("\n"),
            workload.load()
        );
    }
    println!(
        "wakes whose first page came after the {WAKE_FOLLOWED_FOR_MS} ms a wake's followed for, drawn since: {}",
        late.len()
    );
    for wake in &late {
        println!("  {wake}");
    }
    let lost: Vec<String> = no_first_frame.iter().map(BareWake::said).collect();
    assert!(
        lost.is_empty(),
        "a wake's report has no first frame, and not for a first page later than it's followed for:\n{}",
        lost.join("\n")
    );
    let report = ok(&["qa", "state"]);
    let dormant = &report["record"]["sleep"]["gone_dormant"]["left_alone"];
    assert!(dormant.as_u64().unwrap() >= TABS as u64 / 2, "tabs did go dormant: {dormant}");
    // From after the first round's taken everything up, with room for the system's own caches: flat, not to the byte.
    if footprints.len() >= 3 {
        let (settled, last) = (footprints[1], *footprints.last().unwrap());
        assert!(last <= settled + settled / 4 + 8 * 1024 * 1024, "it climbed: {footprints:?}");
    }
    // Edits came through it all, wherever they've been.
    let lists = report["session"]["edits_full"].as_object().unwrap();
    assert!(lists.len() >= TABS / 3, "{}", lists.len());
    // Disk: what edits and a dormancy account for. Generous, as what's at stake is its growing without end.
    let wrote = report["record"]["wrote_bytes"].as_u64().unwrap();
    let dormancies: u64 = ["left_alone", "budget", "asked", "pressure"]
        .iter()
        .map(|why| report["record"]["sleep"]["gone_dormant"][*why].as_u64().unwrap_or(0))
        .sum();
    let budget = 64 * 1024 + edits * 2048 + dormancies * 256 * 1024;
    assert!(
        wrote <= budget,
        "wrote {wrote} bytes, against {budget}, with {edits} edits and {dormancies} tabs going dormant"
    );
    let counts = workload.counts();
    drop(workload);
    eprintln!("long-session progress: phase=quit command=qa quit now");
    run.quit();
    println!(
        "long-session workload transport: {} CLI processes, {} public-client commands, {} sysctl processes; wall {} ms",
        counts.cli,
        counts.protocol,
        counts.sysctl,
        started.elapsed().as_millis()
    );
}
