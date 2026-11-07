//! The session on a socket. **One thread blocks in `poll` and owns every connection,** so a connection costs a small struct and a waiting client costs nothing else: no thread or task per client, per tab
//! or per wait, no timer ticking, and an idle session takes no wake-ups. Work that can take time runs on a short-lived thread per job, capped, and tells the loop through a socket pair when it's done.
//! With nothing happening there's one thread, asleep.
//!
//! Everything a client sends is hostile: lines are capped, partial lines have a deadline, unknown fields are refused, and a client that stops reading is dropped rather than buffered for. A panic in a job
//! becomes an error for that one client. The peer's user is checked on every connection, and there's no network listener, ever.

use crate::host::{Host, unsaved_refusal};
use crate::levels;
use crate::log::{self, ActivityLog};
use crate::state::StatePaths;
use recto_agent::table::{EVENT_GROUPS, event_group};
use recto_agent::{AgentError, Command, Condition, ConditionState, DEFAULT_WATCH_MS, Event, Level, MAX_COMMAND_BYTES, MAX_SELECTOR_BYTES, MAX_WAIT_MS, VERSION, allowed, condition_topics, exit, parse, refusal, reply, table};
use recto_platform::local::{self, Listener, PollEntry, SocketFile, Stream, Token};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Every limit on what clients can make the server do.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_connections: usize,
    /// Longest line a client may send.
    pub max_line_bytes: usize,
    /// How long a line may take to arrive once it's begun, so a client can't hold a slot with half a command.
    pub line_deadline: Duration,
    /// How long a connection may sit with no command in progress.
    pub idle_deadline: Duration,
    /// How long a client may take to read what's been sent to it.
    pub write_deadline: Duration,
    /// Jobs running at once, of any kind.
    pub max_jobs: usize,
    /// Jobs drawing or reading pages at once, each of which can use hundreds of megabytes.
    pub max_draw_jobs: usize,
    /// Clients in `wait`, `watch` or a waiting `permission request`, each of which holds its connection for as long as it likes. Well under `max_connections`, so they can't take every slot.
    pub max_subscribers: usize,
    /// Connections past `max_connections`, which may only do what must always get through: `status`, `stop`, `permission show` and the message after `permission set`. The oldest makes way for the next.
    pub max_overflow: usize,
    /// How long a connection past the cap may stay with nothing to say.
    pub overflow_idle: Duration,
    /// Largest single answer.
    pub max_reply_bytes: usize,
    /// All answers waiting to be read, together, past which only small ones are taken.
    pub max_queued_bytes: usize,
    /// All answers waiting to be read, including small ones. Nothing goes past this: whoever would take it past is dropped. What's between this and `max_queued_bytes` keeps `status` and the like answerable.
    pub max_queued_hard_bytes: usize,
    /// Unread bytes past which a client's next command waits. It keeps what it's been sent to itself rather than making the session work on.
    pub max_unsent_before_pause: usize,
    /// Events a watcher may leave unread before it's dropped.
    pub max_watcher_backlog: usize,
    /// Whose connections to take. This user's alone, except in a test of the refusal.
    pub expected_uid: u32,
    /// How long `stop` waits for running jobs.
    pub stop_grace: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 128,
            max_line_bytes: MAX_COMMAND_BYTES,
            line_deadline: Duration::from_secs(5),
            idle_deadline: Duration::from_secs(60),
            write_deadline: Duration::from_secs(30),
            max_jobs: 8,
            max_draw_jobs: 2,
            max_subscribers: 64,
            max_overflow: 16,
            overflow_idle: Duration::from_secs(5),
            max_reply_bytes: 32 * 1024 * 1024,
            max_queued_bytes: 128 * 1024 * 1024,
            max_queued_hard_bytes: 144 * 1024 * 1024,
            max_unsent_before_pause: 1024 * 1024,
            max_watcher_backlog: 1024 * 1024,
            expected_uid: local::current_uid(),
            stop_grace: Duration::from_secs(5),
        }
    }
}

/// Why the session couldn't start.
#[derive(Debug)]
pub enum BindError {
    AlreadyRunning,
    Refused(AgentError),
}

impl From<BindError> for AgentError {
    fn from(error: BindError) -> Self {
        match error {
            BindError::AlreadyRunning => AgentError::new("already_running", "A session is already running on this state folder.", exit::FAILED).then("`recto status` shows it, and `recto stop` ends it."),
            BindError::Refused(error) => error,
        }
    }
}

/// A bound socket, ready to run. Holds the lock that says this is the one session on this state folder.
#[derive(Debug)]
pub struct Server {
    paths: StatePaths,
    listener: Listener,
    wake_tx: Stream,
    wake_rx: Stream,
    /// Held until this ends, however it ends: the kernel lets go of it even after a kill. That's what makes a leftover socket safe to remove.
    _lock: File,
}

impl Server {
    /// Takes the lock, clears a socket left by a session that died, and binds.
    pub fn bind(paths: &StatePaths) -> Result<Self, BindError> {
        paths.prepare().map_err(BindError::Refused)?;
        let failed = |message: &str| BindError::Refused(AgentError::new("serve_failed", message.to_string(), exit::FAILED).then("Check that the state folder can be written to."));
        let lock = OpenOptions::new().create(true).truncate(false).write(true).open(&paths.lock).map_err(|_| failed("The session's lock file couldn't be opened."))?;
        if lock.try_lock().is_err() {
            return Err(BindError::AlreadyRunning);
        }
        // With the lock held, nothing is serving this folder, so a socket here is left over. Anything that isn't our own socket is someone else's business.
        match local::socket_file(&paths.socket) {
            Ok(SocketFile::Nothing) => {}
            Ok(SocketFile::OwnSocket) => std::fs::remove_file(&paths.socket).map_err(|_| failed("The old socket couldn't be removed."))?,
            Ok(SocketFile::Other) | Err(_) => {
                return Err(BindError::Refused(AgentError::new("socket_refused", "Something that isn't this session's socket is where its socket goes, so it won't be touched.", exit::PERMISSION).then("Tell the user what's there, and that it needs moving.")));
            }
        }
        let listener = Listener::bind(&paths.socket).map_err(|_| failed("The session's socket couldn't be made."))?;
        let (wake_tx, wake_rx) = Stream::pair().map_err(|_| failed("The session couldn't be started."))?;
        wake_tx.set_nonblocking(true).map_err(|_| failed("The session couldn't be started."))?;
        wake_rx.set_nonblocking(true).map_err(|_| failed("The session couldn't be started."))?;
        Ok(Self { paths: paths.clone(), listener, wake_tx, wake_rx, _lock: lock })
    }

    /// Asks a running server to end, from another thread. For a host that started it on its own thread.
    pub fn stopper(&self) -> std::io::Result<Stopper> {
        Ok(Stopper(self.wake_tx.try_clone()?))
    }

    /// Runs until `stop` or a [`Stopper`]. Blocks in `poll` the whole time.
    pub fn run<H: Host>(self, host: Arc<H>, limits: Limits) -> std::io::Result<()> {
        let (done_tx, done_rx) = channel();
        let last_level = levels::current(&self.paths);
        let mut state = Loop {
            log: ActivityLog::new(&self.paths),
            paths: self.paths.clone(),
            host,
            limits,
            started: Instant::now(),
            conns: Vec::new(),
            next_conn: 1,
            queue: VecDeque::new(),
            running: 0,
            running_draws: 0,
            done_tx,
            done_rx,
            wake: self.wake_tx.try_clone()?,
            queued_bytes: 0,
            generations: 0,
            pending: None,
            announced: None,
            announcements_left: MAX_ANNOUNCEMENTS,
            last_level,
            stop_request: None,
            stopping: None,
        };
        let result = state.run(&self.listener, &self.wake_rx);
        // Tidy on the way out. A kill skips this, and the next start clears up.
        let _ = std::fs::remove_file(&self.paths.socket);
        result
    }
}

/// Wakes a server and tells it to end.
#[derive(Debug)]
pub struct Stopper(Stream);

impl Stopper {
    pub fn stop(&self) {
        let _ = (&self.0).write(&[STOP_BYTE]);
    }
}

const WAKE_BYTE: u8 = 1;
const STOP_BYTE: u8 = 2;

#[derive(Debug)]
enum WaitFor {
    Condition { condition: Condition, tab: String, page: Option<u32> },
    Level(Level),
}

#[derive(Debug)]
enum State {
    Idle,
    /// A command is running for this client.
    Busy,
    Waiting { what: WaitFor, tag: &'static str, deadline: Instant, generation: u64, looking: bool, look_again: bool },
    Watching { groups: Option<Vec<String>>, deadline: Instant, remaining: Option<u32>, delivered: u64 },
}

/// One client, plus what it's in the middle of sending and being sent.
// size: asserted <= 320 by `a_connection_stays_small`. Held per connected client, at most `max_connections`.
#[derive(Debug)]
struct Conn {
    id: u64,
    stream: Stream,
    state: State,
    input: Vec<u8>,
    /// When the line now arriving began.
    line_started: Option<Instant>,
    output: Vec<u8>,
    /// How much of `output` has gone.
    sent: usize,
    /// When `output` last stopped being empty or moved.
    output_since: Instant,
    last_activity: Instant,
    input_closed: bool,
    /// Close when `output` has gone.
    closing: bool,
    gone: bool,
    /// Taken past the connection cap: it may only do what must always get through.
    overflow: bool,
}

enum Job {
    Command { conn: u64, command: Command },
    Look { conn: u64, generation: u64, condition: Condition, tab: String, page: Option<u32> },
}

impl Job {
    fn draws(&self) -> bool {
        matches!(self, Self::Command { command, .. } if command.draws())
    }

    fn conn(&self) -> u64 {
        match self {
            Self::Command { conn, .. } | Self::Look { conn, .. } => *conn,
        }
    }
}

enum Done {
    Answered { conn: u64, line: Vec<u8>, tag: &'static str, level: Level, outcome: &'static str, details: Value, events: Vec<Event>, draws: bool, stop: bool },
    Looked { conn: u64, generation: u64, state: ConditionState },
}

struct Loop<H> {
    paths: StatePaths,
    log: ActivityLog,
    host: Arc<H>,
    limits: Limits,
    started: Instant,
    conns: Vec<Conn>,
    next_conn: u64,
    queue: VecDeque<Job>,
    running: usize,
    running_draws: usize,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    wake: Stream,
    /// Everything waiting in clients' `output`.
    queued_bytes: usize,
    generations: u64,
    /// The latest level an agent has asked for. One slot: this tells the user something's wanted, and can't grow.
    pending: Option<Level>,
    /// The highest level announced on stderr since the level last changed, so a client asking in a loop can't fill the file those messages go to.
    announced: Option<Level>,
    /// Announcements left, as a last line of defence for that file.
    announcements_left: u32,
    last_level: Level,
    /// `stop` is waiting for what's running to end, before it checks for unsaved edits and either ends this or says why not.
    stop_request: Option<StopRequest>,
    /// Ending for certain: when to give up on what's running.
    stopping: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct StopRequest {
    conn: u64,
    deadline: Instant,
    /// The check is on its way, so only one goes.
    checking: bool,
}

/// How much of a sent answer can sit in front of the rest of `output` before the room is taken back.
const MAX_SENT_KEPT: usize = 64 * 1024;
/// An answer this small can go past `max_queued_bytes`, so `status` and errors still work while large answers are refused.
const SMALL_ANSWER_BYTES: usize = 64 * 1024;
/// Announcements in one session's life.
const MAX_ANNOUNCEMENTS: u32 = 1024;

impl<H: Host> Loop<H> {
    fn run(&mut self, listener: &Listener, wake_rx: &Stream) -> std::io::Result<()> {
        let mut entries = Vec::with_capacity(self.limits.max_connections + 2);
        loop {
            entries.clear();
            entries.push(PollEntry::new(listener.token(), self.stopping.is_none(), false));
            entries.push(PollEntry::new(wake_rx.token(), true, false));
            for conn in &self.conns {
                // A client that closed its end while its answer is being worked out would wake this for nothing, over and over, until the job ends.
                let token = if conn.input_closed && !conn.has_output() { Token::NONE } else { conn.stream.token() };
                entries.push(PollEntry::new(token, conn.wants_input(self.limits.max_line_bytes, self.limits.max_unsent_before_pause), conn.has_output()));
            }
            let timeout = self.next_deadline().map(|at| at.saturating_duration_since(Instant::now()));
            local::poll(&mut entries, timeout)?;
            if entries[1].readable && self.drain_wake(wake_rx) {
                self.begin_stop();
            }
            while let Ok(done) = self.done_rx.try_recv() {
                self.finish(done);
            }
            if entries[0].readable {
                self.accept(listener);
            }
            // A connection accepted just now isn't in `entries` yet, and comes round next time. Any removed go at the end, so the indices still line up here.
            for (index, entry) in entries.iter().skip(2).enumerate() {
                let Some(conn) = self.conns.get_mut(index) else { break };
                if entry.readable {
                    conn.read_some(self.limits.max_line_bytes);
                }
                if entry.gone && !entry.readable {
                    conn.note_peer_gone();
                }
            }
            self.flush_all();
            self.take_lines();
            self.expire();
            self.advance_stop();
            self.start_jobs();
            self.reap();
            if self.finished_stopping() {
                return Ok(());
            }
        }
    }

    /// Reads the wake bytes. Returns whether one asked to stop.
    fn drain_wake(&self, wake_rx: &Stream) -> bool {
        let mut stop = false;
        let mut bytes = [0u8; 64];
        loop {
            let mut reader = wake_rx;
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => return stop,
                Ok(count) => stop |= bytes[..count].contains(&STOP_BYTE),
            }
        }
    }

    fn accept(&mut self, listener: &Listener) {
        loop {
            match listener.accept() {
                Ok(Some(stream)) => self.take_client(stream),
                // Out of descriptors, the kernel here drops the waiting connection as accept fails, so the listener doesn't stay ready and there's nothing to spin on. `out_of_descriptors_the_session_doesnt_spin` holds that.
                Ok(None) | Err(_) => return,
            }
        }
    }

    fn take_client(&mut self, stream: Stream) {
        // The socket's mode already keeps other users out. This is the second lock on the same door.
        if stream.peer_uid().map_or(true, |uid| uid != self.limits.expected_uid) {
            self.log.record("connect", Level::Off, "peer_refused", &Value::Null);
            return;
        }
        // A full house mustn't hide a lowered level: whoever just connected may be the one lowering it, and may not get to say so.
        self.observe_level();
        let regular = self.conns.iter().filter(|conn| !conn.overflow).count();
        let overflow = regular >= self.limits.max_connections;
        if overflow && self.conns.iter().filter(|conn| conn.overflow && !conn.gone).count() >= self.limits.max_overflow {
            // Someone just arrived with something to say, and the oldest of these has had its turn.
            if let Some(oldest) = self.conns.iter_mut().find(|conn| conn.overflow && !conn.gone) {
                oldest.gone = true;
            }
        }
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let now = Instant::now();
        let id = self.next_conn;
        self.next_conn += 1;
        self.conns.push(Conn { id, stream, state: State::Idle, input: Vec::new(), line_started: None, output: Vec::new(), sent: 0, output_since: now, last_activity: now, input_closed: false, closing: false, gone: false, overflow });
    }

    fn take_lines(&mut self) {
        for index in 0..self.conns.len() {
            // A client with a lot it hasn't read waits before its next command is even looked at, so it can't make the session work for answers nobody takes.
            while matches!(self.conns[index].state, State::Idle) && !self.conns[index].gone && !self.conns[index].closing && self.conns[index].unsent() <= self.limits.max_unsent_before_pause {
                let Some(line) = self.conns[index].next_line() else {
                    if self.conns[index].input.len() > self.limits.max_line_bytes {
                        self.refuse_line(index);
                    }
                    break;
                };
                self.handle_line(index, &line);
            }
        }
    }

    fn refuse_line(&mut self, index: usize) {
        let error = AgentError::new("bad_command", "That command is too long.", exit::FAILED);
        self.log.record("unknown", levels::current(&self.paths), error.code, &Value::Null);
        self.answer(index, "unknown", &Err(error));
        let conn = &mut self.conns[index];
        conn.closing = true;
        conn.input = Vec::new();
    }

    fn handle_line(&mut self, index: usize, line: &[u8]) {
        let level = self.observe_level();
        self.conns[index].last_activity = Instant::now();
        if self.stopping.is_some() {
            self.answer(index, "unknown", &Err(AgentError::new("session_ended", "The session is stopping.", exit::NOT_RUNNING).then("Start it again with `recto serve --background`.")));
            return;
        }
        let command = std::str::from_utf8(line).map_err(|_| AgentError::new("bad_command", "That isn't a command RectoPDF knows.", exit::FAILED)).and_then(parse);
        let command = match command {
            Ok(command) => command,
            Err(error) => {
                self.log.record("unknown", level, error.code, &Value::Null);
                self.answer(index, "unknown", &Err(error));
                return;
            }
        };
        let needed = command.access();
        if !allowed(level, needed) {
            let error = refusal(table::display_name(command.tag()), level, needed);
            self.log.record(command.tag(), level, error.code, &command.log_details());
            self.answer(index, command.tag(), &Err(error));
            return;
        }
        let tag = command.tag();
        // Past the connection cap, only what must always get through.
        if self.conns[index].overflow && !matches!(command, Command::Status {} | Command::LevelChanged {} | Command::PermissionShow {} | Command::Stop {}) {
            self.answer(index, tag, &Err(AgentError::new("busy", "The session has too many connections right now.", exit::BUSY)));
            self.conns[index].closing = true;
            return;
        }
        // Only the quick answers while `stop` waits: anything else could change what it's about to check.
        if self.stop_request.is_some() && !matches!(command, Command::Status {} | Command::LevelChanged {} | Command::PermissionShow {} | Command::Log { .. }) {
            self.answer(index, tag, &Err(stopping_now()));
            return;
        }
        match command {
            Command::Stop {} => {
                self.conns[index].state = State::Busy;
                let conn = self.conns[index].id;
                self.stop_request = Some(StopRequest { conn, deadline: Instant::now() + self.limits.stop_grace, checking: false });
                // Anything still waiting its turn could be an edit that `stop` would then lose.
                let queued: Vec<u64> = self.queue.iter().filter(|job| matches!(job, Job::Command { .. })).map(Job::conn).collect();
                self.queue.retain(|job| !matches!(job, Job::Command { .. }));
                for conn in queued {
                    if let Some(index) = self.conns.iter().position(|c| c.id == conn) {
                        self.answer(index, "unknown", &Err(stopping_now()));
                    }
                }
            }
            Command::Status {} => {
                let data = self.status(level);
                self.log.record(tag, level, "ok", &Value::Null);
                self.answer(index, tag, &Ok(data));
            }
            Command::LevelChanged {} => self.answer(index, tag, &Ok(json!({ "level": level.name() }))),
            Command::PermissionShow {} => {
                let data = levels::show(&self.paths, self.pending);
                self.log.record(tag, level, "ok", &Value::Null);
                self.answer(index, tag, &Ok(data));
            }
            Command::PermissionRequest { level: wanted, timeout_ms } => self.permission_request(index, level, wanted, timeout_ms),
            Command::Wait { tab, .. } if tab.len() > MAX_SELECTOR_BYTES => {
                self.answer(index, tag, &Err(AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND)));
            }
            Command::Wait { .. } | Command::Watch { .. } if self.subscribers() >= self.limits.max_subscribers => {
                self.log.record(tag, level, "busy", &Value::Null);
                self.answer(index, tag, &Err(too_many_waiting()));
            }
            Command::Wait { condition, tab, page, timeout_ms } => {
                let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(MAX_WAIT_MS));
                self.log.record(tag, level, "started", &json!({ "condition": format!("{condition:?}") }));
                let generation = self.next_generation();
                self.conns[index].state = State::Waiting { what: WaitFor::Condition { condition, tab, page }, tag, deadline, generation, looking: false, look_again: false };
                self.look(index);
            }
            Command::Watch { events, timeout_ms, count } => self.watch(index, level, events, timeout_ms, count),
            Command::WatchStop {} => {
                let watching: Vec<(usize, u64)> = self.conns.iter().enumerate().filter_map(|(at, conn)| match conn.state {
                    State::Watching { delivered, .. } => Some((at, delivered)),
                    _ => None,
                }).collect();
                for (at, delivered) in &watching {
                    self.end_watch(*at, "ended by `recto watch stop`", *delivered);
                }
                let stopped = watching.len();
                self.log.record(tag, level, "ok", &json!({ "stopped": stopped }));
                self.answer(index, tag, &Ok(json!({ "stopped": stopped })));
            }
            Command::Log { last } => {
                self.log.flush();
                let data = log::recent(&self.paths, last);
                self.log.record(tag, level, "ok", &Value::Null);
                self.answer(index, tag, &Ok(data));
            }
            command => {
                self.conns[index].state = State::Busy;
                let conn = self.conns[index].id;
                self.queue.push_back(Job::Command { conn, command });
            }
        }
    }

    /// Reads the level, and tells anyone it matters to if it moved. Called for every command, so it's never stale for long even if the file changed with no message.
    fn observe_level(&mut self) -> Level {
        let level = levels::current(&self.paths);
        if level == self.last_level {
            return level;
        }
        let was = self.last_level;
        self.last_level = level;
        self.announced = None;
        if level > was && self.pending.is_some_and(|pending| level >= pending) {
            self.pending = None;
        }
        self.publish(&[Event::new("level_changed", json!({ "level": level.name(), "was": was.name() }))]);
        self.settle_subscribers(level);
        level
    }

    /// Ends waits and watches the new level no longer allows, and answers anyone waiting for this level.
    fn settle_subscribers(&mut self, level: Level) {
        let needed = table::by_tag("watch").map_or(Level::ReadOnly, |spec| match spec.access {
            table::Access::At(level) => level,
            _ => Level::Full,
        });
        for index in 0..self.conns.len() {
            match &self.conns[index].state {
                State::Waiting { what: WaitFor::Level(wanted), .. } if level >= *wanted => {
                    let data = levels::request_answer(*wanted, level);
                    self.answer(index, "permission_request", &Ok(data));
                }
                State::Waiting { what: WaitFor::Condition { .. }, tag, .. } if level < needed => {
                    let tag = *tag;
                    self.answer(index, tag, &Err(refusal(table::display_name(tag), level, table::Access::At(needed))));
                }
                State::Watching { .. } if level < needed => {
                    self.conns[index].closing = true;
                    self.answer(index, "watch", &Err(refusal("watch", level, table::Access::At(needed))));
                }
                _ => {}
            }
        }
    }

    fn status(&self, level: Level) -> Value {
        let mut data = json!({
            "running": true,
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "api": VERSION,
            "level": level.name(),
            "label": level.label(),
            "uptime_ms": u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "socket": self.paths.socket,
            // How many, at every level, so an agent at Off can tell whether asking for more is worthwhile. A count holds nothing from a document. Names need Read-only.
            "tabs": self.host.tab_count(),
        });
        if !self.log.healthy() {
            data["warning"] = json!("The activity log can't be written.");
        }
        data
    }

    fn permission_request(&mut self, index: usize, level: Level, wanted: Level, timeout_ms: Option<u64>) {
        let tag = "permission_request";
        let data = levels::request_answer(wanted, level);
        if level >= wanted {
            self.log.record(tag, level, "granted", &json!({ "level": wanted.name() }));
            self.answer(index, tag, &Ok(data));
            return;
        }
        // Where the user would see it, if they're watching the session's window. The app will show a prompt here instead.
        // Only for something higher than it's said since the level last changed, so a client asking in a loop, or alternating, can't fill the file these go to. A full disk or a closed pipe is no reason to fall over.
        if self.announced.is_none_or(|said| wanted > said) && self.announcements_left > 0 {
            self.announced = Some(wanted);
            self.announcements_left -= 1;
            let _ = writeln!(std::io::stderr(), "An agent asks for {} access. To allow it, run: recto permission set {}", wanted.label(), wanted.name());
        }
        self.pending = Some(self.pending.map_or(wanted, |pending| pending.max(wanted)));
        self.log.record(tag, level, "asked", &json!({ "level": wanted.name() }));
        match timeout_ms {
            None => self.answer(index, tag, &Ok(data)),
            Some(_) if self.subscribers() >= self.limits.max_subscribers => self.answer(index, tag, &Err(too_many_waiting())),
            Some(timeout_ms) => {
                let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(MAX_WAIT_MS));
                let generation = self.next_generation();
                self.conns[index].state = State::Waiting { what: WaitFor::Level(wanted), tag, deadline, generation, looking: false, look_again: false };
            }
        }
    }

    fn watch(&mut self, index: usize, level: Level, events: Option<Vec<String>>, timeout_ms: Option<u64>, count: Option<u32>) {
        if let Some(unknown) = events.iter().flatten().find(|group| !EVENT_GROUPS.contains(&group.as_str())) {
            let error = AgentError::new("bad_events", format!("`{}` isn't something `watch` knows. It knows: {}.", recto_core::clean_line(unknown), EVENT_GROUPS.join(", ")), exit::FAILED).then("`recto help watch` lists them.");
            self.log.record("watch", level, error.code, &Value::Null);
            self.answer(index, "watch", &Err(error));
            return;
        }
        self.log.record("watch", level, "started", &Value::Null);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_WATCH_MS).min(MAX_WAIT_MS));
        let shown = events.clone().map_or_else(|| json!(EVENT_GROUPS), |groups| json!(groups));
        self.conns[index].state = State::Watching { groups: events, deadline, remaining: count, delivered: 0 };
        self.send_event_line(index, &json!({ "watching": true, "events": shown }));
        // Asked for none: there's nothing to wait for, and one would be more than it asked for.
        if count == Some(0) {
            self.end_watch(index, "count reached", 0);
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generations += 1;
        self.generations
    }

    /// Asks a job to look at what this client is waiting for. Only one look at a time per client: if something happens during one, there's one more after.
    fn look(&mut self, index: usize) {
        let id = self.conns[index].id;
        let State::Waiting { what, generation, looking, look_again, .. } = &mut self.conns[index].state else { return };
        let WaitFor::Condition { condition, tab, page } = what else { return };
        if *looking {
            *look_again = true;
            return;
        }
        *looking = true;
        self.queue.push_back(Job::Look { conn: id, generation: *generation, condition: *condition, tab: tab.clone(), page: *page });
    }

    fn finish(&mut self, done: Done) {
        match done {
            Done::Answered { conn, line, tag, level, outcome, details, events, draws, stop } => {
                self.running -= 1;
                if draws {
                    self.running_draws -= 1;
                }
                self.log.record(tag, level, outcome, &details);
                self.publish(&events);
                if tag == "stop" {
                    self.stop_request = None;
                }
                if let Some(index) = self.conns.iter().position(|c| c.id == conn) {
                    let now = Instant::now();
                    let client = &mut self.conns[index];
                    client.state = State::Idle;
                    client.last_activity = now;
                    client.line_started = (!client.input.is_empty()).then_some(now);
                    self.send_answer(index, tag, line);
                }
                if stop {
                    self.begin_stop();
                }
            }
            Done::Looked { conn, generation, state } => {
                self.running -= 1;
                let Some(index) = self.conns.iter().position(|c| c.id == conn) else { return };
                // A look from an earlier wait on this connection says nothing about this one.
                let State::Waiting { generation: current, looking, look_again, tag, .. } = &mut self.conns[index].state else { return };
                if *current != generation {
                    return;
                }
                *looking = false;
                let tag = *tag;
                let again = std::mem::take(look_again);
                match state {
                    ConditionState::Met(data) => self.answer(index, tag, &Ok(data)),
                    ConditionState::Failed(error) => self.answer(index, tag, &Err(error)),
                    ConditionState::Waiting => {
                        if again {
                            self.look(index);
                        }
                    }
                }
            }
        }
    }

    /// Tells everyone who cares. A wait looks again; a watcher gets a line.
    fn publish(&mut self, events: &[Event]) {
        for event in events {
            let group = event_group(event.name);
            for index in 0..self.conns.len() {
                let looks = matches!(&self.conns[index].state, State::Waiting { what: WaitFor::Condition { condition, .. }, .. } if condition_topics(*condition).contains(&group));
                if looks {
                    self.look(index);
                    continue;
                }
                let State::Watching { groups, remaining, delivered, .. } = &mut self.conns[index].state else { continue };
                if groups.as_ref().is_some_and(|groups| !groups.iter().any(|wanted| wanted == group)) {
                    continue;
                }
                *delivered += 1;
                let total = *delivered;
                let finished = match remaining {
                    Some(left) => {
                        *left = left.saturating_sub(1);
                        *left == 0
                    }
                    None => false,
                };
                self.send_event_line(index, &event.to_json());
                if finished {
                    self.end_watch(index, "count reached", total);
                }
            }
        }
    }

    fn end_watch(&mut self, index: usize, reason: &str, delivered: u64) {
        self.conns[index].state = State::Idle;
        self.conns[index].last_activity = Instant::now();
        let data = json!({ "done": true, "reason": reason, "delivered": delivered });
        self.answer(index, "watch", &Ok(data));
    }

    fn send_event_line(&mut self, index: usize, data: &Value) {
        let mut line = reply("watch", &Ok(data.clone())).to_string();
        line.push('\n');
        let conn = &self.conns[index];
        if conn.output.len() - conn.sent + line.len() > self.limits.max_watcher_backlog || self.queued_bytes + line.len() > self.limits.max_queued_hard_bytes {
            // It's stopped reading. Dropping it is better than holding what it won't read.
            self.conns[index].gone = true;
            return;
        }
        self.queue_output(index, line.as_bytes());
    }

    /// Answers a client, which ends whatever it was doing, and takes back anything queued for what it was doing.
    fn answer(&mut self, index: usize, tag: &str, result: &Result<Value, AgentError>) {
        let mut line = reply(tag, result).to_string().into_bytes();
        line.push(b'\n');
        let id = self.conns[index].id;
        if matches!(self.conns[index].state, State::Waiting { .. }) {
            self.queue.retain(|job| !matches!(job, Job::Look { conn, .. } if *conn == id));
        }
        let conn = &mut self.conns[index];
        conn.state = State::Idle;
        conn.last_activity = Instant::now();
        self.send_answer(index, tag, line);
    }

    /// Queues an answer within the caps. One too large for what's left is replaced by an error saying so, small ones keep going past that so `status` still answers, and nothing at all goes past the hard cap.
    fn send_answer(&mut self, index: usize, tag: &str, mut line: Vec<u8>) {
        let over_soft = self.queued_bytes + line.len() > self.limits.max_queued_bytes;
        if line.len() > self.limits.max_reply_bytes || (over_soft && line.len() > SMALL_ANSWER_BYTES) {
            line = too_large(tag);
        }
        if self.queued_bytes + line.len() > self.limits.max_queued_hard_bytes {
            self.conns[index].gone = true;
            return;
        }
        self.queue_output(index, &line);
    }

    fn queue_output(&mut self, index: usize, line: &[u8]) {
        let conn = &mut self.conns[index];
        if conn.output.is_empty() {
            conn.output_since = Instant::now();
        }
        conn.output.extend_from_slice(line);
        self.queued_bytes += line.len();
        let flushed = conn.flush();
        self.queued_bytes -= flushed;
    }

    fn flush_all(&mut self) {
        for conn in &mut self.conns {
            if conn.has_output() {
                let flushed = conn.flush();
                self.queued_bytes -= flushed;
            }
        }
    }

    fn start_jobs(&mut self) {
        let mut index = 0;
        while index < self.queue.len() && self.running < self.limits.max_jobs {
            if self.queue[index].draws() && self.running_draws >= self.limits.max_draw_jobs {
                index += 1;
                continue;
            }
            let Some(job) = self.queue.remove(index) else { break };
            self.spawn(job);
        }
    }

    fn spawn(&mut self, job: Job) {
        let draws = job.draws();
        let conn = job.conn();
        self.running += 1;
        if draws {
            self.running_draws += 1;
        }
        let host = Arc::clone(&self.host);
        let paths = self.paths.clone();
        let done = self.done_tx.clone();
        let wake = self.wake.try_clone();
        let max_reply = self.limits.max_reply_bytes;
        // A hostile file can use a lot of stack, in a parser that recurses. It only costs what it touches.
        let spawned = wake.and_then(|wake| {
            std::thread::Builder::new().name("recto-job".into()).stack_size(8 * 1024 * 1024).spawn(move || {
                let _ = done.send(run_job(&*host, &paths, job, max_reply));
                let _ = (&wake).write(&[WAKE_BYTE]);
            })
        });
        if spawned.is_err() {
            self.running -= 1;
            if draws {
                self.running_draws -= 1;
            }
            if let Some(index) = self.conns.iter().position(|c| c.id == conn) {
                self.answer(index, "unknown", &Err(AgentError::new("busy", "The session couldn't start this right now.", exit::BUSY)));
            }
        }
    }

    /// Ends what's run out of time, and drops clients that have stalled.
    fn expire(&mut self) {
        let now = Instant::now();
        for index in 0..self.conns.len() {
            let conn = &self.conns[index];
            let stalled_output = conn.has_output() && now.saturating_duration_since(conn.output_since) >= self.limits.write_deadline;
            // Only while it's its turn to speak: a client may send its next command while the last one runs.
            let stalled_line = matches!(conn.state, State::Idle) && conn.line_started.is_some_and(|started| now.saturating_duration_since(started) >= self.limits.line_deadline);
            let idle_limit = if conn.overflow { self.limits.overflow_idle } else { self.limits.idle_deadline };
            let idle = matches!(conn.state, State::Idle) && conn.input.is_empty() && !conn.has_output() && now.saturating_duration_since(conn.last_activity) >= idle_limit;
            if stalled_output || stalled_line || idle {
                self.conns[index].gone = true;
                continue;
            }
            match &self.conns[index].state {
                // Not while a look is on its way: what's already true must come back true, even with no time at all to wait.
                State::Waiting { deadline, tag, what, looking: false, .. } if now >= *deadline => {
                    let tag = *tag;
                    let error = match what {
                        WaitFor::Level(wanted) => {
                            let command = format!("recto permission set {}", wanted.name());
                            AgentError::new("timed_out", "The user hasn't allowed this yet.", exit::TIMED_OUT).then(format!("Tell the user to run `{command}` in their own terminal, then try again."))
                        }
                        WaitFor::Condition { .. } => AgentError::new("timed_out", "The time ran out before that happened.", exit::TIMED_OUT).then("Check `recto status` and `recto tabs`, or wait longer with `--timeout`."),
                    };
                    self.log.record(tag, self.last_level, "timed_out", &Value::Null);
                    self.answer(index, tag, &Err(error));
                }
                State::Watching { deadline, remaining, delivered, .. } if now >= *deadline => {
                    let total = *delivered;
                    if remaining.is_some() {
                        let error = AgentError::new("timed_out", "The time ran out before that many events came.", exit::TIMED_OUT);
                        self.answer(index, "watch", &Err(error));
                    } else {
                        self.end_watch(index, "time reached", total);
                    }
                }
                _ => {}
            }
        }
    }

    fn reap(&mut self) {
        for conn in self.conns.iter().filter(|conn| conn.finished()) {
            self.queued_bytes -= conn.output.len() - conn.sent;
            let id = conn.id;
            // A look for someone who's gone is work for nobody.
            self.queue.retain(|job| !matches!(job, Job::Look { conn, .. } if *conn == id));
        }
        self.conns.retain(|conn| !conn.finished());
    }

    fn begin_stop(&mut self) {
        if self.stopping.is_some() {
            return;
        }
        self.stopping = Some(Instant::now() + self.limits.stop_grace);
        // An edit still waiting its turn would answer ok and then be lost.
        let queued: Vec<u64> = self.queue.iter().map(Job::conn).collect();
        self.queue.clear();
        for conn in queued {
            if let Some(index) = self.conns.iter().position(|c| c.id == conn)
                && matches!(self.conns[index].state, State::Busy)
            {
                self.answer(index, "unknown", &Err(stopping_now()));
            }
        }
        for index in 0..self.conns.len() {
            match &self.conns[index].state {
                State::Waiting { tag, .. } => {
                    let tag = *tag;
                    self.answer(index, tag, &Err(AgentError::new("session_ended", "The session is stopping.", exit::NOT_RUNNING).then("Start it again with `recto serve --background`.")));
                }
                State::Watching { delivered, .. } => {
                    let total = *delivered;
                    self.end_watch(index, "session stopping", total);
                }
                _ => {}
            }
        }
    }

    /// `stop` waits for what's running to end, and only then checks for unsaved edits. Anything checked earlier could change.
    fn advance_stop(&mut self) {
        let Some(request) = self.stop_request else { return };
        if request.checking {
            return;
        }
        let queued_commands = self.queue.iter().any(|job| matches!(job, Job::Command { .. }));
        if self.running == 0 && !queued_commands {
            self.queue.push_front(Job::Command { conn: request.conn, command: Command::Stop {} });
            self.stop_request = Some(StopRequest { checking: true, ..request });
        } else if Instant::now() >= request.deadline {
            self.stop_request = None;
            if let Some(index) = self.conns.iter().position(|c| c.id == request.conn) {
                let error = AgentError::new("busy", "A command is still running, so the session wasn't stopped.", exit::BUSY).then("Try again when it's finished. `recto status` shows the session is alive.");
                self.answer(index, "stop", &Err(error));
            }
        }
    }

    fn subscribers(&self) -> usize {
        self.conns.iter().filter(|conn| matches!(conn.state, State::Waiting { .. } | State::Watching { .. })).count()
    }

    fn finished_stopping(&self) -> bool {
        let Some(deadline) = self.stopping else { return false };
        let busy = self.running > 0 || !self.queue.is_empty() || self.conns.iter().any(Conn::has_output);
        !busy || Instant::now() >= deadline
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut soonest = self.stopping;
        let mut consider = |at: Instant| soonest = Some(soonest.map_or(at, |so_far| so_far.min(at)));
        if let Some(request) = self.stop_request
            && !request.checking
        {
            consider(request.deadline);
        }
        for conn in &self.conns {
            if conn.has_output() {
                consider(conn.output_since + self.limits.write_deadline);
            }
            match &conn.state {
                State::Idle => {
                    if let Some(started) = conn.line_started {
                        consider(started + self.limits.line_deadline);
                    }
                    if conn.input.is_empty() && !conn.has_output() {
                        consider(conn.last_activity + if conn.overflow { self.limits.overflow_idle } else { self.limits.idle_deadline });
                    }
                }
                // With a look on its way the deadline can't act yet, and counting it would spin until the look ends.
                State::Waiting { looking: true, .. } => {}
                State::Waiting { deadline, .. } | State::Watching { deadline, .. } => consider(*deadline),
                State::Busy => {}
            }
        }
        soonest
    }
}

fn too_large(tag: &str) -> Vec<u8> {
    let error = AgentError::new("reply_too_large", "That answer is too large to send.", exit::FAILED).then("Ask for less, such as one page or fewer entries.");
    let mut line = reply(tag, &Err(error)).to_string();
    line.push('\n');
    line.into_bytes()
}

impl Conn {
    fn wants_input(&self, max_line: usize, max_unsent: usize) -> bool {
        !self.input_closed && !self.gone && !self.closing && self.input.len() <= max_line && self.unsent() <= max_unsent
    }

    /// What's been queued for this client and not yet taken.
    fn unsent(&self) -> usize {
        self.output.len() - self.sent
    }

    fn has_output(&self) -> bool {
        self.sent < self.output.len()
    }

    /// Everything it was owed has gone, or there's no one to send it to.
    fn finished(&self) -> bool {
        self.gone || (self.closing && !self.has_output()) || (self.input_closed && matches!(self.state, State::Idle) && !self.has_output() && self.input.is_empty())
    }

    fn read_some(&mut self, max_line: usize) {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match (&self.stream).read(&mut chunk) {
                Ok(0) => {
                    self.note_peer_gone();
                    return;
                }
                Ok(_) if matches!(self.state, State::Waiting { .. } | State::Watching { .. }) => {
                    // A subscribed client has nothing more to say, and one that writes without pause would otherwise hold this whole thread. It goes.
                    self.gone = true;
                    return;
                }
                Ok(count) => {
                    if self.input.is_empty() {
                        self.line_started = Some(Instant::now());
                    }
                    self.input.extend_from_slice(&chunk[..count]);
                    if self.input.len() > max_line {
                        return;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.gone = true;
                    return;
                }
            }
        }
    }

    /// The other end has closed. An answer in progress may still be read, by a client that only closed its sending side. A wait or a watch has no one left to tell.
    fn note_peer_gone(&mut self) {
        self.input_closed = true;
        if matches!(self.state, State::Waiting { .. } | State::Watching { .. }) {
            self.gone = true;
        }
        // Half a line that will never be finished.
        if !self.input.contains(&b'\n') {
            self.input = Vec::new();
            self.line_started = None;
        }
    }

    /// The next whole line, if there is one and it isn't blank.
    fn next_line(&mut self) -> Option<Vec<u8>> {
        loop {
            let at = self.input.iter().position(|byte| *byte == b'\n')?;
            let line: Vec<u8> = self.input.drain(..=at).collect();
            self.line_started = (!self.input.is_empty()).then(Instant::now);
            if self.input.is_empty() && self.input.capacity() > 4096 {
                self.input = Vec::new();
            }
            let trimmed = line.trim_ascii();
            if !trimmed.is_empty() {
                return Some(trimmed.to_vec());
            }
        }
    }

    /// Sends what it can without blocking. Returns how many bytes left the queue. A pipe that fails means the client has gone.
    fn flush(&mut self) -> usize {
        let before = self.sent;
        while self.sent < self.output.len() {
            match (&self.stream).write(&self.output[self.sent..]) {
                Ok(0) => {
                    self.gone = true;
                    break;
                }
                Ok(count) => {
                    self.sent += count;
                    self.output_since = Instant::now();
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.gone = true;
                    break;
                }
            }
        }
        let flushed = self.sent - before;
        if self.sent == self.output.len() {
            // Most connections last one command, but one sent a whole page shouldn't keep the room for it.
            self.output = Vec::new();
            self.sent = 0;
        } else if self.sent > MAX_SENT_KEPT {
            // A slow watcher never gets all the way to empty. What it's read goes, or this grows for as long as it watches.
            self.output.drain(..self.sent);
            self.sent = 0;
        }
        flushed
    }
}

/// Runs one job, on its own thread. A panic comes back as an error for that one client.
fn run_job<H: Host>(host: &H, paths: &StatePaths, job: Job, max_reply: usize) -> Done {
    match job {
        Job::Look { conn, generation, condition, tab, page } => {
            // Read again: the answer can hold a tab's details, and the level may have dropped since this was asked for.
            let level = levels::current(paths);
            let needed = table::by_tag("wait").map_or(table::Access::Nobody, |spec| spec.access);
            let state = if allowed(level, needed) {
                catch_unwind(AssertUnwindSafe(|| host.check(condition, &tab, page))).unwrap_or_else(|_| ConditionState::Failed(internal()))
            } else {
                ConditionState::Failed(refusal("wait", level, needed))
            };
            Done::Looked { conn, generation, state }
        }
        Job::Command { conn, command } => {
            let draws = command.draws();
            let tag = command.tag();
            // Read again: it may have been lowered while this waited its turn.
            let level = levels::current(paths);
            // Before it runs, because `close` leaves nothing to ask afterwards. Only for a command that's allowed: a refusal hasn't acted on anything, and shouldn't take the session's lock to say so.
            let acted = if allowed(level, command.access()) { catch_unwind(AssertUnwindSafe(|| host.acted_tab(&command))).unwrap_or(None) } else { None };
            let mut events = Vec::new();
            let result = catch_unwind(AssertUnwindSafe(|| {
                if !allowed(level, command.access()) {
                    return Err(refusal(table::display_name(tag), level, command.access()));
                }
                if matches!(command, Command::Stop {}) {
                    let unsaved = host.unsaved_tabs();
                    return if unsaved.is_empty() { Ok(json!({ "stopping": true })) } else { Err(unsaved_refusal(&unsaved)) };
                }
                host.run(level, &command, &mut events)
            }))
            .unwrap_or_else(|_| Err(internal()));
            let outcome = result.as_ref().map_or_else(|error| error.code, |_| "ok");
            let details = recto_agent::with_acted_tab(command.log_details(), acted, &result, &command);
            let stop = matches!(command, Command::Stop {}) && result.is_ok();
            let mut line = reply(tag, &result).to_string();
            line.push('\n');
            // Build this here, off the socket's thread: a page's text can be large.
            let line = if line.len() > max_reply { too_large(tag) } else { line.into_bytes() };
            Done::Answered { conn, line, tag, level, outcome, details, events, draws, stop }
        }
    }
}

fn stopping_now() -> AgentError {
    AgentError::new("busy", "The session is stopping, so this wasn't done.", exit::BUSY).then("If it didn't stop, try again. `recto status` shows whether it's still there.")
}

fn too_many_waiting() -> AgentError {
    AgentError::new("busy", "Too many clients are waiting on this session right now.", exit::BUSY)
}

fn internal() -> AgentError {
    AgentError::new("internal", "RectoPDF hit an internal error on this command. The session is still running.", exit::FAILED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a watcher that keeps up slowly never gets to empty, so what it's already read stays in its buffer for as long as it watches, up to 24 hours.
    #[test]
    fn a_slow_readers_buffer_doesnt_keep_what_its_read() {
        let (ours, theirs) = Stream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        theirs.set_nonblocking(true).unwrap();
        let now = Instant::now();
        let mut conn = Conn { id: 1, stream: ours, state: State::Idle, input: Vec::new(), line_started: None, output: vec![b'x'; 2 * 1024 * 1024], sent: 0, output_since: now, last_activity: now, input_closed: false, closing: false, gone: false, overflow: false };
        let mut chunk = vec![0u8; 64 * 1024];
        let mut moved = 0;
        // It reads a bit, and never catches up, so the buffer never gets to empty.
        while conn.has_output() && moved < 512 {
            let flushed = conn.flush();
            moved += usize::from(flushed > 0);
            assert!(conn.sent <= MAX_SENT_KEPT, "{} bytes already read are still held", conn.sent);
            let _ = (&theirs).read(&mut chunk);
        }
        assert!(!conn.has_output() || conn.output.len() < 2 * 1024 * 1024, "nothing moved");
    }

    /// Failure: a field added carelessly raises the cost of every client that's waiting.
    #[test]
    fn a_connection_stays_small() {
        assert!(size_of::<Conn>() <= 320, "{}", size_of::<Conn>());
    }
}
