//! **The installer told the new version started:** the `launch-ok` a new version writes once it's up, as the copy that keeps the record or as a second copy.

use super::Files;
use recto_platform::local;

/// **The installer told the new version started:** the one place `launch-ok` is written, in the updater's own folder beside the record's (never the record's
/// folder, so a second copy may write it), with the tabs it came up with if it read the record.
pub(super) fn say_started(files: &Files, version: &str, shown_at_ms: u64, tabs: Option<u32>) {
    let _ = local::ensure_private_dir(&files.work);
    let line = crate::plan::launch_ok_line(version, std::process::id(), shown_at_ms, tabs);
    let _ = local::write_private_file(&files.launch_ok(), line.as_bytes());
}

/// **A second copy of a new version an update opened says it started,** with no tab count, as it read no record: unless a launch-ok is there for this version
/// already, the one the copy that keeps the record wrote with the tabs it came up with (two copies of it, `open -n`), which is left as it is, so the installer's
/// look at a misread isn't lost (the third read, 2026-11-13).
pub(super) fn say_started_as_a_second_copy(files: &Files, version: &str, shown_at_ms: u64) {
    let there = std::fs::read_to_string(files.launch_ok()).unwrap_or_default();
    if crate::plan::read_launch_ok(&there).is_none_or(|(said, _)| said != version) {
        say_started(files, version, shown_at_ms, None);
    }
}
