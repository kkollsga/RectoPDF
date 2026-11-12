//! **Update now: one function for the menu, the Settings button and `recto update install`** (owner, 2026-11-12: "I dont see the update now button in settings"; rule 4: the interfaces call the same methods). [`Updater::update_now`] is what all three run, and [`Updater::settings_action`] is what Settings shows of it: Rust decides the words, whether there's anything to do and whether the button works. The host only draws.

use super::{Asked, InstallRefused, Status, Updater, lock};
use crate::APP_NAME;
use std::sync::Arc;

/// What Settings shows of an update that's waiting or under way: a title, what it's waiting for, and a button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateAction {
    pub title: String,
    pub hint: String,
    pub button: String,
    pub enabled: bool,
}

impl Updater {
    /// Installs what's waiting, now, by hand: a development build that's checked goes at once instead of after its 5 minutes, and else what's on offer from the releases is downloaded and installed. The caller says what came of it; a refusal changes nothing.
    pub fn update_now(self: &Arc<Self>) -> Result<String, InstallRefused> {
        let waiting = {
            let st = lock(&self.st);
            (!st.status.is_under_way())
                .then(|| st.build.as_ref().map(|build| build.version.clone()))
                .flatten()
        };
        match waiting {
            Some(version) => {
                self.build_now(Asked::ByHand);
                Ok(version)
            }
            None => self.install(Asked::ByHand),
        }
    }

    /// What `recto update install` says, with no zip and no `--now`, while a build waits: the build, what it waits for, and how to go at once. The five idle minutes are the owner's rule and stay the default for an agent, so the plain command starts nothing. Nothing when no build waits.
    #[must_use]
    pub fn waiting_build_words(&self) -> Option<(String, String)> {
        let now = self.now_ms();
        let st = lock(&self.st);
        let build = st.build.as_ref().filter(|_| !st.status.is_under_way())?;
        let version = build.version.clone();
        let left = st.install_look_at.map_or_else(
            || "once nothing is in flight".to_owned(),
            |at| format!("in about {} minute(s) if it isn't used", (at.saturating_sub(now) / 60_000) + 1),
        );
        let words = format!(
            "Development build {version} is already checked and waiting for {APP_NAME} to go unused for 5 minutes, which is {left}. Nothing was started. `recto update install --now` goes at once, as does Update now in Settings or the menu."
        );
        Some((version, words))
    }

    /// The hint under *Update automatically*: how it goes, and for a copy that checks by itself, the privacy line.
    #[must_use]
    pub fn auto_update_hint(&self, privacy_line: &str) -> String {
        let hint = if self.source.mode == crate::source::Mode::Development {
            "A local build checks only when you ask, and doesn't replace itself."
        } else {
            "RectoPDF updates itself once you haven't touched it for 30 minutes, and comes back just as you left it."
        };
        if self.source.checks_by_itself() {
            format!("{hint} {privacy_line}")
        } else {
            hint.to_owned()
        }
    }

    /// What Settings says of an update, or nothing when there isn't one: nothing on offer, nothing waiting. Under way, it says so and the button is off, so a second press can't start a second install.
    #[must_use]
    pub fn settings_action(&self) -> Option<UpdateAction> {
        let st = lock(&self.st);
        let app = APP_NAME;
        let under_way = |title: String| UpdateAction {
            title,
            hint: "It comes back just as you left it.".to_owned(),
            button: "Update now".to_owned(),
            enabled: false,
        };
        match &st.status {
            Status::PreparingBuild(version) => return Some(under_way(format!("Checking development build {version}…"))),
            Status::Preparing(version) | Status::Waiting { version, .. } | Status::Installing(version) => {
                return Some(under_way(format!("Updating to {version}…")));
            }
            _ => {}
        }
        if let Some(build) = &st.build {
            let version = &build.version;
            return Some(UpdateAction {
                title: format!("Development build {version} is ready"),
                hint: format!("It goes in once {app} hasn't been used for 5 minutes, or now. {app} comes back just as you left it."),
                button: "Update now".to_owned(),
                enabled: true,
            });
        }
        let version = self.offered_here(&st)?;
        let by_itself = st.saved.auto_update.unwrap_or_else(|| self.source.auto_default());
        Some(UpdateAction {
            title: format!("{app} {version} is available"),
            hint: if by_itself {
                format!("It goes in once {app} hasn't been touched for 30 minutes, or now. {app} comes back just as you left it.")
            } else {
                format!("Update automatically is off, so it goes in when you choose. {app} comes back just as you left it.")
            },
            button: "Update now".to_owned(),
            enabled: true,
        })
    }
}
