//! **Development builds** (SPEC decision 24's development builds, with Seedwell's §12A as the model). Owner, 2026-11-11: "We should only ever have one. We should enable dev updates (similar to the seedwell feature)."
//!
//! *Allow development builds* is off until the owner turns it on, and it's kept in `updates.json` with *Update automatically*. With it on, a development build (`X.Y.Z.N`, [`crate::version`]) handed over as a zip, `recto update install <zip>`, goes in through the same hand-off as a release, with the same exact state, the old app kept and the way back if it doesn't come up. It's checked first: our own zip reader, an `X.Y.Z.N` version, this app's bundle id, and a whole signature naming this app and its helper. Ad hoc, as dev builds never touch the keychain, so not the team's, and not Gatekeeper, as a build isn't notarized. It needs agent access Full, as anyone can sign ad hoc. Then it waits until the app hasn't been used for 5 minutes (owner, 2026-11-11: "for dev work, lets set the limit to 5 mins"), or goes at once with `--now`. With it off, only releases go in, and turning it off while a build runs makes the next release found the way back, older or not, by the 30-minute rule and whichever way *Update automatically* is set.
//!
//! What's waiting is held in memory: a quit before its moment drops it, and the next launch tidies what was staged.

use super::{Asked, Going, InstallRefused, Status, Updater, lock};
use crate::rules::{self, AutoStep};
use crate::saved::Saved;
use crate::stage::{self, StageError};
use crate::version;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// A build that's been checked and is waiting beside the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Build {
    pub(super) version: String,
    /// The signature's hash of what was checked, for the installer's last look.
    pub(super) cdhash: String,
    /// The team that signed it, if one did: most are ad hoc.
    pub(super) team: Option<String>,
}

/// What's said when an update's arrived: for a development build, how it was signed, as one signed ad hoc may have macOS ask again for folders it was given.
pub(super) fn arrived_words(plan: &crate::plan::Plan) -> String {
    match (plan.development_build, plan.team_signed) {
        (false, _) => format!("Updated to {}.", plan.to),
        (true, true) => format!("Updated to development build {}, signed by RectoPDF's developer.", plan.to),
        (true, false) => format!(
            "Updated to development build {}, signed on this Mac, not by RectoPDF's developer: macOS may ask again for access to folders.",
            plan.to
        ),
    }
}

/// The way back from a development build, once development builds have been turned off: a release other than what's running, older included.
pub(super) fn is_way_back(saved: &Saved, offered: &str, current: &str) -> bool {
    saved.back_to_release && version::is_development(current) && offered != current && rules::parse_version(offered).is_some()
}

/// The version a zip handed over is of, by its name, as `tools/dev-build.sh` writes it: `RectoPDF-X.Y.Z.N.zip`, a development build's. A release's number isn't one: a release comes from the releases, held to the team and Gatekeeper, and never as a zip handed over and signed ad hoc.
#[must_use]
pub fn version_of_zip(name: &str) -> Option<String> {
    let inner = name.strip_prefix("RectoPDF-")?.strip_suffix(".zip")?;
    version::is_development(inner).then(|| inner.to_owned())
}

impl Updater {
    /// Whether *Allow development builds* is on. Off until it's chosen.
    #[must_use]
    pub fn dev_builds_on(&self) -> bool {
        lock(&self.st).saved.dev_builds.unwrap_or(false)
    }

    /// Whether this copy takes a development build at all: a build that replaces itself, with the switch on. Not consent for an agent: `recto update install <zip>` needs agent access Full whatever this says (SPEC decision 28), as anyone can sign a build ad hoc.
    #[must_use]
    pub fn takes_builds(&self) -> bool {
        self.source.may_replace_itself() && self.dev_builds_on()
    }

    /// Whether Settings shows *Allow development builds*: on a development build, and while it's on, so it can be turned off. A release that's never had one doesn't show it: the owner's app shows it from the first dev build on (`tools/dev-build.sh --first-install`), and `recto update dev-builds on` works at Full whatever's shown.
    #[must_use]
    pub fn shows_dev_builds_switch(&self) -> bool {
        let st = lock(&self.st);
        st.saved.dev_builds == Some(true) || st.facts.as_ref().is_some_and(|facts| version::is_development(&facts.version))
    }

    /// *Allow development builds* was flipped, in Settings or by `recto update dev-builds`. It holds at once. Off drops a build that's waiting, and on a development build starts the way back to the release: a check now, and the release it finds goes in by itself after 30 quiet minutes, older or not. On ends the way back. Written on a thread of its own, as this may be the UI thread.
    pub fn choose_dev_builds(self: &Arc<Self>, on: bool) {
        let now = self.now_ms();
        let (dropped, check) = {
            let mut st = lock(&self.st);
            st.saved.dev_builds = Some(on);
            let on_a_build = st.facts.as_ref().is_some_and(|facts| version::is_development(&facts.version));
            if on {
                st.saved.back_to_release = false;
                (false, false)
            } else {
                let dropped = !st.status.is_under_way() && st.build.take().is_some();
                if dropped {
                    st.status = Status::Idle;
                    st.install_look_at = None;
                }
                st.saved.back_to_release = on_a_build;
                if on_a_build && self.offered_here(&st).is_some() {
                    st.install_look_at = Some(now);
                }
                (dropped, on_a_build && self.source.checks_by_itself())
            }
        };
        let me = Arc::clone(self);
        let app = self.app();
        let written = std::thread::Builder::new()
            .name("recto-update-save".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                if dropped && let Some(app) = app {
                    stage::clear_staged(&app);
                }
                me.save();
            });
        if written.is_err() {
            self.save();
        }
        self.log(&format!("development builds turned {}", if on { "on" } else { "off" }));
        if check {
            self.check_in_background(Asked::ByItself);
        }
        self.host.changed();
    }

    /// A development build handed over: `recto update install <zip> [--now]`. Checks it here, which takes seconds, so never on the UI thread: a command's job thread is where it's meant to be. Then it waits for 5 minutes without use, or with `now` goes at once, by hand. Another build handed over while one waits takes its place.
    pub fn install_build(self: &Arc<Self>, zip: &Path, now: bool) -> Result<String, InstallRefused> {
        self.install_build_then(zip, now, |_| {})
    }

    /// [`Self::install_build`], with what happens while it's checked, for a test of what's looked at again after.
    fn install_build_then(self: &Arc<Self>, zip: &Path, now: bool, meanwhile: impl FnOnce(&Arc<Self>)) -> Result<String, InstallRefused> {
        if !self.source.may_replace_itself() {
            return Err(InstallRefused::NotInThisBuild);
        }
        let version = zip
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(version_of_zip)
            .ok_or(InstallRefused::NotABuild)?;
        let facts = {
            let mut st = lock(&self.st);
            if !st.saved.dev_builds.unwrap_or(false) {
                return Err(InstallRefused::DevBuildsOff);
            }
            if st.second_copy {
                return Err(InstallRefused::AnotherCopy);
            }
            if st.status.is_under_way() {
                return Err(InstallRefused::Underway);
            }
            let facts = st.facts.clone().ok_or(InstallRefused::Underway)?;
            if facts.version == version {
                return Err(InstallRefused::SameVersion);
            }
            st.build = None;
            st.install_look_at = None;
            st.status = Status::Preparing(version.clone());
            facts
        };
        self.stopping.store(false, Ordering::SeqCst);
        self.host.changed();
        self.ready_folder();
        self.log(&format!("development build {version} handed over"));
        let log_file = self.files.log.clone();
        let staged = stage::prepare_build(zip, &version, &self.source, &facts.app, &mut |step| {
            crate::plan::log(&log_file, &format!("{version}: {step}"));
        });
        meanwhile(self);
        // The switch is asked again once the slow part's done: turned off meanwhile, the build goes, and nothing's left staged.
        let staged = match staged {
            Ok(_) if !self.dev_builds_on() => {
                stage::clear_staged(&facts.app);
                lock(&self.st).status = Status::Idle;
                self.log(&format!(
                    "development build {version} dropped: development builds were turned off while it was checked"
                ));
                self.host.changed();
                return Err(InstallRefused::DevBuildsOff);
            }
            Ok(staged) => staged,
            Err(StageError { code, why, .. }) => {
                stage::clear_staged(&facts.app);
                lock(&self.st).status = Status::Failed {
                    version: version.clone(),
                    why: why.clone(),
                };
                self.log(&format!("development build {version} was refused: {code}"));
                self.host.changed();
                return Err(InstallRefused::Refused { code, why });
            }
        };
        {
            let mut st = lock(&self.st);
            st.build = Some(Build {
                version: version.clone(),
                cdhash: staged.cdhash,
                team: staged.team.clone(),
            });
            st.status = Status::BuildReady(version.clone());
        }
        self.log(&format!(
            "development build {version} checked, signed {}, {}",
            staged.team.as_deref().map_or_else(|| "ad hoc".to_owned(), |team| format!("by team {team}")),
            if now { "going now" } else { "waiting for 5 minutes without use" }
        ));
        if now {
            self.build_now(Asked::ByHand);
        } else {
            self.look_at_build(self.now_ms());
        }
        self.host.changed();
        Ok(version)
    }

    /// Whether a kept copy may be gone back to by itself ([`stage::may_go_back_to`]): where releases are held to the team, a copy signed ad hoc only with *Allow development builds* on, in this copy's own state.
    pub(super) fn may_go_back_to(&self, kept: &Path, saved: &Saved, installed: &Path) -> bool {
        let team = matches!(self.source.trust, crate::source::Trust::DeveloperId { .. }).then_some(crate::TEAM_ID);
        let identifier = self.own_identifier(installed);
        stage::may_go_back_to(kept, team, saved.dev_builds == Some(true), &identifier, crate::RELEASE_BUILD)
    }

    /// This app's identifier: a release's own, or in a test's run, the test's own copy's.
    fn own_identifier(&self, installed: &Path) -> String {
        if self.source.mode != crate::source::Mode::Test {
            return crate::BUNDLE_ID.to_owned();
        }
        stage::bundle_id(installed).unwrap_or_else(|| crate::BUNDLE_ID.to_owned())
    }

    /// The 5-minute rule for the build that's waiting, now: it goes, or the timer's set for when it could.
    pub(super) fn look_at_build(self: &Arc<Self>, now: u64) {
        let look = self.host.look();
        let since_input = self.since_input(&look);
        let step = {
            let mut st = lock(&self.st);
            if st.build.is_none() || st.status.is_under_way() {
                return;
            }
            let step = rules::build_step(since_input, !look.busy.is_empty());
            st.install_look_at = match step {
                AutoStep::Wait(ms) => Some(now.saturating_add(ms)),
                AutoStep::Go | AutoStep::Never => None,
            };
            step
        };
        if step == AutoStep::Go {
            self.build_now(Asked::ByItself);
        }
    }

    /// The build that's waiting goes: through the hand-off, on a thread of its own. By itself, it still needs the 5 minutes when it gets there, as the user may have come back; by hand, it waits only for what's in flight.
    pub(super) fn build_now(self: &Arc<Self>, asked: Asked) {
        let (build, facts) = {
            let mut st = lock(&self.st);
            if st.status.is_under_way() {
                return;
            }
            let (Some(build), Some(facts)) = (st.build.clone(), st.facts.clone()) else {
                return;
            };
            st.status = Status::Installing(build.version.clone());
            st.install_look_at = None;
            (build, facts)
        };
        self.stopping.store(false, Ordering::SeqCst);
        self.host.changed();
        let me = Arc::clone(self);
        let thread_build = build.clone();
        let started = std::thread::Builder::new()
            .name("recto-update-install".into())
            .stack_size(super::CHECK_STACK)
            .spawn(move || {
                let to = Going {
                    version: &thread_build.version,
                    cdhash: &thread_build.cdhash,
                    idle_ms: rules::IDLE_BEFORE_BUILD_MS,
                    any_version: true,
                    development_build: true,
                    team_signed: thread_build.team.is_some(),
                };
                me.wait_and_hand_off(asked, &facts, &to);
            });
        if started.is_err() {
            self.failed(asked, &build.version, "no thread could be started", false);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::super::{AtLaunch, Facts, HandOffRefused, HandedOff, Host, Look};
    use super::*;
    use crate::TEAM_ID;
    use crate::fetch::tests::{Answer, Server};
    use crate::plan::{Plan, TestLaunch};
    use crate::source::{Source, Trust, resolve};
    use crate::stage::tests::{fake_app, latest_json, temp_dir, zip_of};
    use recto_platform::local;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Fake {
        since_input_ms: Mutex<u64>,
        busy: Mutex<Vec<String>>,
        hand_offs: AtomicU32,
        plan_at_quit: Mutex<Option<Plan>>,
    }

    impl Host for Fake {
        fn look(&self) -> Look {
            Look {
                busy: lock(&self.busy).clone(),
                since_input_ms: *lock(&self.since_input_ms),
            }
        }
        fn hand_off(&self, _limit: Duration) -> Result<HandedOff, HandOffRefused> {
            self.hand_offs.fetch_add(1, Ordering::SeqCst);
            Ok(HandedOff::default())
        }
        fn quit_now(&self) -> bool {
            true
        }
        fn cancelled(&self) {}
        fn changed(&self) {}
        fn frontmost(&self) -> bool {
            false
        }
        fn relaunch(&self) -> TestLaunch {
            TestLaunch::default()
        }
        fn start_installer(&self, _helper: &Path, plan: &Path, _log: &Path) -> std::io::Result<u32> {
            *lock(&self.plan_at_quit) = Plan::read(plan);
            Ok(1)
        }
    }

    /// An installed app of a version, a server for releases, and somewhere to build zips.
    struct Bed {
        dir: PathBuf,
        server: Server,
        fake: Arc<Fake>,
        updater: Arc<Updater>,
    }

    impl Bed {
        fn new(name: &str, running: &str) -> Self {
            let server = Server::start();
            let source = server.source();
            Self::with(name, running, server, source)
        }

        fn with(name: &str, running: &str, server: Server, source: Source) -> Self {
            let dir = temp_dir(&format!("b-{name}"));
            let root = dir.join("state");
            local::ensure_private_dir(&root).unwrap();
            let install = dir.join("install");
            std::fs::create_dir_all(&install).unwrap();
            let app = app_of(&install, running, crate::BUNDLE_ID);
            let fake = Arc::new(Fake::default());
            let updater = Updater::new(&root, source, Arc::clone(&fake) as Arc<dyn Host>);
            assert_eq!(
                updater.launched(Facts {
                    version: running.into(),
                    app,
                    system: "15.0".into()
                }),
                AtLaunch::GoOn
            );
            Self { dir, server, fake, updater }
        }

        /// `RectoPDF-<named>.zip` of an app that says it's `inside`, with this bundle id.
        fn zip(&self, named: &str, inside: &str, bundle_id: &str) -> PathBuf {
            let built = self.dir.join(format!("built-{named}-{inside}-{}", bundle_id.len()));
            std::fs::create_dir_all(&built).unwrap();
            let app = app_of(&built, inside, bundle_id);
            let zip = self.dir.join(format!("RectoPDF-{named}.zip"));
            let _ = std::fs::remove_file(&zip);
            zip_of(&app, &zip);
            zip
        }

        fn status(&self) -> Status {
            lock(&self.updater.st).status.clone()
        }

        fn idle_for(&self, ms: u64) {
            *lock(&self.fake.since_input_ms) = ms;
        }

        fn wait_for(&self, what: &str, done: impl Fn(&Self) -> bool) {
            let began = Instant::now();
            while !done(self) {
                assert!(began.elapsed() < Duration::from_secs(30), "waited for {what}: {}", self.updater.report());
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn handed_off(&self) -> Option<Plan> {
            lock(&self.fake.plan_at_quit).clone()
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A stand-in app of a release or a development build: a development version goes in `RectoDevVersion` beside its release, as `app/build.sh` writes it, under the same ad-hoc signature.
    fn app_of(at: &Path, version: &str, bundle_id: &str) -> PathBuf {
        let parsed = version::parse(version).expect("a version");
        let release = format!("{}.{}.{}", parsed.release.0, parsed.release.1, parsed.release.2);
        let app = fake_app(at, &release);
        let plist = app.join("Contents/Info.plist");
        let mut edits = Vec::new();
        if parsed.is_development() {
            edits.push(format!("Add :{} string {version}", version::DEV_VERSION_KEY));
        }
        if bundle_id != crate::BUNDLE_ID {
            edits.push(format!("Set :CFBundleIdentifier {bundle_id}"));
        }
        if !edits.is_empty() {
            for edit in edits {
                assert!(
                    Command::new("/usr/libexec/PlistBuddy")
                        .args(["-c", &edit])
                        .arg(&plist)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            assert!(
                Command::new("/usr/bin/codesign")
                    .args(["--force", "--deep", "--sign", "-"])
                    .arg(&app)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        assert_eq!(stage::app_version(&app).as_deref(), Some(version));
        app
    }

    /// Failure: a development build goes in with *Allow development builds* off, the switch starts on, or it's lost at the next launch. The owner's switch is what lets anything but a release replace the app.
    #[test]
    fn a_build_is_refused_until_the_switch_is_on() {
        let bed = Bed::new("switch", "0.1.0");
        assert!(!bed.updater.dev_builds_on(), "off until chosen");
        assert!(!bed.updater.takes_builds());
        let zip = bed.zip("0.1.0.1", "0.1.0.1", crate::BUNDLE_ID);
        assert_eq!(bed.updater.install_build(&zip, true), Err(InstallRefused::DevBuildsOff));
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        bed.updater.choose_dev_builds(true);
        assert!(bed.updater.takes_builds());
        bed.wait_for("the switch written", |bed| Saved::read(&bed.updater.files.saved).dev_builds == Some(true));
        assert_eq!(bed.updater.report()["dev_builds"], true);
        assert_eq!(bed.updater.install_build(&zip, true), Ok("0.1.0.1".to_owned()));
        bed.wait_for("the hand-off", |bed| bed.handed_off().is_some());
        let plan = bed.handed_off().unwrap();
        assert_eq!((plan.from.as_str(), plan.to.as_str(), plan.any_version), ("0.1.0", "0.1.0.1", true));
        assert_eq!(stage::app_version(&plan.new_app()).as_deref(), Some("0.1.0.1"), "staged beside the app");
        // Its arrival is an update's, as a release's is: what was out of sight stays so, and it comes back behind if it was.
        let arrived = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        arrived.launched(Facts {
            version: "0.1.0.1".into(),
            app: bed.updater.app().unwrap(),
            system: "15.0".into(),
        });
        assert!(arrived.reopened_by_update(), "a development build's arrival is reopened by an update");
    }

    /// Failure: a build goes in while the owner is using the app, waits the release's 30 minutes, never goes, or `--now` waits anyway. The owner's rule for dev work: 5 minutes without use.
    #[test]
    fn a_build_waits_for_five_quiet_minutes_unless_now() {
        let bed = Bed::new("idle", "0.1.0");
        bed.updater.choose_dev_builds(true);
        bed.idle_for(60_000);
        let zip = bed.zip("0.1.0.2", "0.1.0.2", crate::BUNDLE_ID);
        assert_eq!(bed.updater.install_build(&zip, false), Ok("0.1.0.2".to_owned()));
        assert_eq!(bed.status(), Status::BuildReady("0.1.0.2".into()));
        assert_eq!(bed.status().code(), "build_ready");
        assert!(bed.updater.status_line().contains("5 minutes"), "{}", bed.updater.status_line());
        let wake = bed.updater.report()["install_look_in_ms"].as_u64().unwrap();
        // Within a second: the two are read a moment apart.
        let next = bed.updater.next_wake().map(|at| at.saturating_sub(bed.updater.now_ms()));
        assert!(
            next.is_some_and(|next| next.abs_diff(wake) <= 1_000),
            "the one time to wake, and not a check that can't go: {next:?} against {wake}"
        );
        assert!((200_000..=240_000).contains(&wake), "the four minutes still to go: {wake}");
        assert_eq!(bed.updater.menu_item().title, "Update to 0.1.0.2…", "the menu can take it at once");
        // Used again in the meantime: 10 s before the look, which the test clock makes 250 s since, and still waiting.
        bed.idle_for(10_000);
        bed.updater.set_test_clock(i64::try_from(wake).unwrap());
        bed.updater.timer_fired();
        assert_eq!(bed.status(), Status::BuildReady("0.1.0.2".into()));
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "not while it's in use");
        // Five minutes without use.
        bed.idle_for(rules::IDLE_BEFORE_BUILD_MS - 10_000);
        bed.updater.set_test_clock(i64::try_from(wake + rules::IDLE_BEFORE_BUILD_MS).unwrap());
        bed.updater.timer_fired();
        bed.wait_for("the hand-off", |bed| bed.handed_off().is_some());
        assert_eq!(bed.handed_off().unwrap().to, "0.1.0.2");

        let now = Bed::new("now", "0.1.0");
        now.updater.choose_dev_builds(true);
        now.idle_for(0);
        let zip = now.zip("0.1.0.3", "0.1.0.3", crate::BUNDLE_ID);
        assert_eq!(now.updater.install_build(&zip, true), Ok("0.1.0.3".to_owned()));
        now.wait_for("the hand-off at once", |bed| bed.handed_off().is_some());
    }

    /// Failure: something that isn't this app's build gets as far as the hand-off: a zip of another name, another app, a version other than its name says, or the version that's running. Nothing's left staged by any of them.
    #[test]
    fn what_isnt_this_apps_build_is_refused() {
        let bed = Bed::new("refused", "0.1.0.4");
        bed.updater.choose_dev_builds(true);
        let renamed = bed.dir.join("build.zip");
        std::fs::copy(bed.zip("0.1.0.5", "0.1.0.5", crate::BUNDLE_ID), &renamed).unwrap();
        assert_eq!(bed.updater.install_build(&renamed, true), Err(InstallRefused::NotABuild));
        let other_app = bed.zip("0.1.0.6", "0.1.0.6", "com.example.Other");
        assert!(
            matches!(
                bed.updater.install_build(&other_app, true),
                Err(InstallRefused::Refused { code: "wrong_app", .. })
            ),
            "the bundle id"
        );
        let mislabelled = bed.zip("0.1.0.7", "0.1.0.8", crate::BUNDLE_ID);
        assert!(matches!(
            bed.updater.install_build(&mislabelled, true),
            Err(InstallRefused::Refused { code: "wrong_version", .. })
        ));
        let same = bed.zip("0.1.0.4", "0.1.0.4", crate::BUNDLE_ID);
        assert_eq!(bed.updater.install_build(&same, true), Err(InstallRefused::SameVersion));
        let junk = bed.dir.join("RectoPDF-0.1.0.9.zip");
        std::fs::write(&junk, b"not a zip").unwrap();
        assert!(matches!(
            bed.updater.install_build(&junk, true),
            Err(InstallRefused::Refused { code: "zip_bad", .. })
        ));
        assert!(matches!(bed.status(), Status::Failed { .. }), "says how it went: {:?}", bed.status());
        let beside = crate::plan::beside_app(&bed.updater.app().unwrap()).unwrap();
        assert!(!crate::plan::new_app_in(&beside).exists(), "nothing's left staged");
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        // Not a build of this sort at all.
        assert_eq!(version_of_zip("RectoPDF-0.1.0.0.zip"), None);
        assert_eq!(
            version_of_zip("RectoPDF-0.1.0.zip"),
            None,
            "a release's number isn't a build's: a release comes from the releases"
        );
        let release_numbered = bed.zip("0.1.1", "0.1.1", crate::BUNDLE_ID);
        assert_eq!(bed.updater.install_build(&release_numbered, true), Err(InstallRefused::NotABuild));
        assert_eq!(version_of_zip("RectoPDF-../0.1.0.1.zip"), None);
    }

    /// Failure: a development build signed ad hoc, which is what every dev build is (owner, 2026-11-11: dev builds never touch the keychain), is refused where releases are held to the team, or one whose signature doesn't verify whole gets in. The gate for a build is the owner's switch, the bundle id and an intact signature. And what's said when it arrives tells the owner macOS may ask again for folders.
    #[test]
    fn an_ad_hoc_build_goes_in_and_a_broken_one_doesnt() {
        let server = Server::start();
        let source = resolve(false, true, Some(server.source().repo), Some("developer-id".into()));
        assert_eq!(source.trust, Trust::DeveloperId { team: TEAM_ID }, "releases held to the team");
        let bed = Bed::with("adhoc", "0.1.0", server, source);
        bed.updater.choose_dev_builds(true);
        // Changed after it was signed: the seal doesn't hold.
        let broken = bed.zip("0.1.0.1", "0.1.0.1", crate::BUNDLE_ID);
        let unpacked = bed.dir.join("broken");
        std::fs::create_dir_all(&unpacked).unwrap();
        assert!(
            Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(&broken)
                .arg(&unpacked)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(unpacked.join("RectoPDF.app/Contents/MacOS/recto-updater"), "changed").unwrap();
        std::fs::remove_file(&broken).unwrap();
        zip_of(&unpacked.join("RectoPDF.app"), &broken);
        assert!(
            matches!(bed.updater.install_build(&broken, true), Err(InstallRefused::Refused { code: "signature", .. })),
            "{:?}",
            bed.status()
        );
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        let zip = bed.zip("0.1.0.2", "0.1.0.2", crate::BUNDLE_ID);
        assert_eq!(bed.updater.install_build(&zip, true), Ok("0.1.0.2".to_owned()), "{}", bed.updater.status_line());
        bed.wait_for("the hand-off", |bed| bed.handed_off().is_some());
        let plan = bed.handed_off().unwrap();
        assert!(plan.development_build && !plan.team_signed, "{plan:?}");
        assert!(arrived_words(&plan).contains("folders"), "{}", arrived_words(&plan));
        assert!(
            arrived_words(&Plan {
                team_signed: true,
                ..plan.clone()
            })
            .contains("signed by RectoPDF's developer")
        );
        assert_eq!(
            arrived_words(&Plan {
                development_build: false,
                ..plan
            }),
            "Updated to 0.1.0.2."
        );
    }

    /// Failure: a release shows *Allow development builds* in Settings, where the owner has no use for it, or a development build hides it, or one that's on can't be turned off from Settings.
    #[test]
    fn the_switch_shows_on_a_build_or_while_its_on() {
        let release = Bed::new("shows-release", "0.1.0");
        assert!(!release.updater.shows_dev_builds_switch());
        release.updater.choose_dev_builds(true);
        assert!(release.updater.shows_dev_builds_switch(), "on, so it can be turned off");
        release.updater.choose_dev_builds(false);
        assert!(!release.updater.shows_dev_builds_switch());
        let build = Bed::new("shows-build", "0.1.0.2");
        assert!(build.updater.shows_dev_builds_switch(), "a development build, on or off");
    }

    /// Failure: development builds are turned off while a build's being checked, and it goes in anyway, as what was asked before the slow part is all that was looked at.
    #[test]
    fn a_build_whose_switch_goes_off_while_its_checked_is_dropped() {
        let bed = Bed::new("off-meanwhile", "0.1.0");
        bed.updater.choose_dev_builds(true);
        let zip = bed.zip("0.1.0.1", "0.1.0.1", crate::BUNDLE_ID);
        let answer = bed.updater.install_build_then(&zip, true, |updater| updater.choose_dev_builds(false));
        assert_eq!(answer, Err(InstallRefused::DevBuildsOff));
        assert!(lock(&bed.updater.st).build.is_none());
        let beside = crate::plan::beside_app(&bed.updater.app().unwrap()).unwrap();
        assert!(!crate::plan::new_app_in(&beside).exists(), "nothing's left staged");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
    }

    /// Failure: a release that keeps failing goes back by itself to a development build signed ad hoc that the owner no longer allows, as 0.1.0.3, then 0.1.0 by the way back, then 0.1.0 not coming up would. With the switch on it may; with it off, it stays on the release.
    #[test]
    fn going_back_lands_on_a_build_only_with_the_switch_on() {
        for (switch, goes_back) in [(Some(false), false), (None, false), (Some(true), true)] {
            let dir = temp_dir(&format!("b-back-{switch:?}"));
            let root = dir.join("state");
            local::ensure_private_dir(&root).unwrap();
            let install = dir.join("install");
            std::fs::create_dir_all(&install).unwrap();
            let app = app_of(&install, "0.1.0", crate::BUNDLE_ID);
            let kept_at = crate::plan::previous_app_in(&crate::plan::beside_app(&app).unwrap());
            let built = dir.join("kept");
            std::fs::create_dir_all(&built).unwrap();
            std::fs::create_dir_all(kept_at.parent().unwrap()).unwrap();
            std::fs::rename(app_of(&built, "0.1.0.3", crate::BUNDLE_ID), &kept_at).unwrap();
            // The release has failed to come up three times.
            Saved {
                unstarted: 3,
                dev_builds: switch,
                ..Saved::default()
            }
            .write(&root.join("updates.json"));
            let server = Server::start();
            // Releases held to the team, as a real build's are.
            let source = resolve(false, true, Some(server.source().repo), Some("developer-id".into()));
            let updater = Updater::new(&root, source, Arc::new(Fake::default()) as Arc<dyn Host>);
            let at = updater.launched(Facts {
                version: "0.1.0".into(),
                app: app.clone(),
                system: "15.0".into(),
            });
            assert_eq!(at == AtLaunch::Exit, goes_back, "{switch:?}");
            if !goes_back {
                assert!(
                    std::fs::read_to_string(root.join("update.log")).unwrap().contains("isn't one to go back to"),
                    "it says why"
                );
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Failure: a local build (`app/build.sh`, which never replaces itself) takes a build handed over, or says it does.
    #[test]
    fn a_local_build_takes_no_build() {
        let server = Server::start();
        let source = resolve(false, false, None, None);
        let bed = Bed::with("local", "0.1.0", server, source);
        bed.updater.choose_dev_builds(true);
        assert!(!bed.updater.takes_builds());
        let zip = bed.zip("0.1.0.1", "0.1.0.1", crate::BUNDLE_ID);
        assert_eq!(bed.updater.install_build(&zip, true), Err(InstallRefused::NotInThisBuild));
    }

    /// Failure: turning development builds off leaves the owner on a build for good, puts them back on a release only if *Update automatically* is on, goes at once while they're using it, or keeps a build waiting that's no longer allowed. The way back is the latest release, older than the build or not, by the 30-minute rule.
    #[test]
    fn turning_development_builds_off_goes_back_to_the_release() {
        let bed = Bed::new("back", "0.1.0.3");
        bed.updater.choose_dev_builds(true);
        bed.updater.choose_switch(false);
        bed.idle_for(0);
        let zip = bed.zip("0.1.0.4", "0.1.0.4", crate::BUNDLE_ID);
        bed.updater.install_build(&zip, false).unwrap();
        // No release yet: nothing to go back to, and a build waiting is dropped.
        bed.updater.choose_dev_builds(false);
        assert!(lock(&bed.updater.st).build.is_none(), "a build waiting goes with the switch");
        bed.wait_for("the check", |bed| !bed.status().is_under_way());
        assert_eq!(bed.status(), Status::NoReleases);
        assert_eq!(bed.updater.report()["back_to_release"], true, "kept for when there is one");
        // A release now, older than the build.
        let built = bed.dir.join("release");
        std::fs::create_dir_all(&built).unwrap();
        let (bytes, sha) = zip_of(&app_of(&built, "0.1.0", crate::BUNDLE_ID), &built.join("r.zip"));
        bed.server.set("/releases/download/v0.1.0/RectoPDF-0.1.0.zip", Answer::Body(bytes));
        bed.server
            .set("/releases/latest/download/latest.json", Answer::Body(latest_json("0.1.0", &sha)));
        assert_eq!(
            bed.updater.check_here(Asked::ByHand),
            Status::Available("0.1.0".into()),
            "the way back is on offer"
        );
        // Not while it's in use, and then by itself, though *Update automatically* is off.
        bed.updater.timer_fired();
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        bed.idle_for(rules::IDLE_BEFORE_INSTALL_MS);
        bed.updater.set_test_clock(i64::try_from(rules::IDLE_BEFORE_INSTALL_MS).unwrap());
        bed.updater.timer_fired();
        bed.wait_for("the way back", |bed| bed.handed_off().is_some());
        let plan = bed.handed_off().unwrap();
        assert_eq!((plan.from.as_str(), plan.to.as_str(), plan.any_version), ("0.1.0.3", "0.1.0", true));
        // On a release, the way back's done.
        let after = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        after.launched(Facts {
            version: "0.1.0".into(),
            app: bed.updater.app().unwrap(),
            system: "15.0".into(),
        });
        assert_eq!(after.report()["back_to_release"], false);
        // And turning them on again ends it before it's gone.
        let again = Bed::new("again", "0.1.0.3");
        again.updater.choose_dev_builds(false);
        again.updater.choose_dev_builds(true);
        assert_eq!(again.updater.report()["back_to_release"], false);
    }

    /// Failure: a release that's older than the build running is offered as an update, with development builds on: only the way back offers an older one.
    #[test]
    fn with_builds_on_an_older_release_isnt_an_update() {
        let bed = Bed::new("older", "0.1.0.3");
        bed.updater.choose_dev_builds(true);
        let built = bed.dir.join("release");
        std::fs::create_dir_all(&built).unwrap();
        let (_, sha) = zip_of(&app_of(&built, "0.1.0", crate::BUNDLE_ID), &built.join("r.zip"));
        bed.server
            .set("/releases/latest/download/latest.json", Answer::Body(latest_json("0.1.0", &sha)));
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::UpToDate);
        bed.server
            .set("/releases/latest/download/latest.json", Answer::Body(latest_json("0.1.1", &sha)));
        assert_eq!(
            bed.updater.check_here(Asked::ByHand),
            Status::Available("0.1.1".into()),
            "a newer one is, as usual"
        );
    }
}
