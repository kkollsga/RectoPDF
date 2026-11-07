//! The socket itself, attacked and broken: stale files, dead clients, a killed server, hostile bytes and the real home.

mod common;

use common::{Raw, Serving, State, Word, in_a_terminal, open_cmd};
use recto_agent::{Level, exit};
use recto_control::state::STATE_ENV;
use recto_testpdf::{Page, TempPdf, busy_page, pages, text_lines};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn doc() -> TempPdf {
    TempPdf::new("doc.pdf", &pages(&[Page::new(text_lines(&["Words on page one"])), Page::new(text_lines(&["Words on page two"]))]))
}

/// Failure: nothing's running and it reads as a hang or a vague failure, rather than saying so, with its own exit code and the command to start it.
#[test]
fn nothing_running_says_so_with_the_command_to_start_it() {
    let state = State::new();
    let (reply, code) = state.json(&["status"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("not_running"), exit::NOT_RUNNING));
    assert!(reply["error"]["next"].as_str().unwrap().contains("recto serve"), "{reply}");
    let (_, said, code) = state.text(&["tabs"]);
    assert_eq!(code, exit::NOT_RUNNING);
    assert!(said.contains("isn't running") && said.contains("recto serve"), "{said}");
    assert!(!state.root.exists(), "just asking must not create state");
}

/// Failure: the socket is reachable by other users or over the network. It must be 0600 in a 0700 folder, with no TCP socket in the process at all.
#[test]
fn the_socket_is_private_and_there_is_no_network_listener() {
    let state = State::at(Level::Control);
    let serving = state.serve();
    let meta = std::fs::symlink_metadata(state.socket()).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&state.paths.run_dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&state.root).unwrap().permissions().mode() & 0o777, 0o700);
    let lsof = std::process::Command::new("/usr/sbin/lsof").args(["-nP", "-a", "-p", &serving.pid().to_string(), "-i"]).output().unwrap();
    assert!(String::from_utf8_lossy(&lsof.stdout).trim().is_empty(), "a network socket: {}", String::from_utf8_lossy(&lsof.stdout));
    for source in [include_str!("../../recto-control/src/server.rs"), include_str!("../../recto-control/src/client.rs"), include_str!("../../recto-platform/src/local_mac.rs")] {
        assert!(!source.contains("TcpListener") && !source.contains("TcpStream") && !source.contains("std::net::"), "a route to the network in the source");
    }
}

/// Failure: a server killed outright leaves a socket that blocks the next start, or a client takes the leftover for a live session. It must read as not running, and start again cleanly.
#[test]
fn a_stale_socket_reads_as_not_running_and_doesnt_block_a_restart() {
    let state = State::at(Level::Control);
    let mut serving = state.serve();
    let file = doc();
    state.json(&["open", file.0.to_str().unwrap()]);
    serving.kill();
    assert!(state.socket().exists(), "the kill should have left the socket behind");
    let (reply, code) = state.json(&["status"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("not_running"), exit::NOT_RUNNING), "{reply}");
    let _again = state.serve();
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let (tabs, _) = state.json(&["tabs"]);
    assert_eq!(tabs["data"]["count"], 0, "a new session starts empty, and must say so rather than pretending");
}

/// Failure: a server killed in the middle of a command leaves a half-written picture under the real name, a stuck worker, a damaged log, or a client that hangs. It must all be cleanly recoverable.
#[test]
fn a_server_killed_mid_command_recovers_on_restart() {
    let state = State::at(Level::Full);
    let mut serving = state.serve();
    let content = busy_page();
    let many: Vec<Page> = (0..30).map(|_| Page { content: content.clone(), page_extra: String::new(), media: (1400.0, 1800.0) }).collect();
    let file = TempPdf::new("heavy.pdf", &pages(&many));
    state.json(&["open", file.0.to_str().unwrap()]);
    let out = file.0.with_file_name("killed.png");
    // A search across everything keeps a job and a worker busy, and the render beside it has a temp file open.
    let mut search = Raw::connect(&state);
    search.send(&json!({ "cmd": "search", "query": "no such words", "limit": 200 }).to_string());
    let mut render = Raw::connect(&state);
    render.send(&json!({ "cmd": "render", "page": 2, "scale": 4.0, "out": out }).to_string());
    std::thread::sleep(Duration::from_millis(20));
    serving.kill();
    assert!(search.read().is_none(), "a client on a dead session must see it end, not hang");
    assert!(render.read().is_none());
    assert!(!out.exists(), "a half-written picture under the real name");
    let _again = state.serve();
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let (log, code) = state.json(&["log"]);
    assert_eq!(code, 0, "the log must still read after a kill: {log}");
    assert!(log["data"]["entries"].as_array().unwrap().iter().any(|entry| entry["cmd"] == "open"));
    // The worker notices its host has gone, by itself. Nothing should need cleaning up by hand.
    let (open, code) = state.json(&["open", file.0.to_str().unwrap()]);
    assert_eq!(code, 0, "{open}");
    let (text, code) = state.json(&["text", "--page", "1"]);
    assert_eq!(code, 0, "{text}");
    for entry in std::fs::read_dir(file.0.parent().unwrap()).unwrap() {
        let name = entry.unwrap().file_name();
        if name.to_string_lossy().ends_with(".partial") {
            let _ = std::fs::remove_file(file.0.with_file_name(name));
        }
    }
}

/// Failure: something planted at the socket's path is deleted or connected to, or a link there is followed. Only a socket of ours may be cleared.
#[test]
fn what_isnt_our_socket_is_left_alone() {
    let state = State::at(Level::Off);
    state.paths.prepare().unwrap();
    std::fs::write(state.socket(), b"someone's file").unwrap();
    let output = state.command().args(["serve", "--json"]).stdout(Stdio::piped()).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "socket_refused", "{reply}");
    assert_eq!(std::fs::read(state.socket()).unwrap(), b"someone's file");
    let (status, code) = state.json(&["status"]);
    assert_eq!((status["error"]["code"].as_str(), code), (Some("socket_refused"), exit::PERMISSION), "a client must not trust it: {status}");
    std::fs::remove_file(state.socket()).unwrap();
    let target = state.root.join("target");
    std::os::unix::fs::symlink(&target, state.socket()).unwrap();
    let output = state.command().args(["serve", "--json"]).stdout(Stdio::piped()).output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "socket_refused", "{reply}");
    assert!(!target.exists());
}

/// Failure: two sessions run on one state folder, or a second start takes over the first's socket, so tabs vanish.
#[test]
fn a_second_session_is_refused_and_background_is_idempotent() {
    let state = State::at(Level::Control);
    let serving = state.serve();
    let (reply, code) = state.json(&["serve"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("already_running"), exit::FAILED), "{reply}");
    let (reply, code) = state.json(&["serve", "--background"]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!((&reply["data"]["already"], &reply["data"]["pid"]), (&json!(true), &json!(serving.pid())));
    let (status, _) = state.json(&["status"]);
    assert_eq!(status["data"]["pid"], serving.pid(), "the first session must be the one still answering");
}

/// Failure: `serve --background` returns before the session answers, or it dies with the shell that started it. It must be there at once, and `stop` must end it and remove its socket.
#[test]
fn background_serve_is_ready_on_return_and_stops_cleanly() {
    let state = State::at(Level::Control);
    let (reply, code) = state.json(&["serve", "--background"]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["data"]["already"], false);
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "it must answer as soon as serve returns: {status}");
    assert_eq!(status["data"]["pid"], reply["data"]["pid"]);
    let (stopped, code) = state.json(&["stop"]);
    assert_eq!((stopped["data"]["stopping"].as_bool(), code), (Some(true), 0), "{stopped}");
    wait_gone(reply["data"]["pid"].as_u64().unwrap());
    assert!(!state.socket().exists(), "stop must remove the socket");
    let (reply, code) = state.json(&["status"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("not_running"), exit::NOT_RUNNING));
}

fn wait_gone(pid: u64) {
    let started = Instant::now();
    // This is the test watching a process it can't wait on, because it isn't its child.
    while recto_platform::local::process_counters(u32::try_from(pid).unwrap()).is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "it's still running");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Failure: clients interfere: one's answer reaches another, tabs are lost, or the server falls over. Many real clients at once, a mix of reads and changes.
#[test]
fn many_clients_at_once_each_get_their_own_answer() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc();
    let dir = file.0.parent().unwrap();
    let links: Vec<_> = (0..12).map(|i| {
        let link = dir.join(format!("link{i}.pdf"));
        std::os::unix::fs::symlink(&file.0, &link).unwrap();
        link
    }).collect();
    std::thread::scope(|scope| {
        for (index, link) in links.iter().enumerate() {
            let state = &state;
            scope.spawn(move || {
                let (opened, code) = state.json(&["open", link.to_str().unwrap()]);
                assert_eq!(code, 0, "{opened}");
                let tab = opened["data"]["tab"].as_u64().unwrap();
                assert_eq!(opened["data"]["untrusted"]["name"], format!("link{index}"));
                let (status, code) = state.json(&["status"]);
                assert_eq!((code, &status["cmd"]), (0, &json!("status")));
                let (text, code) = state.json(&["text", "--page", "2", "--tab", &format!("#{tab}")]);
                assert_eq!(code, 0, "{text}");
                assert!(text["data"]["untrusted"]["text"].as_str().unwrap().contains("page two"), "{text}");
                let (tabs, code) = state.json(&["tabs"]);
                assert_eq!((code, &tabs["cmd"]), (0, &json!("tabs")));
            });
        }
    });
    let (tabs, _) = state.json(&["tabs"]);
    assert_eq!(tabs["data"]["count"], 12, "{tabs}");
    for link in links {
        std::fs::remove_file(link).unwrap();
    }
}

/// Failure: a client that dies half way through a request, or right after sending one, takes the server or its slot with it.
#[test]
fn a_client_dying_mid_request_costs_nothing() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = doc();
    let out = file.0.with_file_name("orphan.png");
    state.json(&["open", file.0.to_str().unwrap()]);
    // Half a line, then gone.
    {
        let mut half = UnixStream::connect(state.socket()).unwrap();
        half.write_all(br#"{"cmd":"te"#).unwrap();
    }
    // A whole request, and gone before the answer. The first call makes sure it's been accepted, because a client that's gone before that has said nothing: the peer check can't pass.
    {
        let mut gone = Raw::connect(&state);
        assert_eq!(gone.call(&json!({ "cmd": "status" }))["ok"], true);
        gone.send(&json!({ "cmd": "render", "page": 1, "out": out }).to_string());
    }
    // A wait, then gone. The session must find out without waiting for the deadline.
    {
        let mut waiting = Raw::connect(&state);
        waiting.send(&json!({ "cmd": "wait", "condition": "page_drawn", "page": 2, "timeout_ms": 3_600_000 }).to_string());
        let mut watching = Raw::connect(&state);
        watching.send(&json!({ "cmd": "watch", "timeout_ms": 3_600_000 }).to_string());
        assert_eq!(watching.read().unwrap()["data"]["watching"], true);
    }
    // The orphaned job still finishes, and it's logged.
    let mut drawn = Raw::connect(&state);
    let reply = drawn.call(&json!({ "cmd": "wait", "condition": "page_drawn", "page": 1, "timeout_ms": 30_000 }));
    assert_eq!(reply["data"]["drawn"], true, "{reply}");
    assert!(out.exists());
    std::fs::remove_file(&out).unwrap();
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert!(state.log_text().contains("\"cmd\":\"render\""), "the action was taken, so it's logged");
}

/// Failure: hostile bytes crash the server or get a command through. Each must be refused, and the server left answering. Unknown fields are refused, and so is deep nesting and bad UTF-8.
#[test]
fn hostile_messages_are_refused_and_nothing_falls_over() {
    let state = State::at(Level::Full);
    let serving = state.serve();
    // Deep enough to overflow a stack if anything recursed without a limit, and still under the line cap.
    let deep = format!("{}{}", "[".repeat(30_000), "]".repeat(30_000));
    let lines: Vec<Vec<u8>> = vec![
        b"not json".to_vec(),
        b"{".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"42".to_vec(),
        b"\"text\"".to_vec(),
        b"{}".to_vec(),
        b"{\"cmd\":null}".to_vec(),
        b"{\"cmd\":\"text\"}".to_vec(),
        b"{\"cmd\":\"text\",\"page\":\"one\"}".to_vec(),
        b"{\"cmd\":\"text\",\"page\":-1}".to_vec(),
        b"{\"cmd\":\"text\",\"page\":1e99}".to_vec(),
        b"{\"cmd\":\"text\",\"page\":1,\"page\":2}".to_vec(),
        b"{\"cmd\":\"tabs\",\"extra\":1}".to_vec(),
        b"{\"cmd\":\"open\",\"path\":12}".to_vec(),
        b"{\"cmd\":\"\xff\xfe\"}".to_vec(),
        b"\xff\xfe\xfd\xfc".to_vec(),
        b"\x00\x00\x00\x00".to_vec(),
        deep.into_bytes(),
    ];
    let mut raw = Raw::connect(&state);
    for line in lines {
        let mut framed = line.clone();
        framed.push(b'\n');
        raw.stream.write_all(&framed).unwrap();
        let reply = raw.read().unwrap_or_else(|| panic!("the connection closed on {:?}", String::from_utf8_lossy(&line)));
        assert_eq!(reply["error"]["code"], "bad_command", "{:?}: {reply}", String::from_utf8_lossy(&line));
    }
    // Numbers that are in range but absurd: the session must refuse them, not panic on arithmetic.
    let file = doc();
    assert_eq!(raw.call(&open_cmd(&file.0))["ok"], true);
    for (command, code) in [
        (json!({ "cmd": "text", "page": 0 }), "no_such_page"),
        (json!({ "cmd": "text", "page": u32::MAX }), "no_such_page"),
        (json!({ "cmd": "goto", "page": u32::MAX }), "no_such_page"),
        (json!({ "cmd": "page_move", "pages": "4294967295-4294967295", "to": u32::MAX }), "page_out_of_range"),
        (json!({ "cmd": "page_delete", "pages": "1-4294967295" }), "page_out_of_range"),
        (json!({ "cmd": "page_rotate", "pages": "1", "degrees": i32::MAX }), "bad_rotation"),
        (json!({ "cmd": "page_rotate", "pages": "1", "degrees": i32::MIN }), "bad_rotation"),
        (json!({ "cmd": "page_insert", "path": "/no/such.pdf", "at": u32::MAX }), "not_found"),
        (json!({ "cmd": "render", "page": 1, "scale": 1e30, "out": "/tmp/x.png" }), "bad_scale"),
        (json!({ "cmd": "render", "page": 1, "out": "relative.png" }), "bad_path"),
        (json!({ "cmd": "budget_viewport", "viewport": "99999999x99999999" }), "bad_viewport"),
        (json!({ "cmd": "wait", "condition": "page_drawn", "timeout_ms": 1 }), "no_such_page"),
        (json!({ "cmd": "wait", "condition": "indexing_done" }), "not_built"),
        (json!({ "cmd": "watch", "events": ["nonsense"] }), "bad_events"),
        (json!({ "cmd": "search", "query": "x".repeat(5000) }), "bad_query"),
        (json!({ "cmd": "contents", "from": u32::MAX, "limit": u32::MAX, "depth": 255 }), ""),
        (json!({ "cmd": "log", "last": u32::MAX }), ""),
    ] {
        let reply = raw.call(&command);
        if code.is_empty() {
            assert_eq!(reply["ok"], true, "{command}: {reply}");
        } else {
            assert_eq!(reply["error"]["code"], code, "{command}: {reply}");
        }
    }
    // Waits past the maximum are held to it, rather than overflowing the clock.
    raw.send(&json!({ "cmd": "wait", "condition": "tab_open", "tab": "nope", "timeout_ms": u64::MAX }).to_string());
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["pid"], serving.pid(), "it must be the same session, still up");
}

/// Failure: a line with no end grows the buffer without limit. Past the cap it's refused and the connection closed.
#[test]
fn an_endless_line_is_cut_off() {
    let state = State::at(Level::Off);
    let _serving = state.serve();
    let mut raw = Raw::connect(&state);
    let huge = vec![b'x'; 256 * 1024];
    let _ = raw.stream.write_all(&huge);
    let reply = raw.read().unwrap();
    assert_eq!(reply["error"]["code"], "bad_command");
    assert!(reply["error"]["message"].as_str().unwrap().contains("too long"), "{reply}");
    assert!(raw.read().is_none(), "the connection must close");
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
}

/// Failure: a flood of connections holds every slot, and then `status` and the user's own `permission set` get "busy" forever. Past the cap, those still get through, and nothing else does.
#[test]
fn past_the_connection_cap_what_matters_still_gets_through() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let mut held: Vec<Raw> = (0..128).map(|_| Raw::connect(&state)).collect();
    // Each is really taken before the next step.
    assert_eq!(held.last_mut().unwrap().call(&json!({ "cmd": "status" }))["ok"], true);
    // A plain command past the cap is told it's busy, and let go.
    let (reply, code) = state.json(&["tabs"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("busy"), exit::BUSY), "{reply}");
    // What must always get through does.
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    let (shown, code) = state.json(&["permission", "show"]);
    assert_eq!(code, 0, "{shown}");
    // Lowering the level, with every slot taken, is seen at once and acted on.
    let mut held_waiter = held.pop().unwrap();
    held_waiter.send(&json!({ "cmd": "watch" }).to_string());
    assert_eq!(held_waiter.read().unwrap()["data"]["watching"], true);
    let mut full = Raw::connect(&state);
    assert_eq!(full.call(&json!({ "cmd": "status" }))["ok"], true, "the slot it left went to overflow's use");
    let (lowered, code) = state.json(&["permission", "set", "off"]);
    assert_eq!(code, 0, "{lowered}");
    let mut line = held_waiter.read().unwrap();
    if line["data"]["event"] == "level_changed" {
        line = held_waiter.read().unwrap();
    }
    assert_eq!(line["error"]["code"], "permission_denied", "{line}");
    // Closing frees room straight away.
    held.truncate(100);
    assert_eq!(held[0].call(&json!({ "cmd": "status" }))["ok"], true);
    state.set_level(Level::ReadOnly);
    let (tabs, code) = state.json(&["tabs"]);
    assert_eq!(code, 0, "{tabs}");
}

/// Failure: 128 clients each asking with a 24-hour wait take every slot for a day. Waiting clients are capped well below the slots, so everyone else can still work.
#[test]
fn waiting_clients_cant_take_every_slot() {
    let state = State::new();
    let _serving = state.serve();
    let mut waiting: Vec<Raw> = (0..64).map(|_| Raw::connect(&state)).collect();
    for client in &mut waiting {
        client.send(&json!({ "cmd": "permission_request", "level": "full", "timeout_ms": 86_400_000 }).to_string());
    }
    // Asked after they've all been taken: one round trip on each makes sure.
    let mut last = Raw::connect(&state);
    assert_eq!(last.call(&json!({ "cmd": "status" }))["ok"], true);
    let mut refused = last.call(&json!({ "cmd": "permission_request", "level": "full", "timeout_ms": 86_400_000 }));
    // The loop's order can let a few more through as they're read, but never past the cap.
    for _ in 0..8 {
        if refused["error"]["code"] == "busy" {
            break;
        }
        refused = Raw::connect(&state).call(&json!({ "cmd": "permission_request", "level": "full", "timeout_ms": 86_400_000 }));
    }
    assert_eq!((refused["error"]["code"].as_str(), refused["error"]["exit"].as_i64()), (Some("busy"), Some(i64::from(exit::BUSY))), "{refused}");
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    state.set_level(Level::ReadOnly);
    let mut watcher = Raw::connect(&state);
    assert_eq!(watcher.call(&json!({ "cmd": "watch", "count": 1 }))["error"]["code"], "busy");
    assert_eq!(watcher.call(&json!({ "cmd": "wait", "condition": "tab_open", "tab": "x", "timeout_ms": 10_000 }))["error"]["code"], "busy");
    let (tabs, code) = state.json(&["tabs"]);
    assert_eq!(code, 0, "plain work still goes on: {tabs}");
}

/// Failure: a subscribed client writing without pause holds the only thread that does anything. It's dropped the moment it says anything, and everyone else carries on.
#[test]
fn a_subscriber_that_writes_is_dropped() {
    let state = State::at(Level::ReadOnly);
    let _serving = state.serve();
    let mut watcher = Raw::connect(&state);
    watcher.send(&json!({ "cmd": "watch", "timeout_ms": 3_600_000 }).to_string());
    assert_eq!(watcher.read().unwrap()["data"]["watching"], true);
    let _ = watcher.stream.write_all(b"anything at all\n");
    // Reading gives up after a while, so this has to end long before that to mean the session closed it.
    let started = Instant::now();
    assert!(watcher.read().is_none(), "it must be dropped");
    assert!(started.elapsed() < Duration::from_secs(15), "it wasn't closed: the read just timed out");
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
}

/// Failure: with `--timeout 0s`, a wait for something already true times out, because the deadline is looked at before the look runs. Exit 0 for what's true now, and exit 7 for what isn't, either way with no time at all.
#[test]
fn a_wait_with_no_time_still_looks_first() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc();
    state.json(&["open", file.0.to_str().unwrap()]);
    for _ in 0..50 {
        let (reply, code) = state.json(&["wait", "tab-open", "doc", "--timeout", "0s"]);
        assert_eq!(code, 0, "what's already true: {reply}");
    }
    let (reply, code) = state.json(&["wait", "tab-open", "nothing", "--timeout", "0s"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("timed_out"), exit::TIMED_OUT), "{reply}");
}

/// Failure: asking for none of something is answered with one. `watch --count 0` delivers nothing and says so.
#[test]
fn a_watch_for_no_events_ends_at_once() {
    let state = State::at(Level::ReadOnly);
    let _serving = state.serve();
    let (lines, code) = lines_of(watch_process(&state, &["watch", "--count", "0", "--timeout", "30s", "--json"]));
    assert_eq!(code, 0, "{lines:?}");
    assert_eq!((&lines.last().unwrap()["data"]["delivered"], &lines.last().unwrap()["data"]["reason"]), (&json!(0), &json!("count reached")), "{lines:?}");
}

/// Failure: an agent at Off alternating asks writes a line each time, to a file with no cap. The user hears once per level, higher only, and a full or closed stderr can't take the session down with its edits.
#[test]
fn asking_in_a_loop_cant_fill_the_messages_file_or_end_the_session() {
    let state = State::new();
    let (ready, code) = state.json(&["serve", "--background"]);
    assert_eq!(code, 0, "{ready}");
    let mut raw = Raw::connect(&state);
    for round in 0..2000 {
        let level = ["read_only", "control", "full"][round % 3];
        assert_eq!(raw.call(&json!({ "cmd": "permission_request", "level": level }))["ok"], true);
    }
    let messages = std::fs::read_to_string(&state.paths.server_messages).unwrap();
    assert_eq!(messages.lines().count(), 3, "{messages}");
    state.set_level(Level::Control);
    let (stopped, code) = state.json(&["stop"]);
    assert_eq!(code, 0, "{stopped}");

    // A stderr nobody's reading: the first ask writes to a closed pipe, and the session must still be there.
    let state = State::new();
    let child = state.command().args(["serve", "--json"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut serving = Serving { child };
    let stdout = serving.child.stdout.take().unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line).unwrap();
    drop(serving.child.stderr.take());
    let mut raw = Raw::connect(&state);
    assert_eq!(raw.call(&json!({ "cmd": "permission_request", "level": "control" }))["ok"], true);
    assert_eq!(raw.call(&json!({ "cmd": "status" }))["ok"], true, "the session must survive it");
}

/// Failure: a `wait` loop with a 64 KiB tab name holds memory per wait, or one that hung up leaves its looks queued. Long selectors are refused up front.
#[test]
fn a_long_selector_is_refused_up_front() {
    let state = State::at(Level::ReadOnly);
    let _serving = state.serve();
    let mut raw = Raw::connect(&state);
    let long = "x".repeat(32 * 1024);
    let reply = raw.call(&json!({ "cmd": "wait", "condition": "tab_open", "tab": long, "timeout_ms": 0 }));
    assert_eq!(reply["error"]["code"], "no_such_tab", "{reply}");
    let reply = raw.call(&json!({ "cmd": "where", "tab": long }));
    assert_eq!(reply["error"]["code"], "no_such_tab", "{reply}");
}

/// Failure: a client that stops reading a watch makes the session hold its output for ever, and its memory grows.
#[test]
fn a_watcher_that_stops_reading_is_dropped_rather_than_buffered_for() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc();
    let mut watcher = Raw::connect(&state);
    watcher.send(&json!({ "cmd": "watch" }).to_string());
    assert_eq!(watcher.read().unwrap()["data"]["watching"], true);
    let mut raw = Raw::connect(&state);
    assert_eq!(raw.call(&open_cmd(&file.0))["ok"], true);
    // Each goto tells the watcher, which reads nothing. 40,000 of them is over 3 MiB, well past its 1 MiB allowance.
    for sent in 0..40_000u32 {
        let reply = raw.call(&json!({ "cmd": "goto", "page": 1 + sent % 2 }));
        assert_eq!(reply["ok"], true, "{reply}");
    }
    // Reading now gets what fit, and then the connection ends instead of carrying on: its allowance plus what the socket itself holds, and not the rest.
    let mut bytes = 0usize;
    let mut chunk = [0u8; 65536];
    // macOS refuses this on a socket whose peer has gone, which is what's expected here.
    let _ = watcher.stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut ended = false;
    loop {
        match watcher.stream.read(&mut chunk) {
            Ok(0) => {
                ended = true;
                break;
            }
            Err(_) => break,
            Ok(count) => bytes += count,
        }
    }
    assert!(ended, "the session must have dropped it, after {bytes} bytes");
    assert!(bytes < 1024 * 1024 + 512 * 1024, "it should have been dropped at about 1 MiB, and {bytes} bytes came");
}

/// Failure: a test or QA run points at the real home and overwrites the user's level, log or session. Refused with nothing created, whatever `$HOME` or `$TMPDIR` claims.
#[test]
fn the_state_override_refuses_the_real_home_and_home_cant_fool_it() {
    let home = recto_platform::local::real_home().unwrap();
    let fake_home = std::env::temp_dir().canonicalize().unwrap();
    let inside = home.join(format!("recto-should-never-exist-{}", std::process::id()));
    let real_state = home.join("Library/Application Support/RectoPDF/agent");
    for target in [&home, &inside, &real_state] {
        for command in [&["serve", "--json"][..], &["serve", "--background", "--json"][..], &["status", "--json"][..], &["permission", "set", "full", "--json"][..], &["skill", "install", "--path", "/tmp/nope", "--json"][..]] {
            let output = std::process::Command::new(common::bin()).env(STATE_ENV, target).env("HOME", &fake_home).env("TMPDIR", &home).args(command).stdin(Stdio::null()).output().unwrap();
            let reply: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{command:?}: {}", String::from_utf8_lossy(&output.stdout)));
            assert_eq!(reply["error"]["code"], "state_refused", "{target:?} {command:?}: {reply}");
        }
    }
    assert!(!inside.exists(), "something was created inside the real home");
}

/// Failure: `wait` polls, returns early, or misses what happened just before it asked. It must block until the thing happens, then return at once, and see it if it already has.
#[test]
fn wait_blocks_until_it_happens_and_sees_what_already_did() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = doc();
    let path = file.0.to_str().unwrap().to_string();
    let result = std::thread::scope(|scope| {
        let waiter = scope.spawn(|| state.json(&["wait", "tab-open", "doc", "--timeout", "30s"]));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waiter.is_finished(), "it returned before anything happened");
        let started = Instant::now();
        state.json(&["open", &path]);
        let answer = waiter.join().unwrap();
        (answer, started.elapsed())
    });
    let ((reply, code), elapsed) = result;
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["data"]["untrusted"]["name"], "doc");
    assert!(elapsed < Duration::from_secs(15), "it took {elapsed:?} to notice");
    let (again, code) = state.json(&["wait", "tab-open", "doc", "--timeout", "10s"]);
    assert_eq!((code, &again["data"]["tab"]), (0, &json!(1)), "what's already true must return at once");

    let out = file.0.with_file_name("waited.png");
    let answer = std::thread::scope(|scope| {
        let waiter = scope.spawn(|| state.json(&["wait", "page-drawn", "--page", "2", "--timeout", "1m"]));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished());
        let (drawn, code) = state.json(&["render", "--page", "2", "--out", out.to_str().unwrap()]);
        assert_eq!(code, 0, "{drawn}");
        waiter.join().unwrap()
    });
    assert_eq!((answer.1, &answer.0["data"]["drawn"]), (0, &json!(true)), "{}", answer.0);
    std::fs::remove_file(&out).unwrap();

    let closer = std::thread::scope(|scope| {
        let waiter = scope.spawn(|| state.json(&["wait", "tab-closed", "#1", "--timeout", "30s"]));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished());
        state.json(&["close", "--tab", "#1"]);
        waiter.join().unwrap()
    });
    assert_eq!((closer.1, &closer.0["data"]["closed"]), (0, &json!(true)), "{}", closer.0);
}

/// Failure: a page that can't be drawn leaves a waiter waiting, rather than saying so at once. Here lopdf reads the file and PDFium refuses it, which is cheap and real.
#[test]
fn a_page_that_fails_to_draw_ends_the_wait_and_says_so() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let whole = pages(&[Page::new(text_lines(&["Words"]))]);
    let cut = whole.windows(4).position(|window| window == b"xref").unwrap();
    let file = TempPdf::new("no-tail.pdf", &whole[..cut]);
    let (opened, code) = state.json(&["open", file.0.to_str().unwrap()]);
    assert_eq!(code, 0, "{opened}");
    let out = file.0.with_file_name("fails.png");
    let (drawn, code) = state.json(&["render", "--page", "1", "--out", out.to_str().unwrap()]);
    assert_ne!(code, 0, "this fixture must be one PDFium refuses: {drawn}");
    let (reply, code) = state.json(&["wait", "page-drawn", "--page", "1", "--timeout", "30s"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("page_failed"), exit::FAILED), "it must say so at once: {reply}");
    assert!(reply["error"]["message"].as_str().unwrap().contains("couldn't be drawn"), "{reply}");
    assert!(!out.exists());
}

/// Failure: a refused file name tells a waiter the page can't be drawn, or a closed tab's page reads as drawn. One says nothing about the page, and the other's tab isn't there.
#[test]
fn a_refused_output_and_a_closed_tab_arent_a_drawn_page() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = doc();
    state.json(&["open", file.0.to_str().unwrap()]);
    let taken = file.0.with_file_name("taken.png");
    std::fs::write(&taken, b"keep").unwrap();
    let (refused, _) = state.json(&["render", "--page", "1", "--out", taken.to_str().unwrap()]);
    assert_eq!(refused["error"]["code"], "output_exists");
    let (reply, code) = state.json(&["wait", "page-drawn", "--page", "1", "--timeout", "50ms"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("timed_out"), exit::TIMED_OUT), "{reply}");
    std::fs::remove_file(&taken).unwrap();
    let out = file.0.with_file_name("then-closed.png");
    state.json(&["render", "--page", "1", "--out", out.to_str().unwrap()]);
    std::fs::remove_file(&out).unwrap();
    let (drawn, code) = state.json(&["wait", "page-drawn", "--page", "1", "--tab", "#1", "--timeout", "5s"]);
    assert_eq!((code, &drawn["data"]["drawn"]), (0, &json!(true)), "{drawn}");
    state.json(&["close", "--tab", "#1"]);
    let (reply, code) = state.json(&["wait", "page-drawn", "--page", "1", "--tab", "#1", "--timeout", "50ms"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("timed_out"), exit::TIMED_OUT), "{reply}");
}

/// Failure: a wait that can never end runs out its time, or the timeout isn't exit 7.
#[test]
fn a_wait_ends_with_a_fixed_exit_or_a_plain_reason() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let started = Instant::now();
    let (reply, code) = state.json(&["wait", "tab-open", "never", "--timeout", "50ms"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("timed_out"), exit::TIMED_OUT), "{reply}");
    assert!(started.elapsed() < Duration::from_secs(30));
    let (reply, code) = state.json(&["wait", "indexing-done"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("not_built"), exit::FAILED), "it must say so now, not wait: {reply}");
    let file = doc();
    let dir = file.0.parent().unwrap();
    let other = dir.join("other-doc.pdf");
    std::fs::copy(&file.0, &other).unwrap();
    state.json(&["open", file.0.to_str().unwrap()]);
    state.json(&["open", other.to_str().unwrap()]);
    let (reply, code) = state.json(&["wait", "tab-open", "doc", "--timeout", "5s"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("ambiguous"), exit::AMBIGUOUS), "a wait must not guess either: {reply}");
    std::fs::remove_file(&other).unwrap();
    let (reply, code) = state.json(&["wait", "page-drawn", "--timeout", "10ms"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("usage"), exit::USAGE), "{reply}");
}

fn watch_process(state: &State, args: &[&str]) -> std::process::Child {
    state.command().args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap()
}

fn lines_of(mut child: std::process::Child) -> (Vec<Value>, i32) {
    let mut text = String::new();
    child.stdout.take().unwrap().read_to_string(&mut text).unwrap();
    let code = child.wait().unwrap().code().unwrap();
    (text.lines().map(|line| serde_json::from_str(line).unwrap()).collect(), code)
}

/// Failure: `watch` misses events, shows groups it wasn't asked for, or doesn't end when its count or time is up. And it must carry no page text.
#[test]
fn watch_streams_events_and_ends_when_told() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc();
    let mut raw = Raw::connect(&state);
    raw.send(&json!({ "cmd": "watch", "events": ["tabs", "pages"], "count": 2, "timeout_ms": 30_000 }).to_string());
    assert_eq!(raw.read().unwrap()["data"]["watching"], true, "the welcome says it's listening, so nothing's missed");
    state.json(&["open", file.0.to_str().unwrap()]);
    state.json(&["page", "rotate", "1", "--by", "90"]);
    state.json(&["goto", "2"]);
    let names: Vec<String> = (0..2).map(|_| raw.read().unwrap()["data"]["event"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(names, ["tab_opened", "page_changed"], "edits weren't asked for");
    assert_eq!(raw.read().unwrap()["data"]["reason"], "count reached");
    // Through the real client: JSON lines, a clean end, and exit 7 only when a count wasn't met.
    let child = watch_process(&state, &["watch", "--count", "1", "--timeout", "30s", "--json"]);
    let mut poke = Raw::connect(&state);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(poke.call(&json!({ "cmd": "goto", "page": 1 }))["ok"], true);
    let (lines, code) = lines_of(child);
    assert_eq!(code, 0, "{lines:?}");
    assert_eq!(lines[0]["data"]["watching"], true);
    assert_eq!(lines[1]["data"]["event"], "page_changed");
    assert_eq!(lines.last().unwrap()["data"]["done"], true);
    assert!(lines.iter().all(|line| line["v"] == recto_agent::VERSION), "{lines:?}");
    let (lines, code) = lines_of(watch_process(&state, &["watch", "--timeout", "50ms", "--json"]));
    assert_eq!(code, 0, "running out of time with no count is how a watch ends: {lines:?}");
    assert_eq!(lines.last().unwrap()["data"]["reason"], "time reached");
    let (lines, code) = lines_of(watch_process(&state, &["watch", "--count", "5", "--timeout", "50ms", "--json"]));
    assert_eq!(code, exit::TIMED_OUT, "{lines:?}");
}

/// Failure: a watcher keeps getting events after the user took its access away. A lowered level must end it, and the end must say why.
#[test]
fn lowering_the_level_ends_watches_and_waits() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let mut watcher = Raw::connect(&state);
    watcher.send(&json!({ "cmd": "watch" }).to_string());
    assert_eq!(watcher.read().unwrap()["data"]["watching"], true);
    let mut waiter = Raw::connect(&state);
    waiter.send(&json!({ "cmd": "wait", "condition": "tab_open", "tab": "never", "timeout_ms": 3_600_000 }).to_string());
    state.set_level(Level::Off);
    let mut poke = Raw::connect(&state);
    poke.call(&json!({ "cmd": "level_changed" }));
    // A watcher at Read-only or above would hear of the change, and then the refusal ends it. Either way, no more events and an answer that says why.
    let mut line = watcher.read().unwrap();
    if line["data"]["event"] == "level_changed" {
        line = watcher.read().unwrap();
    }
    assert_eq!(line["error"]["code"], "permission_denied", "{line}");
    assert!(watcher.read().is_none(), "it must be over");
    assert_eq!(waiter.read().unwrap()["error"]["code"], "permission_denied");
}

/// Failure: document text or a raw file name reaches the log, every action isn't recorded, or a refusal is left out. The canary is page text and search words, and the file's name has hidden characters and a line break.
#[test]
fn the_log_records_every_action_and_never_document_text() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let canary = "CANARYTEXTFROMAPAGE";
    let hidden: String = "secret".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
    let name = format!("re{hidden}po\nrt.pdf");
    let file = TempPdf::new(&name, &pages(&[Page::new(text_lines(&[canary]))]));
    let (opened, code) = state.json(&["open", file.0.to_str().unwrap()]);
    assert_eq!(code, 0, "{opened}");
    assert_eq!(opened["data"]["untrusted"]["name"], "repo rt", "the tab's name is clean too");
    let (read, code) = state.json(&["text", "--page", "1"]);
    assert_eq!(code, 0);
    assert!(read["data"]["untrusted"]["text"].as_str().unwrap().contains(canary), "the canary must really have been read");
    state.json(&["search", canary]);
    state.json(&["where", "--tab", "repo"]);
    state.json(&["no-such-command"]);
    let mut raw = Raw::connect(&state);
    raw.call(&json!({ "cmd": "not_a_command", "text": canary }));
    state.set_level(Level::Off);
    state.json(&["text", "--page", "1"]);
    state.set_level(Level::ReadOnly);

    let text = state.log_text();
    assert!(!text.contains(canary), "document text in the log");
    assert!(!text.contains("secret") && !text.contains(&hidden), "a hidden name in the log");
    assert!(!text.contains('\u{E0073}'), "a tag character in the log");
    assert!(text.contains("\"untrusted\":{\"file\":\"repo rt.pdf\"}"), "the cleaned name, under `untrusted`: {text}");

    let (log, code) = state.json(&["log", "--last", "100"]);
    assert_eq!(code, 0, "{log}");
    let entries = log["data"]["entries"].as_array().unwrap();
    for entry in entries {
        assert!(entry["t_ms"].as_u64().unwrap() > 0 && entry["time"].as_str().unwrap().ends_with('Z'), "{entry}");
        assert!(entry["cmd"].is_string() && entry["level"].is_string() && entry["outcome"].is_string(), "{entry}");
    }
    let found = |cmd: &str, outcome: &str| entries.iter().any(|entry| entry["cmd"] == cmd && entry["outcome"] == outcome);
    assert!(found("open", "ok") && found("text", "ok") && found("search", "ok"), "{entries:?}");
    assert!(found("text", "permission_denied"), "a refusal must be logged: {entries:?}");
    assert!(found("unknown", "bad_command"), "{entries:?}");
    assert!(entries.iter().any(|entry| entry["cmd"] == "text" && entry["level"] == "off"), "with the level at that moment");
    let (shown, _, code) = state.text(&["log", "--last", "3"]);
    assert_eq!(code, 0);
    assert_eq!(shown.lines().count(), 3, "{shown}");
    let (few, _) = state.json(&["log", "--last", "2"]);
    assert_eq!(few["data"]["entries"].as_array().unwrap().len(), 2);
    let file_meta = std::fs::metadata(&state.paths.log).unwrap();
    assert_eq!(file_meta.permissions().mode() & 0o777, 0o600);
}

/// Failure: an agent at Off erases its own record through the real socket: enough `status` calls and bad lines push out an earlier edit. Far past what would have rotated the old single log.
#[test]
fn a_flood_at_off_cant_erase_an_earlier_edit() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc();
    state.json(&["open", file.0.to_str().unwrap()]);
    state.json(&["page", "rotate", "1", "--by", "90"]);
    state.set_level(Level::Off);
    let mut raw = Raw::connect(&state);
    for _ in 0..20_000 {
        raw.send(r#"{"cmd":"status"}"#);
        raw.send("not json");
        raw.send(r#"{"cmd":"text","page":1}"#);
        for _ in 0..3 {
            assert!(raw.read().is_some());
        }
    }
    let lines = state.log_text().lines().count();
    assert!(lines < 200, "{lines} lines for 60,000 commands");
    state.set_level(Level::ReadOnly);
    let (log, code) = state.json(&["log", "--last", "5000"]);
    assert_eq!(code, 0, "{log}");
    let entries = log["data"]["entries"].as_array().unwrap();
    assert!(entries.iter().any(|entry| entry["cmd"] == "page_rotate" && entry["outcome"] == "ok"), "the edit is gone: {log}");
    let total = |cmd: &str| entries.iter().filter(|entry| entry["cmd"] == cmd).map(|entry| entry["count"].as_u64().unwrap_or(1)).sum::<u64>();
    assert!(total("status") >= 20_000, "{}", total("status"));
    assert!(total("unknown") >= 20_000, "{}", total("unknown"));
    assert!(total("text") >= 20_000, "{}", total("text"));
}

/// Failure: with the session stopped, the same log can't be read, or it can be read at Off.
#[test]
fn the_log_reads_with_no_session_at_the_same_level() {
    let state = State::at(Level::ReadOnly);
    let mut serving: Serving = state.serve();
    state.json(&["tabs"]);
    serving.kill();
    let (log, code) = state.json(&["log"]);
    assert_eq!(code, 0, "{log}");
    assert!(log["data"]["entries"].as_array().unwrap().iter().any(|entry| entry["cmd"] == "tabs"));
    state.set_level(Level::Off);
    let (log, code) = state.json(&["log"]);
    assert_eq!((log["error"]["code"].as_str(), code), (Some("permission_denied"), exit::PERMISSION));
}

/// Failure: an agent writes into the user's agent's skill folder through a pipe, or `--replace` overwrites someone's file with no one at a terminal. Every install takes a real terminal, and `show` stays open at Off.
#[test]
fn skill_install_needs_a_real_terminal_and_show_doesnt() {
    let state = State::new();
    let folder = state.root.with_extension("skill-pipe");
    for args in [&["skill", "install", "--path", folder.to_str().unwrap()][..], &["skill", "install", "--path", folder.to_str().unwrap(), "--dry-run"][..], &["skill", "install", "--path", folder.to_str().unwrap(), "--replace"][..]] {
        let (reply, code) = state.json(args);
        assert_eq!((reply["error"]["code"].as_str(), code), (Some("needs_terminal"), exit::PERMISSION), "{args:?}: {reply}");
        assert!(reply["error"]["next"].as_str().unwrap().contains("recto skill show"), "{reply}");
    }
    assert!(!folder.exists(), "something was written without a terminal");
    // Pointed at the real home by a test state folder, it's refused before it even asks.
    let (reply, code) = state.json(&["skill", "install", "--target", "claude-code"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("state_refused"), exit::PERMISSION), "{reply}");
    // However the path is written.
    let home = recto_platform::local::real_home().unwrap();
    let climbing = format!("/usr/..{}/recto-should-never-exist", home.display());
    let link = state.root.with_extension("home-link");
    std::os::unix::fs::symlink(&home, &link).unwrap();
    let through_link = link.join("recto-should-never-exist");
    for path in [climbing.as_str(), through_link.to_str().unwrap()] {
        let (reply, code) = state.json(&["skill", "install", "--path", path]);
        assert_eq!((reply["error"]["code"].as_str(), code), (Some("state_refused"), exit::PERMISSION), "{path}: {reply}");
    }
    std::fs::remove_file(&link).unwrap();
    let (shown, _, code) = state.text(&["skill", "show"]);
    assert_eq!(code, 0, "an agent at Off must still be able to read it");
    assert!(shown.starts_with("---\nname: recto\n"), "{shown}");
}

/// Failure: `recto skill install` at a real terminal writes outside what was asked, overwrites a changed file without saying so, or a QA run writes into the real home. And its own advice fails with a usage error.
#[test]
fn skill_install_at_a_terminal_writes_only_where_asked() {
    let state = State::new();
    let folder = state.root.with_extension("skill");
    let fake_home = state.root.with_extension("home");
    std::fs::create_dir_all(&fake_home).unwrap();
    let path = folder.to_str().unwrap();
    let (code, shown) = in_a_terminal(&state, &["skill", "install", "--path", path, "--json"], None, Word::Wrong);
    assert_eq!(code, exit::PERMISSION, "{shown}");
    assert!(!folder.exists(), "a wrong word wrote something");
    let (code, shown) = in_a_terminal(&state, &["skill", "install", "--path", path, "--json"], None, Word::Right);
    assert_eq!(code, 0, "{shown}");
    let body = std::fs::read_to_string(folder.join("SKILL.md")).unwrap();
    assert!(body.starts_with("---\nname: recto\n"), "the front matter must come first");
    assert!(folder.join("REFERENCE.md").exists());
    let (show, _, code) = state.text(&["skill", "show"]);
    assert_eq!(code, 0);
    assert_eq!(show.trim_end(), body.trim_end(), "`skill show` and the file must match");

    std::fs::write(folder.join("REFERENCE.md"), "edited by someone").unwrap();
    let (code, shown) = in_a_terminal(&state, &["skill", "install", "--path", path, "--json"], None, Word::Right);
    let reply = common::last_json(&shown);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("skill_changed"), exit::FAILED), "{reply}");
    assert!(reply["error"]["candidates"][0].as_str().unwrap().ends_with("REFERENCE.md"));
    assert_eq!(std::fs::read_to_string(folder.join("REFERENCE.md")).unwrap(), "edited by someone");
    // Its advice must be a line that runs.
    let next = reply["error"]["next"].as_str().unwrap();
    assert!(next.contains("--replace") && next.contains("same"), "{next}");
    let (code, shown) = in_a_terminal(&state, &["skill", "install", "--path", path, "--replace", "--json"], None, Word::Right);
    assert_eq!(code, 0, "{shown}");
    assert_ne!(std::fs::read_to_string(folder.join("REFERENCE.md")).unwrap(), "edited by someone");

    // Claude Code's own place, resolved from a home that's a temp folder here.
    let (code, shown) = in_a_terminal(&state, &["skill", "install", "--target", "claude-code", "--dry-run", "--json"], Some(&fake_home), Word::Right);
    assert_eq!(code, 0, "{shown}");
    assert!(common::last_json(&shown)["data"]["folder"].as_str().unwrap().ends_with(".claude/skills/recto"), "{shown}");
    assert!(!fake_home.join(".claude").exists(), "a dry run wrote something");
    let (code, shown) = in_a_terminal(&state, &["skill", "install", "--target", "claude-code", "--json"], Some(&fake_home), Word::Right);
    assert_eq!(code, 0, "{shown}");
    assert!(fake_home.join(".claude/skills/recto/SKILL.md").exists());
    let (neither, code) = state.json(&["skill", "install"]);
    assert_eq!((neither["error"]["code"].as_str(), code), (Some("usage"), exit::USAGE));
    std::fs::remove_dir_all(&folder).unwrap();
    std::fs::remove_dir_all(&fake_home).unwrap();
}
