//! What the session costs, in its own binary so other tests' servers can't disturb a count. The numbers on a release build are in `docs/ENGINEERING.md`.

mod common;

use common::{Raw, State};
use recto_agent::Level;
use recto_platform::local::process_counters;
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::{Value, json};
use std::time::Duration;

fn footprint(raw: &mut Raw) -> u64 {
    raw.call(&json!({ "cmd": "budget" }))["data"]["footprint_bytes"].as_u64().unwrap()
}

/// Failure: an idle server wakes up, ticks or holds a thread. **The budget is one thread, asleep:** no timer, no thread per client or tab, and no CPU while nothing happens. Asserted by what the kernel counted over a second.
#[test]
fn an_idle_server_is_one_sleeping_thread() {
    let state = State::at(Level::ReadOnly);
    let serving = state.serve();
    {
        let mut raw = Raw::connect(&state);
        assert_eq!(raw.call(&json!({ "cmd": "status" }))["ok"], true);
    }
    // Let it finish with that client and go idle, with nothing left to do.
    std::thread::sleep(Duration::from_millis(100));
    let before = process_counters(serving.pid()).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let after = process_counters(serving.pid()).unwrap();
    println!("idle over 1 s: {before:?} to {after:?}");
    assert_eq!(after.threads, 1, "an idle server should have one thread");
    assert!(after.context_switches - before.context_switches <= 2, "it woke up {} times", after.context_switches - before.context_switches);
    assert!(after.wakeups - before.wakeups <= 2, "{} wake-ups", after.wakeups - before.wakeups);
    assert!(after.cpu_ns - before.cpu_ns < 1_000_000, "{} ns of CPU", after.cpu_ns - before.cpu_ns);
}

/// Failure: waiting clients cost threads or wake the server. Fifty blocked `wait`s and `watch`es are still one thread, and quiet.
#[test]
fn waiting_clients_cost_no_thread_and_no_wake_ups() {
    let state = State::at(Level::ReadOnly);
    let serving = state.serve();
    let mut clients: Vec<Raw> = (0..50).map(|_| Raw::connect(&state)).collect();
    for (index, client) in clients.iter_mut().enumerate() {
        if index % 2 == 0 {
            client.send(&json!({ "cmd": "watch", "timeout_ms": 3_600_000 }).to_string());
            assert_eq!(client.read().unwrap()["data"]["watching"], true);
        } else {
            client.send(&json!({ "cmd": "wait", "condition": "tab_open", "tab": "never", "timeout_ms": 3_600_000 }).to_string());
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    let before = process_counters(serving.pid()).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let after = process_counters(serving.pid()).unwrap();
    println!("50 waiting clients over 1 s: {before:?} to {after:?}");
    assert_eq!(after.threads, 1, "waiting must not hold a thread");
    assert!(after.context_switches - before.context_switches <= 2, "{}", after.context_switches - before.context_switches);
    drop(clients);
}

/// Failure: a tab, or an idle client, costs more than the budget through the session. A cold tab costs at most 4 KiB, with no handle and no thread. Measured over the real socket with a small PDF, because a PDF's size mustn't matter.
#[test]
fn a_thousand_tabs_through_the_socket_stay_cheap() {
    let state = State::at(Level::Control);
    let serving = state.serve();
    let file = TempPdf::new("many.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let dir = file.0.parent().unwrap().join("links");
    std::fs::create_dir_all(&dir).unwrap();
    let mut raw = Raw::connect(&state);
    let open_links = |raw: &mut Raw, from: usize, to: usize| {
        for index in from..to {
            let link = dir.join(format!("tab{index}.pdf"));
            std::os::unix::fs::symlink(&file.0, &link).unwrap();
            let reply: Value = raw.call(&json!({ "cmd": "open", "path": link }));
            assert_eq!(reply["ok"], true, "{reply}");
        }
    };
    open_links(&mut raw, 0, 2);
    let start = footprint(&mut raw);
    let start_handles = raw.call(&json!({ "cmd": "budget" }))["data"]["open_handles"].as_u64().unwrap();
    open_links(&mut raw, 2, 1000);
    let end = footprint(&mut raw);
    let end_handles = raw.call(&json!({ "cmd": "budget" }))["data"]["open_handles"].as_u64().unwrap();
    let per_tab = end.saturating_sub(start) / 998;
    println!("footprint: 2 tabs {start} B, 1000 tabs {end} B, {per_tab} B a tab, handles {start_handles} to {end_handles}");
    assert!(per_tab <= 4096, "a tab costs {per_tab} B");
    assert_eq!(end_handles, start_handles, "tabs must hold no handles");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(process_counters(serving.pid()).unwrap().threads, 1, "tabs must hold no threads");
    drop(raw);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure: the memory of a long session keeps growing, whether from connections, log writes or answers. Many rounds of connect, read and hang up must end near where they started.
#[test]
fn many_connections_leave_nothing_behind() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("few.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    let mut raw = Raw::connect(&state);
    assert_eq!(raw.call(&json!({ "cmd": "open", "path": file.0 }))["ok"], true);
    let round = || {
        for _ in 0..500 {
            let mut client = Raw::connect(&state);
            assert_eq!(client.call(&json!({ "cmd": "tabs" }))["ok"], true);
        }
    };
    round();
    let start = footprint(&mut raw);
    for _ in 0..4 {
        round();
    }
    let end = footprint(&mut raw);
    println!("footprint after 500 clients {start} B, after 2,500 {end} B");
    assert!(end.saturating_sub(start) < 1024 * 1024, "grew {} B", end.saturating_sub(start));
}

/// Failure: a command leaves its worker or a thread behind, so the session stops being idle once anything's been drawn. After drawing it must go back to one thread and no children, by itself.
#[test]
fn after_drawing_nothing_is_left_running() {
    let state = State::at(Level::Control);
    let serving = state.serve();
    let file = TempPdf::new("drawn.pdf", &pages(&[Page::new(text_lines(&["Words to draw"]))]));
    let mut raw = Raw::connect(&state);
    assert_eq!(raw.call(&json!({ "cmd": "open", "path": file.0 }))["ok"], true);
    assert_eq!(raw.call(&json!({ "cmd": "text", "page": 1 }))["ok"], true);
    let children = || {
        let output = std::process::Command::new("/usr/bin/pgrep").args(["-P", &serving.pid().to_string()]).output().unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let started = std::time::Instant::now();
    while process_counters(serving.pid()).unwrap().threads != 1 || !children().is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "threads {:?}, children {:?}", process_counters(serving.pid()), children());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Failure: out of descriptors, accepting fails and the session spins at 100%, or doesn't recover. On macOS a failed accept drops the waiting connection, so there's nothing to spin on, and those clients see the connection end. This holds that if it ever changes.
#[test]
fn out_of_descriptors_the_session_doesnt_spin() {
    let state = State::at(Level::ReadOnly);
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("ulimit -n 32; exec '{}' serve --json", common::bin())])
        .env(recto_control::state::STATE_ENV, &state.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Ended with the test, even if it fails: the `exec` makes this the session itself.
    let mut serving = common::Serving { child };
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(serving.child.stdout.take().unwrap()), &mut line).unwrap();
    assert!(line.contains("\"ok\":true"), "{line}");
    let pid = serving.pid();
    // More than it has descriptors for.
    let mut held: Vec<std::os::unix::net::UnixStream> = (0..60).map(|_| std::os::unix::net::UnixStream::connect(state.socket()).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(100));
    let before = process_counters(pid).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let after = process_counters(pid).unwrap();
    let cpu = after.cpu_ns - before.cpu_ns;
    println!("out of descriptors, over 500 ms: {} ms of CPU, {} context switches", cpu / 1_000_000, after.context_switches - before.context_switches);
    assert!(cpu < 100_000_000, "{} ms of CPU in 500 ms: it's spinning", cpu / 1_000_000);
    held.clear();
    // The session finds out they've gone as it next reads them, so the very next connection can still be one too many.
    let started = std::time::Instant::now();
    loop {
        let mut raw = Raw::connect(&state);
        raw.send(&json!({ "cmd": "status" }).to_string());
        if raw.read().is_some_and(|reply| reply["ok"] == true) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "with descriptors free again, it must take connections again");
    }
    serving.kill();
}
