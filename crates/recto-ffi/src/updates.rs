//! **The updater's records, as the host draws them.** The updater joined to the app is `recto_app::core::updates`'s.

pub(crate) use recto_app::core::updates::*;
pub use recto_app::core::updates::{SettingRow, SettingsAction, SettingsModel, UpdateAlert, UpdateTimer};

/// When the host's one timer should go off.
#[uniffi::remote(Record)]
pub struct UpdateTimer {
    /// What identifies this time: the host arms the timer again only when this changes.
    pub at_ms: u64,
    pub in_seconds: f64,
}

/// One switch of the Settings window.
#[uniffi::remote(Record)]
pub struct SettingRow {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub on: bool,
    pub enabled: bool,
}

/// An update waiting or under way, with the button that installs it now: Rust's words ([`recto_update::updater::UpdateAction`]).
#[uniffi::remote(Record)]
pub struct SettingsAction {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub button: String,
    pub enabled: bool,
}

/// The Settings window, as it stands.
#[uniffi::remote(Record)]
pub struct SettingsModel {
    pub title: String,
    pub rows: Vec<SettingRow>,
    /// A waiting update and its button, above the switches. None when there's nothing to install.
    pub action: Option<SettingsAction>,
    /// How updates stand now.
    pub status: String,
    /// A link to the privacy statement: its words, and where it opens.
    pub privacy_title: String,
    pub privacy_url: String,
}

/// What an update chosen from the menu came to, for an app-modal alert. Rust's words.
#[uniffi::remote(Record)]
pub struct UpdateAlert {
    pub message: String,
    pub button: String,
}
