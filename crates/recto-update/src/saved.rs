//! What's remembered between launches: `updates.json` in the state folder. A few dozen bytes, read when the app starts and written when something's happened. A damaged one is the same as none: the next check puts it right.

use crate::rules::{parse_version, retry_after};
use crate::version;
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
    /// Times in a row an install got as far as quitting the app and reopening it with nothing to show for it.
    pub quits_for_nothing: u32,
    /// Versions that were refused or didn't start. Never tried by itself again.
    pub failed: Vec<String>,
    /// The version that last reached its first frame. A launch of any other counts towards going back.
    pub proven: Option<String>,
    /// Launches of this version that haven't reached their first frame.
    pub unstarted: u32,
    /// The last update that went through, as it's reported.
    pub last_update: Option<LastUpdate>,
    /// *Update automatically*, if it's been chosen. Here and not with the session, so losing a record can't turn it back on against what the user chose.
    pub auto_update: Option<bool>,
    /// *Allow development builds*, if it's been chosen. Off until the owner turns it on: it's what lets a build that isn't a release replace the app.
    pub dev_builds: Option<bool>,
    /// Development builds were turned off while one was running, so the next release found goes in by itself, older or not, as the way back. Until a release is running, or they're turned on again.
    pub back_to_release: bool,
    /// A new version that's come up, watched for a while in case it then keeps ending: see [`crate::updater::EARLY_END_MS`].
    pub probation: Option<Probation>,
}

/// A version that's arrived by an update and come up, and is watched for runs that end early, for as long as the record from before it is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Probation {
    pub version: String,
    /// When it first came up. Watched from then for [`crate::updater::KEEP_RECORD_COPY_MS`].
    pub since_ms: u64,
    /// When the run under way started, until it's been up long enough or quit as asked. Still set at a launch, the last run ended early: a crash, a kill, a hang someone ended.
    pub running_since_ms: Option<u64>,
    /// Runs in a row that ended early.
    pub early_ends: u32,
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
    /// Reads it, keeping only what's well-formed. Nothing from this file reaches the screen or a command without passing here. There not being one is a first launch. One that's there and can't be read says nothing of what the user chose, so nothing's installed by itself until they say: a failing that errs towards not replacing the app under them.
    #[must_use]
    pub fn read(file: &Path) -> Self {
        let unreadable = Self {
            auto_update: Some(false),
            ..Self::default()
        };
        let bytes = match local::read_private_file(file, MAX_BYTES) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Self::default(),
            Err(_) => return unreadable,
        };
        // A list would read as all defaults otherwise, as if there were nothing there.
        if serde_json::from_slice::<serde_json::Value>(&bytes).map_or(true, |value| !value.is_object()) {
            return unreadable;
        }
        let Ok(mut saved) = serde_json::from_slice::<Self>(&bytes) else {
            return unreadable;
        };
        saved.offered = saved.offered.filter(|version| parse_version(version).is_some());
        // An offer is a release's. What ran, failed, was installed or is watched may be a development build's too.
        saved.proven = saved.proven.filter(|known| version::parse(known).is_some());
        saved.failed.retain(|known| version::parse(known).is_some());
        saved.probation = saved.probation.filter(|probation| version::parse(&probation.version).is_some());
        saved.last_update = saved
            .last_update
            .filter(|update| version::parse(&update.from).is_some() && version::parse(&update.to).is_some());
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

    fn folder(name: &str) -> recto_testpdf::TempDir {
        let dir = recto_testpdf::TempDir::new(&format!("recto-saved-{name}-"));
        local::ensure_private_dir(&dir).unwrap();
        dir
    }

    /// Failure: a damaged or tampered `updates.json` stops checks for good, or puts something other than a version into the menu's words or the version being watched.
    #[test]
    fn a_damaged_file_is_as_good_as_none() {
        let dir = folder("damaged");
        let file = dir.join("updates.json");
        assert_eq!(Saved::read(&file), Saved::default());
        let saved = Saved {
            last_check_ms: Some(5),
            offered: Some("0.2.0".into()),
            failed: vec!["0.1.5".into()],
            ..Saved::default()
        };
        saved.write(&file);
        assert_eq!(Saved::read(&file), saved);
        local::write_private_file(
            &file,
            br#"{"offered":"Update to <evil>","failed":["x","0.1.1","0.1.0.2"],"proven":"$(rm)","last_check_ms":7,"probation":{"version":"$(rm)","early_ends":9}}"#,
        )
        .unwrap();
        let read = Saved::read(&file);
        assert_eq!(read.offered, None);
        assert_eq!(read.proven, None);
        assert_eq!(read.probation, None, "nothing's watched that isn't a version");
        assert_eq!(
            read.failed,
            vec!["0.1.1".to_string(), "0.1.0.2".to_string()],
            "a development build can fail too"
        );
        assert_eq!(read.last_check_ms, Some(7));
        for junk in [&b"not json"[..], b"[]", &[0xFF, 0x00]] {
            local::write_private_file(&file, junk).unwrap();
            let read = Saved::read(&file);
            assert_eq!((read.last_check_ms, read.offered), (None, None));
            assert_eq!(
                read.auto_update,
                Some(false),
                "what the user chose is unknown, so nothing's installed by itself"
            );
            assert_eq!(read.dev_builds, None, "and development builds stay off");
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
