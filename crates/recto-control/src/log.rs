//! The activity log: every agent action, with its time, command, level and outcome. Capped and rotated, so it can't fill a disk. **Document text never goes in:** only a command's name, numbers and
//! cleaned file names, from [`recto_agent::Command::log_details`].
//!
//! **What an agent can't erase.** A log with one cap can be flushed by whoever is being logged: about 28,000 `status` calls at Off would push out everything before them. So there are two files with a cap
//! each. What changes something (opening, editing, drawing to disk, setting the level) goes in one, and everything else in the other, so no amount of reading or refused commands can reach it. On top of that,
//! the commands an agent can repeat for free (`status`, a bad line, a refusal) are written once, and then as one line with a count, at most once a minute each.
//!
//! **Going past a file's permissions** (owner, 2026-11-07: an agent keeps `recto copy --anyway`, and each use goes in the activity log). Each use that worked gets a line in a third file of its own, never counted or collapsed. It's at Read-only, so in the changes' file it would let an agent at Read-only push out what's changed, and among the rest a flood of reading could push it out. Here, only more of the same can.

use crate::levels::now_ms;
use crate::state::StatePaths;
use recto_agent::Level;
use recto_platform::local;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// One file's size before it's rotated. Two files are kept for each of three kinds, so at most six times this.
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;
const DEFAULT_LAST: usize = 50;
const MAX_LINE_BYTES: usize = 4096;
/// How long repeats are counted before they're written as one line.
const COLLAPSE_WINDOW: Duration = Duration::from_secs(60);
/// Kinds of repeat counted at once. They're bounded anyway, by commands times levels. This is for certainty.
const MAX_COLLAPSING: usize = 512;

/// One file and its older half.
#[derive(Debug)]
struct Rotating {
    path: PathBuf,
    old_path: PathBuf,
    file: Option<File>,
    size: u64,
}

impl Rotating {
    fn new(path: PathBuf, old_path: PathBuf) -> Self {
        Self { path, old_path, file: None, size: 0 }
    }

    fn append(&mut self, paths: &StatePaths, line: &[u8]) -> std::io::Result<()> {
        if self.size + line.len() as u64 > MAX_LOG_BYTES && self.file.is_some() {
            self.file = None;
            std::fs::rename(&self.path, &self.old_path)?;
        }
        if self.file.is_none() {
            paths.prepare().map_err(|_| std::io::Error::from(std::io::ErrorKind::PermissionDenied))?;
            let file = local::append_private_file(&self.path)?;
            self.size = file.metadata()?.len();
            // Something else may have filled it, and a new session starts here.
            if self.size + line.len() as u64 > MAX_LOG_BYTES {
                drop(file);
                std::fs::rename(&self.path, &self.old_path)?;
                self.size = 0;
                self.file = Some(local::append_private_file(&self.path)?);
            } else {
                self.file = Some(file);
            }
        }
        if let Some(file) = &mut self.file {
            file.write_all(line)?;
            self.size += line.len() as u64;
        }
        Ok(())
    }
}

/// Repeats of one thing since its last line.
#[derive(Debug)]
struct Collapsing {
    cmd: String,
    level: Level,
    outcome: String,
    count: u64,
    since_ms: u64,
    written: Instant,
}

/// Appends to the log. Opens lazily, and a failure to write stays visible in `status` rather than stopping the command.
#[derive(Debug)]
pub struct ActivityLog {
    paths: StatePaths,
    changes: Rotating,
    others: Rotating,
    overrides: Rotating,
    collapsing: Vec<Collapsing>,
    failing: bool,
}

/// What changed something: a command of that kind that worked. A failed one changed nothing, whatever it was stopped by, and a flood of those can't push the changes out. A list of refusals would miss the next one added, as `would_discard_user_edits` was. **Limit:** an agent's own working commands can still push out older ones, since each is a change to record.
fn is_change(cmd: &str, outcome: &str) -> bool {
    outcome == "ok" && matches!(cmd, "open" | "close" | "page_move" | "page_rotate" | "page_delete" | "page_insert" | "undo" | "redo" | "discard" | "render" | "export_svg" | "index_clear" | "stop" | "permission_set")
}

/// Text given past what a file's permissions ask. Only an answer that worked gave anything.
fn is_override(cmd: &str, outcome: &str, details: &Value) -> bool {
    outcome == "ok" && matches!(cmd, "select" | "copy") && details["anyway"] == true
}

/// What an agent can repeat for nothing, so it's counted rather than written each time.
fn collapses(cmd: &str, outcome: &str) -> bool {
    matches!(cmd, "status" | "unknown" | "connect" | "permission_show" | "permission_request") || matches!(outcome, "permission_denied" | "needs_terminal" | "would_clear_redo" | "would_discard_user_edits" | "sign_refused" | "busy")
}

impl ActivityLog {
    pub fn new(paths: &StatePaths) -> Self {
        Self {
            paths: paths.clone(),
            changes: Rotating::new(paths.log.clone(), paths.old_log.clone()),
            others: Rotating::new(paths.other_log.clone(), paths.old_other_log.clone()),
            overrides: Rotating::new(paths.overrides_log.clone(), paths.old_overrides_log.clone()),
            collapsing: Vec::new(),
            failing: false,
        }
    }

    /// One line, written in one call so two processes logging don't tangle.
    pub fn record(&mut self, cmd: &str, level: Level, outcome: &str, details: &Value) {
        self.flush_due(false);
        if collapses(cmd, outcome) && self.note_repeat(cmd, level, outcome) {
            return;
        }
        let details = if collapses(cmd, outcome) { &Value::Null } else { details };
        let mut line = json!({ "t_ms": now_ms(), "cmd": cmd, "level": level.name(), "outcome": outcome, "details": details }).to_string();
        if line.len() > MAX_LINE_BYTES {
            line = json!({ "t_ms": now_ms(), "cmd": cmd, "level": level.name(), "outcome": outcome }).to_string();
        }
        line.push('\n');
        let wrote = if is_override(cmd, outcome, details) {
            self.overrides.append(&self.paths, line.as_bytes())
        } else if is_change(cmd, outcome) {
            self.changes.append(&self.paths, line.as_bytes())
        } else {
            self.others.append(&self.paths, line.as_bytes())
        };
        self.failing = wrote.is_err();
    }

    /// Whether this is a repeat of something already written. The first of a kind is written straight away, so a crash can't lose that it happened at all.
    fn note_repeat(&mut self, cmd: &str, level: Level, outcome: &str) -> bool {
        if let Some(found) = self.collapsing.iter_mut().find(|found| found.cmd == cmd && found.level == level && found.outcome == outcome) {
            found.count += 1;
            return true;
        }
        if self.collapsing.len() >= MAX_COLLAPSING {
            self.flush_due(true);
        }
        self.collapsing.push(Collapsing { cmd: cmd.to_string(), level, outcome: outcome.to_string(), count: 0, since_ms: now_ms(), written: Instant::now() });
        false
    }

    /// Writes the counts that are due, or all of them.
    fn flush_due(&mut self, all: bool) {
        let mut lines = String::new();
        self.collapsing.retain_mut(|found| {
            if !all && found.written.elapsed() < COLLAPSE_WINDOW {
                return true;
            }
            if found.count > 0 {
                lines.push_str(&json!({ "t_ms": now_ms(), "cmd": found.cmd, "level": found.level.name(), "outcome": found.outcome, "count": found.count, "since_ms": found.since_ms }).to_string());
                lines.push('\n');
            }
            false
        });
        if !lines.is_empty() {
            self.failing = self.others.append(&self.paths, lines.as_bytes()).is_err();
        }
    }

    /// Writes every count now, before reading the log or ending.
    pub fn flush(&mut self) {
        self.flush_due(true);
    }

    /// Whether the last write worked.
    pub fn healthy(&self) -> bool {
        !self.failing
    }
}

impl Drop for ActivityLog {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The last entries of both kinds together, oldest first. A line that doesn't parse is skipped: a process can die half way through writing one.
pub fn recent(paths: &StatePaths, last: Option<u32>) -> Value {
    let wanted = last.map_or(DEFAULT_LAST, |last| last as usize).max(1);
    let mut lines = Vec::new();
    let mut kept_bytes = 0u64;
    for path in [&paths.old_log, &paths.log, &paths.old_other_log, &paths.other_log, &paths.old_overrides_log, &paths.overrides_log] {
        if let Ok(Some(bytes)) = local::read_private_file(path, MAX_LOG_BYTES * 2) {
            kept_bytes += bytes.len() as u64;
            for line in String::from_utf8_lossy(&bytes).lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    lines.push(value);
                }
            }
        }
    }
    // Stable, so lines from one file keep their order if two share a millisecond.
    lines.sort_by_key(|entry| entry["t_ms"].as_u64().unwrap_or(0));
    let skip = lines.len().saturating_sub(wanted);
    let entries: Vec<Value> = lines
        .into_iter()
        .skip(skip)
        .map(|entry| {
            let t_ms = entry["t_ms"].as_u64().unwrap_or(0);
            let clean = |key: &str| recto_core::clean_line(entry[key].as_str().unwrap_or(""));
            let mut row = json!({ "time": iso8601(t_ms), "t_ms": t_ms, "cmd": clean("cmd"), "level": clean("level"), "outcome": clean("outcome"), "details": entry["details"] });
            if let Some(count) = entry["count"].as_u64() {
                row["count"] = json!(count);
                row["since"] = json!(iso8601(entry["since_ms"].as_u64().unwrap_or(0)));
            }
            row
        })
        .collect();
    json!({ "entries": entries, "kept_bytes": kept_bytes, "file": paths.log })
}

/// A time as `2026-11-07T01:13:05Z`, always in UTC, so it reads the same everywhere and sorts.
pub fn iso8601(ms: u64) -> String {
    let seconds = ms / 1000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let of_day = seconds % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", of_day / 3600, of_day / 60 % 60, of_day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(name: &str) -> StatePaths {
        let root = std::env::temp_dir().join(format!("recto-log-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        StatePaths::overridden(&root).unwrap()
    }

    /// Failure: a long session or a client in a loop fills the disk. Each file must stay under its cap, and the newest stays.
    #[test]
    fn the_log_is_capped_and_rotated() {
        let paths = temp_paths("rotate");
        let mut log = ActivityLog::new(&paths);
        let details = json!({ "tab": "x".repeat(200) });
        for _ in 0..12_000 {
            log.record("open", Level::Control, "ok", &details);
            log.record("tabs", Level::ReadOnly, "ok", &details);
        }
        assert!(log.healthy());
        let size = |path: &std::path::Path| std::fs::metadata(path).map_or(0, |meta| meta.len());
        assert!(size(&paths.old_log) > 0 && size(&paths.old_other_log) > 0, "it never rotated");
        for path in [&paths.log, &paths.old_log, &paths.other_log, &paths.old_other_log] {
            assert!(size(path) <= MAX_LOG_BYTES);
        }
        log.record("newest", Level::Full, "ok", &Value::Null);
        let shown = recent(&paths, Some(1));
        assert_eq!(shown["entries"][0]["cmd"], "newest");
        std::fs::remove_dir_all(&paths.root).unwrap();
    }

    /// Failure: a refused `discard`, or a failed command of any kind that changes things when it works, writes to the file of changes, so repeating it pushes out real edits. Going by a list of refusals, this fails again on the next code nobody added to it, so these include codes the list never had.
    #[test]
    fn a_failed_command_isnt_a_change() {
        let paths = temp_paths("failed");
        let mut log = ActivityLog::new(&paths);
        log.record("page_delete", Level::Control, "ok", &json!({ "pages": "2" }));
        let size = std::fs::metadata(&paths.log).unwrap().len();
        for (cmd, outcome) in [("discard", "would_discard_user_edits"), ("undo", "nothing_to_undo"), ("redo", "nothing_to_redo"), ("page_rotate", "page_out_of_range"), ("close", "unsaved_changes"), ("open", "not_found"), ("render", "too_large_at_this_scale"), ("stop", "busy"), ("page_move", "some_code_nobody_has_thought_of_yet")] {
            for _ in 0..3 {
                log.record(cmd, Level::Control, outcome, &Value::Null);
            }
            assert_eq!(std::fs::metadata(&paths.log).unwrap().len(), size, "{cmd} {outcome} wrote to the changes");
        }
        // Still in the record, and repeating a refusal is counted rather than written out.
        log.flush();
        let shown = recent(&paths, Some(1000));
        let entries = shown["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["cmd"] == "undo" && entry["outcome"] == "nothing_to_undo"), "{shown}");
        let refused: Vec<&Value> = entries.iter().filter(|entry| entry["outcome"] == "would_discard_user_edits").collect();
        assert_eq!(refused.len(), 2, "one written, and one for the rest: {refused:?}");
        std::fs::remove_dir_all(&paths.root).unwrap();
    }

    /// Failure: an agent erases its own record by asking for `status` about 28,000 times, or sending bad lines, or reading, or being refused. None of them can touch what changed something, and the repeats collapse.
    #[test]
    fn no_flood_can_erase_an_earlier_edit() {
        let paths = temp_paths("flood");
        let mut log = ActivityLog::new(&paths);
        log.record("page_delete", Level::Control, "ok", &json!({ "pages": "2" }));
        let edits_size = std::fs::metadata(&paths.log).unwrap().len();
        // Far past what would rotate a file that took everything.
        for page in 0..60_000 {
            log.record("status", Level::Off, "ok", &Value::Null);
            log.record("unknown", Level::Off, "bad_command", &Value::Null);
            log.record("text", Level::Off, "permission_denied", &json!({ "page": page }));
            log.record("tabs", Level::ReadOnly, "ok", &json!({ "from": page }));
        }
        assert_eq!(std::fs::metadata(&paths.log).unwrap().len(), edits_size, "only a change writes here");
        assert!(!paths.old_log.exists());
        log.flush();
        let shown = recent(&paths, Some(100_000));
        let entries = shown["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["cmd"] == "page_delete"), "the edit is gone");
        // The first of each was written at once, and pushed out by the reads. The count of the rest is there, in one line each.
        for (cmd, outcome) in [("status", "ok"), ("unknown", "bad_command"), ("text", "permission_denied")] {
            let rows: Vec<&Value> = entries.iter().filter(|entry| entry["cmd"] == cmd && entry["outcome"] == outcome && entry["count"].is_u64()).collect();
            assert_eq!(rows.len(), 1, "{cmd}: {rows:?}");
            assert_eq!(rows[0]["count"], 59_999, "{cmd}");
        }
        std::fs::remove_dir_all(&paths.root).unwrap();
    }

    /// Failure: repeats are held back so long that a crash loses that anything happened, or the first of a kind isn't written at once.
    #[test]
    fn the_first_of_a_kind_is_written_at_once() {
        let paths = temp_paths("first");
        let mut log = ActivityLog::new(&paths);
        log.record("unknown", Level::Off, "bad_command", &Value::Null);
        assert!(std::fs::read_to_string(&paths.other_log).unwrap().contains("bad_command"));
        std::fs::remove_dir_all(&paths.root).unwrap();
    }

    /// Failure: a line cut short when a process died breaks reading the whole log.
    #[test]
    fn a_torn_line_is_skipped() {
        let paths = temp_paths("torn");
        let mut log = ActivityLog::new(&paths);
        log.record("open", Level::Control, "ok", &Value::Null);
        {
            let mut file = local::append_private_file(&paths.log).unwrap();
            file.write_all(b"{\"t_ms\":12,\"cmd\":\"te").unwrap();
        }
        let shown = recent(&paths, None);
        assert_eq!(shown["entries"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(&paths.root).unwrap();
    }

    /// Failure: times come out wrong or in a local zone, so entries from two machines can't be compared.
    #[test]
    fn times_are_utc_and_exact() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_794_013_985_000), "2026-11-07T01:13:05Z");
        assert_eq!(iso8601(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    /// Failure: a state folder that can't be written stops the command rather than reporting itself, or the failure goes unseen.
    #[test]
    fn a_log_that_cant_be_written_says_so() {
        let paths = temp_paths("cant");
        std::fs::write(&paths.root, b"a file where the folder should be").unwrap();
        let mut log = ActivityLog::new(&paths);
        log.record("open", Level::Control, "ok", &Value::Null);
        assert!(!log.healthy());
        std::fs::remove_file(&paths.root).unwrap();
    }

    /// Failure: an agent uses `--anyway` and it leaves no line, or one that's counted in with others, or one that a flood of ordinary reading can push out, or by which an agent at Read-only can push out what's changed. Owner, 2026-11-07: "each use" goes in the log.
    #[test]
    fn each_use_of_anyway_has_its_own_line_that_nothing_else_can_push_out() {
        let dir = std::env::temp_dir().join(format!("recto-log-anyway-{}", std::process::id()));
        let paths = StatePaths::overridden(&dir).unwrap();
        let mut log = ActivityLog::new(&paths);
        let anyway = json!({ "tab": "#1", "page": 1, "anyway": true });
        for _ in 0..3 {
            log.record("copy", Level::ReadOnly, "ok", &anyway);
        }
        log.record("select", Level::ReadOnly, "ok", &anyway);
        // Asked for it, and refused: nothing was given.
        log.record("copy", Level::ReadOnly, "copy_restricted", &json!({ "anyway": false }));
        log.record("copy", Level::ReadOnly, "ok", &json!({ "anyway": false }));
        let lines = |path: &PathBuf| std::fs::read_to_string(path).unwrap_or_default().lines().count();
        assert_eq!(lines(&paths.overrides_log), 4, "one for each use");
        assert_eq!(lines(&paths.log), 0, "the changes' file isn't a Read-only agent's to fill");
        // A flood of reading, to past both of the other file's halves.
        let filler = json!({ "page": 1, "filler": "x".repeat(2000) });
        for _ in 0..1200 {
            log.record("text", Level::ReadOnly, "ok", &filler);
        }
        assert!(paths.old_other_log.exists(), "it rotated");
        assert_eq!(lines(&paths.overrides_log), 4, "and these are still there");
        let seen = recent(&paths, Some(10_000));
        let rows = seen["entries"].as_array().unwrap();
        assert_eq!(rows.iter().filter(|row| row["details"]["anyway"] == true).count(), 4, "`recto log` shows them");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
