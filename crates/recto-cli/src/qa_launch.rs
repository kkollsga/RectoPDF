//! One native quiet-app launcher, shared by recto's CLI and its owning test harness.
use recto_agent::table::Invocation;
use recto_agent::{AgentError, Code, Level, exit};
use recto_control::client::Client;
use recto_control::state::STATE_ENV;
use recto_control::{StatePaths, levels};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long an app that is starting may go with no progress (CPU time or pages read in) before it is called stuck, and how long in all it is given
/// (coordinator, 2026-11-26: a start that is slow but working is waited for; the 30 s of wall clock killed those, and the hint it gave named a flag `qa start` hasn't).
pub const READY_STALL: Duration = Duration::from_secs(10);
pub const READY_CAP: Duration = Duration::from_secs(180);
const MAX_READY_BYTES: usize = 64 * 1024;
const MAX_STDERR_BYTES: usize = 4 * 1024;
pub const OWNER_ENV: &str = "RECTO_QA_OWNER_PID";
fn failed(code: Code, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message.into(), exit::FAILED)
}

/// The app's executable: what was asked for, or beside this `recto` as in the bundle, or the one built in this checkout.
pub fn find_app(given: Option<&Path>) -> Result<PathBuf, AgentError> {
    if let Some(path) = given {
        return a_rectopdf_app(path);
    }
    let this = std::env::current_exe().map_err(|_| failed(Code::APP_MISSING, "This program can't tell where it is."))?;
    let here = this.parent().map(Path::to_path_buf).unwrap_or_default();
    let beside = here.join("RectoPDF");
    if beside.is_file() {
        return Ok(beside);
    }
    for dir in here.ancestors() {
        let built = dir.join("app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF");
        if built.is_file() {
            return Ok(built);
        }
    }
    Err(failed(Code::APP_MISSING, "The app isn't built.").then("Run `tools/sysguard/heavy.sh app/build.sh`, or say where it is with `--app`."))
}

/// **The app a QA command starts is RectoPDF's** (2026-11-13): the executable `RectoPDF` in a `.app` bundle's `Contents/MacOS`, where it really is,
/// whose bundle says RectoPDF's identifier (or a test copy's, under it). Refused before anything is started otherwise.
///
/// What this is: it stops `recto` being a way to run any program already on the machine, with `--qa` and what follows it as its arguments. What it isn't:
/// proof that the bundle is ours, since an identifier can be copied into any bundle; the app started is confined to the temp folders by its state folder,
/// whatever it is, so that isn't what keeps the user's files safe.
pub fn a_rectopdf_app(path: &Path) -> Result<PathBuf, AgentError> {
    let not_ours = || {
        AgentError::new(Code::NOT_RECTOPDF, "That isn't RectoPDF's app, so it wasn't started.", exit::USAGE)
            .then("`--app` takes the RectoPDF executable inside a RectoPDF bundle: `RectoPDF.app/Contents/MacOS/RectoPDF`.")
    };
    let real = path.canonicalize().map_err(|_| failed(Code::APP_MISSING, "The app named isn't there."))?;
    let bundle = real
        .parent()
        .filter(|folder| folder.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|bundle| bundle.extension().is_some_and(|extension| extension == "app"));
    let ours = real.file_name().is_some_and(|name| name == "RectoPDF")
        && bundle
            .and_then(recto_update::stage::bundle_id)
            .is_some_and(|id| id == recto_update::BUNDLE_ID || id.strip_prefix(recto_update::BUNDLE_ID).is_some_and(|rest| rest.starts_with('.')));
    if ours { Ok(real) } else { Err(not_ours()) }
}

/// What a quiet run is started with, past `--qa`.
#[derive(Default)]
pub struct Launch {
    pub memory_limit_mib: Option<u64>,
    pub time_limit: Option<Duration>,
    pub owner_pid: Option<u32>,
    pub open: Vec<PathBuf>,
    pub patience: Patience,
}

/// How patient a launch is: the stall it tolerates and the most it waits in all. A test gives small ones to a stand-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Patience {
    pub stall: Duration,
    pub cap: Duration,
}

impl Default for Patience {
    fn default() -> Self {
        Self {
            stall: READY_STALL,
            cap: READY_CAP,
        }
    }
}

/// What to do about a start that has not said it is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Wait,
    Stalled,
    Capped,
}

/// Wait or stop, from how long the start has run and how long since the process last made progress. The cap is checked first, so a busy start is still ended at it.
#[must_use]
pub fn ready_verdict(elapsed: Duration, since_progress: Duration, patience: Patience) -> Verdict {
    if elapsed >= patience.cap {
        Verdict::Capped
    } else if since_progress >= patience.stall {
        Verdict::Stalled
    } else {
        Verdict::Wait
    }
}

/// What a process had used at a moment: CPU time, and pages read in (a start held up by swap reads pages and uses almost no CPU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marks {
    pub cpu_ns: u64,
    pub page_ins: u64,
}

pub fn marks_of(pid: u32) -> Option<Marks> {
    recto_platform::local::process_counters(pid).map(|counters| Marks {
        cpu_ns: counters.cpu_ns,
        page_ins: counters.page_ins,
    })
}

/// Whether the process has made progress since `mark`, which moves to `now` when it has. A page-in is progress. CPU is progress only at a hundredth of the stall
/// since the mark (100 ms in 10 s): the app's own watchdog wakes every 250 ms while its UI thread is stuck, and those wake-ups are not a start working (review, 2026-11-26).
pub fn progressed(mark: &mut Marks, now: Marks, patience: Patience) -> bool {
    let enough = u64::try_from(patience.stall.as_nanos() / 100).unwrap_or(u64::MAX);
    let worked = now.page_ins > mark.page_ins || now.cpu_ns.saturating_sub(mark.cpu_ns) >= enough;
    if worked {
        *mark = now;
    }
    worked
}

/// Starts the app and waits for its ready line.
pub fn launch(app: &Path, paths: &StatePaths, options: &Launch, environment: &[(&str, &std::ffi::OsStr)]) -> Result<(Child, Value), AgentError> {
    paths.prepare()?;
    levels::store(paths, Level::Full).map_err(|_| failed(Code::WRITE_FAILED, "The state folder couldn't be written to."))?;
    // The run before's messages are kept for one launch more: a test that compares runs on one state folder fails once the next has started, and what the run before said went with this launch's truncation (CI, 2026-11-12: the contents' report on a restart, silent in the logs kept).
    let _ = std::fs::rename(&paths.server_messages, paths.server_messages.with_file_name("server-messages.previous.txt"));
    let messages = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&paths.server_messages)
        .map_err(|_| failed(Code::WRITE_FAILED, "The state folder couldn't be written to."))?;
    let mut command = Command::new(app);
    command.arg("--qa");
    if let Some(limit) = options.memory_limit_mib {
        command.arg("--qa-memory-limit-mib").arg(limit.to_string());
    }
    if let Some(limit) = options.time_limit {
        command.arg("--qa-time-limit-ms").arg(limit.as_millis().to_string());
    }
    if let Some(pid) = options.owner_pid {
        command.arg("--qa-owner-pid").arg(pid.to_string());
    }
    command.args(&options.open);
    command
        .envs(environment.iter().copied())
        .env(STATE_ENV, &paths.root)
        .env("MallocSpaceEfficient", "1")
        .env("MallocMaxMagazines", "1")
        .current_dir(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(messages);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|_| failed(Code::APP_MISSING, format!("The app at {} couldn't be started.", app.display())))?;
    let Some(stdout) = child.stdout.take() else {
        return Err(cleanup_error(
            &mut child,
            failed(Code::APP_REFUSED, "The app couldn't be started."),
            Duration::from_secs(1),
        ));
    };
    let (send, receive) = mpsc::channel();
    let _ = std::thread::Builder::new().name("qa-ready".into()).spawn(move || {
        let _ = send.send(read_ready_line(BufReader::new(stdout), MAX_READY_BYTES));
    });
    let said = wait_ready(&receive, child.id(), options.patience);
    let line = match said {
        Ok(Ok(line)) if !line.is_empty() => line,
        Ok(Err(error)) => {
            let primary = failed(Code::APP_REFUSED, format!("The app's ready line couldn't be read: {error}"));
            return Err(cleanup_error(&mut child, primary, Duration::from_secs(1)));
        }
        Ok(Ok(_)) => {
            let message = stderr_excerpt(&paths.server_messages, MAX_STDERR_BYTES);
            let error = failed(
                Code::APP_REFUSED,
                if message.trim().is_empty() {
                    "The app ended before it was ready.".to_string()
                } else {
                    message.trim().to_string()
                },
            );
            return Err(cleanup_error(&mut child, error, Duration::from_secs(1)));
        }
        Err(verdict) => {
            let words = match verdict {
                Verdict::Capped => format!("The app was still starting after {} s, so it was stopped.", options.patience.cap.as_secs()),
                _ => format!(
                    "The app made no progress for {} s while starting, so it was stopped.",
                    options.patience.stall.as_secs()
                ),
            };
            let error = AgentError::new(Code::TIMED_OUT, words, exit::TIMED_OUT)
                .then("Try again when the Mac is less busy; `recto qa start` has no time option for this.");
            return Err(cleanup_error(&mut child, error, Duration::from_secs(1)));
        }
    };
    let value: Value = match serde_json::from_str(line.trim()) {
        Ok(value) => value,
        Err(_) => {
            return Err(cleanup_error(
                &mut child,
                failed(Code::APP_REFUSED, "The app said something that isn't its ready line."),
                Duration::from_secs(1),
            ));
        }
    };
    let ready = ready_value(&mut child, value, Duration::from_secs(1))?;
    Ok((child, ready))
}

/// The ready line, or why the wait ended without one. Progress is read about four times a stall, so a stuck start is called within a quarter of it past.
fn wait_ready(receive: &mpsc::Receiver<std::io::Result<String>>, pid: u32, patience: Patience) -> Result<std::io::Result<String>, Verdict> {
    let began = Instant::now();
    let mut mark = marks_of(pid);
    let mut last_progress = began;
    let poll = (patience.stall / 4).clamp(Duration::from_millis(10), Duration::from_millis(500));
    loop {
        match receive.recv_timeout(poll) {
            Ok(said) => return Ok(said),
            // The reader ended without a line: the caller reads the app's end from its (empty) line.
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(Ok(String::new())),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Counters that can't be read say nothing about progress, so only the cap ends such a wait.
        match (mark.as_mut(), marks_of(pid)) {
            (Some(mark), Some(now)) => {
                if progressed(mark, now, patience) {
                    last_progress = Instant::now();
                }
            }
            (_, now) => {
                mark = now;
                last_progress = Instant::now();
            }
        }
        match ready_verdict(began.elapsed(), last_progress.elapsed(), patience) {
            Verdict::Wait => {}
            stop => return Err(stop),
        }
    }
}

/// A readiness refusal or false process identity still owns the child until bounded cleanup.
pub fn ready_value(child: &mut Child, value: Value, cleanup_wait: Duration) -> Result<Value, AgentError> {
    let error = if value["ok"] != true {
        Some(failed(
            Code::APP_REFUSED,
            value["error"]["message"].as_str().unwrap_or("The app refused to start.").to_string(),
        ))
    } else if value["data"]["pid"].as_u64() != Some(u64::from(child.id())) {
        Some(failed(Code::APP_REFUSED, "The app's ready line didn't identify the process that was started."))
    } else {
        None
    };
    if let Some(error) = error {
        return Err(cleanup_error(child, error, cleanup_wait));
    }
    Ok(value["data"].clone())
}

/// The process a run's to end with: `--owner`, or else the environment's. One that isn't running would end the run as it starts, so it's refused here, with the words.
pub fn owner(given: Option<u64>, from_env: Option<&str>) -> Result<Option<u32>, AgentError> {
    let pid = match (given, from_env.map(str::trim).filter(|text| !text.is_empty())) {
        (Some(pid), _) => u32::try_from(pid).ok(),
        (None, Some(text)) => Some(
            text.parse::<u32>()
                .map_err(|_| AgentError::new(Code::USAGE, format!("{OWNER_ENV} is a process number."), exit::USAGE))?,
        ),
        (None, None) => return Ok(None),
    };
    match pid {
        Some(pid) if pid > 1 && recto_platform::local::process_exists(pid) => Ok(Some(pid)),
        _ => Err(failed(Code::OWNER_GONE, "The process the run's to end with isn't running.")
            .then(format!("Leave out --owner and {OWNER_ENV}, or name one that's running."))),
    }
}

/// The public CLI's parsed invocation, with the same validation and launch for an owning caller.
pub fn start(
    invocation: &Invocation,
    paths: StatePaths,
    owner_from_env: Option<&str>,
    environment: &[(&str, &std::ffi::OsStr)],
) -> Result<(Child, Value), AgentError> {
    let paths = paths.for_a_quiet_run()?;
    if Client::connect(&paths).is_ok() {
        return Err(failed(Code::ALREADY_RUNNING, "Something is already running on this state folder.").then("`recto stop` ends it."));
    }
    let app = find_app(invocation.fields.get("app").and_then(Value::as_str).map(Path::new))?;
    let options = Launch {
        memory_limit_mib: invocation.fields.get("memory_limit_mib").and_then(Value::as_u64),
        time_limit: invocation.fields.get("time_limit_ms").and_then(Value::as_u64).map(Duration::from_millis),
        owner_pid: owner(invocation.fields.get("owner_pid").and_then(Value::as_u64), owner_from_env)?,
        open: Vec::new(),
        patience: Patience::default(),
    };
    let (child, ready) = launch(&app, &paths, &options, environment)?;
    Ok((
        child,
        json!({ "running": true, "pid": ready["pid"], "socket": ready["socket"], "state": paths.root, "quiet": true, "owner": options.owner_pid }),
    ))
}

/// A failed launch still owns its child. Request exit first, then observe/reap within the bound, preserving the readiness error.
pub fn cleanup_error(child: &mut Child, mut primary: AgentError, limit: Duration) -> AgentError {
    match child.try_wait() {
        Ok(Some(_)) => return primary,
        Err(error) => {
            primary.message.push_str(&format!(
                " Cleanup could not establish child {} ownership: {error}; no signal was sent.",
                child.id()
            ));
            return primary;
        }
        Ok(None) => {}
    }
    if let Err(error) = child.kill() {
        if child.try_wait().ok().flatten().is_some() {
            return primary;
        }
        primary
            .message
            .push_str(&format!(" Cleanup could not kill owned child {}: {error}; exit was not observed.", child.id()));
        return primary;
    }
    let began = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return primary,
            Err(error) => {
                primary
                    .message
                    .push_str(&format!(" Cleanup could not reap owned child {}: {error}.", child.id()));
                return primary;
            }
            Ok(None) => {}
        }
        let Some(left) = limit.checked_sub(began.elapsed()) else {
            primary.message.push_str(&format!(
                " Cleanup did not observe owned child {} exit within {limit:?} after kill; it may remain alive.",
                child.id()
            ));
            return primary;
        };
        std::thread::sleep(left.min(Duration::from_millis(10))); // ratchet: poll
    }
}

/// One bounded first line, including its newline. The extra byte detects an overlong external reply without reading to its end.
pub fn read_ready_line(reader: impl BufRead, limit: usize) -> std::io::Result<String> {
    let mut line = String::with_capacity(limit.saturating_add(1));
    let mut bounded = std::io::Read::take(reader, u64::try_from(limit).unwrap_or(u64::MAX - 1).saturating_add(1));
    bounded.read_line(&mut line)?;
    if line.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ready line exceeds {limit} bytes"),
        ));
    }
    Ok(line)
}

/// A startup error keeps only a bounded excerpt of the owned stderr file, never follows a replacement link or waits on a FIFO.
pub fn stderr_excerpt(path: &Path, limit: usize) -> String {
    let file = match recto_platform::local::open_private_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return String::new(),
        Err(error) => return format!("[stderr unavailable: {error}]"),
    };
    let mut bytes = Vec::with_capacity(limit.saturating_add(1));
    if let Err(error) = file
        .take(u64::try_from(limit).unwrap_or(u64::MAX - 1).saturating_add(1))
        .read_to_end(&mut bytes)
    {
        return format!("[stderr unavailable: {error}]");
    }
    let mut truncated = bytes.len() > limit;
    bytes.truncate(limit);
    let mut excerpt = String::from_utf8_lossy(&bytes).into_owned();
    if excerpt.len() > limit {
        let mut end = limit;
        while !excerpt.is_char_boundary(end) {
            end -= 1;
        }
        excerpt.truncate(end);
        truncated = true;
    }
    if truncated {
        excerpt.push_str(&format!("\n[stderr truncated after {limit} bytes]"));
    }
    excerpt
}
