//! The server with a stand-in host and short limits, on a real socket: what a real session is too well-behaved, or too slow, to show.

use recto_agent::{AgentError, Command, Condition, ConditionState, Event, Level, exit};
use recto_control::StatePaths;
use recto_control::client::Client;
use recto_control::host::Host;
use recto_control::levels;
use recto_control::server::{BindError, Limits, Server};
use recto_platform::local::process_counters;
use recto_session::TabId;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A host that does whatever a test's command asks. `facts` panics, `tabs` blocks until released, `text` is slow, `where` answers with something huge.
#[derive(Default)]
struct Stand {
    release: Mutex<bool>,
    released: Condvar,
    running_draws: AtomicUsize,
    most_draws: AtomicUsize,
    unsaved: Mutex<Vec<(TabId, String)>>,
    wheres: AtomicUsize,
    looks_answer: std::sync::atomic::AtomicBool,
    stop_refused: std::sync::atomic::AtomicBool,
    told_off: AtomicUsize,
}

impl Stand {
    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.released.notify_all();
    }
}

impl Host for Stand {
    fn run(&self, _level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        match command {
            Command::Read(recto_agent::commands::Read::Facts(recto_agent::commands::reading::Facts { .. })) => panic!("a bug in a command"),
            Command::Read(recto_agent::commands::Read::Tabs(recto_agent::commands::reading::Tabs { .. })) => {
                let mut released = self.release.lock().unwrap();
                while !*released {
                    released = self.released.wait(released).unwrap();
                }
                Ok(json!({ "released": true }))
            }
            Command::Read(recto_agent::commands::Read::Text(recto_agent::commands::text::Text { .. })) => {
                let now = self.running_draws.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_draws.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                self.running_draws.fetch_sub(1, Ordering::SeqCst);
                Ok(json!({ "read": true }))
            }
            Command::Read(recto_agent::commands::Read::Where(recto_agent::commands::reading::Where { .. })) => {
                self.wheres.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "huge": "x".repeat(5_000_000) }))
            }
            Command::Move(recto_agent::commands::Move::Goto(recto_agent::commands::linking::Goto { page, .. })) => {
                events.push(Event::PageChanged {
                    tab: recto_session::TabId(1),
                    page: *page,
                });
                Ok(json!({ "page": page }))
            }
            Command::Move(recto_agent::commands::Move::Close(recto_agent::commands::tabs::Close { .. })) => {
                events.push(Event::TabClosed { tab: recto_session::TabId(1) });
                Ok(json!({ "closed": true }))
            }
            _ => Ok(json!({ "ran": command.tag() })),
        }
    }

    fn check(&self, _condition: Condition, tab: &str, _page: Option<u32>) -> ConditionState {
        if tab == "panics" {
            panic!("a bug in a look");
        }
        if tab == "holds" {
            let mut released = self.release.lock().unwrap();
            while !*released {
                released = self.released.wait(released).unwrap();
            }
        }
        if self.looks_answer.load(Ordering::SeqCst) && tab == "secret" {
            return ConditionState::Met(json!({ "secret": true }));
        }
        ConditionState::Waiting
    }

    fn tab_count(&self) -> u32 {
        7
    }

    fn access_is_off(&self) {
        self.told_off.fetch_add(1, Ordering::SeqCst);
    }

    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        self.unsaved.lock().unwrap().clone()
    }

    fn stop_refusal(&self) -> Option<AgentError> {
        self.stop_refused
            .load(Ordering::SeqCst)
            .then(|| AgentError::new("only_the_user_quits", "Not over the socket.", exit::FAILED))
    }
}

struct Running {
    paths: StatePaths,
    host: Arc<Stand>,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
    stopper: recto_control::server::Stopper,
    _root: recto_testpdf::TempDir,
}

impl Running {
    fn start(limits: Limits) -> Self {
        let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "rs");
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

    fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(&self.paths.socket).unwrap();
        // On macOS, setting a timeout on a socket whose other end has already closed is `EINVAL`. A server turning a peer away closes at
        // once, so that's the answer being tested, not a failure: the read after sees the close without waiting.
        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(20))) {
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
        }
        stream
    }

    fn client(&self) -> Client {
        Client::connect(&self.paths).unwrap()
    }

    fn call(&self, line: &Value) -> Value {
        self.client().call(&line.to_string(), Duration::from_secs(20)).unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.host.release();
        self.stopper.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_line(stream: &mut UnixStream) -> Option<Value> {
    let mut bytes = Vec::new();
    let mut byte = [0u8];
    loop {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) if byte[0] == b'\n' => return Some(serde_json::from_slice(&bytes).unwrap()),
            Ok(_) => bytes.push(byte[0]),
        }
    }
}

/// Failure: a client of another user gets in, because the socket's mode was the only check. Set up as "expects someone else": every connection must be dropped before it can say anything.
#[test]
fn a_peer_of_another_user_is_turned_away() {
    let limits = Limits {
        expected_uid: recto_platform::local::current_uid() + 1,
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut stream = running.connect();
    let _ = stream.write_all(b"{\"cmd\":\"status\"}\n");
    assert_eq!(read_line(&mut stream), None, "it answered a stranger");
    let log = std::fs::read_to_string(&running.paths.other_log).unwrap_or_default();
    assert!(log.contains("peer_refused"), "{log}");
}

/// Failure: a bug in one command takes the session down or leaves its client waiting. It must be one client's error, and the session carries on.
#[test]
fn a_panic_is_one_clients_error() {
    let running = Running::start(Limits::default());
    let reply = running.call(&json!({ "cmd": "facts" }));
    assert_eq!(reply["error"]["code"], "internal", "{reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("still running"));
    assert_eq!(running.call(&json!({ "cmd": "budget" }))["ok"], true, "it must still answer");
    let mut stream = running.connect();
    stream
        .write_all(b"{\"cmd\":\"wait\",\"condition\":\"tab_open\",\"tab\":\"panics\",\"timeout_ms\":5000}\n")
        .unwrap();
    assert_eq!(read_line(&mut stream).unwrap()["error"]["code"], "internal");
    let log = std::fs::read_to_string(&running.paths.other_log).unwrap();
    assert!(log.contains("\"outcome\":\"internal\""), "{log}");
}

/// Failure: one slow command stalls everyone. `status` must answer while another client's command is stuck, and the stuck one still gets its answer when it's done.
#[test]
fn a_stuck_command_doesnt_stop_status() {
    let running = Running::start(Limits::default());
    let mut stuck = running.connect();
    stuck.write_all(b"{\"cmd\":\"tabs\"}\n").unwrap();
    let started = Instant::now();
    for _ in 0..5 {
        let status = running.call(&json!({ "cmd": "status" }));
        assert_eq!(status["data"]["tabs"], 7, "{status}");
    }
    assert!(started.elapsed() < Duration::from_secs(10));
    running.host.release();
    assert_eq!(read_line(&mut stuck).unwrap()["data"]["released"], true);
}

/// Failure: page drawing runs without limit, each worth hundreds of megabytes, or one client's drawing starves another's plain command. At most the cap draw at once, and the rest still run.
#[test]
fn drawing_is_capped_and_other_work_goes_around_it() {
    let limits = Limits {
        max_draw_jobs: 2,
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut streams: Vec<UnixStream> = (0..6).map(|_| running.connect()).collect();
    for stream in &mut streams {
        stream.write_all(b"{\"cmd\":\"text\",\"page\":1}\n").unwrap();
    }
    assert_eq!(running.call(&json!({ "cmd": "budget" }))["ok"], true, "plain work must go past queued drawing");
    for stream in &mut streams {
        assert_eq!(read_line(stream).unwrap()["data"]["read"], true);
    }
    assert!(
        running.host.most_draws.load(Ordering::SeqCst) <= 2,
        "{} drew at once",
        running.host.most_draws.load(Ordering::SeqCst)
    );
}

/// Failure: half a line holds a slot for ever, or a silent connection never goes. Each has its own deadline, and neither needs anything to happen.
#[test]
fn half_a_line_and_silence_each_run_out() {
    let limits = Limits {
        line_deadline: Duration::from_millis(100),
        idle_deadline: Duration::from_millis(200),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut half = running.connect();
    half.write_all(b"{\"cmd\":").unwrap();
    let mut silent = running.connect();
    let started = Instant::now();
    assert_eq!(read_line(&mut half), None, "half a line was kept");
    assert_eq!(read_line(&mut silent), None, "a silent connection was kept");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

/// Failure: a command's huge answer is sent regardless, or a client that doesn't read costs the server without limit. An answer past the cap is replaced by one that says so.
#[test]
fn an_answer_past_the_cap_says_so() {
    let limits = Limits {
        max_reply_bytes: 1024 * 1024,
        ..Limits::default()
    };
    let running = Running::start(limits);
    let reply = running.call(&json!({ "cmd": "where" }));
    assert_eq!(reply["error"]["code"], "reply_too_large", "{}", reply.to_string().len());
    assert!(reply["error"]["next"].as_str().unwrap().contains("less"));
}

/// Failure: a client that asks for a huge answer and stops reading holds that memory for ever. It's dropped at the write deadline, and the room comes back. The deadline is waited out once the answer has started, not from a fixed moment.
#[test]
fn a_client_that_stops_reading_is_dropped() {
    let limits = Limits {
        write_deadline: Duration::from_millis(100),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut lazy = running.connect();
    lazy.write_all(b"{\"cmd\":\"where\"}\n").unwrap();
    // The answer has started, so the deadline runs from here: from a fixed moment, a loaded Mac's answer started after it, the client then read it
    // all as it came, and the read waited out its 20 s timeout on an open connection (it failed `tools/check.sh --full` on 17ad148).
    let mut first = [0u8; 1];
    assert_eq!(lazy.read(&mut first).expect("the answer starts"), 1);
    // Give the deadline time to pass while it reads nothing more.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    let mut total = 1;
    let mut chunk = [0u8; 65536];
    while let Ok(count) = lazy.read(&mut chunk) {
        if count == 0 {
            break;
        }
        total += count;
    }
    assert!(total < 5_000_000, "it should have been cut off before the end: {total}");
}

/// Failure: a client's next command, sent while the last one runs, is lost or times out. One after another on one connection must work, in order, and blank lines don't count.
#[test]
fn commands_on_one_connection_run_in_order() {
    let limits = Limits {
        line_deadline: Duration::from_millis(50),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut stream = running.connect();
    stream
        .write_all(b"{\"cmd\":\"text\",\"page\":1}\n\n\r\n{\"cmd\":\"goto\",\"page\":2}\r\n{\"cmd\":\"goto\",\"page\":3}\n")
        .unwrap();
    assert_eq!(read_line(&mut stream).unwrap()["data"]["read"], true, "the slow one goes first");
    assert_eq!(read_line(&mut stream).unwrap()["data"]["page"], 2);
    assert_eq!(read_line(&mut stream).unwrap()["data"]["page"], 3);
}

/// Failure: a client that closes its sending side after a command, as `nc` does, loses its answer because it looks like a dead client.
#[test]
fn a_half_closed_client_still_gets_its_answer() {
    let running = Running::start(Limits::default());
    let mut stream = running.connect();
    stream.write_all(b"{\"cmd\":\"text\",\"page\":1}\n").unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    assert_eq!(read_line(&mut stream).unwrap()["data"]["read"], true);
    assert_eq!(read_line(&mut stream), None, "and then it's over");
}

/// Failure: a host that won't be stopped over the socket is, or a refused `stop` leaves the server saying it's stopping. It answers why, and everything goes on.
#[test]
fn a_host_can_refuse_to_be_stopped() {
    let running = Running::start(Limits::default());
    running.host.stop_refused.store(true, Ordering::SeqCst);
    let refused = running.call(&json!({ "cmd": "stop" }));
    assert_eq!(refused["error"]["code"], "only_the_user_quits", "{refused}");
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    assert_eq!(
        running.call(&json!({ "cmd": "goto", "page": 1 }))["ok"],
        true,
        "commands aren't refused as if it were stopping"
    );
}

/// Failure: `stop` drops unsaved edits, or a host can't end the server it's carrying. A host's stopper and `stop` must both end `run`, and both remove the socket.
#[test]
fn stop_keeps_unsaved_edits_and_a_host_can_end_it() {
    let running = Running::start(Limits::default());
    running.host.unsaved.lock().unwrap().push((TabId(1), "report".to_string()));
    let refused = running.call(&json!({ "cmd": "stop" }));
    assert_eq!(refused["error"]["code"], "unsaved_changes", "{refused}");
    assert_eq!(refused["error"]["candidates"][0], "#1");
    assert_eq!(
        refused["error"]["untrusted"]["candidates"][0]["name"], "report",
        "a file's name goes only under `untrusted`"
    );
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    running.host.unsaved.lock().unwrap().clear();
    let mut watcher = running.connect();
    watcher.write_all(b"{\"cmd\":\"watch\"}\n").unwrap();
    assert_eq!(read_line(&mut watcher).unwrap()["data"]["watching"], true);
    assert_eq!(running.call(&json!({ "cmd": "stop" }))["data"]["stopping"], true);
    let ended = read_line(&mut watcher).unwrap();
    assert_eq!(
        (&ended["data"]["done"], &ended["data"]["reason"]),
        (&json!(true), &json!("session stopping")),
        "{ended}"
    );

    let hosted = Running::start(Limits::default());
    hosted.stopper.stop();
    let started = Instant::now();
    while hosted.paths.socket.exists() {
        assert!(started.elapsed() < Duration::from_secs(10), "it didn't stop");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Failure: two servers bind one state folder, or the lock isn't let go when a server ends, so it can't start again.
#[test]
fn the_lock_is_one_server_and_comes_back_when_it_ends() {
    let running = Running::start(Limits::default());
    assert!(matches!(Server::bind(&running.paths), Err(BindError::AlreadyRunning)));
    let paths = running.paths.clone();
    let root_paths = paths.clone();
    // Stop it without removing the folder.
    running.stopper.stop();
    let started = Instant::now();
    while root_paths.socket.exists() {
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut running = running;
    if let Some(thread) = running.thread.take() {
        thread.join().unwrap().unwrap();
    }
    let again = Server::bind(&paths);
    assert!(again.is_ok(), "the lock wasn't let go");
}

/// Failure: a client talks to something that isn't this user's, or a client's answer comes back on a dead server as a hang.
#[test]
fn the_client_checks_the_folder_and_reports_a_dead_server() {
    let running = Running::start(Limits::default());
    let mut client = running.client();
    client.send("{\"cmd\":\"status\"}", Duration::from_secs(5)).unwrap();
    assert_eq!(client.read(Duration::from_secs(5)).unwrap()["ok"], true);
    client.send("{\"cmd\":\"tabs\"}", Duration::from_secs(5)).unwrap();
    let error = client.read(Duration::from_millis(50)).unwrap_err();
    assert_eq!(
        (error.code, error.exit),
        ("no_answer", exit::TIMED_OUT),
        "the stuck command means no answer in time"
    );
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&running.paths.run_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let refused = Client::connect(&running.paths).unwrap_err();
    assert_eq!(
        (refused.code, refused.exit),
        ("socket_refused", exit::PERMISSION),
        "a folder others can reach isn't to be trusted"
    );
    std::fs::set_permissions(&running.paths.run_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Failure: an event goes to a waiter who has no interest, or a wait is told about a page by a wait before it on the same connection. Only one look runs per wait, and it's the newest wait's.
#[test]
fn watchers_hear_events_and_a_finished_wait_cant_answer_the_next() {
    let running = Running::start(Limits::default());
    let mut watcher = running.connect();
    watcher
        .write_all(b"{\"cmd\":\"watch\",\"events\":[\"pages\"],\"count\":2,\"timeout_ms\":20000}\n")
        .unwrap();
    assert_eq!(read_line(&mut watcher).unwrap()["data"]["watching"], true);
    running.call(&json!({ "cmd": "goto", "page": 2 }));
    running.call(&json!({ "cmd": "goto", "page": 3 }));
    assert_eq!(read_line(&mut watcher).unwrap()["data"]["page"], 2);
    assert_eq!(read_line(&mut watcher).unwrap()["data"]["page"], 3);
    let done = read_line(&mut watcher).unwrap();
    assert_eq!(done["data"]["reason"], "count reached", "{done}");
    // The connection's free again, and a wait that runs out says so.
    watcher
        .write_all(b"{\"cmd\":\"wait\",\"condition\":\"tab_open\",\"tab\":\"x\",\"timeout_ms\":10}\n")
        .unwrap();
    assert_eq!(read_line(&mut watcher).unwrap()["error"]["code"], "timed_out");
}

/// Failure: a queued `page rotate` answers ok and is then lost, because `stop` only looked for unsaved edits once. Whatever's waiting when `stop` arrives is refused, and `stop` looks after what's running has finished: that edit stops it.
#[test]
fn stop_checks_after_whats_running_has_finished() {
    let limits = Limits {
        stop_grace: Duration::from_secs(5),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut slow = running.connect();
    // A stuck command, which on being let go leaves a tab unsaved.
    slow.write_all(b"{\"cmd\":\"tabs\"}\n").unwrap();
    let mut stopper = running.connect();
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    stopper.write_all(b"{\"cmd\":\"stop\"}\n").unwrap();
    // Let the stop arrive, then anything else that isn't quick is told it's stopping.
    std::thread::sleep(Duration::from_millis(100));
    let refused = running.call(&json!({ "cmd": "goto", "page": 2 }));
    assert_eq!(refused["error"]["code"], "busy", "{refused}");
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true, "`status` still answers");
    // The running command changes something as it ends.
    running.host.unsaved.lock().unwrap().push((TabId(1), "report".to_string()));
    running.host.release();
    assert_eq!(read_line(&mut slow).unwrap()["data"]["released"], true);
    let answer = read_line(&mut stopper).unwrap();
    assert_eq!(answer["error"]["code"], "unsaved_changes", "it must look after that command ended: {answer}");
    let working = running.call(&json!({ "cmd": "goto", "page": 3 }));
    assert_eq!(working["ok"], true, "a refused stop lets everything carry on: {working}");
}

/// Failure: `stop` with something stuck hangs for ever, or kills it anyway. It says it couldn't, and the session carries on.
#[test]
fn stop_gives_up_on_a_stuck_command_and_says_so() {
    let limits = Limits {
        stop_grace: Duration::from_millis(100),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut slow = running.connect();
    slow.write_all(b"{\"cmd\":\"tabs\"}\n").unwrap();
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    let reply = running.call(&json!({ "cmd": "stop" }));
    assert_eq!(reply["error"]["code"], "busy", "{reply}");
    assert_eq!(running.call(&json!({ "cmd": "budget" }))["ok"], true);
}

/// Failure: a client that sends commands and doesn't read makes the session work without end, and its answers fill the shared allowance, so every other client gets `reply_too_large` for up to 30 s. Past 1 MiB unread, its commands wait.
#[test]
fn a_client_that_doesnt_read_isnt_worked_for() {
    let limits = Limits {
        max_unsent_before_pause: 1024 * 1024,
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut greedy = running.connect();
    // Each answer is 5 MiB, so the first is already past the line.
    let many = "{\"cmd\":\"where\"}\n".repeat(200);
    greedy.write_all(many.as_bytes()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    assert!(
        running.host.wheres.load(Ordering::SeqCst) <= 2,
        "{} answers made for a client that read none",
        running.host.wheres.load(Ordering::SeqCst)
    );
    // When it starts reading, it gets the next ones.
    let mut reader = BufLine::default();
    assert!(reader.next(&mut greedy).unwrap()["data"]["huge"].is_string());
    assert!(reader.next(&mut greedy).unwrap()["data"]["huge"].is_string());
    assert!(running.host.wheres.load(Ordering::SeqCst) >= 2);
}

/// Failure: a client that sends two commands and reads slowly loses its connection, because the second, whole and waiting behind an answer not yet read, was timed as half a line. It failed the full run on a busy Mac, and every time with 24 clients at once. A client gets all its answers however slowly it reads, up to the write deadline.
#[test]
fn a_whole_line_waiting_behind_an_unread_answer_isnt_half_a_line() {
    let limits = Limits {
        line_deadline: Duration::from_millis(100),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut slow = running.connect();
    slow.write_all(b"{\"cmd\":\"where\"}\n{\"cmd\":\"where\"}\n").unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let mut reader = BufLine::default();
    for answer in 0..2 {
        assert!(reader.next(&mut slow).unwrap_or_else(|| panic!("answer {answer}: the connection was dropped"))["data"]["huge"].is_string());
    }
}

/// Failure: with a whole line held back behind an unread answer, the session's next wake-up is a line deadline that nothing acts on, so `poll` returns at once, for ever, and the session's one thread spins at 100%. Measured as the process's CPU time and wake-ups while nothing is happening, once the first answer is on its way: measured from a fixed moment, a loaded Mac's slower answer was counted as spinning.
#[test]
fn a_line_held_back_behind_an_unread_answer_doesnt_spin_the_session() {
    let limits = Limits {
        line_deadline: Duration::from_millis(50),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut slow = running.connect();
    slow.write_all(b"{\"cmd\":\"where\"}\n{\"cmd\":\"where\"}\n").unwrap();
    // The first answer is made and on its way, so the window measured holds none of that work: on a loaded Mac it took past a fixed 200 ms, and its
    // thread's preemptions read as 200 to 340 wake-ups. Its start is waited for (up to the read timeout) and kept for the reading below.
    let mut reader = BufLine::default();
    assert!(reader.started(&mut slow), "the first answer arrives");
    // Then past the deadline it would have spun on, with the answer unread.
    std::thread::sleep(Duration::from_millis(200));
    let before = process_counters(std::process::id()).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let after = process_counters(std::process::id()).unwrap();
    let cpu_ms = (after.cpu_ns - before.cpu_ns) / 1_000_000;
    let wakes = after.context_switches - before.context_switches;
    assert!(cpu_ms < 100, "{cpu_ms} ms of CPU in 500 ms: it's spinning");
    assert!(wakes < 200, "{wakes} wake-ups in 500 ms: it's spinning");
    assert!(reader.next(&mut slow).expect("and the client still gets its answer")["data"]["huge"].is_string());
}

/// Failure: a client that sent several commands and is slowly reading the first huge answer is cut off for sending "half a line", though its lines are whole and only waiting for it to catch up. It showed as this file's other test failing when the Mac was busy, as reading took past the 5 s.
#[test]
fn whole_lines_waiting_arent_a_stalled_line() {
    let limits = Limits {
        line_deadline: Duration::from_millis(100),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut greedy = running.connect();
    greedy
        .write_all("{\"cmd\":\"where\"}\n{\"cmd\":\"where\"}\n{\"cmd\":\"where\"}\n".as_bytes())
        .unwrap();
    // Long past the deadline, with its answer unread and two commands waiting.
    std::thread::sleep(Duration::from_millis(400));
    let mut reader = BufLine::default();
    for _ in 0..3 {
        assert!(reader.next(&mut greedy).unwrap()["data"]["huge"].is_string());
    }
    // And half a line is still caught.
    greedy.write_all(b"{\"cmd\":").unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let mut rest = [0u8; 16];
    assert_eq!(greedy.read(&mut rest).unwrap(), 0, "it's gone");
}

/// Failure: an agent in `recto wait` is dropped by a deadline that's for something else, half a line, an unread answer or a connection with nothing to do, so a wait for a slow index ends early with "the session ended" on a loaded Mac. It holds for as long as it asked for, past all of them, and hears what it was waiting for.
#[test]
fn a_wait_outlasts_every_deadline_a_connection_has() {
    let short = Duration::from_millis(50);
    let limits = Limits {
        line_deadline: short,
        idle_deadline: short,
        write_deadline: short,
        overflow_idle: short,
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut hears = running.connect();
    hears
        .write_all(b"{\"cmd\":\"wait\",\"condition\":\"tab_open\",\"tab\":\"secret\",\"timeout_ms\":20000}\n")
        .unwrap();
    let mut runs_out = running.connect();
    runs_out
        .write_all(b"{\"cmd\":\"wait\",\"condition\":\"tab_open\",\"tab\":\"never\",\"timeout_ms\":500}\n")
        .unwrap();
    // It's told it ran out of time, at ten times any deadline, on its own connection.
    let ran_out = read_line(&mut runs_out).expect("a wait was dropped before its time was up");
    assert_eq!(ran_out["error"]["code"], "timed_out", "{ran_out}");
    running.host.looks_answer.store(true, Ordering::SeqCst);
    let mut closer = running.connect();
    closer.write_all(b"{\"cmd\":\"close\"}\n").unwrap();
    assert_eq!(read_line(&mut closer).unwrap()["ok"], true);
    let heard = read_line(&mut hears).expect("a wait was dropped while what it waited for was still to come");
    assert_eq!(heard["data"]["secret"], true, "{heard}");
}

/// Failure: a client with commands waiting while it catches up on a large answer is dropped, as though it had stalled halfway through a line, if it takes longer than half a line is given. Those commands are whole, and waiting is the session's doing. Found by `a_client_that_doesnt_read_isnt_worked_for` failing on a Mac under load.
#[test]
fn a_client_catching_up_isnt_dropped_for_the_commands_it_has_waiting() {
    let limits = Limits {
        max_unsent_before_pause: 1024 * 1024,
        line_deadline: Duration::from_millis(100),
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut greedy = running.connect();
    greedy.write_all("{\"cmd\":\"where\"}\n".repeat(20).as_bytes()).unwrap();
    // Well past the time half a line gets.
    std::thread::sleep(Duration::from_millis(800));
    let mut reader = BufLine::default();
    assert!(reader.next(&mut greedy).expect("it was dropped while it had commands waiting")["data"]["huge"].is_string());
    assert!(reader.next(&mut greedy).expect("it was dropped after its first answer")["data"]["huge"].is_string());
}

/// Failure: a `wait` in flight when access goes to Off asks for an index as it finishes, after the session has already stopped indexing for Off, so a build runs at Off. Each thing that can have asked for one, finishing after the change, repeats the stop.
#[test]
fn a_look_finishing_after_access_goes_off_stops_indexing_again() {
    let running = Running::start(Limits::default());
    let mut waiting = running.connect();
    waiting
        .write_all(b"{\"cmd\":\"wait\",\"condition\":\"indexing_done\",\"tab\":\"holds\",\"timeout_ms\":60000}\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    levels::store(&running.paths, Level::Off).unwrap();
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    assert_eq!(running.host.told_off.load(Ordering::SeqCst), 1, "going to Off stops it");
    running.host.release();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while running.host.told_off.load(Ordering::SeqCst) < 2 {
        assert!(std::time::Instant::now() < deadline, "the look ended at Off with no second stop");
        assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Default)]
struct BufLine {
    bytes: Vec<u8>,
    /// How much of `bytes` is known to hold no line end. Looking from the start after each read is what made a 5 MB answer take seconds in a debug build.
    searched: usize,
}

impl BufLine {
    /// Waits for an answer to start, within the stream's own timeout, and keeps what came for the line. A few bytes out of a 5 MB answer change nothing about the cap.
    fn started(&mut self, stream: &mut UnixStream) -> bool {
        let mut first = [0u8; 1];
        match stream.read(&mut first) {
            Ok(1) => {
                self.bytes.push(first[0]);
                true
            }
            _ => false,
        }
    }

    fn next(&mut self, stream: &mut UnixStream) -> Option<Value> {
        let mut chunk = vec![0u8; 256 * 1024];
        loop {
            if let Some(at) = self.bytes[self.searched..].iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.bytes.drain(..=self.searched + at).collect();
                self.searched = 0;
                return Some(serde_json::from_slice(&line).unwrap());
            }
            self.searched = self.bytes.len();
            let count = stream.read(&mut chunk).ok()?;
            if count == 0 {
                return None;
            }
            self.bytes.extend_from_slice(&chunk[..count]);
        }
    }
}

/// Failure: the shared allowance runs out, and small answers stop too, or the cap isn't one at all. Large answers are refused while `status` still answers, and nothing goes past the hard cap.
///
/// It waits until both answers have started arriving, which is after the cap has decided about each, before reading either. It used to sleep 300 ms and assume that: under load the second was still being made when the first was read, so the first drained, the second fitted, and neither was refused (seen on a full run, 2026-11-08).
#[test]
fn the_cap_on_unread_answers_is_hard_and_small_ones_still_get_through() {
    let limits = Limits {
        max_queued_bytes: 6 * 1024 * 1024,
        max_queued_hard_bytes: 7 * 1024 * 1024,
        ..Limits::default()
    };
    let running = Running::start(limits);
    let mut one = running.connect();
    one.write_all(b"{\"cmd\":\"where\"}\n").unwrap();
    let mut two = running.connect();
    two.write_all(b"{\"cmd\":\"where\"}\n").unwrap();
    // One of them gets the room, and the other is told there's none. Neither reads its answer, so this holds once both are decided, which is when each starts arriving.
    let mut reader_one = BufLine::default();
    let mut reader_two = BufLine::default();
    assert!(reader_one.started(&mut one), "the first answer never started");
    assert!(reader_two.started(&mut two), "the second answer never started");
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true, "a small answer goes past the soft cap");
    let (first, second) = (reader_one.next(&mut one).unwrap(), reader_two.next(&mut two).unwrap());
    let codes = [first["error"]["code"].as_str(), second["error"]["code"].as_str()];
    assert!(codes.contains(&Some("reply_too_large")), "{codes:?}");
    assert!(first["ok"] == true || second["ok"] == true, "one must get its answer");
}

/// Failure: a wait's look runs after the level dropped and hands over a tab's details, because only the asking was checked. Here the look waits behind a stuck job, and the level drops with no message to the server, so only the look itself can notice.
#[test]
fn a_look_rechecks_the_level() {
    let limits = Limits {
        max_jobs: 1,
        ..Limits::default()
    };
    let running = Running::start(limits);
    running.host.looks_answer.store(true, Ordering::SeqCst);
    let mut slow = running.connect();
    slow.write_all(b"{\"cmd\":\"tabs\"}\n").unwrap();
    let mut waiter = running.connect();
    assert_eq!(running.call(&json!({ "cmd": "status" }))["ok"], true);
    waiter
        .write_all(b"{\"cmd\":\"wait\",\"condition\":\"tab_open\",\"tab\":\"secret\",\"timeout_ms\":20000}\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    levels::store(&running.paths, Level::Off).unwrap();
    running.host.release();
    assert_eq!(read_line(&mut slow).unwrap()["data"]["released"], true);
    let answer = read_line(&mut waiter).unwrap();
    assert_eq!(answer["error"]["code"], "permission_denied", "{answer}");
}

/// Failure: help says killing a watch is how to end it early, and the session goes on holding its slot, so a few killed watches shut out the next. Ending the process must free it at once.
#[test]
fn a_killed_watch_frees_its_place() {
    use std::io::Write;
    let running = Running::start(Limits {
        max_subscribers: 1,
        ..Limits::default()
    });
    let watch = json!({ "cmd": "watch", "timeout_ms": 60_000 }).to_string() + "\n";
    let mut first = running.connect();
    first.write_all(watch.as_bytes()).unwrap();
    assert_eq!(read_line(&mut first).unwrap()["data"]["watching"], true);
    let mut second = running.connect();
    second.write_all(watch.as_bytes()).unwrap();
    assert_eq!(read_line(&mut second).unwrap()["ok"], false, "the one place is taken");
    drop(first);
    // The server hears it when `poll` wakes, which is at once, but it's a separate thread.
    let started = std::time::Instant::now();
    loop {
        let mut third = running.connect();
        third.write_all(watch.as_bytes()).unwrap();
        if read_line(&mut third).unwrap()["ok"] == true {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "the place wasn't freed");
    }
}
