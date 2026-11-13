//! **An update, with no server, no zip, no signature and no second process** (step R1.4): what the headless app is given in place of the release server and the
//! installer, so an update's rules run on the app's own core, with its real look at what's in flight, its real hand-off (the freeze, the write and the read back)
//! and its real restore after it.
//!
//! - [`FakeReleases`] says what's on offer and what making it ready comes to, in words: no download, and nothing unpacked but a stand-in app beside this one.
//! - [`FakeInstaller`] keeps the plan it was started on, or can't be started.
//! - [`Disk::install_app`] puts a stand-in `RectoPDF.app` of a version where the app is. [`Disk::install`] does what the installer would have done once the old app
//!   was gone (swapped, killed after the swap, gone back, not installed), with the real record copy and restore, and leaves the plan or outcome it would leave.
//!   [`Disk::start`] after it is the launch that comes up on it.
//!
//! What only the real thing has stays in `recto-cli`'s `restore::update` tests: the real zip, codesign, the installer process, the swap on a real volume, Launch
//! Services and the window.

use super::FakeClock;
use super::headless::Disk;
use crate::clock::Clock;
use crate::core::updates::{Installer, UpdateSeams};
use recto_update::fetch::FetchError;
use recto_update::plan::{Files, Outcome, OutcomeKind, Plan, beside_app, new_app_in, previous_app_in};
use recto_update::rules::Latest;
use recto_update::source::Source;
use recto_update::stage::{Stage, StageError, Staged};
use recto_update::updater::{Releases, Seams, WallClock};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The Mac this disk is on is this version: the stand-in app's `Info.plist`, its program and its installer.
pub const OLD: &str = "0.1.0";
/// A release newer than [`OLD`].
pub const NEW: &str = "0.1.99";
/// A development build of [`OLD`].
pub const DEV: &str = "0.1.0.1";

/// What a stand-in app is made of. Its programs are empty files: nothing runs them.
fn write_app(app: &Path, version: &str) {
    let parsed = recto_update::version::parse(version).expect("a version");
    let (release, development) = if parsed.is_development() {
        let (a, b, c) = parsed.release;
        (format!("{a}.{b}.{c}"), Some(version))
    } else {
        (version.to_owned(), None)
    };
    let dev_key = development.map_or_else(String::new, |dev| {
        format!("<key>{}</key><string>{dev}</string>", recto_update::version::DEV_VERSION_KEY)
    });
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).expect("a stand-in app's folders");
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{}</string><key>CFBundleExecutable</key><string>RectoPDF</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>{release}</string>{dev_key}</dict></plist>"#,
            recto_update::BUNDLE_ID
        ),
    )
    .expect("a stand-in app's Info.plist");
    std::fs::write(macos.join("RectoPDF"), b"").expect("a stand-in app's program");
    std::fs::write(app.join(recto_update::HELPER), b"").expect("a stand-in app's installer");
}

/// What a release looks like when there's one on offer: any checksum will do, as nothing's downloaded.
fn latest(version: &str) -> Latest {
    Latest {
        version: version.to_owned(),
        sha256: "a".repeat(64),
        minimum_macos: (15, 0, 0),
    }
}

/// **The release server and the zip's checks, as words.** Nothing on offer until [`Self::offer`].
#[derive(Debug)]
pub struct FakeReleases {
    latest: Mutex<Result<Latest, FetchError>>,
    /// What making a release or a build ready comes to: ready, or this refusal.
    refusal: Mutex<Option<StageError>>,
}

impl Default for FakeReleases {
    fn default() -> Self {
        Self {
            latest: Mutex::new(Err(FetchError::NotFound)),
            refusal: Mutex::new(None),
        }
    }
}

impl FakeReleases {
    /// This version is the latest release.
    pub fn offer(&self, version: &str) {
        *crate::lock(&self.latest) = Ok(latest(version));
    }

    /// The release's description is this, or can't be had.
    pub fn answer(&self, answer: Result<Latest, FetchError>) {
        *crate::lock(&self.latest) = answer;
    }

    /// Making what's on offer ready is refused, as the real checks refuse a wrong checksum, a damaged zip or a signature that isn't the team's.
    pub fn refuse(&self, code: &'static str, why: &str, lasting: bool) {
        *crate::lock(&self.refusal) = Some(StageError {
            code,
            why: why.to_owned(),
            lasting,
        });
    }

    /// Making it ready works again.
    pub fn accept(&self) {
        *crate::lock(&self.refusal) = None;
    }

    /// A stand-in of `version` beside the app, where the real checks would have unpacked it, after the real look at whether the app can be replaced where it is.
    fn ready(&self, app: &Path, version: &str) -> Result<Staged, StageError> {
        if let Some(refusal) = crate::lock(&self.refusal).clone() {
            return Err(refusal);
        }
        let beside = recto_update::stage::check_place(app)?;
        let new_app = new_app_in(&beside);
        let _ = std::fs::remove_dir_all(beside.join("new"));
        write_app(&new_app, version);
        Ok(Staged {
            app: new_app,
            cdhash: "0".repeat(40),
            team: None,
        })
    }
}

impl Releases for FakeReleases {
    fn latest(&self, _source: &Source, _user_agent: &str) -> Result<Latest, FetchError> {
        crate::lock(&self.latest).clone()
    }

    fn stage(&self, stage: &Stage<'_>, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError> {
        say("a test's release, ready at once");
        self.ready(stage.app, &stage.latest.version)
    }

    fn stage_build(&self, _zip: &mut std::fs::File, version: &str, _source: &Source, app: &Path, say: &mut dyn FnMut(&str)) -> Result<Staged, StageError> {
        say("a test's build, ready at once");
        self.ready(app, version)
    }
}

/// **The installer, as far as the app sees it:** started on a plan, which it keeps for the test, or not started at all.
#[derive(Debug, Default)]
pub struct FakeInstaller {
    plans: Mutex<Vec<Plan>>,
    fails: AtomicBool,
}

impl FakeInstaller {
    /// From now on it can't be started, as an installer that isn't executable can't.
    pub fn fail_to_start(&self) {
        self.fails.store(true, Ordering::SeqCst);
    }

    /// The plans it was started on, in order.
    #[must_use]
    pub fn plans(&self) -> Vec<Plan> {
        crate::lock(&self.plans).clone()
    }
}

impl Installer for FakeInstaller {
    fn start(&self, _helper: &Path, plan: &Path, _log: &Path) -> std::io::Result<u32> {
        if self.fails.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("a test's installer that can't be started"));
        }
        let read = Plan::read(plan).ok_or_else(|| std::io::Error::other("the plan it was started on can't be read"))?;
        crate::lock(&self.plans).push(read);
        Ok(4242)
    }
}

/// The time of day on a test's disk: where it was at this launch, moved on by the app's fake clock.
struct FakeWall {
    at_start: u64,
    clock: Arc<FakeClock>,
}

impl WallClock for FakeWall {
    fn wall_ms(&self) -> u64 {
        self.at_start.saturating_add(self.clock.now_ms())
    }
}

/// **The release server and the installer, for every launch on one disk.** Clone it into each [`super::headless::Builder::update`]; what it was told holds across them.
#[derive(Clone, Debug, Default)]
pub struct UpdateBed {
    pub releases: Arc<FakeReleases>,
    pub installer: Arc<FakeInstaller>,
}

impl UpdateBed {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The seams a launch is started with: this bed's releases and installer, and the disk's time of day moved on by `clock`.
    pub(crate) fn seams(&self, wall_at_start: u64, clock: &Arc<FakeClock>) -> UpdateSeams {
        UpdateSeams {
            updater: Seams {
                wall: Arc::new(FakeWall {
                    at_start: wall_at_start.saturating_sub(clock.now_ms()),
                    clock: Arc::clone(clock),
                }),
                releases: Arc::clone(&self.releases) as Arc<dyn Releases>,
            },
            installer: Arc::clone(&self.installer) as Arc<dyn Installer>,
        }
    }

    /// A test's source: a test build's, so it replaces itself as a release does, at a server on this machine that's never asked, as the releases are this bed's.
    pub(crate) fn source() -> Source {
        recto_update::source::resolve(false, true, Some("http://127.0.0.1:9".to_owned()), None)
    }
}

/// What the installer did once the old app was gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installed {
    /// The new version's in place, the old one kept, and the installer's waiting for it to come up: the plan's still there for the launch to read.
    Swapped,
    /// Killed straight after the swap: the new version's in place, the old one still where the swap left it, and the plan left behind.
    KilledAfterSwap,
    /// The new version didn't come up, so the old one's back, with the record put back unless the plan said keep it, and the outcome says why: [`Self::Swapped`]
    /// and then [`Disk::went_back`], with nothing run between.
    WentBack(&'static str),
    /// Nothing was replaced, and the outcome says why.
    NotInstalled(&'static str),
}

impl Disk {
    /// The app's bundle on this disk, where an installed `RectoPDF.app` is.
    #[must_use]
    pub fn app(&self) -> PathBuf {
        self.files().parent().expect("the disk's folder").join("install").join(recto_update::APP_BUNDLE)
    }

    /// Puts a stand-in app of `version` where the app is, as an install from a disk image would.
    pub fn install_app(&self, version: &str) {
        let _ = std::fs::remove_dir_all(self.app());
        write_app(&self.app(), version);
    }

    /// The version the app where it's installed says it is.
    #[must_use]
    pub fn app_version(&self) -> Option<String> {
        recto_update::stage::app_version(&self.app())
    }

    /// The version kept beside the app to go back to, if there is one.
    #[must_use]
    pub fn kept_version(&self) -> Option<String> {
        recto_update::stage::app_version(&previous_app_in(&beside_app(&self.app())?))
    }

    /// The updater's own files in this disk's state folder.
    #[must_use]
    pub fn update_files(&self) -> Files {
        Files::in_state(&self.paths().root)
    }

    /// **What the installer did with `plan` once the old app was gone,** as [`Installed`] says, with the real record copy and restore.
    ///
    /// # Panics
    ///
    /// If the swap the installer makes can't be made in the temp folder.
    pub fn install(&self, plan: &Plan, installed: Installed) {
        let files = self.update_files();
        let beside = beside_app(&self.app()).expect("the app is in a folder");
        let swap = || {
            recto_update::install::copy_record(&files.session(), &files.before(), &mut |_| {});
            recto_platform::local::swap_paths(&new_app_in(&beside), &self.app()).expect("the swap");
        };
        match installed {
            Installed::Swapped => {
                swap();
                let kept = previous_app_in(&beside);
                let _ = std::fs::remove_dir_all(kept.parent().expect("the kept app's folder"));
                std::fs::create_dir_all(kept.parent().expect("the kept app's folder")).expect("the kept app's folder");
                std::fs::rename(new_app_in(&beside), &kept).expect("the old app kept");
            }
            Installed::KilledAfterSwap => swap(),
            Installed::WentBack(why) => {
                self.install(plan, Installed::Swapped);
                self.went_back(plan, why);
            }
            Installed::NotInstalled(why) => self.outcome(plan, OutcomeKind::NotInstalled, why),
        }
    }

    /// **The installer, having seen the new version come up** (its launch-ok, read as started): its own files cleared and what it staged gone, as `install::run`
    /// ends an update that went well.
    pub fn installer_saw_it_up(&self) {
        let files = self.update_files();
        for file in [files.plan(), files.launch_ok(), files.launch_failed(), files.helper_pid()] {
            let _ = std::fs::remove_file(file);
        }
        if let Some(beside) = beside_app(&self.app()) {
            let _ = std::fs::remove_dir_all(beside.join("new"));
            let _ = std::fs::remove_file(beside.join("staged.json"));
        }
    }

    /// **The installer going back** once the new version, swapped in by [`Installed::Swapped`] and perhaps run since, didn't come up: the record from before
    /// put back unless the plan said keep it, the kept app swapped back where the app is, the one that didn't work gone, and the outcome saying why.
    ///
    /// # Panics
    ///
    /// If there's no kept app to go back to.
    pub fn went_back(&self, plan: &Plan, why: &str) {
        let files = self.update_files();
        let beside = beside_app(&self.app()).expect("the app is in a folder");
        if !plan.keep_record {
            recto_update::install::restore_record(&files.before(), &files.session());
        }
        let kept = previous_app_in(&beside);
        recto_platform::local::swap_paths(&kept, &self.app()).expect("the kept app swapped back");
        let _ = std::fs::remove_dir_all(kept.parent().expect("the kept app's folder"));
        let _ = std::fs::remove_dir_all(beside.join("new"));
        let _ = std::fs::remove_file(beside.join("staged.json"));
        self.outcome(plan, OutcomeKind::WentBack, why);
    }

    /// What a test's broken release does, put in the installed app as the release would carry it ([`recto_update::TestBreakage`]).
    pub fn break_app(&self, what: recto_update::TestBreakage) {
        let resources = self.app().join("Contents/Resources");
        std::fs::create_dir_all(&resources).expect("the app's resources");
        std::fs::write(resources.join(what.file()), b"").expect("the breakage's file");
    }

    /// **A development build handed over** as `recto update install <zip>` hands it over: a `RectoPDF-<version>.zip` of the test's, brought into the app's
    /// hand-over folder by the same call `recto` makes. Says the copy's path, for the command. Its contents are nothing: [`FakeReleases`] checks it.
    ///
    /// # Panics
    ///
    /// If the copy can't be brought in.
    #[must_use]
    pub fn hand_over_build(&self, version: &str) -> PathBuf {
        let zip = self.file(&format!("RectoPDF-{version}.zip"), b"PK a test's build");
        let folder = recto_update::handover::folder_of(&self.update_files());
        recto_platform::local::ensure_private_dir(&folder).expect("the hand-over folder");
        recto_update::handover::bring_in(&zip, &folder, recto_update::handover::WAITS, &AtomicBool::new(false), &mut |_| {}).expect("the build brought in")
    }

    /// The installer's last word, and the plan taken away, as it leaves them.
    fn outcome(&self, plan: &Plan, kind: OutcomeKind, why: &str) {
        let files = self.update_files();
        Outcome {
            kind,
            from: plan.from.clone(),
            to: plan.to.clone(),
            why: why.to_owned(),
        }
        .write(&files.outcome())
        .expect("the outcome written");
        let _ = std::fs::remove_file(files.plan());
    }
}
