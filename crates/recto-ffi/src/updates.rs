//! The updater, joined to the app (SPEC decision 24). All the rules and words are `recto-update`'s and this one's: the host sets one timer for when it's told, draws a menu item and a small window, and passes on what's chosen.

use crate::ScreenInfo;
use crate::{CoreCall, Inner, lock};
use recto_agent::{AgentError, Command, exit};
use recto_platform::isolate::SharedMemory;
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileKind, TileRect, TileRequest, TileResult};
use recto_session::record::SettingsWindow;
use recto_update::plan::TestLaunch;
use recto_update::source::Mode;
use recto_update::updater::{Asked, Facts, HandOffRefused, Host, InstallRefused, Look};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

/// When the host's one timer should go off.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct UpdateTimer {
    /// What identifies this time: the host arms the timer again only when this changes.
    pub at_ms: u64,
    pub in_seconds: f64,
}

/// One switch of the Settings window.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SettingRow {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub on: bool,
    pub enabled: bool,
}

/// The Settings window, as it stands.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SettingsModel {
    pub title: String,
    pub rows: Vec<SettingRow>,
    /// How updates stand now.
    pub status: String,
}

/// The switch's id, in the window, in QA and nowhere else.
pub(crate) const AUTO_UPDATE_ROW: &str = "update.automatic";
/// *Allow development builds*.
pub(crate) const DEV_BUILDS_ROW: &str = "update.dev_builds";

/// How the updater reaches the app. Holds only a way back to it: it's made first, as the updater's a part of what it reaches.
pub(crate) struct Hooks {
    inner: OnceLock<Weak<Inner>>,
}

impl Hooks {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { inner: OnceLock::new() })
    }

    pub(crate) fn attach(&self, inner: Weak<Inner>) {
        let _ = self.inner.set(inner);
    }

    fn inner(&self) -> Option<Arc<Inner>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}

impl Host for Hooks {
    fn look(&self) -> Look {
        let Some(inner) = self.inner() else {
            return Look {
                busy: vec!["The app is starting.".to_owned()],
                since_input_ms: 0,
            };
        };
        let check = inner.hand_off_check();
        Look {
            busy: check.busy.into_iter().map(|item| item.message).collect(),
            since_input_ms: (check.seconds_since_input * 1000.0) as u64,
        }
    }

    fn hand_off(&self, limit: Duration) -> Result<(), HandOffRefused> {
        let inner = self.inner().ok_or_else(|| HandOffRefused::NotSaved("the app is going away".to_owned()))?;
        inner.hand_off(limit).map(|_| ()).map_err(|error| match error {
            crate::HandOffError::Busy { items } => HandOffRefused::Busy(items.into_iter().map(|item| item.message).collect()),
            crate::HandOffError::NotSaved { message, .. } => HandOffRefused::NotSaved(message),
        })
    }

    fn quit_now(&self) -> bool {
        self.inner().is_some_and(|inner| inner.hand_off_quit_now())
    }

    fn cancelled(&self) {
        if let Some(inner) = self.inner() {
            inner.hand_off_cancelled();
        }
    }

    fn changed(&self) {
        if let Some(inner) = self.inner() {
            inner.core.push(CoreCall::Reconcile);
            inner.listener.model_changed();
        }
    }

    fn frontmost(&self) -> bool {
        self.inner().is_some_and(|inner| lock(&inner.app).focus.app_frontmost)
    }

    /// A quiet run reopens quiet, on its own state folder. Only the updater's test mode ever looks at this.
    fn relaunch(&self) -> TestLaunch {
        let Some(inner) = self.inner() else { return TestLaunch::default() };
        let root = inner.paths.as_ref().map(|paths| paths.root.display().to_string()).unwrap_or_default();
        let args = if inner.qa.is_some() { vec!["--qa".to_owned()] } else { Vec::new() };
        TestLaunch {
            args,
            env: vec![("RECTO_STATE_DIR".to_owned(), root)],
            direct: false,
        }
    }

    /// Asked on the thread that reads the state, before it does: so it waits for the lock as long as reading would, as an app that's just quit can still be letting go of it, and a quick quit and reopen isn't a second copy.
    fn keeps_the_record(&self) -> bool {
        self.inner().is_some_and(|inner| inner.saver.store.wait_for_lock())
    }

    fn can_draw(&self) -> bool {
        let Some(inner) = self.inner() else { return false };
        let Some(paths) = inner.paths.as_ref() else { return false };
        draws_a_page(&paths.root.join("update"))
    }
}

impl Inner {
    /// What `recto update` and `recto settings` come to. The level's been checked.
    pub(crate) fn update_command(&self, command: &Command) -> Result<Value, AgentError> {
        match command {
            Command::UpdateStatus {} => Ok(self.updates.report()),
            Command::UpdateCheck {} => {
                self.updates.check_here(Asked::ByHand);
                Ok(self.updates.report())
            }
            Command::UpdateInstall { zip: Some(zip), now } => self.install_build(zip, *now),
            Command::UpdateDevBuilds { set } => {
                if let Some(word) = set {
                    let on = match word.as_str() {
                        "on" => true,
                        "off" => false,
                        _ => return Err(AgentError::new("usage", "It's `on` or `off`.", exit::USAGE)),
                    };
                    self.set_dev_builds(on);
                }
                Ok(self.dev_builds_answer())
            }
            Command::UpdateInstall { zip: None, .. } => match self.updates.install(Asked::ByHand) {
                Ok(version) => Ok(
                    json!({ "started": true, "version": version, "words": format!("Updating to {version}. RectoPDF downloads and checks it first, then quits and reopens as it was. `recto update status` says how it's going.") }),
                ),
                Err(InstallRefused::NotInThisBuild) => Err(
                    AgentError::new("development_build", "A development build doesn't replace itself.", exit::FAILED)
                        .then("`recto update check` still says whether there's a new version."),
                ),
                Err(InstallRefused::NothingOnOffer) => {
                    Err(AgentError::new("nothing_on_offer", "There's no new version on offer.", exit::FAILED).then("`recto update check` looks for one."))
                }
                Err(InstallRefused::Underway) => Err(AgentError::new("update_under_way", "An update or a check is already under way.", exit::BUSY)
                    .then("`recto update status` says how it's going.")),
                Err(refused) => Err(refused_words(&refused)),
            },
            Command::Settings { auto_update } => {
                if let Some(word) = auto_update {
                    let on = match word.as_str() {
                        "on" => true,
                        "off" => false,
                        _ => return Err(AgentError::new("usage", "It's `on` or `off`.", exit::USAGE)),
                    };
                    self.set_auto_update(on);
                }
                Ok(self.settings_answer())
            }
            _ => Err(recto_agent::update_needs_app()),
        }
    }

    /// `recto update install <zip>`: a development build handed over. Checked here, on the command's own thread, and then it waits for its moment.
    fn install_build(&self, zip: &Path, now: bool) -> Result<Value, AgentError> {
        let version = self.updates.install_build(zip, now).map_err(|refused| refused_words(&refused))?;
        let report = self.updates.report();
        let waits_ms = report["install_look_in_ms"].as_u64();
        let team = report["build_team"].as_str().map(str::to_owned);
        let signed = match &team {
            Some(team) => format!("It's signed by RectoPDF's developer (team {team})."),
            None => "It's signed on this Mac, not by RectoPDF's developer, so macOS may ask again for access to folders.".to_owned(),
        };
        let words = if now || waits_ms.is_none() {
            format!("Installing development build {version} now. RectoPDF quits and reopens as it was. {signed} `recto update status` says how it's going.")
        } else {
            format!(
                "Development build {version} is checked, and goes in once RectoPDF hasn't been used for 5 minutes, then reopens as it was. Choose Update to {version}… or add `--now` to go at once. {signed} `recto update status` says how it's going."
            )
        };
        Ok(json!({ "started": true, "version": version, "waits_ms": waits_ms, "team": team, "words": words }))
    }

    /// The owner's standing consent: with *Allow development builds* on, a development build's install is taken from anyone on this Mac who can reach the socket, which is only this user, at any agent level, Off included. Only that one command, and only what passes the signature check goes in (SPEC decision 24, a coordinator's default awaiting the owner).
    pub(crate) fn consents_to(&self, command: &Command) -> bool {
        matches!(command, Command::UpdateInstall { zip: Some(_), .. }) && self.updates.takes_builds()
    }

    fn set_dev_builds(&self, on: bool) {
        self.updates.choose_dev_builds(on);
        self.core.push(CoreCall::Reconcile);
    }

    fn dev_builds_answer(&self) -> Value {
        let on = self.updates.dev_builds_on();
        let back = self.updates.report()["back_to_release"] == true;
        let words = match (on, back) {
            (true, _) => "Allow development builds: on. `recto update install <zip>` installs one, once RectoPDF hasn't been used for 5 minutes.".to_owned(),
            (false, true) => "Allow development builds: off. This is a development build, so the latest release goes in by itself once there is one and RectoPDF hasn't been touched for 30 minutes.".to_owned(),
            (false, false) => "Allow development builds: off. RectoPDF installs only releases.".to_owned(),
        };
        json!({ "dev_builds": on, "back_to_release": back, "words": words })
    }

    fn settings_answer(&self) -> Value {
        let on = self.updates.switch_on();
        let words = if self.updates.source().mode == Mode::Development {
            format!(
                "Update automatically: {}. A local build checks only when asked, and doesn't replace itself.",
                if on { "on" } else { "off" }
            )
        } else {
            format!("Update automatically: {}.", if on { "on" } else { "off" })
        };
        json!({ "auto_update": on, "words": words })
    }

    /// The switch, however it's changed. Written out soon, with the rest of the state.
    fn set_auto_update(&self, on: bool) {
        self.updates.choose_switch(on);
        self.core.push(CoreCall::Reconcile);
    }

    pub(crate) fn update_report(&self) -> Value {
        self.updates.report()
    }

    pub(crate) fn update_timer_fired(&self) {
        self.updates.timer_fired();
    }

    pub(crate) fn update_test_clock(&self, ahead_ms: i64) -> bool {
        self.updates.set_test_clock(ahead_ms)
    }

    pub(crate) fn has_setting(&self, id: &str) -> bool {
        id == AUTO_UPDATE_ROW || id == DEV_BUILDS_ROW
    }

    pub(crate) fn settings_model(&self) -> SettingsModel {
        let development = self.updates.source().mode == Mode::Development;
        let hint = if development {
            "A local build checks only when you ask, and doesn't replace itself."
        } else {
            "RectoPDF updates itself once you haven't touched it for 30 minutes, and comes back just as you left it."
        };
        SettingsModel {
            title: "Settings".to_owned(),
            rows: vec![
                SettingRow {
                    id: AUTO_UPDATE_ROW.to_owned(),
                    title: "Update automatically".to_owned(),
                    hint: hint.to_owned(),
                    on: self.updates.switch_on(),
                    enabled: !development,
                },
                SettingRow {
                    id: DEV_BUILDS_ROW.to_owned(),
                    title: "Allow development builds".to_owned(),
                    hint: "Lets a development build replace RectoPDF, once you haven't used it for 5 minutes. Off, only releases are installed, and a development build goes back to the latest release.".to_owned(),
                    on: self.updates.dev_builds_on(),
                    enabled: !development,
                },
            ],
            status: self.updates.status_line(),
        }
    }

    pub(crate) fn setting_chosen(&self, id: &str, on: bool) {
        // Past a hand-off's last look, what's done here wouldn't come back.
        if self.core.frozen.load(Ordering::SeqCst) {
            return;
        }
        match id {
            AUTO_UPDATE_ROW => self.set_auto_update(on),
            DEV_BUILDS_ROW => self.set_dev_builds(on),
            _ => {}
        }
    }

    pub(crate) fn settings_window_changed(&self, place: Option<SettingsWindow>) {
        lock(&self.app).settings_window = place;
        self.saver.view_changed();
    }

    pub(crate) fn settings_window(&self) -> Option<SettingsWindow> {
        lock(&self.app).settings_window
    }

    /// The hand-off isn't going ahead.
    pub(crate) fn hand_off_cancelled(&self) {
        self.hand_off_state.store(crate::HandOffState::Idle as u8, Ordering::SeqCst);
        self.set_frozen(false);
    }

    pub(crate) fn note_update_facts(&self, version: String, dev_version: Option<String>, bundle_path: String, system_version: String) {
        // Resolved where the rest of what's asked of the disk is, on the thread that reads the state.
        *lock(&self.update_facts) = Some(Facts {
            version: recto_update::version::running(&version, dev_version.as_deref()),
            app: PathBuf::from(bundle_path),
            system: system_version,
        });
    }
}

/// A page of one colour, as small as a PDF gets. Ours, so it says something about this version and nothing about whatever the user had open.
fn probe_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Contents 4 0 R >>".to_owned(),
        {
            let content = "1 0 0 rg 0 0 64 64 re f";
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
        },
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut places = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        places.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let table = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for place in places {
        pdf.extend_from_slice(format!("{place:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

/// Whether this version can draw: the page above, through a real worker with the real library, and what comes back has to be that colour. What a new version has to show before it counts as having come up, as nothing else says a renderer that can't load is broken until someone's looking at a blank page.
pub(crate) fn draws_a_page(folder: &Path) -> bool {
    let file = folder.join("draws.pdf");
    if std::fs::create_dir_all(folder).is_err() || std::fs::write(&file, probe_pdf()).is_err() {
        return false;
    }
    let drew = (|| {
        let config = ServiceConfig::discover().ok()?;
        let mut document = DocRenderer::new(&file, None, config, Policy::default()).ok()?;
        let surface = SharedMemory::create(64 * 64 * 4).ok()?;
        let request = TileRequest {
            page: 0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            scale: 1.0,
            stride: 64 * 4,
            generation: 0,
        };
        match document.render_tile(&request, &surface).ok()? {
            TileResult::Done(tile) => Some(matches!(tile.kind, TileKind::Blank { rgb } if (rgb >> 16) & 0xFF > 0xC0 && rgb & 0xFFFF < 0x4040)),
            TileResult::Cancelled { .. } => None,
        }
    })();
    let _ = std::fs::remove_file(&file);
    drew == Some(true)
}

/// What a refusal to install comes to, for an agent: a code that doesn't change, our words, and what to do next.
fn refused_words(refused: &InstallRefused) -> AgentError {
    match refused {
        InstallRefused::NotInThisBuild => AgentError::new("development_build", "A local build doesn't replace itself.", exit::FAILED)
            .then("`recto update check` still says whether there's a new version. Install the app with `tools/dev-build.sh` to take builds."),
        InstallRefused::NothingOnOffer => {
            AgentError::new("nothing_on_offer", "There's no new version on offer.", exit::FAILED).then("`recto update check` looks for one.")
        }
        InstallRefused::Underway => {
            AgentError::new("update_under_way", "An update or a check is already under way.", exit::BUSY).then("`recto update status` says how it's going.")
        }
        InstallRefused::AnotherCopy => AgentError::new(
            "another_copy",
            "Another copy of RectoPDF is keeping the state, and it's the one that can update.",
            exit::FAILED,
        )
        .then("Quit this copy, and use that one."),
        InstallRefused::DevBuildsOff => AgentError::new("dev_builds_off", "Development builds are off, so only releases are installed.", exit::FAILED)
            .then("The owner turns on Settings › Allow development builds."),
        InstallRefused::NotABuild => {
            AgentError::new("not_a_build", "That isn't a RectoPDF-X.Y.Z.N.zip.", exit::USAGE).then("`tools/dev-build.sh` makes one and prints its path.")
        }
        InstallRefused::SameVersion => {
            AgentError::new("same_version", "That's the version that's running.", exit::FAILED).then("`tools/dev-build.sh` numbers the next one.")
        }
        InstallRefused::Refused { code, why } => {
            AgentError::new(code, format!("That build wasn't installed: {why}."), exit::FAILED).then("`recto update status` has the log's path.")
        }
    }
}

/// Whether a launch comes up behind what the user's doing: an update arriving, with the app not having been frontmost. Any other launch was asked for by someone, and comes forward as usual.
pub(crate) fn opens_in_background(arrived_by_update: bool, was_frontmost: bool) -> bool {
    arrived_by_update && !was_frontmost
}

/// Going into full screen is another Space, and moves the user there. So not by itself: not until they come to it.
pub(crate) fn full_screen_on_showing(was_full_screen: bool, in_background: bool) -> (bool, bool) {
    (was_full_screen && !in_background, was_full_screen && in_background)
}

/// Where the Settings window goes: where it was, if that's on a screen there is now.
pub(crate) fn settings_place(saved: Option<SettingsWindow>, screens: &[ScreenInfo]) -> Option<Vec<f64>> {
    let saved = saved?;
    screens
        .iter()
        .any(|screen| {
            saved.x >= screen.visible_x
                && saved.x < screen.visible_x + screen.visible_width
                && saved.y >= screen.visible_y
                && saved.y < screen.visible_y + screen.visible_height
        })
        .then(|| vec![saved.x, saved.y])
}

/// What the host's one timer should do. No timer at all if there's nothing to wake for, and never less than a second, so a time already past can't make it spin.
pub(crate) fn timer_for(next_wake: Option<u64>, now_ms: u64) -> Option<UpdateTimer> {
    next_wake.map(|at_ms| UpdateTimer {
        at_ms,
        in_seconds: (at_ms.saturating_sub(now_ms) as f64 / 1000.0).max(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a silent update puts a window over what the user's doing, or moves them to another Space by going into full screen. **Only as far as it can be tested with no screen:** that Rust says behind and not yet, for an update arriving in the background and nothing else. What AppKit then does stays unproven until a signed release on a real desktop.
    #[test]
    fn a_background_update_stays_behind_and_out_of_full_screen() {
        assert!(opens_in_background(true, false));
        assert!(!opens_in_background(true, true), "it was frontmost, so it comes back frontmost");
        assert!(
            !opens_in_background(false, false),
            "a launch someone asked for comes forward, whatever it was before"
        );
        assert_eq!(full_screen_on_showing(true, true), (false, true), "not yet");
        assert_eq!(full_screen_on_showing(true, false), (true, false), "at once, as normal");
        assert_eq!(full_screen_on_showing(false, true), (false, false));
    }

    /// Failure: Settings comes back where there's no screen any more, and can't be reached.
    #[test]
    fn settings_comes_back_only_on_a_screen_there_is() {
        let screen = ScreenInfo {
            id: 1,
            name: "a".into(),
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
            visible_x: 0.0,
            visible_y: 0.0,
            visible_width: 1000.0,
            visible_height: 780.0,
        };
        let on = SettingsWindow { x: 100.0, y: 100.0 };
        assert_eq!(settings_place(Some(on), std::slice::from_ref(&screen)), Some(vec![100.0, 100.0]));
        assert_eq!(
            settings_place(Some(SettingsWindow { x: 2000.0, y: 100.0 }), std::slice::from_ref(&screen)),
            None
        );
        assert_eq!(settings_place(Some(on), &[]), None);
        assert_eq!(settings_place(None, &[screen]), None);
    }

    /// Failure: the page a new version's asked to draw isn't one PDFium reads as a page of that colour, so every update would be sent back, or it says so whatever's drawn. Only the worker's own build isn't here, so the real drawing's in `restore`'s tests of the real app.
    #[test]
    fn the_page_it_draws_is_a_whole_pdf() {
        let pdf = probe_pdf();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        let text = String::from_utf8(pdf).unwrap();
        let table = text.rfind("xref\n0 5").unwrap();
        assert!(text.contains(&format!("startxref\n{table}\n")));
        assert!(text.contains("1 0 0 rg 0 0 64 64 re f"));
    }

    /// Failure: a time that's passed arms a timer that fires at once and again and again, or a time far off comes out as anything but itself. At rest it's one timer a day.
    #[test]
    fn the_timer_never_spins() {
        assert_eq!(timer_for(None, 5), None);
        assert_eq!(timer_for(Some(5), 10_000).unwrap().in_seconds, 1.0);
        assert_eq!(timer_for(Some(5_000), 5_000).unwrap().in_seconds, 1.0);
        let day = timer_for(Some(86_400_000), 0).unwrap();
        assert_eq!(day.in_seconds, 86_400.0);
        assert_eq!(day.at_ms, 86_400_000);
    }
}
