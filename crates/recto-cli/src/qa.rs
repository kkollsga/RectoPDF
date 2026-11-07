//! Quiet runs of the app: starting one, running a script against it, and measuring it. The app does the work and refuses unless started this way. All of it stays on a temp state folder, and nothing here posts an event, takes focus or touches a screen.

use recto_agent::table::{Invocation, parse_cli};
use recto_agent::{AgentError, Level, exit};
use recto_control::client::Client;
use recto_control::state::STATE_ENV;
use recto_control::{StatePaths, levels};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const READY_WAIT: Duration = Duration::from_secs(30);
const DEFAULT_SCRIPT_LIMIT: Duration = Duration::from_secs(120);

fn failed(code: &'static str, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message.into(), exit::FAILED)
}

/// The app's executable: what was asked for, or beside this `recto` as in the bundle, or the one built in this checkout.
fn find_app(given: Option<&Path>) -> Result<PathBuf, AgentError> {
    if let Some(path) = given {
        return Ok(path.to_path_buf());
    }
    let this = std::env::current_exe().map_err(|_| failed("app_missing", "This program can't tell where it is."))?;
    let here = this.parent().map(Path::to_path_buf).unwrap_or_default();
    let beside = here.join("RectoPDF");
    if beside.is_file() {
        return Ok(beside);
    }
    for dir in here.ancestors() {
        let built = dir.join("app/build/RectoPDF.app/Contents/MacOS/RectoPDF");
        if built.is_file() {
            return Ok(built);
        }
    }
    Err(failed("app_missing", "The app isn't built.").then("Run `tools/sysguard/heavy.sh app/build.sh`, or say where it is with `--app`."))
}

/// A quiet run's state must be a folder someone set on purpose, because the level there goes straight to Full.
fn quiet_paths() -> Result<StatePaths, AgentError> {
    let paths = StatePaths::from_env()?;
    if !paths.overridden {
        return Err(AgentError::new("needs_temp_state", "A QA run needs its own state folder, and won't use the real one.", exit::PERMISSION).then(format!("Set {STATE_ENV} to a new folder in a temp folder.")));
    }
    Ok(paths)
}

/// What a quiet run is started with, past `--qa`.
#[derive(Default)]
struct Launch {
    memory_limit_mib: Option<u64>,
    time_limit: Option<Duration>,
    open: Vec<PathBuf>,
}

/// Starts the app and waits for its ready line.
fn launch(app: &Path, paths: &StatePaths, options: &Launch) -> Result<(Child, Value), AgentError> {
    paths.prepare()?;
    levels::store(paths, Level::Full).map_err(|_| failed("write_failed", "The state folder couldn't be written to."))?;
    let messages = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(&paths.server_messages).map_err(|_| failed("write_failed", "The state folder couldn't be written to."))?;
    let mut command = Command::new(app);
    command.arg("--qa");
    if let Some(limit) = options.memory_limit_mib {
        command.arg("--qa-memory-limit-mib").arg(limit.to_string());
    }
    if let Some(limit) = options.time_limit {
        command.arg("--qa-time-limit-ms").arg(limit.as_millis().to_string());
    }
    command.args(&options.open);
    command.env(STATE_ENV, &paths.root).env("MallocSpaceEfficient", "1").env("MallocMaxMagazines", "1").current_dir(&paths.root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(messages);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().map_err(|_| failed("app_missing", format!("The app at {} couldn't be started.", app.display())))?;
    let stdout = child.stdout.take().ok_or_else(|| failed("app_refused", "The app couldn't be started."))?;
    let (send, receive) = mpsc::channel();
    let _ = std::thread::Builder::new().name("qa-ready".into()).spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line);
        let _ = send.send(read.map(|_| line));
    });
    let said = receive.recv_timeout(READY_WAIT);
    let line = match said {
        Ok(Ok(line)) if !line.is_empty() => line,
        Ok(_) => {
            let _ = child.wait();
            let message = std::fs::read_to_string(&paths.server_messages).unwrap_or_default();
            return Err(failed("app_refused", if message.trim().is_empty() { "The app ended before it was ready.".to_string() } else { message.trim().to_string() }));
        }
        Err(_) => {
            let _ = child.kill();
            return Err(AgentError::new("timed_out", "The app wasn't ready in time, so it was stopped.", exit::TIMED_OUT));
        }
    };
    let value: Value = serde_json::from_str(line.trim()).map_err(|_| failed("app_refused", "The app said something that isn't its ready line."))?;
    if value["ok"] != true {
        return Err(failed("app_refused", value["error"]["message"].as_str().unwrap_or("The app refused to start.").to_string()));
    }
    Ok((child, value["data"].clone()))
}

/// `recto qa start`.
pub fn start(invocation: &Invocation) -> Result<Value, AgentError> {
    let paths = quiet_paths()?;
    if Client::connect(&paths).is_ok() {
        return Err(failed("already_running", "Something is already running on this state folder.").then("`recto stop` ends it."));
    }
    let app = find_app(invocation.fields.get("app").and_then(Value::as_str).map(Path::new))?;
    let options = Launch {
        memory_limit_mib: invocation.fields.get("memory_limit_mib").and_then(Value::as_u64),
        time_limit: invocation.fields.get("time_limit_ms").and_then(Value::as_u64).map(Duration::from_millis),
        open: Vec::new(),
    };
    let (_child, ready) = launch(&app, &paths, &options)?;
    Ok(json!({ "running": true, "pid": ready["pid"], "socket": ready["socket"], "state": paths.root, "quiet": true }))
}

/// Splits a line of a script as a shell would for the simple cases there are: spaces, double quotes, and `$NAME` from the environment.
fn script_words(line: &str) -> Result<Vec<String>, String> {
    let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '$' => {
                let mut name = String::new();
                while let Some(next) = chars.peek().filter(|next| next.is_ascii_alphanumeric() || **next == '_') {
                    name.push(*next);
                    chars.next();
                }
                word.push_str(&std::env::var(&name).map_err(|_| format!("${name} isn't set."))?);
                any = true;
            }
            ' ' | '\t' if !quoted => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            other => {
                word.push(other);
                any = true;
            }
        }
    }
    if quoted {
        return Err("A quote isn't closed.".to_string());
    }
    if any {
        words.push(word);
    }
    Ok(words)
}

/// Follows `reader.tiles.attached` into an answer.
fn at_path<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, key| match key.parse::<usize>() {
        Ok(index) if value.is_array() => &value[index],
        _ => &value[key],
    })
}

/// What one `expect` line says about the last answer. `None` if it holds.
fn check(last: &Value, words: &[String]) -> Option<String> {
    let [kind, path, wanted] = words else { return Some("An expect line is `expect PATH VALUE`.".to_string()) };
    let found = at_path(last, path);
    let number = |value: &Value| value.as_f64();
    let wanted_value: Value = serde_json::from_str(wanted).unwrap_or_else(|_| Value::String(wanted.clone()));
    let holds = match kind.as_str() {
        "expect" => *found == wanted_value || (number(found).is_some() && number(found) == number(&wanted_value)),
        "expect-at-least" => number(found).zip(number(&wanted_value)).is_some_and(|(found, wanted)| found >= wanted),
        "expect-at-most" => number(found).zip(number(&wanted_value)).is_some_and(|(found, wanted)| found <= wanted),
        "expect-contains" => found.as_str().is_some_and(|text| text.contains(wanted.as_str())),
        other => return Some(format!("`{other}` isn't something a script can expect.")),
    };
    (!holds).then(|| format!("{path} is {found}, and {kind} said {wanted}."))
}

/// `recto qa run`: PASS or FAIL for the script, and a line for each thing that failed.
pub fn run_script(invocation: &Invocation) -> Result<Value, AgentError> {
    let script = PathBuf::from(invocation.fields.get("script").and_then(Value::as_str).unwrap_or_default());
    let limit = invocation.fields.get("time_limit_ms").and_then(Value::as_u64).map_or(DEFAULT_SCRIPT_LIMIT, Duration::from_millis);
    let text = std::fs::read_to_string(&script).map_err(|_| failed("not_found", format!("The script {} couldn't be read.", script.display())))?;
    let paths = quiet_paths()?;
    let started = Instant::now();
    let mut failures = Vec::new();
    let mut steps = 0;
    let mut last = Value::Null;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if started.elapsed() > limit {
            failures.push(json!({ "line": index + 1, "why": "The script ran out of time." }));
            break;
        }
        steps += 1;
        let words = match script_words(line) {
            Ok(words) => words,
            Err(why) => {
                failures.push(json!({ "line": index + 1, "why": why }));
                continue;
            }
        };
        if words.first().is_some_and(|word| word.starts_with("expect")) {
            if let Some(why) = check(&last, &words) {
                failures.push(json!({ "line": index + 1, "why": why }));
            }
            continue;
        }
        let parsed = match parse_cli(&words) {
            Ok(parsed) => parsed,
            Err(error) => {
                failures.push(json!({ "line": index + 1, "why": error.message }));
                continue;
            }
        };
        let answered = Client::connect(&paths).and_then(|mut client| client.call(&parsed.wire(), Duration::from_secs(parsed.spec.client_wait_s) + limit));
        match answered {
            Ok(value) if value["ok"] == true => last = value["data"].clone(),
            Ok(value) => {
                last = value["error"].clone();
                failures.push(json!({ "line": index + 1, "why": value["error"]["message"] }));
            }
            Err(error) => failures.push(json!({ "line": index + 1, "why": error.message })),
        }
    }
    // A hang fails the run, wherever in it that happened.
    match Client::connect(&paths).and_then(|mut client| client.call(&json!({ "cmd": "qa_read", "verb": "state" }).to_string(), Duration::from_secs(40))) {
        Ok(state) if state["ok"] == true => {
            let stalls = state["data"]["hangs"]["over_2s"].as_u64().unwrap_or(0);
            if stalls > 0 {
                failures.push(json!({ "line": 0, "why": format!("The UI thread stalled for 2 s or more, {stalls} time(s), the worst {} ms.", state["data"]["hangs"]["worst_ms"]) }));
            }
            if state["data"]["app"]["is_active"] == true {
                failures.push(json!({ "line": 0, "why": "The app became the active app, which a quiet run must never do." }));
            }
        }
        _ => failures.push(json!({ "line": 0, "why": "The app didn't answer at the end of the script." })),
    }
    let passed = failures.is_empty();
    let result = json!({ "script": script, "passed": passed, "steps": steps, "took_ms": started.elapsed().as_millis() as u64, "failed": failures });
    if passed {
        Ok(result)
    } else {
        let mut error = failed("qa_failed", format!("FAIL {}: {}", script.display(), failures.iter().map(|failure| format!("line {}: {}", failure["line"], failure["why"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ")));
        error.next = "Fix what it names and run it again.".to_string();
        Err(error)
    }
}

/// One quiet run for measuring, ended when it's dropped. Its state folder goes too.
struct Measured {
    child: Child,
    paths: StatePaths,
    root: PathBuf,
    pid: u32,
}

impl Measured {
    fn start(app: &Path, options: &Launch) -> Result<Self, AgentError> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = recto_platform::local::system_temp_dir().ok_or_else(|| failed("no_temp", "There's no temp folder."))?.join(format!("rqb{}-{number}", std::process::id()));
        let paths = StatePaths::overridden(&root)?;
        let (child, ready) = launch(app, &paths, options)?;
        let pid = ready["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).unwrap_or_else(|| child.id());
        Ok(Self { child, paths, root, pid })
    }

    fn call(&self, line: Value) -> Result<Value, AgentError> {
        let reply = Client::connect(&self.paths)?.call(&line.to_string(), Duration::from_secs(150))?;
        if reply["ok"] == true {
            Ok(reply["data"].clone())
        } else {
            Err(failed("qa_failed", format!("{line} said: {}", reply["error"]["message"])))
        }
    }

    fn settled(&self) -> Result<f64, AgentError> {
        let started = Instant::now();
        self.call(json!({ "cmd": "qa_read", "verb": "settled", "timeout_ms": 60_000 }))?;
        Ok(started.elapsed().as_secs_f64() * 1000.0)
    }

    fn state(&self) -> Result<Value, AgentError> {
        self.call(json!({ "cmd": "qa_read", "verb": "state" }))
    }

    fn drive(&self, verb: &str, value: &str) -> Result<Value, AgentError> {
        self.call(json!({ "cmd": "qa_drive", "verb": verb, "value": value }))
    }
}

impl Drop for Measured {
    fn drop(&mut self) {
        let _ = self.call(json!({ "cmd": "stop" }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn mib(bytes: u64) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0) * 100.0).round() / 100.0
}

fn percentile(sorted: &[f64], share: f64) -> f64 {
    sorted.get(((sorted.len().saturating_sub(1)) as f64 * share).round() as usize).copied().unwrap_or(0.0)
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// `recto qa select-budget`: what selecting costs on one file, on the path the mouse takes, with the numbers as they are under whatever else this Mac is doing.
pub fn select_budget(invocation: &Invocation) -> Result<Value, AgentError> {
    let app = find_app(invocation.fields.get("app").and_then(Value::as_str).map(Path::new))?;
    let file = PathBuf::from(invocation.fields.get("file").and_then(Value::as_str).unwrap_or_default());
    let run = Measured::start(&app, &Launch { open: vec![file.clone()], ..Launch::default() })?;
    run.settled()?;
    let memory = |state: &Value, key: &str| state["memory"][key].as_u64().unwrap_or(0);
    let before = run.state()?;
    let pages = before["reader"]["pages"].as_u64().unwrap_or(1);
    // A place with text on it, as `recto select` says, on a page with some.
    let (mut page, mut place) = (1, None);
    while page <= pages.min(32) && place.is_none() {
        let found = run.call(json!({ "cmd": "select", "page": page }))?;
        place = found["pages"][0]["rects"].as_array().and_then(|rects| rects.get(rects.len() / 2)).map(|rect| (rect[0].as_f64().unwrap_or(0.0) + rect[2].as_f64().unwrap_or(0.0) / 2.0, rect[1].as_f64().unwrap_or(0.0) + rect[3].as_f64().unwrap_or(0.0) / 2.0));
        if place.is_none() {
            page += 1;
        }
    }
    let (x, y) = place.ok_or_else(|| failed("qa_failed", "No page near the start of this file has text to select."))?;
    run.call(json!({ "cmd": "goto", "page": page }))?;
    run.settled()?;

    // The first double-click on a page with no text read: to the highlight, through one client round trip.
    let started = Instant::now();
    run.drive("mouse_down", &format!("{page}:{x},{y}:2"))?;
    run.drive("mouse_up", "all")?;
    run.settled()?;
    let first_word_ms = started.elapsed().as_secs_f64() * 1000.0;

    // A drag over this page and the next few, in steps, with the worker reading each page it reaches.
    let reach = pages.min(page + 6);
    let mut round_trips = Vec::new();
    let started = Instant::now();
    run.drive("mouse_down", &format!("{page}:{x},{y}"))?;
    for step in 0..240u64 {
        let on = page + step % (reach - page + 1);
        let drag = format!("{on}:{},{}", 40.0 + (step % 40) as f64 * 10.0, 40.0 + (step % 60) as f64 * 11.0);
        let one = Instant::now();
        run.drive("mouse_drag", &drag)?;
        round_trips.push(one.elapsed().as_secs_f64() * 1000.0);
    }
    run.drive("mouse_up", "all")?;
    run.settled()?;
    let drag_total_ms = started.elapsed().as_secs_f64() * 1000.0;
    round_trips.sort_by(f64::total_cmp);
    let after_drag = run.state()?;

    // ⌘A: all of it selected, with what's showing highlighted.
    let started = Instant::now();
    run.drive("menu", "edit.select_all")?;
    run.settled()?;
    let select_all_ms = started.elapsed().as_secs_f64() * 1000.0;
    let after_all = run.state()?;

    // ⌘C on all of it, watching the memory until it's on the clipboard.
    let started = Instant::now();
    run.drive("menu", "edit.copy")?;
    let (mut peak_own, mut peak_worker, mut most_text) = (0u64, 0u64, 0u64);
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let state = run.state()?;
        peak_own = peak_own.max(memory(&state, "footprint_bytes"));
        peak_worker = peak_worker.max(memory(&state, "worker_bytes"));
        most_text = most_text.max(state["reader"]["selection"]["text_bytes_held"].as_u64().unwrap_or(0));
        let selection = &state["reader"]["selection"];
        if selection["copying"] != true && selection["copy_waiting_for_host"] != true {
            break;
        }
        if Instant::now() >= deadline {
            return Err(failed("timed_out", "The copy didn't finish."));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let copy_ms = started.elapsed().as_secs_f64() * 1000.0;
    let notice = run.state()?["app"]["copy_notice"].clone();

    // Back at rest, with the selection cleared and a moment for the allocator, before anything reads the clipboard back, which is QA's own cost. What's left is what the clipboard holds in this process.
    run.drive("mouse_down", &format!("{page}:{x},{y}"))?;
    run.drive("mouse_up", "all")?;
    run.settled()?;
    std::thread::sleep(Duration::from_secs(1));
    let after = run.state()?;
    let copied = run.call(json!({ "cmd": "qa_read", "verb": "copied" }))?;
    Ok(json!({
        "file": file.file_name().map(|name| name.to_string_lossy().to_string()),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "pages": pages,
        "footprint_mib": { "before": mib(memory(&before, "footprint_bytes")), "after_drag": mib(memory(&after_drag, "footprint_bytes")), "after_select_all": mib(memory(&after_all, "footprint_bytes")), "peak_during_copy": mib(peak_own), "worker_peak_during_copy": mib(peak_worker), "after": mib(memory(&after, "footprint_bytes")) },
        "text_held_kib": { "after_drag": after_drag["reader"]["selection"]["text_bytes_held"].as_u64().unwrap_or(0) / 1024, "most_during_copy": most_text / 1024, "after": after["reader"]["selection"]["text_bytes_held"].as_u64().unwrap_or(0) / 1024 },
        "threads": { "before": before["memory"]["threads"], "after": after["memory"]["threads"] },
        "handles": { "before": before["memory"]["open_handles"], "after": after["memory"]["open_handles"] },
        "first_double_click_to_highlight_ms": round(first_word_ms),
        "drag": { "steps": round_trips.len(), "round_trip_ms": { "p50": round(percentile(&round_trips, 0.5)), "p99": round(percentile(&round_trips, 0.99)), "worst": round(percentile(&round_trips, 1.0)) }, "total_ms": round(drag_total_ms) },
        "ui_thread": { "pointer_steps": after["app"]["pages"]["pointer_steps"], "worst_pointer_step_us": after["app"]["pages"]["worst_pointer_step_us"], "worst_stall_ms": after["hangs"]["worst_ms"], "stalls_over_2s": after["hangs"]["over_2s"] },
        "select_all_ms": round(select_all_ms),
        "copy": { "ms": round(copy_ms), "chars": copied["chars"], "notice": notice },
    }))
}

/// `recto qa budget`.
pub fn budget(invocation: &Invocation) -> Result<Value, AgentError> {
    let app = find_app(invocation.fields.get("app").and_then(Value::as_str).map(Path::new))?;
    let file = PathBuf::from(invocation.fields.get("file").and_then(Value::as_str).unwrap_or_default());
    let heavy = invocation.fields.get("heavy").and_then(Value::as_str).map(PathBuf::from);
    let runs = invocation.fields.get("runs").and_then(Value::as_u64).unwrap_or(11).max(1);
    let mut report = json!({ "file": file.file_name().map(|name| name.to_string_lossy().to_string()), "app": app, "build": if cfg!(debug_assertions) { "debug" } else { "release" } });

    // Cold start: from starting the process to the first page sharp, opening a file as a double click does.
    let mut colds = Vec::new();
    let mut ready_ms = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        let run = Measured::start(&app, &Launch { open: vec![file.clone()], ..Launch::default() })?;
        ready_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        run.settled()?;
        colds.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    colds.sort_by(f64::total_cmp);
    ready_ms.sort_by(f64::total_cmp);
    report["cold_start_ms"] = json!({ "runs": runs, "p50": round(percentile(&colds, 0.5)), "p95": round(percentile(&colds, 0.95)), "max": round(percentile(&colds, 1.0)), "budget_p50": 300, "budget_p95": 500, "ready_p50": round(percentile(&ready_ms, 0.5)) });

    // One document, at rest on a page, then what idling costs, scrolling, and a closed window.
    let run = Measured::start(&app, &Launch { open: vec![file.clone()], ..Launch::default() })?;
    run.settled()?;
    let state = run.state()?;
    report["empty_tiles"] = state["reader"]["tiles"].clone();
    report["one_document"] = json!({
        "tree_mib": mib(state["memory"]["tree_bytes"].as_u64().unwrap_or(0)),
        "app_mib": mib(state["memory"]["footprint_bytes"].as_u64().unwrap_or(0)),
        "worker_mib": mib(state["memory"]["worker_bytes"].as_u64().unwrap_or(0)),
        "threads": state["memory"]["threads"],
        "handles": state["memory"]["open_handles"],
        "budget_mib": 60,
    });
    std::thread::sleep(Duration::from_secs(1));
    let before = recto_platform::local::process_counters(run.pid);
    let idle_for = Duration::from_secs(20);
    std::thread::sleep(idle_for);
    let after = recto_platform::local::process_counters(run.pid);
    if let (Some(before), Some(after)) = (before, after) {
        let seconds = idle_for.as_secs_f64();
        report["idle"] = json!({
            "seconds": idle_for.as_secs(),
            "cpu_percent": round((after.cpu_ns.saturating_sub(before.cpu_ns)) as f64 / 1e9 / seconds * 100.0),
            "wakeups_per_second": round((after.wakeups.saturating_sub(before.wakeups)) as f64 / seconds),
            "threads": after.threads,
            "budget_wakeups_per_second": 1,
            "note": "A window off every screen, so what a display link or a visible window adds isn't in this.",
        });
    }
    run.call(json!({ "cmd": "goto", "page": (state["reader"]["pages"].as_u64().unwrap_or(2) / 3).max(1) }))?;
    run.settled()?;
    std::thread::sleep(Duration::from_millis(1500));
    let sweep = run.call(json!({ "cmd": "qa_drive", "verb": "sweep", "value": "6", "timeout_ms": 5000 }))?;
    report["scroll"] = json!({
        "pages_per_second": 6,
        "steps": sweep["steps"],
        "skipped": sweep["skipped_because_the_last_was_still_running"],
        "ui_step_us": sweep["step_us"],
        "budget_ui_step_us_p99": 2000,
        "blank_seen_steps": sweep["blank_seen_steps"],
        "blank_steps": sweep["blank_steps"],
        "contents": sweep["contents"],
        "peak_tree_mib": mib(sweep["peak_tree_bytes"].as_u64().unwrap_or(0)),
        "budget_peak_mib": 170,
        "sharp_after_stop_ms": sweep["sharp_after_ms"],
        "budget_sharp_ms": 100,
        "note": "The UI thread's work per step at 120 steps a second, off screen. Frames reaching a display aren't in this: a quiet run has none.",
    });
    // Sharp after scroll stops, on pages that haven't been seen: where the reader's next stop isn't drawn yet.
    let mut stops = Vec::new();
    let pages = state["reader"]["document_height"].as_f64().unwrap_or(0.0) / state["reader"]["pages"].as_f64().unwrap_or(1.0).max(1.0);
    run.call(json!({ "cmd": "goto", "page": 1 }))?;
    run.settled()?;
    for _ in 0..runs.min(20) {
        run.drive("scroll", &format!("{}", pages * 0.75))?;
        stops.push(run.settled()?);
    }
    stops.sort_by(f64::total_cmp);
    report["sharp_after_scroll_stops_ms"] = json!({ "stops": stops.len(), "p50": round(percentile(&stops, 0.5)), "worst": round(percentile(&stops, 1.0)), "budget": 100, "note": "From `qa scroll` returning. Includes one client round trip." });
    let fast = run.call(json!({ "cmd": "qa_drive", "verb": "sweep", "value": "40", "timeout_ms": 5000 }))?;
    report["fast_scroll"] = json!({ "pages_per_second": 40, "contents": fast["contents"], "ui_step_us": fast["step_us"], "blank_seen_steps": fast["blank_seen_steps"], "peak_tree_mib": mib(fast["peak_tree_bytes"].as_u64().unwrap_or(0)), "budget_peak_mib": 170, "sharp_after_stop_ms": fast["sharp_after_ms"], "cancelled": fast["cancelled"] });
    run.drive("window", "close")?;
    std::thread::sleep(Duration::from_millis(500));
    let closed = run.state()?;
    report["closed_window"] = json!({ "tree_mib": mib(closed["memory"]["tree_bytes"].as_u64().unwrap_or(0)), "threads": closed["memory"]["threads"], "handles": closed["memory"]["open_handles"], "budget_mib": 20 });
    report["hangs"] = closed["hangs"].clone();
    drop(run);

    // Many tabs: the footprint with each of many tabs of the same large file, under names of their own.
    let run = Measured::start(&app, &Launch::default())?;
    let tabs_dir = run.root.join("tabs");
    std::fs::create_dir_all(&tabs_dir).map_err(|_| failed("write_failed", "The temp folder couldn't be written to."))?;
    let tab_file = heavy.clone().unwrap_or_else(|| file.clone());
    let mut measures = Vec::new();
    let mut opened = 0u32;
    for count in [2u32, 100, 1000] {
        while opened < count {
            let link = tabs_dir.join(format!("t{opened}.pdf"));
            #[cfg(unix)]
            std::os::unix::fs::symlink(&tab_file, &link).map_err(|_| failed("write_failed", "A link couldn't be made."))?;
            run.call(json!({ "cmd": "open", "path": link }))?;
            opened += 1;
        }
        run.settled()?;
        let state = run.state()?;
        measures.push(json!({ "tabs": count, "tree_mib": mib(state["memory"]["tree_bytes"].as_u64().unwrap_or(0)), "app_mib": mib(state["memory"]["footprint_bytes"].as_u64().unwrap_or(0)), "worker_mib": mib(state["memory"]["worker_bytes"].as_u64().unwrap_or(0)), "threads": state["memory"]["threads"], "handles": state["memory"]["open_handles"], "documents_held": state["reader"]["documents_held"], "tile_layers": state["app"]["pages"]["tile_layers"] }));
    }
    let started = Instant::now();
    run.drive("click_row", "0")?;
    run.settled()?;
    let switch_ms = started.elapsed().as_secs_f64() * 1000.0;
    // The sidebar with 1,000 tabs, shown again after focus view, whose rows are all rebuilt.
    run.call(json!({ "cmd": "view", "focus": "on" }))?;
    run.settled()?;
    let started = Instant::now();
    run.call(json!({ "cmd": "view", "focus": "off" }))?;
    run.settled()?;
    let sidebar_state = run.state()?;
    let sidebar_ms = started.elapsed().as_secs_f64() * 1000.0;
    if sidebar_state["app"]["sidebar_visible"] != true {
        return Err(failed("qa_failed", "The sidebar didn't come back."));
    }
    let (first_tabs, last) = (measures[0]["app_mib"].as_f64().unwrap_or(0.0), measures[2]["app_mib"].as_f64().unwrap_or(0.0));
    report["tabs"] = json!({ "file": tab_file.file_name().map(|name| name.to_string_lossy().to_string()), "measures": measures, "app_mib_over_2_tabs_at_1000": round(last - first_tabs), "budget_mib_over_2_tabs": 4, "switch_to_a_cold_tab_ms": round(switch_ms), "sidebar_opens_ms": round(sidebar_ms), "budget_sidebar_ms": 150 });
    drop(run);

    if let Some(heavy) = heavy {
        // The heavy page: first view, scroll and zoom, as `budget viewport` draws them, now on the real path.
        let started = Instant::now();
        let run = Measured::start(&app, &Launch { open: vec![heavy.clone()], ..Launch::default() })?;
        run.settled()?;
        let first_view = started.elapsed().as_secs_f64() * 1000.0;
        let first = run.state()?;
        // A map like this fits the window whole at the fitted width, so there's nothing to scroll. Zoomed to 3, it's about 2x, as `budget viewport` draws it.
        let started = Instant::now();
        run.drive("zoom", "3")?;
        run.settled()?;
        let zoom_2x_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        run.drive("scroll", "225")?;
        run.settled()?;
        let scroll_ms = started.elapsed().as_secs_f64() * 1000.0;
        let after_scroll = run.state()?;
        let started = Instant::now();
        run.drive("zoom", "4.5")?;
        run.settled()?;
        let zoom_ms = started.elapsed().as_secs_f64() * 1000.0;
        let after_zoom = run.state()?;
        // Long enough for the worker to be let go.
        std::thread::sleep(Duration::from_secs(4));
        let at_rest = run.state()?;
        report["heavy"] = json!({
            "file": heavy.file_name().map(|name| name.to_string_lossy().to_string()),
            "first_view_ms_from_process_start": round(first_view),
            "scroll_quarter_window_ms": round(scroll_ms),
            "zoom_to_about_2x_ms": round(zoom_2x_ms),
            "zoom_1_5_more_ms": round(zoom_ms),
            "first_view_tree_mib": mib(first["memory"]["tree_bytes"].as_u64().unwrap_or(0)),
            "after_scroll_tree_mib": mib(after_scroll["memory"]["tree_bytes"].as_u64().unwrap_or(0)),
            "after_zoom_tree_mib": mib(after_zoom["memory"]["tree_bytes"].as_u64().unwrap_or(0)),
            "worker_peak_mib": mib(after_zoom["memory"]["worker_peak_bytes"].as_u64().unwrap_or(0)),
            "app_mib_at_zoom": mib(after_zoom["memory"]["footprint_bytes"].as_u64().unwrap_or(0)),
            "at_rest_4s_later_tree_mib": mib(at_rest["memory"]["tree_bytes"].as_u64().unwrap_or(0)),
            "budget_note": "Named exception: 415 to 450 MiB accepted by the owner, 2026-11-07.",
            "tile_scale": after_zoom["reader"]["tile_scale"],
            "low_res_pages": after_zoom["reader"]["low_res_pages"],
        });
        drop(run);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a script's line is split differently from how a person reads it, so a path with spaces becomes two words, or an unset name quietly becomes nothing.
    #[test]
    fn script_lines_split_as_they_read() {
        assert_eq!(script_words(r#"open "/a b/c.pdf" --page 2"#).unwrap(), ["open", "/a b/c.pdf", "--page", "2"]);
        assert_eq!(script_words("  qa   state ").unwrap(), ["qa", "state"]);
        assert!(script_words(r#"open "/a"#).is_err());
        assert!(script_words("open $RECTO_NOT_SET_ANYWHERE").is_err());
    }

    /// Failure: an expectation passes on a value that doesn't hold, or a path that isn't there reads as a match.
    #[test]
    fn expectations_hold_only_when_true() {
        let answer = json!({ "reader": { "settled": true, "visible_pages": [3, 4], "tile_scale": 2.5 }, "note": "lower-resolution version" });
        let line = |text: &str| script_words(text).unwrap();
        assert!(check(&answer, &line("expect reader.settled true")).is_none());
        assert!(check(&answer, &line("expect reader.settled false")).is_some());
        assert!(check(&answer, &line("expect reader.visible_pages.1 4")).is_none());
        assert!(check(&answer, &line("expect reader.missing 4")).is_some());
        assert!(check(&answer, &line("expect-at-least reader.tile_scale 2")).is_none());
        assert!(check(&answer, &line("expect-at-most reader.tile_scale 2")).is_some());
        assert!(check(&answer, &line("expect-contains note lower-resolution")).is_none());
        assert!(check(&answer, &line("expect-nothing note x")).is_some());
    }
}
