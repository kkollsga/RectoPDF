//! Permission levels, enforced for real: over a real socket, walking the table, and with the real terminal check.

mod common;

use common::{Raw, State, Word, in_a_terminal, open_cmd};
use recto_agent::table::{Access, Route, TABLE, parse_cli};
use recto_agent::{Level, exit, skill};
use recto_control::levels;
use recto_testpdf::{Page, TempPdf, hidden_text_page, pages, text_lines};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::Stdio;

fn doc() -> TempPdf {
    TempPdf::new(
        "private.pdf",
        &pages(&[Page::new(text_lines(&["Private words on a page"])), Page::new(hidden_text_page())]),
    )
}

/// The line a row's first example sends, with any wait cut short so an allowed one can't hold up the walk.
fn wire_for(spec: &recto_agent::Spec) -> String {
    let words = skill::shell_words(spec.examples[0]);
    let line = parse_cli(&words[1..]).unwrap();
    let mut value: Value = serde_json::from_str(&line.wire()).unwrap();
    // What `recto` reads from stdin for this row, as it puts it in.
    if let Some(field) = recto_agent::table::stdin_secret_field(spec) {
        value[field] = json!("from stdin");
    }
    if value.get("timeout_ms").is_some() {
        value["timeout_ms"] = json!(20);
    }
    if spec.tag == "wait" && value.get("timeout_ms").is_none() {
        value["timeout_ms"] = json!(20);
    }
    if spec.tag == "watch" {
        value["timeout_ms"] = json!(20);
    }
    value.to_string()
}

/// Failure: a command can be reached below its level over the real socket, or is refused above it, or a refusal doesn't say what's missing. Walks the whole table at all four levels, through the same socket a compromised client would use.
#[test]
fn every_row_is_enforced_at_every_level_over_the_socket() {
    let state = State::new();
    let _serving = state.serve();
    let mut raw = Raw::connect(&state);
    let rows: Vec<_> = TABLE
        .iter()
        .filter(|spec| spec.route == Route::Session && !spec.hidden && spec.name != "stop")
        .collect();
    assert!(rows.len() > 25, "{}", rows.len());
    for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
        state.set_level(level);
        for spec in &rows {
            raw.send(&wire_for(spec));
            let mut reply = raw.read().unwrap_or_else(|| panic!("{}: the connection closed at {level:?}", spec.name));
            // A watcher's first line is its welcome, and its answer comes when the short time runs out.
            while reply["data"]["watching"] == true {
                reply = raw.read().unwrap();
            }
            let refused = reply["error"]["exit"] == exit::PERMISSION;
            let allowed = matches!(spec.access, Access::At(needed) if level >= needed);
            assert_eq!(refused, !allowed, "{} at {level:?}: {reply}", spec.name);
            if refused && let Access::At(needed) = spec.access {
                assert_eq!(reply["error"]["code"], "permission_denied", "{reply}");
                let message = reply["error"]["message"].as_str().unwrap();
                assert!(
                    message.contains(needed.label()) && message.contains(level.label()),
                    "it must say what's missing: {message}"
                );
                let ask = reply["error"]["ask"].as_str().unwrap();
                assert_eq!(ask, format!("recto permission request {}", needed.name()));
                assert!(reply["error"]["candidates"].as_array().unwrap().is_empty());
            }
        }
    }
    // `stop` last, since it works.
    state.set_level(Level::ReadOnly);
    assert_eq!(raw.call(&json!({ "cmd": "stop" }))["error"]["code"], "permission_denied");
    assert!(state.paths.socket.exists());
}

/// Failure: page text or a page's picture reaches an agent at Off, the one thing Off exists to stop. And it can't learn what's open: not even how many tabs, or a name in an error.
#[test]
fn nothing_from_a_document_is_available_at_off() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = doc();
    let out = file.0.with_file_name("off.png");
    state.json(&["open", file.0.to_str().unwrap()]);
    state.set_level(Level::Off);
    let attempts: Vec<Vec<&str>> = vec![
        vec!["text", "--page", "1"],
        vec!["render", "--page", "1", "--out", out.to_str().unwrap()],
        vec!["search", "Private"],
        vec!["facts"],
        vec!["contents"],
        vec!["signatures"],
        vec!["tabs"],
        vec!["where"],
        vec!["sidebar"],
        vec!["edits"],
        vec!["budget"],
        vec!["log"],
        vec!["watch", "--timeout", "10ms"],
        vec!["wait", "tab-open", "private", "--timeout", "10ms"],
        vec!["where", "--tab", "private"],
    ];
    for attempt in attempts {
        let (reply, code) = state.json(&attempt);
        assert_eq!(
            (reply["error"]["code"].as_str(), code),
            (Some("permission_denied"), exit::PERMISSION),
            "{attempt:?}: {reply}"
        );
        let text = reply.to_string();
        assert!(!text.contains("Private") && !text.contains("private.pdf"), "{attempt:?} leaked: {text}");
    }
    assert!(!out.exists(), "a picture was drawn at Off");
    let (status, code) = state.json(&["status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["level"], "off");
    // Failure: an agent at Off can't tell whether asking for access is worthwhile. A count is all it gets: no name of a PDF, in JSON or plain words.
    assert_eq!(status["data"]["tabs"], 1, "{status}");
    assert!(
        !status.to_string().contains("private.pdf") && !status.to_string().contains("Private"),
        "{status}"
    );
    let (shown, _, _) = state.text(&["status"]);
    assert!(shown.contains("Open PDFs: 1") && !shown.contains("private.pdf"), "{shown}");
    let (raised, _) = state.json(&["permission", "show"]);
    assert_eq!(raised["data"]["level"], "off");
}

/// Failure: lowering the level doesn't apply until a restart, so an agent keeps access the user took away.
#[test]
fn lowering_applies_to_the_next_command_on_a_connection_already_open() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = doc();
    let mut raw = Raw::connect(&state);
    assert_eq!(raw.call(&open_cmd(&file.0))["ok"], true);
    let text = json!({ "cmd": "text", "page": 1 });
    assert_eq!(raw.call(&text)["ok"], true);
    state.set_level(Level::Off);
    assert_eq!(
        raw.call(&text)["error"]["code"],
        "permission_denied",
        "the same connection must lose access at once"
    );
    state.set_level(Level::ReadOnly);
    assert_eq!(raw.call(&text)["ok"], true);
    assert_eq!(raw.call(&json!({ "cmd": "close" }))["error"]["code"], "permission_denied");
}

/// Failure: a compromised client finds a way to raise its own level: a command that writes it, a field that claims it, a variable or a flag. None may work, and the level's file must not change.
#[test]
fn nothing_an_agent_can_send_or_set_raises_a_level() {
    let state = State::new();
    let _serving = state.serve();
    let file = doc();
    let mut raw = Raw::connect(&state);
    let attempts = [
        json!({ "cmd": "permission_set", "level": "full" }),
        json!({ "cmd": "set_level", "level": "full" }),
        json!({ "cmd": "text", "page": 1, "level": "full" }),
        json!({ "cmd": "tabs", "as_user": true }),
        json!({ "cmd": "level_changed", "level": "full" }),
        json!({ "cmd": "permission_request", "level": "full", "granted": true }),
    ];
    for attempt in &attempts {
        let reply = raw.call(attempt);
        assert_eq!(reply["ok"], false, "{attempt}: {reply}");
        assert_eq!(reply["error"]["code"], "bad_command", "{attempt}: {reply}");
    }
    // Asking is allowed. It grants nothing.
    let asked = raw.call(&json!({ "cmd": "permission_request", "level": "full" }));
    assert_eq!((&asked["data"]["granted"], &asked["data"]["asked"]), (&json!(false), &json!(true)), "{asked}");
    // The message that says the level changed carries nothing and changes nothing.
    assert_eq!(raw.call(&json!({ "cmd": "level_changed" }))["data"]["level"], "off");
    assert_eq!(levels::current(&state.paths), Level::Off);
    assert!(!state.paths.level.exists(), "the server must never write the level");

    for (name, value) in [
        ("RECTO_LEVEL", "full"),
        ("RECTO_PERMISSION", "full"),
        ("RECTO_AGENT_LEVEL", "control"),
        ("RECTO_ALLOW", "1"),
    ] {
        let output = state.command().env(name, value).args(["text", "--page", "1", "--json"]).output().unwrap();
        assert_eq!(output.status.code(), Some(exit::PERMISSION), "{name}");
    }
    for flag in [
        &["tabs", "--level", "full"][..],
        &["--level", "full", "tabs"][..],
        &["tabs", "--force"][..],
        &["tabs", "--yes"][..],
    ] {
        let (reply, code) = state.json(flag);
        assert_eq!((reply["error"]["code"].as_str(), code), (Some("usage"), exit::USAGE), "{flag:?}");
    }
    // The in-process route can't be a way round it.
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "text", "--page", "1"]);
    assert_eq!(
        (reply["error"]["code"].as_str(), code),
        (Some("permission_denied"), exit::PERMISSION),
        "{reply}"
    );
    let mut session = state.command().arg("session").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    writeln!(session.stdin.take().unwrap(), "{}", open_cmd(&file.0)).unwrap();
    let mut said = String::new();
    session.stdout.take().unwrap().read_to_string(&mut said).unwrap();
    session.wait().unwrap();
    assert!(said.contains("permission_denied"), "{said}");
    assert!(!said.contains("Private"), "{said}");
}

/// Failure: an agent with Read-only opens any file on disk through the in-process route, which is what Control is for.
#[test]
fn the_in_process_route_has_the_same_levels() {
    let state = State::at(Level::ReadOnly);
    let file = doc();
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "text", "--page", "1"]);
    assert_eq!(
        (reply["error"]["code"].as_str(), code),
        (Some("permission_denied"), exit::PERMISSION),
        "opening needs Control: {reply}"
    );
    state.set_level(Level::Control);
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "text", "--page", "1"]);
    assert_eq!(code, 0, "{reply}");
    let out = file.0.with_file_name("local.png");
    let (reply, code) = state.json(&["local", file.0.to_str().unwrap(), "render", "--page", "1", "--out", out.to_str().unwrap()]);
    assert_eq!(
        (reply["error"]["code"].as_str(), code),
        (Some("permission_denied"), exit::PERMISSION),
        "{reply}"
    );
    let (reply, code) = state.json(&["budget", "tabs", "--file", file.0.to_str().unwrap(), "--count", "2", "--in-process"]);
    assert_eq!(code, 0, "{reply}");
    state.set_level(Level::ReadOnly);
    let (reply, code) = state.json(&["budget", "tabs", "--file", file.0.to_str().unwrap(), "--count", "2", "--in-process"]);
    assert_eq!(
        (reply["error"]["code"].as_str(), code),
        (Some("permission_denied"), exit::PERMISSION),
        "{reply}"
    );
}

/// Failure: choosing a worker's limits slips through at a lower level, which decides what a hostile file may use.
#[test]
fn choosing_a_workers_limits_needs_full_over_the_socket() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc();
    state.json(&["open", file.0.to_str().unwrap()]);
    let (reply, code) = state.json(&["budget", "render", "--count", "1", "--limit-mib", "4096"]);
    assert_eq!(
        (reply["error"]["code"].as_str(), code),
        (Some("permission_denied"), exit::PERMISSION),
        "{reply}"
    );
    assert_eq!(reply["error"]["ask"], "recto permission request full");
    let (reply, code) = state.json(&["budget", "render", "--count", "1"]);
    assert_eq!(code, 0, "{reply}");
}

/// Failure: an agent raises the level through a plain pipe. With no real terminal it must be refused and nothing change. Lowering takes nothing, since anyone may make things safer.
#[test]
fn raising_needs_a_real_terminal_and_lowering_doesnt() {
    let state = State::at(Level::ReadOnly);
    let mut child = state
        .command()
        .args(["permission", "set", "full", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "yes").unwrap();
    let output = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (reply["error"]["code"].as_str(), output.status.code()),
        (Some("needs_terminal"), Some(exit::PERMISSION)),
        "{reply}"
    );
    assert!(reply["error"]["next"].as_str().unwrap().contains("recto permission set full"));
    assert_eq!(levels::current(&state.paths), Level::ReadOnly, "the level moved with no terminal");

    let (lowered, code) = state.json(&["permission", "set", "off"]);
    assert_eq!(code, 0, "{lowered}");
    assert_eq!((&lowered["data"]["level"], &lowered["data"]["was"]), (&json!("off"), &json!("read_only")));
    assert_eq!(levels::current(&state.paths), Level::Off);
}

/// Failure: the real path doesn't work at all, or takes any answer. On a real terminal the prompt shows a word, the right word raises the level, and a wrong one changes nothing. This is the user's own route, tested on a real pseudo-terminal.
#[test]
fn at_a_real_terminal_the_right_word_raises_and_a_wrong_one_doesnt() {
    let state = State::new();
    state.paths.prepare().unwrap();
    let (_, output) = in_a_terminal(&state, &["permission", "set", "control", "--json"], None, Word::Wrong);
    assert!(output.contains("Give agents Control access"), "{output}");
    assert_eq!(levels::current(&state.paths), Level::Off, "a wrong word raised it: {output}");
    assert!(output.contains("not_confirmed"), "{output}");

    let (code, output) = in_a_terminal(&state, &["permission", "set", "control", "--json"], None, Word::Right);
    assert_eq!(levels::current(&state.paths), Level::Control, "the right word didn't raise it: {output}");
    assert_eq!(code, 0, "{output}");
    assert!(state.log_text().contains("permission_set"), "raising it must be logged");
}

/// Failure: someone at a real terminal raises the level and a running session misses it until the next command, so an agent blocked on asking waits for nothing.
#[test]
fn a_waiting_request_wakes_when_the_user_says_yes() {
    let state = State::new();
    let _serving = state.serve();
    let mut raw = Raw::connect(&state);
    raw.send(&json!({ "cmd": "permission_request", "level": "control", "timeout_ms": 20_000 }).to_string());
    // The welcome line is the answer here. Give it nothing yet, then raise as `permission set` would, with its message.
    std::thread::sleep(std::time::Duration::from_millis(50));
    state.set_level(Level::Control);
    let mut poke = Raw::connect(&state);
    assert_eq!(poke.call(&json!({ "cmd": "level_changed" }))["data"]["level"], "control");
    let answer = raw.read().unwrap();
    assert_eq!(answer["data"]["granted"], true, "{answer}");

    // And it times out plainly, with exit 7 through the real client.
    let state = State::new();
    let _serving = state.serve();
    let (reply, code) = state.json(&["permission", "request", "full", "--timeout", "50ms"]);
    assert_eq!((reply["error"]["code"].as_str(), code), (Some("timed_out"), exit::TIMED_OUT), "{reply}");
    assert!(reply["error"]["next"].as_str().unwrap().contains("recto permission set full"), "{reply}");
}

/// Failure: asking for access with no session running fails rather than saying what to do, or `permission show` needs a session to say what the level is.
#[test]
fn asking_and_showing_work_with_nothing_running() {
    let state = State::new();
    let (reply, code) = state.json(&["permission", "request", "read_only"]);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply["data"]["command"], "recto permission set read_only");
    let (shown, code) = state.json(&["permission", "show"]);
    assert_eq!((code, &shown["data"]["level"]), (0, &json!("off")));
    assert_eq!(shown["data"]["levels"].as_array().unwrap().len(), 4);
    assert!(shown["data"]["how_to_change"].as_str().unwrap().contains("the app's settings"), "{shown}");
    let (text, _, code) = state.text(&["permission", "show"]);
    assert_eq!(code, 0);
    assert!(text.contains("Read-only") && text.contains("own terminal"), "{text}");
}
