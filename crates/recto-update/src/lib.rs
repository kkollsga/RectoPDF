//! **Hands-off updates from GitHub releases** (SPEC decision 24). Rust decides what's on offer, when to check and when to install, and every word; the host sets one timer and draws. Nothing here runs at rest: a timer a day from the host, and a thread only while a check or an install is under way.
//!
//! - [`rules`]: what a release must be to be offered, the schedule, and the 30-minute rule. Pure.
//! - [`source`]: which server, and how far this build may go. A release build reads no override, whatever the environment says.
//! - [`saved`]: what's remembered, in `updates.json` in the state folder.
//! - [`fetch`]: an ordinary download by the system's `curl`, capped and with a limit on time. No document data goes anywhere.
//! - [`stage`]: everything slow, done before the app quits: download, checksum, unpack, signature, team and Gatekeeper.
//! - [`plan`]: what the app hands the installer, and what the installer leaves behind.
//! - [`install`]: the `recto-updater` helper: waits for the old process, swaps the app in one step, and goes back if the new one doesn't start.
//! - [`updater`]: all that together, for the app, with development builds in [`updater::builds`].
//! - [`version`]: a release's `X.Y.Z` and a development build's `X.Y.Z.N`, and their order. Pure.

pub mod fetch;
pub mod handover;
pub mod install;
pub mod plan;
mod process;
pub mod rules;
pub mod saved;
pub mod source;
pub mod stage;
pub mod updater;
pub mod version;
pub mod zip;

/// The project on GitHub. A release build's one and only source.
pub const REPO: &str = "https://github.com/kkollsga/RectoPDF";
/// The Apple Developer team every release is signed by. `app/release-zip.sh` and `release.yml` read it from this line.
pub const TEAM_ID: &str = "6GND8Z2C7Z";
/// The app's bundle id. A build with another isn't this app.
pub const BUNDLE_ID: &str = "io.github.kkollsga.RectoPDF";
/// The renderer helper's identifier when it's signed ad hoc, as every development build's is (`app/render-helper.sh`).
pub const HELPER_ADHOC_ID: &str = "io.github.kkollsga.RectoPDF.Render.adhoc";

/// The user's own state folder, where the real app keeps `updates.json`: what `recto-control`'s `StatePaths` uses with no override, and a test holds the two to each other. Never a folder a plan or a file names.
#[must_use]
pub fn real_state_root() -> Option<std::path::PathBuf> {
    recto_platform::local::real_home().map(|home| home.join("Library").join("Application Support").join("RectoPDF").join("agent"))
}
pub const APP_NAME: &str = "RectoPDF";
/// The bundle's folder name.
pub const APP_BUNDLE: &str = "RectoPDF.app";
/// The app's executable in the bundle.
pub const APP_EXECUTABLE: &str = "Contents/MacOS/RectoPDF";
/// The installer in the bundle.
pub const HELPER: &str = "Contents/MacOS/recto-updater";

/// Whether this was built as a release: `RECTO_RELEASE` is set when the crate is compiled, by `app/release-zip.sh`, and it can't be turned on later. Nothing at run time can make a release build look elsewhere.
pub const RELEASE_BUILD: bool = recto_platform::RELEASE_BUILD;

/// A test's broken release, said by a file in its bundle's `Resources`: how the real app is made to misbehave as a new version might after its first frame, so going back can be tested on it. Never in a release build, whatever's in the bundle.
#[must_use]
pub fn test_breakage(app: &std::path::Path, what: TestBreakage) -> bool {
    !RELEASE_BUILD && app.join("Contents/Resources").join(what.file()).exists()
}

/// What a test's broken release does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestBreakage {
    /// Starts empty, as if it couldn't make sense of the record.
    MisreadsTheRecord,
    /// Ends a moment after its first frame, every launch.
    EndsAfterFirstFrame,
    /// As it ends, leaves the record so the version before can't read it.
    DamagesTheRecordAsItEnds,
}

impl TestBreakage {
    #[must_use]
    pub fn file(self) -> &'static str {
        match self {
            Self::MisreadsTheRecord => "test-misreads-the-record",
            Self::EndsAfterFirstFrame => "test-ends-after-first-frame",
            Self::DamagesTheRecordAsItEnds => "test-damages-the-record-as-it-ends",
        }
    }
}

/// Now, on the wall clock.
#[must_use]
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}
