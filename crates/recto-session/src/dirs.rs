//! Folders: the folder of every tab's file, each kept once however many tabs are in it, so a tab holds a number for it rather than a path.

use std::collections::HashMap;
use std::path::Path;

/// The folders tabs' files are in, by number. A folder no tab is in any more is let go, and its number goes to the next new one, so the table is only as big as the folders in use at once, however many a long session has been through. A number in use never changes.
#[derive(Debug, Default)]
pub(crate) struct Dirs {
    /// By number. `None` is a number let go of, to be given again.
    dirs: Vec<Option<Box<Path>>>,
    dir_ids: HashMap<Box<Path>, u32>,
    /// Numbers let go of.
    free: Vec<u32>,
}

impl Dirs {
    /// A folder's number, given it the first time it's asked for.
    pub(crate) fn intern(&mut self, dir: &Path) -> u32 {
        if let Some(&id) = self.dir_ids.get(dir) {
            return id;
        }
        let id = match self.free.pop() {
            Some(id) => {
                if let Some(slot) = self.dirs.get_mut(id as usize) {
                    *slot = Some(dir.into());
                }
                id
            }
            None => {
                let id = u32::try_from(self.dirs.len()).unwrap_or(u32::MAX);
                self.dirs.push(Some(dir.into()));
                id
            }
        };
        self.dir_ids.insert(dir.into(), id);
        id
    }

    /// No tab is in this folder any more: it goes, and its number is given again.
    pub(crate) fn let_go(&mut self, id: u32) {
        if let Some(dir) = self.dirs.get_mut(id as usize).and_then(Option::take) {
            self.dir_ids.remove(&dir);
            self.free.push(id);
        }
    }

    /// How many folders are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.dir_ids.len()
    }

    /// The folder with this number.
    pub(crate) fn get(&self, id: u32) -> Option<&Path> {
        self.dirs.get(id as usize)?.as_deref()
    }

    /// The number of a folder that has one.
    pub(crate) fn id_of(&self, dir: &Path) -> Option<u32> {
        self.dir_ids.get(dir).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a folder let go of is still found by its path or number, or a number in use changes when another is let go of or given again.
    #[test]
    fn a_folder_let_go_of_gives_its_number_to_the_next_and_no_other_changes() {
        let mut dirs = Dirs::default();
        let a = dirs.intern(Path::new("/a"));
        let b = dirs.intern(Path::new("/b"));
        dirs.let_go(a);
        assert_eq!((dirs.get(a), dirs.id_of(Path::new("/a"))), (None, None));
        let c = dirs.intern(Path::new("/c"));
        assert_eq!(c, a, "the number's given again");
        assert_eq!(dirs.get(b), Some(Path::new("/b")));
        assert_eq!(dirs.get(c), Some(Path::new("/c")));
        assert_eq!(dirs.intern(Path::new("/b")), b);
        dirs.let_go(a);
        dirs.let_go(a);
        assert_eq!(dirs.len(), 1, "letting go twice frees one number");
        assert_ne!(dirs.intern(Path::new("/d")), dirs.intern(Path::new("/e")));
    }
}
