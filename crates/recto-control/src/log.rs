//! The activity log: every agent action, with its time, command, level and outcome. Capped and rotated, so it can't fill a disk. **Document text never goes in:** only a command's name, numbers and
//! cleaned file names, from [`recto_agent::Command::log_details`].

use crate::levels::now_ms;
use crate::state::StatePaths;
use recto_agent::Level;
use recto_platform::local;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Write;

/// One file's size before it's rotated. Two files are kept, so at most twice this.
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;
const DEFAULT_LAST: usize = 50;
const MAX_LINE_BYTES: usize = 4096;

/// Appends to the log. Opens lazily, and a failure to write stays visible in `status` rather than stopping the command.
#[derive(Debug)]
pub struct ActivityLog {
    paths: StatePaths,
    file: Option<File>,
    size: u64,
    failing: bool,
}

impl ActivityLog {
    pub fn new(paths: &StatePaths) -> Self {
        Self { paths: paths.clone(), file: None, size: 0, failing: false }
    }

    /// One line, written in one call so two processes logging don't tangle.
    pub fn record(&mut self, cmd: &str, level: Level, outcome: &str, details: &Value) {
        let mut line = json!({ "t_ms": now_ms(), "cmd": cmd, "level": level.name(), "outcome": outcome, "details": details }).to_string();
        if line.len() > MAX_LINE_BYTES {
            line = json!({ "t_ms": now_ms(), "cmd": cmd, "level": level.name(), "outcome": outcome }).to_string();
        }
        line.push('\n');
        self.failing = self.append(line.as_bytes()).is_err();
    }

    fn append(&mut self, line: &[u8]) -> std::io::Result<()> {
        if self.size + line.len() as u64 > MAX_LOG_BYTES && self.file.is_some() {
            self.file = None;
            std::fs::rename(&self.paths.log, &self.paths.old_log)?;
        }
        if self.file.is_none() {
            self.paths.prepare().map_err(|_| std::io::Error::from(std::io::ErrorKind::PermissionDenied))?;
            let file = local::append_private_file(&self.paths.log)?;
            self.size = file.metadata()?.len();
            // Something else may have filled it, and a new session starts here.
            if self.size + line.len() as u64 > MAX_LOG_BYTES {
                drop(file);
                std::fs::rename(&self.paths.log, &self.paths.old_log)?;
                self.size = 0;
                self.file = Some(local::append_private_file(&self.paths.log)?);
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

    /// Whether the last write worked.
    pub fn healthy(&self) -> bool {
        !self.failing
    }
}

/// The last entries, oldest first. A line that doesn't parse is skipped: a process can die half way through writing one.
pub fn recent(paths: &StatePaths, last: Option<u32>) -> Value {
    let wanted = last.map_or(DEFAULT_LAST, |last| last as usize).max(1);
    let mut lines = Vec::new();
    let mut kept_bytes = 0u64;
    for path in [&paths.old_log, &paths.log] {
        if let Ok(Some(bytes)) = local::read_private_file(path, MAX_LOG_BYTES * 2) {
            kept_bytes += bytes.len() as u64;
            for line in String::from_utf8_lossy(&bytes).lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    lines.push(value);
                }
            }
        }
    }
    let skip = lines.len().saturating_sub(wanted);
    let entries: Vec<Value> = lines
        .into_iter()
        .skip(skip)
        .map(|entry| {
            let t_ms = entry["t_ms"].as_u64().unwrap_or(0);
            let clean = |key: &str| recto_core::clean_line(entry[key].as_str().unwrap_or(""));
            json!({ "time": iso8601(t_ms), "t_ms": t_ms, "cmd": clean("cmd"), "level": clean("level"), "outcome": clean("outcome"), "details": entry["details"] })
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

    /// Failure: a long session or a client in a loop fills the disk. The log must stay under twice its cap, and keep the newest.
    #[test]
    fn the_log_is_capped_and_rotated() {
        let paths = temp_paths("rotate");
        let mut log = ActivityLog::new(&paths);
        let details = json!({ "file": "x".repeat(200) });
        for _ in 0..12_000 {
            log.record("status", Level::Off, "ok", &details);
        }
        assert!(log.healthy());
        let size = |path: &std::path::Path| std::fs::metadata(path).map_or(0, |meta| meta.len());
        assert!(size(&paths.old_log) > 0, "it never rotated");
        assert!(size(&paths.log) <= MAX_LOG_BYTES && size(&paths.old_log) <= MAX_LOG_BYTES);
        log.record("newest", Level::Full, "ok", &Value::Null);
        let shown = recent(&paths, Some(1));
        assert_eq!(shown["entries"][0]["cmd"], "newest");
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
}
