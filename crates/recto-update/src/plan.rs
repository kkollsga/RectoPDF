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
        Self {
            root: root.to_path_buf(),
            saved: root.join("updates.json"),
            log: root.join("update.log"),
            work: root.join("update"),
        }
    }

    #[must_use]
    pub fn plan(&self) -> PathBuf {
        self.work.join("plan.json")
    }

    #[must_use]
    pub fn outcome(&self) -> PathBuf {
        self.work.join("outcome.json")
    }

    /// What the new app writes once it's come up: see [`crate::updater::Updater::first_frame`]. What the installer's waiting for.
    #[must_use]
    pub fn launch_ok(&self) -> PathBuf {
        self.work.join("launch-ok")
    }

    /// What the new app writes if it knows it hasn't come up, so the installer needn't wait to find out.
    #[must_use]
    pub fn launch_failed(&self) -> PathBuf {
        self.work.join("launch-failed")
    }

    /// One installer at a time. Held for its whole life, and the system lets go if it's killed.
    #[must_use]
    pub fn installer_lock(&self) -> PathBuf {
        self.work.join("installer.lock")
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
    /// Which install this is. An installer whose plan on disk has another is out of date, and stands down.
    pub id: String,
    pub from: String,
    pub to: String,
    /// The signature's hash of what was verified, so the installer knows it's swapping in those bytes and no others, however long it's been.
    pub cdhash: String,
    /// A test holding this to the real rule. A real build's installer holds to it whatever this says.
    #[serde(default)]
    pub team_required: bool,
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
    /// How many tabs the old app handed off. A new version that comes up with fewer has misread the record, as no tab is dropped by a restore, and goes back with the record from before it.
    #[serde(default)]
    pub handed_off_tabs: Option<u32>,
    /// Going back leaves the record as it is: a version that's been in use since it came up and then kept ending has the user's latest work in it, which the previous version reads, or leaves alone if it's of a newer kind.
    #[serde(default)]
    pub keep_record: bool,
    #[serde(default)]
    pub test: Option<TestLaunch>,
}

/// What a new version writes to `launch-ok` once it's come up: its version, process, the time, and how many tabs it restored, if it's read the record by then.
#[must_use]
pub fn launch_ok_line(version: &str, pid: u32, shown_at_ms: u64, tabs: Option<u32>) -> String {
    match tabs {
        Some(tabs) => format!("{version} {pid} {shown_at_ms} tabs={tabs}"),
        None => format!("{version} {pid} {shown_at_ms}"),
    }
}

/// What a `launch-ok` says: the version, and the tabs it restored if it says. Anything else in it is passed over.
#[must_use]
pub fn read_launch_ok(text: &str) -> Option<(&str, Option<u32>)> {
    let mut words = text.split_whitespace();
    let version = words.next()?;
    let tabs = words.find_map(|word| word.strip_prefix("tabs=")?.parse().ok());
    Some((version, tabs))
}

/// Whether a new version that's come up has misread the record: it says it restored fewer tabs than were handed off. One that doesn't say, as it showed a frame before the record was read, isn't taken to have.
#[must_use]
pub fn misread(handed_off: Option<u32>, restored: Option<u32>) -> bool {
    matches!((handed_off, restored), (Some(handed_off), Some(restored)) if restored < handed_off)
}

impl Plan {
    /// Reads it, and it has to make sense. A plan that doesn't is no plan, and nothing's moved for it.
    #[must_use]
    pub fn read(file: &Path) -> Option<Self> {
        let bytes = local::read_private_file(file, MAX_PLAN_BYTES).ok()??;
        let plan: Self = serde_json::from_slice(&bytes).ok()?;
        let cdhash_fine = plan.cdhash.is_empty() || (plan.cdhash.len() == 40 && plan.cdhash.bytes().all(|byte| byte.is_ascii_hexdigit()));
        (!plan.id.is_empty()
            && cdhash_fine
            && parse_version(&plan.from).is_some()
            && parse_version(&plan.to).is_some()
            && plan.app.is_absolute()
            && plan.beside.is_absolute()
            && plan.root.is_absolute())
        .then_some(plan)
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

/// A name for this install, that a later one can't have.
#[must_use]
pub fn new_id() -> String {
    format!("{}-{}", std::process::id(), crate::wall_ms())
}

/// The hidden folder beside the app. Beside it, so the swap is on one volume. A dot, so it's not one of the things in Applications, and private, so nobody else can change what's waiting there.
#[must_use]
pub fn beside_app(app: &Path) -> Option<PathBuf> {
    Some(app.parent()?.join(".RectoPDF-update"))
}

/// What the staged new app is called while it waits. **Not `.app`, nor is the kept one:** Launch Services takes any folder called that as an app, and a scan of the app's folder registered a copy in this hidden one (`lsregister -R -f`, 2026-11-11), which puts a second RectoPDF in Open With, and lets the system open a PDF with a version that isn't the installed one, as it prefers the newest. A bundle's signature and Gatekeeper go by what's in it, not its name (checked the same day on a notarized app renamed so), and the swap and going back rename it to the app's own name.
pub const STAGED_BUNDLE: &str = "RectoPDF.staged";
/// What the previous app is called while it's kept: see [`STAGED_BUNDLE`].
pub const KEPT_BUNDLE: &str = "RectoPDF.kept";

#[must_use]
pub fn new_app_in(beside: &Path) -> PathBuf {
    beside.join("new").join(STAGED_BUNDLE)
}

#[must_use]
pub fn previous_app_in(beside: &Path) -> PathBuf {
    beside.join("previous").join(KEPT_BUNDLE)
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
    /// The new version was in place and didn't start, and there was nothing to go back to, or it couldn't be put back.
    CouldntGoBack,
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
    let Ok(Some(bytes)) = local::read_private_file(&files.helper_pid(), 32) else {
        return false;
    };
    let Some(pid) = std::str::from_utf8(&bytes).ok().and_then(|text| text.trim().parse::<u32>().ok()) else {
        return false;
    };
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
        let plan = Plan {
            id: "7-1".into(),
            cdhash: "a".repeat(40),
            team_required: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 7,
            app: "/a/RectoPDF.app".into(),
            beside: "/a/.RectoPDF-update".into(),
            root: "/s".into(),
            quit_at_ms: 1,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            handed_off_tabs: None,
            keep_record: false,
            test: None,
        };
        plan.write(&file).unwrap();
        assert_eq!(Plan::read(&file), Some(plan.clone()));
        for wrong in [
            Plan {
                to: "2".into(),
                ..plan.clone()
            },
            Plan {
                from: "x".into(),
                ..plan.clone()
            },
            Plan {
                app: "relative".into(),
                ..plan.clone()
            },
            Plan {
                id: String::new(),
                ..plan.clone()
            },
            Plan {
                cdhash: "\"or anything".into(),
                ..plan.clone()
            },
        ] {
            wrong.write(&file).unwrap();
            assert_eq!(Plan::read(&file), None);
        }
        local::write_private_file(&file, b"not json").unwrap();
        assert_eq!(Plan::read(&file), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a new version that misread the record and started with fewer tabs than were handed off counts as having come up, and stays, with the user's tabs gone. Or one that showed a frame before the record was read is sent back for having none. A `launch-ok` from before it said its tabs still reads.
    #[test]
    fn fewer_tabs_than_were_handed_off_is_a_misread() {
        let line = launch_ok_line("0.2.0", 9, 5, Some(3));
        assert_eq!(read_launch_ok(&line), Some(("0.2.0", Some(3))));
        assert_eq!(read_launch_ok("0.2.0 9 5"), Some(("0.2.0", None)), "it doesn't say");
        assert_eq!(read_launch_ok("0.2.0 9 5 tabs=x"), Some(("0.2.0", None)));
        assert_eq!(read_launch_ok(""), None);
        assert!(misread(Some(3), Some(0)), "started empty");
        assert!(misread(Some(3), Some(2)));
        assert!(!misread(Some(3), Some(3)));
        assert!(!misread(Some(3), Some(4)), "a file opened as it came up");
        assert!(!misread(Some(3), None), "shown before the record was read");
        assert!(!misread(None, Some(0)), "an old app that didn't say");
    }
}
