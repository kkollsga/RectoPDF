//! What the app hands the installer, and what the installer leaves for the next launch. Files in the state folder's `update` folder, all private, each written whole.

use crate::rules::parse_version;
use recto_platform::local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_PLAN_BYTES: u64 = 64 * 1024;
const MAX_LOG_BYTES: u64 = 256 * 1024;

/// How long the installer waits for the old app to end. It's been told to quit with nothing to ask.
pub const QUIT_LIMIT_MS: u64 = 30_000;
/// How long the installer waits for the new app's first frame. A first launch after an update is checked by the system, which can take a while on a slow Mac.
pub const START_LIMIT_MS: u64 = 120_000;

/// Where the updater's files go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    /// The state folder.
    pub root: PathBuf,
    pub saved: PathBuf,
    pub log: PathBuf,
    /// Its own folder.
    pub work: PathBuf,
}

impl Files {
    #[must_use]
    pub fn in_state(root: &Path) -> Self {
        Self { root: root.to_path_buf(), saved: root.join("updates.json"), log: root.join("update.log"), work: root.join("update") }
    }

    #[must_use]
    pub fn plan(&self) -> PathBuf {
        self.work.join("plan.json")
    }

    #[must_use]
    pub fn outcome(&self) -> PathBuf {
        self.work.join("outcome.json")
    }

    /// What the new app writes at its first frame, which is what the installer's waiting for.
    #[must_use]
    pub fn launch_ok(&self) -> PathBuf {
        self.work.join("launch-ok")
    }

    #[must_use]
    pub fn helper_pid(&self) -> PathBuf {
        self.work.join("helper.pid")
    }

    /// The record as it was before the new app ran, to go back to.
    #[must_use]
    pub fn before(&self) -> PathBuf {
        self.work.join("before")
    }

    /// The record's folder.
    #[must_use]
    pub fn session(&self) -> PathBuf {
        self.root.join("session")
    }

    #[must_use]
    pub fn zip(&self, version: &str) -> PathBuf {
        self.work.join(crate::rules::zip_name(version))
    }
}

/// What only a test's launch has. A release build's installer doesn't look at this.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestLaunch {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Starts the executable itself rather than going through `open`. For a test's stand-in app, which isn't a real one to the system.
    pub direct: bool,
}

/// One install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub from: String,
    pub to: String,
    /// The one process to wait for.
    pub pid: u32,
    pub app: PathBuf,
    /// The hidden folder beside the app, with the new one and the kept one in it.
    pub beside: PathBuf,
    /// The state folder.
    pub root: PathBuf,
    /// When the old app was told to quit.
    pub quit_at_ms: u64,
    /// The app wasn't frontmost, so the new one doesn't take the focus from anyone.
    pub background: bool,
    pub quit_limit_ms: u64,
    pub start_limit_ms: u64,
    #[serde(default)]
    pub test: Option<TestLaunch>,
}

impl Plan {
    /// Reads it, and it has to make sense. A plan that doesn't is no plan, and nothing's moved for it.
    #[must_use]
    pub fn read(file: &Path) -> Option<Self> {
        let bytes = local::read_private_file(file, MAX_PLAN_BYTES).ok()??;
        let plan: Self = serde_json::from_slice(&bytes).ok()?;
        (parse_version(&plan.from).is_some() && parse_version(&plan.to).is_some() && plan.app.is_absolute() && plan.beside.is_absolute() && plan.root.is_absolute()).then_some(plan)
    }

    pub fn write(&self, file: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        local::write_private_file_durably(file, &bytes)
    }

    #[must_use]
    pub fn new_app(&self) -> PathBuf {
        new_app_in(&self.beside)
    }

    #[must_use]
    pub fn previous_app(&self) -> PathBuf {
        previous_app_in(&self.beside)
    }

    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.app.join(crate::APP_EXECUTABLE)
    }
}

/// The hidden folder beside the app. Beside it, so the swap is on one volume. A dot, so it's not one of the things in Applications.
#[must_use]
pub fn beside_app(app: &Path) -> Option<PathBuf> {
    Some(app.parent()?.join(".RectoPDF-update"))
}

#[must_use]
pub fn new_app_in(beside: &Path) -> PathBuf {
    beside.join("new").join(crate::APP_BUNDLE)
}

#[must_use]
pub fn previous_app_in(beside: &Path) -> PathBuf {
    beside.join("previous").join(crate::APP_BUNDLE)
}

/// What the installer has to say that isn't the new app coming up: what the next launch of the old app reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub kind: OutcomeKind,
    pub from: String,
    pub to: String,
    /// Why, as a code of the installer's own.
    pub why: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// Nothing was replaced.
    NotInstalled,
    /// The new version was in place and didn't start, so the previous one came back.
    WentBack,
}

impl Outcome {
    #[must_use]
    pub fn read(file: &Path) -> Option<Self> {
        let bytes = local::read_private_file(file, MAX_PLAN_BYTES).ok()??;
        let outcome: Self = serde_json::from_slice(&bytes).ok()?;
        (parse_version(&outcome.from).is_some() && parse_version(&outcome.to).is_some()).then_some(outcome)
    }

    pub fn write(&self, file: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        local::write_private_file_durably(file, &bytes)
    }
}

/// A line in the update log. Time and words of our own: nothing a server or a document said. Keeps one older file, so it can't grow.
pub fn log(file: &Path, line: &str) {
    use std::io::Write as _;
    if std::fs::metadata(file).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(file, file.with_extension("1.log"));
    }
    if let Ok(mut out) = local::append_private_file(file) {
        let _ = writeln!(out, "{} {line}", crate::wall_ms());
    }
}

/// The installer's own process, if it's still running. Going by the program too, as a number comes round again.
#[must_use]
pub fn helper_alive(files: &Files) -> bool {
    let Ok(Some(bytes)) = local::read_private_file(&files.helper_pid(), 32) else { return false };
    let Some(pid) = std::str::from_utf8(&bytes).ok().and_then(|text| text.trim().parse::<u32>().ok()) else { return false };
    local::process_path(pid).is_some_and(|path| path.file_name().is_some_and(|name| name == "recto-updater"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a damaged or nonsense plan gets acted on, so a broken file could move something.
    #[test]
    fn a_plan_that_doesnt_make_sense_is_no_plan() {
        let dir = std::env::temp_dir().join(format!("recto-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        local::ensure_private_dir(&dir).unwrap();
        let file = dir.join("plan.json");
        assert_eq!(Plan::read(&file), None);
        let plan = Plan { from: "0.1.0".into(), to: "0.2.0".into(), pid: 7, app: "/a/RectoPDF.app".into(), beside: "/a/.RectoPDF-update".into(), root: "/s".into(), quit_at_ms: 1, background: false, quit_limit_ms: 1, start_limit_ms: 1, test: None };
        plan.write(&file).unwrap();
        assert_eq!(Plan::read(&file), Some(plan.clone()));
        for wrong in [Plan { to: "2".into(), ..plan.clone() }, Plan { from: "x".into(), ..plan.clone() }, Plan { app: "relative".into(), ..plan.clone() }] {
            wrong.write(&file).unwrap();
            assert_eq!(Plan::read(&file), None);
        }
        local::write_private_file(&file, b"not json").unwrap();
        assert_eq!(Plan::read(&file), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
