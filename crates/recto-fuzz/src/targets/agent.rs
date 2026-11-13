//! What reaches `recto-agent` from outside: JSON lines, command lines, selectors, page specs, the level file's bytes. Run for real, on a real session, at a level the bytes choose, with a loader that touches no disk.

use crate::guard::{Budget, within};
use crate::input::Take;
use crate::targets::{NamedLoader, facts_of};
use recto_agent::table::{Route, duration_ms, family_help, find_row, help_for, parse_cli, usage_line};
use recto_agent::{Command, Level, MAX_COMMAND_BYTES, parse, reply, run};
use recto_control::levels::parse_stored;
use recto_session::Session;
use std::path::Path;
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(30),
    memory: 256 * 1024 * 1024,
};
/// No answer to a command on a small session comes near this. The server's own cap is 32 MiB.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// A file's name that mustn't reach an agent at Off, whatever it asks for.
const CANARY: &str = "canary-xyzzy";

/// Whether a command is one this check may really run: anything that draws would start a worker, and a stand-in session has nothing for it to draw.
fn safe_to_run(command: &Command) -> bool {
    !command.draws()
}

/// Catches: a panic or a hang on any line or argument; a cap that doesn't hold; a command the CLI builds that the socket then refuses; anything from a document reaching Off.
pub fn check(data: &[u8]) {
    within("reading a command", BUDGET, || {
        let mut take = Take(data);
        let route = take.u8() % 4;
        let level = take.pick(&[Level::Off, Level::ReadOnly, Level::Control, Level::Control]);
        match route {
            0 => json_line(level, take.rest()),
            1 => command_line(take.rest()),
            2 => small_grammars(take.rest()),
            _ => level_file(take.rest()),
        }
    });
}

fn json_line(level: Level, bytes: &[u8]) {
    let line = String::from_utf8_lossy(bytes);
    let Ok(command) = parse(&line) else {
        return;
    };
    assert!(line.len() <= MAX_COMMAND_BYTES, "a line past the cap was read");
    let _ = (command.tag(), command.access(), command.draws());
    let details = command.log_details().to_string();
    // The log holds numbers and cleaned names, and never what was typed. The canary is a way to see that.
    assert!(!details.contains("canary-in-a-line"), "a command's words reached the log: {details}");
    if safe_to_run(&command) {
        run_on_a_session(level, &command);
    }
}

fn run_on_a_session(level: Level, command: &Command) {
    let mut session = Session::new(NamedLoader);
    let mut last = None;
    for (name, pages) in [
        (format!("/fuzz/{CANARY}.pdf"), 5),
        ("/fuzz/other.pdf".to_string(), 200),
        ("/fuzz/third.pdf".to_string(), 1),
    ] {
        last = session.open_with_facts(Path::new(&name), facts_of(pages), None, None).ok();
    }
    let result = run(&mut session, level, command);
    let answer = reply(command.tag(), &result).to_string();
    assert!(answer.len() <= MAX_ANSWER, "{} bytes in an answer to {}", answer.len(), command.tag());
    if level == Level::Off {
        assert!(!answer.contains(CANARY), "a tab's name reached Off: {answer}");
    }
    // What a tab is after any command is still something the session can say.
    let _ = session.sidebar();
    if let Some(id) = last.filter(|id| session.tabs().tab(*id).is_some()) {
        assert!(session.pages().count(id) >= 1, "a tab with no pages");
    }
}

/// A command line, in words. Words are split on zero bytes, so any word can hold spaces.
fn command_line(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let words: Vec<String> = text.split('\0').map(str::to_string).collect();
    // A real shell's limit, as far as the argument count goes.
    if words.len() > 4096 {
        return;
    }
    let Ok(mut invocation) = parse_cli(&words) else {
        return;
    };
    // What `recto` reads from stdin for this row, as it puts it in.
    if let Some(field) = recto_agent::table::stdin_secret_field(invocation.spec) {
        invocation.fields.insert(field.into(), serde_json::json!("from stdin"));
    }
    let wire = invocation.wire();
    // What the CLI builds is what the socket reads, so what it built must be a line the socket reads as a command. A line past the socket's cap gets refused
    // there, in words, which is fine; and a typed payload refuses flags that can't go together as it's read, as usage, the same by either route (R2.5).
    if !invocation.help && invocation.spec.route == Route::Session && wire.len() <= MAX_COMMAND_BYTES {
        if let Err(refused) = invocation.command() {
            assert_eq!(refused.code, "usage", "the CLI built a line the socket refuses: {wire}");
        }
    }
}

fn small_grammars(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    if let Ok(ms) = duration_ms(&text) {
        assert!(ms <= u64::MAX / 2 || text.len() > 8);
    }
    let _ = Level::parse(&text);
    let words: Vec<String> = text.split_whitespace().take(16).map(str::to_string).collect();
    if let Some(spec) = find_row(&words) {
        let _ = (help_for(spec), usage_line(spec));
    }
    let _ = family_help(&words);
    // Page specs and selectors through a real command, since that's where they're read.
    let quoted = serde_json::to_string(&*text).unwrap_or_default();
    for line in [
        format!(r#"{{"cmd":"page_delete","pages":{quoted}}}"#),
        format!(r#"{{"cmd":"where","tab":{quoted}}}"#),
        format!(r#"{{"cmd":"page_move","pages":{quoted},"to":1}}"#),
    ] {
        if let Ok(command) = parse(&line) {
            run_on_a_session(Level::Control, &command);
        }
    }
}

/// Catches: a level file that gives a level without saying one plainly. The file fails closed, so a level only comes from a string in the place a level goes.
fn level_file(bytes: &[u8]) {
    let Some(stored) = parse_stored(bytes) else { return };
    let said: serde_json::Value = serde_json::from_slice(bytes).expect("what gave a level is JSON");
    assert_eq!(said["level"].as_str().and_then(Level::parse), Some(stored.level));
}
