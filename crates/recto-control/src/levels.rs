//! The level the user set, in a file only they can write. Read fresh for every command, so a lowered level applies at once, and read closed: anything odd about the file means Off.
//!
//! Nothing over the socket can raise it. A command only ever carries the name of a level it wants, and the server never writes this file. `recto permission set` does, from the user's own terminal.
//! The app's settings take over from that command later, writing the same thing through the same [`store`].

use crate::state::StatePaths;
use recto_agent::Level;
use recto_platform::local;
use serde_json::{Value, json};

const MAX_LEVEL_FILE: u64 = 1024;

/// The level in force. Fail closed: a missing, unreadable, malformed, linked or open file is Off.
pub fn current(paths: &StatePaths) -> Level {
    read(paths).map_or(Level::Off, |stored| stored.level)
}

/// What the file says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub level: Level,
    pub set_at_ms: u64,
}

fn read(paths: &StatePaths) -> Option<Stored> {
    let bytes = local::read_private_file(&paths.level, MAX_LEVEL_FILE).ok()??;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let level = Level::parse(value["level"].as_str()?)?;
    Some(Stored { level, set_at_ms: value["set_at_ms"].as_u64().unwrap_or(0) })
}

/// Writes the level. Only `recto permission set`, the app's settings later, and test setup call this. No command reaches it.
pub fn store(paths: &StatePaths, level: Level) -> std::io::Result<()> {
    paths.prepare().map_err(|_| std::io::Error::from(std::io::ErrorKind::PermissionDenied))?;
    let body = json!({ "level": level.name(), "set_at_ms": now_ms() });
    local::write_private_file(&paths.level, body.to_string().as_bytes())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// What each level allows, in the one place.
fn allows(level: Level) -> &'static str {
    match level {
        Level::Off => "Status, help, and asking for more. Nothing from a document, no page text and no drawing.",
        Level::ReadOnly => "Read and search documents that are open, and watch what happens. Changes nothing.",
        Level::Control => "Open and close PDFs, move around, and edit pages in memory. Nothing reaches a file.",
        Level::Full => "Write new pictures to disk, throw away edits, and choose a worker's limits. Signing is never included.",
    }
}

pub const HOW_TO_CHANGE: &str = "Only you can raise this, by running `recto permission set <level>` in your own terminal. Later, the app's settings take this over.";

/// `recto permission show`.
pub fn show(paths: &StatePaths, pending: Option<Level>) -> Value {
    let stored = read(paths);
    let level = stored.map_or(Level::Off, |stored| stored.level);
    let levels: Vec<Value> = [Level::Off, Level::ReadOnly, Level::Control, Level::Full].iter().map(|level| json!({ "level": level.name(), "label": level.label(), "allows": allows(*level) })).collect();
    json!({
        "level": level.name(),
        "label": level.label(),
        "levels": levels,
        "how_to_change": HOW_TO_CHANGE,
        "set_at_ms": stored.map(|stored| stored.set_at_ms),
        "pending_request": pending.map(Level::name),
    })
}

/// The answer to `recto permission request`. It grants nothing: it says what the user has to do.
pub fn request_answer(wanted: Level, have: Level) -> Value {
    if have >= wanted {
        return json!({ "level": have.name(), "granted": true, "asked": false });
    }
    json!({
        "level": wanted.name(),
        "granted": false,
        "asked": true,
        "tell_the_user": format!("An agent is asking for {} access to RectoPDF: {} To allow it, run this in your own terminal.", wanted.label(), allows(wanted)),
        "command": format!("recto permission set {}", wanted.name()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_paths(name: &str) -> StatePaths {
        let root = std::env::temp_dir().join(format!("recto-levels-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = StatePaths::overridden(&root).unwrap();
        paths.prepare().unwrap();
        paths
    }

    /// Failure: the default isn't Off, or a damaged, linked or open file counts as whatever it says. Fail closed: an agent must never gain from a broken file.
    #[test]
    fn anything_odd_about_the_file_means_off() {
        let paths = temp_paths("closed");
        assert_eq!(current(&paths), Level::Off, "the default");
        store(&paths, Level::Control).unwrap();
        assert_eq!(current(&paths), Level::Control);
        assert_eq!(std::fs::metadata(&paths.level).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::set_permissions(&paths.level, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(current(&paths), Level::Off, "others can write it");
        std::fs::remove_file(&paths.level).unwrap();
        for junk in ["", "full", "{\"level\":\"root\"}", "{\"level\":7}", &"x".repeat(5000)] {
            local::write_private_file(&paths.level, junk.as_bytes()).unwrap();
            assert_eq!(current(&paths), Level::Off, "{junk:?}");
        }
        std::fs::remove_file(&paths.level).unwrap();
        let elsewhere = paths.root.join("elsewhere");
        store(&paths, Level::Full).unwrap();
        std::fs::rename(&paths.level, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &paths.level).unwrap();
        assert_eq!(current(&paths), Level::Off, "a link");
        std::fs::remove_dir_all(&paths.root).unwrap();
    }

    /// Failure: asking for a level raises it, or the answer doesn't tell the user what to run.
    #[test]
    fn a_request_grants_nothing() {
        let answer = request_answer(Level::Full, Level::Off);
        assert_eq!(answer["granted"], false);
        assert_eq!(answer["command"], "recto permission set full");
        assert_eq!(request_answer(Level::ReadOnly, Level::Control)["granted"], true);
    }
}
