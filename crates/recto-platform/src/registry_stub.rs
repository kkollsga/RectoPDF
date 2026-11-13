//! Launch Services is the Mac's: elsewhere there's nothing registered and nothing to forget.

use std::path::{Path, PathBuf};

pub const ID_PREFIX: &str = "io.github.kkollsga.RectoPDF";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub path: PathBuf,
    pub identifier: String,
}

pub fn forget(_bundle: &Path) {}

pub fn forget_until_gone(_bundle: &Path, _limit: std::time::Duration) -> bool {
    true
}

pub fn forget_soon(_bundle: &Path) {}

pub fn forget_all_under(_folder: &Path) {}

#[must_use]
pub fn registered() -> Vec<Registered> {
    Vec::new()
}

#[must_use]
pub fn strays(registered: &[Registered], allowed: &[&Path]) -> Vec<Registered> {
    registered
        .iter()
        .filter(|copy| !allowed.iter().any(|allowed| copy.path.starts_with(allowed)))
        .cloned()
        .collect()
}

pub const DESCRIBED: &str = "registered in this checkout";

#[must_use]
pub fn describe(_all: &[Registered], _checkout: &Path, _temp: &Path) -> String {
    format!("{DESCRIBED}: nothing is registered here\n")
}
