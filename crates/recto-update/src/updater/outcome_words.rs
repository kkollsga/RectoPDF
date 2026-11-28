//! **What an installer's outcome comes to in words:** why an install didn't happen, which of those left the app reopened, and what's said once a version has
//! gone back. Moved here unchanged from `updater.rs`, for its size, with `removal_pending`'s arm.

use crate::APP_NAME;

/// What's said once a version that didn't work has gone back, by why.
pub(super) fn went_back_words(why: &str, version: &str, current: &str) -> String {
    match why {
        "kept_ending" => {
            format!("{APP_NAME} {version} kept quitting unexpectedly, so {current} came back, with your tabs and edits as {version} last had them.")
        }
        "misread" => format!("{APP_NAME} {version} didn't read your last session properly, so {current} came back, just as you left it."),
        _ => format!("{APP_NAME} {version} didn't start, so {current} came back, just as you left it."),
    }
}

/// Whether the installer opened the app again after not installing, for this reason. For the others the old app was never quit, or someone else opened it.
pub(super) fn installer_reopened(code: &str) -> bool {
    matches!(
        code,
        "swap_failed" | "not_permitted" | "changed_since_checked" | "new_app_missing" | "removal_pending"
    )
}

/// What each of the installer's reasons comes to, and whether it's the sort of thing that'll be the same tomorrow.
pub(super) fn why_not_installed(code: &str) -> (&'static str, bool) {
    match code {
        "swap_failed" => ("the app couldn't be replaced where it is", true),
        // Not lasting: once it's allowed, it works.
        "not_permitted" => (
            "macOS didn't allow RectoPDF to change itself. Allow it in System Settings › Privacy & Security › App Management, then update again",
            false,
        ),
        "changed_since_checked" => ("what was downloaded changed after it was checked", true),
        "old_app_stayed" => ("the old app didn't quit in time", false),
        "app_reopened" => ("the app was opened again while it was going", false),
        "another_installer" => ("another update was still at work", false),
        "removal_pending" => ("a removal of data from a document was left unfinished, and is finished first", false),
        _ => ("it couldn't be put in place", false),
    }
}
