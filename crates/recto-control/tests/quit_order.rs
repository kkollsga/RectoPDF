//! What a client is told when the session ends under it. Its own binary: the loop it runs 200 times would read as spinning in `server.rs`'s tests, which measure the process's CPU.

use recto_agent::{AgentError, Command, Condition, ConditionState, Event, Level};
use recto_control::StatePaths;
use recto_control::client::Client;
use recto_control::host::Host;
use recto_control::levels;
use recto_control::server::{Limits, Server, Stopper};
use recto_session::TabId;
use serde_json::{Value, json};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// `tabs` never answers until released; the rest is nothing.
#[derive(Default)]
struct Stand {
    release: Mutex<bool>,
    released: Condvar,
    ran: Mutex<Vec<&'static str>>,
}

impl Host for Stand {
    fn run(&self, _actor: recto_agent::Actor, command: &Command, _events: &mut Vec<Event>) -> Result<Value, AgentError> {
        self.ran.lock().unwrap().push(command.tag());
        let mut released = self.release.lock().unwrap();
        while !*released && matches!(command, Command::Read(recto_agent::commands::Read::Tabs(_))) {
            released = self.released.wait(released).unwrap();
        }
        Ok(json!({ "ran": command.tag() }))
    }

    fn check(&self, _condition: Condition, _tab: &recto_agent::TabRef, _page: Option<u32>) -> ConditionState {
        ConditionState::Waiting
    }

    fn tab_count(&self) -> u32 {
        0
    }

    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        Vec::new()
    }

    fn for_watchers(&self, _events: &mut Vec<Event>) {}
}

struct Running {
    paths: StatePaths,
    host: Arc<Stand>,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stopper: Stopper,
    _root: recto_testpdf::TempDir,
}

impl Running {
    fn start(limits: Limits) -> Self {
        let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "qs");
        let paths = StatePaths::overridden(&root).unwrap();
        paths.prepare().unwrap();
        levels::store(&paths, Level::Full).unwrap();
        let server = Server::bind(&paths).unwrap();
        let stopper = server.stopper().unwrap();
        let host = Arc::new(Stand::default());
        let hosted = Arc::clone(&host);
        let thread = std::thread::spawn(move || server.run(hosted, limits));
        Self {
            paths,
            host,
            thread: Some(thread),
            stopper,
            _root: root,
        }
    }

    fn client(&self) -> Client {
        Client::connect(&self.paths).unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        *self.host.release.lock().unwrap() = true;
        self.host.released.notify_all();
        self.stopper.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Failure: a session that is ending closes a client's connection while its socket's name is still there, so the client's look at the socket finds it taking connections and
/// says the session is "still running" while it ends (seen on a quit: `connection_dropped`). The name goes before any connection closes, so a client left with no answer, when
/// the session is stopped by a host's stopper or by `stop` over the socket, always hears that the session ended. Repeated, as it was a race.
#[test]
fn a_client_left_without_an_answer_by_a_stop_hears_that_the_session_ended() {
    let limits = || Limits {
        stop_grace: Duration::from_millis(20),
        ..Limits::default()
    };
    for round in 0..200 {
        let running = Running::start(limits());
        // This client's connection is open and unanswered when the session ends. On the host's stop it's waiting on a command that never answers; on `stop` it's idle, as `stop` itself waits for running commands.
        let mut pending = running.client();
        if round % 2 == 0 {
            pending.send("{\"cmd\":\"tabs\"}", Duration::from_secs(20)).unwrap();
            let started = Instant::now();
            while !running.host.ran.lock().unwrap().contains(&"tabs") {
                assert!(started.elapsed() < Duration::from_secs(20), "the command never reached the host");
                std::thread::sleep(Duration::from_micros(200));
            }
            running.stopper.stop();
        } else {
            let mut stopping = running.client();
            let _ = stopping.call("{\"cmd\":\"stop\"}", Duration::from_secs(20));
        }
        let error = pending.read(Duration::from_secs(20)).unwrap_err();
        assert_eq!(error.code, "session_ended", "round {round}: {}", error.message);
    }
}
