//! **Updates, as the window has them:** the Settings window's model and the menu's alert, and the host's one timer.

/// When the host's one timer should go off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpdateTimer {
    /// What identifies this time: the host arms the timer again only when this changes.
    pub at_ms: u64,
    pub in_seconds: f64,
}

/// One switch of the Settings window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingRow {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub on: bool,
    pub enabled: bool,
}

/// An update waiting or under way, with the button that installs it now: Rust's words ([`recto_update::updater::UpdateAction`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsAction {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub button: String,
    pub enabled: bool,
}

/// The Settings window, as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateAlert {
    pub message: String,
    pub button: String,
}
