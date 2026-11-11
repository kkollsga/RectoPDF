//! That a quiet run never shows anything, seen from outside the app: the window server's own list of what's on screen, sampled every few milliseconds through a run's start, its sheets and panels, Settings, a print and a shot.
//!
//! The app's own report can't catch this. Its window says it was never ordered in, and is, until AppKit orders it in to hang a sheet on it, slides it onto the screen from where it was, and takes it away again a moment later (owner, 2026-11-08: "When you are testing the app it flashes open in lower left corner").

use super::common::app::{OwnCopy, numbered};
use super::{Run, app_path};
use recto_platform::screen::{ShownWindow, windows_on_screen};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// Watches the screen on a thread of its own until it's asked what it saw, for any window of a process running this executable.
struct ScreenWatch {
    stop: Arc<AtomicBool>,
    looked: Arc<AtomicUsize>,
    thread: std::thread::JoinHandle<(Vec<ShownWindow>, usize, bool)>,
}

impl ScreenWatch {
    fn start(executable: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let looked = Arc::new(AtomicUsize::new(0));
        let counted = looked.clone();
        let thread = std::thread::spawn(move || {
            let mut seen: Vec<ShownWindow> = Vec::new();
            let mut samples = 0;
            let mut anything = false;
            let mut ours = std::collections::HashMap::<u32, bool>::new();
            while !flag.load(Ordering::SeqCst) {
                let shown = windows_on_screen().unwrap_or_default();
                anything |= !shown.is_empty();
                for window in shown {
                    let is_ours = *ours.entry(window.pid).or_insert_with(|| {
                        recto_platform::local::process_path(window.pid)
                            .and_then(|path| path.canonicalize().ok())
                            .is_some_and(|path| path == executable)
                    });
                    if is_ours && seen.len() < 100 {
                        seen.push(window);
                    }
                }
                samples += 1;
                counted.store(samples, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
            }
            (seen, samples, anything)
        });
        Self { stop, looked, thread }
    }

    /// How many times it has looked so far.
    fn looks(&self) -> usize {
        self.looked.load(Ordering::SeqCst)
    }

    /// Waits, up to 30 s, until it has looked `more` times since now: a step that could flash a window is watched while it's up,
    /// however starved the watcher is. In a loaded full check it once looked a single time through a whole run (2026-11-11).
    fn catch_up(&self, more: usize) {
        let want = self.looks() + more;
        let began = std::time::Instant::now();
        while self.looks() < want {
            assert!(
                began.elapsed() < Duration::from_secs(30),
                "the screen watcher looked {} times in 30 s",
                self.looks()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// What of ours was on screen, how many times it looked, and whether it saw anything at all.
    fn finish(self) -> (Vec<ShownWindow>, usize, bool) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

/// Waits, a bounded time, for the sheet the app reports to be this kind.
fn sheet_up(run: &Run, kind: &str) {
    super::wait_for(run, |state| state["app"]["sheet"]["kind"] == kind);
}

/// Failure: a quiet run puts a window on a screen, even for a frame: the window slid in from off screen to hang a sheet on (the owner saw it flash in the lower left), a panel of its own such as Open, or Settings. Or it takes the focus. The window server's list is sampled every few milliseconds through a run's start, an open, the print panel, a close prompt and Save As, Settings, a print to PDF, the window closing and coming back, and shots with a sheet up; none of the app's windows may be on it at any sample, and the app is never the active one.
#[test]
fn a_quiet_run_never_puts_a_window_on_screen() {
    let Some(app) = app_path() else { return };
    let watch = ScreenWatch::start(app.canonicalize().unwrap());
    let run = Run::start().unwrap();
    let file = numbered(3);
    let folder = recto_testpdf::TempDir::new("recto-quiet-shots-");
    std::fs::create_dir_all(&folder).unwrap();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let mut actives = vec![run.report()["app"]["is_active"].clone()];
    // The print panel, a sheet: held, never shown, and in the shot where it would hang.
    run.ok(&["qa", "menu", "file.print"]);
    sheet_up(&run, "print_panel");
    watch.catch_up(10);
    run.ok(&["qa", "sheet-answer", "cancel"]);
    super::wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
    // A close prompt, an alert, drawn into a shot from its own window.
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.ok(&["qa", "menu", "file.close"]);
    sheet_up(&run, "close_prompt");
    watch.catch_up(10);
    let shot = folder.join("quiet-sheet.png");
    let drawn = run.ok(&["qa", "shot", "--out", shot.to_str().unwrap()]);
    assert!(drawn["bytes"].as_u64().unwrap() > 0, "{drawn}");
    actives.push(run.report()["app"]["is_active"].clone());
    // Its Cancel, then Save As, which an ending of the prompt could put up next.
    run.ok(&["qa", "sheet-answer", "second"]);
    run.ok(&["qa", "menu", "file.save_as"]);
    sheet_up(&run, "save_panel");
    watch.catch_up(10);
    run.ok(&["qa", "sheet-answer", "cancel"]);
    // Settings, a window of its own, and a print, which goes through the system's printing.
    run.ok(&["qa", "settings", "open"]);
    watch.catch_up(10);
    run.ok(&["qa", "shot-settings", "--out", folder.join("quiet-settings.png").to_str().unwrap()]);
    run.ok(&["qa", "print-to-pdf", folder.join("quiet-print.pdf").to_str().unwrap()]);
    watch.catch_up(10);
    run.settled();
    run.ok(&["qa", "window", "close"]);
    run.ok(&["qa", "window", "open"]);
    watch.catch_up(10);
    run.settled();
    run.ok(&["qa", "shot", "--out", folder.join("quiet-after.png").to_str().unwrap()]);
    actives.push(run.report()["app"]["is_active"].clone());
    drop(run);
    let _ = std::fs::remove_dir_all(&folder);
    let (seen, samples, anything) = watch.finish();
    if !anything {
        // No screen in this session, as over SSH: nothing to see, and saying so rather than passing.
        eprintln!("skipped: the window server listed nothing on any screen");
        return;
    }
    assert!(samples > 50, "it looked often enough to see a flash: {samples} looks");
    assert!(
        seen.is_empty(),
        "a quiet run's windows were on screen, {} times in {samples} looks: {:?}",
        seen.len(),
        &seen[..seen.len().min(8)]
    );
    assert!(actives.iter().all(|active| active == false), "the app was never the active one: {actives:?}");
}

/// Failure: the system's opening the bundle again, as `open` does for whoever runs it, reaches a quiet run and makes it a regular app, with a Dock icon and a place in the menu bar, or opens its window. LaunchServices hands the reopen to the running app, and AppKit's own handling of it turned a quiet run regular (measured 2026-11-08, and seen in a full check's restore test while another worktree's update tests were opening their copies). Launch Services itself makes the run regular before it hands over the reopen, and the run's guard puts it back about a millisecond later (2026-11-11, from the change's own stack), so a report read the policy in that moment: 1 of 200 on CI (run 43085841110), here 9 of 1,800 with 240 reopens, read at the end of a report's build after the guard had looked at its start. The run is opened again every few looks, and every look says prohibited, never active and off every screen, for two seconds.
#[test]
fn a_quiet_run_stays_quiet_when_the_system_opens_it_again() {
    // A copy of its own: the system's open reaches a run of the bundle opened, and with the build's it could be any other test's (likeliest for CI run 43085841110, where another test's report said `regular`: nothing else in the suite opens the build).
    let Some(copy) = OwnCopy::new() else { return };
    let (bundle, app) = (copy.bundle.clone(), copy.executable());
    let watch = ScreenWatch::start(app.canonicalize().unwrap());
    let run = Run::start_of(&app, Duration::from_secs(120), &[]);
    // On this run's own state and quiet, should it start a copy rather than reach the one running.
    let open_again = || {
        let opened = std::process::Command::new("/usr/bin/open")
            .arg("-g")
            .arg("--env")
            .arg(format!("{}={}", recto_control::state::STATE_ENV, run.state.root.display()))
            .arg(&bundle)
            .args(["--args", "--qa"])
            .output()
            .unwrap();
        assert!(opened.status.success(), "{opened:?}");
    };
    open_again();
    let began = std::time::Instant::now();
    let mut looks = 0;
    // Two seconds, a few reports and enough looks at the screen, however long each takes: on CI's slower machine five reports took the
    // whole two seconds, and in a loaded full check the watcher looked once while 24 reports were taken (2026-11-11).
    while began.elapsed() < Duration::from_secs(2) || looks <= 5 || watch.looks() <= 50 {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "{looks} reports and {} looks in a minute",
            watch.looks()
        );
        let app = run.report()["app"].clone();
        assert_eq!(
            (app["activation"].clone(), app["is_active"].clone(), app["window"]["is_visible"].clone()),
            (serde_json::json!("prohibited"), serde_json::json!(false), serde_json::json!(false)),
            "opened again, it stays quiet: {app}"
        );
        looks += 1;
        if looks % 4 == 0 {
            open_again();
        } else {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // Reached, rather than a copy started beside it: the run says the system made it regular and it went back.
    let messages = std::fs::read_to_string(&run.state.paths.server_messages).unwrap_or_default();
    drop(run);
    drop(copy);
    let (seen, samples, anything) = watch.finish();
    assert!(messages.contains("changed to regular from outside, and put back"), "{messages}");
    if !anything {
        eprintln!("skipped: the window server listed nothing on any screen");
        return;
    }
    assert!(looks > 5 && samples > 50, "{looks} reports, {samples} looks");
    assert!(seen.is_empty(), "a quiet run's windows were on screen: {:?}", &seen[..seen.len().min(8)]);
}

/// Waits for a run's process to be gone, and fails with what it was waiting for.
fn ends(pid: u32, why: &str) {
    let began = std::time::Instant::now();
    while super::still_running(pid) {
        assert!(began.elapsed() < Duration::from_secs(15), "the quiet run didn't end when {why}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Failure (PLAN, "A quiet QA app outlives a test or script that stopped without saying so"): three quiet runs ran on, orphaned, for up to their limit after their tests had gone and their worktrees been removed, using memory on the shared Mac. A run started for a process ends when that process does, whatever ended it, and any run ends when its state folder goes. A process that isn't running is refused as an owner, as the run would end as it starts.
#[test]
fn a_quiet_run_ends_with_what_started_it() {
    let Some(app) = app_path() else { return };
    let app = app.to_str().unwrap().to_owned();
    let mut owner = std::process::Command::new("/bin/sleep").arg("120").spawn().unwrap();
    let state = super::common::State::new();
    let owner_pid = owner.id().to_string();
    let (reply, code) = state.json(&["qa", "start", "--app", &app, "--time-limit", "2m", "--owner", &owner_pid]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["data"]["owner"].as_u64(), Some(u64::from(owner.id())));
    let pid = reply["data"]["pid"].as_u64().unwrap() as u32;
    owner.kill().unwrap();
    owner.wait().unwrap();
    ends(pid, "the process it was started for was killed");

    // Started by a test, by the test's own number, which `State::command` gives every run: its state folder going ends it too, as a worktree's removal takes the folder or the program.
    let state = super::common::State::new();
    let (reply, code) = state.json(&["qa", "start", "--app", &app, "--time-limit", "2m"]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["data"]["owner"].as_u64(), Some(u64::from(std::process::id())), "{reply}");
    let pid = reply["data"]["pid"].as_u64().unwrap() as u32;
    let moved = state.root.with_extension("gone");
    std::fs::rename(&state.root, &moved).unwrap();
    ends(pid, "its state folder went");
    let _ = std::fs::remove_dir_all(&moved);

    let state = super::common::State::new();
    let (reply, code) = state.json(&["qa", "start", "--app", &app, "--owner", "999999"]);
    assert_ne!(code, 0);
    assert_eq!(reply["error"]["code"], "owner_gone", "{reply}");
}

/// Failure: something outside the app changes a quiet run's activation policy, and a report or a settle look taken before the guard hears of it says `accessory`, failing a test that compares states (`an_installer_killed_after_the_swap_loses_nothing`, in a full check, 2026-11-11), with the run off its rules until the guard catches up. Here the run turns itself `accessory` every 15 ms, as that change would, and every look says `prohibited`.
///
#[test]
fn a_quiet_run_is_prohibited_whenever_its_looked_at() {
    let state = super::common::State::new();
    let Some(run) = Run::on_with_env(&state, &[("RECTO_QA_POLICY_NUDGE_MS", "15".as_ref())]) else {
        return;
    };
    let mut said = std::collections::BTreeMap::<String, usize>::new();
    for look in 0..200 {
        if look % 10 == 0 {
            run.settled();
        }
        let activation = run.report()["app"]["activation"].as_str().unwrap_or("none").to_string();
        *said.entry(activation).or_default() += 1;
    }
    assert_eq!(said.keys().collect::<Vec<_>>(), ["prohibited"], "what 200 reports said: {said:?}");
    // It was changed, and put back: not a run the nudge never reached.
    let messages = std::fs::read_to_string(&state.paths.server_messages).unwrap_or_default();
    assert!(messages.contains("changed to accessory from outside, and put back"), "{messages}");
}
