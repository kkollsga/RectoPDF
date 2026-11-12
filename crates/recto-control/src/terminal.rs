//! `recto permission set`: the one way a level goes up in the headless phase, from the user's own terminal. The app's settings take this over later.
//!
//! Lowering needs nothing: anyone may make things safer. Raising needs a real terminal on stdin and on `/dev/tty`, and a word typed back that's different every time, read from the terminal rather than
//! stdin. That shuts out every ordinary way an agent runs commands. What it can't do is stop a process of this same user that goes to the length of faking a terminal, or writes the file itself. That
//! process is inside the trust boundary, as `docs/engineering/agent-interface.md` says. The app's settings, behind a human gesture, are the stronger answer.

use crate::client::Client;
use crate::log::ActivityLog;
use crate::{levels, state::StatePaths};
use recto_agent::{AgentError, Level, exit};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

/// Sets the level. Raising it reads a confirmation from the real terminal.
pub fn set_level(paths: &StatePaths, wanted: Level) -> Result<Value, AgentError> {
    let was = levels::current(paths);
    if wanted > was {
        confirm_at_terminal(wanted)?;
    }
    levels::store(paths, wanted)
        .map_err(|_| AgentError::new("write_failed", "The level couldn't be saved.", exit::FAILED).then("Check that the state folder can be written to."))?;
    ActivityLog::new(paths).record("permission_set", wanted, "ok", &json!({ "by": "the user's terminal", "was": was.name() }));
    // Tell a running session, so anyone waiting finds out at once. It reads the file itself: this message carries no level.
    if let Ok(mut client) = Client::connect(paths) {
        let _ = client.call("{\"cmd\":\"level_changed\"}", Duration::from_secs(2));
    }
    Ok(json!({ "level": wanted.name(), "label": wanted.label(), "was": was.name(), "was_label": was.label() }))
}

/// The refusal an agent gets. It says who can do this, and how.
pub fn needs_terminal(wanted: Level) -> AgentError {
    let command = format!("recto permission set {}", wanted.name());
    AgentError::new(
        "needs_terminal",
        "Raising agent access needs the user, at a real terminal. Nothing else can do it.",
        exit::PERMISSION,
    )
    .then(format!("Ask the user to run `{command}` in their own terminal."))
}

fn confirm_at_terminal(wanted: Level) -> Result<(), AgentError> {
    let what = format!(
        "Give agents {} access to RectoPDF? {}",
        wanted.label(),
        match wanted {
            Level::Full => "They could write new pictures to disk and throw away unsaved edits.",
            Level::Control => "They could open and close PDFs and change pages in memory.",
            _ => "They could read and search PDFs that are open.",
        }
    );
    let command = format!("recto permission set {}", wanted.name());
    confirm(&what, &command, || needs_terminal(wanted))
}

/// `recto skill install`: writes into the user's agent's own files, so it takes the user, the same way raising a level does. `recto skill show` needs nothing, so an agent can still read it.
pub fn confirm_skill_install(folder: &Path, replace: bool) -> Result<(), AgentError> {
    let shown = recto_core::clean_line(&folder.display().to_string());
    let what = if replace {
        format!("Write RectoPDF's skill to {shown}, replacing files there even if someone changed them?")
    } else {
        format!("Write RectoPDF's skill to {shown}?")
    };
    confirm(&what, "recto skill install", || {
        AgentError::new("needs_terminal", "Installing the skill needs the user, at a real terminal.", exit::PERMISSION)
            .then("Ask the user to run `recto skill install` in their own terminal. `recto skill show` prints the skill for you to read.")
    })
}

/// The real-terminal check, and the word typed back: stdin and `/dev/tty` must be a terminal, and the answer is read from it, never from stdin. `refused` says what to do about it.
fn confirm(what: &str, command: &str, refused: impl Fn() -> AgentError) -> Result<(), AgentError> {
    if !std::io::stdin().is_terminal() {
        return Err(refused());
    }
    let terminal: File = OpenOptions::new().read(true).write(true).open("/dev/tty").map_err(|_| refused())?;
    if !terminal.is_terminal() {
        return Err(refused());
    }
    let word = fresh_word();
    let mut out = &terminal;
    write!(out, "{what}\nTo allow it, type {word} and press return: ").map_err(|_| refused())?;
    let mut answer = String::new();
    BufReader::new(&terminal).read_line(&mut answer).map_err(|_| refused())?;
    if answer.trim() != word {
        let _ = writeln!(out, "That wasn't the word, so nothing changed.");
        return Err(AgentError::new("not_confirmed", "That wasn't the word, so nothing changed.", exit::PERMISSION)
            .then(format!("Run `{command}` again, and type the word it shows.")));
    }
    Ok(())
}

/// A short word to type back. Different every time, from the OS's randomness behind `RandomState`.
fn fresh_word() -> String {
    const CONSONANTS: &[u8] = b"bdfgklmnprstvz";
    const VOWELS: &[u8] = b"aeiou";
    let mut hash = std::collections::hash_map::RandomState::new().build_hasher();
    hash.write_u8(1);
    let mut bits = hash.finish();
    let mut word = String::with_capacity(6);
    for at in 0..6 {
        let letters = if at % 2 == 0 { CONSONANTS } else { VOWELS };
        word.push(char::from(letters[(bits % letters.len() as u64) as usize]));
        bits /= letters.len() as u64;
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the word is the same every time, so a script can type it without reading the prompt.
    #[test]
    fn the_word_changes() {
        let words: std::collections::HashSet<String> = (0..16).map(|_| fresh_word()).collect();
        assert!(words.len() > 8, "{words:?}");
        assert!(words.iter().all(|word| word.len() == 6));
    }
}
