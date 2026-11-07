//! Runs the real `recto` on temp state, the way an agent does. Every server these start ends with its [`Session`], and nothing here can reach the real state folder, because the override refuses it.
#![allow(dead_code)]

use recto_agent::Level;
use recto_control::state::STATE_ENV;
use recto_control::{StatePaths, levels};
use serde_json::{Value, json};
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

impl State {
    pub fn new() -> Self {
        // Short, so a socket fits in the folder even when the temp folder's path is long.
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!("rc{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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

    pub fn command(&self) -> Command {
        let mut command = Command::new(bin());
        command.env(STATE_ENV, &self.root).stdin(Stdio::null());
        command
    }

    /// Runs `recto` and reads its one JSON answer.
    pub fn json(&self, args: &[&str]) -> (Value, i32) {
        let output = self.command().args(args).arg("--json").stderr(Stdio::inherit()).output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let value = serde_json::from_str(text.trim()).unwrap_or_else(|_| panic!("not JSON from {args:?}: {text:?}"));
        (value, output.status.code().unwrap())
    }

    /// Runs `recto` and returns what a person would see.
    pub fn text(&self, args: &[&str]) -> (String, String, i32) {
        let output = self.command().args(args).output().unwrap();
        (String::from_utf8(output.stdout).unwrap(), String::from_utf8(output.stderr).unwrap(), output.status.code().unwrap())
    }

    /// Starts a session and waits for its ready line.
    pub fn serve(&self) -> Serving {
        let mut child = self.command().args(["serve", "--json"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
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

    pub fn log_text(&self) -> String {
        let mut text = std::fs::read_to_string(&self.paths.log).unwrap_or_default();
        text.push_str(&std::fs::read_to_string(&self.paths.old_log).unwrap_or_default());
        text
    }
}

impl Drop for State {
    fn drop(&mut self) {
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

/// A raw client, so a test can do what `recto` wouldn't.
pub struct Raw {
    pub stream: std::os::unix::net::UnixStream,
    buffer: Vec<u8>,
}

impl Raw {
    pub fn connect(state: &State) -> Self {
        let stream = std::os::unix::net::UnixStream::connect(state.socket()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        Self { stream, buffer: Vec::new() }
    }

    pub fn send(&mut self, line: &str) {
        std::io::Write::write_all(&mut self.stream, format!("{line}\n").as_bytes()).unwrap();
    }

    /// The next line, or `None` when the server has closed the connection.
    pub fn read(&mut self) -> Option<Value> {
        loop {
            if let Some(at) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=at).collect();
                return Some(serde_json::from_slice(&line).unwrap());
            }
            let mut chunk = [0u8; 8192];
            match std::io::Read::read(&mut self.stream, &mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
            }
        }
    }

    pub fn call(&mut self, command: &Value) -> Value {
        self.send(&command.to_string());
        self.read().expect("the server closed the connection")
    }
}

pub fn open_cmd(path: &Path) -> Value {
    json!({ "cmd": "open", "path": path })
}
