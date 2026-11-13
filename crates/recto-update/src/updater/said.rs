//! **What a refused install says**, in Rust's words with the next step, for `recto update install`, the menu's alert and Settings. Moved here from the bridge (R1.2): wording is the feature's, the bridge only forwards.

/// What a command's asked to install came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallRefused {
    NotInThisBuild,
    NothingOnOffer,
    Underway,
    /// Another copy of the app has the state, and it's the one that can.
    AnotherCopy,
    /// A development build handed over, and *Allow development builds* is off.
    DevBuildsOff,
    /// What was handed over isn't a `RectoPDF-<version>.zip`.
    NotABuild,
    /// It's the version that's running.
    SameVersion,
    /// This copy is signed by the team, and takes no build signed ad hoc: the first goes in by `tools/dev-build.sh --first-install --replace-release`.
    TeamSignedCopy,
    /// It was checked, and it isn't one to install: the code and our own words.
    Refused {
        code: &'static str,
        why: String,
    },
    /// The app hasn't told the updater what it is yet ([`super::Updater::launched`]): a launch that didn't get the record's lock holds that back until whether it
    /// keeps the record has settled, and nothing about an update can begin meanwhile, a check included (`recto-app`'s `update_command` refuses so).
    Starting,
}

/// How a refusal is classed, for the exit code the command ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaidKind {
    Usage,
    Busy,
    Failed,
}

/// A refusal in words: its code, what happened and what to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub code: &'static str,
    pub words: String,
    pub next: String,
    pub kind: SaidKind,
}

impl InstallRefused {
    /// Its words.
    #[must_use]
    pub fn said(&self) -> Said {
        let (code, words, next, kind): (&'static str, String, &str, SaidKind) = match self {
            Self::NotInThisBuild => (
                "development_build",
                "A local build doesn't replace itself.".into(),
                "`recto update check` still says whether there's a new version. Install the app with `tools/dev-build.sh` to take builds.",
                SaidKind::Failed,
            ),
            Self::NothingOnOffer => (
                "nothing_on_offer",
                "There's no new version on offer.".into(),
                "`recto update check` looks for one.",
                SaidKind::Failed,
            ),
            Self::Underway => (
                "update_under_way",
                "An update or a check is already under way.".into(),
                "`recto update status` says how it's going.",
                SaidKind::Busy,
            ),
            Self::AnotherCopy => (
                "another_copy",
                "Another copy of RectoPDF is keeping the state, and it's the one that can update.".into(),
                "Quit this copy, and use that one.",
                SaidKind::Failed,
            ),
            Self::DevBuildsOff => (
                "dev_builds_off",
                "Development builds are off, so only releases are installed.".into(),
                "The owner turns on Settings › Allow development builds.",
                SaidKind::Failed,
            ),
            Self::NotABuild => (
                "not_a_build",
                "That isn't a RectoPDF-X.Y.Z.N.zip.".into(),
                "`tools/dev-build.sh` makes one and prints its path.",
                SaidKind::Usage,
            ),
            Self::SameVersion => (
                "same_version",
                "That's the version that's running.".into(),
                "`tools/dev-build.sh` numbers the next one.",
                SaidKind::Failed,
            ),
            Self::TeamSignedCopy => (
                "release_copy",
                "This copy is a release signed by RectoPDF's developer, and never takes a development build, which is signed ad hoc.".into(),
                "The coordinator puts the first one in with `tools/dev-build.sh --first-install --replace-release`, which keeps this copy aside.",
                SaidKind::Failed,
            ),
            Self::Starting => (
                "starting",
                "RectoPDF is still starting, so it can't look for or install an update yet.".into(),
                "Try again in a few seconds.",
                SaidKind::Busy,
            ),
            Self::Refused { code, why } => (
                code,
                format!("That build wasn't installed: {why}."),
                "`recto update status` has the log's path.",
                SaidKind::Failed,
            ),
        };
        Said {
            code,
            words,
            next: next.to_owned(),
            kind,
        }
    }
}
