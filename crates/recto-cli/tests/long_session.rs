//! **A long session, in its own binary** (SPEC section 2: "memory flat over a 24-hour session"). 30 tabs, going round them, editing and moving about, with tabs going dormant as they would. What matters is the shape: the whole app's footprint doesn't climb from round to round. Rounds count from `RECTO_LONG_SESSION_ROUNDS`, a few by default. Each round's numbers are printed for a report, with `--no-capture`. On a debug build these are about the shape only, so what's reported comes from a release one.

mod common;

use common::State;
use common::app::{Run, app_path};
use recto_agent::Level;
use recto_testpdf::{Page, pages, text_lines};
use serde_json::{Value, json};

const TABS: usize = 30;

/// The machine's load averages as the system gives them ("{ 3.12 2.80 2.50 }"), for a failure to say how busy the machine was: a wake's time means little without it.
fn load() -> String {
    std::process::Command::new("/usr/sbin/sysctl").args(["-n", "vm.loadavg"]).output().map_or_else(
        |error| format!("unreadable: {error}"),
        |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

/// A wake that showed a bare page, with everything that says why, kept as it was read: the wake's own report, what the page view said right after (settled, tiles, jobs, drawn), the app's worst stall, and the machine's load as the wake began and as it was read.
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

/// Failure: the app's memory climbs as tabs come and go, as something's held per wake or per edit, or going dormant leaves what it should take. Or a tab's edits don't survive all that, or workers pile up, or disk bytes go past what edits and pictures account for.
#[test]
fn a_long_session_stays_flat() {
    let rounds: usize = std::env::var("RECTO_LONG_SESSION_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let state = State::at(Level::Full);
    let Some(app) = app_path() else { return };
    let (reply, code) = state.json(&["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "30m"]);
    assert_eq!(code, 0, "{reply}");
    let pid = reply["data"]["pid"].as_u64().unwrap();
    // The run is attached, so a failure leaves its evidence folder (the app's state and threads, the machine, and the command that runs it again) as the other app tests do, and the app is stopped.
    let run = Run::attach(&state, pid as u32);
    let ok = |args: &[&str]| run.ok(args);
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let mut ids = Vec::with_capacity(TABS);
    for at in 0..TABS {
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
        ok(&["qa", "wait-settled", "--timeout", "30s"]);
    }
    for tab in ok(&["qa", "state"])["session"]["tabs"].as_array().unwrap() {
        ids.push(tab["id"].as_u64().unwrap());
    }
    assert_eq!(ids.len(), TABS);
    // Going dormant in the time this has, not five minutes.
    ok(&["qa", "dormant-after", "2"]);
    // And with no limit on how often pictures are written, as what's being looked at here is what a wake and a dormancy cost, and whether they build up.
    ok(&["qa", "sleep-write-gap", "0"]);
    let mut footprints = Vec::new();
    let (mut peak_workers, mut peak_worker_bytes) = (0u64, 0u64);
    let mut edits = 0u64;
    let mut wakes: Vec<(bool, u64, u64)> = Vec::new();
    let mut blank_frames = 0u64;
    let mut bare: Vec<BareWake> = Vec::new();
    let (mut unowed_bare, mut no_first_frame) = (0u64, Vec::<String>::new());
    for round in 0..rounds {
        for (at, id) in ids.iter().enumerate() {
            // As a click on its row does.
            let _ = id;
            let (load_before, began) = (load(), std::time::Instant::now());
            ok(&["qa", "click-row", &format!("{at}")]);
            ok(&["qa", "wait-settled", "--timeout", "30s"]);
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
                // What a wake had to show when it began, as its report says: a bare page is only a bug where a picture of the view was here.
                let owed = !matches!(woken["had"].as_str(), Some("nothing" | "not_looked") | None);
                if blank > 0 && !owed {
                    unowed_bare += 1;
                }
                if woken["first_frame_us"].is_null() {
                    no_first_frame.push(format!("round {round}, row {at}: {woken}"));
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
                        load_after: load(),
                        wall_ms,
                    });
                }
            }
            ok(&["goto", &format!("{}", (round * 7 + at) % 40 + 1)]);
            if (at + round) % 3 == 0 {
                ok(&["qa", "owner-edit", &format!("rotate:{}", at % 40 + 1)]);
                edits += 1;
            }
            ok(&["qa", "wait-settled", "--timeout", "30s"]);
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
            ids.len()
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
    println!("wakes with a bare page and nothing to show them with, which is allowed until the renderer answers: {unowed_bare}");
    if !bare.is_empty() {
        let said: Vec<String> = bare
            .iter()
            .map(|wake| {
                format!(
                    "round {}, row {}: last_wake {}\n  click to settled {} ms; load as it began {}, as it was read {}\n  hangs.worst {}\n  page view after it: {}",
                    wake.round, wake.row, wake.wake, wake.wall_ms, wake.load_before, wake.load_after, wake.hangs_worst, wake.reader
                )
            })
            .collect();
        panic!(
            "a wake that began with a picture of its view, from the sleep file or the cache, showed a bare page ({} wakes; {blank_frames} bare frames in all, {unowed_bare} wakes bare with nothing to show). `had` and `bare_with_picture` say which.\n{}\nall wakes (from a file, first frame us, sharp us): {wakes:?}\nload now {}",
            bare.len(),
            said.join("\n"),
            load()
        );
    }
    assert!(no_first_frame.is_empty(), "a wake never got its first frame:\n{}", no_first_frame.join("\n"));
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
    // Disk: what edits and a picture a dormancy account for. Generous, as what's at stake is its growing without end.
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
    run.quit();
}
