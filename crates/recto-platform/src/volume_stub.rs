//! No volume identity off the Mac yet: every file reads as on a volume that can't be told from another, so nothing joins by version.

use std::path::Path;

/// Always `None` here.
pub fn volume_identity(_path: &Path) -> Option<[u8; 16]> {
    None
}
