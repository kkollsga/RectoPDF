//! **An agent watching the app over its socket** ([`HeadlessApp::agent_watches`]): the server the app runs, bound on the test's disk and run on the app's
//! own host as `start_serving` runs it, with the app's announcer pointed at it, and a client that sent `watch` reading every line it's told. What this
//! reads is what reaches an agent, by every way an event goes: the session thread's announcements, the Finder's and the helpers' news, and commands' events.

use super::headless::HeadlessApp;
use recto_agent::{Event, Level};
use recto_control::server::{Announcer, Limits, Server, Stopper};
use recto_control::state::StatePaths;
use recto_platform::local::Stream;
use serde_json::Value;
use std::io::{Read, Write};
use std::time::Duration;

/// How long a read of the watcher's stream waits before the test fails: what it's waiting for was announced already.
const READ_LIMIT: Duration = Duration::from_secs(20);

/// The mark [`Watching::heard`] reads up to: published through the server's own channel after everything the test did, so every line before it is what
/// the test's steps were heard as. Its word is one no level has.
const MARK: &str = "test-mark";

/// An agent's `watch` on every group, and the server it's on. Dropped, the server stops and its thread is joined.
pub struct Watching {
    stream: Stream,
    buffer: Vec<u8>,
    paths: StatePaths,
    marker: Announcer,
    stopper: Stopper,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

impl HeadlessApp {
    /// **An agent at `level` watching every group of events,** over the app's socket, from now on. The kit's app runs no socket of its own, so this binds one
    /// on the test's disk and runs the server on the app's host, and the app's announcer goes to it, as `start_serving` does in the app.
    ///
    /// # Panics
    ///
    /// If the socket can't be bound, or the watch isn't begun.
    #[must_use]
    pub fn agent_watches(&self, level: Level) -> Watching {
        let paths = self.disk().paths().clone();
        recto_control::levels::store(&paths, level).expect("the level written");
        let server = Server::bind(&paths).expect("the socket bound");
        let (stopper, marker) = (server.stopper().expect("a stopper"), server.announcer().expect("a marker's announcer"));
        *crate::lock(&self.core().serving.announcer) = Some(server.announcer().expect("the app's announcer"));
        let host = std::sync::Arc::clone(&self.core().host);
        let thread = std::thread::Builder::new()
            .name("test-recto-control".into())
            .spawn(move || server.run(host, Limits::default()))
            .expect("the server's thread");
        let stream = Stream::connect(&paths.socket).expect("connected");
        stream.set_read_timeout(Some(READ_LIMIT)).expect("a read limit");
        let mut watching = Watching {
            stream,
            buffer: Vec::new(),
            paths,
            marker,
            stopper,
            thread: Some(thread),
        };
        (&watching.stream)
            .write_all(b"{\"cmd\":\"watch\",\"timeout_ms\":600000}\n")
            .expect("watch sent");
        let begun = watching.line();
        assert_eq!(begun["data"]["watching"], true, "the watch began: {begun}");
        watching
    }
}

impl Watching {
    /// **Every event the agent was told since it last asked,** each its line's `data`, read up to a mark published after the app settled. Settle first:
    /// what's still under way may be told after the mark.
    ///
    /// # Panics
    ///
    /// If the stream ends or nothing comes for [`READ_LIMIT`].
    pub fn heard(&mut self) -> Vec<Value> {
        self.marker.announce(vec![Event::LevelChanged { level: MARK, was: MARK }]);
        let mut heard = Vec::new();
        loop {
            let line = self.line();
            let data = line["data"].clone();
            if data["event"] == "level_changed" && data["level"] == MARK {
                return heard;
            }
            heard.push(data);
        }
    }

    /// **The level moved to `level`,** as `recto permission set` moves it, and the server told: its next command reads the file. The watch goes on, and is
    /// told `level_changed`.
    ///
    /// # Panics
    ///
    /// If the level can't be written or the server doesn't answer.
    pub fn set_level(&self, level: Level) {
        recto_control::levels::store(&self.paths, level).expect("the level written");
        let mut client = recto_control::client::Client::connect(&self.paths).expect("a client");
        let answer = client.call("{\"cmd\":\"status\"}", READ_LIMIT).expect("status answered");
        assert_eq!(answer["data"]["level"], level.name(), "{answer}");
    }

    fn line(&mut self) -> Value {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                return serde_json::from_slice(&line[..end]).expect("a line of JSON");
            }
            let mut chunk = [0u8; 4096];
            let read = (&self.stream).read(&mut chunk).expect("the watcher's stream read within its limit");
            assert!(read > 0, "the watcher's stream ended");
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.stopper.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
