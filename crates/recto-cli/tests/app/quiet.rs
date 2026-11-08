//! That a quiet run never shows anything, seen from outside the app: the window server's own list of what's on screen, sampled every few milliseconds through a run's start, its sheets and panels, Settings, a print and a shot.
//!
//! The app's own report can't catch this. Its window says it was never ordered in, and is, until AppKit orders it in to hang a sheet on it, slides it onto the screen from where it was, and takes it away again a moment later (owner, 2026-11-08: "When you are testing the app it flashes open in lower left corner").

use super::{Run, app_path, long_document};
use recto_platform::screen::{ShownWindow, windows_on_screen};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Watches the screen on a thread of its own until it's asked what it saw, for any window of a process running this executable.
struct ScreenWatch {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<(Vec<ShownWindow>, usize, bool)>,
}

impl ScreenWatch {
    fn start(executable: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut seen: Vec<ShownWindow> = Vec::new();
            let mut samples = 0;
            let mut anything = false;
            let mut ours = std::collections::HashMap::<u32, bool>::new();
            while !flag.load(Ordering::SeqCst) {
                let shown = windows_on_screen().unwrap_or_default();
                anything |= !shown.is_empty();
                for window in shown {
                    let is_ours = *ours.entry(window.pid).or_insert_with(|| recto_platform::local::process_path(window.pid).and_then(|path| path.canonicalize().ok()).is_some_and(|path| path == executable));
                    if is_ours && seen.len() < 100 {
                        seen.push(window);
                    }
                }
                samples += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            (seen, samples, anything)
        });
        Self { stop, thread }
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
    let file = long_document(3);
    let folder = std::env::temp_dir().join(format!("recto-quiet-shots-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let mut actives = vec![run.report()["app"]["is_active"].clone()];
    // The print panel, a sheet: held, never shown, and in the shot where it would hang.
    run.ok(&["qa", "menu", "file.print"]);
    sheet_up(&run, "print_panel");
    run.ok(&["qa", "sheet-answer", "cancel"]);
    super::wait_for(&run, |state| state["app"]["sheet"].as_object().is_none_or(|sheet| sheet.is_empty()));
    // A close prompt, an alert, drawn into a shot from its own window.
    run.ok(&["qa", "owner-edit", "rotate:1"]);
    run.ok(&["qa", "menu", "file.close"]);
    sheet_up(&run, "close_prompt");
    let shot = folder.join("quiet-sheet.png");
    let drawn = run.ok(&["qa", "shot", "--out", shot.to_str().unwrap()]);
    assert!(drawn["bytes"].as_u64().unwrap() > 0, "{drawn}");
    actives.push(run.report()["app"]["is_active"].clone());
    // Its Cancel, then Save As, which an ending of the prompt could put up next.
    run.ok(&["qa", "sheet-answer", "second"]);
    run.ok(&["qa", "menu", "file.save_as"]);
    sheet_up(&run, "save_panel");
    run.ok(&["qa", "sheet-answer", "cancel"]);
    // Settings, a window of its own, and a print, which goes through the system's printing.
    run.ok(&["qa", "settings", "open"]);
    run.ok(&["qa", "shot-settings", "--out", folder.join("quiet-settings.png").to_str().unwrap()]);
    run.ok(&["qa", "print-to-pdf", folder.join("quiet-print.pdf").to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "window", "close"]);
    run.ok(&["qa", "window", "open"]);
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
    assert!(seen.is_empty(), "a quiet run's windows were on screen, {} times in {samples} looks: {:?}", seen.len(), &seen[..seen.len().min(8)]);
    assert!(actives.iter().all(|active| active == false), "the app was never the active one: {actives:?}");
}
