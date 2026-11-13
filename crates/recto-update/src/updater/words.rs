//! **What the status line and the menu item say**, in Rust's words, from the updater's state. Moved out of `updater.rs` (R1.2) so the words sit together.

use super::*;

impl Updater {
    /// The line in Settings and `update status`.
    #[must_use]
    pub fn status_line(&self) -> String {
        let now = self.now_ms();
        let st = lock(&self.st);
        let version = st.facts.as_ref().map_or_else(String::new, |facts| facts.version.clone());
        match &st.status {
            Status::Idle => format!("{APP_NAME} {version} · not checked yet"),
            Status::Checking => "Checking for updates…".to_owned(),
            Status::UpToDate | Status::NoReleases => st.saved.last_check_ms.map_or_else(
                || format!("{APP_NAME} {version} is up to date"),
                |at| format!("{APP_NAME} {version} is up to date · checked {}", ago(at, now)),
            ),
            Status::Available(offered) => format!("{APP_NAME} {offered} is available"),
            Status::AvailableElsewhere(offered) => format!("{APP_NAME} {offered} is available · a local build doesn't replace itself"),
            Status::CheckFailed(why) => format!("Couldn't check for updates: {why}"),
            Status::Preparing(offered) => format!("Downloading {APP_NAME} {offered}…"),
            Status::PreparingBuild(build) => format!("Checking development build {build}…"),
            Status::Waiting { version: offered, for_what } => format!("Updating to {offered} as soon as this is done: {for_what}"),
            Status::Installing(offered) => format!("Installing {APP_NAME} {offered}…"),
            Status::Failed { version: offered, why } => format!("Update to {offered} failed: {why}"),
            Status::Updated(offered) => format!("Updated to {offered}"),
            Status::WentBack { version: offered, back_on } => format!("{offered} didn't start, so {back_on} came back"),
            Status::BuildReady(build) => format!("Development build {build} is ready, and goes in once {APP_NAME} hasn't been used for 5 minutes"),
        }
    }

    /// **Whether the menu's item goes now** (an update or a build on offer, its title *Update to X…*) rather than checks (*Check for Updates…*): what the
    /// person's choosing it sends, `update install --now` or `update check` (R2.3, C8).
    #[must_use]
    pub fn menu_installs(&self) -> bool {
        let st = lock(&self.st);
        !st.status.is_under_way() && (st.build.is_some() || self.offered_here(&st).is_some())
    }

    /// The menu item. Rust's words, whichever way things stand. An offer stays on it through whatever else has happened: a check that failed last night, or an install that didn't work, which by hand can always be tried again.
    #[must_use]
    pub fn menu_item(&self) -> MenuItem {
        let st = lock(&self.st);
        match &st.status {
            Status::Preparing(version) | Status::Waiting { version, .. } | Status::Installing(version) => {
                return MenuItem {
                    title: format!("Updating to {version}…"),
                    enabled: false,
                };
            }
            Status::PreparingBuild(version) => {
                return MenuItem {
                    title: format!("Checking development build {version}…"),
                    enabled: false,
                };
            }
            Status::Checking => {
                return MenuItem {
                    title: "Check for Updates…".to_owned(),
                    enabled: false,
                };
            }
            _ => {}
        }
        if let Some(build) = &st.build {
            return MenuItem {
                title: format!("Update to {}…", build.version),
                enabled: true,
            };
        }
        match self.offered_here(&st) {
            Some(version) => MenuItem {
                title: format!("Update to {version}…"),
                enabled: true,
            },
            None => MenuItem {
                title: "Check for Updates…".to_owned(),
                enabled: true,
            },
        }
    }
}
