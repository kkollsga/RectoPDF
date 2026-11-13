//! Folders: the folder of every tab's file, each kept once however many tabs are in it, so a tab holds a number for it rather than a path.

use std::collections::HashMap;
use std::path::Path;

/// The folders tabs' files are in, by number.
#[derive(Debug, Default)]
pub(crate) struct Dirs {
    dirs: Vec<Box<Path>>,
    dir_ids: HashMap<Box<Path>, u32>,
}

impl Dirs {
    /// A folder's number, given it the first time it's asked for.
    pub(crate) fn intern(&mut self, dir: &Path) -> u32 {
        if let Some(&id) = self.dir_ids.get(dir) {
            return id;
        }
        let id = u32::try_from(self.dirs.len()).unwrap_or(u32::MAX);
        self.dirs.push(dir.into());
        self.dir_ids.insert(dir.into(), id);
        id
    }

    /// The folder with this number.
    pub(crate) fn get(&self, id: u32) -> Option<&Path> {
        self.dirs.get(id as usize).map(|dir| &**dir)
    }

    /// The number of a folder that has one.
    pub(crate) fn id_of(&self, dir: &Path) -> Option<u32> {
        self.dir_ids.get(dir).copied()
    }
}
