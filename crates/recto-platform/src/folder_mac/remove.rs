//! Taking a pinned folder away whole, by descriptor (`docs/design/removal.md` §10, D5 "Delete the copies whole").
//!
//! The folder to take away is named by its parent's descriptor and the name in it, and must still be the thing that was pinned: opened with
//! `O_DIRECTORY | O_NOFOLLOW`, then its device and inode compared. Each folder below it is opened the same way from its parent's descriptor, so no path is
//! resolved again once the parent is open. A link is unlinked, never followed, whatever it leads to. An entry that was not pinned stops the walk before
//! anything is taken out of that folder, and an entry that is not what was pinned stops it before that entry goes. A pinned entry that is already gone is done.
//! Folders more than [`MAX_DEPTH`] below the first are refused: the pin can't hold them.

use super::{Folder, Opened};
use crate::folder_tree::{Kind, MAX_DEPTH, Pinned, Tree};
use std::ffi::{OsStr, OsString};
use std::io;

/// What took the walk to a stop. Whatever was taken out before stays out.
#[derive(Debug)]
pub enum Failure {
    /// This entry is not what was pinned (another thing, another kind, or a folder that is now a link).
    Changed(OsString),
    /// This entry was not there when the folder was pinned.
    Unpinned(OsString),
    /// The pin goes deeper than [`MAX_DEPTH`].
    TooDeep,
    /// The gate said stop, before this entry.
    Stopped(OsString),
    /// This entry was taken out but the folder could not be made durable, so a power loss may bring it back.
    NotDurable(OsString),
    /// A system call failed on this entry.
    Io(OsString, io::Error),
}

/// How much one walk took out.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    /// Entries and folders unlinked by this walk.
    pub unlinked: usize,
    /// Pinned names that were already gone.
    pub already_gone: usize,
}

/// The order and the stop of a walk.
pub struct Walk<'a> {
    /// Lower goes first among the entries of one folder (the depth is that of the entry's folder: 0 for the folder taken away). Equal ranks go by name.
    /// What has rank 0 is made durable (the folder is flushed) before anything after it is touched, so a stop after it never finds it back.
    pub rank: &'a dyn Fn(usize, &OsStr, Kind) -> u8,
    /// Asked before every entry and folder is unlinked, with its name; `false` stops the walk there.
    pub gate: &'a mut dyn FnMut(&OsStr) -> bool,
}

/// Takes the folder `name` of `parent` away with everything in it, if it is still what `tree` says and everything in it is pinned.
pub fn remove_tree_in(parent: &Folder, name: &OsStr, tree: &Tree, walk: &mut Walk<'_>) -> Result<Removed, Failure> {
    let mut removed = Removed::default();
    remove_folder(parent, name, tree, 0, walk, &mut removed)?;
    Ok(removed)
}

fn fail(name: &OsStr) -> impl Fn(io::Error) -> Failure + '_ {
    move |error| Failure::Io(name.to_owned(), error)
}

fn remove_folder(parent: &Folder, name: &OsStr, tree: &Tree, depth: usize, walk: &mut Walk<'_>, removed: &mut Removed) -> Result<(), Failure> {
    if depth > MAX_DEPTH {
        return Err(Failure::TooDeep);
    }
    let folder = match parent.open_in(name) {
        Opened::Missing => {
            removed.already_gone += 1;
            return Ok(());
        }
        Opened::Other => return Err(Failure::Changed(name.to_owned())),
        Opened::Folder(folder) => folder,
    };
    if folder.identity().map_err(fail(name))? != tree.id {
        return Err(Failure::Changed(name.to_owned()));
    }
    // Everything it holds is looked at first: nothing is taken out of a folder that holds what was not pinned.
    let mut todo: Vec<&Pinned> = Vec::new();
    let mut listed = 0usize;
    for entry in folder.entries().map_err(fail(name))? {
        let entry = entry.map_err(fail(name))?;
        listed += 1;
        let Ok(at) = tree.entries.binary_search_by(|p| p.name.as_os_str().cmp(&entry.name)) else {
            return Err(Failure::Unpinned(entry.name));
        };
        let pinned = &tree.entries[at];
        if pinned.id != entry.identity() {
            return Err(Failure::Changed(entry.name));
        }
        todo.push(pinned);
    }
    removed.already_gone += tree.entries.len() - listed.min(tree.entries.len());
    todo.sort_by(|a, b| {
        (walk.rank)(depth, &a.name, a.id.kind)
            .cmp(&(walk.rank)(depth, &b.name, b.id.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    for pinned in todo {
        if !(walk.gate)(&pinned.name) {
            return Err(Failure::Stopped(pinned.name.clone()));
        }
        match (pinned.id.kind, &pinned.below) {
            (Kind::Folder, Some(below)) => remove_folder(&folder, &pinned.name, below, depth + 1, walk, removed)?,
            (Kind::Folder, None) => return Err(Failure::Changed(pinned.name.clone())),
            _ => {
                // The last look before the unlink: whatever is under the name now is what was pinned, or it stays.
                match folder.look(&pinned.name) {
                    Ok(now) if now.identity() == pinned.id => {}
                    Ok(_) => return Err(Failure::Changed(pinned.name.clone())),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        removed.already_gone += 1;
                        continue;
                    }
                    Err(error) => return Err(Failure::Io(pinned.name.clone(), error)),
                }
                match folder.unlink_in(&pinned.name, false) {
                    Ok(()) => removed.unlinked += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => removed.already_gone += 1,
                    Err(error) => return Err(Failure::Io(pinned.name.clone(), error)),
                }
                if (walk.rank)(depth, &pinned.name, pinned.id.kind) == 0 && folder.sync().is_err() {
                    return Err(Failure::NotDurable(pinned.name.clone()));
                }
            }
        }
    }
    if depth == 0 && !(walk.gate)(name) {
        return Err(Failure::Stopped(name.to_owned()));
    }
    // The last look before the folder goes by its name: it is the one that was opened, and not another put in its place since.
    match parent.look(name) {
        Ok(now) if now.identity() == tree.id => {}
        Ok(_) => return Err(Failure::Changed(name.to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            removed.already_gone += 1;
            return Ok(());
        }
        Err(error) => return Err(Failure::Io(name.to_owned(), error)),
    }
    match parent.unlink_in(name, true) {
        Ok(()) => removed.unlinked += 1,
        Err(error) if error.kind() == io::ErrorKind::NotFound => removed.already_gone += 1,
        // Something was put in since it was looked at.
        Err(error) if error.raw_os_error() == Some(libc::ENOTEMPTY) => return Err(Failure::Unpinned(name.to_owned())),
        Err(error) => return Err(Failure::Io(name.to_owned(), error)),
    }
    Ok(())
}
