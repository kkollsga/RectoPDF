//! **What moving the updates' methods off `Inner` must not change, pinned before the move** (R3.7, S4; AGENTS.md, "guards go in before a move"): what each
//! update command answers at each state, to the person and to an agent at two levels; what the host is told, in order, for each Settings switch, by the
//! person and by an agent; that the updater hears of the launch exactly once however keeping settles (the app's half of the pure walk in
//! `lifecycle::keeping`'s tests); and what an update writes for the installer. No wording is held: codes, exit codes, and the fields that say which sentence.

use super::coverage::heard_kind;
use super::late_lock::{handed_off_with_the_old_copy_holding, installer_sees, next_try_fails, state_but_the_launch_ok, the_copy_before};
use crate::core::controls::UiAction;
use crate::core::updates::{AUTO_UPDATE_ROW, DEV_BUILDS_ROW, UPDATE_NOW_ROW};
use crate::lifecycle::keeping::{Keeping, TRIES, TRY_EVERY_MS};
use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{DEV, Installed, NEW, OLD, UpdateBed};
use recto_agent::{Actor, AgentError, Level};
use recto_control::host::Host;
use recto_control::record_store::RecordStore;
use recto_platform::slow::{Seam, gate};
use serde_json::{Map, Value};

/// The states an update command is asked in. Not here: a check under way, which nothing in `recto-update` lets a test hold; its refusal is
/// `update_under_way`, the one an install under way gives too.
#[derive(Clone, Copy, Debug)]
enum State {
    /// Installed, nothing offered, nothing asked.
    Idle,
    /// A release newer than this one on offer, seen by a check.
    Offered,
    /// A development build handed over and checked, waiting for five quiet minutes.
    BuildWaiting,
    /// An install under way, waiting for a sheet to close.
    UnderWay,
    /// The launch held back from the updater while it tries for the record's lock: "still starting".
    Held,
    /// The tries ran out: another copy keeps the record.
    SecondCopy,
}

const STATES: [State; 6] = [
    State::Idle,
    State::Offered,
    State::BuildWaiting,
    State::UnderWay,
    State::Held,
    State::SecondCopy,
];

/// Who asks: the person, through the one executor as the window's commands run; an agent at Full; an agent at Control.
const ACTORS: [(&str, Option<Level>); 3] = [("person", None), ("full", Some(Level::Full)), ("control", Some(Level::Control))];

/// The update commands, in an order that leaves what's asked last to change the most: the looks, the setters, then a check and an install.
const COMMANDS: [&str; 9] = [
    r#"{"cmd":"update_status"}"#,
    r#"{"cmd":"update_dev_builds"}"#,
    r#"{"cmd":"settings"}"#,
    r#"{"cmd":"update_install"}"#,
    r#"{"cmd":"settings","auto_update":"off"}"#,
    r#"{"cmd":"update_dev_builds","set":"off"}"#,
    r#"{"cmd":"settings","auto_update":"on"}"#,
    r#"{"cmd":"update_check"}"#,
    r#"{"cmd":"update_install","now":true}"#,
];

/// An app on the update bed in `state`, with what holds it there (the copy before, keeping the record) and what setting it up answered, said.
fn in_state(state: State) -> (HeadlessApp, UpdateBed, Option<RecordStore>, Vec<String>) {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let mut setup = Vec::new();
    let held = matches!(state, State::Held | State::SecondCopy).then(|| the_copy_before(&disk));
    let app = if held.is_some() {
        disk.builder().update(&bed).another_keeps().start()
    } else {
        disk.builder().update(&bed).start()
    };
    let full = |app: &HeadlessApp, line: &str| said(&app.agent(Level::Full, line));
    match state {
        State::Idle => {}
        State::Offered => {
            bed.releases.offer(NEW);
            setup.push(full(&app, r#"{"cmd":"update_check"}"#));
        }
        State::BuildWaiting => {
            recto_control::levels::store(app.disk().paths(), Level::Full).expect("the level's file");
            setup.push(full(&app, r#"{"cmd":"update_dev_builds","set":"on"}"#));
            let zip = app.disk().hand_over_build(DEV);
            app.touch();
            setup.push(full(&app, &format!(r#"{{"cmd":"update_install","zip":{}}}"#, serde_json::json!(zip))));
        }
        State::UnderWay => {
            app.core().sheet_changed(true);
            bed.releases.offer(NEW);
            setup.push(full(&app, r#"{"cmd":"update_check"}"#));
            setup.push(full(&app, r#"{"cmd":"update_install"}"#));
            app.wait_for_status("waiting");
        }
        State::Held => {}
        State::SecondCopy => {
            for _ in 1..TRIES {
                next_try_fails(&app);
            }
            app.advance(TRY_EVERY_MS);
            app.wait_until("told it's a second copy", || app.update_status()["second_copy"] == true);
        }
    }
    app.settle();
    (app, bed, held, setup)
}

/// An answer as the golden holds it: a refusal's code, exit code and whether it says what to do next; an answer's fields, with its words, a title, a path
/// and a time only said to be there, as they're wording or this run's.
fn said(result: &Result<Value, AgentError>) -> String {
    match result {
        Err(error) => format!("err {} exit {} next {}", error.code, error.exit, !error.next.is_empty()),
        Ok(value) => format!("ok {}", shape(value)),
    }
}

fn shape(value: &Value) -> Value {
    let Value::Object(fields) = value else { return value.clone() };
    let mut out = Map::new();
    for (key, field) in fields {
        let kept = match (key.as_str(), field) {
            (_, Value::Null) => Value::Null,
            ("words" | "title" | "log" | "handover_dir" | "kept" | "last_update", _) => Value::from("<there>"),
            (key, _) if key.ends_with("_ms") => Value::from("<there>"),
            ("menu", menu) => serde_json::json!({ "enabled": menu["enabled"] }),
            _ => shape(field),
        };
        out.insert(key.clone(), kept);
    }
    Value::Object(out)
}

/// The golden's lines: each state, each actor on an app of its own, what setting it up answered and what each command answers in order.
fn answers_now() -> Vec<String> {
    let mut lines = Vec::new();
    for state in STATES {
        for (actor, level) in ACTORS {
            let (app, _bed, held, setup) = in_state(state);
            // A sheet open, so an install that starts waits for it and goes no further: each answer after it is given in one state, not wherever the
            // install had got to.
            app.core().sheet_changed(true);
            for answer in setup {
                lines.push(format!("{state:?} {actor} setup | {answer}"));
            }
            for line in COMMANDS {
                let command = recto_agent::parse(line).expect("a command");
                let result = match level {
                    None => app.core().host.run(Actor::Person, &command, &mut Vec::new()),
                    Some(level) => app.agent(level, line),
                };
                lines.push(format!("{state:?} {actor} {line} | {}", said(&result)));
                app.wait_until("what it started to stop where it goes no further", || {
                    !matches!(app.update_status()["status"].as_str(), Some("checking" | "preparing" | "installing"))
                });
                app.settle();
            }
            drop(app);
            drop(held);
        }
    }
    lines
}

/// **Lines held against a golden:** the first that differs, named with both; and the whole of what's there now in `target/recto-golden/<name>`, to compare.
fn against(golden: &str, now: &[String], name: &str) {
    let golden: Vec<&str> = golden.lines().collect();
    if golden.iter().copied().eq(now.iter().map(String::as_str)) {
        return;
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recto-golden");
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(out.join(name), now.join("\n") + "\n");
    let first = golden
        .iter()
        .zip(now)
        .position(|(was, is)| *was != is.as_str())
        .unwrap_or(golden.len().min(now.len()));
    panic!(
        "{name} differs from the golden at line {}:\n  golden: {}\n  now:    {}\n(all of now in target/recto-golden/{name}; {} lines against {})",
        first + 1,
        golden.get(first).unwrap_or(&"<none>"),
        now.get(first).map_or("<none>", String::as_str),
        now.len(),
        golden.len()
    );
}

const ANSWERS: &str = include_str!("golden/update_answers.txt");

/// Failure: the move changes what an update command answers to someone, at some state: a check let through while the launch is held back, an install
/// refused with another code, a setter that stops answering with its switch, a Control agent let past the level an install needs (decided in the agent's
/// route and the executor, which the move doesn't touch), or the person's answer drifting from the agent's. Held line by line against the golden of
/// 2026-11-14, before the move.
#[test]
fn every_update_command_answers_as_it_did_at_every_state() {
    against(ANSWERS, &answers_now(), "update_answers.txt");
}

/// What one Settings choice did: the person's commands it sent, the host's calls by kind in order, and the switches as Settings then shows them.
fn chosen(id: &str, on: bool, by_agent: Option<&str>) -> String {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let app = disk.builder().update(&bed).start();
    recto_control::levels::store(app.disk().paths(), Level::Full).expect("the level's file");
    bed.releases.offer(NEW);
    app.agent(Level::Full, r#"{"cmd":"update_check"}"#).expect("checked");
    // A sheet open, so *Update Now* starts an install that waits for it and goes no further.
    app.core().sheet_changed(true);
    app.settle();
    app.keep_sent();
    drop(app.listener().take());
    let answer = match by_agent {
        None => {
            let acted = app.act(UiAction::SettingChosen { id: id.to_owned(), on });
            format!("host {:?} understood {}", acted.host, acted.understood)
        }
        Some(line) => said(&app.agent(Level::Full, line)),
    };
    app.settle();
    app.wait_until("what it started to stop where it goes no further", || {
        !matches!(app.update_status()["status"].as_str(), Some("checking" | "preparing" | "installing"))
    });
    app.settle();
    let sent: Vec<String> = app.sent().iter().map(recto_agent::Command::wire).collect();
    let mut heard: Vec<&str> = app.listener().take().iter().map(heard_kind).collect();
    // An install's steps (preparing, then waiting) are each a change the host is told of, and two the session thread's look finds together are one call:
    // how many is the install's pace. The kinds, in order, are what's held for it, as the controls' golden holds a save's.
    if sent.iter().any(|wire| wire.contains("update_install")) {
        heard.dedup();
        heard = heard
            .into_iter()
            .map(|kind| if kind == "model_changed" { "model_changed+" } else { kind })
            .collect();
    }
    let model = app.core().settings_model();
    let rows: Vec<String> = model.rows.iter().map(|row| format!("{}={}/{}", row.id, row.on, row.enabled)).collect();
    format!(
        "{} | {answer} | sent {sent:?} | heard {heard:?} | rows {rows:?} action {:?}",
        by_agent.map_or_else(|| format!("{id} {on}"), str::to_owned),
        model.action.map(|action| action.enabled)
    )
}

/// Failure: a Settings switch, by the person or by an agent's command, stops redrawing the window, or redraws it twice, or sends another command, or one
/// that isn't a switch sends anything: the host told something other than it was, in another order. Each choice on an app of its own with a release
/// on offer, so *Update Now* has something to install.
#[test]
fn each_settings_choice_tells_the_host_what_it_did() {
    let person = [
        (AUTO_UPDATE_ROW, false),
        (AUTO_UPDATE_ROW, true),
        (DEV_BUILDS_ROW, true),
        (DEV_BUILDS_ROW, false),
        (UPDATE_NOW_ROW, true),
        ("update.nothing", true),
    ];
    let agent = [
        r#"{"cmd":"settings","auto_update":"off"}"#,
        r#"{"cmd":"update_dev_builds","set":"on"}"#,
        r#"{"cmd":"update_dev_builds","set":"off"}"#,
    ];
    let now: Vec<String> = person
        .iter()
        .map(|(id, on)| chosen(id, *on, None))
        .chain(agent.iter().map(|line| chosen("", false, Some(line))))
        .collect();
    against(SETTINGS, &now, "update_settings.txt");
}

/// What each choice did on 2026-11-14, before the move: a line each, as [`chosen`] makes them.
const SETTINGS: &str = include_str!("golden/update_settings.txt");

/// How many times the updater has been told of a launch: arrivals at its look, a seam held from before the start and let through from the first.
fn told() -> u64 {
    gate::arrived(Seam::Launch)
}

/// The updater's look at the launch counted from here, and never waited at.
fn count_the_tells() {
    gate::hold(&[Seam::Launch]);
    gate::let_through(Seam::Launch, 1_000);
}

/// Failure (the record's-lock retry, the app's half of `lifecycle::keeping`'s walk): the updater told of the launch twice, or never, on one of the ways
/// keeping settles: so an update is settled twice, counts an unstarted launch twice towards going back, or is never told at all and every check says
/// "still starting" for the rest of the run. Once whether the lock came at the start, came to a later try, never came, or the person worked in the window
/// first; never for a quit while it tries.
#[test]
fn the_updater_hears_of_the_launch_once_however_keeping_settles() {
    let bed = UpdateBed::new();
    let installed = || {
        let disk = Disk::new();
        disk.install_app(OLD);
        disk
    };
    // The lock at the start.
    count_the_tells();
    let app = installed().builder().update(&bed).start();
    app.settle();
    assert_eq!(told(), 1, "kept at the launch");
    drop(app.quit());

    // The lock at the second try. (Each app's end opens every seam, so the count is begun again for each.)
    count_the_tells();
    let disk = installed();
    let copy = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    assert_eq!(told(), 0, "held back while it tries");
    next_try_fails(&app);
    drop(copy);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.wait_until("told", || told() > 0);
    app.settle();
    assert_eq!(told(), 1, "kept by a late try");
    drop(app.quit());

    // The lock never comes.
    count_the_tells();
    let disk = installed();
    let copy = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    for _ in 1..TRIES {
        next_try_fails(&app);
    }
    app.advance(TRY_EVERY_MS);
    app.wait_until("told it's a second copy", || app.update_status()["second_copy"] == true);
    app.settle();
    assert_eq!(told(), 1, "a second copy");
    drop(app.quit());
    drop(copy);

    // The person works in the window before it comes.
    count_the_tells();
    let disk = installed();
    let copy = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    next_try_fails(&app);
    let file = app.disk().pdf("c.pdf", 2);
    app.open(&file);
    app.wait_until("told it's a second copy", || app.update_status()["second_copy"] == true);
    app.settle();
    assert_eq!(told(), 1, "worked in first");
    drop(app.quit());
    drop(copy);

    // A quit while it tries.
    count_the_tells();
    let disk = installed();
    let copy = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    next_try_fails(&app);
    let (answer, _disk) = app.quit();
    assert!(answer.go, "{answer:?}");
    assert_eq!(told(), 0, "a quit while it tries tells it nothing");
    drop(copy);
}

/// Failure (the coordinator's condition on S4): the app ends while the updater is being told of a launch held back, and the tell, going on without it, writes
/// what only a copy that keeps the record writes: a launch-ok with a tab count, or the install settled (`updates.json` or the plan changed). The tell held at
/// the updater's look, the app ended there (as a process's end opens every seam), the tell goes on: the installer at most hears it started, with no tab count.
#[test]
fn an_app_ended_while_the_updater_is_told_writes_nothing_a_kept_copy_would() {
    let (disk, bed, _plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let was = state_but_the_launch_ok(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    crate::core::updates::first_frame_shown(app.core());
    for _ in 1..crate::lifecycle::keeping::TRIES_AFTER_UPDATE {
        next_try_fails(&app);
    }
    gate::hold(&[Seam::Launch]);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the updater's look at the launch held", || app.waiting(Seam::Launch) > 0);
    let disk = app.kill();
    let files = disk.update_files();
    let launch_ok = std::fs::read_to_string(files.launch_ok()).ok();
    let said = launch_ok.as_deref().and_then(recto_update::plan::read_launch_ok);
    assert!(said.is_none_or(|(_, tabs)| tabs.is_none()), "no tab count: {launch_ok:?}");
    let now = (
        super::late_lock::record_folder(&disk),
        std::fs::read(&files.saved).ok(),
        std::fs::read(files.plan()).ok(),
    );
    assert!(now == was, "the record, updates.json and the plan as they were");
    drop(old_on_its_way_out);
}

/// What an update by hand writes for the installer and the next launch, made the same on every run: the plan's fields, `updates.json` as the new version
/// leaves it once it's up, and the launch-ok's version and tab count.
fn written_by_an_update() -> Vec<String> {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let file = disk.pdf("alpha.pdf", 5);
    let app = disk.builder().update(&bed).start();
    app.open(&file);
    bed.releases.offer(NEW);
    app.agent(Level::Full, r#"{"cmd":"update_check"}"#).expect("checked");
    app.agent(Level::Full, r#"{"cmd":"update_install"}"#).expect("installing");
    let (plan, disk) = app.handed_off(&bed);
    let root = disk.paths().root.display().to_string();
    let app_dir = disk.app().display().to_string();
    let clean = |text: String| text.replace(&app_dir, "<app>").replace(&root, "<state>");
    let mut lines = vec![clean(format!(
        "plan from {} to {} tabs {:?} background {} development {} keep_record {}",
        plan.from, plan.to, plan.handed_off_tabs, plan.background, plan.development_build, plan.keep_record
    ))];
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    crate::core::updates::first_frame_shown(app.core());
    app.wait_for("the installer to see it up", || installer_sees(app.disk(), &plan));
    app.settle();
    let launch_ok = std::fs::read_to_string(app.disk().update_files().launch_ok()).unwrap_or_default();
    lines.push(format!("launch-ok {:?}", recto_update::plan::read_launch_ok(&launch_ok)));
    let saved: Value = serde_json::from_slice(&std::fs::read(app.disk().update_files().saved).unwrap_or_default()).unwrap_or(Value::Null);
    lines.push(format!("updates.json {}", shape(&saved)));
    let (_, disk) = app.quit();
    lines.push(format!("plan left {}", disk.update_files().plan().exists()));
    lines
}

/// Failure: the move changes what an update writes for the installer or what the new version leaves once it's up: the plan's tabs or flags, the launch-ok's
/// version or tab count, the version proven in `updates.json`. Held against the golden of 2026-11-14, times and paths said only to be there.
#[test]
fn an_update_writes_what_it_did() {
    let now = written_by_an_update();
    against(WRITTEN, &now, "update_written.txt");
}

const WRITTEN: &str = include_str!("golden/update_written.txt");
