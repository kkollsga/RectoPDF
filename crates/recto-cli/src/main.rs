//! `recto`: a thin client of the session on the control socket. It parses the line through the command table, sends one JSON command, and prints the answer. Everything decided is decided in
//! `recto-agent` and `recto-control`. A few commands run here because they need no session: help, the skill, starting the session, and the user's own `permission set`.

mod handover;
mod qa;

use recto_agent::commands::access::{Log, PermissionRequest};
use recto_agent::commands::sync::{Wait, Watch};
use recto_agent::commands::{Rights, Waits};
use recto_agent::table::{Invocation, Route, family_help, find_row, help_for, help_text, parse_cli, table_json};
use recto_agent::{AgentError, Command, DrawableLoader, Level, VERSION, exit, human, parse, reply, skill};
use recto_control::client::Client;
use recto_control::host::SessionHost;
use recto_control::log::{self, ActivityLog};
use recto_control::server::{Limits, Server};
use recto_control::{StatePaths, levels, local, terminal};
use recto_platform::helper::Priority;
use recto_session::Session;
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    let json_wanted = args.iter().any(|arg| arg == "--json");
    let invocation = match parse_cli(args) {
        Ok(invocation) => invocation,
        Err(error) => {
            if args.is_empty() {
                print_out(&help_text());
                return exit::USAGE;
            }
            return finish("usage", json_wanted, &Err(error));
        }
    };
    if invocation.help {
        print_out(&help_for(invocation.spec));
        return exit::OK;
    }
    let mut invocation = invocation;
    if let Some(field) = recto_agent::table::stdin_secret_field(invocation.spec) {
        match read_secret_line() {
            Ok(secret) => {
                invocation.fields.insert(field.into(), Value::String(secret));
            }
            Err(error) => return finish(invocation.spec.tag, json_wanted, &Err(error)),
        }
    }
    match invocation.spec.route {
        Route::Session => through_session(&invocation),
        Route::Here => here(&invocation),
    }
}

/// The first line of stdin, for a secret that mustn't be on the command line: at most a password's length, and never from a terminal, where it would be echoed.
fn read_secret_line() -> Result<String, AgentError> {
    let refused = |words: &str| AgentError::new("usage", words, exit::USAGE).then("Pipe it in: `printf '%s' \"$PASSWORD\" | recto qa password`.");
    if std::io::stdin().is_terminal() {
        return Err(refused("This reads the password from stdin, and stdin is a terminal, which would show it."));
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(recto_core::MAX_PASSWORD_BYTES as u64 + 2)
        .read_to_end(&mut bytes)
        .map_err(|_| refused("Stdin couldn't be read."))?;
    let line = bytes.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.is_empty() || line.len() > recto_core::MAX_PASSWORD_BYTES {
        return Err(refused("Stdin's first line must be the password, from 1 to 1024 bytes."));
    }
    String::from_utf8(line.to_vec()).map_err(|_| refused("The password must be UTF-8."))
}

/// Writes to stdout. A reader that's gone away, as in `recto tabs | head`, is no failure of ours.
fn print_out(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    if !text.ends_with('\n') {
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

fn print_err(text: &str) {
    let mut out = std::io::stderr().lock();
    let _ = writeln!(out, "{text}");
}

/// Prints a result as asked and returns the exit code.
fn finish(tag: &str, json_wanted: bool, result: &Result<Value, AgentError>) -> i32 {
    if json_wanted {
        print_out(&reply(tag, result).to_string());
    } else {
        match result {
            Ok(data) => print_out(&human::render(tag, data)),
            Err(error) => print_err(&human::error(&reply(tag, &Err(error.clone()))["error"])),
        }
    }
    result.as_ref().map_or_else(|error| error.exit, |_| exit::OK)
}

/// Prints an answer the session already enveloped, and returns the exit code.
fn finish_envelope(tag: &str, json_wanted: bool, value: &Value) -> i32 {
    if json_wanted {
        print_out(&value.to_string());
    } else if value["ok"] == true {
        print_out(&human::render(tag, &value["data"]));
    } else {
        print_err(&human::error(&value["error"]));
    }
    if value["ok"] == true {
        exit::OK
    } else {
        value["error"]["exit"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .unwrap_or(exit::FAILED)
    }
}

fn paths() -> Result<StatePaths, AgentError> {
    StatePaths::from_env()
}

/// Sends the line to the session and prints what comes back.
fn through_session(invocation: &Invocation) -> i32 {
    let tag = invocation.spec.tag;
    let json_wanted = invocation.json;
    let outcome = invocation.command().and_then(|command| {
        let paths = paths()?;
        let wait = client_wait(invocation.spec.client_wait_s, &command);
        match (&command, Client::connect(&paths)) {
            (Command::Rights(Rights::PermissionShow(_)), Err(error)) if error.exit == exit::NOT_RUNNING => Ok(Answer::Here(levels::show(&paths, None))),
            (Command::Rights(Rights::PermissionRequest(PermissionRequest { level, .. })), Err(error)) if error.exit == exit::NOT_RUNNING => {
                ActivityLog::new(&paths).record(tag, levels::current(&paths), "asked", &command.log_details());
                Ok(Answer::Here(levels::request_answer(*level, levels::current(&paths))))
            }
            // The user at their own terminal can always read what agents did.
            (Command::Rights(Rights::Log(Log { last })), _) if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() => {
                Ok(Answer::Here(log::recent(&paths, *last)))
            }
            // With no session, the same level still decides who may read it.
            (Command::Rights(Rights::Log(Log { last })), Err(error)) if error.exit == exit::NOT_RUNNING => {
                let level = levels::current(&paths);
                if recto_agent::allowed(level, command.access()) {
                    Ok(Answer::Here(log::recent(&paths, *last)))
                } else {
                    Err(recto_agent::refusal("log", level, command.access()))
                }
            }
            (Command::Save { .. } | Command::Rights(Rights::Sign(_)), Err(error)) if error.exit == exit::NOT_RUNNING => {
                // These always refuse. Saying why is more use than saying nothing's running.
                let mut session = Session::new(DrawableLoader);
                local::run_here(&mut session, &paths, &mut ActivityLog::new(&paths), &command).map(Answer::Here)
            }
            (_, Err(error)) => Err(error),
            (Command::Waits(Waits::Watch(_)), Ok(client)) => Ok(Answer::Stream(client, wait)),
            (Command::UpdateInstall { zip: Some(zip), .. }, Ok(client)) => {
                drop(client);
                handover::install(&paths, invocation, zip, wait).map(Answer::Envelope)
            }
            (_, Ok(mut client)) => client.call(&invocation.wire(), wait).map(Answer::Envelope),
        }
    });
    match outcome {
        Ok(Answer::Here(data)) => finish(tag, json_wanted, &Ok(data)),
        Ok(Answer::Envelope(value)) => finish_envelope(tag, json_wanted, &value),
        Ok(Answer::Stream(client, wait)) => stream(client, &invocation.wire(), wait, json_wanted),
        Err(error) => finish(tag, json_wanted, &Err(error)),
    }
}

enum Answer {
    Here(Value),
    Envelope(Value),
    Stream(Client, Duration),
}

/// How long the client gives the session, on top of how long the command says it may wait.
fn client_wait(base_s: u64, command: &Command) -> Duration {
    let waited = match command {
        Command::Waits(Waits::Wait(Wait { timeout_ms, .. })) => *timeout_ms,
        Command::Waits(Waits::Watch(Watch { timeout_ms, .. })) => timeout_ms.unwrap_or(recto_agent::DEFAULT_WATCH_MS),
        Command::Rights(Rights::PermissionRequest(PermissionRequest { timeout_ms, .. })) => timeout_ms.unwrap_or(0),
        // `qa wait-settled --timeout 2m` waits that long in the app, so the client waits that long too, past its own time to answer.
        Command::QaRead { timeout_ms, .. } | Command::QaDrive { timeout_ms, .. } => timeout_ms.unwrap_or(0),
        _ => 0,
    };
    Duration::from_secs(base_s) + Duration::from_millis(waited)
}

/// A watcher: one line per event, and its last line says how it ended.
fn stream(mut client: Client, line: &str, wait: Duration, json_wanted: bool) -> i32 {
    if let Err(error) = client.send(line, Duration::from_secs(5)) {
        return finish("watch", json_wanted, &Err(error));
    }
    loop {
        let value = match client.read(wait) {
            Ok(value) => value,
            Err(error) => return finish("watch", json_wanted, &Err(error)),
        };
        if value["ok"] != true {
            return finish_envelope("watch", json_wanted, &value);
        }
        if json_wanted {
            print_out(&value.to_string());
        } else {
            print_out(&human::event(&value["data"]));
        }
        if value["data"]["done"] == true {
            return exit::OK;
        }
    }
}

/// The commands that run in this process.
fn here(invocation: &Invocation) -> i32 {
    let tag = invocation.spec.tag;
    let json_wanted = invocation.json;
    let result = match tag {
        "help" => return help(invocation),
        "version" => Ok(json!({ "version": env!("CARGO_PKG_VERSION"), "api": VERSION })),
        "permission_set" => permission_set(invocation),
        "skill_show" => return skill_show(invocation),
        "skill_install" => skill_install(invocation),
        "serve" => return serve(invocation),
        "local" => return run_local(invocation),
        "session" => return session(),
        "budget_tabs" => budget_tabs(invocation),
        "qa_start" => qa::start(invocation),
        "qa_run" => qa::run_script(invocation),
        "qa_budget" => qa::budget(invocation),
        "qa_compare_budget" => qa::compare_budget(invocation),
        "qa_select_budget" => qa::select_budget(invocation),
        "budget_index" => budget_index(invocation),
        // A row with no handler is a bug here, and a test walks the table for it.
        _ => Err(AgentError::new("internal", "This command isn't wired up.", exit::FAILED)),
    };
    finish(tag, json_wanted, &result)
}

fn help(invocation: &Invocation) -> i32 {
    if invocation.rest.is_empty() {
        if invocation.json {
            print_out(&reply("help", &Ok(table_json())).to_string());
        } else {
            print_out(&help_text());
        }
        return exit::OK;
    }
    if let Some(spec) = find_row(&invocation.rest) {
        print_out(&help_for(spec));
        return exit::OK;
    }
    if let Some(group) = family_help(&invocation.rest) {
        print_out(&group);
        return exit::OK;
    }
    // Parsing it gives the same words a mistyped command gets anywhere else: the closest real ones.
    finish("help", invocation.json, &parse_cli(&invocation.rest).map(|_| Value::Null))
}

fn permission_set(invocation: &Invocation) -> Result<Value, AgentError> {
    let level = invocation
        .fields
        .get("level")
        .and_then(Value::as_str)
        .and_then(Level::parse)
        .ok_or_else(|| AgentError::new("usage", "Which level?", exit::USAGE))?;
    terminal::set_level(&paths()?, level)
}

fn skill_show(invocation: &Invocation) -> i32 {
    let body = match invocation.fields.get("file").and_then(Value::as_str) {
        Some(skill::REFERENCE_FILE) => skill::reference_md(),
        _ => skill::skill_md(),
    };
    print_out(&body);
    exit::OK
}

fn skill_install(invocation: &Invocation) -> Result<Value, AgentError> {
    let path = invocation.fields.get("path").and_then(Value::as_str);
    let target = invocation.fields.get("target").and_then(Value::as_str);
    let folder = match (path, target) {
        (Some(path), None) => PathBuf::from(path),
        (None, Some(_)) => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .or_else(recto_platform::local::real_home)
                .ok_or_else(|| AgentError::new("write_failed", "This user's home folder couldn't be found.", exit::FAILED))?;
            home.join(skill::CLAUDE_CODE_SKILL_FOLDER)
        }
        _ => {
            return Err(
                AgentError::new("usage", "Say where: `--path <folder>`, or `--target claude-code`.", exit::USAGE)
                    .then("`recto help skill install` shows both."),
            );
        }
    };
    // A QA run writes nothing of the user's own, however it's been pointed.
    if paths()?.overridden && recto_control::state::is_in_real_home(&folder) {
        return Err(AgentError::new(
            "state_refused",
            "This run uses a test state folder, so it won't write into the real home.",
            exit::PERMISSION,
        )
        .then("Use `--path` with a temp folder, or set `HOME` to one."));
    }
    let flag = |key: &str| invocation.fields.get(key).and_then(Value::as_bool).unwrap_or(false);
    terminal::confirm_skill_install(&folder, flag("replace"))?;
    skill::install(&folder, flag("replace"), flag("dry_run"))
}

/// Starts the session. In the foreground it runs here until stopped. `--background` starts another copy of this program and returns once it's answering.
fn serve(invocation: &Invocation) -> i32 {
    let json_wanted = invocation.json;
    let paths = match paths() {
        Ok(paths) => paths,
        Err(error) => return finish("serve", json_wanted, &Err(error)),
    };
    if invocation.fields.contains_key("background") {
        let result = serve_in_background(&paths);
        return finish("serve", json_wanted, &result);
    }
    let server = match Server::bind(&paths) {
        Ok(server) => server,
        Err(error) => return finish("serve", json_wanted, &Err(error.into())),
    };
    let ready = json!({ "running": true, "pid": std::process::id(), "socket": paths.socket, "already": false });
    finish("serve", json_wanted, &Ok(ready));
    match server.run(
        Arc::new(
            SessionHost::default()
                .with_indexing(&paths)
                .with_generating(&paths)
                .with_own_record(&paths)
                .with_backups(&paths),
        ),
        Limits::default(),
    ) {
        Ok(()) => exit::OK,
        Err(_) => {
            print_err("The session stopped because its socket failed.");
            exit::FAILED
        }
    }
}

fn serve_in_background(paths: &StatePaths) -> Result<Value, AgentError> {
    if let Ok(mut client) = Client::connect(paths)
        && let Ok(status) = client.call("{\"cmd\":\"status\"}", Duration::from_secs(5))
        && status["ok"] == true
    {
        return Ok(json!({ "running": true, "pid": status["data"]["pid"], "socket": paths.socket, "already": true }));
    }
    paths.prepare()?;
    let failed = || AgentError::new("serve_failed", "The session couldn't be started.", exit::FAILED).then("Run `recto serve` in the foreground to see why.");
    let messages = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&paths.server_messages)
        .map_err(|_| failed())?;
    let exe = std::env::current_exe().map_err(|_| failed())?;
    let mut command = std::process::Command::new(exe);
    command
        .args(["serve", "--json"])
        .current_dir(&paths.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(messages);
    own_process_group(&mut command);
    let mut child = command.spawn().map_err(|_| failed())?;
    // It says one line when it's ready, or ends without one. Blocking on that, not polling for the socket.
    let mut line = String::new();
    let stdout = child.stdout.take().ok_or_else(failed)?;
    let read = std::io::BufReader::new(stdout).read_line(&mut line);
    if read.is_ok()
        && let Ok(value) = serde_json::from_str::<Value>(&line)
    {
        if value["ok"] == true {
            return Ok(value["data"].clone());
        }
        // Lost a race with another start: that's fine if there's now a session.
        if value["error"]["code"] == "already_running"
            && let Ok(mut client) = Client::connect(paths)
            && let Ok(status) = client.call("{\"cmd\":\"status\"}", Duration::from_secs(5))
        {
            return Ok(json!({ "running": true, "pid": status["data"]["pid"], "socket": paths.socket, "already": true }));
        }
        if let (Some(code), Some(message)) = (value["error"]["code"].as_str(), value["error"]["message"].as_str()) {
            let code = if code == "state_refused" { "state_refused" } else { "serve_failed" };
            return Err(AgentError::new(code, message.to_string(), exit::FAILED).then(
                value["error"]["next"]
                    .as_str()
                    .unwrap_or("Run `recto serve` in the foreground to see why.")
                    .to_string(),
            ));
        }
    }
    let _ = child.wait();
    let mut said = String::new();
    let _ = std::fs::File::open(&paths.server_messages).and_then(|mut file| file.read_to_string(&mut said));
    Err(failed().then(if said.trim().is_empty() {
        "Run `recto serve` in the foreground to see why.".to_string()
    } else {
        format!("It said: {}", recto_core::clean_line(&said))
    }))
}

/// Its own group, so closing the terminal or tool that started it doesn't end it.
#[cfg(unix)]
fn own_process_group(command: &mut std::process::Command) {
    std::os::unix::process::CommandExt::process_group(command, 0);
}

#[cfg(not(unix))]
fn own_process_group(_command: &mut std::process::Command) {}

/// `recto local <file> <command...>`: one command in a throwaway session here. The file is opened with `open`, at the level in force, so this reaches no further than the socket does.
fn run_local(invocation: &Invocation) -> i32 {
    let json_wanted = invocation.json;
    let inner = match parse_cli(&invocation.rest) {
        Ok(inner) => inner,
        Err(error) => return finish("local", json_wanted, &Err(error)),
    };
    let tag = inner.spec.tag;
    if inner.help {
        print_out(&help_for(inner.spec));
        return exit::OK;
    }
    // `recto local file.pdf page --help` asks about a group.
    if tag == "help" {
        return help(&inner);
    }
    if inner.spec.route != Route::Session {
        let error = AgentError::new("usage", format!("`{}` doesn't run on a document.", inner.spec.name), exit::USAGE)
            .then("`recto local <file.pdf> text --page 1` is the kind of thing this does.");
        return finish("local", json_wanted, &Err(error));
    }
    let file = invocation.fields.get("file").and_then(Value::as_str).map(PathBuf::from).unwrap_or_default();
    let result = (|| {
        let paths = paths()?;
        let mut log = ActivityLog::new(&paths);
        let command = inner.command()?;
        let mut session = Session::new(DrawableLoader);
        local::run_here(&mut session, &paths, &mut log, &Command::Open { path: file, page: None })?;
        local::run_here(&mut session, &paths, &mut log, &command)
    })();
    finish(tag, json_wanted, &result)
}

/// Reads commands line by line and answers each one, in a throwaway session here. QA scripts drive this. It takes its level from the state folder like everything else.
fn session() -> i32 {
    let paths = match paths() {
        Ok(paths) => paths,
        Err(error) => return finish("session", true, &Err(error)),
    };
    let mut log = ActivityLog::new(&paths);
    let mut session = Session::new(DrawableLoader);
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let (tag, result) = match parse(&line) {
            Ok(command) => (command.tag(), local::run_here(&mut session, &paths, &mut log, &command)),
            Err(error) => {
                log.record("unknown", levels::current(&paths), error.code, &Value::Null);
                ("unknown", Err(error))
            }
        };
        print_out(&reply(tag, &result).to_string());
    }
    exit::OK
}

/// Opens one real PDF as many separate tabs and reports memory and handles. Each tab gets its own link in a temporary folder, because opening the same path twice reuses its tab. Through the session, it's the
/// session's own numbers that count.
fn budget_tabs(invocation: &Invocation) -> Result<Value, AgentError> {
    let file = invocation.fields.get("file").and_then(Value::as_str).map(PathBuf::from).unwrap_or_default();
    let count = invocation.fields.get("count").and_then(Value::as_u64).unwrap_or(0);
    let file = file
        .canonicalize()
        .map_err(|_| AgentError::new("not_found", "That file couldn't be found.", exit::NOT_FOUND))?;
    // A folder of this run's own, never one a run killed partway left behind, and gone when this ends, by a panic too.
    let base = std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir());
    let fresh = recto_platform::fresh::FreshDir::new_in(&base, "recto-budget-", "")
        .map_err(|_| AgentError::new("write_failed", "Couldn't make a temporary folder.", exit::FAILED))?;
    let dir = fresh.path();
    let paths = paths()?;
    let file =
        recto_platform::local::clone_source_in(&file, dir).map_err(|_| AgentError::new("write_failed", "Couldn't make a file to clone.", exit::FAILED))?;
    if invocation.fields.contains_key("in_process") {
        measure_in_process(&paths, &file, dir, count)
    } else {
        measure_through_session(&paths, &file, dir, count)
    }
}

/// What's a search without an index if nothing's named. A word nothing has, so every page is read; a common word, so the limit is reached; and a phrase.
const DEFAULT_BUDGET_QUERIES: &str = "zzzqqqxx,the,of the";

/// Builds a PDF's index in a folder of its own, and reports what that costs and what it gets you. The build is the helper's own numbers, since it's the real one, and the searches are timed here, in this process.
fn budget_index(invocation: &Invocation) -> Result<Value, AgentError> {
    let paths = paths()?;
    let level = levels::current(&paths);
    let needed = invocation.spec.access;
    if !recto_agent::allowed(level, needed) {
        return Err(recto_agent::refusal(invocation.spec.name, level, needed));
    }
    paths.prepare()?;
    let file = invocation.fields.get("file").and_then(Value::as_str).map(PathBuf::from).unwrap_or_default();
    let file = file
        .canonicalize()
        .map_err(|_| AgentError::new("not_found", "That file couldn't be found.", exit::NOT_FOUND))?;
    let runs = invocation.fields.get("runs").and_then(Value::as_u64).unwrap_or(7) as usize;
    let queries = invocation.fields.get("queries").and_then(Value::as_str).unwrap_or(DEFAULT_BUDGET_QUERIES);
    let foreground = invocation.fields.contains_key("foreground");
    let key = recto_index::corpus::CorpusKey::of_file(&file).map_err(|_| AgentError::new("not_found", "That file couldn't be read.", exit::NOT_FOUND))?;
    // In the OS's temp folder, which it cleans for itself, since a run killed partway leaves a document's text here and nothing else would ever look at it.
    let temp =
        recto_platform::local::system_temp_dir().ok_or_else(|| AgentError::new("write_failed", "This Mac's temp folder couldn't be found.", exit::FAILED))?;
    // A folder of this run's own, never one a run killed partway left behind, and gone when this ends, by a panic too.
    let fresh = recto_platform::fresh::FreshDir::new_in(&temp, "recto-budget-", ".noindex")
        .map_err(|_| AgentError::new("write_failed", "Couldn't make a temporary folder.", exit::FAILED))?;
    let root = fresh.path().to_path_buf();
    let result = (|| {
        let build = recto_control::indexing::build_now(
            &file,
            key,
            &root,
            if foreground {
                Priority::Inherited
            } else {
                recto_control::helpers::HELPER_PRIORITY
            },
        );
        if build["state"] != "ready" {
            let code = build["code"].as_str().unwrap_or("unknown");
            return Err(AgentError::new("index_failed", format!("The index couldn't be built ({code})."), exit::FAILED));
        }
        let corpus = recto_agent::index::ReadOnlyCorpus(root.clone());
        let mut session = Session::new(DrawableLoader);
        recto_agent::run(
            &mut session,
            Level::Full,
            &Command::Open {
                path: file.clone(),
                page: None,
            },
        )?;
        let mut searches = Vec::new();
        for query in queries.split(',').filter(|query| !query.trim().is_empty()) {
            let mut one = json!({ "untrusted": { "query": query } });
            for (word, scan, regex, rank) in [
                ("index", false, false, false),
                ("scan", true, false, false),
                ("regex_index", false, true, false),
                ("regex_scan", true, true, false),
                ("rank_index", false, false, true),
                ("rank_scan", true, false, true),
            ] {
                let command = Command::from(recto_agent::commands::text::Search {
                    tab: "active".to_string(),
                    query: query.to_string(),
                    pages: None,
                    limit: None,
                    scan,
                    regex,
                    rank,
                    context: None,
                });
                let mut times = Vec::with_capacity(runs);
                let mut last = Ok(Value::Null);
                for _ in 0..runs {
                    let started = std::time::Instant::now();
                    last = recto_agent::run_indexed(&mut session, Level::Full, &command, Some(&corpus));
                    times.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                times.sort_by(f64::total_cmp);
                let at = |share: usize| times[(times.len() * share / 100).min(times.len() - 1)];
                let last = last?;
                one[word] = json!({ "route": last["route"], "p50_ms": at(50), "p95_ms": at(95), "hits_total": last["coverage"]["hits_total"], "hits_shown": last["hits"].as_array().map_or(0, Vec::len) });
            }
            searches.push(one);
        }
        Ok(json!({
            "file_bytes": std::fs::metadata(&file).map(|meta| meta.len()).ok(),
            "build": build,
            "searches": searches,
            "corpus_folder": root,
            "this_process_peak_bytes": recto_platform::isolate::process_memory(std::process::id()).map(|memory| memory.peak_footprint_bytes),
        }))
    })();
    drop(fresh);
    result
}

/// Another file of the same bytes, at no cost on this disk: a link would be the same PDF, and a PDF is open once (SPEC decision 26).
fn make_link(file: &Path, dir: &Path, index: u64) -> Result<PathBuf, AgentError> {
    let link = dir.join(format!("tab-{index:05}.pdf"));
    recto_platform::local::clone_file(file, &link).map_err(|_| AgentError::new("write_failed", "Couldn't make a test copy.", exit::FAILED))?;
    Ok(link)
}

fn measure_in_process(paths: &StatePaths, file: &Path, dir: &Path, count: u64) -> Result<Value, AgentError> {
    let mut session = Session::new(DrawableLoader);
    let mut log = ActivityLog::new(paths);
    let start_footprint = recto_platform::footprint_bytes();
    let start_handles = recto_platform::open_handle_count();
    let started = std::time::Instant::now();
    let mut samples = Vec::new();
    for index in 0..count {
        let link = make_link(file, dir, index)?;
        local::run_here(&mut session, paths, &mut log, &Command::Open { path: link, page: None })?;
        if index + 1 == 2 || index + 1 == count {
            samples.push(json!({ "tabs": index + 1, "footprint_bytes": recto_platform::footprint_bytes() }));
        }
    }
    if session.tabs().len() as u64 != count {
        return Err(AgentError::new(
            "budget_wrong_tabs",
            format!(
                "Asked for {count} tabs and got {}, so these numbers wouldn't mean what they say.",
                session.tabs().len()
            ),
            exit::FAILED,
        ));
    }
    Ok(json!({
        "route": "in this process",
        "file_bytes": std::fs::metadata(file).map(|meta| meta.len()).ok(),
        "tabs": count,
        "start_footprint_bytes": start_footprint,
        "samples": samples,
        "start_handles": start_handles,
        "end_handles": recto_platform::open_handle_count(),
        "ms_per_open": started.elapsed().as_secs_f64() * 1000.0 / count.max(1) as f64,
    }))
}

/// One connection for all of it, because this measures the session rather than starting a process each time. Closes what it opened, even on a failure, since the links are about to go.
fn measure_through_session(paths: &StatePaths, file: &Path, dir: &Path, count: u64) -> Result<Value, AgentError> {
    let wait = Duration::from_secs(120);
    let mut client = Client::connect(paths)?;
    let mut call = |line: String| -> Result<Value, AgentError> {
        let value = client.call(&line, wait)?;
        if value["ok"] == true {
            Ok(value["data"].clone())
        } else {
            Err(error_from(&value))
        }
    };
    let before = call("{\"cmd\":\"budget\"}".to_string())?;
    let started = std::time::Instant::now();
    let mut samples = Vec::new();
    let mut opened = Vec::new();
    let mut failure = None;
    for index in 0..count {
        let link = make_link(file, dir, index)?;
        let line = json!({ "cmd": "open", "path": link }).to_string();
        match call(line) {
            Ok(data) => opened.push(data["tab"].as_u64().unwrap_or(0)),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
        if index + 1 == 2 || index + 1 == count {
            let now = call("{\"cmd\":\"budget\"}".to_string())?;
            samples.push(json!({ "tabs": index + 1, "footprint_bytes": now["footprint_bytes"], "open_handles": now["open_handles"] }));
        }
    }
    let per_open = started.elapsed().as_secs_f64() * 1000.0 / count.max(1) as f64;
    // Numbers for so many tabs from fewer than that would be worse than none: a PDF is open once.
    let distinct: std::collections::HashSet<u64> = opened.iter().copied().collect();
    if failure.is_none() && distinct.len() as u64 != count {
        failure = Some(AgentError::new(
            "budget_wrong_tabs",
            format!(
                "Asked for {count} tabs and got {}, so these numbers wouldn't mean what they say.",
                distinct.len()
            ),
            exit::FAILED,
        ));
    }
    let after = call("{\"cmd\":\"budget\"}".to_string());
    for tab in opened.iter().rev() {
        let _ = call(json!({ "cmd": "close", "tab": format!("#{tab}") }).to_string());
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let after = after?;
    Ok(json!({
        "route": "through the session",
        "file_bytes": std::fs::metadata(file).map(|meta| meta.len()).ok(),
        "tabs": count,
        "start_footprint_bytes": before["footprint_bytes"],
        "samples": samples,
        "start_handles": before["open_handles"],
        "end_handles": after["open_handles"],
        "ms_per_open": per_open,
    }))
}

fn error_from(value: &Value) -> AgentError {
    let error = &value["error"];
    let exit_code = error["exit"].as_i64().and_then(|code| i32::try_from(code).ok()).unwrap_or(exit::FAILED);
    AgentError::new(
        "session_refused",
        error["message"].as_str().unwrap_or("The session refused this.").to_string(),
        exit_code,
    )
    .then(error["next"].as_str().unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: `recto qa wait-settled --timeout 2m` gives up after the client's own 35 s and says the session didn't answer, while the app is still waiting as it was asked to: found printing 300 pages, whose pages take longer than that to prepare.
    #[test]
    fn a_qa_command_with_a_timeout_is_waited_for_that_long() {
        let settle = Command::QaRead {
            verb: "settled".into(),
            out: None,
            timeout_ms: Some(120_000),
        };
        assert_eq!(client_wait(35, &settle), Duration::from_secs(155));
        let plain = Command::QaRead {
            verb: "state".into(),
            out: None,
            timeout_ms: None,
        };
        assert_eq!(client_wait(35, &plain), Duration::from_secs(35));
    }
}
