//! **What the app's life does at a launch, a quit and a hand-off, step by step, pinned before it moves** (R3.7, S3; AGENTS.md, "guards go in before a move"):
//! a launch with the state on time; the state late and the window made again from it; the record's lock late and kept; late and never let go; a quit with
//! nothing unsaved; a quit that asks and stays; a hand-off, its undoing, one refused while a sheet is up, and its quit told once. After each step: what the host
//! was told of the life, in order (the window's go-ahead, the quit's answer, the quit for a hand-off; not the model's redraws, which aren't the life's), the
//! life as it stands, the two freezes, the notice, whether the store keeps the record, what was written (records and log appends), and what the updater was
//! told (its status, whether it was told this is a second copy, and what the installer's launch-ok says). Held against `golden/life.txt`.

use super::coverage::heard_kind;
use super::late_lock::{a_saved_session, next_try_fails, not_kept_words, the_copy_before};
use crate::lifecycle::keeping::{Keeping, TRIES, TRY_EVERY_MS};
use crate::testkit::Heard;
use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{OLD, UpdateBed};
use recto_platform::slow::Seam;
use std::sync::atomic::Ordering;

/// The script's lines, as the scenarios add them: each step after the one before, with what the host heard since.
struct Script {
    lines: Vec<String>,
    heard: usize,
    writes: (u64, u64),
}

impl Script {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            heard: 0,
            writes: (0, 0),
        }
    }

    /// A new app's first line: what it heard and wrote is counted from its start.
    fn launched(&mut self, scenario: &str, app: &HeadlessApp, disk_updated: Option<&Disk>) {
        self.heard = 0;
        self.writes = (0, 0);
        self.step(scenario, "launched", app, disk_updated);
    }

    /// The line for a step just taken, with the app settled.
    fn step(&mut self, scenario: &str, step: &str, app: &HeadlessApp, disk_updated: Option<&Disk>) {
        app.settle();
        let inner = app.core();
        let heard = app.heard();
        let told: Vec<String> = heard[self.heard..]
            .iter()
            .filter(|heard| matches!(heard, Heard::RestoreReady | Heard::QuitAnswer(_) | Heard::HandOffQuit | Heard::SessionEnded))
            .map(|heard| match heard {
                Heard::QuitAnswer(answer) => format!("quit_answer(go={}, asks={})", answer.go, !answer.quit_button.is_empty()),
                other => heard_kind(other).to_owned(),
            })
            .collect();
        self.heard = heard.len();
        let writes = inner.saver.store.writes();
        let wrote = (writes.0 - self.writes.0, writes.1 - self.writes.1);
        self.writes = writes;
        let life = inner.life.life();
        let keeping = match life.keeping() {
            Keeping::Trying { tries, asking, by_update, .. } => format!("Trying(tries={tries}, asking={asking}, by_update={by_update})"),
            Keeping::Stopped { tries, asking, by_update, .. } => format!("Stopped(tries={tries}, asking={asking}, by_update={by_update})"),
            Keeping::Restoring { claimed, .. } => format!("Restoring(claimed={claimed})"),
            other => format!("{other:?}"),
        };
        let notice = match app.reader().note {
            None => "none".to_owned(),
            Some(words) if Some(&words) == not_kept_words().as_ref() => "not kept".to_owned(),
            Some(_) => "other".to_owned(),
        };
        let updater = disk_updated.map_or_else(String::new, |disk| {
            let status = app.update_status();
            let launch_ok = std::fs::read_to_string(disk.update_files().launch_ok())
                .ok()
                .and_then(|text| recto_update::plan::read_launch_ok(&text).map(|(version, tabs)| format!("{version} tabs={tabs:?}")))
                .unwrap_or_else(|| "none".to_owned());
            format!(
                " | updater {} version={} second_copy={} launch_ok {launch_ok}",
                status["status"].as_str().unwrap_or("?"),
                status["version"],
                status["second_copy"]
            )
        });
        self.lines.push(format!(
            "{scenario}: {step} | heard {} | life restored={} shown={:?} remade={} (host {}) hand_off={:?} quit={:?} frozen={} keeping {keeping} | takes input={} may remake={} | \
             freezes host={} queue={} | notice {notice} | keeps the record={} | wrote {} appended {}{updater}",
            if told.is_empty() { "nothing".to_owned() } else { told.join(" ") },
            life.restored(),
            life.shown(),
            life.remade(),
            crate::lock(&app.host).remade,
            life.hand_off(),
            life.quit(),
            life.frozen(),
            inner.life.takes_input(),
            inner.life.may_remake_window(),
            inner.host.handing_off.load(Ordering::SeqCst),
            inner.core.frozen.load(Ordering::SeqCst),
            inner.saver.store.holds_the_record(),
            wrote.0,
            wrote.1,
        ));
    }
}

/// The state folder's own part that the record goes in, made so it can't be written, or writable again.
#[cfg(unix)]
fn record_folder_writable(app: &HeadlessApp, writable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let folder = app.core().saver.store.main_folder();
    std::fs::create_dir_all(&folder).expect("the record's folder");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(if writable { 0o700 } else { 0o500 })).expect("the record's folder");
}

/// A saved session on a disk with [`OLD`] installed, so the updater is told of each launch.
fn a_saved_session_and_an_app() -> (Disk, UpdateBed) {
    let disk = a_saved_session();
    disk.install_app(OLD);
    (disk, UpdateBed::new())
}

/// Every scenario, in order, a line a step.
fn script_lines() -> Vec<String> {
    let mut script = Script::new();

    // The state on time, then a quit with everything kept.
    let (disk, bed) = a_saved_session_and_an_app();
    let app = disk.builder().update(&bed).start();
    script.launched("on time", &app, Some(app.disk()));
    app.core().note_input();
    script.step("on time", "touched", &app, Some(app.disk()));
    let answer = app.ask_to_quit();
    assert!(answer.go, "{answer:?}");
    script.step("on time", "quit", &app, Some(app.disk()));
    let disk = app.quit_anyway();
    drop(disk);

    // The state late: the window up without it, then made again from it.
    let (disk, bed) = a_saved_session_and_an_app();
    let app = disk.builder().update(&bed).hold(&[Seam::State]).start();
    script.launched("state late", &app, Some(app.disk()));
    app.let_state_in();
    script.step("state late", "state in", &app, Some(app.disk()));
    drop(app.quit());

    // The record's lock late, then let go and kept: the saved session restored and the window made again.
    let (disk, bed) = a_saved_session_and_an_app();
    let before = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().hold(&[Seam::Remake]).start();
    script.launched("lock late, kept", &app, Some(app.disk()));
    next_try_fails(&app);
    script.step("lock late, kept", "a try that fails", &app, Some(app.disk()));
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("kept", || app.core().life.keeping() == Keeping::Keeps);
    script.step("lock late, kept", "let go and tried", &app, Some(app.disk()));
    app.wait_until("the host told", || app.waiting(Seam::Remake) > 0);
    app.let_through(Seam::Remake, 1);
    script.step("lock late, kept", "the window made again", &app, Some(app.disk()));
    drop(app.quit());

    // The record's lock late and never let go: said after the tries, and the updater told it's a second copy.
    let (disk, bed) = a_saved_session_and_an_app();
    let before = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    script.launched("lock late, not kept", &app, Some(app.disk()));
    for _ in 1..TRIES {
        next_try_fails(&app);
    }
    script.step("lock late, not kept", "all tries but the last", &app, Some(app.disk()));
    app.advance(TRY_EVERY_MS);
    app.wait_until("not kept", || app.core().life.keeping() == Keeping::NotKept);
    script.step("lock late, not kept", "the last try", &app, Some(app.disk()));
    let answer = app.ask_to_quit();
    assert!(answer.go, "{answer:?}");
    script.step("lock late, not kept", "quit", &app, Some(app.disk()));
    drop(app.quit_anyway());
    drop(before);

    // A quit that can't keep an edit: it asks, Stay, then asked again and gone.
    let disk = Disk::new();
    let doc = disk.pdf("edited.pdf", 3);
    let app = disk.start();
    app.open(&doc);
    app.act(crate::core::controls::UiAction::Menu { id: "rotate.right".into() });
    script.launched("quit stays", &app, None);
    record_folder_writable(&app, false);
    let asked = app.ask_to_quit();
    assert!(!asked.go, "{asked:?}");
    script.step("quit stays", "asked", &app, None);
    app.stay();
    script.step("quit stays", "stayed", &app, None);
    app.core().quit_stayed();
    script.step("quit stays", "stayed with nothing asked", &app, None);
    record_folder_writable(&app, true);
    let answer = app.ask_to_quit();
    assert!(answer.go, "{answer:?}");
    script.step("quit stays", "asked again, written", &app, None);
    drop(app.quit_anyway());

    // A hand-off: refused while a sheet's up, written, undone, written again, and its quit told once.
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 3);
    let app = disk.start();
    app.open(&doc);
    script.launched("hand-off", &app, None);
    app.core().sheet_changed(true);
    let refused = app.core().hand_off(std::time::Duration::from_secs(5));
    assert!(refused.is_err(), "refused with a sheet up");
    script.step("hand-off", "refused with a sheet up", &app, None);
    app.core().sheet_changed(false);
    app.core().hand_off(std::time::Duration::from_secs(5)).expect("handed off");
    script.step("hand-off", "written", &app, None);
    app.core().hand_off_cancelled();
    script.step("hand-off", "undone", &app, None);
    app.core().hand_off(std::time::Duration::from_secs(5)).expect("handed off again");
    script.step("hand-off", "written again", &app, None);
    assert!(app.core().hand_off_quit_now(), "told to quit");
    script.step("hand-off", "quit now", &app, None);
    assert!(app.core().hand_off_quit_now(), "still told");
    script.step("hand-off", "quit now again", &app, None);
    drop(app.kill());

    script.lines
}

/// Failure (R3.7, S3, a guard before the app's life moves off `Inner` and onto its own part): a launch, a quit or a hand-off tells the host something else or in
/// another order (the window's go-ahead twice, or not after a late state; a quit's answer that goes where it asked; the quit for a hand-off told twice or
/// not at all), leaves the life, a freeze or the notice otherwise, writes where it didn't (or not where it did), or tells the updater otherwise (a second copy
/// that isn't, or a launch-ok that's missing). Each step's line is held against the golden, naming the first that differs.
#[test]
fn the_apps_life_goes_as_it_did_step_by_step() {
    let now = script_lines();
    let golden: Vec<&str> = include_str!("golden/life.txt").lines().collect();
    if golden.iter().copied().eq(now.iter().map(String::as_str)) {
        return;
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recto-golden");
    let _ = std::fs::create_dir_all(&out);
    let written = out.join("life.txt");
    let _ = std::fs::write(&written, now.join("\n") + "\n");
    let first = golden.iter().zip(&now).find(|(was, is)| **was != is.as_str()).map_or_else(
        || format!("{} lines, the golden {}", now.len(), golden.len()),
        |(was, is)| format!("golden: {was}\nnow:    {is}"),
    );
    panic!("the app's life went otherwise:\n{first}\n(the whole script now: {})", written.display());
}
