//! What's remembered between launches: `updates.json` in the state folder. A few dozen bytes, read when the app starts and written when something's happened. A damaged one is the same as none: the next check puts it right.

use crate::rules::{parse_version, retry_after};
use recto_platform::local;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Far more than it needs. A file this size isn't ours.
const MAX_BYTES: u64 = 16 * 1024;
/// Failed versions remembered, the newest.
const FAILED_KEPT: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The last check that got an answer.
    pub last_check_ms: Option<u64>,
    /// The last try, whichever way it went, and failed ones in a row since one that worked.
    pub last_attempt_ms: Option<u64>,
    pub failures: u32,
    /// The version on offer at the last check. The checksum isn't kept: an install reads it again, which also shows it hasn't been withdrawn.
    pub offered: Option<String>,
    /// Installs that failed in a row since one that worked, and when the last one tried, for going easy on a transient failure.
    pub install_failures: u32,
    pub last_install_ms: Option<u64>,
    /// Versions that were refused or didn't start. Never tried by itself again.
    pub failed: Vec<String>,
    /// The version that last reached its first frame. A launch of any other counts towards going back.
    pub proven: Option<String>,
    /// Launches of this version that haven't reached their first frame.
    pub unstarted: u32,
    /// The last update that went through, as it's reported.
    pub last_update: Option<LastUpdate>,
}

/// How the last update went.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastUpdate {
    pub from: String,
    pub to: String,
    /// From the old app being told to quit to the new one's first correct frame.
    pub gap_ms: u64,
    pub at_ms: u64,
}

impl Saved {
    /// Reads it, keeping only what's well-formed. Nothing from this file reaches the screen or a command without passing here.
    #[must_use]
    pub fn read(file: &Path) -> Self {
        let Ok(Some(bytes)) = local::read_private_file(file, MAX_BYTES) else { return Self::default() };
        let Ok(mut saved) = serde_json::from_slice::<Self>(&bytes) else { return Self::default() };
        saved.offered = saved.offered.filter(|version| parse_version(version).is_some());
        saved.proven = saved.proven.filter(|version| parse_version(version).is_some());
        saved.failed.retain(|version| parse_version(version).is_some());
        saved.last_update = saved.last_update.filter(|update| parse_version(&update.from).is_some() && parse_version(&update.to).is_some());
        saved
    }

    /// Writes it, whole, by a rename. A failed write costs a check.
    pub fn write(&self, file: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = local::write_private_file(file, &bytes);
        }
    }

    pub fn note_failed_version(&mut self, version: &str) {
        if !self.failed.iter().any(|known| known == version) {
            self.failed.push(version.to_owned());
        }
        let over = self.failed.len().saturating_sub(FAILED_KEPT);
        self.failed.drain(..over);
    }

    /// How long until an install that failed may be tried by itself again.
    #[must_use]
    pub fn install_back_off(&self, now_ms: u64) -> u64 {
        if self.install_failures == 0 {
            return 0;
        }
        let at = self.last_install_ms.unwrap_or(0).min(now_ms);
        at.saturating_add(retry_after(self.install_failures)).saturating_sub(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("recto-saved-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        local::ensure_private_dir(&dir).unwrap();
        dir
    }

    /// Failure: a damaged or tampered `updates.json` stops checks for good, or puts something other than a version into the menu's words.
    #[test]
    fn a_damaged_file_is_as_good_as_none() {
        let dir = folder("damaged");
        let file = dir.join("updates.json");
        assert_eq!(Saved::read(&file), Saved::default());
        let saved = Saved { last_check_ms: Some(5), offered: Some("0.2.0".into()), failed: vec!["0.1.5".into()], ..Saved::default() };
        saved.write(&file);
        assert_eq!(Saved::read(&file), saved);
        local::write_private_file(&file, br#"{"offered":"Update to <evil>","failed":["x","0.1.1"],"proven":"$(rm)","last_check_ms":7}"#).unwrap();
        let read = Saved::read(&file);
        assert_eq!(read.offered, None);
        assert_eq!(read.proven, None);
        assert_eq!(read.failed, vec!["0.1.1".to_string()]);
        assert_eq!(read.last_check_ms, Some(7));
        for junk in [&b"not json"[..], b"[]", &[0xFF, 0x00]] {
            local::write_private_file(&file, junk).unwrap();
            assert_eq!(Saved::read(&file), Saved::default());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: the list of failed versions grows without end, or a transient failure is retried at once, over and over.
    #[test]
    fn what_failed_is_bounded_and_backs_off() {
        let mut saved = Saved::default();
        for minor in 0..30 {
            saved.note_failed_version(&format!("0.{minor}.0"));
            saved.note_failed_version(&format!("0.{minor}.0"));
        }
        assert_eq!(saved.failed.len(), FAILED_KEPT);
        assert_eq!(saved.failed.last().map(String::as_str), Some("0.29.0"));
        assert_eq!(saved.install_back_off(100), 0);
        saved.install_failures = 1;
        saved.last_install_ms = Some(1_000);
        assert_eq!(saved.install_back_off(1_000), 3_600_000);
        assert_eq!(saved.install_back_off(1_000 + 3_600_000), 0);
    }
}
