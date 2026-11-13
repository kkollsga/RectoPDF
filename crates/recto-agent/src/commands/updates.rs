//! **Updates and settings, which only the app has:** `update status`, `update check`, `update install`, `update dev-builds` and `settings`. The app's updater answers them (`recto-app`'s `core/updates.rs`); a session with no window says it has none.

use super::words::OnOff;
use super::{Cmd, Cx, OnlyAsks, row_access};
use crate::Code;
use crate::human::text;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

/// What's running, what's on offer, and how the last update went.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateStatus {}

impl Cmd for UpdateStatus {
    const TAG: &'static str = "update_status";
    const ROWS: &'static [Spec] = ROWS_UPDATE_STATUS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    /// Only the app has an updater, or settings.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(update_needs_app())
    }
    fn human(data: &Value) -> String {
        status_words(data)
    }
}

/// Looks for a new version now, and says what it found.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCheck {}

impl Cmd for UpdateCheck {
    const TAG: &'static str = "update_check";
    const ROWS: &'static [Spec] = ROWS_UPDATE_CHECK;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    /// Only the app has an updater, or settings.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(update_needs_app())
    }
    fn human(data: &Value) -> String {
        status_words(data)
    }
}

/// Starts installing what's on offer, with the app's state carried across. With a zip, a development build handed over.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateInstall {
    #[serde(default)]
    pub zip: Option<PathBuf>,
    /// A development build goes at once, not after 5 minutes without use.
    #[serde(default)]
    pub now: bool,
}

impl Cmd for UpdateInstall {
    const TAG: &'static str = "update_install";
    const ROWS: &'static [Spec] = ROWS_UPDATE_INSTALL;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "build": self.zip.as_deref().map(file_label), "now": self.now })
    }
    /// Only the app has an updater, or settings.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(update_needs_app())
    }
    fn human(data: &Value) -> String {
        text(&data["words"]).to_string()
    }
}

/// *Allow development builds*. With a value, sets it.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(into = "line::UpdateDevBuilds")]
pub struct UpdateDevBuilds {
    pub set: Option<OnOff>,
}

/// `update dev-builds` and `settings` as they're written on the wire.
pub mod line {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateDevBuilds {
        #[serde(default)]
        pub set: Option<String>,
    }

    #[derive(Clone, Debug, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    pub struct Settings {
        #[serde(default)]
        pub auto_update: Option<String>,
    }
}

/// `on` or `off`, or the refusal of what isn't, in the words the app gave it.
fn on_off(word: Option<String>) -> Result<Option<OnOff>, AgentError> {
    word.map(|word| OnOff::from_word(&word).ok_or_else(|| AgentError::new(Code::USAGE, "It's `on` or `off`.", exit::USAGE)))
        .transpose()
}

impl TryFrom<line::UpdateDevBuilds> for UpdateDevBuilds {
    type Error = AgentError;
    fn try_from(line: line::UpdateDevBuilds) -> Result<Self, AgentError> {
        Ok(Self { set: on_off(line.set)? })
    }
}

impl From<&UpdateDevBuilds> for line::UpdateDevBuilds {
    fn from(asked: &UpdateDevBuilds) -> Self {
        Self {
            set: asked.set.map(|set| set.word().to_string()),
        }
    }
}

impl From<UpdateDevBuilds> for line::UpdateDevBuilds {
    fn from(asked: UpdateDevBuilds) -> Self {
        Self::from(&asked)
    }
}

impl std::fmt::Debug for UpdateDevBuilds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        line::UpdateDevBuilds::from(self).fmt(f)
    }
}

impl Cmd for UpdateDevBuilds {
    const TAG: &'static str = "update_dev_builds";
    const ROWS: &'static [Spec] = ROWS_UPDATE_DEV_BUILDS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    /// The switch is what lets a build that isn't a release replace the app, so setting it is as far-reaching as installing.
    fn access(&self) -> Access {
        match self.set {
            Some(_) => Access::At(Level::Full),
            None => row_access(Self::TAG),
        }
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "set": self.set.map(OnOff::word) })
    }
    /// Only the app has an updater, or settings.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(update_needs_app())
    }
    fn human(data: &Value) -> String {
        text(&data["words"]).to_string()
    }
}

/// The app's settings. With a value, changes it.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(into = "line::Settings")]
pub struct Settings {
    pub auto_update: Option<OnOff>,
}

impl TryFrom<line::Settings> for Settings {
    type Error = AgentError;
    fn try_from(line: line::Settings) -> Result<Self, AgentError> {
        Ok(Self {
            auto_update: on_off(line.auto_update)?,
        })
    }
}

impl From<&Settings> for line::Settings {
    fn from(asked: &Settings) -> Self {
        Self {
            auto_update: asked.auto_update.map(|on| on.word().to_string()),
        }
    }
}

impl From<Settings> for line::Settings {
    fn from(asked: Settings) -> Self {
        Self::from(&asked)
    }
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        line::Settings::from(self).fmt(f)
    }
}

impl Cmd for Settings {
    const TAG: &'static str = "settings";
    const ROWS: &'static [Spec] = ROWS_SETTINGS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&TabRef> {
        None
    }
    /// Asking is reading. Whether this app replaces itself is as far-reaching as installing, so it's the same level.
    fn access(&self) -> Access {
        match self.auto_update {
            Some(_) => Access::At(Level::Full),
            None => row_access(Self::TAG),
        }
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "auto_update": self.auto_update.map(OnOff::word) })
    }
    /// Only the app has an updater, or settings.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(update_needs_app())
    }
    fn human(data: &Value) -> String {
        text(&data["words"]).to_string()
    }
}

impl super::Updates {
    /// Asking how things stand doesn't change what an update carries across, and is answered while one hands off; checking, installing and setting are waited for.
    pub fn only_asks(&self) -> bool {
        match self {
            Self::UpdateStatus(command) => command.only_asks(),
            Self::UpdateCheck(command) => command.only_asks(),
            Self::UpdateInstall(command) => command.only_asks(),
            Self::UpdateDevBuilds(command) => command.only_asks(),
            Self::Settings(command) => command.only_asks(),
        }
    }
}

impl OnlyAsks for UpdateStatus {
    fn only_asks(&self) -> bool {
        true
    }
}

impl OnlyAsks for UpdateCheck {
    fn only_asks(&self) -> bool {
        false
    }
}

impl OnlyAsks for UpdateInstall {
    fn only_asks(&self) -> bool {
        false
    }
}

impl OnlyAsks for UpdateDevBuilds {
    fn only_asks(&self) -> bool {
        self.set.is_none()
    }
}

impl OnlyAsks for Settings {
    fn only_asks(&self) -> bool {
        self.auto_update.is_none()
    }
}

/// Updates and settings are the app's. A session with no window has nothing to replace, and nothing to set.
pub fn update_needs_app() -> AgentError {
    AgentError::new(
        Code::NEEDS_APP,
        "Updates and settings belong to the app, and this session has no window.",
        exit::FAILED,
    )
    .then("Open RectoPDF, and `recto` will talk to that.")
}

/// What `update status` says to a person.
fn status_words(data: &Value) -> String {
    let mut out = text(&data["words"]).to_string();
    let _ = write!(out, "\nAutomatic updates: {}.", if data["auto_update"] == true { "on" } else { "off" });
    let _ = write!(out, "\nDevelopment builds: {}.", if data["dev_builds"] == true { "allowed" } else { "off" });
    if let Some(waiting) = data["build_waiting"].as_str() {
        let _ = write!(out, "\nWaiting to go in: development build {waiting}.");
    }
    if let Some(kept) = data["kept"].as_str() {
        let _ = write!(out, "\nKept to go back to: {kept}.");
    }
    if let (Some(from), Some(to), Some(gap)) = (
        data["last_update"]["from"].as_str(),
        data["last_update"]["to"].as_str(),
        data["last_update"]["gap_ms"].as_u64(),
    ) {
        let _ = write!(out, "\nLast update: {from} to {to}, {gap} ms from quit to first frame.");
    }
    out
}

pub(crate) const ROWS_UPDATE_STATUS: &[Spec] = &[Spec {
    name: "update status",
    tag: "update_status",
    group: GROUP_APP,
    summary: "What version is running, what's on offer, how things stand, and how the last update went. Works while an update is under way. Only in the app.",
    data: "`{ version, build, status, words, menu: { title, enabled }, offered, auto_update, second_copy, failed_versions, last_check_ms, last_update: { from, to, gap_ms, at_ms }, next_wake_in_ms, install_look_in_ms, kept, dev_builds, back_to_release, build_waiting, log, handover_dir }`. `version` is `X.Y.Z.N` on a development build. `build` is `release` for a release and a development build alike, `development` for a local build from `app/build.sh`, and `test` for a test's. `status` is one of `idle`, `checking`, `up_to_date`, `no_releases`, `available`, `available_not_installed`, `check_failed`, `preparing`, `waiting`, `installing`, `failed`, `updated`, `went_back`, `build_ready`.",
    examples: &["recto update status"],
    ..BASE
}];

pub(crate) const ROWS_UPDATE_CHECK: &[Spec] = &[Spec {
    name: "update check",
    tag: "update_check",
    group: GROUP_APP,
    access: Access::At(Level::Control),
    summary: "Looks for a new version now and waits for the answer, by what Check for Updates… does. It sends GitHub an ordinary download request with no document data. A local build (`app/build.sh`, `build: development`) checks too, and won't replace itself. Only in the app.",
    data: "What `update status` gives.",
    examples: &["recto update check"],
    client_wait_s: 30,
    ..BASE
}];

pub(crate) const ROWS_UPDATE_INSTALL: &[Spec] = &[Spec {
    name: "update install",
    tag: "update_install",
    group: GROUP_APP,
    access: Access::At(Level::Full),
    summary: "Installs what's on offer, by what Update to X.Y.Z… does: downloads and verifies with the app in use, then waits for anything in flight, hands off and reopens. It answers as soon as it's started, as the app it's talking to is about to quit: wait for the socket, then `update status`. Unsaved edits and all the app's state come across. With a zip, installs that development build (`RectoPDF-X.Y.Z.N.zip`, from `tools/dev-build.sh`) the same way. **`recto` itself opens the zip** (a link, a device or a file over 512 MiB is refused; if macOS is asking about a removable volume it says \"Waiting for macOS to allow access to …\" after 5 seconds and gives up after 2 minutes, Ctrl-C ends it cleanly) and hands the app a copy in the app's own folder, a clone on the same volume, so the app never opens a path an agent names; a client that skips `recto` and names another path gets `not_handed_over`. It is checked once the app has the copy: an `X.Y.Z.N` version, this app's bundle id, and a whole signature naming this app, ad hoc as every dev build is. Only with Allow development builds on, and always at Full, as anyone can sign ad hoc. It waits until the app hasn't been used for 5 minutes, or goes at once with `--now`. **With no zip and no `--now` while a build already waits, nothing starts:** it answers with the build, what it waits for and how to go at once (`started: false`); the five idle minutes are the owner's rule. `--now` (with or without a zip), the menu's Update to X.Y.Z… and Settings' Update now button are the one call that goes. Only in the app.",
    args: &[
        Arg {
            key: "zip",
            flag: None,
            kind: Kind::Path,
            required: false,
            help: "A development build's zip, `RectoPDF-X.Y.Z.N.zip`. Left out, what's on offer from the releases.",
        },
        Arg {
            key: "now",
            flag: Some("--now"),
            kind: Kind::Switch,
            required: false,
            help: "A development build goes at once, waiting only for anything in flight, not for 5 minutes without use. With no zip, it installs the build that waits.",
        },
    ],
    data: "`{ started, version, words }`, and for a build `waits_ms`, until it can go. `started` is false when a plain install found a build waiting and started nothing.",
    examples: &[
        "recto update install",
        "recto update install RectoPDF-0.1.0.3.zip",
        "recto update install RectoPDF-0.1.0.3.zip --now",
    ],
    client_wait_s: 120,
    ..BASE
}];

pub(crate) const ROWS_UPDATE_DEV_BUILDS: &[Spec] = &[Spec {
    name: "update dev-builds",
    tag: "update_dev_builds",
    group: GROUP_APP,
    summary: "Allow development builds, as Settings has it: off unless the owner's turned it on. On, `update install <zip>` takes a development build. Off, the app installs only releases, and on a development build goes back to the latest release by itself, once there is one and the app hasn't been touched for 30 minutes. Setting it needs Full. Only in the app.",
    args: &[Arg {
        key: "set",
        flag: None,
        kind: Kind::Choice(&["on", "off"]),
        required: false,
        help: "`on` or `off`. Left out, says how it is.",
    }],
    data: "`{ dev_builds, back_to_release, words }`.",
    examples: &["recto update dev-builds", "recto update dev-builds on"],
    ..BASE
}];

pub(crate) const ROWS_SETTINGS: &[Spec] = &[Spec {
    name: "settings",
    tag: "settings",
    group: GROUP_APP,
    summary: "The app's settings, as the Settings window shows them. With a value, sets it, which needs Full, as whether the app replaces itself is as far-reaching as installing. Only in the app.",
    args: &[Arg {
        key: "auto_update",
        flag: Some("--auto-update"),
        kind: Kind::Choice(&["on", "off"]),
        required: false,
        help: "`on` installs a new version by itself once the user hasn't touched the app for 30 minutes. `off` leaves it to the menu. The daily check runs either way.",
    }],
    data: "`{ auto_update, words }`.",
    examples: &["recto settings", "recto settings --auto-update off"],
    ..BASE
}];
