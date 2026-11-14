//! Runs the real `recto` on temp state, the way an agent does. Every server these start ends with its [`Session`], and nothing here can reach the real state folder, because the override refuses it.
#![allow(dead_code)]

/// The real app in a quiet run.
pub mod app;
/// Whose a bed's temp folder is, by its name.
pub mod bed_folder;
/// What a failing test that started the app leaves behind, and how to run it again.
pub mod evidence;
/// A process's threads by name, for a count's failure to say which they were.
pub mod threads;

use recto_agent::Level;
use recto_control::state::STATE_ENV;
use recto_control::{StatePaths, levels};
use serde_json::{Value, json};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

static NEXT: AtomicU32 = AtomicU32::new(0);

pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_recto")
}

/// A state folder of its own, removed on drop.
pub struct State {
    pub root: PathBuf,
    pub paths: StatePaths,
}

/// A name for a state folder that nothing has. Short, so a socket fits in it even when the temp folder's path is long. A name that's taken is passed over: a run that was killed leaves its folder behind, a process id comes round again, and a folder left at another mode reads as "isn't private" to whichever test lands on it. It isn't made here, since not making state is what some tests are about, and only this process uses names with its id in them.
pub fn free_folder() -> PathBuf {
    let temp = std::env::temp_dir().canonicalize().unwrap();
    loop {
        let root = temp.join(format!("rc{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        if std::fs::symlink_metadata(&root).is_err() {
            return root;
        }
    }
}

/// The name [`free_folder`] tries next, for the test of what it does when that's taken.
pub fn next_folder_name() -> PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("rc{}-{}", std::process::id(), NEXT.load(Ordering::Relaxed)))
}

impl State {
    pub fn new() -> Self {
        let root = free_folder();
        let paths = StatePaths::overridden(&root).unwrap();
        assert!(paths.overridden);
        Self { root, paths }
    }

    pub fn at(level: Level) -> Self {
        let state = Self::new();
        state.set_level(level);
        state
    }

    /// Stands in for the user. This writes the level's file, as `recto permission set` does after its terminal check, which has its own test.
    pub fn set_level(&self, level: Level) {
        levels::store(&self.paths, level).unwrap();
    }

    /// `recto` on this state. A quiet run it starts ends with this test's process, so a test that's killed leaves no app behind.
    pub fn command(&self) -> Command {
        let mut command = Command::new(bin());
        command
            .env(STATE_ENV, &self.root)
            .env("RECTO_QA_OWNER_PID", std::process::id().to_string())
            .stdin(Stdio::null());
        command
    }

    /// Runs `recto` and reads its one JSON answer.
    pub fn json(&self, args: &[&str]) -> (Value, i32) {
        let output = self.command().args(args).arg("--json").stderr(Stdio::inherit()).output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| panic!("not JSON from {args:?}: {text:?}"));
        (value, output.status.code().unwrap())
    }

    /// Runs `recto` with this on its stdin, as `qa password` takes a password, and reads its one JSON answer.
    #[allow(dead_code)] // Not every test binary types a password.
    pub fn json_with_stdin(&self, args: &[&str], input: &str) -> (Value, i32) {
        use std::io::Write;
        let mut child = self
            .command()
            .args(args)
            .arg("--json")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| panic!("not JSON from {args:?}: {text:?}"));
        (value, output.status.code().unwrap())
    }

    /// Runs `recto` and returns what a person would see.
    pub fn text(&self, args: &[&str]) -> (String, String, i32) {
        let output = self.command().args(args).output().unwrap();
        (
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
            output.status.code().unwrap(),
        )
    }

    /// Starts a session and waits for its ready line.
    pub fn serve(&self) -> Serving {
        let mut child = self
            .command()
            .args(["serve", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line).unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("no ready line: {line:?}"));
        assert_eq!(ready["ok"], true, "{ready}");
        Serving { child }
    }

    pub fn socket(&self) -> &Path {
        &self.paths.socket
    }

    /// Waits until nothing answers at this state's socket, failing 15 s after `began`: a process that can't be asked about any more can still be
    /// closing it, and a run started on the same folder before it has is refused as already running (a full check at a load of 27, 2026-11-14).
    #[allow(dead_code)] // Not every test binary starts the app.
    pub fn wait_for_socket_to_close(&self, began: std::time::Instant) {
        while std::os::unix::net::UnixStream::connect(&self.paths.socket).is_ok() {
            assert!(began.elapsed() < Duration::from_secs(15), "the socket stayed");
            std::thread::sleep(Duration::from_millis(10)); // ratchet: poll
        }
    }

    pub fn log_text(&self) -> String {
        [&self.paths.log, &self.paths.old_log, &self.paths.other_log, &self.paths.old_other_log]
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap_or_default())
            .collect()
    }
}

/// Where a failing test's quiet-run logs are kept, past its state folder's removal: CI sets it and uploads what's there (`.github/workflows/ci.yml`).
pub const KEEP_LOGS_ENV: &str = "RECTO_TEST_LOGS";
/// The most of one log a failure message carries.
const LAST_WORDS_LINES: usize = 30;

impl State {
    /// The logs a run writes in its state folder: the app's own messages (its stderr, where a panic or a quiet run's end is said), the update log, and the activity logs.
    fn log_files(&self) -> Vec<PathBuf> {
        let paths = &self.paths;
        [&paths.server_messages, &paths.log, &paths.old_log, &paths.other_log, &paths.old_other_log]
            .into_iter()
            .cloned()
            .chain([
                self.root.join("update.log"),
                paths.server_messages.with_file_name("server-messages.previous.txt"),
            ])
            .collect()
    }

    /// The last lines of what the app said on its way out: its own messages and the update log. For a failure message, when it's gone.
    pub fn last_words(&self) -> String {
        [&self.paths.server_messages, &self.root.join("update.log")]
            .iter()
            .map(|path| {
                let text = std::fs::read_to_string(path).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                let tail = lines[lines.len().saturating_sub(LAST_WORDS_LINES)..].join("\n    ");
                format!(
                    "{}:\n    {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    if tail.is_empty() { "(nothing)" } else { &tail }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A failing test's logs, copied to [`KEEP_LOGS_ENV`]'s folder, if it's set, under the test's name.
    fn keep_logs(&self) {
        let Some(keep) = std::env::var_os(KEEP_LOGS_ENV) else { return };
        let test: String = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '-' })
            .collect();
        let into = Path::new(&keep).join(format!("{test}-{}", self.root.file_name().unwrap_or_default().to_string_lossy()));
        if std::fs::create_dir_all(&into).is_err() {
            return;
        }
        for file in self.log_files() {
            if let Some(name) = file.file_name() {
                let _ = std::fs::copy(&file, into.join(name));
            }
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.keep_logs();
            evidence::at_failure(self);
        }
        let _ = std::fs::remove_file(&self.paths.socket);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A running session. Killed if it's still there at the end of the test.
pub struct Serving {
    pub child: Child,
}

impl Serving {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Ends it as a crash would.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.kill();
    }
}

/// How long a raw client waits for an answer that has no time of its own.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A raw client, so a test can do what `recto` wouldn't.
pub struct Raw {
    pub stream: std::os::unix::net::UnixStream,
    buffer: Vec<u8>,
}

impl Raw {
    pub fn connect(state: &State) -> Self {
        Self::try_connect(state).unwrap()
    }

    /// A connection that can fail, as it does when a session out of descriptors drops it: on macOS setting a timeout on a socket whose other end has gone is `EINVAL`.
    pub fn try_connect(state: &State) -> std::io::Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(state.socket())?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        Ok(Self { stream, buffer: Vec::new() })
    }

    pub fn send(&mut self, line: &str) {
        self.try_send(line).unwrap();
    }

    /// Sends, and says if the connection had already ended. A connection the session dropped as it arrived fails here, as `EPIPE` or `ENOTCONN` depending on when the drop lands, or at the read after.
    pub fn try_send(&mut self, line: &str) -> std::io::Result<()> {
        std::io::Write::write_all(&mut self.stream, format!("{line}\n").as_bytes())
    }

    /// The next line, or `None` when the server has closed the connection. A client running out of patience isn't that, and says so.
    pub fn read(&mut self) -> Option<Value> {
        loop {
            if let Some(at) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=at).collect();
                return Some(serde_json::from_slice(&line).unwrap());
            }
            let mut chunk = [0u8; 8192];
            match std::io::Read::read(&mut self.stream, &mut chunk) {
                Ok(0) => return None,
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => panic!(
                    "no answer within {:?}: this client gave up, and the server hasn't closed the connection",
                    self.stream.read_timeout().ok().flatten()
                ),
                Err(_) => return None,
            }
        }
    }

    /// Gives the session as long as the command says it may take, on top of the usual, as `recto` does.
    pub fn call(&mut self, command: &Value) -> Value {
        let allowed = Duration::from_millis(command["timeout_ms"].as_u64().unwrap_or(0));
        // macOS refuses these once the session has closed, which `stop` does as soon as it's answered. Sending and reading are what say so.
        let _ = self.stream.set_read_timeout(Some(READ_TIMEOUT + allowed));
        self.send(&command.to_string());
        let answer = self.read().expect("the server closed the connection");
        let _ = self.stream.set_read_timeout(Some(READ_TIMEOUT));
        answer
    }
}

/// How to answer the word a command asks for at a terminal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Word {
    Right,
    Wrong,
}

/// Runs `recto` the way a person does: `script` gives it a real pseudo-terminal with a controlling terminal, and the word is read from the prompt and typed back through it. Returns the exit code and everything shown.
pub fn in_a_terminal(state: &State, args: &[&str], home: Option<&Path>, word: Word) -> (i32, String) {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut script = Command::new("/usr/bin/script");
    script
        .args(["-q", "/dev/null", bin()])
        .args(args)
        .env(STATE_ENV, &state.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(home) = home {
        script.env("HOME", home);
    }
    let mut child = script.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut seen = String::new();
    // The prompt ends with no line end, at "press return: ", so this reads what's there a byte at a time.
    let mut byte = [0u8];
    while !seen.contains("press return: ") {
        if stdout.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        seen.push(char::from(byte[0]));
    }
    let shown = seen.rsplit("type ").next().unwrap_or("").split(' ').next().unwrap_or("").to_string();
    let answer = if word == Word::Wrong { "wrongword".to_string() } else { shown };
    let _ = writeln!(stdin, "{answer}");
    let mut rest = String::new();
    for line in stdout.by_ref().lines() {
        let Ok(line) = line else { break };
        rest.push_str(&line);
        rest.push('\n');
    }
    drop(stdin);
    let status = child.wait().unwrap();
    (status.code().unwrap_or(-1), seen + &rest)
}

/// The JSON answer at the end of what a terminal showed.
pub fn last_json(shown: &str) -> Value {
    let line = shown
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON in {shown:?}"));
    serde_json::from_str(line.trim()).unwrap_or_else(|_| panic!("not JSON: {line:?}"))
}

pub fn open_cmd(path: &Path) -> Value {
    json!({ "cmd": "open", "path": path })
}

/// A file that takes a password: the standard handler's check values match no empty password.
pub fn password_protected() -> Vec<u8> {
    let bytes = recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["Secret words"]))]);
    let key = "AB".repeat(32);
    String::from_utf8_lossy(&bytes)
        .replace(
            "/Root 1 0 R",
            &format!("/Root 1 0 R /Encrypt << /Filter /Standard /V 1 /R 2 /O <{key}> /U <{key}> /P -4 >> /ID [<00> <00>]"),
        )
        .into_bytes()
}

/// The chapter title in [`locked_with`]'s contents, which only its password can read.
#[allow(dead_code)]
pub const LOCKED_CHAPTER: &str = "Secret chapter";

/// A real protected file: it opens only with the user password given, written by lopdf's eager writer, which alone encrypts. Each page says "Locked page N", and its contents have one chapter, [`LOCKED_CHAPTER`], so a test can tell whether anything decrypted got out.
#[allow(clippy::disallowed_methods, dead_code)] // A test fixture built with lopdf's eager writer, which alone encrypts; never over a user's file. Not every test binary uses it.
pub fn locked_with(password: &str, pages: usize) -> Vec<u8> {
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    use lopdf::{Object, dictionary};
    let plain: Vec<recto_testpdf::Page> = (0..pages)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Locked page {}", page + 1)])))
        .collect();
    let mut document = lopdf::Document::load_mem(&recto_testpdf::pages(&plain)).unwrap();
    let first_page = *document.get_pages().values().next().unwrap();
    let outlines = document.new_object_id();
    let chapter = document
        .add_object(dictionary! { "Title" => Object::string_literal(LOCKED_CHAPTER), "Parent" => outlines, "Dest" => vec![first_page.into(), "Fit".into()] });
    document.objects.insert(
        outlines,
        Object::Dictionary(dictionary! { "Type" => "Outlines", "First" => chapter, "Last" => chapter, "Count" => 1 }),
    );
    let root = document.trailer.get(b"Root").unwrap().as_reference().unwrap();
    document.get_object_mut(root).unwrap().as_dict_mut().unwrap().set("Outlines", outlines);
    document.trailer.set(
        "ID",
        lopdf::Object::Array(vec![
            lopdf::Object::string_literal("0123456789abcdef"),
            lopdf::Object::string_literal("0123456789abcdef"),
        ]),
    );
    let encryption = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner",
        user_password: password,
        key_length: 128,
        permissions: Permissions::all(),
    })
    .unwrap();
    document.encrypt(&encryption).unwrap();
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

/// What every app this test process started said, the run before's kept too, from each state folder of this process's still there: for a line a passing test prints when it let something through it would otherwise have failed on.
pub fn messages_so_far() -> String {
    let temp = std::env::temp_dir().canonicalize().unwrap();
    let mine = format!("rc{}-", std::process::id());
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&temp)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    folders.retain(|folder| folder.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&mine)));
    folders.sort();
    let mut said = Vec::new();
    for folder in folders {
        for name in ["server-messages.previous.txt", "server-messages.txt"] {
            if let Ok(text) = std::fs::read_to_string(folder.join(name))
                && !text.trim().is_empty()
            {
                said.push(format!("{}/{name}:\n{}", folder.display(), text.trim_end()));
            }
        }
    }
    if said.is_empty() {
        "(the apps said nothing)".to_string()
    } else {
        said.join("\n")
    }
}
