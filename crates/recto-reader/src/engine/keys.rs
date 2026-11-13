//! **Passwords for protected files, while they're open:** the one keyring every page view's workers open protected files with.

use super::lock;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// **Passwords for protected files, while they're open,** by which file each is for: its device and inode (`recto_platform::secret::file_identity`), not its path, so a file moved while it's open keeps its password, and another file put at its path doesn't get it. In memory only: never written, logged or journaled, and each is overwritten when its last holder lets go (`recto_core::Password`). The app forgets one when no tab has its file open. At most [`MAX_KEYS`].
#[derive(Default)]
pub struct Keyring {
    keys: Mutex<Vec<(FileIdentity, Arc<recto_core::Password>)>>,
}

/// A file's device and inode: which file it is, wherever it's moved to on its volume.
pub type FileIdentity = (u64, u64);

/// Passwords held at once: one per protected file open, and more open protected files than this is past anything a person types passwords for. Past it, the oldest goes, and its file asks again.
pub const MAX_KEYS: usize = 64;

impl std::fmt::Debug for Keyring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Keyring({} held)", lock(&self.keys).len())
    }
}

impl Keyring {
    /// Keeps a password for the file with this identity, in place of any it had.
    pub fn set(&self, identity: (u64, u64), password: Arc<recto_core::Password>) {
        let mut keys = lock(&self.keys);
        keys.retain(|(known, _)| *known != identity);
        if keys.len() >= MAX_KEYS {
            keys.remove(0);
        }
        keys.push((identity, password));
    }

    /// The password for the file with this identity, if one was typed.
    pub fn get(&self, identity: (u64, u64)) -> Option<Arc<recto_core::Password>> {
        lock(&self.keys)
            .iter()
            .find(|(known, _)| *known == identity)
            .map(|(_, password)| Arc::clone(password))
    }

    /// The password for the file at this path now, if one was typed for it.
    pub fn for_path(&self, path: &Path) -> Option<Arc<recto_core::Password>> {
        if self.is_empty() {
            return None;
        }
        self.get(recto_platform::secret::file_identity(path)?)
    }

    /// Keeps only the passwords for these files, the ones open in tabs: any other goes, and is overwritten.
    pub fn keep_only(&self, open: &[(u64, u64)]) {
        lock(&self.keys).retain(|(identity, _)| open.contains(identity));
    }

    pub fn len(&self) -> usize {
        lock(&self.keys).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
