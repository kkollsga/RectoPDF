//! The updater, joined to the app (SPEC decision 24). All the rules and words are `recto-update`'s and this one's: the host sets one timer for when it's told, draws a menu item and a small window, and passes on what's chosen.

use crate::{CoreCall, Inner, lock};
use recto_agent::{AgentError, Command, exit};
use recto_session::record::SettingsWindow;
use recto_update::plan::TestLaunch;
use recto_update::source::Mode;
use recto_update::updater::{Asked, Facts, HandOffRefused, Host, InstallRefused, Look};
use serde_json::{Value, json};
use std::path::PathBuf;
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
        let Some(inner) = self.inner() else { return Look { busy: vec!["The app is starting.".to_owned()], since_input_ms: 0 } };
        let check = inner.hand_off_check();
        Look { busy: check.busy.into_iter().map(|item| item.message).collect(), since_input_ms: (check.seconds_since_input * 1000.0) as u64 }
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
        TestLaunch { args, env: vec![("RECTO_STATE_DIR".to_owned(), root)], direct: false }
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
            Command::UpdateInstall {} => match self.updates.install(Asked::ByHand) {
                Ok(version) => Ok(json!({ "started": true, "version": version, "words": format!("Updating to {version}. RectoPDF downloads and checks it first, then quits and reopens as it was. `recto update status` says how it's going.") })),
                Err(InstallRefused::NotInThisBuild) => Err(AgentError::new("development_build", "A development build doesn't replace itself.", exit::FAILED).then("`recto update check` still says whether there's a new version.")),
                Err(InstallRefused::NothingOnOffer) => Err(AgentError::new("nothing_on_offer", "There's no new version on offer.", exit::FAILED).then("`recto update check` looks for one.")),
                Err(InstallRefused::Underway) => Err(AgentError::new("update_under_way", "An update or a check is already under way.", exit::BUSY).then("`recto update status` says how it's going.")),
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

    fn settings_answer(&self) -> Value {
        let on = self.updates.switch_on();
        let words = if self.updates.source().mode == Mode::Development {
            format!("Update automatically: {}. A development build checks only when asked, and doesn't replace itself.", if on { "on" } else { "off" })
        } else {
            format!("Update automatically: {}.", if on { "on" } else { "off" })
        };
        json!({ "auto_update": on, "words": words })
    }

    /// The switch, however it's changed. Written out soon, with the rest of the state.
    fn set_auto_update(&self, on: bool) {
        self.updates.set_switch(Some(on));
        self.saver.write_soon();
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
        id == AUTO_UPDATE_ROW
    }

    pub(crate) fn settings_model(&self) -> SettingsModel {
        let development = self.updates.source().mode == Mode::Development;
        let hint = if development { "A development build checks only when you ask, and doesn't replace itself." } else { "Once a new version's out, RectoPDF updates itself when you haven't touched it for 30 minutes, and comes back just as you left it." };
        SettingsModel { title: "Settings".to_owned(), rows: vec![SettingRow { id: AUTO_UPDATE_ROW.to_owned(), title: "Update automatically".to_owned(), hint: hint.to_owned(), on: self.updates.switch_on(), enabled: !development }], status: self.updates.status_line() }
    }

    pub(crate) fn setting_chosen(&self, id: &str, on: bool) {
        // Past a hand-off's last look, what's done here wouldn't come back.
        if id == AUTO_UPDATE_ROW && !self.core.frozen.load(Ordering::SeqCst) {
            self.set_auto_update(on);
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

    pub(crate) fn note_update_facts(&self, version: String, bundle_path: String, system_version: String) {
        let app = PathBuf::from(&bundle_path);
        let app = app.canonicalize().unwrap_or(app);
        *lock(&self.update_facts) = Some(Facts { version, app, system: system_version });
    }
}

/// What the host's one timer should do. No timer at all if there's nothing to wake for, and never less than a second, so a time already past can't make it spin.
pub(crate) fn timer_for(next_wake: Option<u64>, now_ms: u64) -> Option<UpdateTimer> {
    next_wake.map(|at_ms| UpdateTimer { at_ms, in_seconds: (at_ms.saturating_sub(now_ms) as f64 / 1000.0).max(1.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

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
