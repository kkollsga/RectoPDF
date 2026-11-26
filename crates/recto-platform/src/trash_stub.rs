//! No Trash here: nothing is ever moved, and the answer to "has this volume one" is no.

use std::path::{Path, PathBuf};

/// Why an item was not moved to the Trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    BadPath,
    Refused(isize),
    MovedUnknown,
}

pub fn has_trash_for(_path: &Path) -> bool {
    false
}

pub fn move_to_trash(_path: &Path) -> Result<PathBuf, Failure> {
    Err(Failure::Unavailable)
}
