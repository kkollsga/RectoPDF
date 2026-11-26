//! What the lineage says about a document's file versions and paths, as the app's own stores name them. Used by every survey that places a
//! record by a version and a path (the journals and the session records), so the rule is written once (`docs/design/removal.md` §4, §10).

use recto_core::removal::{Lineage, Side, Sides};
use recto_platform::FileStamp;
use recto_session::record::journal_file_name;
use std::collections::HashMap;
use std::path::Path;

pub(super) struct Maps {
    /// By the 16-digit key a name carries (from inode, size and mtime: the device is a mount number and the journal doesn't hold the volume).
    pub(super) versions: HashMap<String, Sides>,
    /// By the path's bytes.
    paths: HashMap<Vec<u8>, Sides>,
}

impl Maps {
    pub(super) fn of(lineage: &Lineage<'_>) -> Self {
        let mut versions: HashMap<String, Sides> = HashMap::new();
        let current = lineage.current();
        let all = std::iter::once((current, Side::Target)).chain(lineage.versions());
        for (version, side) in all.filter(|(version, _)| version.inode != 0) {
            let sides = versions.entry(key_of(version.inode, version.size, version.mtime_ns)).or_default();
            *sides = sides.with(side);
        }
        let mut paths: HashMap<Vec<u8>, Sides> = HashMap::new();
        for (path, side) in lineage.paths() {
            let sides = paths.entry(path.as_os_str().as_encoded_bytes().to_vec()).or_default();
            *sides = sides.with(side);
        }
        Self { versions, paths }
    }

    /// The sides of the version a header names, if its inode is real and the lineage saw it.
    pub(super) fn version(&self, inode: u64, size: u64, mtime_ns: i64) -> Option<Sides> {
        (inode != 0).then(|| self.versions.get(&key_of(inode, size, mtime_ns)).copied()).flatten()
    }

    /// The sides of a file named by its version and its path: both must be found (a path alone never joins, and nor does a same-version copy
    /// elsewhere), and their sides are joined.
    pub(super) fn place(&self, path: &[u8], inode: u64, size: u64, mtime_ns: i64) -> Option<Sides> {
        let version = self.version(inode, size, mtime_ns)?;
        let at = self.paths.get(path).copied()?;
        Some(version.union(at))
    }
}

/// The key a journal's name carries for a file version, by the one function that names journals.
pub(super) fn key_of(inode: u64, size: u64, mtime_ns: i64) -> String {
    let stamp = FileStamp {
        size,
        mtime_ns,
        device: 0,
        inode,
    };
    let name = journal_file_name(Some(stamp), Path::new(""), None);
    name.strip_prefix("journal-")
        .and_then(|rest| rest.strip_suffix(".log"))
        .unwrap_or_default()
        .to_string()
}
