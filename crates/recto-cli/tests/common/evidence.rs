//! **One failure is enough** (owner, 2026-11-12: "What's the solution to ensure we are able to debug and fix issues after one failure?"; PLAN R0.7a and b). A failing test that started the app leaves one folder of evidence, made here by the harness and not by each test: every launch's messages, `qa state` as each launch was last read (a comparison's before and after, as its two launches), the app's threads by name and a `sample` of it while it's still there, the slow mode's seams crossed and its seed, the system pressure the app counted, the machine's load, memory and OS, what Launch Services has registered of this tree, the test runner's command line and the settings that matter, and the one command that runs it again here as it ran there. On CI a failure there cost a rerun each to learn anything (2026-11-12: messages overwritten by the next launch, a thread count that said how many and not which, a leak seen one run in three).
//!
//! **Where.** `RECTO_EVIDENCE` names the root: `tools/check.sh` sets it to `target/recto-evidence/<stage>` and empties `target/recto-evidence` at its start, and CI to a folder it uploads when a part fails. Without it, `target/recto-evidence/adhoc/<the runner's run>`, and the first failure of a run removes what older runs left there. One folder a test, `<binary>--<test>`, at most [`FOLDER_CAP`] bytes.
//!
//! **Nothing of the owner's.** Every file copied is from the test's own state folder, which is in temp and never the real one ([`super::State::new`]'s override refuses that), and every document a test opens is one it made (`recto_testpdf`). Settings are kept by an allow-list of names; Launch Services is listed only for this checkout and the tests' temp folders; other processes are listed by short name and load only.

use super::State;
use super::threads;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

/// The root of the evidence folders, from `tools/check.sh` or CI.
pub const ROOT_ENV: &str = "RECTO_EVIDENCE";
/// The most one test's folder holds: a file that would go past it is cut, and says so.
pub const FOLDER_CAP: u64 = 16 * 1024 * 1024;
/// The most of one log kept: its end, where a run's last words are.
const LOG_CAP: usize = 1024 * 1024;
/// How long `sample` looks at the app, in seconds.
const SAMPLE_SECONDS: u32 = 3;

/// The settings that change what a test does or say where it ran, kept with their values. Any other `RECTO_` setting is named without its value, as a folder of the owner's could be in one (`RECTO_BENCH100`).
const SETTINGS: [&str; 18] = [
    "RECTO_QA_SLOW",
    "RECTO_QA_SLOW_HOLD",
    "RECTO_QA_RESTORE_DELAY_MS",
    "RECTO_QA_REMAKE_DELAY_MS",
    "RECTO_REQUIRE_APP",
    "RECTO_SKIP_APP",
    "RECTO_EVIDENCE",
    "NEXTEST_PROFILE",
    "NEXTEST_RUN_ID",
    "NEXTEST_TEST_THREADS",
    "NEXTEST_RETRIES",
    "CI",
    "GITHUB_ACTIONS",
    "GITHUB_RUN_ID",
    "GITHUB_SHA",
    "CHECK_PART",
    "CHECK_STAGE",
    "CHECK_STAGE_JOBS",
];

/// One launch of the app this test process made or met.
struct Launch {
    pid: u32,
    state_root: PathBuf,
    how: String,
    /// What the state folder's messages held as this launch began: the launch before's, which the app keeps one launch back only.
    messages_before: Option<String>,
    /// The state folder's files as this launch began, with sizes and times, so a file written later shows (a sleep file a quit rewrote, 2026-11-12).
    files_before: String,
    last_state: Option<Value>,
}

static LAUNCHES: Mutex<Vec<Launch>> = Mutex::new(Vec::new());
/// The panic's words and place, for the summary.
static PANIC: Mutex<Option<String>> = Mutex::new(None);
static HOOK: Once = Once::new();
static WRITTEN: Once = Once::new();

/// A launch of the app on `state`, before it starts: its folder's messages so far are the launch before's, kept here as the next launch moves them along.
pub fn launching(state: &State) -> usize {
    HOOK.call_once(|| {
        let earlier = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            *PANIC.lock().unwrap_or_else(PoisonError::into_inner) = Some(format!("{info}"));
            earlier(info);
        }));
    });
    let messages_before = std::fs::read_to_string(&state.paths.server_messages).ok().map(|text| tail(&text, LOG_CAP));
    let files_before = files_of(&state.root);
    let mut launches = LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner);
    launches.push(Launch {
        pid: 0,
        state_root: state.root.clone(),
        how: "qa start".into(),
        messages_before,
        files_before,
        last_state: None,
    });
    launches.len() - 1
}

/// The launch's process, once it's known.
pub fn launched(index: usize, pid: u32) {
    if let Some(launch) = LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner).get_mut(index) {
        launch.pid = pid;
    }
}

/// A run some other way started, such as the app an update put in place.
pub fn met(state: &State, pid: u32, how: &str) {
    LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner).push(Launch {
        pid,
        state_root: state.root.clone(),
        how: how.into(),
        messages_before: None,
        files_before: files_of(&state.root),
        last_state: None,
    });
}

/// `qa state` as a test just read it from this process.
pub fn read_state(pid: u32, report: &Value) {
    let mut launches = LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(launch) = launches.iter_mut().rev().find(|launch| launch.pid == pid) {
        launch.last_state = Some(report.clone());
    }
}

/// What can only be had while the app's still there, from a run dropped as its test fails: `qa state` now, its threads by name, and a `sample`.
pub fn while_running(state: &State, pid: u32) {
    let Some(mut folder) = Folder::of_this_test() else { return };
    let index = LAUNCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rposition(|launch| launch.pid == pid)
        .unwrap_or(usize::MAX);
    let name = |what: &str| {
        if index == usize::MAX {
            format!("pid-{pid}-{what}")
        } else {
            format!("launch-{}-{what}", index + 1)
        }
    };
    if !super::app::still_running(pid) {
        folder.put(&name("gone.txt"), format!("process {pid} had ended when its test failed\n").as_bytes());
        return;
    }
    let mut asked = state.command();
    asked.args(["qa", "state", "--json"]);
    if let Some(out) = bounded(asked, Duration::from_secs(10)) {
        folder.put(&name("qa-state-at-failure.json"), &out.stdout);
    }
    folder.put(&name("threads.txt"), format!("{}\n", threads::named(pid)).as_bytes());
    // Into the folder, not temp, where it writes by default; then cut to the cap like anything else.
    let into = folder.path.join(name("sample.txt"));
    let mut sample = Command::new("/usr/bin/sample");
    sample
        .arg(pid.to_string())
        .arg(SAMPLE_SECONDS.to_string())
        .arg("-mayDie")
        .arg("-file")
        .arg(&into);
    match bounded(sample, Duration::from_secs(u64::from(SAMPLE_SECONDS) + 15)) {
        Some(out) if out.status.success() && into.is_file() => {
            let text = std::fs::read(&into).unwrap_or_default();
            let _ = std::fs::remove_file(&into);
            folder.put(&name("sample.txt"), &text);
        }
        Some(out) => folder.put(
            &name("sample.txt"),
            format!("sample failed ({}): {}\n", out.status, String::from_utf8_lossy(&out.stderr)).as_bytes(),
        ),
        None => folder.put(&name("sample.txt"), b"sample didn't end within its bound\n"),
    }
}

/// The rest, from a state folder dropped as its test fails: its messages, and once a test the launches, the machine, the settings, Launch Services and the command that runs it again. Nothing for a test that never started the app.
pub fn at_failure(state: &State) {
    if !LAUNCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|launch| launch.state_root == state.root)
    {
        return;
    }
    let Some(mut folder) = Folder::of_this_test() else { return };
    let own = state.root.file_name().unwrap_or_default().to_string_lossy().to_string();
    let launches = LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner);
    for (index, launch) in launches.iter().enumerate().filter(|(_, launch)| launch.state_root == state.root) {
        if let Some(text) = &launch.messages_before {
            folder.put(&format!("messages/{own}/before-launch-{}.txt", index + 1), text.as_bytes());
        }
        folder.put(&format!("messages/{own}/files-before-launch-{}.txt", index + 1), launch.files_before.as_bytes());
    }
    drop(launches);
    folder.put(&format!("messages/{own}/files-at-failure.txt"), files_of(&state.root).as_bytes());
    let paths = &state.paths;
    let logs = [
        paths.server_messages.clone(),
        paths.server_messages.with_file_name("server-messages.previous.txt"),
        state.root.join("update.log"),
        paths.log.clone(),
        paths.old_log.clone(),
        paths.other_log.clone(),
        paths.old_other_log.clone(),
    ];
    for log in logs {
        if let (Ok(text), Some(name)) = (std::fs::read_to_string(&log), log.file_name()) {
            folder.put(&format!("messages/{own}/{}", name.to_string_lossy()), tail(&text, LOG_CAP).as_bytes());
        }
    }
    WRITTEN.call_once(|| write_once(&mut folder));
}

/// What's written once a test: the summary, the launches with their last `qa state`, the machine, the settings, Launch Services, and how to run it again.
fn write_once(folder: &mut Folder) {
    let launches = LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner);
    let mut said = Vec::new();
    let mut last_slow = Value::Null;
    for (index, launch) in launches.iter().enumerate() {
        let n = index + 1;
        if let Some(report) = &launch.last_state {
            folder.put(
                &format!("launch-{n}-qa-state.json"),
                serde_json::to_string_pretty(report).unwrap_or_default().as_bytes(),
            );
            if !report["slow"].is_null() {
                last_slow = report["slow"].clone();
            }
        }
        let report = launch.last_state.as_ref();
        said.push(json!({
            "launch": n,
            "how": launch.how,
            "pid": launch.pid,
            "state_folder": launch.state_root,
            "qa_state_read": report.is_some(),
            "slow": report.map_or(Value::Null, |report| report["slow"].clone()),
            "system_pressure": report.map_or(Value::Null, |report| report["record"]["sleep"]["system_pressure"].clone()),
        }));
    }
    drop(launches);
    folder.put("launches.json", serde_json::to_string_pretty(&said).unwrap_or_default().as_bytes());
    if !last_slow.is_null() {
        folder.put("slow.json", serde_json::to_string_pretty(&last_slow).unwrap_or_default().as_bytes());
    }
    folder.put("machine.txt", machine().as_bytes());
    folder.put("settings.txt", settings().as_bytes());
    folder.put("launch-services.txt", launch_services().as_bytes());
    let again = rerun_line(&last_slow);
    folder.put("rerun.txt", format!("{again}\n").as_bytes());
    let panic = PANIC
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| "(no panic message caught)".into());
    let summary = format!(
        "test: {}\nbinary: {}\nfailed: {}\nlaunches: {}\nrun it again here as it ran: {again}\nevidence: {}\n",
        test_name(),
        binary_id(),
        tail(&panic, 4000),
        said.len(),
        folder.path.display()
    );
    folder.put("summary.txt", summary.as_bytes());
    eprintln!(
        "\nevidence of this failure: {}\nrun it again here as it ran:\n  {again}\n",
        folder.path.display()
    );
}

/// A state folder's files, a line each: size, modification time to the microsecond, path. Names and times only, nothing read: cheap enough for every launch.
fn files_of(root: &Path) -> String {
    fn walk(base: &Path, at: &Path, into: &mut Vec<String>, depth: usize) {
        for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 4 {
                    walk(base, &path, into, depth + 1);
                }
                continue;
            }
            let at = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or_else(|| "?".to_owned(), |since| format!("{}.{:06}", since.as_secs(), since.subsec_micros()));
            into.push(format!("{:>10} {at} {}", meta.len(), path.strip_prefix(base).unwrap_or(&path).display()));
        }
    }
    let mut lines = Vec::new();
    walk(root, root, &mut lines, 0);
    lines.sort_by(|a, b| a.split_whitespace().nth(2).cmp(&b.split_whitespace().nth(2)));
    lines.join("\n") + "\n"
}

/// The one command that runs this test again here as it ran: CI's limits, its profile and concurrency (`tools/check.sh --like-ci`), and the slow mode's seams and seed as this run met them.
pub fn rerun_line(slow: &Value) -> String {
    let filter = format!("binary_id(={}) & test(={})", binary_id(), test_name());
    let mut env = Vec::new();
    if let Some(switch) = slow_switch(slow).or_else(|| std::env::var("RECTO_QA_SLOW").ok()) {
        env.push(format!("RECTO_QA_SLOW='{switch}'"));
    }
    if let Ok(hold) = std::env::var("RECTO_QA_SLOW_HOLD") {
        env.push(format!("RECTO_QA_SLOW_HOLD='{hold}'"));
    }
    let profile = std::env::var("NEXTEST_PROFILE").unwrap_or_else(|_| "default".into());
    let jobs = std::env::var("CHECK_STAGE_JOBS").ok().filter(|jobs| !jobs.is_empty());
    let mut line = env.join(" ");
    if !line.is_empty() {
        line.push(' ');
    }
    line.push_str(&format!("tools/check.sh --like-ci '{filter}' --profile {profile}"));
    if let Some(jobs) = jobs {
        line.push_str(&format!(" --jobs {jobs}"));
    }
    line
}

/// `RECTO_QA_SLOW` again from what a run's slow mode said it was: each seam's delay, and the jitter's seed when there was one. `None` with the slow mode off.
pub fn slow_switch(slow: &Value) -> Option<String> {
    let seams = slow["seams"].as_object()?;
    let mut items: Vec<String> = seams
        .iter()
        .filter_map(|(seam, said)| said["ms"].as_u64().filter(|ms| *ms > 0).map(|ms| format!("{seam}={ms}")))
        .collect();
    if let Some(seed) = slow["jitter"].as_u64() {
        items.push(format!("jitter={seed}"));
    }
    (!items.is_empty()).then(|| items.join(","))
}

/// The test's name, as the runner gave it to its thread.
pub fn test_name() -> String {
    std::env::var("NEXTEST_TEST_NAME")
        .ok()
        .or_else(|| std::thread::current().name().map(str::to_owned))
        .unwrap_or_else(|| "unnamed".into())
}

/// `recto-cli::app` and the like, as nextest names a test binary: from nextest, or from the executable's name less cargo's hash.
pub fn binary_id() -> String {
    if let Ok(id) = std::env::var("NEXTEST_BINARY_ID") {
        return id;
    }
    let exe = std::env::current_exe().ok();
    let stem = exe
        .as_deref()
        .and_then(Path::file_stem)
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = stem.rsplit_once('-').map_or(stem.as_str(), |(name, _)| name);
    format!("recto-cli::{name}")
}

/// The machine as the test failed: load, memory, cores, the OS, and what's using the CPU, by short name.
fn machine() -> String {
    let mut text = String::new();
    for (what, program, args) in [
        ("OS", "/usr/bin/sw_vers", &[][..]),
        ("load and uptime", "/usr/bin/uptime", &[][..]),
        (
            "cores, memory",
            "/usr/sbin/sysctl",
            &[
                "hw.ncpu",
                "hw.perflevel0.physicalcpu",
                "hw.perflevel1.physicalcpu",
                "hw.memsize",
                "vm.loadavg",
                "vm.swapusage",
            ][..],
        ),
        ("memory pressure", "/usr/bin/memory_pressure", &["-Q"][..]),
        ("pages", "/usr/bin/vm_stat", &[][..]),
        ("busiest processes", "/bin/ps", &["-A", "-c", "-r", "-o", "pid=,%cpu=,%mem=,comm="][..]),
    ] {
        let mut command = Command::new(program);
        command.args(args);
        let out = bounded(command, Duration::from_secs(10))
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|| "(didn't answer)".into());
        let out: String = if what == "busiest processes" {
            out.lines().take(12).collect::<Vec<_>>().join("\n")
        } else {
            out
        };
        text.push_str(&format!("## {what}\n{}\n\n", out.trim_end()));
    }
    text
}

/// The settings that matter, and the runner's command line, from this process's parents.
fn settings() -> String {
    let mut text = String::from("## settings\n");
    let mut other = Vec::new();
    for (name, value) in std::env::vars() {
        if SETTINGS.contains(&name.as_str()) {
            text.push_str(&format!("{name}={value}\n"));
        } else if name.starts_with("RECTO_") || name.starts_with("NEXTEST_") {
            other.push(name);
        }
    }
    other.sort();
    if !other.is_empty() {
        text.push_str(&format!("set, values left out: {}\n", other.join(" ")));
    }
    // This process, then its parents as long as they're the test runner or the check: not the shell or the terminal past them, whose lines are the owner's.
    text.push_str("\n## the commands this ran under (this process, and the runner and check above it)\n");
    let mut pid = std::process::id();
    for depth in 0..8 {
        let mut ps = Command::new("/bin/ps");
        ps.args(["-o", "ppid=,command=", "-p", &pid.to_string()]);
        let Some(out) = bounded(ps, Duration::from_secs(5)) else { break };
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let Some((parent, command)) = line.split_once(' ') else { break };
        let ours = ["nextest", "check.sh", "heavy.sh", "cargo"].iter().any(|tool| command.contains(tool));
        if depth > 0 && !ours {
            break;
        }
        text.push_str(&format!("{pid}: {}\n", command.trim().chars().take(2000).collect::<String>()));
        match parent.trim().parse::<u32>() {
            Ok(parent) if parent > 1 => pid = parent,
            _ => break,
        }
    }
    text
}

/// What Launch Services has registered of the app in this checkout and in the tests' temp folders, and how many copies elsewhere, which aren't named.
fn launch_services() -> String {
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_default();
    let temp = std::env::temp_dir().canonicalize().unwrap_or_default();
    let (sender, receiver) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("evidence-ls".into()).spawn(move || {
        let _ = sender.send(recto_platform::registry::registered());
    });
    let began = Instant::now();
    let Ok(all) = receiver.recv_timeout(Duration::from_secs(30)) else {
        return format!("{DUMP_TIMED_OUT} ({} s)\n", began.elapsed().as_secs());
    };
    recto_platform::registry::describe(&all, &checkout, &temp)
}

/// What the file says when the system's dump (seconds of work on a quiet machine) didn't answer in time: CI, 2026-11-13, five app tests dumping at once on a runner under load.
pub const DUMP_TIMED_OUT: &str = "Launch Services' dump didn't answer within 30 s";
/// What the file starts with when it did.
pub const DUMP_ANSWERED: &str = recto_platform::registry::DESCRIBED;

/// A command's output, or `None` if it didn't end within `limit` (it's killed then) or couldn't start.
pub fn bounded(mut command: Command, limit: Duration) -> Option<std::process::Output> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let reading = std::thread::Builder::new()
        .name("evidence-read".into())
        .spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            out
        })
        .ok()?;
    let began = Instant::now();
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if began.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20)); // ratchet: poll
    };
    let mut err = Vec::new();
    let _ = stderr.read_to_end(&mut err);
    let out = reading.join().unwrap_or_default();
    Some(std::process::Output {
        status: status?,
        stdout: out,
        stderr: err,
    })
}

fn tail(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let mut start = text.len() - cap;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("(the first {start} bytes left out)\n{}", &text[start..])
}

/// The root, as `tools/check.sh` or CI set it, or this run's own under the target folder, where the first failure of a run clears what older runs left.
pub fn root() -> PathBuf {
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        return PathBuf::from(root);
    }
    // target/<profile>/deps/<binary>: the target folder, CARGO_TARGET_DIR's too.
    let target = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let adhoc = target.join("recto-evidence/adhoc");
    let run = std::env::var("NEXTEST_RUN_ID").unwrap_or_else(|_| format!("process-{}", std::process::id()));
    let folder = adhoc.join(run);
    let _ = std::fs::create_dir_all(&adhoc);
    if std::fs::create_dir(&folder).is_ok() {
        // The first failure of this run: what older runs left goes, unless it's from the last ten minutes, which can be another run's under way.
        for entry in std::fs::read_dir(&adhoc).into_iter().flatten().flatten() {
            let old = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(600));
            if entry.path() != folder && old {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    folder
}

/// One test's folder, and how much of its cap it's used.
struct Folder {
    path: PathBuf,
}

static FOLDERS: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());

impl Folder {
    /// This test's, made the first time it's asked for.
    fn of_this_test() -> Option<Self> {
        let test = test_name();
        let mut folders = FOLDERS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(path) = folders.get(&test) {
            return Some(Self { path: path.clone() });
        }
        let clean = |text: &str| -> String { text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '-' }).collect() };
        let base = root().join(format!("{}--{}", clean(&binary_id()), clean(&test)));
        let mut path = base.clone();
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = PathBuf::from(format!("{}-{n}", base.display()));
        }
        std::fs::create_dir_all(&path).ok()?;
        folders.insert(test, path.clone());
        Some(Self { path })
    }

    fn used(&self) -> u64 {
        fn size(folder: &Path) -> u64 {
            std::fs::read_dir(folder)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| match entry.metadata() {
                    Ok(meta) if meta.is_dir() => size(&entry.path()),
                    Ok(meta) => meta.len(),
                    Err(_) => 0,
                })
                .sum()
        }
        size(&self.path)
    }

    /// Writes a file, cut to what's left of the cap.
    fn put(&mut self, name: &str, bytes: &[u8]) {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let room = FOLDER_CAP.saturating_sub(self.used()) as usize;
        let note = b"\n(cut here: the evidence folder is at its cap)\n";
        let Ok(mut file) = std::fs::File::create(&path) else { return };
        if bytes.len() <= room {
            let _ = file.write_all(bytes);
        } else {
            let _ = file.write_all(&bytes[..room.saturating_sub(note.len())]);
            let _ = file.write_all(note);
        }
    }
}
