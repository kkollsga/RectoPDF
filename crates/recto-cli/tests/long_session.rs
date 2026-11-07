//! **A long session, in its own binary** (SPEC section 2: "memory flat over a 24-hour session"). 30 tabs, going round them, editing and moving about, with tabs going dormant as they would. What matters is the shape: the whole app's footprint doesn't climb from round to round. Rounds count from `RECTO_LONG_SESSION_ROUNDS`, a few by default. Each round's numbers are printed for a report, with `--no-capture`. On a debug build these are about the shape only, so what's reported comes from a release one.

mod common;

use common::State;
use recto_agent::Level;
use recto_testpdf::{Page, pages, text_lines};
use serde_json::{Value, json};
use std::path::PathBuf;

const TABS: usize = 30;

/// The app, unless this run's said it isn't to be used: a stage of `tools/check.sh` that runs before the app's been built from this tree would otherwise use whatever's there.
fn app_path() -> Option<PathBuf> {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let root = exe.ancestors().find(|folder| folder.join("Cargo.lock").exists())?.to_path_buf();
    let app = root.join("app/build/RectoPDF.app/Contents/MacOS/RectoPDF");
    if app.exists() {
        return Some(app);
    }
    assert!(std::env::var_os("RECTO_REQUIRE_APP").is_none(), "this needs the app built");
    None
}

fn ok(state: &State, args: &[&str]) -> Value {
    let (reply, code) = state.json(args);
    assert_eq!((code, &reply["ok"]), (0, &json!(true)), "{args:?}: {reply}");
    reply["data"].clone()
}

/// Failure: the app's memory climbs as tabs come and go, as something's held per wake or per edit, or going dormant leaves what it should take. Or a tab's edits don't survive all that, or workers pile up, or disk bytes go past what edits and pictures account for.
#[test]
fn a_long_session_stays_flat() {
    let rounds: usize = std::env::var("RECTO_LONG_SESSION_ROUNDS").ok().and_then(|value| value.parse().ok()).unwrap_or(3);
    let state = State::at(Level::Full);
    let Some(app) = app_path() else { return };
    let (reply, code) = state.json(&["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "30m"]);
    assert_eq!(code, 0, "{reply}");
    let pid = reply["data"]["pid"].as_u64().unwrap();
    let folder = state.root.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    let mut ids = Vec::with_capacity(TABS);
    for at in 0..TABS {
        let file = folder.join(format!("doc{at}.pdf"));
        std::fs::write(&file, pages(&(0..40).map(|page| Page::new(text_lines(&[&format!("Document {at}, page {page}"), "Some more words on the page, so it's a page of text and not a blank."]))).collect::<Vec<_>>())).unwrap();
        ok(&state, &["open", file.to_str().unwrap()]);
        ok(&state, &["qa", "wait-settled", "--timeout", "30s"]);
    }
    for tab in ok(&state, &["qa", "state"])["session"]["tabs"].as_array().unwrap() {
        ids.push(tab["id"].as_u64().unwrap());
    }
    assert_eq!(ids.len(), TABS);
    // Going dormant in the time this has, not five minutes.
    ok(&state, &["qa", "dormant-after", "2"]);
    // And with no limit on how often pictures are written, as what's being looked at here is what a wake and a dormancy cost, and whether they build up.
    ok(&state, &["qa", "sleep-write-gap", "0"]);
    let mut footprints = Vec::new();
    let (mut peak_workers, mut peak_worker_bytes) = (0u64, 0u64);
    let mut edits = 0u64;
    let mut wakes: Vec<(bool, u64, u64)> = Vec::new();
    let mut blank_frames = 0u64;
    for round in 0..rounds {
        for (at, id) in ids.iter().enumerate() {
            // As a click on its row does.
            let _ = id;
            ok(&state, &["qa", "click-row", &format!("{at}")]);
            ok(&state, &["qa", "wait-settled", "--timeout", "30s"]);
            let woken = ok(&state, &["qa", "state"])["reader"]["last_wake"].clone();
            if woken["tab"] == json!(id) && round > 0 {
                let from_sleep = woken["from_sleep"] == true;
                wakes.push((from_sleep, woken["first_frame_us"].as_u64().unwrap_or(0), woken["sharp_us"].as_u64().unwrap_or(0)));
                blank_frames += woken["blank_frames"].as_u64().unwrap_or(0);
                if woken["blank_frames"].as_u64().unwrap_or(0) > 0 {
                    println!("a wake with bare frames, round {round}, row {at}: {woken}");
                }
            }
            ok(&state, &["goto", &format!("{}", (round * 7 + at) % 40 + 1)]);
            if (at + round) % 3 == 0 {
                ok(&state, &["qa", "owner-edit", &format!("rotate:{}", at % 40 + 1)]);
                edits += 1;
            }
            ok(&state, &["qa", "wait-settled", "--timeout", "30s"]);
            let memory = ok(&state, &["qa", "state"])["memory"].clone();
            peak_workers = peak_workers.max(memory["workers"].as_u64().unwrap_or(0));
            peak_worker_bytes = peak_worker_bytes.max(memory["worker_bytes"].as_u64().unwrap_or(0));
        }
        // A moment's rest for what's left alone to go, on the one deadline.
        std::thread::sleep(std::time::Duration::from_secs(3));
        let report = ok(&state, &["qa", "state"]);
        let memory = &report["memory"];
        let footprint = memory["footprint_bytes"].as_u64().unwrap();
        footprints.push(footprint);
        println!("round {round}: app {:.1} MiB, workers now {:.1} MiB ({} alive), held {}, threads {}, wrote {} bytes, dormant {}, sleep files {} ({} bytes), lists in memory {}", footprint as f64 / 1048576.0, memory["worker_bytes"].as_u64().unwrap_or(0) as f64 / 1048576.0, memory["workers"], report["reader"]["documents_held"], memory["threads"], report["record"]["wrote_bytes"], report["record"]["sleep"]["gone_dormant"], report["record"]["sleep"]["sleep_files_written"], report["record"]["sleep"]["sleep_bytes_written"], ids.len());
        assert!(report["reader"]["documents_held"].as_u64().unwrap() <= 3, "{}", report["reader"]);
    }
    println!("{TABS} tabs, {rounds} rounds, {edits} edits. Workers: at most {peak_workers} alive, {:.1} MiB between them.", peak_worker_bytes as f64 / 1048576.0);
    let sleeping: Vec<&(bool, u64, u64)> = wakes.iter().filter(|wake| wake.0).collect();
    let middle = |pick: fn(&(bool, u64, u64)) -> u64, at: f64| {
        let mut all: Vec<u64> = sleeping.iter().map(|wake| pick(wake)).collect();
        all.sort_unstable();
        all.get(((all.len().max(1) - 1) as f64 * at) as usize).copied().unwrap_or(0)
    };
    println!("wakes: {} of {} from a sleep file, first frame p50 {} us p95 {} us, sharp p50 {} us p95 {} us, blank frames {blank_frames}", sleeping.len(), wakes.len(), middle(|wake| wake.1, 0.5), middle(|wake| wake.1, 0.95), middle(|wake| wake.2, 0.5), middle(|wake| wake.2, 0.95));
    assert!(sleeping.len() * 2 >= wakes.len(), "most wakes are from a file: {} of {}", sleeping.len(), wakes.len());
    assert_eq!(blank_frames, 0, "no wake from a file or otherwise shows a bare page for long: {wakes:?}");
    let report = ok(&state, &["qa", "state"]);
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
    let dormancies: u64 = ["left_alone", "budget", "asked", "pressure"].iter().map(|why| report["record"]["sleep"]["gone_dormant"][*why].as_u64().unwrap_or(0)).sum();
    let budget = 64 * 1024 + edits * 2048 + dormancies * 256 * 1024;
    assert!(wrote <= budget, "wrote {wrote} bytes, against {budget}, with {edits} edits and {dormancies} tabs going dormant");
    let _ = state.json(&["qa", "quit"]);
    let _ = pid;
}
