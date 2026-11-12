//! **What the socket accepts and refuses stays what it was** (step R2.2, the command registry). Every line in `golden/requests.txt` is parsed, and what came of it is held to `golden/command_wire.txt`: the command it means, field by field with every default filled in, or the whole refusal, code, message, exit and next step. For each command the lines are its smallest and its fullest request, an unknown field, each field of the wrong type, null and missing, its positional array form, its name spelled as the Rust type is, and a field under its Rust name where the wire renames it.
//!
//! Failure caught: moving a command into a module of its own changes what serde reads: a field's default, a rename, whether an unknown field is still refused (serde ignores them on a unit variant, and might through a newtype variant), or the words of the refusal.
//!
//! `RECTO_BLESS=1` rewrites the golden file, for a change that means to change the wire. Its diff is the review.

use recto_agent::{envelope, parse};

const REQUESTS: &str = include_str!("golden/requests.txt");
const GOLDEN: &str = include_str!("golden/command_wire.txt");

/// The families a command may be held in, which `Debug` puts around it. What's held to the golden file is the command itself.
const FAMILIES: &[&str] = &["Start", "Read", "Move", "Edit", "Waits", "Rights", "Session", "Measure"];

fn without_family(debug: String) -> String {
    for family in FAMILIES {
        if let Some(inner) = debug.strip_prefix(&format!("{family}(")).and_then(|rest| rest.strip_suffix(')')) {
            return inner.to_string();
        }
    }
    debug
}

fn made() -> String {
    let mut out = String::new();
    let long = format!(r#"{{"cmd":"open","path":"/{}"}}"#, "a".repeat(recto_agent::MAX_COMMAND_BYTES));
    for line in REQUESTS.lines().chain([long.as_str()]) {
        let shown = if line.len() > 200 {
            format!("<{} bytes>", line.len())
        } else {
            line.to_string()
        };
        let result = match parse(line) {
            Ok(command) => format!("ok {} {}", command.tag(), without_family(format!("{command:?}"))),
            Err(error) => format!("refused {}", envelope(&Err(error))),
        };
        out.push_str(&format!("{shown}\n    {result}\n"));
    }
    out
}

#[test]
fn every_request_is_read_as_it_was() {
    let made = made();
    if std::env::var_os("RECTO_BLESS").is_some() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/command_wire.txt"), &made).unwrap();
        return;
    }
    for (number, (now, was)) in made.lines().zip(GOLDEN.lines()).enumerate() {
        assert_eq!(now, was, "line {} of golden/command_wire.txt", number + 1);
    }
    assert_eq!(made.lines().count(), GOLDEN.lines().count(), "the number of lines");
}
