//! What an update reads from the internet and the disk: `latest.json`, a release's zip, and what the app and the installer leave for each other. All hostile. The zip goes through the whole of the reader, and what it takes is unpacked for real, in a temp folder of its own.
//!
//! What must hold: no panic, no bomb, and nothing's written outside the folder or past what the zip said.

use crate::guard::{Budget, within};
use recto_update::rules::parse_latest;
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
