//! **The installer told the new version started:** the `launch-ok` a new version writes once it's up, as the copy that keeps the record or as a second copy.

use super::{Files, Host, Notice, St, Status, lock};
use crate::APP_NAME;
use recto_platform::local;
use std::sync::Mutex;

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

/// An arrived copy that cannot draw tells the installer; the primary copy also reports its live failure.
pub(super) fn drawing_failed(files: &Files, state: &Mutex<St>, host: &dyn Host, version: &str, second_copy: bool) {
    let _ = local::ensure_private_dir(&files.work);
    let _ = local::write_private_file(&files.launch_failed(), version.as_bytes());
    if second_copy {
        return;
    }
    {
        let mut state = lock(state);
        state.status = Status::Failed {
            version: version.to_owned(),
            why: "its renderer could not draw a page".to_owned(),
        };
        state.notice = Some(Notice {
            text: format!("{APP_NAME} {version} couldn't draw a page."),
            fades: false,
        });
    }
    host.changed();
}

#[cfg(test)]
pub(super) fn assert_failed_frame(updater: &super::Updater, shown_at_ms: u64, quits: &std::sync::atomic::AtomicU32, hand_offs: &std::sync::atomic::AtomicU32) {
    use super::{Status, lock};
    use std::sync::atomic::Ordering;
    let saved = lock(&updater.st).saved.clone();
    let disk = std::fs::read(&updater.files.saved).ok();
    let calls = (quits.load(Ordering::SeqCst), hand_offs.load(Ordering::SeqCst));
    let version = lock(&updater.st).facts.as_ref().unwrap().version.clone();
    updater.first_frame(shown_at_ms);
    assert!(
        matches!(&lock(&updater.st).status, Status::Failed { version: failed, why } if failed == &version && why.contains("renderer")),
        "an arrived version that cannot draw must not keep reporting Updated"
    );
    let notice = updater.notice().expect("a live rendering failure is explained");
    assert!(
        !notice.fades && notice.text.contains(&version) && notice.text.contains("couldn't draw"),
        "{notice:?}"
    );
    assert_eq!(lock(&updater.st).saved, saved, "render failure does not change remembered metadata");
    assert_eq!(std::fs::read(&updater.files.saved).ok(), disk);
    assert_eq!((quits.load(Ordering::SeqCst), hand_offs.load(Ordering::SeqCst)), calls, "no handoff or quit");
}

#[cfg(test)]
pub(super) fn assert_second_copy_preserves_status(updater: &super::Updater, shown_at_ms: u64) {
    use super::lock;
    let before = {
        let st = lock(&updater.st);
        (st.status.clone(), st.notice.clone())
    };
    updater.first_frame(shown_at_ms);
    let st = lock(&updater.st);
    assert_eq!((st.status.clone(), st.notice.clone()), before, "a second copy only notifies the installer");
}
