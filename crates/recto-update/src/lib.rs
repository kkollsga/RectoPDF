//! **Hands-off updates from GitHub releases** (SPEC decision 24). Rust decides what's on offer, when to check and when to install, and every word; the host sets one timer and draws. Nothing here runs at rest: a timer a day from the host, and a thread only while a check or an install is under way.
//!
//! - [`rules`]: what a release must be to be offered, the schedule, and the 30-minute rule. Pure.
//! - [`source`]: which server, and how far this build may go. A release build reads no override, whatever the environment says.
//! - [`saved`]: what's remembered, in `updates.json` in the state folder.
//! - [`fetch`]: an ordinary download by the system's `curl`, capped and with a limit on time. No document data goes anywhere.
//! - [`stage`]: everything slow, done before the app quits: download, checksum, unpack, signature, team and Gatekeeper.
//! - [`plan`]: what the app hands the installer, and what the installer leaves behind.
//! - [`install`]: the `recto-updater` helper: waits for the old process, swaps the app in one step, and goes back if the new one doesn't start.
//! - [`updater`]: all that together, for the app.

pub mod fetch;
pub mod install;
pub mod plan;
pub mod rules;
pub mod saved;
pub mod source;
pub mod stage;
pub mod updater;
pub mod zip;
mod process;

/// The project on GitHub. A release build's one and only source.
pub const REPO: &str = "https://github.com/kkollsga/RectoPDF";
/// The Apple Developer team every release is signed by. `app/release-zip.sh` and `release.yml` read it from this line.
pub const TEAM_ID: &str = "6GND8Z2C7Z";
/// The app's bundle id. A build with another isn't this app.
pub const BUNDLE_ID: &str = "io.github.kkollsga.RectoPDF";
pub const APP_NAME: &str = "RectoPDF";
/// The bundle's folder name.
pub const APP_BUNDLE: &str = "RectoPDF.app";
/// The app's executable in the bundle.
pub const APP_EXECUTABLE: &str = "Contents/MacOS/RectoPDF";
/// The installer in the bundle.
pub const HELPER: &str = "Contents/MacOS/recto-updater";

/// Whether this was built as a release: `RECTO_RELEASE` is set when the crate is compiled, by `app/release-zip.sh`, and it can't be turned on later. Nothing at run time can make a release build look elsewhere.
pub const RELEASE_BUILD: bool = option_env!("RECTO_RELEASE").is_some();

/// Now, on the wall clock.
#[must_use]
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}
