//! What an update reads from the internet and the disk: `latest.json`, a release's zip, and what the app and the installer leave for each other: the plan, the outcome, the saved settings, the line a new version says it's up with, and what `lsregister` lists. All hostile. The zip goes through the whole of the reader, and what it takes is unpacked for real, in a temp folder of its own.
//!
//! What must hold: no panic, no bomb, nothing's written outside the folder or past what the zip said, and what's read from the app's own files keeps the rules its reader says it does.

use crate::guard::{Budget, within};
use recto_update::plan::{MAX_OPEN, Outcome, Plan, read_launch_ok};
use recto_update::rules::{parse_latest, parse_version};
use recto_update::saved::Saved;
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const BUDGET: Budget = Budget {
    time: Duration::from_secs(120),
    memory: 512 * 1024 * 1024,
};
const APP: &str = "RectoPDF.app";

static NEXT: AtomicU32 = AtomicU32::new(0);

pub fn check(data: &[u8]) {
    within("update", BUDGET, || {
        let _ = parse_latest(data);
        local_files(data);
        let Ok(entries) = recto_update::zip::read_directory(&mut Cursor::new(data), APP) else {
            return;
        };
        let folder = std::env::temp_dir().join(format!("recto-fuzz-update-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&folder);
        if std::fs::create_dir_all(&folder).is_ok() {
            let _ = recto_update::zip::unpack_entries(&mut Cursor::new(data), &entries, &folder, Instant::now() + Duration::from_secs(60));
            let _ = std::fs::remove_dir_all(&folder);
        }
    });
}

/// The files the app and the installer leave for each other, as whatever's on disk.
fn local_files(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Some((version, tabs)) = read_launch_ok(&text) {
        assert!(!version.is_empty() && !version.contains(char::is_whitespace));
        let _ = recto_update::plan::misread(tabs, tabs.map(|tabs| tabs.saturating_sub(1)));
    }
    // What `lsregister` lists, which names what any app on the Mac calls itself.
    #[cfg(target_os = "macos")]
    for found in recto_platform::registry::parse_dump(&text) {
        assert!(
            found.identifier.starts_with(recto_platform::registry::ID_PREFIX),
            "{:?} isn't ours",
            found.identifier
        );
    }
    // The three files are JSON objects, and their readers turn down anything else before they look at it, so only what could be one goes to disk.
    if data.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return;
    }
    let folder = std::env::temp_dir().join(format!(
        "recto-fuzz-update-files-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&folder);
    if recto_platform::local::ensure_private_dir(&folder).is_err() {
        return;
    }
    let file = folder.join("file.json");
    if recto_platform::local::write_private_file(&file, data).is_ok() {
        if let Some(plan) = Plan::read(&file) {
            assert!(plan.open.len() <= MAX_OPEN && plan.open.iter().all(|path| path.is_absolute()));
            assert!(parse_version(&plan.from).is_some() && parse_version(&plan.to).is_some());
            assert!(plan.app.is_absolute() && plan.beside.is_absolute() && plan.root.is_absolute());
        }
        if let Some(outcome) = Outcome::read(&file) {
            assert!(parse_version(&outcome.from).is_some() && parse_version(&outcome.to).is_some());
        }
        let saved = Saved::read(&file);
        assert!(saved.offered.as_deref().is_none_or(|offered| parse_version(offered).is_some()));
        assert!(saved.failed.iter().all(|known| recto_update::version::parse(known).is_some()));
    }
    let _ = std::fs::remove_dir_all(&folder);
}
